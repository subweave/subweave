use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};

#[derive(clap::Args)]
pub struct Args {
    /// Install a systemd service that runs the daemon, and start it (Linux, root)
    #[arg(long)]
    install: bool,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    if args.install {
        let unit = install("subweave", "Subweave daemon", "daemon")?;
        println!("wrote {unit}; subweave runs and starts at boot");
    } else {
        crate::daemon::run().await?;
    }
    Ok(ExitCode::SUCCESS)
}

/// `/etc/systemd/system/<name>.service` running `subweave <command>`, enabled and started.
pub(super) fn install(name: &str, description: &str, command: &str) -> Result<String> {
    if !cfg!(target_os = "linux") {
        bail!("--install needs Linux with systemd");
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        bail!("--install needs root");
    }
    let exe = std::env::current_exe().context("finding this binary")?;
    let path = unit(name);
    let unit = format!(
        "[Unit]\nDescription={description}\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nExecStart={} {command}\nRestart=always\nRestartSec=5\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        exe.display()
    );
    std::fs::write(&path, unit).with_context(|| format!("write {path}"))?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", name])?;
    Ok(path)
}

pub(super) fn unit(name: &str) -> String {
    format!("/etc/systemd/system/{name}.service")
}

pub(super) fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .status()
        .context("running systemctl")?;
    if !status.success() {
        bail!("systemctl {} exited with {status}", args.join(" "));
    }
    Ok(())
}
