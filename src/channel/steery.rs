//! Steery: an ask is a request with one `buttons` control named `answer`; the answer is
//! accepted only with a valid signature from a pinned phone or mac signer.
//!
//! The signed message (`steery.response.v3`) binds the request content and the answer:
//! `content_sha256` is taken over what the server returned, after checking that title,
//! body, payload and buttons are what we sent.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use tokio::task::JoinSet;
use uuid::Uuid;

use super::{Answer, Channel, Kind, Message, Pin, Sent, client, http_json};

/// Read requests this long, so `poll` returns within about a minute.
const WAIT_SECS: u64 = 50;
const CONTROL: &str = "answer";
/// The longest fact value a field shows.
const FACT_VALUE: usize = 500;

/// What we raised, kept to check the server's copy against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Raised {
    pub id: String,
    pub workspace: String,
    pub title: String,
    pub body: String,
    pub payload: Value,
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub id: String,
    pub stance: String,
}

fn base() -> String {
    std::env::var("STEERY_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "https://steery.dev".into())
        .trim_end_matches('/')
        .to_owned()
}

fn key(ch: &Channel) -> Result<&str> {
    ch.secret
        .as_deref()
        .ok_or_else(|| anyhow!("no Steery key; add the channel again"))
}

fn workspace(ch: &Channel) -> Result<&str> {
    ch.target
        .as_deref()
        .ok_or_else(|| anyhow!("no Steery workspace; add the channel again"))
}

fn stance(word: &str) -> &'static str {
    match word {
        "no" | "never" => "rejection",
        _ => "neutral",
    }
}

/// A button word's label when the message carries none: info's `seen`, and older asks.
fn label(msg: &Message, word: &str) -> String {
    if let Some(l) = msg.ask.as_ref().and_then(|a| a.labels.get(word)) {
        return l.clone();
    }
    let mut c = word.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// One line of a bullet: Markdown has no line break inside it.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The body is a weave Steery checks for shape even unjudged: each gist a sentence of at
/// most 20 words. The summary's lines, then one that names the service and the box.
fn body(msg: &Message) -> String {
    let mut out = String::new();
    for s in &msg.summary {
        out.push_str(&format!("- {}\n", one_line(s)));
    }
    let what = match msg.kind {
        Kind::Ask => "needs your answer",
        Kind::Warning => "has a problem that Subweave is fixing",
        Kind::Info => "has news",
    };
    out.push_str(&format!("- **{}** on {} {what}.", msg.service, msg.host));
    out
}

/// Under the body: the card's lines, then the facts as fields.
fn view(msg: &Message) -> Value {
    let mut view = Vec::new();
    let lines: String = msg
        .text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("- {}\n", one_line(l)))
        .collect();
    if !lines.is_empty() {
        view.push(json!({"kind": "markdown", "text": lines.trim_end()}));
    }
    if !msg.facts.is_empty() {
        let fields: Vec<Value> = msg
            .facts
            .iter()
            .map(|(k, v)| {
                let v = match v.char_indices().nth(FACT_VALUE) {
                    Some((i, _)) => format!("{}…", &v[..i]),
                    None => v.clone(),
                };
                json!({"label": k, "value": v})
            })
            .collect();
        view.push(json!({"kind": "fields", "fields": fields}));
    }
    Value::Array(view)
}

pub(super) async fn send(ch: &Channel, msg: &Message) -> Result<Option<Sent>> {
    let key = key(ch)?;
    let workspace = workspace(ch)?;
    let id = Uuid::now_v7().to_string();
    let mut payload = json!({
        "service": msg.service,
        "host": msg.host,
        "facts": msg.facts,
    });
    if let Some(goal) = &msg.goal {
        payload["goal"] = json!(goal);
    }
    if let Some(ask) = &msg.ask {
        payload["n"] = json!(ask.n);
    }
    let words: Vec<&str> = match &msg.ask {
        Some(ask) => ask.answers.iter().map(String::as_str).collect(),
        None => vec!["seen"],
    };
    let options: Vec<Value> = words
        .iter()
        .map(|w| json!({"id": w, "label": label(msg, w), "stance": stance(w)}))
        .collect();
    let raised = Raised {
        id: id.clone(),
        workspace: workspace.to_owned(),
        title: msg.title.clone(),
        body: body(msg),
        payload,
        buttons: words
            .iter()
            .map(|w| Button {
                id: (*w).to_owned(),
                stance: stance(w).to_owned(),
            })
            .collect(),
    };
    // The daemon's code builds this form, not a model: Steery's card judge, which holds
    // an agent's words to its guidelines, does not apply to it.
    let request = json!({
        "id": id,
        "title": raised.title,
        "body": raised.body,
        "payload": raised.payload,
        "view": view(msg),
        "input": [{"kind": "buttons", "name": CONTROL, "options": options}],
        "unjudged": true,
    });
    let _: Value = http_json(
        "steery raise",
        client()
            .post(format!("{}/v1/requests", base()))
            .bearer_auth(key)
            .header("Steery-Workspace", workspace)
            .json(&request),
    )
    .await?;
    if msg.ask.is_none() {
        return Ok(None);
    }
    Ok(Some(Sent(serde_json::to_value(raised)?)))
}

