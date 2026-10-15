use std::process::ExitCode;

use anyhow::Result;
use indexmap::IndexMap;
use serde::Serialize;

use crate::daemon::journal;
use crate::daemon::state::GoalState;
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
            "SERVICE", "GOALS", "FAILING", "ASKS", "REPAIR", "MODELS", "MONTH",
        ]
        .map(String::from),
    ];
    for r in services {
        let asks: Vec<String> = r.asks.iter().map(|n| format!("#{n}")).collect();
        rows.push([
            r.service,
            r.goals
                .map_or_else(|| "-".into(), |g| format!("{}/{}", g.held, g.total)),
            dash(r.failing.join(" ")),
            dash(asks.join(" ")),
            r.repair.unwrap_or_else(|| "-".into()),
            r.models,
            r.spend_month_usd.map_or_else(|| "-".into(), money),
        ]);
    }
    let mut widths = [0usize; 7];
    for r in &rows {
        for (w, c) in widths.iter_mut().zip(r) {
            *w = (*w).max(c.chars().count());
        }
    }
    for r in &rows {
        let mut line = String::new();
        for (i, (c, w)) in r.iter().zip(widths).enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            line.push_str(c);
            line.extend(std::iter::repeat_n(' ', w - c.chars().count()));
        }
        println!("{}", line.trim_end());
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
