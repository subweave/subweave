use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};

const UNIT: &str = "/etc/systemd/system/subweave.service";

#[derive(clap::Args)]
pub struct Args {
    /// Install a systemd service that runs the daemon, and start it (Linux, root)
    #[arg(long)]
    install: bool,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    if args.install {
        install()?;
    } else {
        crate::daemon::run().await?;
    }
    Ok(ExitCode::SUCCESS)
}

fn install() -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("--install needs Linux with systemd");
    }
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        bail!("--install needs root");
    }
    let exe = std::env::current_exe().context("finding this binary")?;
    let unit = format!(
        "[Unit]\nDescription=Subweave daemon\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nExecStart={} daemon\nRestart=always\nRestartSec=5\n\n\
         [Install]\nWantedBy=multi-user.target\n",
        exe.display()
    );
    std::fs::write(UNIT, unit).with_context(|| format!("write {UNIT}"))?;
    for args in [&["daemon-reload"][..], &["enable", "--now", "subweave"][..]] {
        let status = Command::new("systemctl")
            .args(args)
            .status()
            .context("running systemctl")?;
        if !status.success() {
            bail!("systemctl {} exited with {status}", args.join(" "));
        }
    }
    println!("wrote {UNIT}; subweave runs and starts at boot");
    Ok(())
}
