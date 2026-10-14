//! Slack: an incoming webhook (URL = secret) gets `{text}`.

use anyhow::{Result, anyhow};
use serde_json::json;

use super::{Channel, Message, client, http, plain};

pub(super) async fn send(ch: &Channel, msg: &Message) -> Result<()> {
    let url = ch
        .secret
        .as_deref()
        .ok_or_else(|| anyhow!("no webhook URL; add the channel again"))?;
    // Slack reads `<…>` as a link and `&` as an entity: escape all three.
    let text = plain(msg, false)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    http("slack", client().post(url).json(&json!({ "text": text }))).await?;
    Ok(())
}
