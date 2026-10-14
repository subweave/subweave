use std::process::ExitCode;

use anyhow::{Result, bail};

use super::{daemon, term};
use crate::dash::{self, Config, config};
use crate::paths;

const UNIT: &str = "subweave-dash";

#[derive(clap::Args)]
pub struct Args {
    /// Listen on this address, like 127.0.0.1 or 0.0.0.0; saved, then the command exits
    #[arg(long, value_name = "ADDR")]
    bind: Option<String>,
    /// Listen on this port; saved, then the command exits
    #[arg(long)]
    port: Option<u16>,
    /// Set the password, one line read from stdin; saved, then the command exits
    #[arg(long, conflicts_with = "no_password")]
    password: bool,
    /// Remove the password: no login, and loopback only
    #[arg(long)]
    no_password: bool,
    /// Install a systemd service that runs the dashboard, and start it (Linux, root)
    #[arg(long)]
    install: bool,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let set = args.bind.is_some() || args.port.is_some() || args.password || args.no_password;
    if !set && !args.install {
        dash::serve(Config::load()?).await?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut cfg = Config::load()?;
    if set {
        if let Some(b) = args.bind {
            cfg.bind = b;
        }
        if let Some(p) = args.port {
            cfg.port = p;
        }
        if args.no_password {
            cfg.password = None;
        }
        if args.password {
            let pw = term::secret("dashboard password")?;
            if pw.is_empty() {
                bail!("the password is empty: nothing changed");
            }
            cfg.password = Some(config::hash(&pw)?);
        }
        cfg.addr()?;
        cfg.save()?;
        println!(
            "wrote {}: {}:{}, {}",
            paths::dash().display(),
            cfg.bind,
            cfg.port,
            if cfg.password.is_some() {
                "password set"
            } else {
                "no password"
            }
        );
        if !args.install && std::path::Path::new(&daemon::unit(UNIT)).exists() {
            daemon::systemctl(&["restart", UNIT])?;
            println!("restarted {UNIT}");
        }
    }
    if args.install {
        cfg.addr()?;
        let unit = daemon::install(UNIT, "Subweave dashboard", "dash")?;
        if set {
            daemon::systemctl(&["restart", UNIT])?;
        }
        println!(
            "wrote {unit}; the dashboard runs on {}:{} and starts at boot",
            cfg.bind, cfg.port
        );
    }
    Ok(ExitCode::SUCCESS)
}
