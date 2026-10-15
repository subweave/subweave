use std::collections::{BTreeMap, BTreeSet};
use std::process::ExitCode;

use anyhow::Result;
use jiff::Timestamp;

use crate::service::{self, JobKind, Kind, Line, TASK};

const HOURS: usize = 24;
const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The ladder table covers this many weeks.
const WEEKS: usize = 4;
const WEEK: i64 = 7 * 86_400;

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
    print!("{}", facts_table(&lines, now));
    println!();
    print!("{}", ladder_table(&lines, now));
    Ok(ExitCode::SUCCESS)
}

fn facts_table(lines: &[Line], now: i64) -> String {
    let facts = series(lines, now, 3600, HOURS);
    if facts.is_empty() {
        return "no numeric facts in the last 24 h\n".into();
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
    let mut out = String::new();
    for r in std::iter::once(&head).chain(&rows) {
        out.push_str(&format!(
            "{:w0$}  {:>w1$}  {:>w2$}  {:>w3$}  {}\n",
            r[0],
            r[1],
            r[2],
            r[3],
            r[4],
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        ));
    }
    out
}

/// One week of the ladder: repairs each rung settled, escalations by who settled them,
/// and weave changes.
#[derive(Default)]
struct Week {
    code: u32,
    decider: u32,
    coder: u32,
    escalated: BTreeMap<String, u32>,
    changes: u32,
}

/// Who settled an escalation, from its answer's `by`: the part before `:` or `@`
/// (`cli`, `mcp`, `steery`, `telegram`, `dash`), an agent channel's name, or
/// `withdrawn` when its goal held again.
fn settler(by: &str) -> String {
    if by.starts_with("daemon:") {
        return "withdrawn".into();
    }
    match by.find([':', '@']) {
        Some(i) => by[..i].to_owned(),
        None => by.to_owned(),
    }
}

/// The last `WEEKS` weeks from the journal, oldest first. A repair counts in the week
/// its action passed, under the highest rung its episode used: the coder (a coder
/// conversation), the decider (a `decide` line), else code (learned steps or `RUN`).
/// A repair that escalated counts as an escalation, in the week of its answer.
fn ladder(lines: &[Line], now: i64) -> Vec<Week> {
    let mut weeks: Vec<Week> = (0..WEEKS).map(|_| Week::default()).collect();
    let week = |t: &str| -> Option<usize> {
        let age = now - t.parse::<Timestamp>().ok()?.as_second();
        (0..WEEK * WEEKS as i64)
            .contains(&age)
            .then(|| WEEKS - 1 - (age / WEEK) as usize)
    };
    let repairs: BTreeSet<&str> = lines
        .iter()
        .filter(|l| l.job == Some(JobKind::Repair))
        .filter_map(|l| l.ep.as_deref())
        .collect();
    let in_ep = |ep: &str, f: &dyn Fn(&Line) -> bool| {
        lines.iter().any(|l| l.ep.as_deref() == Some(ep) && f(l))
    };
    for l in lines {
        let Some(k) = week(&l.t) else { continue };
        let w = &mut weeks[k];
        match l.kind {
            Kind::Action if l.ok == Some(true) && l.action.as_deref() != Some(TASK) => {
                let Some(ep) = l.ep.as_deref().filter(|ep| repairs.contains(ep)) else {
                    continue;
                };
                if in_ep(ep, &|x| x.kind == Kind::Ask) {
                    continue;
                }
                if in_ep(ep, &|x| x.kind == Kind::Note && x.usage.is_some() && x.n.is_none()) {
                    w.coder += 1;
                } else if in_ep(ep, &|x| x.kind == Kind::Decide) {
                    w.decider += 1;
                } else {
                    w.code += 1;
                }
            }
            Kind::Answer if l.n.is_some() => {
                let who = settler(l.by.as_deref().unwrap_or("unknown"));
                *w.escalated.entry(who).or_default() += 1;
            }
            Kind::Note if l.text.as_deref().is_some_and(|t| t.starts_with("weave changed by ")) => {
                w.changes += 1;
            }
            _ => {}
        }
    }
    weeks
}

fn ladder_table(lines: &[Line], now: i64) -> String {
    let head = [
        "WEEK FROM",
        "CODE",
        "DECIDER",
        "CODER",
        "ESCALATED",
        "WEAVE CHANGES",
        "SETTLED BY",
    ]
    .map(String::from);
    let rows: Vec<[String; 7]> = ladder(lines, now)
        .into_iter()
        .enumerate()
        .map(|(k, w)| {
            let from = now - (WEEKS - k) as i64 * WEEK;
            let day = Timestamp::from_second(from)
                .map(|t| t.strftime("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let by: Vec<String> = w
                .escalated
                .iter()
                .map(|(who, n)| format!("{who} {n}"))
                .collect();
            [
                day,
                w.code.to_string(),
                w.decider.to_string(),
                w.coder.to_string(),
                w.escalated.values().sum::<u32>().to_string(),
                w.changes.to_string(),
                if by.is_empty() { "-".into() } else { by.join(", ") },
            ]
        })
        .collect();
    let mut widths = [0usize; 6];
    for r in std::iter::once(&head).chain(&rows) {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    let mut out = String::new();
    for r in std::iter::once(&head).chain(&rows) {
        out.push_str(&format!("{:w$}", r[0], w = widths[0]));
        for (c, w) in r[1..6].iter().zip(&widths[1..]) {
            out.push_str(&format!("  {c:>w$}"));
        }
        out.push_str(&format!("  {}\n", r[6]));
    }
    out
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
