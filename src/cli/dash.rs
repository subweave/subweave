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
    #[arg(long)]
    password: bool,
    /// Answer to this host name too, like dash.acme.dev (repeat for more); saved
    #[arg(long, value_name = "NAME")]
    host: Vec<String>,
    /// Install a systemd service that runs the dashboard, and start it (Linux, root)
    #[arg(long)]
    install: bool,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let set = args.bind.is_some() || args.port.is_some() || args.password || !args.host.is_empty();
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
        for h in args.host {
            let h = h.trim().trim_end_matches('.').to_ascii_lowercase();
            if h.is_empty() || h.contains(['/', ' ', ':']) {
                bail!("{h:?} is no host name");
            }
            if !cfg.hosts.contains(&h) {
                cfg.hosts.push(h);
            }
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
                "no password yet: subweave dash --password"
            }
        );
        if !args.install
            && cfg.password.is_some()
            && std::path::Path::new(&daemon::unit(UNIT)).exists()
        {
            daemon::systemctl(&["restart", UNIT])?;
            println!("restarted {UNIT}");
        }
    }
    if args.install {
        cfg.addr()?;
        cfg.need_password()?;
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
