use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::Result;
use jiff::Timestamp;

use crate::service::{self, Kind, Line};

const HOURS: usize = 24;
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// One numeric fact over a span: its last, lowest and highest value, and the mean of
/// each step, oldest first; `None` for a step with no check.
pub(crate) struct Series {
    pub last: f64,
    pub min: f64,
    pub max: f64,
    pub means: Vec<Option<f64>>,
}

/// The numeric fact values of `check` lines in the `steps` steps of `step` seconds that
/// end at `now`, per fact. Step `k` starts at `now - (steps - k) * step`.
pub(crate) fn series(lines: &[Line], now: i64, step: i64, steps: usize) -> BTreeMap<String, Series> {
    let mut sums: BTreeMap<String, (Series, Vec<f64>, Vec<u32>)> = BTreeMap::new();
    for l in lines {
        if l.kind != Kind::Check {
            continue;
        }
        let Some(values) = &l.facts else { continue };
        let Ok(t) = l.t.parse::<Timestamp>() else {
            continue;
        };
        let age = now - t.as_second();
        if !(0..step * steps as i64).contains(&age) {
            continue;
        }
        let k = steps - 1 - (age / step) as usize;
        for (name, v) in values {
            let Ok(v) = v.trim().parse::<f64>() else {
                continue;
            };
            if !v.is_finite() {
                continue;
            }
            let (s, sum, count) = sums.entry(name.clone()).or_insert_with(|| {
                (
                    Series {
                        last: v,
                        min: v,
                        max: v,
                        means: Vec::new(),
                    },
                    vec![0.0; steps],
                    vec![0; steps],
                )
            });
            s.last = v;
            s.min = s.min.min(v);
            s.max = s.max.max(v);
            sum[k] += v;
            count[k] += 1;
        }
    }
    sums.into_iter()
        .map(|(name, (mut s, sum, count))| {
            s.means = (0..steps)
                .map(|k| (count[k] > 0).then(|| sum[k] / f64::from(count[k])))
                .collect();
            (name, s)
        })
        .collect()
}

pub fn run(args: Args) -> Result<ExitCode> {
    super::known(&args.service)?;
    let now = Timestamp::now().as_second();
    let lines = service::journal(&args.service)?;
    let facts = series(&lines, now, 3600, HOURS);
    if facts.is_empty() {
        println!("no numeric facts in the last 24 h");
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<[String; 5]> = facts
        .iter()
        .map(|(name, s)| [name.clone(), num(s.last), num(s.min), num(s.max), spark(s)])
        .collect();
    let head = ["FACT", "LAST", "MIN", "MAX", "24 H, OLDEST FIRST"].map(String::from);
    let mut widths = [0usize; 4];
    for r in std::iter::once(&head).chain(&rows) {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    for r in std::iter::once(&head).chain(&rows) {
        println!(
            "{:w0$}  {:>w1$}  {:>w2$}  {:>w3$}  {}",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// One bar per hour's mean, scaled from the lowest mean to the highest; a blank for an
/// hour with no check.
fn spark(s: &Series) -> String {
    let lo = s
        .means
        .iter()
        .flatten()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let hi = s
        .means
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    s.means
        .iter()
        .map(|m| match m {
            None => ' ',
            Some(_) if hi <= lo => BARS[0],
            Some(m) => {
                let i = ((m - lo) / (hi - lo) * (BARS.len() - 1) as f64).round() as usize;
                BARS[i.min(BARS.len() - 1)]
            }
        })
        .collect()
}

/// Whole numbers without decimals, others with up to two.
fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
