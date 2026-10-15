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
    /// Why, in a sentence; it goes in the journal after the answer
    #[arg(long)]
    note: Option<String>,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub fn run() -> Result<ExitCode> {
    let asks = service::all_asks()?;
    if asks.is_empty() {
        println!("no open escalations");
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
            "#{:<3} {:sw$}  {:gw$}  {}  [{}]{}",
            a.n,
            a.service,
            a.goal.as_deref().unwrap_or("-"),
            a.title.trim(),
            a.answers
                .iter()
                .map(|w| match a.labels.get(w).filter(|l| *l != w) {
                    Some(l) => format!("{w}: {l}"),
                    None => w.clone(),
                })
                .collect::<Vec<_>>()
                .join(" | "),
            if a.signed { "  signed: answer on Steery" } else { "" },
        );
    }
    println!("\nanswer with: subweave answer <n> <word>");
    Ok(ExitCode::SUCCESS)
}

/// The open ask `n`, when `word` answers it and an answer from here may settle it.
pub(crate) fn check_answer(n: u64, word: &str) -> Result<Ask> {
    let Some(ask) = service::all_asks()?.into_iter().find(|a| a.n == n) else {
        bail!("no open escalation #{n}: subweave inbox lists them");
    };
    if ask.signed {
        bail!("#{n}: only a signed answer on Steery settles this");
    }
    if !ask.answers.iter().any(|a| a == word) {
        bail!("#{n} takes {}, not {word:?}", ask.answers.join(", "));
    }
    Ok(ask)
}

/// Hands the daemon `word` for open ask `n`, when `word` answers it; `note` goes in the
/// journal after the answer.
pub(crate) fn send_answer(n: u64, word: &str, by: String, note: Option<String>) -> Result<Ask> {
    let ask = check_answer(n, word)?;
    service::put(&Command::Answer {
        n,
        word: word.to_string(),
        by,
        note,
    })?;
    Ok(ask)
}

pub fn answer(args: AnswerArgs) -> Result<ExitCode> {
    let note = args
        .note
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let ask = send_answer(args.n, &args.word, super::by(), note)?;
    println!("answered #{} for {}: {}", args.n, ask.service, args.word);
    Ok(ExitCode::SUCCESS)
}
