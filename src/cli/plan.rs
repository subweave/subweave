use std::collections::BTreeSet;
use std::fmt::Write;
use std::process::ExitCode;

use anyhow::Result;

use crate::model::{self, decider::Decider};
use crate::plan::{self, Level};
use crate::weave::{ArgKind, FactSource, Weave, graph};

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

/// The ARGs, the SECRETs, the FACTs with their types, the goal tree and the KNOWN lines.
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
                FactSource::Command(_) => "command",
                FactSource::Quoted(_) => "learned",
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
    out
}
