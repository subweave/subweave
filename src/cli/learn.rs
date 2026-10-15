use std::fmt::Write;
use std::process::ExitCode;

use anyhow::Result;

use crate::daemon::{journal, memory};
use crate::service;

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// Drafts for the weave's author to paste in: one SKILL per learned steps entry, then a
/// KNOWN line per kind of log line the box knows and the weave does not.
pub fn run(args: Args) -> Result<ExitCode> {
    let w = super::service_weave(&args.service)?;
    let spec = service::spec(&args.service)?;
    let learned = service::learned(&args.service)?;
    let args_said: Vec<String> = spec.args.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let mut out = String::new();
    for (key, steps) in &learned.steps {
        let (action, goal) = key.split_once('/').unwrap_or((key.as_str(), ""));
        let when = w
            .action(action)
            .map(|a| a.when.replace('"', "'"))
            .unwrap_or_else(|| format!("{action} makes {goal} hold"));
        if !out.is_empty() {
            out.push('\n');
        }
        let name = if goal.is_empty() {
            action.to_string()
        } else {
            format!("{action}-{goal}")
        };
        let _ = writeln!(out, "SKILL {name} WHEN \"{when}\" <<EOF");
        let _ = writeln!(
            out,
            "  Learned by {} for goal {goal} on {}, with {}.",
            args.service,
            steps.t,
            if args_said.is_empty() {
                "no ARGs".to_string()
            } else {
                args_said.join(", ")
            }
        );
        out.push_str("  These are the commands that made it hold; replace values with $name.\n\n");
        out.push_str("  ```recipe\n");
        for c in &steps.commands {
            for l in c.trim().lines() {
                let _ = writeln!(out, "  {l}");
            }
        }
        out.push_str("  ```\nEOF\n");
    }
    let st = journal::read(&args.service);
    let taught = memory::taught(&w);
    let mut known = String::new();
    for (goal, kinds) in &st.memory.goals {
        let judged = w.goal(goal).is_some_and(|g| w.judged_lines(g).next().is_some());
        let mine = taught.get(goal);
        for (template, e) in kinds {
            if !judged || e.sample.is_empty() || mine.is_some_and(|m| m.contains_key(template)) {
                continue;
            }
            let Some((class, _)) = st.memory.class(&taught, goal, template) else {
                continue;
            };
            let sample = e.sample.replace('\\', "\\\\").replace('"', "\\\"");
            let _ = writeln!(known, "KNOWN {goal} {} \"{sample}\"", class.keyword());
        }
    }
    if !known.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(
            out,
            "# What {} knows of its log lines; take out names private to this box.",
            args.service
        );
        out.push_str(&known);
    }
    if out.is_empty() {
        println!("{} learned no steps and knows no kind of line yet", args.service);
        return Ok(ExitCode::SUCCESS);
    }
    print!("{out}");
    Ok(ExitCode::SUCCESS)
}
