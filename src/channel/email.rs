//! Email: SMTP URL = secret (`smtps://user:pass@host:465`), to = target. The sender is
//! the SMTP user when it is an address, else `subweave@<smtp host>`.

use anyhow::{Result, anyhow, bail};
use lettre::message::header::ContentType;
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Tokio1Executor};

use super::{Channel, Message, details, headline};

/// Refuse a URL lettre cannot use. Its errors never carry the password.
pub(super) fn check_url(url: &str) -> Result<()> {
    AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
        .map(|_| ())
        .map_err(|e| anyhow!("SMTP URL: {e}"))?;
    sender(url).map(|_| ())
}

pub(super) async fn send(ch: &Channel, msg: &Message) -> Result<()> {
    let url = ch
        .secret
        .as_deref()
        .ok_or_else(|| anyhow!("no SMTP URL; add the channel again"))?;
    let to = ch
        .target
        .as_deref()
        .ok_or_else(|| anyhow!("no address; add the channel again"))?;
    let mail = lettre::Message::builder()
        .from(
            sender(url)?
                .parse()
                .map_err(|e| anyhow!("sender address: {e}"))?,
        )
        .to(to.parse().map_err(|e| anyhow!("address {to:?}: {e}"))?)
        .subject(headline(msg))
        .header(ContentType::TEXT_PLAIN)
        .body(details(msg, false))
        .map_err(|e| anyhow!("email: {e}"))?;
    let mailer = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
        .map_err(|e| anyhow!("SMTP URL: {e}"))?
        .build();
    mailer.send(mail).await.map_err(|e| anyhow!("SMTP: {e}"))?;
    Ok(())
}

/// `user%40acme.dev` in `smtps://user%40acme.dev:pass@host` gives `user@acme.dev`.
fn sender(url: &str) -> Result<String> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| anyhow!("SMTP URL has no scheme"))?;
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let (userinfo, hostport) = authority.rsplit_once('@').unwrap_or(("", authority));
    let user = percent_decode(userinfo.split(':').next().unwrap_or(""));
    if user.contains('@') {
        return Ok(user);
    }
    let host = match hostport.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(v6),
        None => hostport.split(':').next().unwrap_or(hostport),
    };
    if host.is_empty() {
        bail!("SMTP URL has no host");
    }
    Ok(format!("subweave@{host}"))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (bytes[i], bytes.get(i + 1), bytes.get(i + 2)) {
            (b'%', Some(&h), Some(&l)) if hex(h).is_some() && hex(l).is_some() => {
                out.push((hex(h).unwrap_or(0) * 16 + hex(l).unwrap_or(0)) as u8);
                i += 3;
            }
            (b, _, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
