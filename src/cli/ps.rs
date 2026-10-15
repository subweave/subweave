use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::service::{self, GoalState};

/// One service as `ps` shows it.
#[derive(Serialize)]
pub(crate) struct Row {
    pub service: String,
    /// `None` until the daemon's first tick of the service.
    pub goals: Option<Goals>,
    pub failing: Vec<String>,
    /// Open asks, by number.
    pub asks: Vec<u64>,
    /// `stopped`, `starting` before the first tick, or the action running now.
    pub repair: Option<String>,
    /// The decider, then the coder chain.
    pub models: String,
    /// `None` until the daemon's first tick of the service.
    pub spend_month_usd: Option<f64>,
}

#[derive(Serialize)]
pub(crate) struct Goals {
    pub held: usize,
    pub total: usize,
}

/// Every service the daemon keeps, by name.
pub(crate) fn rows() -> Result<Vec<Row>> {
    let mut rows = Vec::new();
    for name in service::names()? {
        let row = match service::status(&name)? {
            Some(st) => Row {
                goals: Some(Goals {
                    held: st
                        .goals
                        .iter()
                        .filter(|g| g.state == GoalState::Holds)
                        .count(),
                    total: st.goals.len(),
                }),
                failing: st
                    .goals
                    .iter()
                    .filter(|g| g.state == GoalState::Fails)
                    .map(|g| g.name.clone())
                    .collect(),
                asks: st.asks,
                repair: if st.stopped {
                    Some("stopped".into())
                } else {
                    st.repair
                },
                models: models(st.decide, &st.code),
                spend_month_usd: Some(st.spend_month_usd),
                service: name,
            },
            None => {
                let spec = service::spec(&name)?;
                let state = if spec.stopped { "stopped" } else { "starting" };
                Row {
                    goals: None,
                    failing: Vec::new(),
                    asks: Vec::new(),
                    repair: Some(state.into()),
                    models: models(spec.decide, &spec.code),
                    spend_month_usd: None,
                    service: name,
                }
            }
        };
        rows.push(row);
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
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run() -> Result<ExitCode> {
    let services = rows()?;
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
