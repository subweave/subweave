//! One end-to-end scenario: the built `subweave` binary runs as a daemon and as CLI
//! commands against a temporary state and config directory. Only the vendors are
//! mocked: one local HTTP server plays OpenRouter (`/systemone` for the decider,
//! `/chat/completions` for the coder), Steery (`/v1/…`, signing as a pinned phone) and
//! a webhook receiver.
//!
//! The mock answers by what each request asks (question keys, the failing goal, the
//! box's facts, the coder's earlier tool calls), never by call order.

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};

const SECRET: &str = "tok-e2e-5f1c9a77d3";
const API_KEY: &str = "mock-key-e2e-81b2c";
const FIREWALL_RULE: &str = "open a port on the firewall";
const STEERY_KEY: &str = "sk-e2e-3b9d0c51";
const WORKSPACE: &str = "0199f3a0-1c2d-7e4f-8a5b-6c7d8e9f0a1b";
const SIGNER: &str = "0199f3a0-2d3e-7f40-9b6c-7d8e9f0a1b2c";
const SIGNER_NAME: &str = "e2e phone";
/// How an agent elsewhere runs subweave on the toy's box.
const REACH: &str = "root@box-e2e";
/// Lines of the toy's event log: a kind the weave knows normal, a routine kind the model
/// learns, a kind a person says is trouble, and one the model calls trouble that the
/// person's answer says is normal.
const HEARTBEAT: &str = "heartbeat 1 ok from 10.0.0.2";
const ROUTINE_CONN: &str = "conn 17 from 10.0.0.7 closed after 1200ms";
const DISK_FAILED: &str = "disk 3 failed: I/O error at sector 81920";
const CACHE_WARM: &str = "cache warmed in 3 passes";

/// Every request the mock got.
#[derive(Default)]
struct Seen {
    /// (path, Authorization header, body).
    requests: Vec<(String, String, String)>,
    /// Webhook bodies.
    hooks: Vec<Value>,
    /// Questions or requests the script has no answer for.
    unexpected: Vec<String>,
    /// OTLP trace exports, protobuf.
    traces: Vec<Vec<u8>>,
    /// GitHub release list requests.
    releases: usize,
}

/// What the Steery mock holds.
#[derive(Default)]
struct Steery {
    /// Each `POST`: (path, body).
    posts: Vec<(String, Value)>,
    throws: std::collections::BTreeMap<String, Value>,
    requests: std::collections::BTreeMap<String, Value>,
    /// Sessions that answer 404 to the next throw naming them.
    gone: Vec<String>,
}

#[derive(Clone)]
struct Mock {
    seen: Arc<Mutex<Seen>>,
    steery: Arc<Mutex<Steery>>,
    /// While set, a card summary waits: an ask can settle before its card goes out.
    hold_summary: Arc<AtomicBool>,
    /// Where the forbidden coder command would leave its mark.
    firewall_mark: PathBuf,
}

impl Mock {
    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Coder model requests, without the card summaries (they offer the `save` tool).
    fn coder_requests(&self) -> usize {
        self.seen()
            .requests
            .iter()
            .filter(|(p, _, b)| p == "/chat/completions" && !offers_tool(b, "save"))
            .count()
    }

    /// Coder requests that learn a quoted fact (they offer the `fact` tool).
    fn fact_requests(&self) -> usize {
        self.seen()
            .requests
            .iter()
            .filter(|(p, _, b)| p == "/chat/completions" && offers_tool(b, "fact"))
            .count()
    }

    fn hooks(&self) -> Vec<Value> {
        self.seen().hooks.clone()
    }

    fn steery(&self) -> std::sync::MutexGuard<'_, Steery> {
        self.steery.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The bodies Steery got on `POST path`.
    fn posts(&self, path: &str) -> Vec<Value> {
        self.steery()
            .posts
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .collect()
    }

    /// The throws for escalation `n`, oldest first.
    fn throws_for(&self, n: u64) -> Vec<Value> {
        let head = format!("# Escalation #{n}:");
        self.posts("/v1/throws")
            .into_iter()
            .filter(|t| t["body"].as_str().is_some_and(|b| b.starts_with(&head)))
            .collect()
    }

    /// The person's Go delivered `throw` to `session`: its newest aim says so.
    fn deliver(&self, throw: &str, session: &str) {
        let mut s = self.steery();
        let t = s.throws.get_mut(throw).expect("the throw was made");
        t["status"] = json!("delivered");
        let index = t["aims"].as_array().map_or(0, Vec::len);
        t["aims"]
            .as_array_mut()
            .unwrap()
            .push(json!({"index": index, "status": "delivered", "act": "continue", "session_id": session}));
    }

    /// The person presses `word` on card `id`, signed by the pinned phone.
    fn respond(&self, id: &str, word: &str) {
        use p256::ecdsa::signature::Signer as _;
        use subweave::channel::steery;
        let mut s = self.steery();
        let r = s.requests.get_mut(id).expect("the card was raised");
        let content = steery::content_sha256(r).unwrap();
        let answer = json!({"values": {"answer": word}});
        let at = "2026-10-15T12:00:00.000Z";
        let message = steery::message(
            WORKSPACE,
            id,
            SIGNER,
            &content,
            &steery::answer_sha256(&answer).unwrap(),
            "neutral",
            at,
        );
        let signature: p256::ecdsa::Signature = signing_key().sign(message.as_bytes());
        r["status"] = json!("responded");
        r["response"] = json!({
            "answer": answer,
            "stance": "neutral",
            "signer": SIGNER,
            "at": at,
            "signature": URL_SAFE_NO_PAD.encode(signature.to_der().as_bytes()),
        });
    }
}

/// The pinned phone's key.
fn signing_key() -> p256::ecdsa::SigningKey {
    p256::ecdsa::SigningKey::from_slice(&[0x2a; 32]).expect("a P-256 scalar")
}

/// A card's button words, in order.
fn buttons(card: &Value) -> Vec<String> {
    card["buttons"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["word"].as_str().map(String::from))
        .collect()
}

/// A card's button for `word`.
fn label<'a>(card: &'a Value, word: &str) -> &'a Value {
    card["buttons"]
        .as_array()
        .and_then(|bs| bs.iter().find(|b| b["word"] == word))
        .map_or(&Value::Null, |b| &b["label"])
}

/// A card's plain lines, one string.
fn card_lines(card: &Value) -> String {
    card["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

fn steery_ok(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|h| h.to_str().ok())
        == Some(&format!("Bearer {STEERY_KEY}"))
}

async fn steery_me(headers: HeaderMap) -> (StatusCode, axum::Json<Value>) {
    if !steery_ok(&headers) {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({})));
    }
    let me = json!({"workspace": {"id": WORKSPACE, "name": "e2e"}});
    (StatusCode::OK, axum::Json(me))
}

async fn steery_signers(headers: HeaderMap) -> (StatusCode, axum::Json<Value>) {
    if !steery_ok(&headers) || headers.get("steery-workspace").is_none() {
        return (StatusCode::UNAUTHORIZED, axum::Json(json!({})));
    }
    let key = signing_key();
    let point = key.verifying_key().to_encoded_point(false);
    let signers = json!([{
        "id": SIGNER,
        "kind": "phone",
        "name": SIGNER_NAME,
        "public_key": URL_SAFE_NO_PAD.encode(point.as_bytes()),
        "revoked_at": null,
    }]);
    (StatusCode::OK, axum::Json(signers))
}

/// `POST /v1/throws`: with `session`, delivered at once, or 404 once for a gone one;
/// without, held for the person's pick.
async fn throw_create(State(m): State<Mock>, headers: HeaderMap, body: String) -> (StatusCode, axum::Json<Value>) {
    let t: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let mut s = m.steery();
    s.posts.push(("/v1/throws".into(), t.clone()));
    let Some(id) = t["id"].as_str().filter(|_| steery_ok(&headers)) else {
        return (StatusCode::BAD_REQUEST, axum::Json(json!({"error": {"code": "invalid"}})));
    };
    let throw = match t["session"].as_str() {
        Some(session) => {
            if let Some(i) = s.gone.iter().position(|g| g == session) {
                s.gone.remove(i);
                let gone = json!({"error": {"code": "not_found", "message": "no such session"}});
                return (StatusCode::NOT_FOUND, axum::Json(gone));
            }
            json!({"id": id, "status": "delivered", "body": t["body"], "aims": [
                {"index": 0, "status": "delivered", "act": t["act"], "session_id": session}
            ]})
        }
        None => json!({"id": id, "status": "held", "body": t["body"], "aims": [
            {"index": 0, "status": "held", "act": "continue"}
        ]}),
    };
    s.throws.insert(id.to_owned(), throw.clone());
    (StatusCode::CREATED, axum::Json(throw))
}

async fn throw_get(State(m): State<Mock>, UrlPath(id): UrlPath<String>) -> (StatusCode, axum::Json<Value>) {
    match m.steery().throws.get(&id) {
        Some(t) => (StatusCode::OK, axum::Json(t.clone())),
        None => (StatusCode::NOT_FOUND, axum::Json(json!({"error": {"code": "not_found"}}))),
    }
}

async fn request_create(State(m): State<Mock>, headers: HeaderMap, body: String) -> (StatusCode, axum::Json<Value>) {
    let mut r: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let mut s = m.steery();
    s.posts.push(("/v1/requests".into(), r.clone()));
    let Some(id) = r["id"].as_str().filter(|_| steery_ok(&headers)).map(String::from) else {
        return (StatusCode::BAD_REQUEST, axum::Json(json!({"error": {"code": "invalid"}})));
    };
    r["workspace"] = json!(WORKSPACE);
    r["status"] = json!("open");
    s.requests.insert(id, r.clone());
    (StatusCode::CREATED, axum::Json(r))
}

/// `GET /v1/requests/{id}`: with `wait_secs`, held until it is answered or the wait ends.
async fn request_get(
    State(m): State<Mock>,
    UrlPath(id): UrlPath<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, axum::Json<Value>) {
    let wait = q.get("wait_secs").and_then(|w| w.parse().ok()).unwrap_or(0u64);
    let end = Instant::now() + Duration::from_secs(wait.min(50));
    loop {
        match m.steery().requests.get(&id) {
            None => return (StatusCode::NOT_FOUND, axum::Json(json!({"error": {"code": "not_found"}}))),
            Some(r) if r["status"] != "open" || Instant::now() >= end => {
                return (StatusCode::OK, axum::Json(r.clone()));
            }
            Some(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn offers_tool(body: &str, name: &str) -> bool {
    serde_json::from_str::<Value>(body).is_ok_and(|v| {
        v["tools"]
            .as_array()
            .is_some_and(|t| t.iter().any(|t| t["function"]["name"] == name))
    })
}

fn record(m: &Mock, path: &str, headers: &HeaderMap, body: &str) {
    let auth = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .unwrap_or_default()
        .to_string();
    m.seen()
        .requests
        .push((path.to_string(), auth, body.to_string()));
}

fn noul(p: f64) -> Value {
    json!({"type": "noul", "noul": p})
}

/// A choice of `pick` at probability `p`; the rest share what is left.
fn choice(q: &Value, pick: &str, p: f64) -> Value {
    let keys: Vec<String> = q["criteria"]
        .as_object()
        .map(|c| c.keys().cloned().collect())
        .unwrap_or_default();
    let rest = (1.0 - p) / (keys.len().saturating_sub(1).max(1)) as f64;
    let probs: Map<String, Value> = keys
        .iter()
        .map(|k| (k.clone(), json!(if k == pick { p } else { rest })))
        .collect();
    json!({"type": "choice", "choice": pick, "probabilities": probs, "confidence": p})
}

/// `/systemone`: each question answered by its key and the state it is asked about.
async fn systemone(State(m): State<Mock>, headers: HeaderMap, body: String) -> axum::Json<Value> {
    record(&m, "/systemone", &headers, &body);
    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let state = &req["state"];
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    let mut answers = Map::new();
    for (key, q) in req["questions"].as_object().cloned().unwrap_or_default() {
        let a = match key.as_str() {
            // The request check: the port is the request's number word.
            "slot.port" => {
                let word = state["words"].as_array().and_then(|ws| {
                    ws.iter().position(|w| {
                        w.as_str()
                            .is_some_and(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_digit()))
                    })
                });
                match word {
                    Some(i) => choice(&q, &format!("w{i}"), 0.92),
                    None => choice(&q, "none", 0.9),
                }
            }
            k if k.starts_with("slot.") => choice(&q, "none", 0.9),
            k if k.starts_with("fit.") => noul(0.95),
            k if k.starts_with("never.") || k.starts_with("ask.") => noul(0.02),
            "uncovered" => noul(0.04),
            // A repair: which action, and whether text on the box steers it.
            "next" => {
                let goal = text(&state["failing_goal"]);
                let unsure = state["program_output"]["scene"]
                    .as_str()
                    .is_some_and(|s| s.contains("unsure"));
                if goal.starts_with("installed") {
                    choice(&q, "install", 0.9)
                } else if goal.starts_with("running") && unsure {
                    choice(&q, "restart", 0.31)
                } else if goal.starts_with("running") {
                    choice(&q, "restart", 0.93)
                } else if goal.starts_with("quiet") {
                    let kinds = text(&state["program_output"]["line kinds"]);
                    if kinds.contains("cache warmed") {
                        choice(&q, "ask", 0.9)
                    } else {
                        choice(&q, "hush", 0.9)
                    }
                } else {
                    m.seen().unexpected.push(format!("next for {goal}"));
                    choice(&q, "ask", 0.9)
                }
            }
            "steered" => noul(0.02),
            // The quoted rules on a coder command.
            k if k.starts_with("rule") => {
                let cmd = format!("{} {}", text(&state["command"]), text(&state["purpose"]));
                let instructions = text(&q["instructions"]);
                if instructions.contains(FIREWALL_RULE) && cmd.contains("firewall") {
                    noul(0.96)
                } else {
                    noul(0.03)
                }
            }
            // A learned fact command: does it change the box?
            "changes" => noul(0.03),
            // Does it hide its read's failure behind a fallback value?
            "hides" => noul(0.03),
            // A learned fact's printed value: `served` is 0 or 1, never an HTTP status.
            "fits" => {
                let printed = text(&state["printed"]);
                noul(if printed == "0" || printed == "1" {
                    0.93
                } else {
                    0.04
                })
            }
            "kind" => choice(&q, "other", 0.8),
            // A PART release's notes: a security fix?
            k if k.starts_with("release.") => {
                let notes = text(&state["program_output"][k]);
                noul(if notes.contains("CVE") { 0.95 } else { 0.05 })
            }
            // One kind of line of a goal's window: does it show trouble?
            k if k.starts_with("line.") => {
                let line = text(&state["program_output"][k]);
                if line.contains("closed after") {
                    noul(0.05)
                } else if line.contains("cache warmed") {
                    noul(0.9)
                } else {
                    m.seen().unexpected.push(format!("line {line}"));
                    noul(0.0)
                }
            }
            other => {
                m.seen().unexpected.push(format!("question {other}"));
                noul(0.0)
            }
        };
        answers.insert(key, a);
    }
    axum::Json(json!({
        "answers": answers,
        "usage": {"input_tokens": 120, "output_tokens": 2, "cost": 0.00001},
    }))
}

fn tool_call(id: &str, name: &str, args: Value) -> axum::Json<Value> {
    axum::Json(json!({
        "choices": [{"message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [{"id": id, "type": "function", "function": {
                "name": name, "arguments": args.to_string(),
            }}],
        }}],
        "usage": {
            "prompt_tokens": 900,
            "completion_tokens": 40,
            "completion_tokens_details": {"reasoning_tokens": 30},
            "cost": 0.0001,
        },
    }))
}

const MKDIR: &str = "mkdir -p \"$root\" && echo \"web root $root, token $TOY_TOKEN\"";
const WRITE: &str = "printf '<html><body><h1>hello</h1></body></html>\\n' > \"$root/index.html\"";
const FACT_CMD: &str = "if curl -fs \"http://127.0.0.1:$port/\" 2>/dev/null | grep hello >/dev/null; then echo 1; else echo 0; fi";
/// The learner's first try reads the right thing in the wrong form: an HTTP status.
const FACT_WRONG: &str =
    "curl -s -o /dev/null -w '%{http_code}' \"http://127.0.0.1:$port/\" || true";

/// A person's task, and what the coder does and says for it.
const TASK_TEXT: &str = "tidy the web root";
const TASK_CMD: &str = "echo \"tidied $root on port $port\"";
const TASK_SUMMARY: &str = "the web root was already tidy";
/// The two lines the model writes on top of every card.
const CARD_SUMMARY: [&str; 2] = [
    "A step on the toy server waits for you.",
    "Your answer decides if Subweave goes on.",
];

