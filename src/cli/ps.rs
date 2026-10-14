use std::process::ExitCode;

use anyhow::Result;

use crate::service::{self, GoalState};

#[derive(clap::Args)]
pub struct Args {
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run() -> Result<ExitCode> {
    let names = service::names()?;
    if names.is_empty() {
        println!("no services here");
        return Ok(ExitCode::SUCCESS);
    }
    let mut rows = vec![
        [
            "SERVICE", "GOALS", "FAILING", "ASKS", "REPAIR", "MODELS", "MONTH",
        ]
        .map(String::from),
    ];
    for name in names {
        let row = match service::status(&name)? {
            Some(st) => {
                let held = st
                    .goals
                    .iter()
                    .filter(|g| g.state == GoalState::Holds)
                    .count();
                let failing: Vec<&str> = st
                    .goals
                    .iter()
                    .filter(|g| g.state == GoalState::Fails)
                    .map(|g| g.name.as_str())
                    .collect();
                let asks: Vec<String> = st.asks.iter().map(|n| format!("#{n}")).collect();
                let repair = if st.stopped {
                    "stopped".to_string()
                } else {
                    st.repair.clone().unwrap_or_else(|| "-".into())
                };
                let mut models = st.decide.clone();
                if !st.code.is_empty() {
                    models.push_str(" / ");
                    models.push_str(&st.code.join(" then "));
                }
                [
                    name,
                    format!("{held}/{}", st.goals.len()),
                    dash(failing.join(" ")),
                    dash(asks.join(" ")),
                    repair,
                    models,
                    money(st.spend_month_usd),
                ]
            }
            None => {
                let spec = service::spec(&name)?;
                let mut models = spec.decide;
                if !spec.code.is_empty() {
                    models.push_str(" / ");
                    models.push_str(&spec.code.join(" then "));
                }
                let state = if spec.stopped { "stopped" } else { "starting" };
                [
                    name,
                    "-".into(),
                    "-".into(),
                    "-".into(),
                    state.into(),
                    models,
                    "-".into(),
                ]
            }
        };
        rows.push(row);
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
