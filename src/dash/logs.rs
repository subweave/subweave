//! Log streams: each FOLLOW fact (its recent values from `check` lines, then the live
//! lines of its command, run here with the service's environment and redaction) and the
//! box's systemd journal. A stream's process ends when its client leaves; a session
//! holds at most 8.

use std::convert::Infallible;
use std::process::Stdio;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

use super::read::spec;
use super::{ApiError, App, Svc, auth};
use crate::daemon::{bash, command_env, kill_group, now, redact};
use crate::service::{self, Kind};
use crate::weave::{FactSource, Weave};
use crate::paths;

const BOX: &str = "box";
const BACKLOG: usize = 300;

fn weave_of(s: &str) -> Result<Weave, ApiError> {
    let text = std::fs::read_to_string(paths::weavefile(s)).map_err(|e| ApiError::internal(e.into()))?;
    crate::weave::parse(s, &text)
        .map_err(|_| ApiError::conflict(format!("the Weavefile of {s} does not parse")))
}

pub(crate) async fn list(Svc(s): Svc) -> Result<Json<Value>, ApiError> {
    spec(&s)?;
    let w = weave_of(&s)?;
    let mut out: Vec<Value> = w
        .facts
        .iter()
        .filter(|f| f.follow)
        .map(|f| {
            let title = match &f.source {
                FactSource::Command(c) | FactSource::Quoted(c) => c.clone(),
            };
            json!({"name": f.name, "title": title, "source": "fact"})
        })
        .collect();
    out.push(json!({"name": BOX, "title": "Box journal", "source": "box"}));
    Ok(Json(Value::Array(out)))
}

#[derive(Deserialize)]
pub(crate) struct BoxQuery {
    unit: Option<String>,
    priority: Option<String>,
}

/// `error` for an error, `warning` for a warning, else `info`, by the words in it.
fn level_of(text: &str) -> &'static str {
    let up = text.to_ascii_uppercase();
    if ["ERROR", "FATAL", "PANIC", "CRIT", "EMERG", "ALERT", "FAIL"]
        .iter()
        .any(|w| up.contains(w))
    {
        "error"
    } else if up.contains("WARN") {
        "warning"
    } else {
        "info"
    }
}

fn line(t: &str, text: &str, level: &str) -> Event {
    Event::default()
        .event("line")
        .data(json!({"t": t, "text": text, "level": level}).to_string())
}

/// Sends each line of a pipe, redacted, with `parse` making it an event.
fn pump<R, F>(
    pipe: Option<R>,
    tx: mpsc::Sender<Event>,
    secrets: Vec<(String, String)>,
    parse: F,
) -> Option<tokio::task::JoinHandle<()>>
where
    R: AsyncRead + Unpin + Send + 'static,
    F: Fn(&str) -> Option<(String, String, &'static str)> + Send + 'static,
{
    let pipe = pipe?;
    Some(tokio::spawn(async move {
        let mut lines = BufReader::new(pipe).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            let Some((t, text, level)) = parse(&l) else {
                continue;
            };
            if tx.send(line(&t, &redact(&text, &secrets), level)).await.is_err() {
                break;
            }
        }
    }))
}

/// A `journalctl -o json` line: its time, `unit: message` and its priority's level.
fn journal_line(raw: &str) -> Option<(String, String, &'static str)> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let message = match &v["MESSAGE"] {
        Value::String(s) => s.clone(),
        // Binary messages come as an array of bytes.
        Value::Array(b) => {
            let bytes: Vec<u8> = b.iter().filter_map(|x| x.as_u64().map(|n| n as u8)).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
        _ => return None,
    };
    let who = v["SYSLOG_IDENTIFIER"]
        .as_str()
        .or(v["_SYSTEMD_UNIT"].as_str())
        .unwrap_or("kernel");
    let level = match v["PRIORITY"].as_str().and_then(|p| p.parse::<u8>().ok()) {
        Some(0..=3) => "error",
        Some(4) => "warning",
        _ => "info",
    };
    let t = v["__REALTIME_TIMESTAMP"]
        .as_str()
        .and_then(|us| us.parse::<i64>().ok())
        .and_then(|us| jiff::Timestamp::from_microsecond(us).ok())
        .map(|ts| ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(now);
    Some((t, format!("{who}: {message}"), level))
}

fn valid_unit(u: &str) -> bool {
    !u.is_empty()
        && !u.starts_with('-')
        && u
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._:-\\".contains(&b))
}

fn valid_priority(p: &str) -> bool {
    matches!(
        p,
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "emerg" | "alert" | "crit" | "err"
            | "warning" | "notice" | "info" | "debug"
    )
}

#[derive(Deserialize)]
pub(crate) struct LogName {
    name: String,
}

/// Every value a box journal line could leak: each service's SECRETs, and the channel
/// and provider keys. Longest first.
fn box_secrets() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = Vec::new();
    for name in service::names().unwrap_or_default() {
        if let Ok(spec) = service::spec(&name) {
            all.extend(command_env(&name, &spec).1);
        }
    }
    all.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.cmp(b)));
    all.dedup();
    all
}

