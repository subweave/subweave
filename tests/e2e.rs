//! One end-to-end scenario: the built `subweave` binary runs as a daemon and as CLI
//! commands against a temporary state and config directory. Only the vendors are
//! mocked: one local HTTP server plays OpenRouter (`/systemone` for the decider,
//! `/chat/completions` for the coder) and a webhook receiver.
//!
//! The mock answers by what each request asks (question keys, the failing goal, the
//! box's facts, the coder's earlier tool calls), never by call order.

use std::net::TcpListener as StdListener;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use serde_json::{Map, Value, json};

const SECRET: &str = "tok-e2e-5f1c9a77d3";
const API_KEY: &str = "mock-key-e2e-81b2c";
const FIREWALL_RULE: &str = "open a port on the firewall";

/// Every request the mock got.
#[derive(Default)]
struct Seen {
    /// (path, Authorization header, body).
    requests: Vec<(String, String, String)>,
    /// Webhook bodies.
    hooks: Vec<Value>,
    /// Questions or requests the script has no answer for.
    unexpected: Vec<String>,
}

#[derive(Clone)]
struct Mock {
    seen: Arc<Mutex<Seen>>,
    /// Where the forbidden coder command would leave its mark.
    firewall_mark: PathBuf,
}

impl Mock {
    fn seen(&self) -> std::sync::MutexGuard<'_, Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn count(&self, path: &str) -> usize {
        self.seen()
            .requests
            .iter()
            .filter(|(p, _, _)| p == path)
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

/// `/chat/completions`: a fact learner gets the fact command; a person's task gets one
/// command and `done`; an action's coder goes on from the tool calls it already made: a
/// forbidden firewall command, mkdir, the page, then `done`.
async fn chat(State(m): State<Mock>, headers: HeaderMap, body: String) -> axum::Json<Value> {
    record(&m, "/chat/completions", &headers, &body);
    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
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
}

impl Drop for Box_ {
    fn drop(&mut self) {
        for c in [&mut self.daemon, &mut self.dash].into_iter().flatten() {
            let _ = c.start_kill();
        }
        kill_pidfile(&self.dir.join("toy.pid"));
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
    fn command(&self) -> tokio::process::Command {
        let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_subweave"));
        c.env("SUBWEAVE_STATE", &self.state)
            .env("SUBWEAVE_CONFIG", &self.config)
            .env("MOCK_API_KEY", API_KEY)
            .env("TOY_TOKEN", SECRET)
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("GITHUB_TOKEN")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        c
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
        std::fs::read_to_string(self.service().join("journal.jsonl"))
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

    fn status(&self) -> Value {
        std::fs::read(self.service().join("status.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or(Value::Null)
    }

    fn goal(&self, name: &str) -> String {
        self.status()["goals"]
            .as_array()
            .and_then(|gs| gs.iter().find(|g| g["name"] == name))
            .and_then(|g| g["state"].as_str())
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
            "{what}\n--- status.json\n{}\n--- journal.jsonl\n{journal}\n--- daemon log\n{log}",
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

    /// The open ask for `goal` in the journal: its number and answers.
    fn open_ask(&self, goal: &str) -> Option<(u64, Vec<String>)> {
        let j = self.journal();
        let answered: Vec<u64> = j
            .iter()
            .filter(|l| l["kind"] == "answer")
            .filter_map(|l| l["n"].as_u64())
            .collect();
        j.iter()
            .filter(|l| l["kind"] == "ask" && l["goal"] == goal)
            .filter_map(|l| Some((l["n"].as_u64()?, l)))
            .find(|(n, _)| !answered.contains(n))
            .map(|(n, l)| {
                let words = l["answers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w.as_str().map(String::from))
                    .collect();
                (n, words)
            })
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
ASK   BEFORE rotate

FACT present  test -f $root/index.html
FACT scene    echo "$(cat {d}/scene 2>/dev/null) $TOY_TOKEN"
FACT served   "1 when http://127.0.0.1:$port/ answers HTTP 200 with a page that contains the word hello, else 0"

GOAL installed                  EVERY 1s                   present
GOAL running   AFTER installed  EVERY 1s  TRIES 10 PER 1h  served == 1
GOAL rotated   AFTER running    EVERY 1s                   SINCE(rotate) < 1h

ACTION install FOR installed WITHIN 5s
  WHEN "the directory or its index.html is missing: nothing is installed yet"
  DO   "create the directory $root and write $root/index.html, a small HTML page that says hello"

ACTION restart FOR running WITHIN 5s
  WHEN "the fix when the server process is gone, crashed or was killed, and its files are in place: a restart brings it back"
  RUN <<EOF
    kill "$(cat {d}/toy.pid 2>/dev/null)" 2>/dev/null || true
    cd "$root"
    nohup python3 -m http.server "$port" --bind 127.0.0.1 >{d}/http.log 2>&1 &
    echo $! > {d}/toy.pid
    for i in $(seq 100); do curl -fs "http://127.0.0.1:$port/" >/dev/null 2>&1 && exit 0; sleep 0.1; done
    exit 1
  EOF

ACTION fix FOR installed, running WITHIN 5s
  WHEN "no other action fits, or the fitting ones already failed"
  DO   "make the failing goal hold again"

ACTION rotate FOR rotated WITHIN 5s
  WHEN "the server log needs a fresh start"
  RUN  date > {d}/rotated
"#,
        from = from_line(),
        www = www.display(),
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
    };
    let weave_dir = dir.join("examples/toy");
    std::fs::create_dir_all(&weave_dir).unwrap();
    std::fs::write(weave_dir.join("Weavefile"), weavefile(&dir, &www)).unwrap();
    std::fs::write(dir.join("scene"), "calm").unwrap();
    let weave = weave_dir.display().to_string();

    // The vendors: OpenRouter and a webhook, on one local server.
    let mock = Mock {
        seen: Arc::new(Mutex::new(Seen::default())),
        firewall_mark: dir.join("firewall-opened"),
    };
    let app = Router::new()
        .route("/systemone", post(systemone))
        .route("/chat/completions", post(chat))
        .route("/hook", post(hook))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock_url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

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
    assert_eq!(mock.count("/chat/completions"), 0);

    // The daemon, in the background.
    let log = std::fs::File::create(dir.join("daemon.log")).unwrap();
    let daemon = b
        .command()
        .env_remove("TOY_TOKEN")
        .arg("daemon")
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("the daemon starts");
    b.daemon = Some(daemon);

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
    let chats_after_install = mock.count("/chat/completions");
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
        row.contains("toy") && row.contains("rotated") && row.contains("run rotate for rotated?"),
        "{row}"
    );
    assert!(row.contains("[yes no always never]"), "{row}");
    let hook_ask = b
        .until("the webhook to get the rotate ask", 10, || {
            mock.hooks()
                .into_iter()
                .find(|h| h["kind"] == "ask" && h["ask"]["n"] == n)
        })
        .await;
    assert_eq!(hook_ask["service"], "toy");
    assert_eq!(hook_ask["goal"], "rotated");
    assert_eq!(
        hook_ask["ask"]["answers"],
        json!(["yes", "no", "always", "never"])
    );
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
            h["title"]
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
        mock.count("/chat/completions"),
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
        mock.count("/chat/completions"),
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
        row.contains("running fails: which action?") && row.contains("[restart fix no]"),
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
    assert!(out.contains("no open asks"), "{out}");

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
    assert_eq!(mock.count("/chat/completions"), chats_after_install);

    // 6f. An ask whose goal holds again is withdrawn: the person brought the server back.
    phase("6f withdrawn ask");
    std::fs::write(dir.join("scene"), "unsure").unwrap();
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
        .until("the withdrawn answer", 30, || {
            b.lines("answer", |l| l["n"] == n).into_iter().next()
        })
        .await;
    assert_eq!(withdrawn["text"], "withdrawn", "{withdrawn}");
    assert_eq!(withdrawn["by"], "daemon: running holds again", "{withdrawn}");
    let out = b.ok(&["inbox"]).await;
    assert!(out.contains("no open asks"), "{out}");
    b.until("the webhook to hear the withdrawal", 10, || {
        mock.hooks()
            .into_iter()
            .find(|h| h["title"] == format!("#{n} withdrawn"))
    })
    .await;
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
        b.lines("answer", |l| l["n"] == n).is_empty(),
        "a person's ask is not withdrawn"
    );
    b.ok(&["answer", &n.to_string(), "no"]).await;

    // 7. ps, log, learned.
    phase("7 ps, log, learned");
    let out = b.ok(&["ps"]).await;
    let row = out
        .lines()
        .find(|l| l.starts_with("toy "))
        .unwrap_or_else(|| b.fail(&format!("no toy in ps:\n{out}")));
    assert!(row.contains("3/3"), "{row}");
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

    // 6d. The secret never left the box: journal, state, webhook, model requests.
    let journal_text = std::fs::read_to_string(b.service().join("journal.jsonl")).unwrap();
    assert!(
        !journal_text.contains(SECRET),
        "the journal holds the secret"
    );
    for f in ["status.json", "learned.json", "service.json"] {
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
    assert!(
        hooks.iter().any(|h| h.to_string().contains("[TOY_TOKEN]")),
        "the scene fact reached the webhook redacted"
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

    // 7. stop: the loop ends and the journal stops growing, even with the server down.
    phase("7 stop");
    let out = b.ok(&["stop", "toy"]).await;
    assert!(out.contains("asked the daemon to stop toy"), "{out}");
    b.until("the stop", 10, || {
        (b.status()["stopped"] == true && b.journal().last()?["text"] == "stopped").then_some(())
    })
    .await;
    let out = b.ok(&["ps"]).await;
    assert!(
        out.lines()
            .any(|l| l.starts_with("toy ") && l.contains("stopped")),
        "{out}"
    );
    let size = std::fs::metadata(b.service().join("journal.jsonl"))
        .unwrap()
        .len();
    let status_t = b.status()["t"].clone();
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
    assert_eq!(b.status()["t"], status_t, "status.json changed after stop");
    assert_eq!(b.pid(), pid, "a stopped service restarted its server");
    assert_eq!(
        mock.seen().requests.len(),
        requests,
        "a stopped service called a model"
    );

    eprintln!("e2e scenario took {:.1}s", started.elapsed().as_secs_f64());
}
