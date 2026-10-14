use std::io::{IsTerminal, Read};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

use crate::model::provider;

#[derive(clap::Args)]
pub struct Args {
    /// The provider, like openrouter
    provider: String,
}

pub fn run(args: Args) -> Result<ExitCode> {
    provider::get(&args.provider)?;
    let stdin = std::io::stdin();
    let mut key = String::new();
    if stdin.is_terminal() {
        eprintln!("Paste the key for {}, then press Enter:", args.provider);
        stdin.read_line(&mut key).context("reading the key")?;
    } else {
        stdin
            .lock()
            .read_to_string(&mut key)
            .context("reading the key from stdin")?;
    }
    let key = key.trim();
    if key.is_empty() {
        bail!("no key on stdin");
    }
    if key.contains(char::is_whitespace) {
        bail!("a key is one word; stdin held several");
    }
    let file = provider::store_key(&args.provider, key)?;
    println!("stored the key for {} in {}", args.provider, file.display());
    Ok(ExitCode::SUCCESS)
}
