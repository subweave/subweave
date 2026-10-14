use std::fmt::Write;
use std::process::ExitCode;

use anyhow::Result;

use crate::service;

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// One draft SKILL per learned steps entry, for the weave's author to paste in.
pub fn run(args: Args) -> Result<ExitCode> {
    let w = super::service_weave(&args.service)?;
    let spec = service::spec(&args.service)?;
    let learned = service::learned(&args.service)?;
    if learned.steps.is_empty() {
        println!("{} learned no steps yet", args.service);
        return Ok(ExitCode::SUCCESS);
    }
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
    print!("{out}");
    Ok(ExitCode::SUCCESS)
}