/// Long-poll every open request at once. Each read also keeps its request alive (a
/// request the raiser has not read for 15 minutes is withdrawn) and reopens one
/// withdrawn for not being read. An empty or failed round still takes about
/// `WAIT_SECS`, so a caller looping on `poll` does not spin.
pub(super) async fn poll(ch: &Channel, open: &[(u64, Sent)]) -> Result<Vec<Answer>> {
    let started = Instant::now();
    let key: Arc<str> = key(ch)?.into();
    let pins: Arc<[Pin]> = ch.pins.clone().into();
    let mut reads = JoinSet::new();
    for (n, sent) in open {
        let n = *n;
        let raised: Raised = serde_json::from_value(sent.0.clone())
            .map_err(|e| anyhow!("ask #{n}: not a Steery request: {e}"))?;
        let key = key.clone();
        let pins = pins.clone();
        reads.spawn(async move { (n, read(&key, &raised, &pins).await) });
    }
    let mut answers = Vec::new();
    let mut errors = Vec::new();
    while let Some(joined) = reads.join_next().await {
        match joined {
            Ok((n, Ok(Some((word, by))))) => answers.push(Answer { n, word, by }),
            Ok((_, Ok(None))) => {}
            Ok((n, Err(e))) => errors.push(format!("ask #{n}: {e:#}")),
            Err(e) => errors.push(e.to_string()),
        }
    }
    if answers.is_empty() {
        let floor = Duration::from_secs(WAIT_SECS);
        if let Some(rest) = floor.checked_sub(started.elapsed()) {
            tokio::time::sleep(rest).await;
        }
        if !errors.is_empty() {
            bail!("{}", errors.join("; "));
        }
    }
    Ok(answers)
}

/// `Some((word, by))` once the request holds a verified answer.
async fn read(key: &str, raised: &Raised, pins: &[Pin]) -> Result<Option<(String, String)>> {
    let request: Value = http_json(
        "steery read",
        client()
            .get(format!(
                "{}/v1/requests/{}?wait_secs={WAIT_SECS}",
                base(),
                raised.id
            ))
            .bearer_auth(key)
            .header("Steery-Workspace", &raised.workspace),
    )
    .await?;
    match request["status"].as_str() {
        Some("open") => Ok(None),
        Some("responded") => verify(raised, &request, pins).map(Some),
        Some("withdrawn") => match request["reason"].as_str() {
            // This read reopened it.
            Some("session_did_not_poll") => Ok(None),
            reason => bail!("withdrawn in Steery ({})", reason.unwrap_or("no reason")),
        },
        other => bail!("unknown request status {other:?}"),
    }
}

/// Check a responded request against what we raised and the pinned signers; the
/// answer word and who signed it.
pub fn verify(raised: &Raised, request: &Value, pins: &[Pin]) -> Result<(String, String)> {
    let id = uuid(&request["id"], "id")?;
    if id != uuid(&json!(raised.id), "raised id")? {
        bail!("the server returned request {id}, not {}", raised.id);
    }
    let workspace = uuid(&json!(raised.workspace), "workspace")?;
    if uuid(&request["workspace"], "workspace")? != workspace {
        bail!("the request is in another workspace");
    }
    if request["title"] != json!(raised.title) {
        bail!("the title differs from the one sent");
    }
    if request["body"] != json!(raised.body) {
        bail!("the body differs from the one sent");
    }
    if request.get("payload") != Some(&raised.payload) {
        bail!("the payload differs from the one sent");
    }
    if buttons(request)? != raised.buttons {
        bail!("the buttons differ from the ones sent");
    }
    let content = content_sha256(request)?;

    let response = &request["response"];
    let answer = &response["answer"];
    let word = answer["values"][CONTROL]
        .as_str()
        .ok_or_else(|| anyhow!("the answer presses no button"))?;
    let button = raised
        .buttons
        .iter()
        .find(|b| b.id == word)
        .ok_or_else(|| anyhow!("the answer {word:?} is no button that was sent"))?;
    let signed_stance = text(&response["stance"], "stance")?;
    if signed_stance != button.stance {
        bail!("stance {signed_stance} does not match button {word}");
    }
    let signer = uuid(&response["signer"], "signer")?;
    let pin = pins
        .iter()
        .find(|p| uuid(&json!(p.id), "pin").is_ok_and(|id| id == signer))
        .ok_or_else(|| anyhow!("signed by signer {signer}, which is not pinned"))?;
    let message = message(
        &workspace,
        &id,
        &signer,
        &content,
        &answer_sha256(answer)?,
        signed_stance,
        text(&response["at"], "at")?,
    );
    verify_signature(
        &pin.public_key,
        message.as_bytes(),
        text(&response["signature"], "signature")?,
    )?;
    Ok((word.to_owned(), pin.name.clone()))
}

