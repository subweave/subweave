use std::process::ExitCode;

use anyhow::Result;
use indexmap::IndexMap;
use serde::Serialize;

use crate::daemon::journal;
use crate::daemon::state::{GoalState, LastDrill};
use crate::service;

/// One service as `ps` shows it, from its journal's fold.
#[derive(Serialize)]
pub(crate) struct Row {
    pub service: String,
    /// `None` until the daemon's loop journals the service's goals.
    pub goals: Option<Goals>,
    pub failing: Vec<String>,
    /// Open asks, by number.
    pub asks: Vec<u64>,
    /// `stopped`, `starting` before the loop journals the goals, or what runs now.
    pub repair: Option<String>,
    /// The decider, then the coder chain.
    pub models: String,
    /// `None` until the daemon's loop journals the service's goals.
    pub spend_month_usd: Option<f64>,
    pub stopped: bool,
    /// Each goal's state, in graph order.
    pub states: IndexMap<String, GoalState>,
    /// The drill that ran last, and how it went.
    pub drill: Option<LastDrill>,
}

#[derive(Serialize)]
pub(crate) struct Goals {
    pub held: usize,
    pub total: usize,
}

/// Every service the daemon keeps, by name.
pub(crate) fn rows() -> Result<Vec<Row>> {
    let now = crate::daemon::now();
    let month = &now[..7];
    let mut rows = Vec::new();
    for name in service::names()? {
        let spec = service::spec(&name)?;
        let st = journal::read(&name);
        let started = !st.goals.is_empty();
        rows.push(Row {
            goals: started.then(|| Goals {
                held: st.goals.values().filter(|g| g.holds()).count(),
                // An off goal does not apply on this box: it is neither held nor owed.
                total: st.goals.values().filter(|g| !g.off()).count(),
            }),
            failing: st
                .goals
                .iter()
                .filter(|(_, g)| g.fails())
                .map(|(n, _)| n.clone())
                .collect(),
            asks: st.open().map(|e| e.n).collect(),
            repair: if spec.stopped {
                Some("stopped".into())
            } else if !started {
                Some("starting".into())
            } else {
                st.running()
            },
            models: models(spec.decide, &spec.code),
            spend_month_usd: started.then(|| st.spend_in(month)),
            stopped: spec.stopped,
            drill: st.last_drill(),
            states: st.goals,
            service: name,
        });
    }
    Ok(rows)
}

fn models(mut decide: String, code: &[String]) -> String {
    if !code.is_empty() {
        decide.push_str(" / ");
        decide.push_str(&code.join(" then "));
    }
    decide
}

#[derive(clap::Args)]
pub struct Args {
    /// Print every service as JSON, with each goal's state
    #[arg(long)]
    pub json: bool,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let services = rows()?;
    if args.json {
        println!("{}", serde_json::to_string(&services)?);
        return Ok(ExitCode::SUCCESS);
    }
    if services.is_empty() {
        println!("no services here");
        return Ok(ExitCode::SUCCESS);
    }
    let mut rows = vec![
        [
            "SERVICE", "GOALS", "FAILING", "ASKS", "REPAIR", "MODELS", "MONTH", "DRILL",
        ]
        .map(String::from),
    ];
    // Under each service, its goals that wait on a goal or cannot be judged, and why; and
    // why its last drill failed.
    let mut notes: Vec<Vec<String>> = vec![Vec::new()];
    for r in services {
        let asks: Vec<String> = r.asks.iter().map(|n| format!("#{n}")).collect();
        let mut under: Vec<String> = r
            .states
            .iter()
            .filter_map(|(g, s)| match s {
                GoalState::Waiting { on } => Some(format!("  {g}: waits on {on}")),
                GoalState::Unknown { why: Some(why) } => Some(format!("  {g}: unknown: {why}")),
                _ => None,
            })
            .collect();
        if let Some((d, why)) = r.drill.as_ref().and_then(|d| Some((d, d.failure()?))) {
            under.push(format!("  drill {}: {why}", d.name));
        }
        notes.push(under);
        rows.push([
            r.service,
            r.goals
                .map_or_else(|| "-".into(), |g| format!("{}/{}", g.held, g.total)),
            dash(r.failing.join(" ")),
            dash(asks.join(" ")),
            r.repair.unwrap_or_else(|| "-".into()),
            r.models,
            r.spend_month_usd.map_or_else(|| "-".into(), money),
            r.drill.as_ref().map_or_else(|| "-".into(), LastDrill::short),
        ]);
    }
    let mut widths = [0usize; 8];
    for r in &rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    for (r, notes) in rows.iter().zip(&notes) {
        let mut line = String::new();
        for (i, (c, w)) in r.iter().zip(widths).enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            line.push_str(c);
            line.extend(std::iter::repeat_n(' ', w - c.chars().count()));
        }
        println!("{}", line.trim_end());
        for n in notes {
            println!("{n}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Cents above a dollar; below, the decider's fractions of a cent still show.
fn money(usd: f64) -> String {
    if usd >= 1.0 {
        format!("${usd:.2}")
    } else {
        format!("${usd:.4}")
    }
}

fn dash(s: String) -> String {
    if s.is_empty() { "-".into() } else { s }
}
