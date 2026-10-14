//! Webhook: the Message as JSON, POSTed to the target; the secret, when set, as Bearer.

use anyhow::{Result, anyhow};

use super::{Channel, Message, client, http};

pub(super) async fn send(ch: &Channel, msg: &Message) -> Result<()> {
    let url = ch
        .target
        .as_deref()
        .ok_or_else(|| anyhow!("no URL; add the channel again"))?;
    let mut req = client().post(url).json(msg);
    if let Some(token) = &ch.secret {
        req = req.bearer_auth(token);
    }
    http("webhook", req).await?;
    Ok(())
}