/// `/chat/completions`: a fact learner gets the fact command; a person's task gets one
/// command and `done`; an action's coder goes on from the tool calls it already made: a
/// forbidden firewall command, mkdir, the page, then `done`.
async fn chat(State(m): State<Mock>, headers: HeaderMap, body: String) -> axum::Json<Value> {
    record(&m, "/chat/completions", &headers, &body);
    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if offers_tool(&body, "save") {
        let end = Instant::now() + Duration::from_secs(15);
        while m.hold_summary.load(Ordering::SeqCst) && Instant::now() < end {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        return tool_call("s1", "save", json!({"lines": CARD_SUMMARY}));
    }
    if offers_tool(&body, "fact") {
        let tried = req["messages"]
            .as_array()
            .is_some_and(|ms| ms.iter().any(|msg| msg["role"] == "assistant"));
        let command = if tried { FACT_CMD } else { FACT_WRONG };
        return tool_call("f1", "fact", json!({"command": command}));
    }
    let made: String = req["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|msg| msg["role"] == "assistant")
        .flat_map(|msg| msg["tool_calls"].as_array().cloned().unwrap_or_default())
        .map(|c| {
            c["function"]["arguments"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    // A person's task from the dashboard: one command, then `done` with a summary.
    if body.contains("Task from the person") {
        if !made.contains("tidied") {
            return tool_call(
                "t1",
                "shell",
                json!({"command": TASK_CMD, "purpose": "tidy the web root"}),
            );
        }
        return tool_call(
            "t2",
            "done",
            json!({"commands": [TASK_CMD], "summary": TASK_SUMMARY}),
        );
    }
    if !made.contains("firewall") {
        let cmd = format!("touch {} && echo opened", m.firewall_mark.display());
        return tool_call(
            "c1",
            "shell",
            json!({"command": cmd, "purpose": "open port 80 on the firewall so the page is reachable"}),
        );
    }
    if !made.contains("mkdir") {
        return tool_call(
            "c2",
            "shell",
            json!({"command": MKDIR, "purpose": "create the web root"}),
        );
    }
    if !made.contains("index.html") {
        return tool_call(
            "c3",
            "shell",
            json!({"command": WRITE, "purpose": "write the hello page"}),
        );
    }
    tool_call(
        "c4",
        "done",
        json!({"commands": [MKDIR, WRITE], "cause": "nothing was installed yet"}),
    )
}

async fn traces(State(m): State<Mock>, body: axum::body::Bytes) -> &'static str {
    m.seen().traces.push(body.to_vec());
    ""
}

/// GitHub's releases of `toy/web`, newest first, with a draft and a pre-release that do
/// not count.
async fn releases(State(m): State<Mock>, UrlPath((org, repo)): UrlPath<(String, String)>) -> (StatusCode, axum::Json<Value>) {
    m.seen().releases += 1;
    if (org.as_str(), repo.as_str()) != ("toy", "web") {
        m.seen().unexpected.push(format!("releases of {org}/{repo}"));
        return (StatusCode::NOT_FOUND, axum::Json(json!({"message": "Not Found"})));
    }
    (
        StatusCode::OK,
        axum::Json(json!([
            {"tag_name": "v1.3.0", "body": "Draft.", "draft": true, "prerelease": false},
            {"tag_name": "v1.3.0-rc.1", "body": "Try it.", "draft": false, "prerelease": true},
            {"tag_name": "v1.2.0", "body": "Fixes CVE-2026-4411, a remote crash in the parser. Upgrade soon.", "draft": false, "prerelease": false},
            {"tag_name": "v1.1.0", "body": "Faster startup and clearer logs.", "draft": false, "prerelease": false},
            {"tag_name": "v1.0.0", "body": "First release.", "draft": false, "prerelease": false},
        ])),
    )
}

async fn hook(State(m): State<Mock>, body: String) -> &'static str {
    let v = serde_json::from_str(&body).unwrap_or(Value::String(body));
    m.seen().hooks.push(v);
    "ok"
}

/// The temporary box: directories, the daemon, the dashboard, the toy server's pid file.
struct Box_ {
    dir: PathBuf,
    state: PathBuf,
    config: PathBuf,
    daemon: Option<tokio::process::Child>,
    dash: Option<tokio::process::Child>,
    /// The mock server, which plays Steery too.
    steery_url: String,
}

impl Drop for Box_ {
    fn drop(&mut self) {
        for c in [&mut self.daemon, &mut self.dash].into_iter().flatten() {
            let _ = c.start_kill();
        }
        kill_pidfile(&self.dir.join("toy.pid"));
        // A killed daemon leaves its LINES followers behind.
        for log in ["app.log", "events.log"] {
            let _ = std::process::Command::new("pkill")
                .arg("-f")
                .arg(self.dir.join(log))
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn kill_pidfile(file: &Path) {
    if let Some(pid) = std::fs::read_to_string(file)
        .ok()
        .and_then(|p| p.trim().parse::<i32>().ok())
    {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status();
    }
}

impl Box_ {
    fn std_command(&self) -> std::process::Command {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_subweave"));
        c.env("SUBWEAVE_STATE", &self.state)
            .env("SUBWEAVE_CONFIG", &self.config)
            .env("MOCK_API_KEY", API_KEY)
            .env("STEERY_URL", &self.steery_url)
            .env("TOY_TOKEN", SECRET)
            .env("GITHUB_API_URL", &self.steery_url)
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::null());
        c
    }

    fn command(&self) -> tokio::process::Command {
        let mut c = tokio::process::Command::from(self.std_command());
        c.kill_on_drop(true);
        c
    }

    /// `subweave daemon` in the background, its output appended to `daemon.log`; `otel`
    /// receives its traces.
    fn start_daemon(&mut self, otel: &str) {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("daemon.log"))
            .unwrap();
        let daemon = self
            .command()
            .env_remove("TOY_TOKEN")
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", otel)
            .env("OTEL_BSP_SCHEDULE_DELAY", "200")
            .arg("daemon")
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .expect("the daemon starts");
        self.daemon = Some(daemon);
    }

    /// Kills the daemon, as a crash would, and starts another.
    async fn restart_daemon(&mut self, otel: &str) {
        if let Some(mut d) = self.daemon.take() {
            d.kill().await.expect("the daemon dies");
        }
        self.start_daemon(otel);
    }

    async fn sw(&self, args: &[&str]) -> Output {
        self.command()
            .args(args)
            .output()
            .await
            .expect("subweave runs")
    }

    /// `subweave <args>` that must succeed; its stdout.
    async fn ok(&self, args: &[&str]) -> String {
        let o = self.sw(args).await;
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        if !o.status.success() {
            self.fail(&format!(
                "subweave {} exited {}\nstdout:\n{out}\nstderr:\n{}",
                args.join(" "),
                o.status,
                String::from_utf8_lossy(&o.stderr)
            ));
        }
        out
    }

    fn service(&self) -> PathBuf {
        self.state.join("services/toy")
    }

    fn journal(&self) -> Vec<Value> {
        self.journal_of("toy")
    }

    fn journal_of(&self, service: &str) -> Vec<Value> {
        std::fs::read_to_string(self.state.join("services").join(service).join("journal.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn lines(&self, kind: &str, f: impl Fn(&Value) -> bool) -> Vec<Value> {
        self.journal()
            .into_iter()
            .filter(|l| l["kind"] == kind && f(l))
            .collect()
    }

    /// `action` lines of `action` that passed.
    fn passes(&self, action: &str) -> usize {
        self.lines("action", |l| l["action"] == action && l["ok"] == true)
            .len()
    }

    /// The toy service's row in `subweave ps --json`: its journal's fold.
    fn status(&self) -> Value {
        self.row("toy")
    }

    fn row(&self, service: &str) -> Value {
        let o = self
            .std_command()
            .args(["ps", "--json"])
            .output()
            .expect("subweave runs");
        serde_json::from_slice::<Vec<Value>>(&o.stdout)
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r["service"] == service))
            .unwrap_or(Value::Null)
    }

    fn goal(&self, name: &str) -> String {
        self.status()["states"][name]["state"]
            .as_str()
            .unwrap_or("-")
            .to_string()
    }

    fn learned(&self) -> Value {
        std::fs::read(self.service().join("learned.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null)
    }

    fn pid(&self) -> String {
        std::fs::read_to_string(self.dir.join("toy.pid")).unwrap_or_default()
    }

    fn fail(&self, what: &str) -> ! {
        let log = std::fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default();
        let journal =
            std::fs::read_to_string(self.service().join("journal.jsonl")).unwrap_or_default();
        panic!(
            "{what}\n--- ps --json\n{}\n--- journal.jsonl\n{journal}\n--- daemon log\n{log}",
            self.status()
        );
    }

    /// Polls `f` every 100 ms until it gives a value, or fails after `secs`.
    async fn until<T>(&self, what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            if let Some(v) = f() {
                return v;
            }
            if Instant::now() >= end {
                self.fail(&format!("timed out after {secs}s waiting for {what}"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn holds(&self, goals: &[&str], secs: u64) {
        self.until(&format!("{goals:?} to hold"), secs, || {
            goals.iter().all(|g| self.goal(g) == "holds").then_some(())
        })
        .await;
    }

    /// The open ask for `goal` in the journal: its number and its buttons' words.
    fn open_ask(&self, goal: &str) -> Option<(u64, Vec<String>)> {
        let j = self.journal();
        let settled: Vec<u64> = j
            .iter()
            .filter(|l| l["kind"] == "answer" || l["kind"] == "withdraw")
            .filter_map(|l| l["n"].as_u64())
            .collect();
        j.iter()
            .filter(|l| l["kind"] == "ask" && l["goal"] == goal)
            .filter_map(|l| Some((l["n"].as_u64()?, l)))
            .find(|(n, _)| !settled.contains(n))
            .map(|(n, l)| (n, buttons(&l["card"])))
    }

    /// `subweave do toy rotate`: ASK BEFORE rotate escalates it; the new ask's number.
    async fn rotate_ask(&self) -> u64 {
        let asks = |b: &Box_| -> Vec<u64> {
            b.lines("ask", |l| l["action"] == "rotate")
                .iter()
                .filter_map(|l| l["n"].as_u64())
                .collect()
        };
        let before = asks(self);
        self.ok(&["do", "toy", "rotate"]).await;
        self.until("the ask for a rotate by hand", 20, || {
            asks(self).into_iter().find(|n| !before.contains(n))
        })
        .await
    }

    /// The answer line of ask `n`, once it has one.
    async fn answered(&self, n: u64, secs: u64) -> Value {
        self.until(&format!("the answer to #{n}"), secs, || {
            self.lines("answer", |l| l["n"] == n).into_iter().next()
        })
        .await
    }

    /// The first note whose text starts with `head`, once there is one.
    async fn note(&self, head: &str, secs: u64) -> Value {
        self.until(&format!("a note {head:?}"), secs, || {
            self.lines("note", |l| l["text"].as_str().is_some_and(|t| t.starts_with(head)))
                .into_iter()
                .next()
        })
        .await
    }

    /// `subweave <args>` with `input` on stdin.
    async fn sw_stdin(&self, args: &[&str], input: &str) -> Output {
        use tokio::io::AsyncWriteExt;
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("subweave runs");
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).await.unwrap();
        drop(stdin);
        child.wait_with_output().await.expect("subweave runs")
    }
}

/// `subweave mcp` with the test's environment: one JSON-RPC message per line.
struct Mcp {
    _child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::Lines<tokio::io::BufReader<tokio::process::ChildStdout>>,
    id: u64,
}

impl Mcp {
    fn start(b: &Box_) -> Mcp {
        let mut child = b
            .command()
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("subweave mcp runs");
        let stdin = child.stdin.take().unwrap();
        let stdout = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(
            child.stdout.take().unwrap(),
        ));
        Mcp {
            _child: child,
            stdin,
            stdout,
            id: 0,
        }
    }

    async fn send(&mut self, msg: &Value) {
        use tokio::io::AsyncWriteExt;
        eprintln!("mcp > {msg}");
        self.stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .unwrap();
    }

    /// A request's response.
    async fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let id = self.id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(20), self.stdout.next_line())
                .await
                .expect("an MCP response within 20s")
                .unwrap()
                .expect("subweave mcp is still running");
            eprintln!("mcp < {line}");
            let v: Value = serde_json::from_str(&line).expect("stdout holds only JSON-RPC");
            if v["id"] == id {
                return v;
            }
        }
    }

    /// A `tools/call` result, and its text parsed as JSON when it is JSON.
    async fn call(&mut self, tool: &str, args: Value) -> (Value, Value) {
        let r = self
            .request("tools/call", json!({"name": tool, "arguments": args}))
            .await;
        let result = r["result"].clone();
        assert!(result.is_object(), "{r}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        let parsed = serde_json::from_str(text).unwrap_or(Value::Null);
        (result, parsed)
    }
}

/// `macos:26` or `ubuntu:24.04`: what `src/boxinfo.rs` reads here.
fn from_line() -> String {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .expect("sw_vers runs");
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let major = v.split('.').next().unwrap_or_default().to_string();
        format!("macos:{major}")
    } else {
        let release = std::fs::read_to_string("/etc/os-release").expect("/etc/os-release");
        let field = |key: &str| {
            release
                .lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .map(|v| v.trim().trim_matches('"').to_string())
                .unwrap_or_default()
        };
        format!("{}:{}", field("ID"), field("VERSION_ID"))
    }
}

fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free port")
        .port()
}

fn weavefile(dir: &Path, www: &Path) -> String {
    let d = dir.display();
    format!(
        r#"# The e2e toy: a static hello page, kept up by the daemon.
ABOUT  "a static HTTP server on 127.0.0.1 that serves one hello page"
FROM   {from}

ARG port                MUST "a free TCP port, like 18731"
ARG root={www}  MAY  "the directory to serve, like /tmp/www"

SECRET TOY_TOKEN "a token the page never shows"

NEVER IF port < 1024
NEVER "{FIREWALL_RULE}"
ASK   BEFORE rotate BECAUSE "a rotate starts the server log over"
ASK   BEFORE CHANGES data BECAUSE "it deletes what the spool holds"

FACT present  BOOL    test -f $root/index.html
FACT scene    TEXT    echo "$(cat {d}/scene 2>/dev/null) $TOY_TOKEN"
FACT served   NUMBER  "1 when http://127.0.0.1:$port/ answers HTTP 200 with a page that contains the word hello, else 0"
FACT errors   LINES   tail -n 0 -F {d}/app.log
FACT events   LINES   tail -n 0 -F {d}/events.log
FACT spooled  NUMBER  ls {d}/spool | wc -l
FACT setting  TEXT    cat {d}/setting
FACT drills   TEXT    cat {d}/drills 2>/dev/null || true

PART web github:toy/web VERSION cat {d}/web-version

GOAL installed "the hello page is installed"                  EVERY 1s                   present
GOAL running   "the hello page is up" \
               AFTER installed  EVERY 1s  TRIES 10 PER 1h  served == 1
GOAL rotated   "the server log is fresh"  AFTER running    EVERY 1s                   SINCE(rotate) < 1h
GOAL calm      "the app log stays quiet"  EVERY 2s  OVER 12s  COUNT(errors) == 0
# OVER outlasts a failure's two checks and the repair's start, which drops a goal that holds.
GOAL quiet     "the event log shows no trouble"  EVERY 1s  OVER 8s  TRIES 10 PER 1h \
               "the lines in `events` show nothing that needs action"
GOAL tidy      "the spool is empty"       EVERY 1s  spooled == 0
GOAL tuned     "the setting is on"        EVERY 1s  TRIES 1 PER 1h  setting == 'on'
GOAL patched   "no security release of web waits" \
               EVERY 1s  web.newest >= web.running AND NOT web.security

KNOWN quiet NORMAL "{HEARTBEAT}"

ACTION install FOR installed WITHIN 5s CHANGES config
  WHEN "the directory or its index.html is missing: nothing is installed yet"
  DO   "create the directory $root and write $root/index.html, a small HTML page that says hello"

ACTION restart "Restart the server" FOR running WITHIN 5s CHANGES restart
  WHEN "the fix when the server process is gone, crashed or was killed, and its files are in place: a restart brings it back"
  RUN <<EOF
    kill "$(cat {d}/toy.pid 2>/dev/null)" 2>/dev/null || true
    cd "$root"
    nohup python3 -m http.server "$port" --bind 127.0.0.1 >{d}/http.log 2>&1 &
    echo $! > {d}/toy.pid
    for i in $(seq 100); do curl -fs "http://127.0.0.1:$port/" >/dev/null 2>&1 && exit 0; sleep 0.1; done
    exit 1
  EOF

ACTION fix FOR installed, running WITHIN 5s CHANGES config, restart
  WHEN "no other action fits, or the fitting ones already failed"
  DO   "make the failing goal hold again"

ACTION rotate "Start a fresh server log" FOR rotated WITHIN 5s CHANGES config
  WHEN "the server log needs a fresh start"
  RUN  date > {d}/rotated

ACTION hush "Clear the disk alarm" FOR quiet WITHIN 2s CHANGES nothing
  WHEN "a line in `events` says a disk failed"
  RUN  date >> {d}/hushed

ACTION mute "Mute the event log" FOR quiet WITHIN 2s CHANGES nothing
  WHEN "the event log is noisy with lines that need no action"
  RUN  date >> {d}/muted

ACTION purge "Empty the spool" FOR tidy WITHIN 3s CHANGES data
  WHEN "files wait in the spool"
  RUN  rm -f {d}/spool/*

ACTION tune "Turn the setting on" FOR tuned WITHIN 2s CHANGES config UNDO untune
  WHEN "the setting is off"
  RUN  echo half > {d}/setting

ACTION untune "Put the setting back" WITHIN 3s CHANGES config
  WHEN "tune made things worse"
  RUN  echo off > {d}/setting

ACTION upgrade "Upgrade web" FOR patched WITHIN 3s CHANGES version
  WHEN "a newer release of web fixes a security issue"
  RUN  echo 1.2.0 > {d}/web-version

DRILL down "Kill the server" BREAK pkill -f "http.server $port" \
      EXPECT running FAILS WITHIN 10s HOLDS WITHIN 40s EVERY 2s IF drills == 'on'
  RESTORE date > {d}/down-restored

DRILL blind "Leave a note outside the spool" BREAK date > {d}/blind-broke \
      EXPECT tidy FAILS WITHIN 3s HOLDS WITHIN 6s
  RESTORE date > {d}/blind-restored
"#,
        from = from_line(),
        www = www.display(),
    )
}

/// A second service on the toy's box: its goal comes AFTER the toy's `tidy` and reads the
/// toy's `spooled`.
fn relay_weavefile(dir: &Path) -> String {
    format!(
        r#"ABOUT  "relays what the toy's spool held"
FROM   {from}

GOAL relayed "the toy's spool is relayed"  AFTER toy:tidy  EVERY 1s  toy:spooled == 0

ACTION relay "Relay again" FOR relayed WITHIN 2s CHANGES nothing
  WHEN "the relay stopped"
  RUN  date >> {d}/relayed
"#,
        from = from_line(),
        d = dir.display(),
    )
}

const DASH_PASSWORD: &str = "pw-e2e-41c7";

/// Reads an event stream until it holds `want`, or fails after `secs`; what came.
async fn sse_until(b: &Box_, events: &mut reqwest::Response, got: &mut String, want: &str, secs: u64) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !got.contains(want) {
        let left = end.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, events.chunk()).await {
            Ok(Ok(Some(c))) => got.push_str(&String::from_utf8_lossy(&c)),
            Ok(Ok(None)) => b.fail(&format!("the event stream ended before {want:?}:\n{got}")),
            Ok(Err(e)) => b.fail(&format!("the event stream failed: {e}\n{got}")),
            Err(_) => b.fail(&format!("no {want:?} in the event stream in {secs}s:\n{got}")),
        }
    }
}

/// `subweave dash` with a password, against the running daemon: login, the services,
/// a journal line live over SSE, and a task in words that the mock coder runs.
async fn dashboard(b: &mut Box_) {
    use tokio::io::AsyncWriteExt;
    let port = free_port();
    let o = b.sw(&["dash"]).await;
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        !o.status.success() && err.contains("set one first: subweave dash --password"),
        "a dashboard with no password does not start: {err}"
    );
    // On every address: a scrape from a non-loopback one needs the password.
    let mut set = b.command();
    set.args(["dash", "--bind", "0.0.0.0", "--port", &port.to_string(), "--password"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = set.spawn().expect("subweave dash --password runs");
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(format!("{DASH_PASSWORD}\n").as_bytes())
        .await
        .unwrap();
    drop(stdin);
    let o = child.wait_with_output().await.unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && out.contains("password set"), "{out}");
    let saved = std::fs::read_to_string(b.config.join("dash.json")).unwrap();
    assert!(saved.contains("$argon2id$") && !saved.contains(DASH_PASSWORD), "{saved}");

    let log = std::fs::File::create(b.dir.join("dash.log")).unwrap();
    let dash = b
        .command()
        .arg("dash")
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("the dashboard starts");
    b.dash = Some(dash);
    let api = |p: &str| format!("http://127.0.0.1:{port}/api{p}");
    let http = reqwest::Client::new();
    let mut session = None;
    for _ in 0..100 {
        if let Ok(r) = http.get(api("/session")).send().await {
            session = Some(r.json::<Value>().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(session, Some(json!({"ok": false})));
    let r = http
        .get(api("/session"))
        .header("host", "rebound.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 421, "a host name the dashboard does not answer to");
    let r = http.get(api("/services")).send().await.unwrap();
    assert_eq!(r.status(), 401, "no session, no services");
    let r = http
        .post(api("/login"))
        .json(&json!({"password": "not it"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = http
        .post(api("/login"))
        .json(&json!({"password": DASH_PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let set_cookie = r.headers()["set-cookie"].to_str().unwrap().to_string();
    assert!(
        set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Strict"),
        "{set_cookie}"
    );
    let cookie = set_cookie.split(';').next().unwrap().to_string();

    let services: Value = http
        .get(api("/services"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let toy = services
        .as_array()
        .and_then(|s| s.iter().find(|s| s["name"] == "toy"))
        .unwrap_or_else(|| b.fail(&format!("no toy in /api/services: {services}")));
    assert_eq!(toy["state"], "running", "{toy}");

    let mut events = http
        .get(api("/events"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(events.status(), 200);
    let mut got = String::new();
    sse_until(b, &mut events, &mut got, "event: hello", 10).await;

    let r = http
        .post(api("/services/toy/task"))
        .header("cookie", &cookie)
        .json(&json!({"text": TASK_TEXT}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    // The daemon writes the task's start line; the dashboard streams it.
    sse_until(b, &mut events, &mut got, TASK_TEXT, 30).await;
    let start = got
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .find(|e| e["line"]["text"] == TASK_TEXT)
        .unwrap_or_else(|| b.fail(&format!("no task line in the stream:\n{got}")));
    assert_eq!(start["service"], "toy");
    assert_eq!(start["line"]["kind"], "action");
    assert_eq!(start["line"]["action"], "task");
    assert_eq!(start["line"]["by"], "dash@127.0.0.1");
    assert!(start["line"]["i"].is_u64(), "{start}");
    assert!(start["line"].get("ok").is_none(), "{start}");

    let end = b
        .until("the task to end", 30, || {
            b.lines("action", |l| l["action"] == "task" && l["ok"].is_boolean())
                .first()
                .cloned()
        })
        .await;
    assert_eq!(end["ok"], true, "{end}");
    assert_eq!(end["text"], TASK_SUMMARY);
    assert_eq!(end["by"], "dash@127.0.0.1");
    // The start line, the command and the end are one episode.
    assert!(start["line"]["ep"].is_string(), "{start}");
    assert_eq!(end["ep"], start["line"]["ep"], "{end}");
    let cmd = b.lines("command", |l| l["action"] == "task");
    assert_eq!(cmd.len(), 1, "one command for the task");
    assert!(cmd[0].get("goal").is_none(), "{}", cmd[0]);
    assert_eq!(cmd[0]["text"], TASK_CMD);
    assert_eq!(cmd[0]["ep"], start["line"]["ep"], "{}", cmd[0]);
    assert!(
        cmd[0]["output"]
            .as_str()
            .is_some_and(|o| o.starts_with("tidied ") && o.contains(" on port ")),
        "{}",
        cmd[0]
    );
    assert!(
        b.lines("note", |l| l["action"] == "task"
            && l["text"].as_str().is_some_and(|t| t.starts_with("coder ")))
            .len()
            == 1,
        "the task's coder session is journaled"
    );

    // `/metrics` shows what the flow did.
    let metrics = format!("http://127.0.0.1:{port}/metrics");
    let r = http
        .get(&metrics)
        .bearer_auth(DASH_PASSWORD)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain; version=0.0.4"),
        "{:?}",
        r.headers()
    );
    let text = r.text().await.unwrap();
    // The sum of the samples whose line starts with `series` and holds every `label`.
    let value = |series: &str, labels: &[&str]| -> f64 {
        let found: Vec<f64> = text
            .lines()
            .filter(|l| l.starts_with(series) && labels.iter().all(|x| l.contains(x)))
            .filter_map(|l| l.rsplit(' ').next()?.parse().ok())
            .collect();
        if found.is_empty() {
            b.fail(&format!("no {series} {labels:?} in /metrics:\n{text}"));
        }
        found.iter().sum()
    };
    let install_ok = value(
        "subweave_actions_total{",
        &[r#"action="install""#, r#"goal="installed""#, r#"ok="true""#],
    );
    assert_eq!(install_ok, b.passes("install") as f64, "{text}");
    assert_eq!(
        value("subweave_goal_holds{", &[r#"service="toy""#, r#"goal="running""#]),
        1.0
    );
    assert!(
        value(
            "subweave_model_tokens_total{",
            &[r#"model="deepseek/x"#, r#"type="input""#]
        ) > 0.0,
        "{text}"
    );
    assert!(
        value("subweave_goal_recovery_seconds_count{", &[r#"goal="running""#]) >= 1.0,
        "{text}"
    );
    assert!(
        value(
            "subweave_episodes_total{",
            &[r#"kind="task""#, r#"outcome="ok""#]
        ) >= 1.0,
        "{text}"
    );
    assert!(
        value("subweave_action_duration_seconds_count{", &[r#"action="restart""#]) >= 1.0,
        "{text}"
    );
    let r = http.get(&metrics).send().await.unwrap();
    assert_eq!(r.status(), 200, "a loopback scrape needs no password");
    // A proxy on this box forwards for a client elsewhere: the password is needed.
    let forwarded = |auth: Option<&str>| {
        let mut req = http.get(&metrics).header("x-forwarded-for", "203.0.113.7");
        if let Some(a) = auth {
            req = req.bearer_auth(a);
        }
        req.send()
    };
    let r = forwarded(None).await.unwrap();
    assert_eq!(r.status(), 401, "a forwarded scrape with no password");
    let r = forwarded(Some("not it")).await.unwrap();
    assert_eq!(r.status(), 401, "a forwarded scrape with a wrong password");
    let r = forwarded(Some(DASH_PASSWORD)).await.unwrap();
    assert_eq!(r.status(), 200, "a forwarded scrape with the password");
    // A scrape from this box's own non-loopback address, when it has one and can reach it.
    let lan = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|()| s))
        .and_then(|s| s.local_addr())
        .map(|a| a.ip())
        .ok()
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified());
    let reached = match lan {
        Some(ip) => {
            let url = format!("http://{ip}:{port}/metrics");
            let get = |auth: Option<&str>| {
                let mut req = http.get(&url).timeout(Duration::from_secs(5));
                if let Some(a) = auth {
                    req = req.bearer_auth(a);
                }
                req.send()
            };
            match get(None).await {
                Ok(r) => {
                    assert_eq!(r.status(), 401, "{ip}: no password, no metrics");
                    let r = get(Some("not it")).await.unwrap();
                    assert_eq!(r.status(), 401, "{ip}: a wrong password");
                    let r = get(Some(DASH_PASSWORD)).await.unwrap();
                    assert_eq!(r.status(), 200, "{ip}: the password");
                    let r = get(Some(DASH_PASSWORD)).await.unwrap();
                    assert_eq!(r.status(), 200, "{ip}: the kept password");
                    true
                }
                Err(e) => {
                    eprintln!("skipped the non-loopback scrape: {url}: {e}");
                    false
                }
            }
        }
        None => false,
    };
    eprintln!(
        "/metrics, non-loopback half {}:\n{}",
        if reached { "ran" } else { "skipped" },
        text.lines()
            .filter(|l| l.contains("service=\"toy\"")
                && (l.starts_with("subweave_actions_total")
                    || l.starts_with("subweave_goal_holds")
                    || l.starts_with("subweave_episodes_total")
                    || l.starts_with("subweave_model_tokens_total")
                    || l.starts_with("subweave_goal_recovery_seconds_count")))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

const AGENT_NOTE: &str = "the agent ran the rotate";
const GOAL_WHY: &str = "a rotate every two hours is enough";
const RULE_WHY: &str = "the web root holds the only copy of the page";
const WIPE_RULE: &str = "delete the web root";

/// stdout and stderr.
fn both(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Escalation targets: an agent channel settles an escalation; Steery session mode
/// connects once, sends straight to the stored session, and connects again when it is
/// gone; an agent changes the weave, a goal at once and a rule on the person's signed
/// answer only; `stats` counts who settled what.
async fn escalation(b: &Box_, mock: &Mock, weave: &str, port: u16, www: &Path) {
    let dir = &b.dir;
    let d = dir.display();
    let sw = env!("CARGO_BIN_EXE_subweave");
    let svc_dir = b.service();
    let base = std::fs::read_to_string(svc_dir.join("Weavefile")).unwrap();

    // 9a. What an agent reads first, and changes refused before the daemon hears them.
    eprintln!("9a weave");
    let out = b.ok(&["weave", "toy"]).await;
    assert!(out.starts_with(&base), "{out}");
    assert!(
        out.contains(&format!("\nport={port}\n")) && out.contains(&format!("\nroot={}\n", www.display())),
        "{out}"
    );
    assert!(out.ends_with(&format!("\nsource: {weave}\n")), "{out}");
    let goal_text = base.replace("SINCE(rotate) < 1h", "SINCE(rotate) < 2h");
    let firewall = format!("NEVER \"{FIREWALL_RULE}\"\n");
    let rule_text = goal_text.replace(&firewall, &format!("{firewall}NEVER \"{WIPE_RULE}\"\n"));
    assert!(goal_text != base && rule_text != goal_text);
    let (goal_file, rule_file, bad_file) = (
        dir.join("goal.Weavefile"),
        dir.join("rule.Weavefile"),
        dir.join("bad.Weavefile"),
    );
    std::fs::write(&goal_file, &goal_text).unwrap();
    std::fs::write(&rule_file, &rule_text).unwrap();
    std::fs::write(&bad_file, format!("{base}\nGOAL\n")).unwrap();
    let path = |p: &PathBuf| p.display().to_string();
    for (file, why, want) in [
        (&bad_file, "a test", "the new Weavefile does not parse"),
        (&goal_file, "", "say why, in a sentence"),
        (&rule_file, RULE_WHY, "only a signed answer on Steery settles that: this box has no Steery channel"),
    ] {
        let o = b.sw(&["weave", "toy", "--change", &path(file), "--why", why]).await;
        assert!(!o.status.success() && both(&o).contains(want), "{}", both(&o));
    }
    assert_eq!(std::fs::read_to_string(svc_dir.join("Weavefile")).unwrap(), base);

    // 9b. An agent channel: its command reads the escalation on stdin and answers it.
    eprintln!("9b agent channel");
    let script = dir.join("answer.sh");
    std::fs::write(
        &script,
        format!(
            "cat > {d}/agent-$SUBWEAVE_ASK.md\n\
             echo \"agent in $(pwd) for $SUBWEAVE_SERVICE #$SUBWEAVE_ASK, token {SECRET}\"\n\
             exec {sw} answer \"$SUBWEAVE_ASK\" yes --note '{AGENT_NOTE}'\n"
        ),
    )
    .unwrap();
    let command = format!("sh {}", script.display());
    let out = b.ok(&["channel", "add", "agent", &command]).await;
    assert!(out.contains(&format!("added agent: agent {command}, level ask")), "{out}");
    let rotates = b.passes("rotate");
    let n = b.rotate_ask().await;
    let answer = b.answered(n, 20).await;
    assert_eq!(answer["text"], "yes", "{answer}");
    assert_eq!(answer["by"], "agent", "{answer}");
    let noted = b.lines("note", |l| l["text"] == AGENT_NOTE);
    assert!(noted.len() == 1 && noted[0]["by"] == "agent", "{noted:?}");
    let ended = b.note(&format!("escalated #{n} to agent: "), 10).await;
    assert_eq!(
        ended["text"],
        format!("escalated #{n} to agent: exit 0; output in escalations/{n}.log")
    );
    let log_file = svc_dir.join(format!("escalations/{n}.log"));
    let log = std::fs::read_to_string(&log_file).unwrap();
    assert!(
        log.contains(&format!(
            "agent in {} for toy #{n}, token [TOY_TOKEN]",
            svc_dir.display()
        )) && !log.contains(SECRET),
        "{log}"
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&log_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let read = std::fs::read_to_string(dir.join(format!("agent-{n}.md"))).unwrap();
    assert!(read.starts_with(&format!("# Escalation #{n}: ")), "{read}");
    assert!(
        read.contains(&format!("`subweave answer {n} <word> --note")) && read.contains("# Subweave escalation"),
        "{read}"
    );
    b.until("the agent's yes to run rotate", 20, || {
        (b.passes("rotate") > rotates).then_some(())
    })
    .await;
    b.ok(&["channel", "rm", "agent"]).await;

    // 9c. Steery session mode: the first escalation connects a conversation, the next
    // goes straight to it, and a gone session connects again.
    eprintln!("9c steery session");
    let o = b.sw_stdin(&["channel", "add", "steery"], STEERY_KEY).await;
    assert!(
        !o.status.success() && both(&o).contains("needs --reach USER@HOST"),
        "{}",
        both(&o)
    );
    let o = b
        .sw_stdin(&["channel", "add", "steery", "--reach", REACH], STEERY_KEY)
        .await;
    let out = both(&o);
    assert!(o.status.success(), "{out}");
    assert!(
        out.contains(&format!(
            "added steery: steery {WORKSPACE}, level ask, escalates session via {REACH}"
        )),
        "{out}"
    );
    assert!(out.lines().any(|l| l.starts_with("phone") && l.ends_with(&format!("{SIGNER_NAME}  pinned"))), "{out}");
    let out = b.ok(&["channel", "ls"]).await;
    assert!(
        out.contains(&format!("escalates session via {REACH}  signers: {SIGNER_NAME}")),
        "{out}"
    );
    let connected = || -> Option<Value> {
        serde_json::from_slice(&std::fs::read(svc_dir.join("escalation.json")).ok()?).ok()
    };
    let n1 = b.rotate_ask().await;
    let (throw, request) = b
        .until("the connect throw and its request", 20, || {
            let t = mock.throws_for(n1).into_iter().next()?;
            let r = mock.posts("/v1/requests").into_iter().find(|r| r["throw"] == t["id"])?;
            Some((t, r))
        })
        .await;
    assert!(throw.get("session").is_none() && throw.get("act").is_none(), "{throw}");
    let text = throw["body"].as_str().unwrap_or_default();
    for want in [
        format!("`ssh {REACH} sudo -n subweave answer {n1} <word> --note"),
        format!("`ssh {REACH} sudo -n subweave log toy`"),
        format!("`ssh {REACH} sudo -n subweave mcp`"),
        "# Subweave escalation".to_string(),
    ] {
        assert!(text.contains(&want), "the throw lacks {want:?}:\n{text}");
    }
    assert!(request.get("input").is_none() && request.get("payload").is_none(), "{request}");
    assert!(
        request["body"].as_str().is_some_and(|t| t.contains("pick the agent conversation")),
        "{request}"
    );
    assert!(connected().is_none());
    let s1 = "0199f3a0-3e4f-7a51-8c7d-8e9f0a1b2c3d";
    mock.deliver(throw["id"].as_str().unwrap(), s1);
    b.until("the connected session", 20, || {
        connected().filter(|c| c["steery_session"] == s1)
    })
    .await;
    b.ok(&["answer", &n1.to_string(), "yes"]).await;

    let requests = mock.posts("/v1/requests").len();
    let n2 = b.rotate_ask().await;
    let t2 = b
        .until("the throw to the session", 20, || mock.throws_for(n2).into_iter().next())
        .await;
    assert!(t2["session"] == s1 && t2["act"] == "continue", "{t2}");
    assert!(
        t2["body"].as_str().is_some_and(|t| t.contains("# Subweave escalation")),
        "the first throw to a new session carries the skill: {t2}"
    );
    b.until("the skill marked sent", 10, || {
        connected().filter(|c| c["skill_sent"] == true)
    })
    .await;
    assert_eq!(mock.posts("/v1/requests").len(), requests, "a connected session needs no request");
    b.ok(&["answer", &n2.to_string(), "yes"]).await;

    // The session is gone: forgotten, and a new connect throw and request go. Its ask
    // settles before the person's Go; the Go still connects.
    mock.steery().gone.push(s1.to_string());
    let n3 = b.rotate_ask().await;
    let t3 = b
        .until("the connect throw again", 20, || {
            let t = mock.throws_for(n3).into_iter().find(|t| t.get("session").is_none())?;
            mock.posts("/v1/requests")
                .iter()
                .any(|r| r["throw"] == t["id"])
                .then_some(t)
        })
        .await;
    let tried = mock.throws_for(n3);
    assert!(tried.len() == 2 && tried[0]["session"] == s1, "{tried:?}");
    assert!(connected().is_none(), "the gone session is forgotten");
    b.ok(&["answer", &n3.to_string(), "yes"]).await;
    let s2 = "0199f3a0-4f50-7b62-9d8e-9f0a1b2c3d4e";
    mock.deliver(t3["id"].as_str().unwrap(), s2);
    b.until("the session connected again", 20, || {
        connected().filter(|c| c["steery_session"] == s2)
    })
    .await;

    // 9d. An agent changes the weave: a goal at once; a rule only on the signed answer.
    eprintln!("9d weave changes by an agent");
    let spec_before: Value =
        serde_json::from_slice(&std::fs::read(svc_dir.join("service.json")).unwrap()).unwrap();
    let script = dir.join("change.sh");
    std::fs::write(
        &script,
        format!(
            "cat > /dev/null\n\
             {sw} weave toy > {d}/weave-read.txt\n\
             {sw} weave toy --change {goal} --why '{GOAL_WHY}' > {d}/goal.out 2>&1\n\
             for i in $(seq 100); do {sw} weave toy | grep -q 'SINCE(rotate) < 2h' && break; sleep 0.2; done\n\
             {sw} weave toy --change {rule} --why '{RULE_WHY}' > {d}/rule.out 2>&1\n\
             exec {sw} answer \"$SUBWEAVE_ASK\" yes --note 'the weave changes are proposed'\n",
            goal = goal_file.display(),
            rule = rule_file.display(),
        ),
    )
    .unwrap();
    let command = format!("sh {}", script.display());
    b.ok(&["channel", "add", "agent", &command]).await;
    let n4 = b.rotate_ask().await;
    let answer = b.answered(n4, 60).await;
    assert_eq!(answer["by"], "agent", "{answer}");
    let t4 = b
        .until("the throw to the new session", 20, || mock.throws_for(n4).into_iter().next())
        .await;
    assert!(
        t4["session"] == s2 && t4["body"].as_str().is_some_and(|t| t.contains("# Subweave escalation")),
        "{t4}"
    );
    let read = std::fs::read_to_string(dir.join("weave-read.txt")).unwrap();
    assert!(read.starts_with(&base), "{read}");
    assert_eq!(
        std::fs::read_to_string(dir.join("goal.out")).unwrap(),
        "handed the change to toy to the daemon: GOAL rotated changed\n"
    );
    let rule_out = std::fs::read_to_string(dir.join("rule.out")).unwrap();
    assert!(
        rule_out.starts_with(
            "the rules of toy change: an escalation asks the person for a signed answer on Steery\n"
        ) && rule_out.contains(&format!("+ NEVER \"{WIPE_RULE}\"")),
        "{rule_out}"
    );
    let changed = b.note(&format!("weave changed by agent: {GOAL_WHY}"), 10).await;
    assert_eq!(
        changed["text"],
        format!("weave changed by agent: {GOAL_WHY}; GOAL rotated changed")
    );
    assert_eq!(changed["by"], "agent");
    let spec: Value =
        serde_json::from_slice(&std::fs::read(svc_dir.join("service.json")).unwrap()).unwrap();
    assert_eq!(spec["args"], spec_before["args"]);
    assert_eq!(spec["created"], spec_before["created"]);
    assert!(std::fs::read_to_string(svc_dir.join("secrets.json")).unwrap().contains(SECRET));
    let running = std::fs::read_to_string(svc_dir.join("Weavefile")).unwrap();
    assert!(running.contains("SINCE(rotate) < 2h") && !running.contains(WIPE_RULE), "{running}");

    let signed = b
        .until("the signed ask", 20, || {
            b.lines("ask", |l| l["action"] == "weave").into_iter().next()
        })
        .await;
    let ns = signed["n"].as_u64().unwrap();
    assert_eq!(signed["signed"], true, "{signed}");
    assert_eq!(signed["card"]["title"], "Change the rules of toy?", "{signed}");
    assert_eq!(buttons(&signed["card"]), ["yes", "no"], "{signed}");
    let lines = card_lines(&signed["card"]);
    assert!(
        lines.contains(RULE_WHY) && lines.contains(&format!("Added: NEVER \"{WIPE_RULE}\"")),
        "{signed}"
    );
    let out = b.ok(&["inbox"]).await;
    let row = out
        .lines()
        .find(|l| l.starts_with(&format!("#{ns} ")))
        .unwrap_or_else(|| b.fail(&format!("no #{ns} in inbox:\n{out}")));
    assert!(row.contains("Change the rules of toy?") && row.ends_with("signed: answer on Steery"), "{row}");
    let refused = "only a signed answer on Steery settles this";
    let o = b.sw(&["answer", &ns.to_string(), "yes"]).await;
    assert!(!o.status.success() && both(&o).contains(refused), "{}", both(&o));
    let mut mcp = Mcp::start(b);
    mcp.request(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "e2e-agent", "version": "1"}}),
    )
    .await;
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    let (bad, _) = mcp
        .call("answer", json!({"service": "toy", "n": ns, "word": "yes"}))
        .await;
    assert!(
        bad["isError"] == true && bad["content"][0]["text"].as_str().is_some_and(|t| t.contains(refused)),
        "{bad}"
    );
    drop(mcp);
    let card = b
        .until("the signed card", 20, || {
            mock.posts("/v1/requests").into_iter().find(|r| r["payload"]["n"] == ns)
        })
        .await;
    assert_eq!(card["title"], "Change the rules of toy?", "{card}");
    assert_eq!(
        card["input"][0]["options"],
        json!([
            {"id": "yes", "label": "Apply it", "stance": "neutral"},
            {"id": "no", "label": "Keep the rules", "stance": "rejection"},
        ]),
        "{card}"
    );
    assert!(mock.throws_for(ns).is_empty(), "a signed escalation goes as no throw");
    assert!(
        b.lines("note", |l| l["n"] == ns).is_empty() && b.lines("summary", |l| l["n"] == ns).is_empty(),
        "a signed escalation has no model summary and no agent"
    );
    assert!(!svc_dir.join(format!("escalations/{ns}.log")).exists());
    let kept = svc_dir.join(format!("changes/{ns}.json"));
    assert!(kept.is_file());
    assert!(!std::fs::read_to_string(svc_dir.join("Weavefile")).unwrap().contains(WIPE_RULE));
    mock.respond(card["id"].as_str().unwrap(), "yes");
    let applied = b.note(&format!("weave changed by agent: {RULE_WHY}"), 70).await;
    assert_eq!(
        applied["text"],
        format!(
            "weave changed by agent: {RULE_WHY}; no fact, goal, action, skill or KNOWN line changed; signed yes on #{ns} by steery:{SIGNER_NAME}"
        )
    );
    let answer = b.answered(ns, 5).await;
    assert_eq!(answer["text"], "yes", "{answer}");
    assert_eq!(answer["by"], format!("steery:{SIGNER_NAME}"), "{answer}");
    let running = std::fs::read_to_string(svc_dir.join("Weavefile")).unwrap();
    assert_eq!(running, rule_text);
    assert!(!kept.exists());
    b.note(&format!("escalated #{n4} to agent: exit 0"), 10).await;
    b.ok(&["channel", "rm", "agent"]).await;
    b.holds(&["installed", "running", "rotated"], 30).await;

    // After the skill went whole once, a throw to the session points at it.
    let n5 = b.rotate_ask().await;
    let t5 = b
        .until("a later throw to the session", 20, || mock.throws_for(n5).into_iter().next())
        .await;
    let text = t5["body"].as_str().unwrap_or_default();
    assert!(
        t5["session"] == s2
            && text.contains(&format!("Read the escalation skill first: `ssh {REACH} sudo -n subweave skill`."))
            && !text.contains("# Subweave escalation"),
        "{t5}"
    );
    // The session's words render the card: the rule that holds rotate, with its BECAUSE.
    assert!(
        text.contains("`ASK BEFORE rotate` holds it: a rotate starts the server log over."),
        "{t5}"
    );
    b.ok(&["answer", &n5.to_string(), "yes"]).await;
    for (path, body) in &mock.steery().posts {
        assert!(!body.to_string().contains(SECRET), "a Steery {path} holds the secret");
    }

    // 9d. An escalation withdrawn while its summary is written goes out on no channel:
    // no Steery throw or request, no agent, no webhook.
    eprintln!("9d withdrawn before it goes");
    b.ok(&["channel", "add", "agent", &command]).await;
    let (throws, requests) = (mock.posts("/v1/throws").len(), mock.posts("/v1/requests").len());
    std::fs::write(dir.join("scene"), "unsure").unwrap();
    mock.hold_summary.store(true, Ordering::SeqCst);
    kill_pidfile(&dir.join("toy.pid"));
    let (nw, _) = b
        .until("an ask to withdraw", 30, || b.open_ask("running"))
        .await;
    std::fs::write(dir.join("scene"), "calm").unwrap();
    let started_by_hand = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cd '{}' && nohup python3 -m http.server {port} --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > '{}'",
            www.display(),
            dir.join("toy.pid").display()
        ))
        .status()
        .unwrap();
    assert!(started_by_hand.success());
    b.until("the withdraw", 30, || {
        b.lines("withdraw", |l| l["n"] == nw).into_iter().next()
    })
    .await;
    mock.hold_summary.store(false, Ordering::SeqCst);
    b.until("the withdrawn ask's summary", 20, || {
        b.lines("summary", |l| l["n"] == nw).into_iter().next()
    })
    .await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(mock.throws_for(nw).is_empty(), "the withdrawn #{nw} went as a throw");
    assert_eq!(mock.posts("/v1/throws").len(), throws, "a throw after the withdraw");
    assert_eq!(mock.posts("/v1/requests").len(), requests, "a Steery request after the withdraw");
    assert!(
        !mock.hooks().iter().any(|h| h["kind"] == "ask" && h["ask"]["n"] == nw),
        "the withdrawn #{nw} went to the webhook"
    );
    assert!(!svc_dir.join(format!("escalations/{nw}.log")).exists(), "an agent ran for the withdrawn #{nw}");
    assert!(b.lines("sent", |l| l["n"] == nw).is_empty(), "the withdrawn #{nw} was sent");
    assert!(b.lines("answer", |l| l["n"] == nw).is_empty(), "the withdrawn #{nw} was answered");
    b.ok(&["channel", "rm", "agent"]).await;
    b.holds(&["installed", "running", "rotated"], 30).await;

    // 9e. stats: this week's repairs by rung, escalations by who settled them, and the
    // two weave changes.
    eprintln!("9e stats");
    let out = b.ok(&["stats", "toy"]).await;
    let row = out.lines().last().unwrap_or_default();
    let cols: Vec<&str> = row.split_whitespace().collect();
    let num = |i: usize| cols.get(i).and_then(|c| c.parse::<usize>().ok());
    assert!(num(2) >= Some(1) && num(3) >= Some(1), "decider and coder repairs: {out}");
    let settled = b.lines("answer", |l| l["n"].is_u64()).len() + b.lines("withdraw", |_| true).len();
    assert_eq!(num(4), Some(settled), "{out}");
    assert_eq!(num(5), Some(2), "{out}");
    // Phase 6n's drills: one passed, one did not notice.
    assert_eq!(cols.get(6), Some(&"1/2"), "drills passed: {out}");
    for want in ["agent 2", "mcp 1", "steery 1", "withdrawn "] {
        assert!(row.contains(want), "stats lacks {want:?}:\n{out}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn daemon_and_cli_keep_a_toy_service() {
    let started = Instant::now();
    let tmp = tempfile::Builder::new()
        .prefix("sw-e2e-")
        .tempdir()
        .expect("a temp dir");
    // macOS hands out /var/…, a link to /private/var/…: one spelling everywhere.
    let dir = std::fs::canonicalize(tmp.path()).expect("the temp dir");
    let www = dir.join("www");
    let mut b = Box_ {
        state: dir.join("state"),
        config: dir.join("config"),
        dir: dir.clone(),
        daemon: None,
        dash: None,
        steery_url: String::new(),
    };
    let weave_dir = dir.join("examples/toy");
    std::fs::create_dir_all(&weave_dir).unwrap();
    std::fs::write(weave_dir.join("Weavefile"), weavefile(&dir, &www)).unwrap();
    std::fs::write(dir.join("scene"), "calm").unwrap();
    std::fs::write(dir.join("app.log"), "").unwrap();
    std::fs::write(dir.join("events.log"), "").unwrap();
    std::fs::create_dir_all(dir.join("spool")).unwrap();
    std::fs::write(dir.join("setting"), "on\n").unwrap();
    std::fs::write(dir.join("web-version"), "1.2.0\n").unwrap();
    let weave = weave_dir.display().to_string();

    // The vendors: OpenRouter and a webhook, on one local server.
    let mock = Mock {
        seen: Arc::new(Mutex::new(Seen::default())),
        steery: Arc::new(Mutex::new(Steery::default())),
        hold_summary: Arc::new(AtomicBool::new(false)),
        firewall_mark: dir.join("firewall-opened"),
    };
    let app = Router::new()
        .route("/systemone", post(systemone))
        .route("/chat/completions", post(chat))
        .route("/hook", post(hook))
        .route("/v1/traces", post(traces))
        .route("/v1/me", get(steery_me))
        .route("/v1/signers", get(steery_signers))
        .route("/v1/throws", post(throw_create))
        .route("/v1/throws/{id}", get(throw_get))
        .route("/v1/requests", post(request_create))
        .route("/v1/requests/{id}", get(request_get))
        .route("/repos/{org}/{repo}/releases", get(releases))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });
    b.steery_url = mock_url.clone();

    let out = b.ok(&["provider", "add", "mock", &mock_url]).await;
    assert!(out.contains("added mock"), "{out}");
    let out = b.ok(&["provider", "ls"]).await;
    assert!(out.contains("key from $MOCK_API_KEY"), "{out}");
    let hook_url = format!("{mock_url}/hook");
    let out = b
        .ok(&["channel", "add", "webhook", &hook_url, "--level", "info"])
        .await;
    assert!(out.contains("added webhook"), "{out}");

    let models = [
        "--decide",
        "typesafe/jev-1.13@mock",
        "--code",
        "deepseek/x@mock",
    ];

    // 6e. A request that breaks NEVER IF: exit 1, nothing handed over.
    let phase = |what: &str| eprintln!("{:>5.1}s {what}", started.elapsed().as_secs_f64());
    phase("6e request check");
    let mut args = vec![
        "run",
        &weave,
        "serve the hello page on port 80",
        "--yes",
        "-d",
    ];
    args.extend(["--name", "toy-bad"]);
    args.extend(models);
    let o = b.sw(&args).await;
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("NEVER IF") && out.contains("80 < 1024"),
        "{out}"
    );
    assert!(out.contains("nothing runs"), "{out}");
    let inbox: Vec<_> = std::fs::read_dir(b.state.join("inbox"))
        .map(|d| d.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();
    assert!(inbox.is_empty(), "the inbox holds {inbox:?}");
    assert!(!b.state.join("services/toy-bad").exists());
    assert_eq!(mock.coder_requests(), 0);

    // 6j. plan refuses a goal that compares a percentage with a size, before anything runs.
    phase("6j units");
    let units = dir.join("examples/units");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::write(
        units.join("Weavefile"),
        "ABOUT \"a box whose disk has room\"\n\
         FACT disk NUMBER % \"percent used of the root filesystem\"\n\
         GOAL room \"the disk has room\" disk < 1GB\n",
    )
    .unwrap();
    let o = b.sw(&["plan", &units.display().to_string()]).await;
    assert!(
        !o.status.success()
            && both(&o).contains(
                "Weavefile:3: `disk < 1GB`: `disk` is a percentage and `1GB` is a size, which do not compare"
            ),
        "{}",
        both(&o)
    );

    // 6k. plan refuses an effect outside the closed set.
    let effects = dir.join("examples/effects");
    std::fs::create_dir_all(&effects).unwrap();
    std::fs::write(
        effects.join("Weavefile"),
        "ABOUT \"a box that wipes its disk\"\n\
         FACT full BOOL test -f /nonexistent\n\
         GOAL room \"the disk has room\" NOT full\n\
         ACTION wipe FOR room CHANGES disk\n\
         \x20 WHEN \"the disk is full\"\n\
         \x20 RUN true\n",
    )
    .unwrap();
    let o = b.sw(&["plan", &effects.display().to_string()]).await;
    assert!(
        !o.status.success()
            && both(&o).contains(
                "Weavefile:4: ACTION wipe: `disk` is no effect: restart, config, version, data or network, or nothing alone"
            ),
        "{}",
        both(&o)
    );

    // 6m. plan with no daemon here says which references to other services it could not
    // check, and passes.
    let relay_dir = dir.join("examples/relay");
    std::fs::create_dir_all(&relay_dir).unwrap();
    std::fs::write(relay_dir.join("Weavefile"), relay_weavefile(&dir)).unwrap();
    let relay = relay_dir.display().to_string();
    let o = b.sw(&["plan", &relay]).await;
    for want in [
        "Weavefile:4: AFTER toy:tidy: not checked: no daemon runs here",
        "Weavefile:4: toy:spooled: not checked: no daemon runs here",
    ] {
        assert!(o.status.success() && both(&o).contains(want), "{}", both(&o));
    }

    // The daemon, in the background.
    b.start_daemon(&mock_url);

    // 1. run: the plan, the inbox, the daemon applies it.
    phase("1 run");
    let port = free_port();
    let request = format!("keep the hello page up on port {port}");
    let mut args = vec!["run", &weave, &request, "--yes", "-d"];
    args.extend(models);
    let out = b.ok(&args).await;
    let port_s = port.to_string();
    assert!(
        out.lines()
            .any(|l| l.starts_with("info") && l.contains("port") && l.contains(&port_s)),
        "plan output: {out}"
    );
    assert!(out.contains("ready to run"), "{out}");
    assert!(out.contains("handed toy to the daemon"), "{out}");
    b.until("the daemon to take the run", 15, || {
        b.lines("note", |l| {
            l["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("run ") && t.contains(&request))
        })
        .first()
        .cloned()
    })
    .await;
    let spec: Value =
        serde_json::from_slice(&std::fs::read(b.service().join("service.json")).unwrap()).unwrap();
    assert_eq!(spec["args"]["port"], port_s);
    assert_eq!(spec["args"]["root"], www.display().to_string());
    assert_eq!(spec["decide"], "typesafe/jev-1.13@mock");
    assert_eq!(spec["code"], json!(["deepseek/x@mock"]));
    let secrets = std::fs::read_to_string(b.service().join("secrets.json")).unwrap();
    assert!(secrets.contains(SECRET), "TOY_TOKEN reaches secrets.json");

    // 2. Install through the coder; then the first restart starts the server.
    phase("2 install");
    b.holds(&["installed", "running"], 40).await;
    assert!(www.join("index.html").is_file());
    let page = std::fs::read_to_string(www.join("index.html")).unwrap();
    assert!(page.contains("hello"), "{page}");
    assert_eq!(b.passes("install"), 1);
    let ran: Vec<String> = b
        .lines("command", |l| l["action"] == "install" && l["ok"] == true)
        .iter()
        .filter_map(|l| l["text"].as_str().map(String::from))
        .collect();
    assert_eq!(ran, [MKDIR, WRITE]);
    // A command line carries its output, redacted.
    let mkdir = b.lines("command", |l| l["action"] == "install" && l["text"] == MKDIR);
    assert_eq!(
        mkdir[0]["output"],
        format!("web root {}, token [TOY_TOKEN]", www.display())
    );
    // The coder's commands keep their whole output, redacted, in files only the owner reads.
    let kept: Vec<PathBuf> = std::fs::read_dir(b.service().join("output"))
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    let mkdir_file = kept
        .iter()
        .find(|p| {
            std::fs::read_to_string(p).is_ok_and(|t| {
                t == format!("web root {}, token [TOY_TOKEN]", www.display())
            })
        })
        .unwrap_or_else(|| b.fail(&format!("no output file for mkdir in {kept:?}")));
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(mkdir_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(
        kept.iter().all(|p| p
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".txt") && n.contains('-'))),
        "{kept:?}"
    );
    let learned = b.learned();
    assert_eq!(
        learned["steps"]["install/installed"]["commands"],
        json!([MKDIR, WRITE]),
        "{learned}"
    );
    assert_eq!(
        b.lines("learn", |l| l["text"]
            == "steps for install/installed: 2 commands")
            .len(),
        1
    );
    let chats_after_install = mock.coder_requests();
    // Two fact learner turns, four coder turns: firewall, mkdir, page, done.
    assert_eq!(chats_after_install, 6);
    // The note that ends the coder's conversation counts its reasoning tokens.
    let usage: Vec<Value> = b
        .lines("note", |l| l["action"] == "install" && l["usage"].is_object())
        .into_iter()
        .map(|l| l["usage"].clone())
        .collect();
    assert_eq!(usage.len(), 1, "{usage:?}");
    assert_eq!(usage[0]["calls"], 4);
    assert_eq!(usage[0]["output"], 160);
    assert_eq!(usage[0]["reasoning"], 120);
    assert!(
        mock.seen()
            .requests
            .iter()
            .all(|(_, auth, _)| auth == &format!("Bearer {API_KEY}")),
        "every request carries MOCK_API_KEY"
    );
    let models_seen: Vec<String> = mock
        .seen()
        .requests
        .iter()
        .filter_map(|(_, _, body)| {
            serde_json::from_str::<Value>(body).ok()?["model"]
                .as_str()
                .map(String::from)
        })
        .collect();
    assert!(models_seen.iter().any(|m| m == "typesafe/jev-1.13"));
    assert!(models_seen.iter().any(|m| m == "deepseek/x"));
    assert!(
        models_seen
            .iter()
            .all(|m| m == "typesafe/jev-1.13" || m == "deepseek/x"),
        "{models_seen:?}"
    );

    // 6a. The quoted NEVER refused the coder's firewall command: journaled, never run.
    let gates = b.lines("gate", |l| {
        l["action"] == "install"
            && l["text"].as_str().is_some_and(|t| {
                t.starts_with(&format!("NEVER \"{FIREWALL_RULE}\""))
                    && t.contains("firewall-opened")
            })
    });
    assert_eq!(gates.len(), 1, "one gate line for the firewall command");
    assert!(gates[0]["p"]["yes"].as_f64().unwrap() >= 0.5);
    assert!(
        !mock.firewall_mark.exists(),
        "the refused command never ran"
    );
    assert!(
        b.lines("command", |l| l["text"]
            .as_str()
            .is_some_and(|t| t.contains("firewall")))
            .is_empty(),
        "no command line for the refused command"
    );
    assert!(
        mock.seen().requests.iter().any(|(p, _, body)| {
            p == "/chat/completions" && body.contains("Refused by the rule NEVER")
        }),
        "the coder read the refusal"
    );

    // 4 (first time). The server was down: the decider picked restart, its RUN ran. The
    // loop's own check can see `running` hold before the restart's WITHIN check writes
    // its action line.
    let restarts = b
        .until("the first restart to pass", 15, || {
            Some(b.passes("restart")).filter(|n| *n >= 1)
        })
        .await;
    assert!(
        b.lines("decide", |l| l["goal"] == "running"
            && l["text"] == "restart")
            .iter()
            .all(|l| l["p"]["restart"].as_f64().unwrap() >= 0.5)
    );

    // 3. The quoted fact `served` was learned once and now runs as code.
    let fact_cmd = learned["facts"]["served"]["command"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(fact_cmd, FACT_CMD, "{learned}");
    let served_checks = b.lines("check", |l| {
        l["goal"] == "running" && l["ok"] == true && l["facts"]["served"] == "1"
    });
    assert!(
        !served_checks.is_empty(),
        "running holds with served=1 read in code"
    );
    // The first command printed an HTTP status for a 0/1 fact: rejected by `fits`, and
    // the coder told so; the second try is the one kept.
    {
        let seen = mock.seen();
        let fits: Vec<Value> = seen
            .requests
            .iter()
            .filter(|(p, _, _)| p == "/systemone")
            .filter_map(|(_, _, body)| serde_json::from_str::<Value>(body).ok())
            .filter(|v| v["questions"]["fits"].is_object())
            .collect();
        let printed: Vec<&str> = fits
            .iter()
            .filter_map(|v| v["state"]["printed"].as_str())
            .collect();
        assert_eq!(printed, ["000", "0"], "fits saw the status, then the flag");
        assert!(fits.iter().all(|v| {
            v["state"]["fact"]
                .as_str()
                .is_some_and(|f| f.contains("answers HTTP 200"))
        }));
        let learners: Vec<&String> = seen
            .requests
            .iter()
            .filter(|(p, _, b)| p == "/chat/completions" && offers_tool(b, "fact"))
            .map(|(_, _, b)| b)
            .collect();
        assert_eq!(learners.len(), 2);
        assert!(
            learners[1].contains("It printed `000`, which is not a value of the fact"),
            "the coder heard why its first command was refused"
        );
    }
    assert!(!fact_cmd.contains("http_code"));

    // 6b. ASK BEFORE rotate: an ask in the inbox and on the webhook; yes runs it.
    phase("6b ask before rotate");
    let (n, words) = b
        .until("the rotate ask", 20, || b.open_ask("rotated"))
        .await;
    assert_eq!(words, ["yes", "no", "always", "never"]);
    let gate = b.lines("gate", |l| l["n"] == n);
    assert_eq!(gate.len(), 1);
    assert_eq!(gate[0]["text"], "ASK BEFORE rotate");
    assert_eq!(gate[0]["action"], "rotate");
    let out = b.ok(&["inbox"]).await;
    let row = out
        .lines()
        .find(|l| l.starts_with(&format!("#{n} ")))
        .unwrap_or_else(|| b.fail(&format!("no #{n} in inbox:\n{out}")));
    assert!(
        row.contains("toy") && row.contains("rotated") && row.contains("Start a fresh server log?"),
        "{row}"
    );
    assert!(
        row.contains("[yes: Run it | no: Leave it for 1h | always: Always run it | never: Never run it]"),
        "{row}"
    );
    let hook_ask = b
        .until("the webhook to get the rotate ask", 10, || {
            mock.hooks()
                .into_iter()
                .find(|h| h["kind"] == "ask" && h["ask"]["n"] == n)
        })
        .await;
    assert_eq!(hook_ask["service"], "toy");
    assert_eq!(hook_ask["goal"], "rotated");
    let card = &hook_ask["card"];
    assert_eq!(buttons(card), ["yes", "no", "always", "never"], "{hook_ask}");
    assert_eq!(label(card, "yes"), "Run it", "{hook_ask}");
    // The card names the action, what it changes and the rule that holds it, with its
    // BECAUSE; the goal reads no fact, so the card shows none.
    assert_eq!(card["goal"]["name"], "rotated", "{hook_ask}");
    assert_eq!(card["action"]["name"], "rotate", "{hook_ask}");
    assert_eq!(card["action"]["changes"], json!(["config"]), "{hook_ask}");
    assert_eq!(
        card["action"]["held"],
        json!({"rule": "ASK BEFORE rotate", "because": "a rotate starts the server log over"}),
        "{hook_ask}"
    );
    assert!(card.get("reads").is_none() && card.get("window").is_none(), "{hook_ask}");
    assert!(!hook_ask.to_string().contains("scene"), "an unrelated fact on the card: {hook_ask}");
    // The summary the model wrote leads the card, and its call is journaled with its cost.
    assert_eq!(card["summary"], json!(CARD_SUMMARY), "{hook_ask}");
    let summary = b.lines("summary", |l| l["n"] == n);
    assert_eq!(summary.len(), 1, "one summary per card");
    assert_eq!(summary[0]["lines"], json!(CARD_SUMMARY), "{}", summary[0]);
    assert!(summary[0]["usage"]["calls"] == 1 && summary[0]["cost_usd"].as_f64() > Some(0.0), "{}", summary[0]);
    assert!(!dir.join("rotated").exists(), "rotate waits for the answer");
    let out = b.ok(&["answer", &n.to_string(), "yes"]).await;
    assert!(
        out.contains(&format!("answered #{n} for toy: yes")),
        "{out}"
    );
    b.holds(&["rotated"], 20).await;
    assert!(dir.join("rotated").is_file());
    assert_eq!(b.passes("rotate"), 1);
    let answers = b.lines("answer", |l| l["n"] == n && l["text"] == "yes");
    assert_eq!(answers.len(), 1, "the answer is journaled");
    assert!(
        answers[0]["by"].as_str().is_some_and(|by| by.starts_with("cli:")),
        "{}",
        answers[0]
    );
    b.until("the webhook to hear the answer", 10, || {
        mock.hooks().into_iter().find(|h| {
            h["card"]["title"]
                .as_str()
                .is_some_and(|t| t.starts_with(&format!("#{n} answered yes")))
        })
    })
    .await;

    // 4. Kill the server: restart at high confidence, its RUN, running holds again.
    phase("4 kill the server");
    let pid = b.pid();
    let decides = b.lines("decide", |l| l["goal"] == "running").len();
    kill_pidfile(&dir.join("toy.pid"));
    b.until("a new restart to pass", 30, || {
        (b.passes("restart") > restarts && b.pid() != pid).then_some(())
    })
    .await;
    b.holds(&["installed", "running", "rotated"], 15).await;
    let d = b.lines("decide", |l| l["goal"] == "running");
    assert!(d.len() > decides, "the decider was asked");
    let last = d.last().unwrap();
    assert_eq!(last["text"], "restart");
    assert!(last["p"]["restart"].as_f64().unwrap() >= 0.9);
    assert!(last["p"]["steered"].as_f64().unwrap() < 0.5);
    assert!(
        !b.lines("command", |l| l["action"] == "restart"
            && l["ok"] == true
            && l["text"]
                .as_str()
                .is_some_and(|t| t.contains("http.server")))
            .is_empty(),
        "the RUN step ran"
    );
    assert_eq!(
        mock.coder_requests(),
        chats_after_install,
        "no coder for a RUN"
    );

    // 5. Break it again: the learned install steps fix it, with zero coder requests.
    phase("5 break it again");
    let installs = b.passes("install");
    std::fs::remove_file(www.join("index.html")).unwrap();
    b.until("a new install to pass", 30, || {
        (b.passes("install") > installs).then_some(())
    })
    .await;
    b.holds(&["installed", "running", "rotated"], 15).await;
    assert_eq!(
        mock.coder_requests(),
        chats_after_install,
        "learned steps ran without the coder"
    );
    let sessions = b.lines("note", |l| {
        l["action"] == "install" && l["text"].as_str().is_some_and(|t| t.starts_with("coder "))
    });
    assert_eq!(
        sessions.len(),
        1,
        "only the first install had a coder session"
    );
    let ran: Vec<String> = b
        .lines("command", |l| l["action"] == "install" && l["ok"] == true)
        .iter()
        .filter_map(|l| l["text"].as_str().map(String::from))
        .collect();
    assert_eq!(ran, [MKDIR, WRITE, MKDIR, WRITE]);
    assert!(b.learned()["steps"]["install/installed"].is_object());

    // 6c. The decider is unsure: an ask with the candidate actions; answering one runs it.
    phase("6c unsure decider");
    std::fs::write(dir.join("scene"), "unsure").unwrap();
    let restarts = b.passes("restart");
    kill_pidfile(&dir.join("toy.pid"));
    let (n, words) = b
        .until("the which-action ask", 30, || b.open_ask("running"))
        .await;
    assert_eq!(words, ["restart", "fix", "no"]);
    let low = b.lines("decide", |l| l["goal"] == "running");
    let low = low.last().unwrap();
    assert_eq!(low["text"], "restart");
    assert!(low["p"]["restart"].as_f64().unwrap() < 0.5, "{low}");
    let out = b.ok(&["inbox"]).await;
    let row = out
        .lines()
        .find(|l| l.starts_with(&format!("#{n} ")))
        .unwrap_or_else(|| b.fail(&format!("no #{n} in inbox:\n{out}")));
    assert!(
        row.contains("Which fix for: the hello page is up?")
            && row.contains("[restart: Restart the server | fix | no: Leave it for 1h]"),
        "{row}"
    );
    b.until("the webhook to get the which-action ask", 10, || {
        mock.hooks()
            .into_iter()
            .find(|h| h["kind"] == "ask" && h["ask"]["n"] == n)
    })
    .await;
    assert_eq!(
        b.passes("restart"),
        restarts,
        "nothing runs before the answer"
    );
    // A daemon restart keeps the failing goal and its open escalation: the new daemon
    // replays the journal, raises nothing again and runs nothing before the answer.
    phase("6c daemon restart");
    let asks = b.lines("ask", |_| true).len();
    let checks = b.lines("check", |l| l["goal"] == "running").len();
    b.restart_daemon(&mock_url).await;
    b.until("the new daemon's first check", 20, || {
        (b.lines("check", |l| l["goal"] == "running").len() > checks).then_some(())
    })
    .await;
    let st = b.status();
    assert_eq!(st["states"]["running"]["state"], "fails", "{st}");
    assert_eq!(st["asks"], json!([n]), "{st}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        b.lines("ask", |_| true).len(),
        asks,
        "the restart raised an escalation again"
    );
    assert_eq!(b.goal("running"), "fails");
    assert_eq!(b.open_ask("running").map(|(m, _)| m), Some(n));
    assert_eq!(
        b.passes("restart"),
        restarts,
        "nothing runs before the answer, after a restart either"
    );
    std::fs::write(dir.join("scene"), "calm").unwrap();
    let o = b.sw(&["answer", &n.to_string(), "reboot"]).await;
    assert!(
        !o.status.success(),
        "a word the ask does not take is refused"
    );
    let out = b.ok(&["answer", &n.to_string(), "restart"]).await;
    assert!(
        out.contains(&format!("answered #{n} for toy: restart")),
        "{out}"
    );
    b.until("the answered restart to pass", 30, || {
        (b.passes("restart") > restarts).then_some(())
    })
    .await;
    b.holds(&["installed", "running", "rotated"], 15).await;
    let answer = b.lines("answer", |l| l["n"] == n);
    assert_eq!(answer.len(), 1);
    assert_eq!(answer[0]["action"], "restart");
    let out = b.ok(&["inbox"]).await;
    assert!(out.contains("no open escalations"), "{out}");

    // 3 (again). Still one learn line and one learner request for `served`.
    assert_eq!(
        b.lines("learn", |l| l["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("fact served")))
            .len(),
        1
    );
    assert_eq!(
        mock.fact_requests(),
        2,
        "the wrong form, then the right one"
    );
    assert_eq!(mock.coder_requests(), chats_after_install);

    // 6f. An ask whose goal holds again is withdrawn: the person brought the server back.
    // Its card summary is held until then: a withdrawn ask goes to no channel.
    phase("6f withdrawn ask");
    std::fs::write(dir.join("scene"), "unsure").unwrap();
    mock.hold_summary.store(true, Ordering::SeqCst);
    kill_pidfile(&dir.join("toy.pid"));
    let (n, _) = b
        .until("a second which-action ask", 30, || b.open_ask("running"))
        .await;
    std::fs::write(dir.join("scene"), "calm").unwrap();
    let started_by_hand = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cd '{}' && nohup python3 -m http.server {port} --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > '{}'",
            www.display(),
            dir.join("toy.pid").display()
        ))
        .status()
        .unwrap();
    assert!(started_by_hand.success());
    let withdrawn = b
        .until("the withdraw", 30, || {
            b.lines("withdraw", |l| l["n"] == n).into_iter().next()
        })
        .await;
    assert_eq!(withdrawn["text"], "running holds again", "{withdrawn}");
    mock.hold_summary.store(false, Ordering::SeqCst);
    b.until("the withdrawn ask's summary", 20, || {
        b.lines("summary", |l| l["n"] == n).into_iter().next()
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !mock
            .hooks()
            .iter()
            .any(|h| h["kind"] == "ask" && h["ask"]["n"] == n),
        "the withdrawn ask #{n} still went to the webhook"
    );
    let out = b.ok(&["inbox"]).await;
    assert!(out.contains("no open escalations"), "{out}");
    b.until("the webhook to hear the withdrawal", 10, || {
        mock.hooks()
            .into_iter()
            .find(|h| h["card"]["title"] == format!("#{n} withdrawn"))
    })
    .await;
    // An answer after the withdraw settles nothing: the journal keeps it as refused.
    let restarts = b.passes("restart");
    let o = b.sw(&["answer", &n.to_string(), "restart"]).await;
    assert!(
        !o.status.success() && both(&o).contains(&format!("#{n} was withdrawn at ")),
        "{}",
        both(&o)
    );
    let refused = b
        .until("the refused answer", 10, || {
            b.lines("refused", |l| l["refused"]["n"] == n).into_iter().next()
        })
        .await;
    assert_eq!(refused["refused"]["kind"], "answer", "{refused}");
    assert_eq!(refused["refused"]["text"], "restart", "{refused}");
    assert!(
        refused["text"]
            .as_str()
            .is_some_and(|t| t.starts_with(&format!("#{n} was withdrawn at ")) && t.ends_with(": running holds again")),
        "{refused}"
    );
    assert!(refused["by"].as_str().is_some_and(|by| by.starts_with("cli:")), "{refused}");
    assert!(b.lines("answer", |l| l["n"] == n).is_empty(), "#{n} took a late answer");
    assert_eq!(b.passes("restart"), restarts, "a refused answer runs nothing");
    b.holds(&["installed", "running", "rotated"], 15).await;
    // An ask a person caused stays until a person answers, though its goal holds.
    b.ok(&["do", "toy", "rotate"]).await;
    let (n, _) = b
        .until("the ask for a rotate run by hand", 20, || b.open_ask("rotated"))
        .await;
    let ask = b.lines("ask", |l| l["n"] == n);
    assert!(
        ask[0]["by"].as_str().is_some_and(|by| by.starts_with("cli:")),
        "{}",
        ask[0]
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        b.lines("withdraw", |l| l["n"] == n).is_empty(),
        "a person's ask is not withdrawn"
    );

    // 6g. An agent answers it over MCP: a word the ask does not take is refused, the
    // answer and its note are journaled by the agent.
    phase("6g mcp");
    let mut mcp = Mcp::start(&b);
    let init = mcp
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "e2e-agent", "version": "1"},
            }),
        )
        .await;
    assert_eq!(init["result"]["serverInfo"]["name"], "subweave", "{init}");
    assert_eq!(init["result"]["capabilities"], json!({"tools": {}}), "{init}");
    assert!(
        init["result"]["instructions"]
            .as_str()
            .is_some_and(|t| t.contains("Read the `skill` tool first")),
        "{init}"
    );
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    let list = mcp.request("tools/list", json!({})).await;
    let tools: Vec<&str> = list["result"]["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(
        tools,
        [
            "inbox", "wait_ask", "answer", "known", "forget_known", "known_release", "status", "log",
            "task", "do", "drill", "weave", "weave_change", "skill"
        ],
        "{list}"
    );
    let (skill, _) = mcp.call("skill", json!({})).await;
    assert!(
        skill["content"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("# Subweave escalation") && t.contains("Never answer a signed escalation.")),
        "{skill}"
    );
    let (_, shown) = mcp.call("weave", json!({"service": "toy"})).await;
    assert_eq!(shown["source"], weave, "{shown}");
    assert_eq!(shown["args"]["port"], port.to_string(), "{shown}");
    assert!(
        shown["weavefile"].as_str().is_some_and(|w| w.contains("ASK   BEFORE rotate")),
        "{shown}"
    );
    let (_, asks) = mcp.call("inbox", json!({})).await;
    let ask = asks
        .as_array()
        .and_then(|a| a.iter().find(|a| a["n"] == n))
        .unwrap_or_else(|| b.fail(&format!("no #{n} in the mcp inbox: {asks}")));
    assert_eq!(ask["service"], "toy", "{ask}");
    assert_eq!(buttons(&ask["card"]), ["yes", "no", "always", "never"], "{ask}");
    assert_eq!(ask["card"]["action"]["held"]["rule"], "ASK BEFORE rotate", "{ask}");
    assert!(
        ask["text"].as_str().is_some_and(|t| t.contains("`ASK BEFORE rotate` holds it: a rotate starts the server log over.")),
        "{ask}"
    );
    let (_, waited) = mcp
        .call("wait_ask", json!({"timeout_secs": 5, "seen": []}))
        .await;
    assert!(
        waited.as_array().is_some_and(|a| a.iter().any(|a| a["n"] == n)),
        "an unseen ask ends the wait at once: {waited}"
    );
    let (bad, _) = mcp
        .call("answer", json!({"service": "toy", "n": n, "word": "reboot"}))
        .await;
    assert_eq!(bad["isError"], true, "{bad}");
    assert!(
        bad["content"][0]["text"]
            .as_str()
            .is_some_and(|t| t.contains("yes, no, always, never")),
        "{bad}"
    );
    let note = "the rotate by hand is not needed today";
    let (ok, _) = mcp
        .call(
            "answer",
            json!({"service": "toy", "n": n, "word": "no", "note": note}),
        )
        .await;
    assert!(ok.get("isError").is_none(), "{ok}");
    let answer = b
        .until("the mcp answer", 10, || {
            b.lines("answer", |l| l["n"] == n).into_iter().next()
        })
        .await;
    assert_eq!(answer["text"], "no", "{answer}");
    assert_eq!(answer["by"], "mcp:e2e-agent", "{answer}");
    let noted = b.lines("note", |l| l["text"] == note);
    assert_eq!(noted.len(), 1, "the note is journaled once");
    assert_eq!(noted[0]["by"], "mcp:e2e-agent", "{}", noted[0]);
    assert_eq!(noted[0]["goal"], "rotated", "{}", noted[0]);
    assert!(b.open_ask("rotated").is_none(), "#{n} is still open");
    let (_, asks) = mcp.call("inbox", json!({})).await;
    assert_eq!(asks, json!([]), "the mcp inbox is empty");
    drop(mcp);

    // 6i. A LINES goal judged over its window: one routine line in app.log fails `calm`
    // twice in a row, and again after a daemon restart; it holds only once OVER has passed
    // without the line. No action is FOR it: one escalation, withdrawn when it holds.
    phase("6i lines window");
    const OVER_MS: i64 = 12_000;
    const ROUTINE: &str = "GET /health 200 in 0.4ms";
    // An event's `t` has whole seconds; a line's `t` has milliseconds.
    let at_ms = |l: &Value| -> i64 {
        l["t"]
            .as_str()
            .and_then(|t| t.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_millisecond())
            .unwrap_or_else(|| panic!("no time on {l}"))
    };
    let calm = |b: &Box_| b.lines("check", |l| l["goal"] == "calm");
    b.holds(&["calm"], 30).await;
    assert!(b.lines("ask", |l| l["goal"] == "calm").is_empty());
    let from = calm(&b).len();
    {
        use std::io::Write;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("app.log"))
            .unwrap();
        writeln!(log, "{ROUTINE}").unwrap();
    }
    let first = b
        .until("calm's first failed check", 10, || {
            calm(&b).into_iter().skip(from).find(|l| l["ok"] == false)
        })
        .await;
    // The line is under `lines`, with the time the follower read it; never under `facts`.
    let line_t = first["lines"]["errors"]
        .as_array()
        .and_then(|ls| ls.iter().find(|x| x["text"] == ROUTINE))
        .and_then(|x| x["t"].as_i64())
        .unwrap_or_else(|| b.fail(&format!("the first failed check lacks the line: {first}")));
    assert!(first["facts"].get("errors").is_none(), "{first}");
    b.until("calm to fail", 10, || (b.goal("calm") == "fails").then_some(()))
        .await;
    let since_line: Vec<Value> = calm(&b).into_iter().skip(from).collect();
    assert!(
        since_line.iter().all(|l| l["ok"] != true),
        "calm held between its failed checks: {since_line:?}"
    );
    let failed: Vec<i64> = since_line
        .iter()
        .filter(|l| l["ok"] == false)
        .map(|l| at_ms(l))
        .collect();
    assert!(
        failed.len() >= 2 && failed[1] < line_t + OVER_MS,
        "the re-check reads the line's window again: {since_line:?}"
    );
    let (n, _) = b
        .until("calm's escalation", 10, || b.open_ask("calm"))
        .await;
    // Its card shows the window and its lines by kind, not every fact.
    let raised = b.lines("ask", |l| l["n"] == n);
    let card = &raised[0]["card"];
    assert_eq!(card["window"]["over"], "12s", "{card}");
    let kinds = &card["window"]["lines"]["kinds"];
    assert!(
        kinds.as_array().is_some_and(|ks| ks.iter().any(|k| k["sample"] == ROUTINE && k.get("known").is_none())),
        "{card}"
    );
    assert!(card.get("reads").is_none() && !card.to_string().contains("scene"), "{card}");
    // A daemon restart while the line is still in the window: the rebuilt ring holds it.
    let restarted = jiff::Timestamp::now().as_millisecond();
    assert!(
        restarted + 3_000 < line_t + OVER_MS,
        "too late to restart inside the window: line at {line_t}, now {restarted}"
    );
    let before_restart = calm(&b).len();
    b.restart_daemon(&mock_url).await;
    let again = b
        .until("calm's first check after the restart", 15, || {
            calm(&b).into_iter().nth(before_restart)
        })
        .await;
    assert!(
        again["ok"] == false && at_ms(&again) < line_t + OVER_MS,
        "the line fails calm after the restart: {again}"
    );
    assert!(again.get("lines").is_none(), "a journaled line is journaled again: {again}");
    assert_eq!(b.goal("calm"), "fails");
    b.holds(&["calm"], 40).await;
    let held = calm(&b)
        .into_iter()
        .skip(from)
        .find(|l| l["ok"] == true)
        .unwrap_or_else(|| b.fail("no check of calm that holds"));
    // OVER after the line, in the event's whole seconds.
    assert!(
        at_ms(&held) >= (line_t + OVER_MS) / 1000 * 1000,
        "calm held at {} with the line at {line_t}ms, inside OVER",
        held["t"]
    );
    let asks = b.lines("ask", |l| l["goal"] == "calm");
    assert!(
        asks.len() == 1 && asks[0]["n"] == n,
        "the restart raised calm's escalation again: {asks:?}"
    );
    let withdrawn = b
        .until("calm's escalation withdrawn", 10, || {
            b.lines("withdraw", |l| l["n"] == n).into_iter().next()
        })
        .await;
    assert_eq!(withdrawn["text"], "calm holds again", "{withdrawn}");
    assert!(
        b.lines("action", |l| l["goal"] == "calm").is_empty(),
        "an action ran for calm"
    );

    // 6j. Memory: `quiet` judges each kind of line in its window once. A kind the weave
    // knows normal never reaches the decider; a routine kind does twice, then never again.
    // A kind a person says is trouble fails in code; the action that fixed it twice runs
    // for it without the decider. An escalation answered with `--known normal` settles
    // and teaches.
    phase("6j memory");
    let event = |line: &str| {
        use std::io::Write;
        let mut log = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join("events.log"))
            .unwrap();
        writeln!(log, "{line}").unwrap();
    };
    let asked = |what: &str| {
        mock.seen()
            .requests
            .iter()
            .filter(|(p, _, body)| p == "/systemone" && body.contains(what))
            .count()
    };
    let verdicts = |b: &Box_, what: &str| {
        b.lines("verdict", |l| {
            l["goal"] == "quiet" && l["text"].as_str().is_some_and(|t| t.contains(what))
        })
    };
    let failed_quiet = |b: &Box_| b.lines("check", |l| l["goal"] == "quiet" && l["ok"] == false).len();
    b.holds(&["quiet"], 20).await;
    let failed_before = failed_quiet(&b);
    // The weave's KNOWN: another heartbeat is dropped in code.
    event("heartbeat 2 ok from 10.0.0.3");
    // A routine kind: the decider says normal twice while it is in the window.
    event(ROUTINE_CONN);
    let two = b
        .until("two verdicts on the routine kind", 15, || {
            let v = verdicts(&b, "closed after");
            (v.len() >= 2).then_some(v)
        })
        .await;
    assert!(
        two.iter().all(|v| v["p"].as_f64().is_some_and(|p| p <= 0.2)),
        "{two:?}"
    );
    let conn_asks = asked("closed after");
    assert_eq!(conn_asks, 2, "the routine kind went to the decider {conn_asks} times");
    event("conn 42 from 10.0.0.9 closed after 37ms");
    // Three EVERYs with the new line in the window: none asks the decider.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(asked("closed after"), 2, "a known normal kind reached the decider");
    assert_eq!(verdicts(&b, "closed after").len(), 2);
    assert_eq!(asked("heartbeat"), 0, "a KNOWN kind reached the decider");
    assert_eq!(failed_quiet(&b), failed_before, "a normal line failed quiet");
    assert_eq!(b.goal("quiet"), "holds");

    // A person says a disk failure is trouble: each one fails quiet in code.
    let out = b
        .ok(&["known", "toy", "quiet", "trouble", DISK_FAILED, "--why", "a failed disk loses data"])
        .await;
    assert!(out.contains("known trouble"), "{out}");
    let told = b
        .until("the known line", 10, || {
            let t = b.lines("known", |l| l["text"] == DISK_FAILED);
            (!t.is_empty()).then_some(t)
        })
        .await;
    assert!(
        told.len() == 1
            && told[0]["class"] == "trouble"
            && told[0]["why"] == "a failed disk loses data"
            && told[0]["by"].as_str().is_some_and(|by| by.starts_with("cli:")),
        "{told:?}"
    );
    for (i, disk) in ["disk 4 failed: I/O error at sector 512", "disk 5 failed: I/O error at sector 7", "disk 6 failed: I/O error at sector 90"]
        .iter()
        .enumerate()
    {
        let decides = b.lines("decide", |l| l["goal"] == "quiet").len();
        let checks = b.lines("check", |l| l["goal"] == "quiet").len();
        let passes = b.passes("hush");
        event(disk);
        let failed = b
            .until("quiet to fail on the disk line", 10, || {
                b.lines("check", |l| l["goal"] == "quiet")
                    .into_iter()
                    .skip(checks)
                    .find(|l| l["ok"] == false)
            })
            .await;
        assert!(failed["p"].is_null(), "a known trouble kind asked the decider: {failed}");
        b.until("hush to pass", 30, || (b.passes("hush") > passes).then_some(()))
            .await;
        let started = b
            .lines("action", |l| l["action"] == "hush" && l.get("ok").is_none())
            .pop()
            .unwrap_or_else(|| b.fail("no start of hush"));
        assert!(
            started["on"].as_array().is_some_and(|on| on.iter().any(|x| x == *disk)),
            "hush did not run for the disk line: {started}"
        );
        b.holds(&["quiet"], 20).await;
        // A hold ends a fix's wait: none may come after it, or the next disk line's
        // failure waits it out with no repair while its window closes.
        let j = b.journal();
        let from = j
            .iter()
            .rposition(|l| l["kind"] == "action" && l["action"] == "hush" && l["ok"].is_null())
            .unwrap_or_else(|| b.fail("no start of hush"));
        let held = j[from..]
            .iter()
            .position(|l| l["kind"] == "check" && l["goal"] == "quiet" && l["ok"] == true);
        if let Some(h) = held {
            let late: Vec<&Value> = j[from + h..]
                .iter()
                .filter(|l| l["kind"] == "wait" && l["goal"] == "quiet")
                .collect();
            assert!(late.is_empty(), "a wait after quiet held parks its next failure: {late:?}");
        }
        let decided = b.lines("decide", |l| l["goal"] == "quiet").len() - decides;
        if i < 2 {
            assert_eq!(decided, 1, "pass {i} of hush was not the decider's");
        } else {
            assert_eq!(decided, 0, "memory asked the decider on the third disk line");
            let note = b.note("memory: hush passed 2 times", 5).await;
            assert_eq!(note["goal"], "quiet", "{note}");
        }
    }
    assert!(verdicts(&b, "failed: I/O error").is_empty(), "a told kind got model verdicts");
    assert_eq!(asked("failed: I/O error"), 2, "the decider saw the disk lines beyond two picks");

    // The model calls a cache line trouble; the person answers that it is normal.
    event(CACHE_WARM);
    let (n, words) = b
        .until("quiet's escalation on the cache line", 30, || b.open_ask("quiet"))
        .await;
    assert!(words.contains(&"no".to_string()), "{words:?}");
    let ask = b
        .lines("ask", |l| l["n"] == n)
        .pop()
        .unwrap_or_else(|| b.fail("no ask line"));
    assert_eq!(ask["lines"], json!([CACHE_WARM]), "{ask}");
    // The card shows only quiet's read set: its window's lines by kind, each with what the
    // box knows of it and who knows it. No other fact: not `scene`, not `spooled`.
    let card = &ask["card"];
    assert_eq!(card["goal"]["name"], "quiet", "{ask}");
    assert_eq!(card["window"]["over"], "8s", "{ask}");
    let kinds = card["window"]["lines"]["kinds"].as_array().cloned().unwrap_or_default();
    assert!(
        kinds.iter().any(|k| k["sample"] == CACHE_WARM
            && k["count"] == 1
            && k["known"] == json!({"class": "trouble", "by": "the model"})),
        "the card lacks the kind with its verdict: {ask}"
    );
    for k in &kinds {
        let s = k["sample"].as_str().unwrap_or_default();
        assert!(
            s.starts_with("disk ") || s.starts_with("conn ") || s.starts_with("cache ") || s.starts_with("heartbeat "),
            "a line not from `events` on the card: {ask}"
        );
    }
    assert!(
        card.get("reads").is_none() && card.get("parts").is_none() && card["window"].get("values").is_none(),
        "{ask}"
    );
    for unrelated in ["scene", "spooled", "served", "setting", "web."] {
        assert!(!card.to_string().contains(unrelated), "{unrelated} on quiet's card: {ask}");
    }
    // The webhook renders the same card.
    let hooked = b
        .until("the webhook to get quiet's ask", 10, || {
            mock.hooks().into_iter().find(|h| h["kind"] == "ask" && h["ask"]["n"] == n)
        })
        .await;
    assert_eq!(hooked["card"]["window"], card["window"], "{hooked}");
    let out = b
        .ok(&["answer", &n.to_string(), "no", "--known", "normal", "--note", "a warm cache is routine"])
        .await;
    assert!(out.contains("its lines are known normal"), "{out}");
    let taught = b
        .until("the answer's known line", 10, || {
            b.lines("known", |l| l["text"] == CACHE_WARM).into_iter().next()
        })
        .await;
    assert_eq!(taught["class"], "normal", "{taught}");
    assert_eq!(b.answered(n, 5).await["text"], "no");
    let cache_verdicts = verdicts(&b, "cache warmed").len();
    event("cache warmed in 9 passes");
    b.holds(&["quiet"], 15).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(b.goal("quiet"), "holds", "a kind the person said is normal fails quiet");
    assert_eq!(verdicts(&b, "cache warmed").len(), cache_verdicts);

    // What the box knows, drafted as KNOWN lines; a forgotten kind is gone from them.
    let out = b.ok(&["learn", "toy"]).await;
    for want in [
        format!("KNOWN quiet TROUBLE \"disk"),
        format!("KNOWN quiet NORMAL \"conn"),
        format!("KNOWN quiet NORMAL \"cache warmed"),
    ] {
        assert!(out.contains(&want), "learn lacks {want}:\n{out}");
    }
    assert!(!out.contains("heartbeat"), "learn drafts what the weave knows:\n{out}");
    b.ok(&["forget-known", "toy", "quiet", "cache warmed in 1 passes"]).await;
    b.until("the forget", 10, || {
        b.lines("known", |l| l["class"].is_null()).into_iter().next()
    })
    .await;
    let out = b.ok(&["learn", "toy"]).await;
    assert!(!out.contains("cache warmed"), "{out}");
    let o = b.sw(&["forget-known", "toy", "quiet", "never seen 1"]).await;
    assert!(!o.status.success(), "forgetting an unknown kind passed");

    // 6m. A second service: its goal comes AFTER toy:tidy and reads toy:spooled. plan
    // refuses a service the box does not run and an AFTER cycle across services; so does run.
    phase("6m composition");
    let mut args = vec!["run", &relay, "--yes", "-d"];
    args.extend(models);
    let out = b.ok(&args).await;
    assert!(out.contains("handed relay to the daemon"), "{out}");
    b.until("relayed to hold", 30, || {
        (b.row("relay")["states"]["relayed"]["state"] == "holds").then_some(())
    })
    .await;
    let o = b.sw(&["plan", &relay]).await;
    assert!(o.status.success() && !both(&o).contains("not checked"), "{}", both(&o));
    let stray = dir.join("examples/stray");
    std::fs::create_dir_all(&stray).unwrap();
    std::fs::write(
        stray.join("Weavefile"),
        format!(
            "ABOUT \"waits on a service this box does not run\"\nFROM {}\nFACT up BOOL true\nGOAL up-too \"it is up\" AFTER nosuch:up up\n",
            from_line()
        ),
    )
    .unwrap();
    let o = b.sw(&["plan", &stray.display().to_string()]).await;
    assert!(
        !o.status.success() && both(&o).contains("Weavefile:4: AFTER nosuch:up: no service nosuch on this box"),
        "{}",
        both(&o)
    );
    // A new toy weave AFTER relay:relayed closes a cycle through the relay.
    let cycle = dir.join("cycle/toy");
    std::fs::create_dir_all(&cycle).unwrap();
    std::fs::write(
        cycle.join("Weavefile"),
        format!(
            "ABOUT \"a toy that waits on its relay\"\nFROM {}\nFACT empty BOOL true\nGOAL tidy \"the spool is empty\" AFTER relay:relayed empty\n",
            from_line()
        ),
    )
    .unwrap();
    let cycle = cycle.display().to_string();
    let want = "Weavefile:4: AFTER cycle across services: toy:tidy → relay:relayed → toy:tidy";
    let o = b.sw(&["plan", &cycle]).await;
    assert!(!o.status.success() && both(&o).contains(want), "{}", both(&o));
    let o = b.sw(&["run", &cycle, "--yes", "-d"]).await;
    assert!(!o.status.success() && both(&o).contains(want), "{}", both(&o));

    // 6l. ASK BEFORE CHANGES data gates purge, which it never names.
    phase("6l effects, undo, part");
    std::fs::write(dir.join("spool/job-1"), "x").unwrap();
    let (n, words) = b.until("the purge ask", 20, || b.open_ask("tidy")).await;
    // The relay waits on the toy's failing goal: no check, no repair, no escalation.
    b.until("relayed to wait on toy:tidy", 10, || {
        (b.row("relay")["states"]["relayed"] == json!({"state": "waiting", "on": "toy:tidy"}))
            .then_some(())
    })
    .await;
    let out = b.ok(&["ps"]).await;
    assert!(out.contains("\n  relayed: waits on toy:tidy\n"), "{out}");
    let waited = b.journal_of("relay").into_iter().filter(|l| l["kind"] == "waiting").count();
    assert!(waited >= 1, "no waiting line in the relay's journal");
    let ask = b.lines("ask", |l| l["n"] == n).remove(0);
    assert_eq!(ask["action"], "purge", "{ask}");
    assert_eq!(ask["rule"], "ASK BEFORE CHANGES data", "{ask}");
    // The card names the effect and the rule that holds purge, and reads only `spooled`.
    let card = &ask["card"];
    assert_eq!(card["action"]["name"], "purge", "{ask}");
    assert_eq!(card["action"]["changes"], json!(["data"]), "{ask}");
    assert_eq!(
        card["action"]["held"],
        json!({"rule": "ASK BEFORE CHANGES data", "because": "it deletes what the spool holds"}),
        "{ask}"
    );
    assert_eq!(card["reads"], json!([{"fact": "spooled", "value": "1"}]), "{ask}");
    assert!(!card.to_string().contains("scene"), "{ask}");
    let hooked = b
        .until("the webhook to get the purge ask", 10, || {
            mock.hooks().into_iter().find(|h| h["kind"] == "ask" && h["ask"]["n"] == n)
        })
        .await;
    assert_eq!(hooked["card"]["action"], card["action"], "{hooked}");
    assert!(
        hooked["card"]["summary"] == json!(CARD_SUMMARY) && !hooked.to_string().contains("scene"),
        "{hooked}"
    );
    assert!(dir.join("spool/job-1").exists(), "purge ran before the answer");
    assert!(words.contains(&"yes".to_string()), "{words:?}");
    b.ok(&["answer", &n.to_string(), "yes"]).await;
    b.until("purge to pass", 20, || (b.passes("purge") == 1).then_some(())).await;
    assert!(!dir.join("spool/job-1").exists());
    b.holds(&["tidy"], 10).await;
    b.until("relayed to hold again", 10, || {
        (b.row("relay")["states"]["relayed"]["state"] == "holds").then_some(())
    })
    .await;
    let repairs: Vec<Value> = b
        .journal_of("relay")
        .into_iter()
        .filter(|l| ["action", "decide", "ask", "wait"].contains(&l["kind"].as_str().unwrap_or("")))
        .collect();
    assert!(repairs.is_empty(), "the relay repaired or escalated: {repairs:?}");
    assert!(!dir.join("relayed").exists(), "relay ran");

    // A fix whose goal does not hold within its WITHIN: its UNDO runs and is journaled.
    std::fs::write(dir.join("setting"), "off\n").unwrap();
    let undone = b
        .until("the undo of tune", 30, || {
            b.lines("action", |l| l["action"] == "untune" && !l["ok"].is_null())
                .into_iter()
                .next()
        })
        .await;
    assert_eq!(undone["ok"], true, "{undone}");
    let tune = b.lines("action", |l| l["action"] == "tune" && !l["ok"].is_null());
    assert!(tune.len() == 1 && tune[0]["ok"] == false, "{tune:?}");
    let undo_start = b.lines("action", |l| l["action"] == "untune" && l["ok"].is_null());
    assert!(undo_start.len() == 1 && undo_start[0]["undoes"] == "tune", "{undo_start:?}");
    assert!(
        b.lines("action", |l| l["action"] == "tune" && l["ok"].is_null())[0]["undo"] == "untune",
        "the start of tune names its undo"
    );
    assert_eq!(std::fs::read_to_string(dir.join("setting")).unwrap(), "off\n");
    std::fs::write(dir.join("setting"), "on\n").unwrap();
    b.holds(&["tuned"], 15).await;

    // A PART reads its running version and GitHub's releases; the decider reads each
    // newer release's notes once, and a security one makes `upgrade` the fix.
    let asked = || {
        mock.seen()
            .requests
            .iter()
            .filter(|(p, _, body)| p == "/systemone" && body.contains("\"release.0\""))
            .count()
    };
    assert_eq!(asked(), 0, "running the newest release needs no verdict");
    std::fs::write(dir.join("web-version"), "1.0.0\n").unwrap();
    b.until("upgrade to pass", 30, || (b.passes("upgrade") == 1).then_some(())).await;
    b.holds(&["patched"], 10).await;
    let read = b
        .lines("check", |l| l["goal"] == "patched" && l["facts"]["web.running"] == "1.0.0")
        .remove(0);
    assert_eq!(read["facts"]["web.newest"], "v1.2.0", "{read}");
    assert_eq!(read["facts"]["web.security"], "yes", "{read}");
    // The warning that the loop fixes `patched` carries a PART card: running, newest and
    // whether a security release waits; the part's facts are not read one by one.
    let warned = b
        .until("the webhook to get patched's warning", 10, || {
            mock.hooks()
                .into_iter()
                .find(|h| h["kind"] == "warning" && h["goal"] == "patched")
        })
        .await;
    let part = &warned["card"]["parts"][0];
    assert!(
        part["name"] == "web" && part["running"] == "1.0.0" && part["newest"] == "v1.2.0" && part["security"] == "yes",
        "{warned}"
    );
    assert!(warned["card"].get("reads").is_none() && !warned.to_string().contains("scene"), "{warned}");
    let verdicts = b.lines("release", |l| l["part"] == "web");
    let mut versions: Vec<&str> = verdicts.iter().filter_map(|l| l["text"].as_str()).collect();
    versions.sort();
    assert_eq!(versions, ["1.1.0", "1.2.0"], "{verdicts:?}");
    for v in &verdicts {
        assert_eq!(v["security"], v["text"] == "1.2.0", "{v}");
    }
    // Again behind: the verdicts are memory, the list is cached, nobody is asked again.
    std::fs::write(dir.join("web-version"), "1.0.0\n").unwrap();
    b.until("a second upgrade", 30, || (b.passes("upgrade") == 2).then_some(())).await;
    assert_eq!(asked(), 1, "the decider judged each release once");
    assert_eq!(b.lines("release", |_| true).len(), 2);
    assert_eq!(mock.seen().releases, 1, "the release list is cached");
    // A person's word beats the model's verdict.
    let out = b.ok(&["known", "toy", "web", "routine", "v1.2.0", "--why", "not reachable here"]).await;
    assert!(out.contains("toy web 1.2.0: known routine"), "{out}");
    let told = b
        .until("the person's release line", 10, || {
            b.lines("release", |l| l["by"].is_string()).into_iter().next()
        })
        .await;
    assert!(told["text"] == "1.2.0" && told["security"] == false, "{told}");
    std::fs::write(dir.join("web-version"), "1.0.0\n").unwrap();
    b.until("patched to hold while behind", 15, || {
        b.lines("check", |l| {
            l["goal"] == "patched" && l["ok"] == true && l["facts"]["web.running"] == "1.0.0"
        })
        .into_iter()
        .next()
    })
    .await;
    assert_eq!(b.passes("upgrade"), 2, "no upgrade for a routine release");
    std::fs::write(dir.join("web-version"), "1.2.0\n").unwrap();

    // 7. ps, log, learned.
    phase("7 ps, log, learned");
    let out = b.ok(&["ps"]).await;
    let row = out
        .lines()
        .find(|l| l.starts_with("toy "))
        .unwrap_or_else(|| b.fail(&format!("no toy in ps:\n{out}")));
    assert!(row.contains("8/8"), "{row}");
    assert!(
        row.contains("typesafe/jev-1.13@mock / deepseek/x@mock"),
        "{row}"
    );
    let out = b.ok(&["log", "toy"]).await;
    for want in [
        "learn",
        "steps for install/installed: 2 commands",
        "fact served",
        "gate",
        "ASK BEFORE rotate",
        "decide",
        "restart",
    ] {
        assert!(out.contains(want), "log lacks {want:?}:\n{out}");
    }
    assert_eq!(
        out.lines().count(),
        b.journal().len(),
        "log prints every journal line"
    );
    let out = b.ok(&["learned", "toy"]).await;
    assert!(
        out.contains("facts") && out.contains("served") && out.contains("curl"),
        "{out}"
    );
    assert!(
        out.contains("steps") && out.contains("install/installed"),
        "{out}"
    );
    assert!(
        out.contains("mkdir -p") && out.contains("index.html"),
        "{out}"
    );

    // 8. The dashboard: a password, the API, the live journal, a task in words.
    phase("8 dash");
    dashboard(&mut b).await;

    // Traces: the first install's repair is one trace whose id is its episode, with a
    // child span per command line.
    phase("traces");
    let ep = b
        .lines("command", |l| l["action"] == "install")
        .first()
        .and_then(|l| l["ep"].as_str().map(String::from))
        .unwrap_or_else(|| b.fail("no install command with an ep"));
    let commands: Vec<String> = b
        .lines("command", |l| l["ep"] == ep)
        .iter()
        .filter_map(|l| l["text"].as_str().map(String::from))
        .collect();
    assert_eq!(commands, [MKDIR, WRITE]);
    let (resources, spans) = b
        .until("the install episode's trace", 15, || {
            use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
            use prost::Message;
            let seen = mock.seen();
            let mut resources = Vec::new();
            let mut spans = Vec::new();
            for body in &seen.traces {
                let Ok(req) = ExportTraceServiceRequest::decode(body.as_slice()) else {
                    continue;
                };
                for rs in req.resource_spans {
                    let mine: Vec<_> = rs
                        .scope_spans
                        .into_iter()
                        .flat_map(|ss| ss.spans)
                        .filter(|s| hex::encode(&s.trace_id) == ep)
                        .collect();
                    if !mine.is_empty() {
                        resources.extend(rs.resource.into_iter().flat_map(|r| r.attributes));
                        spans.extend(mine);
                    }
                }
            }
            spans
                .iter()
                .any(|s| s.parent_span_id.is_empty())
                .then_some((resources, spans))
        })
        .await;
    let text = |attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue], key: &str| {
        use opentelemetry_proto::tonic::common::v1::any_value::Value as V;
        attrs
            .iter()
            .find(|kv| kv.key == key)
            .and_then(|kv| match kv.value.as_ref()?.value.as_ref()? {
                V::StringValue(s) => Some(s.clone()),
                _ => None,
            })
    };
    assert_eq!(text(&resources, "service.name").as_deref(), Some("subweave"));
    assert!(text(&resources, "host.name").is_some(), "{resources:?}");
    let roots: Vec<_> = spans.iter().filter(|s| s.parent_span_id.is_empty()).collect();
    assert_eq!(roots.len(), 1, "{spans:?}");
    let root = roots[0];
    assert_eq!(root.name, "repair of installed");
    assert_eq!(text(&root.attributes, "subweave.kind").as_deref(), Some("repair"));
    assert!(
        spans
            .iter()
            .filter(|s| !s.parent_span_id.is_empty())
            .all(|s| s.parent_span_id == root.span_id),
        "every step is a child of the job"
    );
    let traced: Vec<String> = spans
        .iter()
        .filter(|s| s.name == "command")
        .filter_map(|s| text(&s.attributes, "subweave.command"))
        .collect();
    assert_eq!(traced, commands, "a span per command line");
    assert!(
        spans
            .iter()
            .filter(|s| s.name == "command")
            .all(|s| s.end_time_unix_nano > s.start_time_unix_nano
                && s.start_time_unix_nano >= root.start_time_unix_nano),
        "{spans:?}"
    );

    // 6d. The secret never left the box: journal, state, webhook, model requests.
    let journal_text = std::fs::read_to_string(b.service().join("journal.jsonl")).unwrap();
    assert!(
        !journal_text.contains(SECRET),
        "the journal holds the secret"
    );
    for f in ["state.json", "learned.json", "service.json"] {
        let text = std::fs::read_to_string(b.service().join(f)).unwrap();
        assert!(!text.contains(SECRET), "{f} holds the secret");
    }
    let hooks = mock.hooks();
    assert!(!hooks.is_empty());
    for h in &hooks {
        assert!(
            !h.to_string().contains(SECRET),
            "a webhook body holds the secret: {h}"
        );
    }
    // A card shows its verdict's read set: `scene`, which no goal reads, is on none.
    assert!(
        !hooks.iter().any(|h| h.to_string().contains("[TOY_TOKEN]")),
        "the scene fact reached the webhook"
    );
    {
        let seen = mock.seen();
        for (path, _, body) in &seen.requests {
            assert!(!body.contains(SECRET), "a {path} request holds the secret");
            assert!(
                !body.contains(API_KEY),
                "a {path} request holds the API key"
            );
        }
        assert!(
            seen.requests
                .iter()
                .any(|(p, _, b)| p == "/chat/completions" && b.contains("token [TOY_TOKEN]")),
            "the coder read the mkdir output redacted"
        );
        assert!(
            seen.requests
                .iter()
                .any(|(p, _, b)| p == "/systemone" && b.contains("calm [TOY_TOKEN]")),
            "the decider read the scene fact redacted"
        );
        assert!(
            seen.unexpected.is_empty(),
            "unscripted: {:?}",
            seen.unexpected
        );
    }

    // 6h. run again, as for a new weave version: no request, no TOY_TOKEN in the shell. The
    // ARGs and the SECRET stay, and the request check asks the decider nothing.
    phase("6h run again");
    let checks = || {
        mock.seen()
            .requests
            .iter()
            .filter(|(p, _, b)| p == "/systemone" && b.contains("\"words\""))
            .count()
    };
    let checks_before = checks();
    let spec_before = std::fs::read(b.service().join("service.json")).unwrap();
    let mut args = vec!["run", &weave, "--yes", "-d"];
    args.extend(models);
    let o = b
        .command()
        .env_remove("TOY_TOKEN")
        .args(&args)
        .output()
        .await
        .expect("subweave runs");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{out}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(out.contains("handed toy to the daemon"), "{out}");
    assert_eq!(checks(), checks_before, "the run again asked the decider");
    b.until("the daemon to take the run again", 15, || {
        b.lines("note", |l| {
            l["text"].as_str().is_some_and(|t| t.starts_with("run "))
        })
        .get(1)
        .cloned()
    })
    .await;
    let spec: Value =
        serde_json::from_slice(&std::fs::read(b.service().join("service.json")).unwrap()).unwrap();
    let was: Value = serde_json::from_slice(&spec_before).unwrap();
    assert_eq!(spec["args"], was["args"]);
    assert_eq!(spec["request"], was["request"]);
    assert_eq!(spec["created"], was["created"]);
    let secrets = std::fs::read_to_string(b.service().join("secrets.json")).unwrap();
    assert!(secrets.contains(SECRET), "the run again kept TOY_TOKEN");
    b.holds(&["installed", "running"], 30).await;

    // 6n. Drills. `down` is due every 2s once `drills` is on, but not while a goal fails;
    // then it runs, the loop notices and restarts the server, and RESTORE does not run.
    // `blind`, by hand, breaks nothing `tidy` reads: it escalates and RESTORE runs.
    phase("6n drills");
    let drill = |b: &Box_, name: &str| b.lines("drill", |l| l["drill"] == name);
    std::fs::write(dir.join("setting"), "off\n").unwrap();
    b.until("tuned to fail", 15, || (b.goal("tuned") == "fails").then_some(()))
        .await;
    std::fs::write(dir.join("drills"), "on\n").unwrap();
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(drill(&b, "down").is_empty(), "an EVERY drill started while tuned failed");
    std::fs::write(dir.join("setting"), "on\n").unwrap();
    b.holds(&["tuned"], 15).await;
    b.until("down to start on its own", 120, || drill(&b, "down").into_iter().next())
        .await;
    std::fs::write(dir.join("drills"), "off\n").unwrap();
    let ended = b
        .until("down to end", 60, || {
            drill(&b, "down").into_iter().find(|l| l["step"] == "ended")
        })
        .await;
    if ended["ok"] != true {
        b.fail(&format!("down did not pass: {ended}"));
    }
    let steps = Value::Array(drill(&b, "down").iter().map(|l| l["step"].clone()).collect());
    assert_eq!(steps, json!(["started", "broke", "detected", "fixed", "ended"]), "{steps:?}");
    let first = &drill(&b, "down")[0];
    assert!(first.get("by").is_none(), "EVERY started down, not a person: {first}");
    assert!(!dir.join("down-restored").exists(), "RESTORE ran after a drill that passed");
    let out = b.ok(&["ps"]).await;
    assert!(out.contains("down passed"), "{out}");

    let out = b.ok(&["drill", "toy", "blind"]).await;
    assert!(out.contains("asked the daemon to run drill blind on toy"), "{out}");
    let ended = b
        .until("blind to end", 30, || {
            drill(&b, "blind").into_iter().find(|l| l["step"] == "ended")
        })
        .await;
    let label = "Leave a note outside the spool";
    assert_eq!(ended["ok"], false, "{ended}");
    assert_eq!(ended["text"], format!("tidy did not notice {label}"), "{ended}");
    let steps = Value::Array(drill(&b, "blind").iter().map(|l| l["step"].clone()).collect());
    assert_eq!(steps, json!(["started", "broke", "detected", "restored", "ended"]), "{steps:?}");
    let missed = &drill(&b, "blind")[2];
    assert!(missed["ok"] == false && missed["text"] == "not within 3s", "{missed}");
    assert!(dir.join("blind-restored").exists(), "RESTORE did not run");
    let ask = b
        .until("blind's escalation", 15, || {
            b.lines("ask", |l| l["drill"] == "blind").into_iter().next()
        })
        .await;
    assert_eq!(ask["action"], "drill", "{ask}");
    assert!(
        ask["card"]["title"].as_str().is_some_and(|t| t.contains("`tidy` did not notice")),
        "{ask}"
    );
    // The card says which drill, and shows tidy's read set.
    assert_eq!(ask["card"]["drill"], json!({"name": "blind", "label": label}), "{ask}");
    assert_eq!(ask["card"]["goal"]["name"], "tidy", "{ask}");
    assert!(!ask["card"].to_string().contains("scene"), "{ask}");
    let out = b.ok(&["ps"]).await;
    assert!(out.contains("blind failed") && out.contains(&format!("drill blind: tidy did not notice {label}")), "{out}");
    b.ok(&["answer", &ask["n"].to_string(), "no"]).await;

    // 9. Escalation targets: an agent channel, Steery sessions, weave changes, stats.
    phase("9 escalation");
    escalation(&b, &mock, &weave, port, &www).await;

    // 7. stop: the loop ends and the journal stops growing, even with the server down.
    phase("7 stop");
    let out = b.ok(&["stop", "toy"]).await;
    assert!(out.contains("asked the daemon to stop toy"), "{out}");
    b.until("the stop", 10, || {
        (b.status()["stopped"] == true && b.journal().last()?["text"] == "stopped").then_some(())
    })
    .await;
    // A service the relay comes AFTER is stopped: its goal cannot be judged, and ps says why.
    b.until("relayed to be unknown", 10, || {
        (b.row("relay")["states"]["relayed"] == json!({"state": "unknown", "why": "toy is stopped"}))
            .then_some(())
    })
    .await;
    let out = b.ok(&["ps"]).await;
    assert!(out.contains("\n  relayed: unknown: toy is stopped\n"), "{out}");
    let out = b.ok(&["ps"]).await;
    assert!(
        out.lines()
            .any(|l| l.starts_with("toy ") && l.contains("stopped")),
        "{out}"
    );
    let size = std::fs::metadata(b.service().join("journal.jsonl"))
        .unwrap()
        .len();
    let pid = b.pid();
    let requests = mock.seen().requests.len();
    kill_pidfile(&dir.join("toy.pid"));
    // Nothing to poll for: the claim is that nothing happens. Three EVERYs pass.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        std::fs::metadata(b.service().join("journal.jsonl"))
            .unwrap()
            .len(),
        size,
        "the journal grew after stop"
    );
    assert_eq!(b.pid(), pid, "a stopped service restarted its server");
    assert_eq!(
        mock.seen().requests.len(),
        requests,
        "a stopped service called a model"
    );

    eprintln!("e2e scenario took {:.1}s", started.elapsed().as_secs_f64());
}
