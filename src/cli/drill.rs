use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::service::{self, Command};

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// A DRILL of the service's weave
    drill: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// Hands the daemon drill `drill` of `service`, when its weave has the drill.
pub(crate) fn send(service: &str, drill: &str, by: String) -> Result<()> {
    let w = super::service_weave(service)?;
    if w.drill(drill).is_none() {
        let names: Vec<&str> = w.drills.iter().map(|d| d.name.as_str()).collect();
        if names.is_empty() {
            bail!("{service} has no drill");
        }
        bail!("{service} has no drill {drill}: {}", names.join(", "));
    }
    service::put(&Command::Drill {
        service: service.to_string(),
        drill: drill.to_string(),
        by,
    })
}

pub fn run(args: Args) -> Result<ExitCode> {
    send(&args.service, &args.drill, super::by())?;
    println!(
        "asked the daemon to run drill {} on {}: subweave log -f {}",
        args.drill, args.service, args.service
    );
    Ok(ExitCode::SUCCESS)
}
