//! What the dashboard reads: the files the daemon keeps, through the CLI's helpers, and
//! each service's state, folded from its journal.

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::Query;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ApiError, Svc};
use crate::cli::stats;
use crate::daemon::journal;
use crate::daemon::state::{Event, LastDrill, ServiceState};
use crate::service::{self, Spec};
use crate::{paths, weave};

const JOURNAL_PAGE: usize = 100;
const JOURNAL_MAX: usize = 500;

/// A journal event with its index in the file.
#[derive(Serialize)]
pub(crate) struct Numbered<'a> {
    pub i: u64,
    #[serde(flatten)]
    pub line: &'a Event,
}

/// A service's state as the dashboard shows it.
#[derive(Serialize, PartialEq)]
pub(crate) struct Status {
    pub service: String,
    pub goals: Vec<GoalStatus>,
    /// What runs now.
    pub repair: Option<String>,
    /// Open asks, by number.
    pub asks: Vec<u64>,
    pub spend_month_usd: f64,
    pub decide: String,
    pub code: Vec<String>,
    pub stopped: bool,
    /// The drill that ran last, and how it went.
    pub drill: Option<LastDrill>,
}

#[derive(Serialize, PartialEq)]
pub(crate) struct GoalStatus {
    pub name: String,
    pub state: &'static str,
    /// When it last changed state, when its state keeps that.
    pub since: Option<String>,
}

/// `None` until the daemon's loop journals the service's goals.
pub(crate) fn status(name: &str, spec: &Spec, st: &ServiceState) -> Option<Status> {
    if st.goals.is_empty() {
        return None;
    }
    Some(Status {
        service: name.to_string(),
        goals: st
            .goals
            .iter()
            .map(|(g, s)| GoalStatus {
                name: g.clone(),
                state: s.name(),
                since: s.since().map(str::to_string),
            })
            .collect(),
        repair: if spec.stopped { None } else { st.running() },
        asks: st.open().map(|e| e.n).collect(),
        spend_month_usd: st.spend_in(&crate::daemon::now()[..7]),
        decide: spec.decide.clone(),
        code: spec.code.clone(),
        stopped: spec.stopped,
        drill: st.last_drill(),
    })
}

/// The service's spec, or 404.
pub(crate) fn spec(s: &str) -> Result<Spec, ApiError> {
    if !paths::service_json(s).is_file() {
        return Err(ApiError::not_found(format!("no service {s} here")));
    }
    service::spec(s).map_err(ApiError::internal)
}

/// `running`, `stopped`, or `starting` before the daemon's loop journaled the goals.
fn state(spec: &Spec, status: &Option<Status>) -> &'static str {
    if spec.stopped {
        "stopped"
    } else if status.is_none() {
        "starting"
    } else {
        "running"
    }
}

pub(crate) async fn services() -> Result<Json<Value>, ApiError> {
    let mut out = Vec::new();
    for name in service::names().map_err(ApiError::internal)? {
        let spec = service::spec(&name).map_err(ApiError::internal)?;
        let status = status(&name, &spec, &journal::read(&name));
        out.push(json!({
            "name": name,
            "weave": spec.weave,
            "request": spec.request,
            "stopped": spec.stopped,
            "state": state(&spec, &status),
            "status": status,
        }));
    }
    Ok(Json(Value::Array(out)))
}

pub(crate) async fn service(Svc(s): Svc) -> Result<Json<Value>, ApiError> {
    let spec = spec(&s)?;
    let st = journal::read(&s);
    let weavefile = std::fs::read_to_string(paths::weavefile(&s)).ok();
    // The parsed weave as the box reads it: `$name` replaced. `weavefile` stays raw.
    let weave = weavefile.as_deref().and_then(|text| {
        let mut w = weave::parse(&s, text).ok()?;
        w.substitute(&spec.args);
        Some(w)
    });
    let readme = std::fs::read_to_string(paths::readme(&s)).ok();
    let learned = service::learned(&s).map_err(ApiError::internal)?;
    Ok(Json(json!({
        "status": status(&s, &spec, &st),
        "asks": journal::asks(&s, &st),
        "spec": spec,
        "weave": weave,
        "weavefile": weavefile,
        "readme": readme,
        "learned": learned,
    })))
}