fn text<'a>(value: &'a Value, what: &str) -> Result<&'a str> {
    value
        .as_str()
        .ok_or_else(|| anyhow!("the response has no {what}"))
}

/// Lowercase hyphenated, as the signed message spells it.
fn uuid(value: &Value, what: &str) -> Result<String> {
    let s = text(value, what)?;
    Uuid::parse_str(s)
        .map(|u| u.to_string())
        .map_err(|_| anyhow!("{what} {s:?} is not a UUID"))
}

/// The buttons of the `answer` control as the server holds them.
fn buttons(request: &Value) -> Result<Vec<Button>> {
    let control = request["input"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["kind"] == "buttons" && c["name"] == CONTROL)
        .ok_or_else(|| anyhow!("the request has no {CONTROL} buttons"))?;
    control["options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|o| {
            Ok(Button {
                id: text(&o["id"], "button id")?.to_owned(),
                stance: text(&o["stance"], "button stance")?.to_owned(),
            })
        })
        .collect()
}

/// Hex SHA-256 of the canonical `{title, body, payload?, view, links?, input}` of a
/// request as the server returned it: `payload` omitted when absent or null, `links`
/// when empty.
pub fn content_sha256(request: &Value) -> Result<String> {
    let mut content = Map::new();
    for field in ["title", "body"] {
        let value = &request[field];
        if !value.is_string() {
            bail!("the request has no {field}");
        }
        content.insert(field.into(), value.clone());
    }
    if let Some(payload) = request.get("payload").filter(|p| !p.is_null()) {
        content.insert("payload".into(), payload.clone());
    }
    content.insert(
        "view".into(),
        request.get("view").cloned().unwrap_or_else(|| json!([])),
    );
    if let Some(links) = request["links"].as_array().filter(|l| !l.is_empty()) {
        content.insert("links".into(), Value::Array(links.clone()));
    }
    let input = request
        .get("input")
        .filter(|i| i.is_array())
        .ok_or_else(|| anyhow!("the request has no input"))?;
    content.insert("input".into(), input.clone());
    Ok(sha256_hex(jcs(&Value::Object(content))?.as_bytes()))
}

/// Hex SHA-256 of the canonical `{values, payload?, files?}` of an answer as received:
/// `payload` omitted when absent or null, `files` when empty.
pub fn answer_sha256(answer: &Value) -> Result<String> {
    let mut canon = Map::new();
    let values = answer
        .get("values")
        .filter(|v| v.is_object())
        .ok_or_else(|| anyhow!("the answer has no values"))?;
    canon.insert("values".into(), values.clone());
    if let Some(payload) = answer.get("payload").filter(|p| !p.is_null()) {
        canon.insert("payload".into(), payload.clone());
    }
    if let Some(files) = answer["files"].as_array().filter(|f| !f.is_empty()) {
        canon.insert("files".into(), Value::Array(files.clone()));
    }
    Ok(sha256_hex(jcs(&Value::Object(canon))?.as_bytes()))
}

/// The eight lines a signer signs, trailing newline included. UUIDs lowercase, `at`
/// exactly as the response spells it.
pub fn message(
    workspace: &str,
    request: &str,
    signer: &str,
    content_sha256: &str,
    answer_sha256: &str,
    stance: &str,
    at: &str,
) -> String {
    format!(
        "steery.response.v3\n{workspace}\n{request}\n{signer}\n{content_sha256}\n{answer_sha256}\n{stance}\n{at}\n"
    )
}

/// Check a DER signature (base64url, unpadded) over `message` under a pinned key.
pub fn verify_signature(public_key: &str, message: &[u8], signature: &str) -> Result<()> {
    let (key, _) = parse_public_key(public_key)?;
    let der = URL_SAFE_NO_PAD
        .decode(signature.as_bytes())
        .map_err(|_| anyhow!("the signature is not base64url"))?;
    let signature = Signature::from_der(&der).map_err(|_| anyhow!("the signature is not DER"))?;
    key.verify(message, &signature)
        .map_err(|_| anyhow!("the signature does not verify"))
}

