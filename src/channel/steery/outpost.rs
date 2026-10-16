//! The box as a Steery outpost: the daemon offers its MCP tools to the workspace's
//! conversations over the one stream the Steery channel's key holds outbound. Steery's
//! wire (its specs/OUTPOSTS.md): `PUT /v1/outposts` names the outpost, its `holder` and
//! its tools; `GET /v1/events` streams `invocation.sent` to this key; each invocation
//! is answered with `POST /v1/invocations/{id}/report`.
//!
//! A free tool runs at once. A gated one runs only after the person's Go on a signed
//! card, raised through the one path escalation cards take and verified here against the
//! channel's pins, as a signed ask is.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::anyhow;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use uuid::Uuid;

use super::{Button, Raised, Reading, base, jcs, raise, reading};
use crate::channel::{self, ChannelKind, Pin, Via};

/// No call starts this close to its deadline.
const START_MARGIN: Duration = Duration::from_secs(5);
/// A stream that says nothing for this long is dead; the server comments well within it.
const SILENCE: Duration = Duration::from_secs(45);
/// A reconnection waits 1 s, doubling to this.
const RECONNECT_MAX: Duration = Duration::from_secs(15);
/// Steery keeps a result of at most this many bytes of canonical JSON.
const RESULT_BYTES: usize = 256 * 1024;
/// Steery's longest `why`.
const LINE_MAX: usize = 300;
/// The gated card's button that runs the call.
const GO: &str = "go";

/// What an outpost call needs before it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Policy {
    /// Runs at once: it goes through the weave's own rules.
    Free,
    /// Runs only after the person's Go on a signed card.
    Gated,
}

/// A tool call: (tool, arguments, by) to an MCP `CallToolResult`, secrets redacted.
pub(crate) type Call = Arc<
    dyn Fn(String, Value, String) -> std::pin::Pin<Box<dyn Future<Output = Value> + Send>>
        + Send
        + Sync,
>;

/// The tools an outpost offers: their MCP objects in order, what each needs, and how one
/// is called.
#[derive(Clone)]
pub(crate) struct Tools {
    pub list: Vec<Value>,
    pub policy: fn(&str) -> Option<Policy>,
    pub call: Call,
}

/// This daemon's hold on the outpost, for its life: `id` is the `holder` it puts, and the
/// invocations it took and the cards it consumed are each acted on once.
pub(crate) struct Holder {
    pub id: Uuid,
    taken: Mutex<HashSet<Uuid>>,
    consumed: Mutex<HashSet<String>>,
}

impl Holder {
    pub fn new() -> Holder {
        Holder {
            id: Uuid::now_v7(),
            taken: Mutex::default(),
            consumed: Mutex::default(),
        }
    }
}

/// The outpost the box offers: its first Steery channel's key, workspace and pins, under
/// that channel's `outpost` name or the hostname's.
#[derive(Clone, PartialEq)]
pub(crate) struct Config {
    pub channel: String,
    key: String,
    workspace: String,
    pub name: String,
    pins: Vec<Pin>,
}

pub(crate) fn config() -> Option<Config> {
    channel::ls()
        .ok()?
        .into_iter()
        .find(|c| c.kind() == ChannelKind::Steery)
        .and_then(|c| match c.via {
            Via::Steery { pins, outpost, .. } => Some(Config {
                name: name_of(outpost.as_deref()),
                channel: c.name,
                key: c.secret?,
                workspace: c.target?,
                pins,
            }),
            _ => None,
        })
}

/// A channel's `outpost` name, else the hostname's first label as an outpost name.
pub fn name_of(outpost: Option<&str>) -> String {
    if let Some(name) = outpost {
        return name.to_owned();
    }
    let host = channel::hostname().to_ascii_lowercase();
    let mut name = String::new();
    for c in host.split('.').next().unwrap_or_default().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' };
        if !(c == '-' && (name.is_empty() || name.ends_with('-'))) {
            name.push(c);
        }
    }
    name.truncate(40);
    name.trim_end_matches('-').to_owned()
}

/// Steery's rule for an outpost's name.
pub fn check_name(name: &str) -> Result<(), String> {
    let b = name.as_bytes();
    let shaped = (2..=40).contains(&b.len())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
        && b.first().is_some_and(|c| *c != b'-')
        && b.last().is_some_and(|c| *c != b'-');
    if shaped {
        Ok(())
    } else {
        Err(format!(
            "an outpost's name is 2–40 of a-z, 0-9 and -, beginning and ending with a letter or digit: «{name}» is not"
        ))
    }
}

