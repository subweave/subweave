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

/// Fails unless the service's weave has the action.
pub(crate) fn check_action(service: &str, action: &str) -> Result<()> {
    let w = super::service_weave(service)?;
    if w.action(action).is_none() {
        let names: Vec<&str> = w.actions.iter().map(|a| a.name.as_str()).collect();
        bail!("{service} has no action {action}: {}", names.join(", "));
    }
    Ok(())
}

pub fn run(args: Args) -> Result<ExitCode> {
    check_action(&args.service, &args.action)?;
    service::put(&Command::Do {
        service: args.service.clone(),
        action: args.action.clone(),
        by: super::by(),
    })?;
    println!(
        "asked the daemon to run {} on {}: subweave log -f {}",
        args.action, args.service, args.service
    );
    Ok(ExitCode::SUCCESS)
}
