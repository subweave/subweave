use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::service::{self, Command, Line};

#[derive(clap::Args)]
pub struct Args {
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

#[derive(clap::Args)]
pub struct AnswerArgs {
    /// The ask's number, as `inbox` shows it
    n: u64,
    /// yes, no, always, never, or an action's name
    word: String,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

/// Every open ask on this box, by number, with its service.
fn open() -> Result<Vec<(String, Line)>> {
    let mut out = Vec::new();
    for name in service::names()? {
        let journal = service::journal(&name)?;
        out.extend(
            service::open_asks(&journal)
                .into_iter()
                .map(|l| (name.clone(), l.clone())),
        );
    }
    out.sort_by_key(|(_, l)| l.n);
    Ok(out)
}

pub fn run() -> Result<ExitCode> {
    let asks = open()?;
    if asks.is_empty() {
        println!("no open asks");
        return Ok(ExitCode::SUCCESS);
    }
    let sw = asks.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
    let gw = asks
        .iter()
        .map(|(_, l)| l.goal.as_deref().map_or(1, str::len))
        .max()
        .unwrap_or(0);
    for (service, l) in &asks {
        println!(
            "#{:<3} {service:sw$}  {:gw$}  {}  [{}]",
            l.n.unwrap_or_default(),
            l.goal.as_deref().unwrap_or("-"),
            l.text.as_deref().unwrap_or("").trim(),
            l.answers.as_deref().unwrap_or_default().join(" "),
        );
    }
    println!("\nanswer with: subweave answer <n> <word>");
    Ok(ExitCode::SUCCESS)
}

pub fn answer(args: AnswerArgs) -> Result<ExitCode> {
    let asks = open()?;
    let Some((service, ask)) = asks.iter().find(|(_, l)| l.n == Some(args.n)) else {
        bail!("no open ask #{}: subweave inbox lists them", args.n);
    };
    let answers = ask.answers.as_deref().unwrap_or_default();
    if !answers.iter().any(|a| *a == args.word) {
        bail!(
            "#{} takes {}, not {:?}",
            args.n,
            answers.join(", "),
            args.word
        );
    }
    service::put(&Command::Answer {
        n: args.n,
        word: args.word.clone(),
        by: format!("cli:{}", super::user()),
    })?;
    println!("answered #{} for {service}: {}", args.n, args.word);
    Ok(ExitCode::SUCCESS)
}