/// Why [`run`] returned.
pub(crate) enum Ended {
    /// Told to stop.
    Stopped,
    /// Steery refused the key: `end {reconnect: false}`, or 401 or 403.
    Revoked(String),
    /// An invocation named another holder after this one's put: another daemon holds it.
    HeldElsewhere,
    /// Steery refused the outpost itself (its name, a name another key holds).
    Refused(String),
}

/// What [`run`] waits on to stop: `Some(why)`, said of each call in hand.
pub(crate) type Stop = watch::Receiver<Option<&'static str>>;

async fn stopped(stop: &mut Stop) -> &'static str {
    loop {
        if let Some(why) = *stop.borrow_and_update() {
            return why;
        }
        if stop.changed().await.is_err() {
            return "the daemon stopped";
        }
    }
}

/// Hold outpost `cfg` until told to stop, Steery refuses the key or the outpost, or
/// another daemon holds it.
pub(crate) async fn run(cfg: &Config, holder: &Arc<Holder>, tools: Tools, mut stop: Stop) -> Ended {
    if let Err(why) = check_name(&cfg.name) {
        return Ended::Refused(format!("{why}: add the Steery channel with --outpost <name>"));
    }
    // The stream from the log's head, so nothing older is read and the key is online
    // before it puts.
    let mut failures = 0;
    let head = loop {
        let failed = match api(cfg, reqwest::Method::GET, "/v1/me", None).await {
            Ok(me) => break me["workspace"]["head"].as_i64(),
            Err(Failure::Refused(401 | 403, why)) => return Ended::Revoked(why),
            Err(e) => e,
        };
        tracing::warn!(error = %failed, "outpost: reading Steery's log head");
        failures += 1;
        tokio::select! {
            () = tokio::time::sleep(backoff(failures)) => {}
            _ = stopped(&mut stop) => return Ended::Stopped,
        }
    };
    let (events_tx, mut events) = mpsc::unbounded_channel();
    let (up_tx, mut up) = watch::channel(false);
    let mut follow = tokio::spawn(follow(cfg.clone(), head, events_tx, up_tx));
    tokio::select! {
        _ = up.wait_for(|up| *up) => {}
        revoked = &mut follow => return Ended::Revoked(joined(revoked)),
        _ = stopped(&mut stop) => {
            follow.abort();
            return Ended::Stopped;
        }
    }

    let put = json!({"name": cfg.name, "holder": holder.id, "tools": tools.list});
    let mut failures = 0;
    let held_since = loop {
        let failed = match api(cfg, reqwest::Method::PUT, "/v1/outposts", Some(&put)).await {
            Ok(outpost) => break timestamp(&outpost["updated_at"]),
            Err(Failure::Refused(401 | 403, why)) => {
                follow.abort();
                return Ended::Revoked(why);
            }
            Err(Failure::Refused(_, why)) => {
                follow.abort();
                return Ended::Refused(why);
            }
            Err(e) => e,
        };
        tracing::warn!(error = %failed, "outpost: putting the outpost");
        failures += 1;
        tokio::select! {
            () = tokio::time::sleep(backoff(failures)) => {}
            _ = stopped(&mut stop) => {
                follow.abort();
                return Ended::Stopped;
            }
        }
    };
    tracing::info!(outpost = %cfg.name, holder = %holder.id, tools = tools.list.len(), "outpost: held");

    let running = Arc::new(Running {
        cfg: cfg.clone(),
        holder: holder.clone(),
        tools,
        in_hand: Mutex::default(),
    });
    let mut calls = JoinSet::new();
    let (ended, why) = loop {
        tokio::select! {
            why = stopped(&mut stop) => break (Ended::Stopped, why),
            revoked = &mut follow => break (Ended::Revoked(joined(revoked)), "Steery revoked this box's key"),
            Some(event) = events.recv() => {
                let event: Value = event;
                if event["kind"] != "invocation.sent" {
                    continue;
                }
                let sent: Sent = match serde_json::from_value(event["detail"].clone()) {
                    Ok(sent) => sent,
                    Err(e) => {
                        tracing::warn!(error = %e, "outpost: an unreadable invocation.sent");
                        continue;
                    }
                };
                if sent.outpost != cfg.name {
                    continue;
                }
                if sent.holder != holder.id {
                    // Sent after this daemon's put, to another holder: it put after us.
                    let later = timestamp(&event["at"])
                        .zip(held_since)
                        .is_some_and(|(at, since)| at > since);
                    if later {
                        break (Ended::HeldElsewhere, "another daemon holds the outpost");
                    }
                    continue;
                }
                if !lock(&holder.taken).insert(sent.id) {
                    continue;
                }
                lock(&running.in_hand).insert(sent.id);
                calls.spawn(invoke(running.clone(), sent));
            }
            Some(_) = calls.join_next(), if !calls.is_empty() => {}
        }
    };
    calls.abort_all();
    follow.abort();
    let in_hand: Vec<Uuid> = lock(&running.in_hand).drain().collect();
    for id in in_hand {
        running.report(id, &Report::Failed(why.to_owned())).await;
    }
    ended
}

