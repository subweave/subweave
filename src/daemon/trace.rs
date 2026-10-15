//! OpenTelemetry traces over OTLP (http/protobuf), on only when `OTEL_EXPORTER_OTLP_ENDPOINT`
//! or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` is set. One trace per episode, its id the
//! episode's: a root span for the job and a child span for each of its journal lines, which
//! ends as the line is written and starts its `ms` earlier.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{
    Span as _, SpanBuilder, SpanContext, SpanId, Status, TraceContextExt, TraceFlags, TraceId,
    TraceState, Tracer as _, TracerProvider as _,
};
use opentelemetry::{Context, KeyValue};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};

use crate::service::{JobKind, Kind, Line, TASK};

struct Traces {
    provider: SdkTracerProvider,
    tracer: SdkTracer,
    /// Episode → what its root span gathers until the episode ends.
    open: Mutex<HashMap<String, Root>>,
}

struct Root {
    service: String,
    job: Option<JobKind>,
    start: SystemTime,
    end: SystemTime,
    goal: Option<String>,
    action: Option<String>,
    ok: Option<bool>,
    cost_usd: f64,
}

/// `None` when the environment names no endpoint, or the exporter cannot start.
static TRACES: LazyLock<Option<Traces>> = LazyLock::new(|| {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    if !set("OTEL_EXPORTER_OTLP_ENDPOINT") && !set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
        return None;
    }
    let exporter = match opentelemetry_otlp::SpanExporter::builder().with_http().build() {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "no OTLP trace exporter");
            return None;
        }
    };
    let resource = Resource::builder()
        .with_service_name("subweave")
        .with_attribute(KeyValue::new("host.name", super::hostname()))
        .build();
    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource)
        .build();
    let tracer = provider.tracer("subweave");
    tracing::info!("exporting traces over OTLP");
    Some(Traces {
        provider,
        tracer,
        open: Mutex::new(HashMap::new()),
    })
});

/// Starts the exporter now, when the environment names an endpoint.
pub(crate) fn init() {
    LazyLock::force(&TRACES);
}

/// Sends what is queued. Blocks: call it off the async workers.
pub(crate) fn shutdown() {
    if let Some(t) = TRACES.as_ref() {
        if let Err(e) = t.provider.shutdown() {
            tracing::warn!(error = %e, "flushing traces failed");
        }
    }
}

fn ids(ep: &str) -> Option<(TraceId, SpanId)> {
    let id = u128::from_str_radix(ep, 16).ok()?;
    // UUIDv7's low 64 bits are random: the root's span id.
    Some((TraceId::from(id), SpanId::from_bytes((id as u64).to_be_bytes())))
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Check => "check",
        Kind::Decide => "decide",
        Kind::Gate => "gate",
        Kind::Action => "action",
        Kind::Command => "command",
        Kind::Learn => "learn",
        Kind::Ask => "ask",
        Kind::Answer => "answer",
        Kind::Message => "message",
        Kind::Note => "note",
    }
}

/// How long an open episode with no new line keeps its root span back.
const STALE: Duration = Duration::from_secs(3600);