pub(crate) async fn stream(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Svc(s): Svc,
    Path(LogName { name }): Path<LogName>,
    Query(q): Query<BoxQuery>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let spec = spec(&s)?;
    let slot = auth::stream_slot(&app, &headers)?;
    let (env, secrets) = if name == BOX {
        (Default::default(), box_secrets())
    } else {
        command_env(&s, &spec)
    };
    let (tx, rx) = mpsc::channel::<Event>(256);
    let mut backlog: Vec<Event> = Vec::new();
    let child = if name == BOX {
        let mut cmd = tokio::process::Command::new("journalctl");
        cmd.args(["-f", "-n", "300", "-o", "json"]);
        if let Some(u) = q.unit.as_deref().filter(|u| !u.is_empty()) {
            if !valid_unit(u) {
                return Err(ApiError::bad(format!("{u:?} is no unit name")));
            }
            cmd.args(["-u", u]);
        }
        if let Some(p) = q.priority.as_deref().filter(|p| !p.is_empty()) {
            if !valid_priority(p) {
                return Err(ApiError::bad(format!("{p:?} is no priority: 0 to 7, or err, warning, info")));
            }
            cmd.args(["-p", p]);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        cmd.spawn().map(|c| (c, true))
    } else {
        let w = weave_of(&s)?;
        let Some(fact) = w.fact(&name).filter(|f| f.follow) else {
            return Err(ApiError::not_found(format!("{s} has no FOLLOW fact {name}")));
        };
        let command = match &fact.source {
            FactSource::Command(c) => c.clone(),
            FactSource::Quoted(_) => service::learned(&s)
                .map_err(ApiError::internal)?
                .facts
                .get(&name)
                .map(|l| l.command.clone())
                .ok_or_else(|| {
                    ApiError::conflict(format!("this box has not learned the command of {name} yet"))
                })?,
        };
        let lines = service::journal(&s).map_err(ApiError::internal)?;
        let mut recent: Vec<(String, String)> = Vec::new();
        for l in lines.iter().filter(|l| l.kind == Kind::Check) {
            if let Some(v) = l.facts.as_ref().and_then(|f| f.get(&name)) {
                recent.extend(
                    v.lines()
                        .filter(|x| !x.trim().is_empty())
                        .map(|x| (l.t.clone(), x.to_string())),
                );
            }
        }
        let skip = recent.len().saturating_sub(BACKLOG);
        backlog = recent[skip..]
            .iter()
            .map(|(t, x)| line(t, x, level_of(x)))
            .collect();
        bash(&command, &env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map(|c| (c, false))
    };
    let (mut child, journal) = match child {
        Ok(c) => c,
        Err(e) => {
            let what = if name == BOX { "journalctl" } else { "bash" };
            let end = Event::default()
                .event("end")
                .data(json!({"text": format!("{what}: {e}")}).to_string());
            let events: Vec<Event> = backlog.into_iter().chain([end]).collect();
            let s = stream::iter(events).map(Ok).left_stream();
            return Ok(Sse::new(s).keep_alive(KeepAlive::default()));
        }
    };
    let pid = child.id();
    let plain = |l: &str| Some((now(), l.to_string(), level_of(l)));
    let out = if journal {
        pump(child.stdout.take(), tx.clone(), secrets.clone(), journal_line)
    } else {
        pump(child.stdout.take(), tx.clone(), secrets.clone(), plain)
    };
    let err = pump(child.stderr.take(), tx.clone(), secrets, plain);
    // Only this task reaps the child, and it kills the group first when the client
    // leaves: a group is never killed after its leader was reaped and its pid reused.
    tokio::spawn(async move {
        let status = tokio::select! {
            s = child.wait() => s,
            () = tx.closed() => {
                if let Some(pid) = pid {
                    kill_group(pid);
                }
                child.wait().await
            }
        };
        let drained = async {
            for p in [out, err].into_iter().flatten() {
                let _ = p.await;
            }
        };
        // The end is said after the last of the child's lines.
        tokio::select! {
            () = drained => {}
            () = tx.closed() => return,
        }
        let text = match status {
            Ok(s) => format!("the command ended: {s}"),
            Err(e) => format!("the command ended: {e}"),
        };
        let _ = tx
            .send(Event::default().event("end").data(json!({"text": text}).to_string()))
            .await;
    });
    let live = stream::unfold((rx, slot), |(mut rx, slot)| async move {
        let ev = rx.recv().await?;
        Some((ev, (rx, slot)))
    });
    let all = stream::iter(backlog).chain(live).map(Ok).right_stream();
    Ok(Sse::new(all).keep_alive(KeepAlive::default()))
}