/// The `detail` of an `invocation.sent` event.
#[derive(Deserialize)]
struct Sent {
    id: Uuid,
    outpost: String,
    holder: Uuid,
    tool: String,
    arguments: Map<String, Value>,
    deadline: String,
    caller: Caller,
}

#[derive(Deserialize)]
struct Caller {
    handle: String,
    #[serde(default)]
    session: Option<Uuid>,
    #[serde(default)]
    topic: Option<String>,
}

/// What an outpost says became of an invocation.
enum Report {
    Waiting(String),
    Result(Value),
    Failed(String),
}

/// One outpost while it is held: what each invocation shares.
struct Running {
    cfg: Config,
    holder: Arc<Holder>,
    tools: Tools,
    /// Invocations taken and not yet reported.
    in_hand: Mutex<HashSet<Uuid>>,
}

/// One invocation, from its event to its report.
async fn invoke(o: Arc<Running>, sent: Sent) {
    let report = match (o.tools.policy)(&sent.tool) {
        None => Some(Report::Failed(format!(
            "«{}» lists no tool «{}»",
            o.cfg.name, sent.tool
        ))),
        Some(Policy::Free) => match sent.deadline.parse::<jiff::Timestamp>() {
            Ok(deadline) => o.call(&sent, deadline).await,
            Err(_) => Some(Report::Failed(format!("no deadline: {:?}", sent.deadline))),
        },
        Some(Policy::Gated) => o.gated(&sent).await,
    };
    if let Some(report) = report {
        o.report(sent.id, &report).await;
    }
    lock(&o.in_hand).remove(&sent.id);
}

impl Running {
    /// Run the call by `deadline`: `None` when too near to start it, or when it passes
    /// first (Steery expires the invocation).
    async fn call(&self, sent: &Sent, deadline: jiff::Timestamp) -> Option<Report> {
        let left = Duration::try_from(deadline.duration_since(jiff::Timestamp::now())).ok()?;
        if left <= START_MARGIN {
            return None;
        }
        let by = format!("outpost:{}", sent.caller.handle);
        let call = (self.tools.call)(
            sent.tool.clone(),
            Value::Object(sent.arguments.clone()),
            by,
        );
        let result = tokio::time::timeout(left, call).await.ok()?;
        if !result["content"].is_array() {
            return Some(Report::Failed("the tool answered no CallToolResult".into()));
        }
        let bytes = jcs(&result).map_or(usize::MAX, |text| text.len());
        if bytes > RESULT_BYTES {
            return Some(Report::Failed(format!(
                "the result is {bytes} bytes; at most {RESULT_BYTES}"
            )));
        }
        Some(Report::Result(result))
    }

