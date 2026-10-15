//! `/metrics`: Prometheus text format 0.0.4, a view of each journal and its fold, and the
//! box's vitals. A scrape reads each journal on from where the last one stopped.

use std::collections::BTreeMap;
use std::fmt::{Display, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};

use super::vitals::Sample;
use super::{ApiError, App, auth};
use crate::daemon::journal::Follower;
use crate::daemon::state::{Act, Event, JobKind, Kind};
use crate::daemon::{epoch, epoch_of};
use crate::service::{self, TASK};

const ACTION_BUCKETS: [f64; 9] = [1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0];
const RECOVERY_BUCKETS: [f64; 10] = [
    10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0, 21600.0, 86400.0,
];
/// Episodes not ended yet that a service's counts keep, the newest.
const OPEN_EPISODES: usize = 1024;

#[derive(Default)]
pub(crate) struct Metrics {
    journals: Mutex<BTreeMap<String, Journal>>,
}

struct Journal {
    /// The journal read so far, folded.
    follower: Follower,
    counts: Counts,
}

/// Cumulative buckets over a family's bounds.
#[derive(Default)]
struct Hist {
    buckets: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Hist {
    fn observe(&mut self, bounds: &[f64], v: f64) {
        self.buckets.resize(bounds.len(), 0);
        for (le, n) in bounds.iter().zip(&mut self.buckets) {
            if v <= *le {
                *n += 1;
            }
        }
        self.sum += v;
        self.count += 1;
    }
}

/// What one journal's lines add up to.
#[derive(Default)]
struct Counts {
    /// (goal, ok) → check lines.
    checks: BTreeMap<(String, &'static str), u64>,
    /// The newest numeric value of each fact.
    facts: BTreeMap<String, f64>,
    /// (action, goal, ok) → action lines.
    actions: BTreeMap<(String, String, bool), u64>,
    action_seconds: BTreeMap<String, Hist>,
    /// Line kind → (seconds, lines) of the timed lines.
    steps: BTreeMap<&'static str, (f64, u64)>,
    /// (job, outcome) → episodes.
    episodes: BTreeMap<(JobKind, &'static str), u64>,
    /// Episode → its job, until a line ends it.
    open: BTreeMap<String, JobKind>,
    /// Goal → when its checks began to fail.
    failing_since: BTreeMap<String, i64>,
    recovery: BTreeMap<String, Hist>,
    asks: u64,
    answers: BTreeMap<&'static str, u64>,
    /// (model, role) → USD.
    cost: BTreeMap<(String, &'static str), f64>,
    /// (model, type) → tokens.
    tokens: BTreeMap<(String, &'static str), u64>,
    calls: BTreeMap<String, u64>,
}

impl Counts {
    /// `decider`: the service's decider, whose spend a line without `usage` carries.
    fn add(&mut self, l: &Event, decider: &str) {
        let t = epoch_of(&l.t);
        let ended = match &l.kind {
            Kind::Action(Act::Ended {
                goal, action, ok, ..
            }) if action != TASK => Some((goal, action, *ok)),
            _ => None,
        };
        if let Some(ms) = l.ms {
            let secs = ms as f64 / 1000.0;
            match ended {
                Some((_, a, _)) => self
                    .action_seconds
                    .entry(a.to_string())
                    .or_default()
                    .observe(&ACTION_BUCKETS, secs),
                None => {
                    let s = self.steps.entry(step(l)).or_default();
                    s.0 += secs;
                    s.1 += 1;
                }
            }
        }
        if let Some((goal, a, ok)) = ended {
            let goal = goal.clone().unwrap_or_default();
            *self.actions.entry((a.clone(), goal, ok)).or_default() += 1;
        }
        match &l.kind {
            Kind::Check { .. } => self.check(l, t),
            Kind::Ask { .. } => self.asks += 1,
            Kind::Answer { .. } => *self.answers.entry("person").or_default() += 1,
            Kind::Withdraw { .. } => *self.answers.entry("withdrawn").or_default() += 1,
            Kind::Gate { ok: true, .. } => *self.answers.entry("always").or_default() += 1,
            _ => {}
        }
        match &l.usage {
            Some(u) => {
                let role = if matches!(l.kind, Kind::Decide { .. }) {
                    "decider"
                } else {
                    "coder"
                };
                if let Some(c) = l.cost_usd {
                    *self.cost.entry((u.model.clone(), role)).or_default() += c;
                }
                for (kind, n) in [
                    ("input", u.input),
                    ("cached", u.cached),
                    ("output", u.output),
                    ("reasoning", u.reasoning),
                ] {
                    *self.tokens.entry((u.model.clone(), kind)).or_default() += n;
                }
                *self.calls.entry(u.model.clone()).or_default() += u64::from(u.calls);
            }
            None => {
                if let Some(c) = l.cost_usd.filter(|c| *c > 0.0) {
                    *self.cost.entry((decider.to_string(), "decider")).or_default() += c;
                }
            }
        }
        self.episode(l);
    }

    fn check(&mut self, l: &Event, t: Option<i64>) {
        let Kind::Check {
            goal: g, ok, facts, ..
        } = &l.kind
        else {
            return;
        };
        let shown = match ok {
            Some(true) => "true",
            Some(false) => "false",
            None => "unknown",
        };
        *self.checks.entry((g.clone(), shown)).or_default() += 1;
        match (ok, t) {
            (Some(false), Some(t)) => {
                self.failing_since.entry(g.clone()).or_insert(t);
            }
            (Some(true), Some(t)) => {
                if let Some(from) = self.failing_since.remove(g) {
                    self.recovery
                        .entry(g.clone())
                        .or_default()
                        .observe(&RECOVERY_BUCKETS, (t - from) as f64);
                }
            }
            _ => {}
        }
        for (name, v) in facts {
            match v.trim().parse::<f64>() {
                Ok(x) if x.is_finite() => {
                    self.facts.insert(name.clone(), x);
                }
                _ => {
                    self.facts.remove(name);
                }
            }
        }
    }

    /// An episode counts once, on the line that ends it.
    fn episode(&mut self, l: &Event) {
        let Some(ep) = &l.ep else { return };
        if let Some(job) = l.job {
            self.open.insert(ep.clone(), job);
            // Ids are UUIDv7: the first is the oldest.
            while self.open.len() > OPEN_EPISODES {
                self.open.pop_first();
            }
        }
        let Some(&job) = self.open.get(ep) else { return };
        if let Some(outcome) = outcome(job, l) {
            self.open.remove(ep);
            *self.episodes.entry((job, outcome)).or_default() += 1;
        }
    }
}

/// The step a timed event ends.
fn step(l: &Event) -> &'static str {
    match &l.kind {
        Kind::Check { .. } => "check",
        Kind::Decide { .. } => "decide",
        Kind::Command { .. } => "command",
        Kind::Learn { .. } => "learn",
        Kind::Action(_) => "task",
        Kind::Note { .. } if l.usage.is_some() => "coder",
        _ => "other",
    }
}

/// How the event ends its episode, if it does: a job's action or task end, a repair's
/// ask or `wait`, a learn's result. A task's ask leaves it open for the answer.
fn outcome(job: JobKind, l: &Event) -> Option<&'static str> {
    match (job, &l.kind) {
        (_, Kind::Action(Act::Ended { ok, .. })) => Some(if *ok { "ok" } else { "failed" }),
        (JobKind::Repair | JobKind::Learn, Kind::Ask { .. }) => Some("asked"),
        (JobKind::Repair, Kind::Decide { text, .. }) if text == "wait" => Some("wait"),
        (JobKind::Learn, Kind::Learn { ok: true, .. }) => Some("learned"),
        (JobKind::Learn, Kind::Note { text, .. }) if text.starts_with("cannot learn ") => {
            Some("failed")
        }
        _ => None,
    }
}

fn job_name(j: JobKind) -> &'static str {
    match j {
        JobKind::Repair => "repair",
        JobKind::Task => "task",
        JobKind::Learn => "learn",
    }
}

/// The exposition text.
#[derive(Default)]
struct Out(String);

impl Out {
    fn family(&mut self, name: &str, kind: &str, help: &str) {
        let _ = writeln!(self.0, "# HELP {name} {help}\n# TYPE {name} {kind}");
    }

