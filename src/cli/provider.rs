use std::process::ExitCode;

use anyhow::Result;
use clap::{Subcommand, ValueEnum};

use crate::model::provider::{self, KeySource, Protocol, Provider};

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a provider, or change the one with that name
    Add {
        name: String,
        /// The API base URL, like https://api.anthropic.com/v1
        endpoint: String,
        #[arg(long, value_enum, default_value = "openai")]
        protocol: ProtocolArg,
    },
    /// Remove a provider and its stored key
    Rm { name: String },
    /// List providers and where their keys come from
    Ls,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProtocolArg {
    Openai,
    Anthropic,
}

pub fn run(args: Args) -> Result<ExitCode> {
    match args.command {
        Cmd::Add {
            name,
            endpoint,
            protocol,
        } => {
            let protocol = match protocol {
                ProtocolArg::Openai => Protocol::OpenAi,
                ProtocolArg::Anthropic => Protocol::Anthropic,
            };
            let said = format!("added {name}: {endpoint}, {protocol}");
            provider::add(Provider {
                name,
                endpoint,
                protocol,
            })?;
            println!("{said}");
        }
        Cmd::Rm { name } => {
            provider::remove(&name)?;
            println!("removed {name}");
        }
        Cmd::Ls => {
            let list = provider::all()?;
            let nw = list.iter().map(|p| p.name.len()).max().unwrap_or(0);
            let ew = list.iter().map(|p| p.endpoint.len()).max().unwrap_or(0);
            for p in &list {
                let key = match p.key_source()? {
                    KeySource::Env => format!("key from ${}", provider::env_var(&p.name)),
                    KeySource::File => "key stored".to_string(),
                    KeySource::None => "no key".to_string(),
                };
                println!(
                    "{:nw$}  {:ew$}  {:9}  {key}",
                    p.name,
                    p.endpoint,
                    p.protocol.as_str()
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
