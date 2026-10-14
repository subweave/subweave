use std::process::ExitCode;

use anyhow::Result;

use super::term;
use crate::service::{self, Command};

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

#[derive(clap::Args)]
pub struct RmArgs {
    pub service: String,
    /// Remove without asking
    #[arg(long)]
    yes: bool,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn stop(args: Args) -> Result<ExitCode> {
    super::known(&args.service)?;
    service::put(&Command::Stop {
        service: args.service.clone(),
    })?;
    println!(
        "asked the daemon to stop {}; subweave start {} resumes it",
        args.service, args.service
    );
    Ok(ExitCode::SUCCESS)
}

pub fn start(args: Args) -> Result<ExitCode> {
    super::known(&args.service)?;
    service::put(&Command::Start {
        service: args.service.clone(),
    })?;
    println!("asked the daemon to start {}", args.service);
    Ok(ExitCode::SUCCESS)
}

pub fn rm(args: RmArgs) -> Result<ExitCode> {
    super::known(&args.service)?;
    let question = format!(
        "remove {}, its journal, secrets and learned commands? What it installed stays.",
        args.service
    );
    if !args.yes && !term::confirm(&question)? {
        println!("nothing removed");
        return Ok(ExitCode::FAILURE);
    }
    service::put(&Command::Rm {
        service: args.service.clone(),
    })?;
    println!("asked the daemon to remove {}", args.service);
    Ok(ExitCode::SUCCESS)
}
