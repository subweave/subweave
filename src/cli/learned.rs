use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::service::{self, Command};

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

#[derive(clap::Args)]
pub struct ForgetArgs {
    pub service: String,
    /// A FACT, an ACTION (every goal's steps), or <action>/<goal>
    name: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run(args: Args) -> Result<ExitCode> {
    super::known(&args.service)?;
    let learned = service::learned(&args.service)?;
    if learned.facts.is_empty() && learned.steps.is_empty() {
        println!("{} learned nothing yet", args.service);
        return Ok(ExitCode::SUCCESS);
    }
    if !learned.facts.is_empty() {
        let w = learned.facts.keys().map(String::len).max().unwrap_or(0);
        println!("facts");
        for (name, f) in &learned.facts {
            println!("  {name:w$}  {}   {}", f.command.trim(), f.t);
        }
    }
    if !learned.steps.is_empty() {
        if !learned.facts.is_empty() {
            println!();
        }
        println!("steps");
        for (key, s) in &learned.steps {
            println!("  {key}   {}", s.t);
            for c in &s.commands {
                println!("    {}", c.trim().replace('\n', "\n    "));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Fails unless the service learned something under `name`.
pub(crate) fn check_forget(service: &str, name: &str) -> Result<()> {
    super::known(service)?;
    let learned = service::learned(service)?;
    let prefix = format!("{name}/");
    let found = learned.facts.contains_key(name)
        || learned.steps.contains_key(name)
        || learned.steps.keys().any(|k| k.starts_with(&prefix));
    if !found {
        bail!("{service} learned nothing named {name}: subweave learned {service} lists it");
    }
    Ok(())
}

pub fn forget(args: ForgetArgs) -> Result<ExitCode> {
    check_forget(&args.service, &args.name)?;
    service::put(&Command::Forget {
        service: args.service.clone(),
        name: args.name.clone(),
        by: super::by(),
    })?;
    println!(
        "asked the daemon to forget {} in {}",
        args.name, args.service
    );
    Ok(ExitCode::SUCCESS)
}
