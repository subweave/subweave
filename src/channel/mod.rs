//! Where the daemon's messages go: Steery, Telegram, Slack, email, a webhook, a heartbeat.
//! Channels live in `config()/channels.json` (mode 0600). The answering channels are the
//! escalation targets: an ask the ladder could not settle goes to them.

pub mod card;
pub mod email;
pub mod escalation;
pub mod heartbeat;
pub mod slack;
pub mod steery;
pub mod telegram;
pub mod webhook;

use std::fmt;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

pub use card::Card;

/// What a message is. Order matters: a channel gets messages at or above its `level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Ask,
    Warning,
    Info,
}

impl Kind {
    fn rank(self) -> u8 {
        match self {
            Kind::Info => 0,
            Kind::Warning => 1,
            Kind::Ask => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ask => "ask",
            Kind::Warning => "warning",
            Kind::Info => "info",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "ask" => Ok(Kind::Ask),
            "warning" => Ok(Kind::Warning),
            "info" => Ok(Kind::Info),
            _ => bail!("level is ask, warning or info, not {s:?}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub kind: Kind,
    pub host: String,
    pub service: String,
    /// The goal the message is about, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    pub card: Card,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<AskInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskInfo {
    pub n: u64,
    /// Only the person's signed answer on a Steery card settles it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub signed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Steery,
    Telegram,
    Slack,
    Email,
    Webhook,
    Heartbeat,
    Agent,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Steery => "steery",
            ChannelKind::Telegram => "telegram",
            ChannelKind::Slack => "slack",
            ChannelKind::Email => "email",
            ChannelKind::Webhook => "webhook",
            ChannelKind::Heartbeat => "heartbeat",
            ChannelKind::Agent => "agent",
        }
    }

    /// Whether the channel is an escalation target: it settles asks, by `poll` or
    /// through the inbox (an agent runs `subweave answer`).
    pub fn answers(self) -> bool {
        matches!(
            self,
            ChannelKind::Steery | ChannelKind::Telegram | ChannelKind::Agent
        )
    }
}

impl fmt::Display for ChannelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChannelKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "steery" => Ok(ChannelKind::Steery),
            "telegram" => Ok(ChannelKind::Telegram),
            "slack" => Ok(ChannelKind::Slack),
            "email" => Ok(ChannelKind::Email),
            "webhook" => Ok(ChannelKind::Webhook),
            "heartbeat" => Ok(ChannelKind::Heartbeat),
            "agent" => Ok(ChannelKind::Agent),
            _ => bail!(
                "channel kind is steery, telegram, slack, email, webhook, heartbeat or agent, not {s:?}"
            ),
        }
    }
}

/// One entry of `channels.json`.
///
/// `target`: steery the workspace id, telegram the chat id, slack the channel id (bot
/// token) or none (webhook), email the address, webhook and heartbeat the URL. `secret`:
/// steery the key, telegram and slack the bot token, a slack incoming-webhook URL, email
/// the SMTP URL, webhook an optional bearer token.
#[derive(Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    #[serde(flatten)]
    pub via: Via,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub level: Kind,
}

impl Channel {
    pub fn kind(&self) -> ChannelKind {
        self.via.kind()
    }
}

// The secret never reaches a log line through `{:?}`.
impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("name", &self.name)
            .field("via", &self.via)
            .field("target", &self.target)
            .field("secret", &self.secret.as_ref().map(|_| "[secret]"))
            .field("level", &self.level)
            .finish()
    }
}

/// A channel's kind, with what only that kind keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Via {
    Steery {
        escalate: Escalate,
        /// Signers whose signatures are accepted.
        pins: Vec<Pin>,
    },
    Telegram,
    Slack,
    Email,
    Webhook,
    Heartbeat,
    /// A command on this box, run once per escalation (`target`).
    Agent,
}

impl Via {
    pub fn kind(&self) -> ChannelKind {
        match self {
            Via::Steery { .. } => ChannelKind::Steery,
            Via::Telegram => ChannelKind::Telegram,
            Via::Slack => ChannelKind::Slack,
            Via::Email => ChannelKind::Email,
            Via::Webhook => ChannelKind::Webhook,
            Via::Heartbeat => ChannelKind::Heartbeat,
            Via::Agent => ChannelKind::Agent,
        }
    }
}

