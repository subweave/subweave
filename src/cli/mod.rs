//! The `subweave` command line. Each command is one `Command` variant and one file
//! with `Args` and `run`. A command with `--on <user@host>`, or one naming a service that
//! `boxes.json` places on a box, runs on that box instead (`crate::remote`).

pub(crate) mod act;
mod channel;
mod control;
mod daemon;
mod dash;
mod flow;
pub(crate) mod inbox;
mod key;
mod learn;
pub(crate) mod learned;
pub(crate) mod log;
mod mcp;
mod plan;
mod provider;
mod ps;
mod review;
mod run;
pub(crate) mod stats;
pub(crate) mod tell;
mod task;
mod term;
mod version;
mod watch;
pub(crate) mod weavefile;

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::{paths, remote, weave};

#[derive(Parser)]
#[command(
    name = "subweave",
    version,
    about = "Keeps one service running on one Linux box, as its Weavefile says"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check a request against a weave, or show its ARGs and goals; changes nothing
    Plan(plan::Args),
    /// Check a request, then hand the service to the daemon
    Run(run::Args),
    /// Change a service's ARGs by sentence, or its models
    Tell(tell::Args),
    /// Services, their goals, asks, models and spend this month
    Ps(ps::Args),
    /// A service's journal
    Log(log::Args),
    /// Numeric facts over the last 24 hours
    Stats(stats::Args),
    /// Open asks of every service
    Inbox(inbox::Args),
    /// Answer an open ask
    Answer(inbox::AnswerArgs),
    /// Run one action now, through the same rules
    Do(act::Args),
    /// Give the coder a task in words, through the same rules
    Task(task::Args),
    /// The commands this box learned for a service
    Learned(learned::Args),
    /// Drop a learned fact command, or an action's learned steps
    Forget(learned::ForgetArgs),
    /// A service's running Weavefile and ARGs; --change proposes a new Weavefile
    Weave(weavefile::Args),
    /// Draft SKILL recipes from learned steps, for the weave's author
    Learn(learn::Args),
    /// Pause a service
    Stop(control::Args),
    /// Resume a service
    Start(control::Args),
    /// Remove a service, its journal and what it learned
    Rm(control::RmArgs),
    /// Where the daemon sends messages
    Channel(channel::Args),
    /// Every box's asks and warnings, as notifications on this computer
    Watch,
    /// An MCP server on stdin and stdout, for an agent that answers asks
    Mcp,
    /// Store a provider's key, read from stdin
    Key(key::Args),
    /// Add, remove or list model providers
    Provider(provider::Args),
    /// Score a weave against the review rules; changes nothing
    Review(review::Args),
    /// Run every service in the foreground; --install makes it a systemd service
    Daemon(daemon::Args),
    /// The web dashboard; with flags, set where it listens and its password
    Dash(dash::Args),
    /// Print the version
    Version,
}

