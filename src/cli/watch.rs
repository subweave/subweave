use std::collections::BTreeSet;
use std::process::{ExitCode, Stdio};
use std::time::Duration;

use anyhow::{Result, bail};
use jiff::Timestamp;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::log::human;
use crate::daemon::state::{Event, Kind};
use crate::remote;

/// Follows every box in `boxes.json` and shows its asks and warnings as notifications.
pub async fn run() -> Result<ExitCode> {
    let hosts: BTreeSet<String> = remote::boxes()?.into_values().collect();
    if hosts.is_empty() {
        bail!("no boxes yet: subweave run <weave> --on <user@host> adds one");
    }
    println!(
        "watching {}",
        hosts.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    let since = Timestamp::now();
    let mut tasks = tokio::task::JoinSet::new();
    for host in hosts {
        tasks.spawn(follow(host, since));
    }
    while tasks.join_next().await.is_some() {}
    Ok(ExitCode::SUCCESS)
}

/// One box's journals over ssh; connects again 10 s after the connection ends.
async fn follow(host: String, since: Timestamp) {
    loop {
        if let Err(e) = follow_once(&host, since).await {
            eprintln!("{host}: {e:#}");
        }
        eprintln!("{host}: connection ended; again in 10 s");
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

async fn follow_once(host: &str, since: Timestamp) -> Result<()> {
    let mut child = tokio::process::Command::from(remote::ssh(host, false))
        .arg("subweave log -f --all --json")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let Some(stdout) = child.stdout.take() else {
        bail!("no output from ssh");
    };
    let mut lines = BufReader::new(stdout).lines();
    while let Some(raw) = lines.next_line().await? {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let service = value["service"].as_str().unwrap_or("-").to_string();
        let Ok(e) = serde_json::from_value::<Event>(value) else {
            continue;
        };
        if e.t.parse::<Timestamp>().is_ok_and(|t| t < since) {
            continue;
        }
        let text = e.text().unwrap_or("").trim();
        let note = match &e.kind {
            Kind::Ask { n, card, .. } => {
                let words = card.words().join("|");
                Some((
                    format!("ask #{n}: {service} on {host}"),
                    format!("{text} — subweave answer {n} <{words}>"),
                ))
            }
            Kind::Message { .. } => text
                .strip_prefix("warning: ")
                .map(|title| (format!("warning: {service} on {host}"), title.to_string())),
            _ => None,
        };
        if let Some((title, body)) = note {
            println!("{host}  {}", human(Some(&service), &e));
            notify(&title, &body).await;
        }
    }
    child.wait().await?;
    Ok(())
}

async fn notify(title: &str, body: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        // The text goes in as argv, so quotes in it cannot end the script.
        let mut c = tokio::process::Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            body,
        ]);
        c
    } else {
        let mut c = tokio::process::Command::new("notify-send");
        c.args([title, body]);
        c
    };
    if let Err(e) = cmd.stdout(Stdio::null()).status().await {
        eprintln!("notification failed: {e}");
    }
}