/// How a Steery channel escalates an ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Escalate {
    /// A throw to the service's agent conversation, connected once by the person.
    /// `reach`: how that agent runs `subweave` on this box, `ssh <reach> sudo -n subweave`.
    Session { reach: String },
    /// A card with the answers as buttons, signed by the person.
    Card,
}

/// `--escalate`: the mode, before its `--reach` is joined to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Session,
    Card,
}

impl FromStr for Mode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "session" => Ok(Mode::Session),
            "card" => Ok(Mode::Card),
            _ => bail!("--escalate is session or card, not {s:?}"),
        }
    }
}

/// A Steery signer pinned at `channel add steery`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub id: String,
    pub kind: String,
    pub name: String,
    /// 65-byte uncompressed SEC1 point, base64url without padding.
    pub public_key: String,
    pub fingerprint: String,
}

/// What a channel needs to find the answer to an ask later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sent(pub serde_json::Value);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub n: u64,
    pub word: String,
    /// `<channel kind>:<who>`: `steery:Aram's iPhone`, `telegram:@aram`.
    pub by: String,
}

/// What one `poll` round brought: answers, and the asks this channel stops watching
/// with no answer to come on it.
#[derive(Debug, Default)]
pub struct Polled {
    pub answers: Vec<Answer>,
    pub done: Vec<u64>,
}

/// Whether the escalation a message carries still needs hearing: read before every
/// post, so nothing goes out once it is answered or withdrawn. A message with no ask
/// passes `&always`.
pub type Open<'a> = &'a (dyn Fn() -> bool + Send + Sync);

pub fn always() -> bool {
    true
}

/// Send `msg` if the channel's level lets it through. An ask always goes out: to an
/// answering channel with its answers, to the rest ending with the `subweave answer`
/// command. A heartbeat pings on every message, throttled to one a minute.
/// `Some(Sent)` only for an ask on an answering channel.
pub async fn send(ch: &Channel, msg: &Message, open: Open<'_>) -> Result<Option<Sent>> {
    if ch.kind() != ChannelKind::Heartbeat
        && msg.kind != Kind::Ask
        && msg.kind.rank() < ch.level.rank()
    {
        return Ok(None);
    }
    deliver(ch, msg, open).await
}

async fn deliver(ch: &Channel, msg: &Message, open: Open<'_>) -> Result<Option<Sent>> {
    if !open() {
        return Ok(None);
    }
    let sent = match ch.kind() {
        ChannelKind::Steery => steery::send(ch, msg, open).await,
        ChannelKind::Telegram => telegram::send(ch, msg).await,
        ChannelKind::Slack => slack::send(ch, msg).await.map(|()| None),
        ChannelKind::Email => email::send(ch, msg).await.map(|()| None),
        ChannelKind::Webhook => webhook::send(ch, msg).await.map(|()| None),
        ChannelKind::Heartbeat => heartbeat::send(ch).await.map(|()| None),
        // The daemon runs an agent's command on an escalation; the rest is not for it.
        ChannelKind::Agent => Ok(None),
    };
    sent.with_context(|| format!("channel {}", ch.name))
}

/// The answers to `open` asks that arrived, returning within about a minute. Each
/// answer's `by` names the channel's kind first.
pub async fn poll(ch: &Channel, open: &[(u64, Sent)]) -> Result<Polled> {
    let kind = ch.kind();
    let polled = match kind {
        ChannelKind::Steery => steery::poll(ch, open).await,
        ChannelKind::Telegram => telegram::poll(ch, open).await.map(|answers| Polled {
            answers,
            done: Vec::new(),
        }),
        other => bail!("{other} channels do not answer"),
    };
    let mut polled = polled.with_context(|| format!("channel {}", ch.name))?;
    for a in &mut polled.answers {
        a.by = format!("{kind}:{}", a.by);
    }
    Ok(polled)
}

/// Whether a channel keeps watching `sent` after its ask is settled: a Steery connect
/// throw, watched until the person's Go stores the service's conversation.
pub fn outlives_ask(sent: &Sent) -> bool {
    steery::is_connect(sent)
}