#[derive(Deserialize)]
pub(crate) struct JournalQuery {
    before: Option<u64>,
    limit: Option<usize>,
    kind: Option<String>,
    goal: Option<String>,
    action: Option<String>,
    q: Option<String>,
    /// With a filter, also every other line of each matching line's episode, so the
    /// timeline can show the whole episode.
    #[serde(default)]
    whole: bool,
}

pub(crate) async fn journal(
    Svc(s): Svc,
    Query(q): Query<JournalQuery>,
) -> Result<Json<Value>, ApiError> {
    spec(&s)?;
    let limit = q.limit.unwrap_or(JOURNAL_PAGE).clamp(1, JOURNAL_MAX);
    let kinds: Option<BTreeSet<&str>> = q
        .kind
        .as_deref()
        .filter(|k| !k.is_empty())
        .map(|k| k.split(',').map(str::trim).collect());
    let needle = q.q.as_deref().filter(|t| !t.is_empty()).map(str::to_lowercase);
    let lines = journal::numbered(&s);
    let hit = |l: &Event| {
        kinds.as_ref().is_none_or(|k| k.contains(l.kind.name()))
            && q.goal.as_deref().is_none_or(|g| l.goal() == Some(g))
            && q.action.as_deref().is_none_or(|a| l.action() == Some(a))
            && needle.as_ref().is_none_or(|n| matches(l, n))
    };
    let eps: BTreeSet<&str> = if q.whole {
        lines
            .iter()
            .filter(|(_, l)| hit(l))
            .filter_map(|(_, l)| l.ep.as_deref())
            .collect()
    } else {
        BTreeSet::new()
    };
    let mut hits = lines.iter().rev().filter(|(i, l)| {
        q.before.is_none_or(|b| *i < b)
            && (hit(l) || l.ep.as_deref().is_some_and(|e| eps.contains(e)))
    });
    let page: Vec<Numbered> = hits
        .by_ref()
        .take(limit)
        .map(|(i, line)| Numbered { i: *i, line })
        .collect();
    let more = hits.next().is_some();
    Ok(Json(json!({"lines": page, "more": more})))
}

/// The search box: the text, the goal, the action, a fact or the output, any case.
fn matches(l: &Event, needle: &str) -> bool {
    let has = |s: &str| s.to_lowercase().contains(needle);
    l.text().is_some_and(has)
        || l.goal().is_some_and(has)
        || l.action().is_some_and(has)
        || l.kind.output().is_some_and(has)
        || l.by.as_deref().is_some_and(has)
        || l
            .kind
            .facts()
            .is_some_and(|f| f.iter().any(|(k, v)| has(k) || has(v)))
}

#[derive(Deserialize)]
pub(crate) struct Range {
    range: Option<String>,
}

/// Numeric fact values from `check` events, the mean of each step, as `stats` reads them.
pub(crate) async fn metrics(
    Svc(s): Svc,
    Query(q): Query<Range>,
) -> Result<Json<Value>, ApiError> {
    spec(&s)?;
    let (step, steps) = match q.range.as_deref().unwrap_or("24h") {
        "24h" => (600, 144),
        "7d" => (3600, 168),
        "30d" => (4 * 3600, 180),
        other => return Err(ApiError::bad(format!("range {other:?}: 24h, 7d or 30d"))),
    };
    let now = crate::daemon::epoch();
    let lines = journal::events(&s);
    let series = stats::series(&lines, now, step, steps);
    let t: Vec<String> = (0..steps as i64)
        .filter_map(|k| jiff::Timestamp::from_second(now - (steps as i64 - k) * step).ok())
        .map(|ts| ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .collect();
    let facts: serde_json::Map<String, Value> = series
        .into_iter()
        .map(|(name, s)| (name, json!(s.means)))
        .collect();
    Ok(Json(json!({"step_s": step, "t": t, "facts": facts})))
}
