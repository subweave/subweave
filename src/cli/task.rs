use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::service::{self, Command};

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// What to do, in words, like "vacuum the shop database"
    text: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run(args: Args) -> Result<ExitCode> {
    super::known(&args.service)?;
    let text = args.text.trim();
    if text.is_empty() {
        bail!("say what to do");
    }
    service::put(&Command::Task {
        service: args.service.clone(),
        text: text.to_string(),
        by: super::by(),
    })?;
    println!(
        "gave {} the task; it runs when no repair does: subweave log -f {}",
        args.service, args.service
    );
    Ok(ExitCode::SUCCESS)
}
