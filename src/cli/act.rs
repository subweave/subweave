use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::service::{self, Command};

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// An ACTION of the service's weave
    action: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run(args: Args) -> Result<ExitCode> {
    let w = super::service_weave(&args.service)?;
    if w.action(&args.action).is_none() {
        let names: Vec<&str> = w.actions.iter().map(|a| a.name.as_str()).collect();
        bail!(
            "{} has no action {}: {}",
            args.service,
            args.action,
            names.join(", ")
        );
    }
    service::put(&Command::Do {
        service: args.service.clone(),
        action: args.action.clone(),
    })?;
    println!(
        "asked the daemon to run {} on {}: subweave log -f {}",
        args.action, args.service, args.service
    );
    Ok(ExitCode::SUCCESS)
}
