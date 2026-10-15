use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::daemon::journal;
use crate::service::{self, Command};

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
    let asks = journal::all_asks()?;
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

/// Hands the daemon `word` for escalation `n` of service `of` (of any, when `None`),
/// when `word` answers it and an answer from here may settle it; `note` goes in the
/// journal after the answer. The escalation's service, or why the answer settles
/// nothing: one to an escalation settled already still goes, and the daemon journals it
/// as refused.
pub(crate) fn send_answer(
    of: Option<&str>,
    n: u64,
    word: &str,
    by: String,
    note: Option<String>,
) -> Result<String> {
    let found = journal::escalation(n)?.filter(|(s, _)| of.is_none_or(|want| want == s));
    let Some((owner, esc)) = found else {
        match of {
            Some(s) => bail!("{s} has no escalation #{n}: subweave inbox lists the open ones"),
            None => bail!("no escalation #{n}: subweave inbox lists the open ones"),
        }
    };
    if esc.signed {
        bail!("#{n}: only a signed answer on Steery settles this");
    }
    if !esc.card.answers.iter().any(|a| a == word) {
        bail!("#{n} takes {}, not {word:?}", esc.card.answers.join(", "));
    }
    service::put(&Command::Answer {
        n,
        word: word.to_string(),
        by,
        note,
    })?;
    if let Some(why) = esc.settled() {
        bail!("{why}; the journal keeps this answer as refused");
    }
    Ok(owner)
}

pub fn answer(args: AnswerArgs) -> Result<ExitCode> {
    let note = args
        .note
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let service = send_answer(None, args.n, &args.word, super::by(), note)?;
    println!("answered #{} for {service}: {}", args.n, args.word);
    Ok(ExitCode::SUCCESS)
}