impl Command {
    /// `--on`, and the service the command names.
    fn route(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Command::Run(a) => (a.on.as_deref(), None),
            Command::Tell(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Ps(a) => (a.on.as_deref(), None),
            Command::Log(a) => (a.on.as_deref(), a.service.as_deref()),
            Command::Stats(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Inbox(a) => (a.on.as_deref(), None),
            Command::Answer(a) => (a.on.as_deref(), None),
            Command::Do(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Task(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Learned(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Forget(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Learn(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Stop(a) | Command::Start(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Rm(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Weave(a) => (a.on.as_deref(), Some(&a.service)),
            Command::Channel(a) => (a.on.as_deref(), None),
            Command::Key(a) => (a.on.as_deref(), None),
            Command::Provider(a) => (a.on.as_deref(), None),
            Command::Dash(a) => (a.on.as_deref(), None),
            Command::Plan(_)
            | Command::Review(_)
            | Command::Daemon(_)
            | Command::Watch
            | Command::Mcp
            | Command::Version => (None, None),
        }
    }

    /// The box this command runs on, when not this machine.
    fn host(&self) -> Result<Option<String>> {
        let (on, service) = self.route();
        if let Some(h) = on {
            return Ok(Some(h.to_string()));
        }
        match service {
            Some(s) => remote::host_of(s),
            None => Ok(None),
        }
    }
}

pub async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command.host() {
        Ok(Some(host)) => forward(cli.command, &host).await,
        Ok(None) => local(cli.command).await,
        Err(e) => Err(e),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn local(command: Command) -> Result<ExitCode> {
    match command {
        Command::Plan(args) => plan::run(args).await,
        Command::Run(args) => run::run(args).await,
        Command::Tell(args) => tell::run(args).await,
        Command::Ps(_) => ps::run(),
        Command::Log(args) => log::run(args).await,
        Command::Stats(args) => stats::run(args),
        Command::Inbox(_) => inbox::run(),
        Command::Answer(args) => inbox::answer(args),
        Command::Do(args) => act::run(args),
        Command::Task(args) => task::run(args),
        Command::Learned(args) => learned::run(args),
        Command::Forget(args) => learned::forget(args),
        Command::Learn(args) => learn::run(args),
        Command::Stop(args) => control::stop(args),
        Command::Start(args) => control::start(args),
        Command::Rm(args) => control::rm(args),
        Command::Weave(args) => weavefile::run(args).await,
        Command::Channel(args) => channel::run(args).await,
        Command::Watch => watch::run().await,
        Command::Mcp => mcp::run().await,
        Command::Key(args) => key::run(args),
        Command::Provider(args) => provider::run(args),
        Command::Review(args) => review::run(args).await,
        Command::Daemon(args) => daemon::run(args).await,
        Command::Dash(args) => dash::run(args).await,
        Command::Version => version::run(),
    }
}

/// The same command line on the box, without `--on`.
async fn forward(command: Command, host: &str) -> Result<ExitCode> {
    let mut args = Vec::new();
    let mut argv = std::env::args_os().skip(1);
    while let Some(a) = argv.next() {
        let a = a
            .into_string()
            .map_err(|a| anyhow::anyhow!("{} is not UTF-8", a.to_string_lossy()))?;
        if a == "--on" {
            argv.next();
        } else if !a.starts_with("--on=") {
            args.push(a);
        }
    }
    remote::ensure(host).await?;
    let code = match command {
        Command::Run(a) => run::forward(a, host, args).await?,
        Command::Tell(a) => {
            tell::send_keys(&a, host)?;
            remote::run(host, &args)?
        }
        Command::Rm(a) => {
            let code = remote::run(host, &args)?;
            if code == 0 {
                remote::forget(&a.service)?;
            }
            code
        }
        _ => remote::run(host, &args)?,
    };
    Ok(exit(code))
}

fn exit(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

/// Reads and checks a weave; on problems prints `Weavefile:<line>: <message>` lines to
/// stderr and gives `None`.
pub(crate) async fn load(spec: &str) -> Result<Option<weave::Weave>> {
    Ok(load_text(spec).await?.map(|(w, _)| w))
}

/// `load`, with the Weavefile's text.
pub(crate) async fn load_text(spec: &str) -> Result<Option<(weave::Weave, String)>> {
    let (name, text) = weave::source::resolve(spec).await?;
    Ok(parsed(&name, &text).map(|w| (w, text)))
}

fn parsed(name: &str, text: &str) -> Option<weave::Weave> {
    match weave::parse(name, text) {
        Ok(w) => Some(w),
        Err(problems) => {
            for p in problems {
                eprintln!("{p}");
            }
            None
        }
    }
}

/// Fails unless the daemon here keeps `service`.
pub(crate) fn known(service: &str) -> Result<()> {
    if !paths::service_json(service).is_file() {
        bail!("no service {service} here");
    }
    Ok(())
}

/// The Weavefile the daemon keeps for a service.
pub(crate) fn service_weave(service: &str) -> Result<weave::Weave> {
    known(service)?;
    let file = paths::weavefile(service);
    let text =
        std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
    parsed(service, &text).with_context(|| format!("{} does not parse", file.display()))
}

/// Who acted, on the Commands this terminal sends: the agent channel that runs it
/// (`SUBWEAVE_BY`, which the daemon sets), else `cli:<user>`.
pub(crate) fn by() -> String {
    match std::env::var("SUBWEAVE_BY") {
        Ok(by) if !by.is_empty() => by,
        _ => format!("cli:{}", user()),
    }
}

/// Who answers at this terminal.
fn user() -> String {
    std::env::var("SUDO_USER")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown".into())
}
