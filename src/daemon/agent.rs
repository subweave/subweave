//! An `agent` channel: its command runs on this box once per escalation, with the
//! escalation on stdin. It settles the ask as any agent does, with `subweave answer`.

use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;

use crate::channel::{self, Channel, Message, escalation};
use crate::paths;
use crate::service::{Kind, Line};

use super::{Out, Shared, drain, journal, kill_group, ms, now, redact};

/// An agent still running after this long is killed.
const LIMIT: Duration = Duration::from_secs(30 * 60);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs the channel's command for the escalation `msg`, once the service's earlier
/// agent, if any, has exited. Env: `SUBWEAVE_SERVICE`, `SUBWEAVE_ASK` and `SUBWEAVE_BY`
/// (the channel's name, which its `subweave answer` carries); cwd: the service's
/// directory. Its output, `secrets` redacted, goes to `escalations/<n>.log` there, and a
/// journal note says how it ended.
pub(crate) fn start(
    shared: Arc<Shared>,
    ch: &Channel,
    msg: &Message,
    secrets: Vec<(String, String)>,
) {
    let (Some(command), Some(ask)) = (ch.target.clone(), msg.ask.as_ref()) else {
        return;
    };
    let n = ask.n;
    let name = ch.name.clone();
    let service = msg.service.clone();
    let body = escalation::body(msg, None, true);
    let turn = lock(&shared.agent_turns)
        .entry(service.clone())
        .or_default()
        .clone();
    tokio::spawn(async move {
        let _turn = turn.lock().await;
        // Settled or withdrawn while it waited for its turn.
        if lock(&shared.answered).contains(&n) {
            return;
        }
        let dir = paths::service(&service);
        let started = Instant::now();
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-c", &command])
            .current_dir(&dir)
            .env("SUBWEAVE_SERVICE", &service)
            .env("SUBWEAVE_ASK", n.to_string())
            .env("SUBWEAVE_BY", &name)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let text = format!("escalated #{n} to {name}: cannot start sh: {e}");
                note(&service, n, &name, text, started);
                return;
            }
        };
        let pid = child.id();
        if let Some(pid) = pid {
            lock(&shared.agents).insert(n, pid);
        }
        let (out, out_task) = drain(child.stdout.take());
        let (err, err_task) = drain(child.stderr.take());
        if let Some(mut stdin) = child.stdin.take() {
            // In its own task: an agent that never reads stdin must not stall the wait.
            tokio::spawn(async move {
                let _ = stdin.write_all(body.as_bytes()).await;
            });
        }
        let ended = match tokio::time::timeout(LIMIT, child.wait()).await {
            Ok(Ok(status)) => match status.code() {
                Some(code) => format!("exit {code}"),
                None => "killed".to_owned(),
            },
            Ok(Err(e)) => format!("lost: {e}"),
            Err(_) => {
                if let Some(pid) = pid {
                    kill_group(pid);
                }
                let _ = child.kill().await;
                format!("killed after {} min", LIMIT.as_secs() / 60)
            }
        };
        lock(&shared.agents).remove(&n);
        let grace = Duration::from_secs(1);
        let _ = tokio::time::timeout(grace, out_task).await;
        let _ = tokio::time::timeout(grace, err_task).await;
        let take = |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&lock(b)).into_owned();
        let output = Out {
            code: None,
            stdout: take(&out),
            stderr: take(&err),
        }
        .both();
        let log = dir.join("escalations").join(format!("{n}.log"));
        let saved = channel::write_private(&log, redact(&output, &secrets).as_bytes());
        let kept = match saved {
            Ok(()) => format!("output in escalations/{n}.log"),
            Err(e) => format!("its output was not kept: {e:#}"),
        };
        note(
            &service,
            n,
            &name,
            format!("escalated #{n} to {name}: {ended}; {kept}"),
            started,
        );
    });
}

/// A withdrawn escalation needs its agent no more: the process group goes.
pub(crate) fn kill(shared: &Shared, n: u64) {
    if let Some(pid) = lock(&shared.agents).get(&n) {
        kill_group(*pid);
    }
}

fn note(service: &str, n: u64, by: &str, text: String, started: Instant) {
    journal::append(
        service,
        &Line {
            t: now(),
            kind: Kind::Note,
            text: Some(text),
            n: Some(n),
            by: Some(by.to_owned()),
            ms: ms(started),
            ..Default::default()
        },
    );
}
