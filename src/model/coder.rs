//! The coder: a chat model that works through tools, one conversation per task, in a
//! chain of models (`MODEL CODE a THEN b`). OpenAi protocol → `POST {endpoint}/chat/completions`;
//! Anthropic → `POST {endpoint}/messages`.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

use super::ModelRef;
use super::provider::{self, Protocol, Provider};

const RETRIES: u32 = 3;
const MAX_TOKENS: u32 = 8192;

/// A tool the model may call; `parameters` is a JSON schema object.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub id: String,
    pub name: String,
    /// The arguments; `Value::Null` when the model sent no valid JSON.
    pub input: Value,
}

#[derive(Debug, Clone)]
pub enum Msg {
    User(String),
    Assistant {
        text: String,
        calls: Vec<Call>,
    },
    /// Tool results in call order: (call id, content).
    Results(Vec<(String, String)>),
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<Call>,
    pub cost_usd: f64,
    pub input_tokens: u64,
    /// Input tokens the provider read from its prompt cache, part of `input_tokens`.
    pub cached_tokens: u64,
    pub output_tokens: u64,
}

/// What one conversation with a model used.
#[derive(Debug, Clone, Default)]
pub struct Tally {
    pub calls: u32,
    /// Input tokens of the first request: the system prompt and the first message.
    pub first_input: u64,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub cost_usd: f64,
}

impl Tally {
    pub fn add(&mut self, r: &Reply) {
        if self.calls == 0 {
            self.first_input = r.input_tokens;
        }
        self.calls += 1;
        self.input += r.input_tokens;
        self.cached += r.cached_tokens;
        self.output += r.output_tokens;
        self.cost_usd += r.cost_usd;
    }

    /// `3 calls, system prompt 2140 chars, first request 812 tokens in, 2950 in (1600 cached), 210 out`.
    pub fn describe(&self, system_chars: usize) -> String {
        format!(
            "{} calls, system prompt {system_chars} chars, first request {} tokens in, {} in ({} cached), {} out",
            self.calls, self.first_input, self.input, self.cached, self.output
        )
    }

    /// The journal's record of this conversation with `model`.
    pub fn usage(&self, model: String) -> crate::service::Usage {
        crate::service::Usage {
            model,
            calls: self.calls,
            input: self.input,
            cached: self.cached,
            output: self.output,
        }
    }
}

struct Endpoint {
    model: ModelRef,
    provider: Provider,
    key: Option<String>,
}

pub struct Coder {
    chain: Vec<Endpoint>,
    client: reqwest::Client,
}

impl Coder {
    pub fn new(models: &[ModelRef]) -> Result<Coder> {
        if models.is_empty() {
            return Err(anyhow!("no coder model"));
        }
        let mut chain = Vec::with_capacity(models.len());
        for m in models {
            let provider = provider::get(&m.provider)?;
            let key = provider.key()?;
            chain.push(Endpoint {
                model: m.clone(),
                provider,
                key,
            });
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("starting the HTTP client")?;
        Ok(Coder { chain, client })
    }

    /// The chain's length.
    pub fn len(&self) -> usize {
        self.chain.len()
    }

    pub fn model(&self, i: usize) -> &ModelRef {
        &self.chain[i].model
    }

    /// One model turn of a conversation. The model must call a tool. `session` is sent to
    /// OpenRouter as `session_id`: it pins one provider from the first request, so the
    /// conversation's prompt cache holds.
    pub async fn send(
        &self,
        i: usize,
        session: Option<&str>,
        system: &str,
        msgs: &[Msg],
        tools: &[Tool],
    ) -> Result<Reply> {
        let ep = &self.chain[i];
        let (url, body) = match ep.provider.protocol {
            Protocol::OpenAi => (
                ep.provider.url("chat/completions"),
                openai_body(ep, session, system, msgs, tools),
            ),
            Protocol::Anthropic => (
                ep.provider.url("messages"),
                anthropic_body(ep, system, msgs, tools),
            ),
        };
        let mut attempt = 0;
        let text = loop {
            let mut req = self.client.post(&url).json(&body);
            if let Some(key) = &ep.key {
                req = match ep.provider.protocol {
                    Protocol::OpenAi => req.bearer_auth(key),
                    Protocol::Anthropic => req.header("x-api-key", key),
                };
            }
            if ep.provider.protocol == Protocol::Anthropic {
                req = req.header("anthropic-version", "2023-06-01");
            }
            // OpenRouter sends 200 at once and holds the body until the model answers, so
            // the 120 s limit can end while the body is read: that is a timeout too.
            let sent = match req.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    resp.text().await.map(|text| (status, text))
                }
                Err(e) => Err(e),
            };
            let retry = match sent {
                Ok((status, text)) => {
                    if status.is_success() {
                        break text;
                    }
                    let err = anyhow!(
                        "coder {}: HTTP {}: {}",
                        ep.model,
                        status.as_u16(),
                        short(&text)
                    );
                    if !(status.as_u16() == 429 || status.is_server_error()) {
                        return Err(err);
                    }
                    err
                }
                // A model that took two minutes is not asked again: the task would stall.
                Err(e) if e.is_timeout() => {
                    return Err(anyhow!("coder {}: no answer in 120 s", ep.model));
                }
                Err(e) if e.is_connect() || e.is_request() || e.is_body() => {
                    anyhow!("coder {}: {}", ep.model, e.without_url())
                }
                Err(e) => return Err(anyhow!("coder {}: {}", ep.model, e.without_url())),
            };
            if attempt == RETRIES {
                return Err(retry);
            }
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
            attempt += 1;
        };
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("coder {}: unreadable answer: {}", ep.model, short(&text)))?;
        match ep.provider.protocol {
            Protocol::OpenAi => openai_reply(&ep.model, &v),
            Protocol::Anthropic => anthropic_reply(&ep.model, &v),
        }
    }
}

