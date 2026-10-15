//! `subweave mcp`: an MCP server on stdin and stdout, for an agent that answers this box's
//! asks for its owner. One JSON-RPC message per line; stdout carries nothing else. Each
//! tool goes through the code path of its CLI command, `by` = `mcp:<client name>`.

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Stdout};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::{act, inbox, ps, task};
use crate::{paths, service};

const PROTOCOL: &str = "2025-06-18";
const INSTRUCTIONS: &str = "You answer this box's asks for its owner: read `inbox`, and call `wait_ask` to block until a new ask comes. Answer an ask only with one of its `answers`, and leave an ask you are unsure about open for the owner.";
/// How often `wait_ask` reads the asks.
const POLL: Duration = Duration::from_secs(2);

pub async fn run() -> Result<ExitCode> {
    let out = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut by = client_by(None);
    while let Some(line) = lines.next_line().await.context("read stdin")? {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                send(&out, error(Value::Null, -32600, "not a JSON-RPC message")).await;
                continue;
            }
            Err(e) => {
                send(&out, error(Value::Null, -32700, &format!("parse error: {e}"))).await;
                continue;
            }
        };
        // Responses and notifications (`notifications/initialized`, cancels) need no reply.
        let (Some(method), Some(id)) = (msg["method"].as_str(), msg.get("id").cloned()) else {
            continue;
        };
        let params = &msg["params"];
        match method {
            "initialize" => {
                by = client_by(params["clientInfo"]["name"].as_str());
                let init = json!({
                    "protocolVersion": PROTOCOL,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "subweave", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                });
                send(&out, result(id, init)).await;
            }
            "ping" => send(&out, result(id, json!({}))).await,
            "tools/list" => send(&out, result(id, json!({"tools": tools()}))).await,
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or_default().to_string();
                if !tools().iter().any(|t| t["name"] == name) {
                    send(&out, error(id, -32602, &format!("no tool {name:?}"))).await;
                    continue;
                }
                let args = match params.get("arguments") {
                    Some(Value::Null) | None => json!({}),
                    Some(a) => a.clone(),
                };
                let (out, by) = (out.clone(), by.clone());
                // `wait_ask` blocks; other requests are answered meanwhile.
                tokio::spawn(async move {
                    let reply = tool_result(call(&name, args, &by).await);
                    send(&out, result(id, reply)).await;
                });
            }
            _ => send(&out, error(id, -32601, &format!("no method {method}"))).await,
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn client_by(name: Option<&str>) -> String {
    let name = name.map(str::trim).filter(|n| !n.is_empty());
    format!("mcp:{}", name.unwrap_or("unknown"))
}

async fn send(out: &Mutex<Stdout>, msg: Value) {
    let mut line = msg.to_string();
    line.push('\n');
    let mut out = out.lock().await;
    let written = async {
        out.write_all(line.as_bytes()).await?;
        out.flush().await
    };
    if let Err(e) = written.await {
        eprintln!("subweave mcp: cannot write stdout: {e}");
    }
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_result(r: Result<Value>) -> Value {
    match r {
        Ok(v) => {
            let text = serde_json::to_string_pretty(&v).unwrap_or_else(|_| v.to_string());
            json!({"content": [{"type": "text", "text": text}]})
        }
        Err(e) => json!({"content": [{"type": "text", "text": format!("{e:#}")}], "isError": true}),
    }
}

fn tools() -> Vec<Value> {
    let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
    let service = json!({"type": "string", "description": "The service's name"});
    vec![
        json!({
            "name": "inbox",
            "description": "Every open ask on this box: service, n, goal, action, rule, title, text (its body), facts, answers (the only words that answer it) and t (when it was raised).",
            "inputSchema": none,
        }),
        json!({
            "name": "wait_ask",
            "description": "Blocks until an open ask not in `seen` exists, then gives every open ask, as `inbox` does; an empty list once `timeout_secs` pass first.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 3600, "default": 600},
                    "seen": {
                        "type": "array",
                        "description": "Asks you already have",
                        "items": {
                            "type": "object",
                            "properties": {"service": {"type": "string"}, "n": {"type": "integer"}},
                            "required": ["service", "n"],
                        },
                    },
                },
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "answer",
            "description": "Answers an open ask with one of its `answers`; the daemon acts on it. `note`, why, goes in the journal with the answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "service": service,
                    "n": {"type": "integer", "description": "The ask's number"},
                    "word": {"type": "string", "description": "One of the ask's answers"},
                    "note": {"type": "string", "description": "Why, in a sentence"},
                },
                "required": ["service", "n", "word"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "status",
            "description": "Each service as `subweave ps` shows it: goals held and total, failing goals, open asks, repair, models, this month's spend in USD.",
            "inputSchema": none,
        }),
        json!({
            "name": "log",
            "description": "The newest lines of a service's journal, oldest first, as the journal stores them.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "service": service,
                    "lines": {"type": "integer", "minimum": 1, "maximum": 200, "default": 50},
                },
                "required": ["service"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "task",
            "description": "Gives a service's coder a task in words, as `subweave task`: through the same rules, when no repair runs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "service": service,
                    "text": {"type": "string", "description": "What to do, like \"vacuum the shop database\""},
                },
                "required": ["service", "text"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "do",
            "description": "Runs one ACTION of a service's weave now, as `subweave do`, through the same rules.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "service": service,
                    "action": {"type": "string", "description": "An ACTION of the service's weave"},
                },
                "required": ["service", "action"],
                "additionalProperties": false,
            },
        }),
    ]
}

