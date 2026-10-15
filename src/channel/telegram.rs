//! Telegram: the person's own bot (token = secret) writes to one chat (target). An ask
//! carries an inline keyboard whose buttons send `<n>:<word>` back as callback data.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Answer, Channel, Message, Sent, client, http_json, plain, write_private};

const API: &str = "https://api.telegram.org";
const WAIT_SECS: u64 = 50;

#[derive(Serialize, Deserialize)]
struct Asked {
    chat: i64,
    message: i64,
    answers: Vec<String>,
}

#[derive(Deserialize)]
struct Reply<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct Posted {
    message_id: i64,
    chat: Chat,
}

#[derive(Deserialize)]
struct Chat {
    id: i64,
}

#[derive(Deserialize)]
struct Update {
    update_id: i64,
    callback_query: Option<Callback>,
}

#[derive(Deserialize)]
struct Callback {
    id: String,
    from: User,
    message: Option<Posted>,
    data: Option<String>,
}

#[derive(Deserialize)]
struct User {
    first_name: String,
    username: Option<String>,
}

fn token(ch: &Channel) -> Result<&str> {
    ch.secret
        .as_deref()
        .ok_or_else(|| anyhow!("no bot token; add the channel again"))
}

/// Call a Bot API method. The token is in the URL, so only the method name reaches
/// an error.
async fn call<T: serde::de::DeserializeOwned>(
    token: &str,
    method: &str,
    body: &Value,
) -> Result<T> {
    let reply: Reply<T> = http_json(
        &format!("telegram {method}"),
        client()
            .post(format!("{API}/bot{token}/{method}"))
            .json(body),
    )
    .await?;
    match (reply.ok, reply.result) {
        (true, Some(result)) => Ok(result),
        _ => bail!(
            "telegram {method}: {}",
            reply.description.unwrap_or_else(|| "not ok".into())
        ),
    }
}

pub(super) async fn send(ch: &Channel, msg: &Message) -> Result<Option<Sent>> {
    let token = token(ch)?;
    let chat = ch
        .target
        .as_deref()
        .ok_or_else(|| anyhow!("no chat id; add the channel again"))?;
    let mut body = json!({
        "chat_id": chat,
        "text": plain(msg, true),
        "link_preview_options": {"is_disabled": true},
    });
    if let Some(ask) = &msg.ask {
        let buttons = ask
            .answers
            .iter()
            .map(|word| {
                let data = format!("{}:{word}", ask.n);
                // Telegram's limit on callback data.
                if data.len() > 64 {
                    bail!("answer {word:?} is too long for a Telegram button");
                }
                Ok(json!([{"text": ask.label(word), "callback_data": data}]))
            })
            .collect::<Result<Vec<_>>>()?;
        // One button per row: labels are short sentences.
        body["reply_markup"] = json!({"inline_keyboard": buttons});
    }
    let posted: Posted = call(token, "sendMessage", &body).await?;
    let Some(ask) = &msg.ask else {
        return Ok(None);
    };
    let asked = Asked {
        chat: posted.chat.id,
        message: posted.message_id,
        answers: ask.answers.clone(),
    };
    Ok(Some(Sent(serde_json::to_value(asked)?)))
}

/// Takes the buttons off an ask's message.
pub(super) async fn close(ch: &Channel, sent: &Sent) -> Result<()> {
    let asked: Asked = serde_json::from_value(sent.0.clone())
        .map_err(|e| anyhow!("not a Telegram message: {e}"))?;
    call::<Value>(
        token(ch)?,
        "editMessageReplyMarkup",
        &json!({
            "chat_id": asked.chat,
            "message_id": asked.message,
            "reply_markup": {"inline_keyboard": []},
        }),
    )
    .await?;
    Ok(())
}

/// One `getUpdates` long poll. A button press on an open ask's own message becomes an
/// answer; every press is acknowledged so the button stops spinning.
pub(super) async fn poll(ch: &Channel, open: &[(u64, Sent)]) -> Result<Vec<Answer>> {
    let token = token(ch)?;
    let asked: Vec<(u64, Asked)> = open
        .iter()
        .map(|(n, sent)| {
            serde_json::from_value(sent.0.clone())
                .map(|a| (*n, a))
                .map_err(|e| anyhow!("ask #{n}: not a Telegram message: {e}"))
        })
        .collect::<Result<_>>()?;
    let mut offsets = load_offsets()?;
    let offset = offsets.get(&ch.name).copied().unwrap_or(0);
    let updates: Vec<Update> = call(
        token,
        "getUpdates",
        &json!({"offset": offset, "timeout": WAIT_SECS, "allowed_updates": ["callback_query"]}),
    )
    .await?;
    let Some(last) = updates.iter().map(|u| u.update_id).max() else {
        return Ok(Vec::new());
    };
    offsets.insert(ch.name.clone(), last + 1);
    save_offsets(&offsets)?;

    let mut answers = Vec::new();
    for callback in updates.into_iter().filter_map(|u| u.callback_query) {
        let found = callback.data.as_deref().and_then(|data| {
            let (n, word) = data.split_once(':')?;
            let n: u64 = n.parse().ok()?;
            let posted = callback.message.as_ref()?;
            asked
                .iter()
                .find(|(open_n, a)| {
                    *open_n == n
                        && a.chat == posted.chat.id
                        && a.message == posted.message_id
                        && a.answers.iter().any(|w| w == word)
                })
                .map(|_| (n, word.to_owned()))
        });
        let note = match &found {
            Some((_, word)) => format!("Answered {word}"),
            None => "This ask is closed".to_owned(),
        };
        // The offset is saved: a failed acknowledgement must not lose the answer.
        let _ = call::<Value>(
            token,
            "answerCallbackQuery",
            &json!({"callback_query_id": callback.id, "text": note}),
        )
        .await;
        if let Some((n, word)) = found {
            let by = match callback.from.username {
                Some(username) => format!("@{username}"),
                None => callback.from.first_name,
            };
            answers.push(Answer { n, word, by });
        }
    }
    Ok(answers)
}

/// `config()/telegram.offset`: the next update id per Telegram channel name.
fn offset_file() -> PathBuf {
    crate::paths::config().join("telegram.offset")
}

fn load_offsets() -> Result<BTreeMap<String, i64>> {
    let path = offset_file();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| anyhow!("{} is not valid: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(anyhow!("read {}: {e}", path.display())),
    }
}

fn save_offsets(offsets: &BTreeMap<String, i64>) -> Result<()> {
    write_private(&offset_file(), &serde_json::to_vec(offsets)?)
}
