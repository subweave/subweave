//! What `run` and `tell` share at the terminal: the plan's asks and warnings, SECRETs,
//! the daemon, and following the service until it settles.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Result, bail};
use jiff::Timestamp;

use super::log::{Tail, human};
use super::term;
use crate::plan::{Level, Plan};
use crate::service::{self, GoalState, Kind, Line};
use crate::{paths, weave};

/// Prints the plan and puts its asks and warnings to the person. False: nothing runs.
pub fn settle(plan: &Plan, yes: bool) -> Result<bool> {
    print!("{}", crate::plan::render(plan));
    if plan.count(Level::Error) > 0 {
        return Ok(false);
    }
    for f in plan.findings.iter().filter(|f| f.level == Level::Ask) {
        if !term::confirm(&format!("ask: {} {}. Go on?", f.subject, f.text))? {
            println!("nothing runs.");
            return Ok(false);
        }
    }
    if plan.count(Level::Warning) > 0 && !yes && !term::confirm("go on?")? {
        println!("nothing runs.");
        return Ok(false);
    }
    Ok(true)
}

/// Each named SECRET from the env var of the same name, else asked without echo.
pub fn secrets(
    w: &weave::Weave,
    names: &[String],
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for name in names {
        let value = match std::env::var(name) {
            Ok(v) if !v.is_empty() => v,
            _ => {
                let what = w
                    .secrets
                    .iter()
                    .find(|s| &s.name == name)
                    .map(|s| weave::subst(&s.what, values))
                    .unwrap_or_default();
                term::secret(&format!("{name}, {what}"))?
            }
        };
        if value.is_empty() {
            bail!("{name} is empty: nothing runs");
        }
        out.insert(name.clone(), value);
    }
    Ok(out)
}

/// On Linux as root, starts the systemd unit when it is not running; elsewhere says how
/// to run the daemon.
pub fn ensure_daemon() -> Result<()> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let root = cfg!(target_os = "linux") && unsafe { libc::geteuid() } == 0;
    if !root {
        println!("keep `subweave daemon` running on this machine: it applies the service");
        return Ok(());
    }
    let active = std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "subweave"])
        .status()
        .is_ok_and(|s| s.success());
    if active {
        return Ok(());
    }
    println!("starting the daemon: subweave daemon --install");
    let status = std::process::Command::new(std::env::current_exe()?)
        .args(["daemon", "--install"])
        .status()?;
    if !status.success() {
        bail!("subweave daemon --install failed");
    }
    Ok(())
}

/// Prints the service's new journal lines until every goal that is on holds, an ask
/// opens, or Ctrl-C.
pub async fn follow(service: &str, since: Timestamp, mut journal: Tail) -> Result<()> {
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    loop {
        tokio::select! {
            _ = &mut ctrl_c => {
                println!("\nstopped following; {service} keeps running: subweave log -f {service}");
                return Ok(());
            }
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        for raw in journal.lines() {
            let Ok(line) = serde_json::from_str::<Line>(&raw) else {
                continue;
            };
            println!("{}", human(None, &line));
            if line.kind == Kind::Ask
                && let Some(n) = line.n
            {
                let words = line.answers.as_deref().unwrap_or_default().join("|");
                println!(
                    "\n{service} waits for an answer: subweave answer {n} <{}>",
                    if words.is_empty() { "yes|no" } else { &words }
                );
                return Ok(());
            }
        }
        let Some(status) = service::status(service)? else {
            continue;
        };
        let fresh = status.t.parse::<Timestamp>().is_ok_and(|t| t >= since);
        let on: Vec<_> = status
            .goals
            .iter()
            .filter(|g| g.state != GoalState::Off)
            .collect();
        if fresh && !on.is_empty() && on.iter().all(|g| g.state == GoalState::Holds) {
            println!(
                "\n{service}: all {} goals that are on hold. subweave ps shows it.",
                on.len()
            );
            return Ok(());
        }
    }
}

/// From the end of the service's journal now.
pub fn journal_end(service: &str) -> Tail {
    Tail::end(paths::journal(service))
}