fn openai_body(
    ep: &Endpoint,
    session: Option<&str>,
    system: &str,
    msgs: &[Msg],
    tools: &[Tool],
) -> Value {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for m in msgs {
        match m {
            Msg::User(t) => messages.push(json!({"role": "user", "content": t})),
            Msg::Assistant { text, calls } => {
                let mut a = json!({"role": "assistant", "content": text});
                if !calls.is_empty() {
                    a["tool_calls"] = calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {"name": c.name, "arguments": c.input.to_string()},
                            })
                        })
                        .collect();
                }
                messages.push(a);
            }
            Msg::Results(rs) => {
                for (id, content) in rs {
                    messages.push(json!({"role": "tool", "tool_call_id": id, "content": content}));
                }
            }
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name, "description": t.description, "parameters": t.parameters,
            }})
        })
        .collect();
    let mut body = json!({
        "model": ep.model.id,
        "messages": messages,
        "tools": tools,
        "tool_choice": "required",
    });
    if ep.provider.name == provider::OPENROUTER {
        body["usage"] = json!({"include": true});
        if let Some(s) = session {
            body["session_id"] = json!(s);
        }
    }
    body
}

fn openai_reply(model: &ModelRef, v: &Value) -> Result<Reply> {
    let msg = v.pointer("/choices/0/message").ok_or_else(|| {
        anyhow!(
            "coder {model}: no message in the answer: {}",
            short(&v.to_string())
        )
    })?;
    let text = msg["content"].as_str().unwrap_or_default().to_string();
    let calls = msg["tool_calls"]
        .as_array()
        .map(|calls| {
            calls
                .iter()
                .map(|c| Call {
                    id: c["id"].as_str().unwrap_or_default().to_string(),
                    name: c["function"]["name"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    input: c["function"]["arguments"]
                        .as_str()
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or(Value::Null),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Reply {
        text,
        calls,
        cost_usd: v["usage"]["cost"].as_f64().unwrap_or(0.0),
        input_tokens: v["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        cached_tokens: v
            .pointer("/usage/prompt_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: v["usage"]["completion_tokens"].as_u64().unwrap_or(0),
    })
}

fn anthropic_body(ep: &Endpoint, system: &str, msgs: &[Msg], tools: &[Tool]) -> Value {
    // Anthropic wants alternating roles: adjacent user turns become one.
    let mut messages: Vec<Value> = Vec::new();
    let mut push = |role: &str, blocks: Vec<Value>| match messages.last_mut() {
        Some(last) if last["role"] == role => {
            if let Some(arr) = last["content"].as_array_mut() {
                arr.extend(blocks);
            }
        }
        _ => messages.push(json!({"role": role, "content": blocks})),
    };
    for m in msgs {
        match m {
            Msg::User(t) => push("user", vec![json!({"type": "text", "text": t})]),
            Msg::Assistant { text, calls } => {
                let mut blocks = Vec::new();
                if !text.is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                for c in calls {
                    let input = if c.input.is_object() { c.input.clone() } else { json!({}) };
                    blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": input}));
                }
                push("assistant", blocks);
            }
            Msg::Results(rs) => push(
                "user",
                rs.iter()
                    .map(|(id, content)| {
                        json!({"type": "tool_result", "tool_use_id": id, "content": content})
                    })
                    .collect(),
            ),
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(
            |t| json!({"name": t.name, "description": t.description, "input_schema": t.parameters}),
        )
        .collect();
    json!({
        "model": ep.model.id,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": messages,
        "tools": tools,
        "tool_choice": {"type": "any"},
    })
}

fn anthropic_reply(model: &ModelRef, v: &Value) -> Result<Reply> {
    let blocks = v["content"].as_array().ok_or_else(|| {
        anyhow!(
            "coder {model}: no content in the answer: {}",
            short(&v.to_string())
        )
    })?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for b in blocks {
        match b["type"].as_str() {
            Some("text") => text.push_str(b["text"].as_str().unwrap_or_default()),
            Some("tool_use") => calls.push(Call {
                id: b["id"].as_str().unwrap_or_default().to_string(),
                name: b["name"].as_str().unwrap_or_default().to_string(),
                input: b["input"].clone(),
            }),
            _ => {}
        }
    }
    // Anthropic counts cache reads and writes apart from `input_tokens`.
    let u = &v["usage"];
    let count = |k: &str| u[k].as_u64().unwrap_or(0);
    let cached = count("cache_read_input_tokens");
    Ok(Reply {
        text,
        calls,
        cost_usd: u["cost"].as_f64().unwrap_or(0.0),
        input_tokens: count("input_tokens") + cached + count("cache_creation_input_tokens"),
        cached_tokens: cached,
        output_tokens: count("output_tokens"),
    })
}

fn short(text: &str) -> String {
    let t = text.trim();
    match t.char_indices().nth(300) {
        Some((at, _)) => format!("{}…", &t[..at]),
        None => t.to_string(),
    }
}
