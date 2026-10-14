use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::Result;
use jiff::Timestamp;

use crate::service::{self, Kind};

const HOURS: usize = 24;
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

#[derive(Default)]
struct Series {
    last: f64,
    min: f64,
    max: f64,
    sum: [f64; HOURS],
    count: [u32; HOURS],
}

pub fn run(args: Args) -> Result<ExitCode> {
    super::known(&args.service)?;
    let now = Timestamp::now().as_second();
    let mut facts: BTreeMap<String, Series> = BTreeMap::new();
    for l in service::journal(&args.service)? {
        if l.kind != Kind::Check {
            continue;
        }
        let Some(values) = &l.facts else { continue };
        let Ok(t) = l.t.parse::<Timestamp>() else {
            continue;
        };
        let age = now - t.as_second();
        if !(0..HOURS as i64 * 3600).contains(&age) {
            continue;
        }
        let hour = HOURS - 1 - (age / 3600) as usize;
        for (name, v) in values {
            let Ok(v) = v.trim().parse::<f64>() else {
                continue;
            };
            if !v.is_finite() {
                continue;
            }
            let s = facts.entry(name.clone()).or_insert_with(|| Series {
                min: v,
                max: v,
                ..Series::default()
            });
            s.last = v;
            s.min = s.min.min(v);
            s.max = s.max.max(v);
            s.sum[hour] += v;
            s.count[hour] += 1;
        }
    }
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
    let means: Vec<Option<f64>> = (0..HOURS)
        .map(|h| (s.count[h] > 0).then(|| s.sum[h] / f64::from(s.count[h])))
        .collect();
    let lo = means
        .iter()
        .flatten()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let hi = means
        .iter()
        .flatten()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    means
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