/// A journal line of `service`, just written: a child span of its episode's trace.
pub(crate) fn line(service: &str, l: &Line) {
    let Some(t) = TRACES.as_ref() else { return };
    let Some(ep) = l.ep.as_deref() else { return };
    let Some((trace_id, root_id)) = ids(ep) else { return };
    let end = SystemTime::now();
    let start = l
        .ms
        .and_then(|ms| end.checked_sub(Duration::from_millis(ms)))
        .unwrap_or(end);
    let ends_task =
        l.kind == Kind::Action && l.action.as_deref() == Some(TASK) && l.ok.is_some();
    let mut stale = Vec::new();
    let ended = {
        let mut open = t.open.lock().unwrap_or_else(|e| e.into_inner());
        // A task that waits on an unanswered ask, or a line after its episode ended, never
        // sees an end: its root goes out once it has been quiet for STALE.
        if !open.contains_key(ep) {
            let before = end.checked_sub(STALE).unwrap_or(end);
            let old: Vec<String> =
                open.iter().filter(|(_, r)| r.end < before).map(|(k, _)| k.clone()).collect();
            stale.extend(old.into_iter().filter_map(|k| open.remove(&k).map(|r| (k, r))));
        }
        let r = open.entry(ep.to_string()).or_insert_with(|| Root {
            service: service.to_string(),
            job: None,
            start,
            end,
            goal: None,
            action: None,
            ok: None,
            cost_usd: 0.0,
        });
        r.job = r.job.or(l.job);
        r.start = r.start.min(start);
        r.end = r.end.max(end);
        if r.goal.is_none() {
            r.goal.clone_from(&l.goal);
        }
        if l.kind == Kind::Action && l.ok.is_some() {
            r.action.clone_from(&l.action);
            r.ok = l.ok;
        }
        r.cost_usd += l.cost_usd.unwrap_or(0.0);
        if ends_task { open.remove(ep) } else { None }
    };

    let mut attrs = vec![
        KeyValue::new("subweave.service", service.to_string()),
        KeyValue::new("subweave.kind", kind_name(l.kind)),
    ];
    if let Some(g) = &l.goal {
        attrs.push(KeyValue::new("subweave.goal", g.clone()));
    }
    if let Some(a) = &l.action {
        attrs.push(KeyValue::new("subweave.action", a.clone()));
    }
    if let Some(ok) = l.ok {
        attrs.push(KeyValue::new("subweave.ok", ok));
    }
    if let Some(c) = l.cost_usd {
        attrs.push(KeyValue::new("subweave.cost_usd", c));
    }
    if let Some(u) = &l.usage {
        attrs.extend([
            KeyValue::new("subweave.model", u.model.clone()),
            KeyValue::new("subweave.tokens.input", u.input as i64),
            KeyValue::new("subweave.tokens.cached", u.cached as i64),
            KeyValue::new("subweave.tokens.output", u.output as i64),
            KeyValue::new("subweave.tokens.reasoning", u.reasoning as i64),
        ]);
    }
    if let Some(text) = &l.text {
        // Redacted already: the journal wrote it.
        let key = if l.kind == Kind::Command { "subweave.command" } else { "subweave.text" };
        attrs.push(KeyValue::new(key, text.clone()));
    }
    let parent = Context::new().with_remote_span_context(SpanContext::new(
        trace_id,
        root_id,
        TraceFlags::SAMPLED,
        true,
        TraceState::default(),
    ));
    let mut span = t.tracer.build_with_context(
        SpanBuilder::from_name(kind_name(l.kind))
            .with_start_time(start)
            .with_attributes(attrs),
        &parent,
    );
    if l.ok == Some(false) {
        span.set_status(Status::error("failed"));
    }
    span.end_with_timestamp(end);
    if let Some(r) = ended {
        root(t, trace_id, root_id, r);
    }
    for (ep, r) in stale {
        if let Some((trace_id, root_id)) = ids(&ep) {
            root(t, trace_id, root_id, r);
        }
    }
}

/// A repair or learn ended: its root span goes out. A task's ends with its end line.
pub(crate) fn ended(ep: &str) {
    let Some(t) = TRACES.as_ref() else { return };
    let Some((trace_id, root_id)) = ids(ep) else { return };
    let r = t.open.lock().unwrap_or_else(|e| e.into_inner()).remove(ep);
    if let Some(r) = r {
        root(t, trace_id, root_id, r);
    }
}

fn root(t: &Traces, trace_id: TraceId, root_id: SpanId, r: Root) {
    let job = match r.job {
        Some(JobKind::Repair) => "repair",
        Some(JobKind::Task) => "task",
        Some(JobKind::Learn) => "learn",
        None => "episode",
    };
    let name = match (&r.job, &r.goal) {
        (Some(JobKind::Repair), Some(g)) => format!("repair of {g}"),
        _ => job.to_string(),
    };
    let mut attrs = vec![
        KeyValue::new("subweave.service", r.service),
        KeyValue::new("subweave.kind", job),
        KeyValue::new("subweave.cost_usd", r.cost_usd),
    ];
    if let Some(g) = r.goal {
        attrs.push(KeyValue::new("subweave.goal", g));
    }
    if let Some(a) = r.action {
        attrs.push(KeyValue::new("subweave.action", a));
    }
    if let Some(ok) = r.ok {
        attrs.push(KeyValue::new("subweave.ok", ok));
    }
    let mut span = t.tracer.build_with_context(
        SpanBuilder::from_name(name)
            .with_trace_id(trace_id)
            .with_span_id(root_id)
            .with_start_time(r.start)
            .with_attributes(attrs),
        &Context::new(),
    );
    if r.ok == Some(false) {
        span.set_status(Status::error("failed"));
    }
    span.end_with_timestamp(r.end);
}
