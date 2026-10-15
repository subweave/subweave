//! What the person does from the dashboard: each a Command in the inbox, checked as the
//! CLI checks it, carrying `by: "dash@<ip>"`.

use std::collections::BTreeMap;
use std::net::SocketAddr;

use axum::Json;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};

use super::read::spec;
use super::{ApiError, Body, Svc, auth};
use crate::cli::{act, inbox, learned, tell};
use crate::plan::Level;
use crate::service::{self, Command, Spec};

type Accepted = Result<StatusCode, ApiError>;

fn put(cmd: Command) -> Accepted {
    service::put(&cmd).map_err(ApiError::internal)?;
    Ok(StatusCode::ACCEPTED)
}

/// The service, which must not be stopped for work to run.
fn running(s: &str) -> Result<Spec, ApiError> {
    let spec = spec(s)?;
    if spec.stopped {
        return Err(ApiError::conflict(format!("{s} is stopped: start it first")));
    }
    Ok(spec)
}

#[derive(Deserialize)]
pub(crate) struct Answer {
    n: u64,
    word: String,
}

pub(crate) async fn answer(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(b): Body<Answer>,
) -> Accepted {
    spec(&s)?;
    let lines = service::journal(&s).map_err(ApiError::internal)?;
    if !service::asks(&s, &lines).iter().any(|a| a.n == b.n) {
        return Err(ApiError::not_found(format!("{s} has no open escalation #{}", b.n)));
    }
    inbox::check_answer(b.n, &b.word).map_err(|e| ApiError::bad(format!("{e:#}")))?;
    put(Command::Answer {
        n: b.n,
        word: b.word,
        by: auth::by(&headers, peer),
        note: None,
    })
}

#[derive(Deserialize)]
pub(crate) struct Do {
    action: String,
}

pub(crate) async fn do_action(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(b): Body<Do>,
) -> Accepted {
    running(&s)?;
    act::check_action(&s, &b.action).map_err(|e| ApiError::bad(format!("{e:#}")))?;
    put(Command::Do {
        service: s,
        action: b.action,
        by: auth::by(&headers, peer),
    })
}

#[derive(Deserialize)]
pub(crate) struct Task {
    text: String,
}

pub(crate) async fn task(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(b): Body<Task>,
) -> Accepted {
    running(&s)?;
    let text = b.text.trim();
    if text.is_empty() {
        return Err(ApiError::bad("say what to do"));
    }
    put(Command::Task {
        service: s,
        text: text.to_string(),
        by: auth::by(&headers, peer),
    })
}

#[derive(Deserialize)]
pub(crate) struct Tell {
    request: String,
    #[serde(default)]
    apply: bool,
}

/// The `tell` check; with `apply` and nothing in the way, the change goes to the daemon.
/// A change that turns on a SECRET the daemon does not hold needs a terminal to type it.
pub(crate) async fn tell(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(b): Body<Tell>,
) -> Result<Json<Value>, ApiError> {
    spec(&s)?;
    let request = b.request.trim();
    if request.is_empty() {
        return Err(ApiError::bad("say what to change"));
    }
    let c = tell::check(&s, request, None)
        .await
        .map_err(|e| ApiError::bad(format!("{e:#}")))?;
    let mut findings: Vec<Value> = c
        .plan
        .findings
        .iter()
        .map(|f| json!({"level": f.level.as_str(), "text": format!("{}: {}", f.subject, f.text)}))
        .collect();
    for name in &c.missing {
        findings.push(json!({
            "level": "error",
            "text": format!("{name}: the change needs this SECRET; type it at a terminal with subweave tell {s}"),
        }));
    }
    let ready = c.plan.count(Level::Error) == 0 && c.missing.is_empty();
    if b.apply && ready {
        service::put(&Command::Tell {
            service: s,
            request: request.to_string(),
            args: c.plan.values.clone(),
            secrets: BTreeMap::new(),
            decide: None,
            code: None,
            by: auth::by(&headers, peer),
        })
        .map_err(ApiError::internal)?;
    }
    Ok(Json(json!({
        "ready": ready,
        "findings": findings,
        "args_before": c.before,
        "args_after": c.plan.values,
    })))
}

pub(crate) async fn stop(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Accepted {
    if spec(&s)?.stopped {
        return Err(ApiError::conflict(format!("{s} is stopped already")));
    }
    put(Command::Stop {
        service: s,
        by: auth::by(&headers, peer),
    })
}

pub(crate) async fn start(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Accepted {
    if !spec(&s)?.stopped {
        return Err(ApiError::conflict(format!("{s} runs already")));
    }
    put(Command::Start {
        service: s,
        by: auth::by(&headers, peer),
    })
}

#[derive(Deserialize)]
pub(crate) struct Forget {
    name: String,
}

pub(crate) async fn forget(
    Svc(s): Svc,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Body(b): Body<Forget>,
) -> Accepted {
    spec(&s)?;
    learned::check_forget(&s, &b.name).map_err(|e| ApiError::not_found(format!("{e:#}")))?;
    put(Command::Forget {
        service: s,
        name: b.name,
        by: auth::by(&headers, peer),
    })
}