    /// A gated call: a signed card whose payload is the exact call, assigned to the
    /// person; on its Go, verified against the channel's pins and consumed once, the
    /// call runs. `None` where Steery moves the invocation on itself.
    async fn gated(&self, sent: &Sent) -> Option<Report> {
        if self.cfg.pins.is_empty() {
            return Some(Report::Failed(format!(
                "the Steery channel {} pins no signer: add it again",
                self.cfg.channel
            )));
        }
        let raised = self.ask(sent);
        let mine = json!({"kind": "mine", "why": format!("It runs on {}", self.cfg.name)});
        if let Err(e) = raise(&self.cfg.key, &raised, Some(mine)).await {
            return Some(Report::Failed(line(&format!("could not ask: {e:#}"))));
        }
        let waiting = self.report(sent.id, &Report::Waiting(raised.id.clone())).await?;
        let Some(deadline) = timestamp(&waiting["deadline"]) else {
            tracing::warn!(invocation = %sent.id, "outpost: the waiting invocation has no deadline");
            return None;
        };
        loop {
            if jiff::Timestamp::now() >= deadline {
                return None;
            }
            match reading(&self.cfg.key, &raised, &self.cfg.pins).await {
                Ok(Reading::Open) => {}
                Ok(Reading::Withdrawn(_)) => return None,
                Ok(Reading::Unverified(e)) => {
                    return Some(Report::Failed(line(&format!(
                        "the answer did not verify: {e:#}"
                    ))));
                }
                Ok(Reading::Verified(word, by)) => {
                    if word != GO {
                        return Some(Report::Failed(format!("declined by {by}")));
                    }
                    if !lock(&self.holder.consumed).insert(raised.id.clone()) {
                        return None;
                    }
                    tracing::info!(invocation = %sent.id, tool = %sent.tool, %by, "outpost: a signed Go");
                    return self.call(sent, deadline).await;
                }
                Err(e) => {
                    tracing::warn!(invocation = %sent.id, error = %format!("{e:#}"), "outpost: reading the card");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    /// The gated call's card: code's words over the event's values, the arguments
    /// fenced, the exact call as its payload.
    fn ask(&self, sent: &Sent) -> Raised {
        let arguments = serde_json::to_string_pretty(&sent.arguments).unwrap_or_default();
        let conversation = match (sent.caller.session, &sent.caller.topic) {
            (Some(session), Some(topic)) => {
                format!("'s conversation [{topic}](steery://session/{session})")
            }
            (Some(session), None) => format!("'s [conversation](steery://session/{session})"),
            (None, _) => String::new(),
        };
        let name = &self.cfg.name;
        Raised {
            id: Uuid::now_v7().to_string(),
            workspace: self.cfg.workspace.clone(),
            title: format!("Run «{}» on «{name}»?", sent.tool),
            body: format!(
                "@{}{conversation} asks to run `{}` on {name} with:\n```json\n{arguments}\n```",
                sent.caller.handle, sent.tool
            ),
            payload: json!({"id": sent.id, "outpost": name, "tool": sent.tool, "arguments": sent.arguments}),
            view: json!([]),
            buttons: vec![
                Button {
                    id: GO.into(),
                    label: "Go".into(),
                    stance: "neutral".into(),
                },
                Button {
                    id: "no".into(),
                    label: "Decline".into(),
                    stance: "rejection".into(),
                },
            ],
        }
    }

    /// Say what became of invocation `id`: the invocation as Steery holds it, or `None`
    /// when Steery refused the report (the invocation moved on) or could not be reached.
    async fn report(&self, id: Uuid, report: &Report) -> Option<Value> {
        let body = match report {
            Report::Waiting(request) => json!({"waiting": request}),
            Report::Result(result) => json!({"result": result}),
            Report::Failed(why) => json!({"failed": line(why)}),
        };
        let path = format!("/v1/invocations/{id}/report");
        match api(&self.cfg, reqwest::Method::POST, &path, Some(&body)).await {
            Ok(row) => Some(row),
            Err(e) => {
                tracing::warn!(invocation = %id, error = %e, "outpost: reporting an invocation");
                None
            }
        }
    }
}

/// Why a Steery call failed: refused with a status and Steery's words, or not answered.
enum Failure {
    Refused(u16, String),
    Down(anyhow::Error),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Refused(status, why) => write!(f, "HTTP {status}: {why}"),
            Failure::Down(e) => write!(f, "{e:#}"),
        }
    }
}

async fn api(cfg: &Config, method: reqwest::Method, path: &str, body: Option<&Value>) -> Result<Value, Failure> {
    let mut req = channel::client()
        .request(method, format!("{}{path}", base()))
        .bearer_auth(&cfg.key)
        .header("Steery-Workspace", &cfg.workspace);
    if let Some(body) = body {
        req = req.json(body);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| Failure::Down(anyhow!("steery {path}: {}", e.without_url())))?;
    let status = resp.status();
    if status.is_success() {
        return resp
            .json()
            .await
            .map_err(|e| Failure::Down(anyhow!("steery {path}: {}", e.without_url())));
    }
    let text = resp.text().await.unwrap_or_default();
    if status.is_client_error() {
        return Err(Failure::Refused(status.as_u16(), said(&text)));
    }
    Err(Failure::Down(anyhow!("steery {path}: HTTP {}: {}", status.as_u16(), said(&text))))
}

/// Steery's error message in `text`, else its first 300 characters.
fn said(text: &str) -> String {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| text.trim().chars().take(LINE_MAX).collect())
}

/// The event stream has no deadline: `SILENCE` ends it.
static STREAMS: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .user_agent(concat!("subweave/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("a client with rustls builds")
});

/// Follow `GET /v1/events` from `after`, each log entry to `events`, whether a stream is
/// open to `up`. A drop, silence and `end {reconnect: true}` reconnect from the last
/// entry by `Last-Event-ID`. It returns only when Steery refuses the key: why.
async fn follow(
    cfg: Config,
    mut after: Option<i64>,
    events: mpsc::UnboundedSender<Value>,
    up: watch::Sender<bool>,
) -> String {
    let mut failures = 0;
    loop {
        let mut req = STREAMS
            .get(format!("{}/v1/events", base()))
            .bearer_auth(&cfg.key)
            .header("Steery-Workspace", &cfg.workspace)
            .header(reqwest::header::ACCEPT, "text/event-stream");
        if let Some(after) = after {
            req = req.header("Last-Event-ID", after.to_string());
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                failures = 0;
                up.send_replace(true);
                let mut body = resp.bytes_stream();
                let mut parser = Parser::default();
                let mut revoked = None;
                'read: loop {
                    let chunk = match tokio::time::timeout(SILENCE, body.next()).await {
                        Ok(Some(Ok(chunk))) => chunk,
                        Ok(Some(Err(e))) => {
                            tracing::warn!(error = %e.without_url(), "outpost: the event stream ended");
                            break;
                        }
                        Ok(None) => break,
                        Err(_) => {
                            tracing::warn!("outpost: the event stream fell silent");
                            break;
                        }
                    };
                    for frame in parser.push(&chunk) {
                        if frame.event == "end" {
                            revoked = refused(&frame.data);
                            break 'read;
                        }
                        // Only a log entry has an id, its seq; the rest are pokes.
                        let Some(seq) = frame.id else { continue };
                        after = Some(seq);
                        match serde_json::from_str(&frame.data) {
                            Ok(event) => {
                                let _ = events.send(event);
                            }
                            Err(e) => tracing::warn!(error = %e, "outpost: an unreadable event"),
                        }
                    }
                }
                up.send_replace(false);
                if let Some(why) = revoked {
                    return why;
                }
            }
            Ok(resp) if matches!(resp.status().as_u16(), 401 | 403) => {
                return said(&resp.text().await.unwrap_or_default());
            }
            Ok(resp) => tracing::warn!(status = resp.status().as_u16(), "outpost: the event stream was refused"),
            Err(e) => tracing::warn!(error = %e.without_url(), "outpost: cannot reach the event stream"),
        }
        failures += 1;
        tokio::time::sleep(backoff(failures)).await;
    }
}

