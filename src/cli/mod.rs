//! The `subweave` command line. Each command is one `Command` variant and one file
//! with `Args` and `run`.

mod key;
mod plan;
mod provider;
mod review;
mod version;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

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
    /// Store a provider's key, read from stdin
    Key(key::Args),
    /// Add, remove or list model providers
    Provider(provider::Args),
    /// Score a weave against the review rules; changes nothing
    Review(review::Args),
    /// Print the version
    Version,
}

pub async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Plan(args) => plan::run(args).await,
        Command::Key(args) => key::run(args),
        Command::Provider(args) => provider::run(args),
        Command::Review(args) => review::run(args).await,
        Command::Version => version::run(),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Reads and checks a weave; on problems prints `Weavefile:<line>: <message>` lines to
/// stderr and gives `None`.
pub(crate) async fn load(spec: &str) -> anyhow::Result<Option<crate::weave::Weave>> {
    let (name, text) = crate::weave::source::resolve(spec).await?;
    match crate::weave::parse(&name, &text) {
        Ok(w) => Ok(Some(w)),
        Err(problems) => {
            for p in problems {
                eprintln!("{p}");
            }
            Ok(None)
        }
    }
}