/// An ask is closed before anyone answered it: a channel that showed buttons takes
/// them away. Steery withdraws a request its raiser stops polling; the rest never had
/// buttons.
pub async fn close(ch: &Channel, sent: &Sent) -> Result<()> {
    match ch.kind() {
        ChannelKind::Telegram => telegram::close(ch, sent)
            .await
            .with_context(|| format!("channel {}", ch.name)),
        _ => Ok(()),
    }
}

/// Add a channel and save it. Steery reads the workspace and its signers with the key,
/// prints them, and pins the phone and mac signers. `mode` and `reach` are Steery's:
/// `session`, the default, needs `reach`.
pub async fn add(
    kind: ChannelKind,
    target: Option<String>,
    level: Option<Kind>,
    secret: Option<String>,
    mode: Option<Mode>,
    reach: Option<String>,
) -> Result<Channel> {
    let target = target
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());
    let secret = secret
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let reach = reach
        .map(|r| r.trim().to_owned())
        .filter(|r| !r.is_empty());
    if kind != ChannelKind::Steery && (mode.is_some() || reach.is_some()) {
        bail!("--escalate and --reach are for steery channels");
    }
    let need_target = |what: &str| -> Result<()> {
        if target.is_none() {
            bail!("{kind} needs {what}");
        }
        Ok(())
    };
    let need_secret = |what: &str| -> Result<()> {
        if secret.is_none() {
            bail!("{kind} needs {what} on stdin");
        }
        Ok(())
    };
    let need_url = |url: &str| -> Result<()> {
        if !url.starts_with("https://") && !url.starts_with("http://") {
            bail!("{kind} needs an http:// or https:// URL, not {url:?}");
        }
        Ok(())
    };
    let (via, target) = match kind {
        ChannelKind::Steery => {
            if target.is_some() {
                bail!("steery takes no target: the workspace comes from the key");
            }
            let escalate = match (mode.unwrap_or(Mode::Session), reach) {
                (Mode::Session, Some(reach)) => Escalate::Session { reach },
                (Mode::Session, None) => bail!(
                    "steery --escalate session needs --reach USER@HOST: how an agent elsewhere runs subweave on this box, as ssh USER@HOST sudo -n subweave"
                ),
                (Mode::Card, None) => Escalate::Card,
                (Mode::Card, Some(_)) => bail!("--reach is for --escalate session"),
            };
            need_secret("a Steery key")?;
            let (workspace, pins) = steery::enroll(secret.as_deref().unwrap_or_default()).await?;
            (Via::Steery { escalate, pins }, Some(workspace))
        }
        ChannelKind::Telegram => {
            need_target("the chat id")?;
            need_secret("the bot token")?;
            (Via::Telegram, target)
        }
        ChannelKind::Slack => {
            let secret = secret.as_deref().unwrap_or_default();
            match &target {
                Some(_) if !secret.starts_with("xoxb-") => {
                    bail!("slack with a channel id needs a bot token (xoxb-…) on stdin")
                }
                Some(_) => {}
                None => {
                    need_secret("an incoming-webhook URL, or a bot token with a channel id")?;
                    need_url(secret)?;
                }
            }
            (Via::Slack, target)
        }
        ChannelKind::Email => {
            need_target("the address to send to")?;
            need_secret("an SMTP URL such as smtps://user:pass@host:465")?;
            email::check_url(secret.as_deref().unwrap_or_default())?;
            (Via::Email, target)
        }
        ChannelKind::Webhook | ChannelKind::Heartbeat => {
            need_target("a URL")?;
            need_url(target.as_deref().unwrap_or_default())?;
            let via = match kind {
                ChannelKind::Webhook => Via::Webhook,
                _ => Via::Heartbeat,
            };
            (via, target)
        }
        ChannelKind::Agent => {
            need_target("the command to run, in quotes")?;
            (Via::Agent, target)
        }
    };
    let level = level.unwrap_or(match kind {
        ChannelKind::Steery | ChannelKind::Agent => Kind::Ask,
        _ => Kind::Warning,
    });
    let mut channels = ls()?;
    let name = (1..)
        .map(|i| match i {
            1 => kind.as_str().to_owned(),
            i => format!("{kind}-{i}"),
        })
        .find(|name| channels.iter().all(|c| &c.name != name))
        .unwrap_or_default();
    let channel = Channel {
        name,
        via,
        target,
        secret,
        level,
    };
    channels.push(channel.clone());
    save(&channels)?;
    Ok(channel)
}

