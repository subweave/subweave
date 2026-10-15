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

/// Hands `service` a task in words.
pub(crate) fn send(service: &str, text: &str, by: String) -> Result<()> {
    super::known(service)?;
    let text = text.trim();
    if text.is_empty() {
        bail!("say what to do");
    }
    service::put(&Command::Task {
        service: service.to_string(),
        text: text.to_string(),
        by,
    })
}

pub fn run(args: Args) -> Result<ExitCode> {
    send(&args.service, &args.text, super::by())?;
    println!(
        "gave {} the task; it runs when no repair does: subweave log -f {}",
        args.service, args.service
    );
    Ok(ExitCode::SUCCESS)
}