async fn call(name: &str, args: Value, by: &str) -> Result<Value> {
    Ok(match name {
        "inbox" => serde_json::to_value(service::all_asks()?)?,
        "wait_ask" => serde_json::to_value(wait_ask(parse(args)?).await?)?,
        "answer" => answer(parse(args)?, by)?,
        "status" => serde_json::to_value(ps::rows()?)?,
        "log" => log(parse(args)?)?,
        "task" => {
            let a: TaskArgs = parse(args)?;
            task::send(&a.service, &a.text, by.to_string())?;
            json!({"service": a.service, "task": a.text.trim(), "by": by})
        }
        "do" => {
            let a: DoArgs = parse(args)?;
            act::send(&a.service, &a.action, by.to_string())?;
            json!({"service": a.service, "action": a.action, "by": by})
        }
        _ => bail!("no tool {name:?}"),
    })
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T> {
    serde_json::from_value(args).map_err(|e| anyhow!("arguments: {e}"))
}

#[derive(Deserialize)]
struct WaitArgs {
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    #[serde(default)]
    seen: Vec<Seen>,
}

fn default_timeout() -> u64 {
    600
}

#[derive(Deserialize)]
struct Seen {
    service: String,
    n: u64,
}

async fn wait_ask(a: WaitArgs) -> Result<Vec<service::Ask>> {
    if !(1..=3600).contains(&a.timeout_secs) {
        bail!("timeout_secs is 1 to 3600, not {}", a.timeout_secs);
    }
    let end = Instant::now() + Duration::from_secs(a.timeout_secs);
    loop {
        let asks = service::all_asks()?;
        let new = |ask: &service::Ask| {
            !a.seen
                .iter()
                .any(|s| s.n == ask.n && s.service == ask.service)
        };
        if asks.iter().any(new) {
            return Ok(asks);
        }
        let now = Instant::now();
        if now >= end {
            return Ok(Vec::new());
        }
        tokio::time::sleep(POLL.min(end - now)).await;
    }
}

#[derive(Deserialize)]
struct AnswerArgs {
    service: String,
    n: u64,
    word: String,
    note: Option<String>,
}

fn answer(a: AnswerArgs, by: &str) -> Result<Value> {
    let open = service::all_asks()?;
    if !open.iter().any(|ask| ask.n == a.n && ask.service == a.service) {
        bail!("{} has no open ask #{}: inbox lists them", a.service, a.n);
    }
    let note = a
        .note
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    inbox::send_answer(a.n, &a.word, by.to_string(), note.clone())?;
    Ok(json!({"service": a.service, "n": a.n, "word": a.word, "by": by, "note": note}))
}

#[derive(Deserialize)]
struct LogArgs {
    service: String,
    #[serde(default = "default_lines")]
    lines: usize,
}

fn default_lines() -> usize {
    50
}

fn log(a: LogArgs) -> Result<Value> {
    if !(1..=200).contains(&a.lines) {
        bail!("lines is 1 to 200, not {}", a.lines);
    }
    super::known(&a.service)?;
    let path = paths::journal(&a.service);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let all: Vec<&str> = service::numbered_lines(&text).collect();
    let newest = &all[all.len().saturating_sub(a.lines)..];
    Ok(Value::Array(
        newest
            .iter()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|_| Value::String((*l).to_string())))
            .collect(),
    ))
}

#[derive(Deserialize)]
struct TaskArgs {
    service: String,
    text: String,
}

#[derive(Deserialize)]
struct DoArgs {
    service: String,
    action: String,
}