/// A 65-byte uncompressed SEC1 P-256 point, base64url without padding: the only
/// spelling accepted, so one key has one fingerprint.
pub fn parse_public_key(encoded: &str) -> Result<(VerifyingKey, [u8; 65])> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|_| anyhow!("public key is not unpadded base64url"))?;
    let bytes: [u8; 65] = decoded
        .try_into()
        .map_err(|_| anyhow!("public key is not 65 bytes"))?;
    if bytes[0] != 0x04 {
        bail!("public key is not an uncompressed point");
    }
    let key =
        VerifyingKey::from_sec1_bytes(&bytes).map_err(|_| anyhow!("public key is not on P-256"))?;
    Ok((key, bytes))
}

/// The first 16 hex characters of sha256 of the key bytes.
pub fn fingerprint(key_bytes: &[u8]) -> String {
    let mut hex = sha256_hex(key_bytes);
    hex.truncate(16);
    hex
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Read the workspace and signers with `key`, print every signer that is not revoked,
/// and pin the phone and mac ones (telegram and slack keys are held by the server).
pub(super) async fn enroll(key: &str) -> Result<(String, Vec<Pin>)> {
    let me: Value = http_json(
        "steery /v1/me",
        client().get(format!("{}/v1/me", base())).bearer_auth(key),
    )
    .await?;
    let workspace = uuid(&me["workspace"]["id"], "workspace id")?;
    let signers: Vec<Value> = http_json(
        "steery /v1/signers",
        client()
            .get(format!("{}/v1/signers", base()))
            .bearer_auth(key)
            .header("Steery-Workspace", &workspace),
    )
    .await?;
    let name = me["workspace"]["name"].as_str().unwrap_or("");
    println!("workspace {workspace} {name}");
    let mut pins = Vec::new();
    for signer in &signers {
        if !signer["revoked_at"].is_null() {
            continue;
        }
        let kind = text(&signer["kind"], "signer kind")?;
        let name = signer["name"].as_str().unwrap_or("");
        let public_key = text(&signer["public_key"], "signer public key")?;
        let (_, bytes) = parse_public_key(public_key)?;
        let print = fingerprint(&bytes);
        if signer["fingerprint"].as_str().is_some_and(|f| f != print) {
            bail!("signer {name}: the server's fingerprint does not match its key");
        }
        let pinned = matches!(kind, "phone" | "mac");
        println!(
            "{kind:<9} {print}  {name}{}",
            if pinned { "  pinned" } else { "" }
        );
        if pinned {
            pins.push(Pin {
                id: uuid(&signer["id"], "signer id")?,
                kind: kind.to_owned(),
                name: name.to_owned(),
                public_key: public_key.to_owned(),
                fingerprint: print,
            });
        }
    }
    if pins.is_empty() {
        bail!("no phone or mac signer in workspace {workspace}: install the Steery app first");
    }
    Ok((workspace, pins))
}

/// RFC 8785 JSON Canonicalization Scheme.
pub fn jcs(value: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(&mut out, value)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            let v = n
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| anyhow!("number {n} has no JSON form"))?;
            write_double(out, v);
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(members) => {
            // RFC 8785 orders members by UTF-16 code units, which differs from Rust's
            // code-point order for astral characters against U+E000..U+FFFF.
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort_unstable_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, &members[key])?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// ECMAScript `Number::toString` over the shortest round-tripping decimal.
fn write_double(out: &mut String, value: f64) {
    if value == 0.0 {
        // -0 prints as 0.
        out.push('0');
        return;
    }
    if value < 0.0 {
        out.push('-');
    }
    let mut buffer = ryu::Buffer::new();
    let printed = buffer.format_finite(value.abs());
    let (mantissa, exponent) = match printed.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i32>().unwrap_or(0)),
        None => (printed, 0),
    };
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = String::with_capacity(int_part.len() + frac_part.len());
    digits.push_str(int_part);
    digits.push_str(frac_part);
    // The value is 0.digits × 10^n.
    let mut n = int_part.len() as i32 + exponent;
    let trimmed = digits.trim_start_matches('0');
    n -= (digits.len() - trimmed.len()) as i32;
    let digits = trimmed.trim_end_matches('0');
    let k = digits.len() as i32;

    if k <= n && n <= 21 {
        out.push_str(digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..-n {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = n - 1;
        let _ = write!(out, "e{}{}", if e < 0 { '-' } else { '+' }, e.abs());
    }
}
