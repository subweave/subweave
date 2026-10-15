use std::collections::BTreeSet;
use std::fmt::Write;
use std::process::ExitCode;

use anyhow::Result;

use crate::model::{self, decider::Decider};
use crate::plan::{self, Level};
use crate::weave::{self, ArgKind, FactSource, Weave, graph, show_duration};

#[derive(clap::Args)]
pub struct Args {
    /// A weave directory, or a name like postgres or postgres:0.5
    weave: String,
    /// One sentence saying what you want, like "postgres 18, private network"
    request: Option<String>,
    /// Print JSON: the findings with a request, else the parsed weave
    #[arg(long)]
    json: bool,
    /// The decider model, <id>[@<provider>]
    #[arg(long, value_name = "MODEL")]
    decide: Option<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let Some(w) = super::load(&args.weave).await? else {
        return Ok(ExitCode::FAILURE);
    };
    // References to other services are checked against the services a running daemon
    // keeps; with none here, each is only said.
    if crate::daemon::running().is_some() {
        if !super::composed(&w.name, &w)? {
            return Ok(ExitCode::FAILURE);
        }
    } else {
        for r in weave::compose::refs(&w) {
            eprintln!(
                "{}",
                weave::Problem::new(r.line, format!("{}: not checked: no daemon runs here", r.text))
            );
        }
    }
    let Some(request) = args.request else {
        if args.json {
            println!("{}", serde_json::to_string_pretty(&w)?);
        } else {
            print!("{}", overview(&w));
        }
        return Ok(ExitCode::SUCCESS);
    };
    let decider = Decider::new(model::decide_model(args.decide.as_deref(), &w.model)?)?;
    let plan = plan::plan(&w, &request, &decider, None).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print!("{}", plan::render(&plan));
    }
    Ok(if plan.count(Level::Error) > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// The ARGs, the SECRETs, the FACTs with their types, the goal tree, the KNOWN lines and
/// the drills.
fn overview(w: &Weave) -> String {
    let mut out = String::new();
    let nw = w.args.iter().map(|a| a.name.len()).max().unwrap_or(0);
    let dw = w
        .args
        .iter()
        .map(|a| a.default.as_deref().map_or(1, str::len))
        .max()
        .unwrap_or(0);
    for a in &w.args {
        let default = a.default.as_deref().unwrap_or("-");
        let head = format!("{:nw$}  {:6}  {:dw$}", a.name, a.level.as_str(), default);
        match &a.kind {
            ArgKind::Value { what } => {
                let _ = writeln!(out, "{head}  {what}");
            }
            ArgKind::Options(opts) => {
                let _ = writeln!(out, "{}", head.trim_end());
                let lw = opts.iter().map(|o| o.label.len()).max().unwrap_or(0);
                for o in opts {
                    let _ = writeln!(out, "    {:lw$}  {}", o.label, o.meaning);
                }
            }
        }
    }
    if !w.secrets.is_empty() {
        out.push('\n');
        let sw = w.secrets.iter().map(|s| s.name.len()).max().unwrap_or(0);
        for s in &w.secrets {
            let _ = write!(out, "{:sw$}  {}", s.name, s.what);
            if let Some(e) = &s.cond {
                let _ = write!(out, "   IF {e}");
            }
            out.push('\n');
        }
    }
    if !w.facts.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let fw = w.facts.iter().map(|f| f.name.len()).max().unwrap_or(0);
        for f in &w.facts {
            let how = match &f.source {
                FactSource::Command(_) => "command".to_string(),
                FactSource::Quoted(_) => "learned".to_string(),
                FactSource::Part { part, .. } => match w.part(part) {
                    Some(p) => format!("PART {}", p.source),
                    None => "PART".to_string(),
                },
            };
            let _ = writeln!(out, "{:fw$}  {:9}  {how}", f.name, f.ty.to_string());
        }
    }
    if !w.goals.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&graph::tree(w, &BTreeSet::new()));
    }
    if !w.known.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let gw = w.known.iter().map(|k| k.goal.len()).max().unwrap_or(0);
        for k in &w.known {
            let _ = writeln!(out, "KNOWN {:gw$}  {:7}  {}", k.goal, k.class.keyword(), k.sample);
        }
    }
    if !w.drills.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let nw = w.drills.iter().map(|d| d.name.len()).max().unwrap_or(0);
        for d in &w.drills {
            let _ = write!(
                out,
                "DRILL {:nw$}  \"{}\"  EXPECT {} FAILS WITHIN {} HOLDS WITHIN {}",
                d.name,
                d.label,
                d.expect,
                show_duration(d.fails_within),
                show_duration(d.holds_within)
            );
            if let Some(every) = d.every {
                let _ = write!(out, " EVERY {}", show_duration(every));
            }
            if let Some(e) = &d.when {
                let _ = write!(out, " IF {e}");
            }
            if d.restore.is_none() {
                out.push_str("  (no RESTORE)");
            }
            out.push('\n');
        }
    }
    out
}