    fn put(&mut self, name: &str, labels: &[(&str, &str)], v: impl Display) {
        self.0.push_str(name);
        if !labels.is_empty() {
            self.0.push('{');
            for (k, (key, value)) in labels.iter().enumerate() {
                if k > 0 {
                    self.0.push(',');
                }
                let _ = write!(self.0, "{key}=\"");
                for c in value.chars() {
                    match c {
                        '\\' => self.0.push_str("\\\\"),
                        '"' => self.0.push_str("\\\""),
                        '\n' => self.0.push_str("\\n"),
                        c => self.0.push(c),
                    }
                }
                self.0.push('"');
            }
            self.0.push('}');
        }
        let _ = writeln!(self.0, " {v}");
    }

    fn hist(&mut self, name: &str, labels: &[(&str, &str)], bounds: &[f64], h: &Hist) {
        let bucket = format!("{name}_bucket");
        for (le, n) in bounds.iter().zip(&h.buckets) {
            let le = le.to_string();
            let mut with = labels.to_vec();
            with.push(("le", le.as_str()));
            self.put(&bucket, &with, n);
        }
        let mut with = labels.to_vec();
        with.push(("le", "+Inf"));
        self.put(&bucket, &with, h.count);
        self.put(&format!("{name}_sum"), labels, h.sum);
        self.put(&format!("{name}_count"), labels, h.count);
    }
}

impl Metrics {
    /// Reads what each journal gained, then writes every metric.
    fn render(&self, vitals: Option<Sample>) -> String {
        let names = service::names().unwrap_or_default();
        let mut journals = self.journals.lock().unwrap_or_else(|e| e.into_inner());
        journals.retain(|n, _| names.contains(n));
        for name in &names {
            let j = journals.entry(name.clone()).or_insert_with(|| Journal {
                follower: Follower::start(name),
                counts: Counts::default(),
            });
            let (cut, events) = j.follower.read();
            if cut {
                // The file was cut: its counts start over.
                j.counts = Counts::default();
            }
            let decider = service::spec(name).map(|s| s.decide).unwrap_or_default();
            for (_, e) in &events {
                j.counts.add(e, &decider);
            }
        }
        let now = epoch();
        let mut o = Out::default();

        o.family("subweave_goal_holds", "gauge", "1 when the goal holds, else 0.");
        for (s, j) in journals.iter() {
            for (goal, g) in &j.follower.state.goals {
                let labels = [("service", s.as_str()), ("goal", goal.as_str())];
                o.put("subweave_goal_holds", &labels, u8::from(g.holds()));
            }
        }
        o.family(
            "subweave_goal_state_seconds",
            "gauge",
            "Seconds the goal has been holding, suspect or failing.",
        );
        for (s, j) in journals.iter() {
            for (goal, g) in &j.follower.state.goals {
                let Some(since) = g.since().and_then(epoch_of) else {
                    continue;
                };
                let labels = [("service", s.as_str()), ("goal", goal.as_str()), ("state", g.name())];
                o.put("subweave_goal_state_seconds", &labels, (now - since).max(0));
            }
        }
        o.family(
            "subweave_checks_total",
            "counter",
            "Check lines: a goal's result when it changed, or hourly.",
        );
        for (s, j) in journals.iter() {
            for ((goal, ok), n) in &j.counts.checks {
                let labels = [("service", s.as_str()), ("goal", goal.as_str()), ("ok", *ok)];
                o.put("subweave_checks_total", &labels, n);
            }
        }
        o.family(
            "subweave_fact_value",
            "gauge",
            "A fact's newest value in a check line, numeric facts only.",
        );
        for (s, j) in journals.iter() {
            for (fact, v) in &j.counts.facts {
                let labels = [("service", s.as_str()), ("fact", fact.as_str())];
                o.put("subweave_fact_value", &labels, v);
            }
        }
        o.family(
            "subweave_actions_total",
            "counter",
            "Actions that ended; ok: the goal held within the action's WITHIN.",
        );
        for (s, j) in journals.iter() {
            for ((action, goal, ok), n) in &j.counts.actions {
                let ok = if *ok { "true" } else { "false" };
                let labels = [
                    ("service", s.as_str()),
                    ("action", action.as_str()),
                    ("goal", goal.as_str()),
                    ("ok", ok),
                ];
                o.put("subweave_actions_total", &labels, n);
            }
        }
        o.family(
            "subweave_action_duration_seconds",
            "histogram",
            "An action's wall time: its steps and its WITHIN wait.",
        );
        for (s, j) in journals.iter() {
            for (action, h) in &j.counts.action_seconds {
                let labels = [("service", s.as_str()), ("action", action.as_str())];
                o.hist("subweave_action_duration_seconds", &labels, &ACTION_BUCKETS, h);
            }
        }
        o.family(
            "subweave_step_duration_seconds",
            "summary",
            "Wall time of timed journal steps: command, check, decide, learn, coder, task.",
        );
        for (s, j) in journals.iter() {
            for (kind, (sum, n)) in &j.counts.steps {
                let labels = [("service", s.as_str()), ("kind", *kind)];
                o.put("subweave_step_duration_seconds_sum", &labels, sum);
                o.put("subweave_step_duration_seconds_count", &labels, n);
            }
        }
        o.family(
            "subweave_episodes_total",
            "counter",
            "Episodes that ended, by job (repair, task, learn) and outcome.",
        );
        for (s, j) in journals.iter() {
            for ((job, outcome), n) in &j.counts.episodes {
                let labels = [
                    ("service", s.as_str()),
                    ("kind", job_name(*job)),
                    ("outcome", *outcome),
                ];
                o.put("subweave_episodes_total", &labels, n);
            }
        }
        o.family(
            "subweave_goal_recovery_seconds",
            "histogram",
            "Seconds from a goal's first failing check to the check that holds again.",
        );
        for (s, j) in journals.iter() {
            for (goal, h) in &j.counts.recovery {
                let labels = [("service", s.as_str()), ("goal", goal.as_str())];
                o.hist("subweave_goal_recovery_seconds", &labels, &RECOVERY_BUCKETS, h);
            }
        }
        o.family("subweave_asks_open", "gauge", "Asks waiting for an answer.");
        for (s, j) in journals.iter() {
            let open = j.follower.state.open().count();
            o.put("subweave_asks_open", &[("service", s.as_str())], open);
        }
        o.family("subweave_asks_total", "counter", "Asks raised.");
        for (s, j) in journals.iter() {
            o.put("subweave_asks_total", &[("service", s.as_str())], j.counts.asks);
        }
        o.family(
            "subweave_answers_total",
            "counter",
            "Asks settled: by a person, withdrawn as the goal held again, or a rule passed by a kept always.",
        );
        for (s, j) in journals.iter() {
            for (by, n) in &j.counts.answers {
                let labels = [("service", s.as_str()), ("by_kind", *by)];
                o.put("subweave_answers_total", &labels, n);
            }
        }
        o.family(
            "subweave_model_cost_usd_total",
            "counter",
            "Model spend in USD, by model and role.",
        );
        for (s, j) in journals.iter() {
            for ((model, role), usd) in &j.counts.cost {
                let labels = [("service", s.as_str()), ("model", model.as_str()), ("role", *role)];
                o.put("subweave_model_cost_usd_total", &labels, usd);
            }
        }
        o.family(
            "subweave_model_tokens_total",
            "counter",
            "Model tokens: input (cached is part of it), output (reasoning is part of it).",
        );
        for (s, j) in journals.iter() {
            for ((model, kind), n) in &j.counts.tokens {
                let labels = [("service", s.as_str()), ("model", model.as_str()), ("type", *kind)];
                o.put("subweave_model_tokens_total", &labels, n);
            }
        }
        o.family("subweave_model_calls_total", "counter", "Model requests.");
        for (s, j) in journals.iter() {
            for (model, n) in &j.counts.calls {
                let labels = [("service", s.as_str()), ("model", model.as_str())];
                o.put("subweave_model_calls_total", &labels, n);
            }
        }
        if let Some(v) = vitals {
            o.family("subweave_box_cpu_ratio", "gauge", "CPU busy share of the box.");
            o.put("subweave_box_cpu_ratio", &[], v.cpu_pct / 100.0);
            o.family(
                "subweave_box_memory_used_ratio",
                "gauge",
                "Memory share in use, MemAvailable counted free.",
            );
            o.put("subweave_box_memory_used_ratio", &[], v.mem_pct / 100.0);
            o.family("subweave_box_load1", "gauge", "One-minute load average.");
            o.put("subweave_box_load1", &[], v.load1);
            o.family(
                "subweave_box_disk_used_ratio",
                "gauge",
                "Used share of each real filesystem, as df counts it.",
            );
            for (mount, pct) in &v.disk_pct {
                o.put("subweave_box_disk_used_ratio", &[("mount", mount.as_str())], pct / 100.0);
            }
        }
        o.0
    }
}

pub(crate) async fn get(
    State(app): State<Arc<App>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    auth::scraper(&app, &headers, peer).await?;
    let a = app.clone();
    let text = tokio::task::spawn_blocking(move || a.metrics.render(a.vitals.last()))
        .await
        .map_err(|e| ApiError::internal(e.into()))?;
    Ok((
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        text,
    )
        .into_response())
}