pub fn ls() -> Result<Vec<Channel>> {
    let path = file();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not valid", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Removes by name over the file's raw entries, so an entry `ls` cannot read (one an
/// older subweave wrote) can still go.
pub fn rm(name: &str) -> Result<()> {
    let path = file();
    let mut entries: Vec<serde_json::Value> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("{} is not valid", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    let before = entries.len();
    entries.retain(|c| c["name"] != name);
    if entries.len() == before {
        bail!("no channel named {name}");
    }
    let mut bytes = serde_json::to_vec_pretty(&entries)?;
    bytes.push(b'\n');
    write_private(&path, &bytes)
}

/// Send an info message to the channel whatever its level.
pub async fn test(name: &str) -> Result<()> {
    let ch = ls()?
        .into_iter()
        .find(|c| c.name == name)
        .ok_or_else(|| anyhow!("no channel named {name}"))?;
    if ch.kind() == ChannelKind::Agent {
        bail!("an agent channel hears escalations only: its command runs once per escalation");
    }
    let msg = Message {
        kind: Kind::Info,
        host: hostname(),
        service: "subweave".into(),
        goal: None,
        card: Card::note(
            "Channel test",
            vec![format!("This is a test message for channel {name}.")],
        ),
        ask: None,
    };
    deliver(&ch, &msg, &always).await.map(|_| ())
}

fn file() -> PathBuf {
    crate::paths::config().join("channels.json")
}

fn save(channels: &[Channel]) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(channels)?;
    bytes.push(b'\n');
    write_private(&file(), &bytes)
}

/// Write `bytes` to `path` with mode 0600, replacing it atomically.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("write {}", tmp.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
        })
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "localhost".into())
}

/// One HTTP client for every channel; the timeout covers a 50 s long poll.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(75))
        .user_agent(concat!("subweave/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("a client with rustls builds")
});

pub(crate) fn client() -> &'static reqwest::Client {
    &CLIENT
}

/// Send `req` and require a 2xx. Errors never carry the URL: for Telegram and Slack
/// the URL holds the secret.
pub(crate) async fn http(what: &str, req: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let resp = req
        .send()
        .await
        .map_err(|e| anyhow!("{what}: {}", e.without_url()))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.trim().chars().take(300).collect();
    bail!("{what}: HTTP {}: {body}", status.as_u16())
}

pub(crate) async fn http_json<T: serde::de::DeserializeOwned>(
    what: &str,
    req: reqwest::RequestBuilder,
) -> Result<T> {
    http(what, req)
        .await?
        .json()
        .await
        .map_err(|e| anyhow!("{what}: {}", e.without_url()))
}

/// `[ask #3] box-1/postgres: Restart postgres?`
pub(crate) fn headline(msg: &Message) -> String {
    let label = match &msg.ask {
        Some(ask) => format!("ask #{}", ask.n),
        None => msg.kind.as_str().to_owned(),
    };
    format!("[{label}] {}/{}: {}", msg.host, msg.service, msg.card.title)
}

/// The card as plain text; for an ask on a channel that cannot answer, each answer word
/// with its button and, last, the command that gives one.
pub(crate) fn details(msg: &Message, answering: bool) -> String {
    let mut out = msg.card.text();
    if let (Some(ask), false) = (&msg.ask, answering) {
        out.push_str("\n\nAnswers:");
        for b in &msg.card.buttons {
            out.push_str(&format!("\n  {}: {}", b.word, b.label));
        }
        out.push_str(&format!("\nsubweave answer {} <word>", ask.n));
    }
    out
}

/// Headline and details as one text.
pub(crate) fn plain(msg: &Message, answering: bool) -> String {
    format!("{}\n\n{}", headline(msg), details(msg, answering))
}
