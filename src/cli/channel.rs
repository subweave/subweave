use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;

use super::term;
use crate::channel::{self, ChannelKind, Escalate, Kind, Mode, Via};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Cmd,
    /// Run on this box, over ssh
    #[arg(long, global = true, value_name = "USER@HOST")]
    pub on: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a channel; its key or URL, when it has one, comes from stdin
    Add {
        /// steery, telegram, slack, email, webhook or heartbeat
        kind: ChannelKind,
        /// telegram: the chat id; slack: the channel id, for a bot token; email: the address; webhook, heartbeat: the URL
        target: Option<String>,
        /// The lowest message it gets: ask, warning or info
        #[arg(long)]
        level: Option<Kind>,
        /// steery: session (the default) escalates to an agent conversation; card asks the person on a signed card
        #[arg(long, value_name = "MODE")]
        escalate: Option<Mode>,
        /// steery session: how that agent runs subweave on this box, as ssh USER@HOST sudo -n subweave
        #[arg(long, value_name = "USER@HOST")]
        reach: Option<String>,
    },
    /// List channels
    Ls,
    /// Remove a channel
    Rm { name: String },
    /// Send a test message to a channel
    Test { name: String },
}

pub async fn run(args: Args) -> Result<ExitCode> {
    match args.command {
        Cmd::Add {
            kind,
            target,
            level,
            escalate,
            reach,
        } => {
            let secret = read_secret(kind)?;
            let ch = channel::add(kind, target, level, secret, escalate, reach).await?;
            let target = ch
                .target
                .as_deref()
                .map(|t| format!(" {t}"))
                .unwrap_or_default();
            let how = match &ch.via {
                Via::Steery { escalate, .. } => format!(", escalates {}", shown(escalate)),
                _ => String::new(),
            };
            println!(
                "added {}: {}{target}, level {}{how}",
                ch.name,
                ch.kind(),
                ch.level
            );
        }
        Cmd::Ls => {
            let list = channel::ls()?;
            if list.is_empty() {
                println!("no channels: subweave channel add <kind> adds one");
            }
            let nw = list.iter().map(|c| c.name.len()).max().unwrap_or(0);
            let tw = list
                .iter()
                .map(|c| c.target.as_deref().map_or(1, str::len))
                .max()
                .unwrap_or(0);
            for c in &list {
                let mut line = format!(
                    "{:nw$}  {:9}  {:tw$}  {}",
                    c.name,
                    c.kind().as_str(),
                    c.target.as_deref().unwrap_or("-"),
                    c.level
                );
                if let Via::Steery { escalate, pins } = &c.via {
                    line.push_str(&format!("  escalates {}", shown(escalate)));
                    let pins: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
                    line.push_str(&format!("  signers: {}", pins.join(", ")));
                }
                println!("{line}");
            }
        }
        Cmd::Rm { name } => {
            channel::rm(&name)?;
            println!("removed {name}");
        }
        Cmd::Test { name } => {
            channel::test(&name).await?;
            println!("sent a test message to {name}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `session via root@box-1`, `card`.
fn shown(e: &Escalate) -> String {
    match e {
        Escalate::Session { reach } => format!("session via {reach}"),
        Escalate::Card => "card".into(),
    }
}

/// The kind's key or URL: asked without echo at a terminal, else all of stdin. A webhook's
/// bearer token is optional and read only from a pipe.
fn read_secret(kind: ChannelKind) -> Result<Option<String>> {
    let what = match kind {
        ChannelKind::Steery => "the Steery key",
        ChannelKind::Telegram => "the bot token",
        ChannelKind::Slack => "the Slack bot token (xoxb-…) or incoming-webhook URL",
        ChannelKind::Email => "the SMTP URL, like smtps://user:pass@host:465",
        ChannelKind::Webhook => {
            if std::io::stdin().is_terminal() {
                return Ok(None);
            }
            "the bearer token"
        }
        ChannelKind::Heartbeat => return Ok(None),
    };
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(Some(term::secret(&format!("Paste {what}"))?));
    }
    let mut s = String::new();
    stdin
        .lock()
        .read_to_string(&mut s)
        .with_context(|| format!("reading {what} from stdin"))?;
    Ok(Some(s.trim().to_string()))
}
