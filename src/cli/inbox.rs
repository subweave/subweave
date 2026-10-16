use std::process::ExitCode;

use anyhow::{Result, bail};

use crate::daemon::journal;
use crate::service::{self, Command};
use crate::weave::Class;

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
    /// What the escalation's log lines are, normal or trouble: the box knows lines of
    /// their kinds from then on
    #[arg(long, value_name = "CLASS", value_parser = super::known::class)]
    known: Option<Class>,
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
            a.card.title.trim(),
            a.card
                .buttons
                .iter()
                .map(|b| if b.label == b.word {
                    b.word.clone()
                } else {
                    format!("{}: {}", b.word, b.label)
                })
                .collect::<Vec<_>>()
                .join(" | "),
            if a.signed { "  signed: answer on Steery" } else { "" },
        );
        for line in a.card.text().lines().filter(|l| !l.trim().is_empty()) {
            println!("      {line}");
        }
    }
    println!("\nanswer with: subweave answer <n> <word>");
    Ok(ExitCode::SUCCESS)
}

/// Hands the daemon `word` for escalation `n` of service `of` (of any, when `None`),
/// when `word` answers it and an answer from here may settle it; `note` goes in the
/// journal after the answer, and `known` teaches the box what its lines are. The
/// escalation's service, or why the answer settles nothing: one to an escalation settled
/// already still goes, and the daemon journals it as refused.
pub(crate) fn send_answer(
    of: Option<&str>,
    n: u64,
    word: &str,
    by: String,
    note: Option<String>,
    known: Option<Class>,
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
    let words = esc.card.words();
    if !words.iter().any(|a| a == word) {
        bail!("#{n} takes {}, not {word:?}", words.join(", "));
    }
    if known.is_some() && esc.lines.is_empty() {
        bail!("#{n} holds no log lines to teach");
    }
    service::put(&Command::Answer {
        n,
        word: word.to_string(),
        by,
        note,
        known,
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
    let service = send_answer(None, args.n, &args.word, super::by(), note, args.known)?;
    match args.known {
        Some(c) => println!(
            "answered #{} for {service}: {}; its lines are known {}",
            args.n,
            args.word,
            c.as_str()
        ),
        None => println!("answered #{} for {service}: {}", args.n, args.word),
    }
    Ok(ExitCode::SUCCESS)
}
