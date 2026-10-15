//! Slack: with a channel id as target, a bot token (secret) posts with `chat.postMessage`;
//! without one, an incoming webhook (URL = secret) gets `{text}`.

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde_json::json;

use super::{Channel, Message, client, http, http_json, plain};

#[derive(Deserialize)]
struct Posted {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
}

pub(super) async fn send(ch: &Channel, msg: &Message) -> Result<()> {
    let secret = ch
        .secret
        .as_deref()
        .ok_or_else(|| anyhow!("no Slack token or webhook URL; add the channel again"))?;
    // Slack reads `<…>` as a link and `&` as an entity: escape all three.
    let text = plain(msg, false)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let Some(channel) = ch.target.as_deref() else {
        http("slack", client().post(secret).json(&json!({ "text": text }))).await?;
        return Ok(());
    };
    // The Web API answers 200 with `ok: false` on a refusal.
    let posted: Posted = http_json(
        "slack chat.postMessage",
        client()
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(secret)
            .json(&json!({ "channel": channel, "text": text, "unfurl_links": false })),
    )
    .await?;
    if !posted.ok {
        bail!(
            "slack chat.postMessage: {}",
            posted.error.as_deref().unwrap_or("not ok")
        );
    }
    Ok(())
}
