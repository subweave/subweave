use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::service::{self, Ask, Command};

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

pub fn run() -> Result<ExitCode> {
    let asks = service::all_asks()?;
    if asks.is_empty() {
        println!("no open asks");
        return Ok(ExitCode::SUCCESS);
    }
    let sw = asks.iter().map(|a| a.service.len()).max().unwrap_or(0);
    let gw = asks
        .iter()
        .map(|a| a.goal.as_deref().map_or(1, str::len))
        .max()
        .unwrap_or(0);
    for a in &asks {
        println!(
            "#{:<3} {:sw$}  {:gw$}  {}  [{}]",
            a.n,
            a.service,
            a.goal.as_deref().unwrap_or("-"),
            a.title.trim(),
            a.answers.join(" "),
        );
    }
    println!("\nanswer with: subweave answer <n> <word>");
    Ok(ExitCode::SUCCESS)
}

/// The open ask `n`, when `word` answers it.
pub(crate) fn check_answer(n: u64, word: &str) -> Result<Ask> {
    let Some(ask) = service::all_asks()?.into_iter().find(|a| a.n == n) else {
        bail!("no open ask #{n}: subweave inbox lists them");
    };
    if !ask.answers.iter().any(|a| a == word) {
        bail!("#{n} takes {}, not {word:?}", ask.answers.join(", "));
    }
    Ok(ask)
}

pub fn answer(args: AnswerArgs) -> Result<ExitCode> {
    let ask = check_answer(args.n, &args.word)?;
    service::put(&Command::Answer {
        n: args.n,
        word: args.word.clone(),
        by: super::by(),
    })?;
    println!("answered #{} for {}: {}", args.n, ask.service, args.word);
    Ok(ExitCode::SUCCESS)
}