/// An `end` that says not to reconnect, as why.
fn refused(data: &str) -> Option<String> {
    let end: Value = serde_json::from_str(data).ok()?;
    (end["reconnect"] == false).then(|| {
        end["reason"]
            .as_str()
            .unwrap_or("the server ended the stream")
            .to_owned()
    })
}

struct Frame {
    id: Option<i64>,
    event: String,
    data: String,
}

/// `id:`, `event:` and `data:` lines, dispatched on a blank line; comments ignored.
#[derive(Default)]
struct Parser {
    buffer: Vec<u8>,
    id: Option<i64>,
    event: String,
    data: String,
}

impl Parser {
    fn push(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.buffer.drain(..=end).collect();
            let text = String::from_utf8_lossy(&raw);
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                let event = std::mem::take(&mut self.event);
                if event.is_empty() && self.data.is_empty() {
                    self.id = None;
                    continue;
                }
                frames.push(Frame {
                    id: self.id.take(),
                    event,
                    data: std::mem::take(&mut self.data),
                });
                continue;
            }
            let Some((field, value)) = text.split_once(':') else {
                continue;
            };
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "id" => self.id = value.parse().ok(),
                "event" => self.event = value.to_owned(),
                "data" => {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(value);
                }
                _ => {}
            }
        }
        frames
    }
}

/// 1 s, doubling to `RECONNECT_MAX`.
fn backoff(failures: u32) -> Duration {
    Duration::from_secs(1u64 << failures.saturating_sub(1).min(4)).min(RECONNECT_MAX)
}

fn joined(follow: Result<String, tokio::task::JoinError>) -> String {
    follow.unwrap_or_else(|e| e.to_string())
}

fn timestamp(value: &Value) -> Option<jiff::Timestamp> {
    value.as_str()?.parse().ok()
}

/// `why` as one line of at most `LINE_MAX` characters.
fn line(why: &str) -> String {
    let one = why.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(LINE_MAX).collect()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
