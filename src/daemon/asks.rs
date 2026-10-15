//! Asks: numbered daemon-wide, journaled, sent to every channel; answers come from the
//! inbox or from the answering channels, which one background task polls.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::channel::{self, AskInfo, Message, Sent};
use crate::model::coder::{Coder, Tally};
use crate::paths;
use crate::service::{Kind, Line, TASK};

use super::Shared;
use super::card::{self, Card};
use super::service::Ctx;

/// Where one ask went, so its answer can be read back. Kept in `asks.sent.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SentAsk {
    pub service: String,
    /// Channel name → what it needs to find the answer.
    pub sent: BTreeMap<String, Sent>,
    /// The ask is answered or withdrawn: what is left is watched for its own sake (a
    /// Steery connect), never for an answer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub settled: bool,
}

fn sent_file() -> std::path::PathBuf {
    paths::state().join("asks.sent.json")
}

pub(crate) fn load_sent() -> BTreeMap<u64, SentAsk> {
    std::fs::read(sent_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_sent(sent: &BTreeMap<u64, SentAsk>) {
    if let Err(e) = super::save_json(&sent_file(), sent) {
        tracing::error!(error = %format!("{e:#}"), "cannot save asks.sent.json");
    }
}

/// The ask is settled: every channel stops watching it, except a watch that outlives
/// its ask (`channel::outlives_ask`). What stopped, for its buttons to be taken away.
pub(crate) fn forget_sent(shared: &Shared, n: u64) -> Option<SentAsk> {
    let mut sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
    let entry = sent.get_mut(&n)?;
    let (keep, gone): (BTreeMap<_, _>, BTreeMap<_, _>) = std::mem::take(&mut entry.sent)
        .into_iter()
        .partition(|(_, s)| channel::outlives_ask(s));
    let gone = SentAsk {
        service: entry.service.clone(),
        sent: gone,
        settled: true,
    };
    if keep.is_empty() {
        sent.remove(&n);
    } else {
        entry.sent = keep;
        entry.settled = true;
    }
    save_sent(&sent);
    Some(gone)
}

/// An ask nobody needs to answer now: it counts as settled, so a send still to come
/// skips it and a later answer is ignored; its agent process, if any, is killed, no
/// channel polls for it, and each channel that showed buttons takes them away.
pub(crate) fn withdraw(shared: &Shared, n: u64) {
    shared.answered.lock().unwrap_or_else(|e| e.into_inner()).insert(n);
    super::agent::kill(shared, n);
    let Some(gone) = forget_sent(shared, n) else {
        return;
    };
    tokio::spawn(async move {
        let channels = channel::ls().unwrap_or_default();
        for (name, sent) in gone.sent {
            if let Some(ch) = channels.iter().find(|c| c.name == name) {
                if let Err(e) = channel::close(ch, &sent).await {
                    tracing::warn!(ask = n, channel = %name, error = %format!("{e:#}"), "closing an ask failed");
                }
            }
        }
    });
}

impl Shared {
    /// The next ask number, from `asks.next`.
    fn next_ask(&self) -> u64 {
        let _g = self.ask_lock.lock().unwrap_or_else(|e| e.into_inner());
        let path = paths::asks_next();
        let n: u64 = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| t.trim().parse().ok())
            .unwrap_or(1);
        if let Err(e) = std::fs::write(&path, format!("{}\n", n + 1)) {
            tracing::error!(path = %path.display(), error = %e, "cannot write the next ask number");
        }
        n
    }
}

pub(crate) struct AskReq {
    pub goal: Option<String>,
    pub action: Option<String>,
    /// The rule that holds the action, as written: its `always`/`never` answers are kept.
    pub rule: Option<String>,
    pub card: Card,
    pub facts: BTreeMap<String, String>,
    /// Only the person's signed answer on a Steery card settles it: it goes to no other
    /// channel, and no inbox answer is taken.
    pub signed: bool,
}

/// Journals the ask (and its rule as a `gate` line), sends it to every channel. An ask a
/// person's job raised carries their `by`: it waits for a person's answer and is never
/// withdrawn. When a channel will show it, the coder's first model writes its summary
/// first, journaled with what it cost.
pub(crate) fn raise(ctx: &Ctx, req: AskReq) -> u64 {
    let n = ctx.shared.next_ask();
    let by = ctx
        .by
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .filter(|b| !b.is_empty());
    let answers = req.card.words();
    let labels = req.card.labels();
    let (title, text, facts) = {
        let mut j = ctx.journal();
        if let Some(rule) = &req.rule {
            j.add(Line {
                kind: Kind::Gate,
                goal: req.goal.clone(),
                action: req.action.clone(),
                text: Some(rule.clone()),
                n: Some(n),
                by: by.clone(),
                ..Default::default()
            });
        }
        let l = j.add(Line {
            kind: Kind::Ask,
            goal: req.goal.clone(),
            action: req.action.clone(),
            text: Some(req.card.title.clone()),
            facts: Some(req.facts.clone()),
            n: Some(n),
            answers: Some(answers.clone()),
            labels: Some(labels.clone()),
            body: Some(req.card.lines.join("\n")),
            signed: req.signed.then_some(true),
            by,
            ..Default::default()
        });
        let title = l.text.unwrap_or_default();
        j.add(Line {
            kind: Kind::Message,
            goal: req.goal.clone(),
            action: req.action.clone().filter(|a| a == TASK),
            text: Some(format!("ask: #{n} {title}")),
            ..Default::default()
        });
        (title, l.body.unwrap_or_default(), l.facts.unwrap_or_default())
    };
    let mut msg = Message {
        kind: channel::Kind::Ask,
        host: ctx.shared.host.clone(),
        service: ctx.name.clone(),
        goal: req.goal,
        title,
        text,
        summary: Vec::new(),
        facts,
        ask: Some(AskInfo {
            n,
            answers,
            labels,
            signed: req.signed,
        }),
    };
    let shared = ctx.shared.clone();
    let service = ctx.name.clone();
    let coder = ctx.coder.clone();
    let journal = ctx.journal_handle();
    let secrets = ctx.secrets.clone();
    let about = ctx.w.about.clone();
    let ep = super::episode::for_line().map(|(ep, _)| ep);
    let signed = req.signed;
    tokio::spawn(async move {
        let channels = match channel::ls() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "cannot read the channels");
                return;
            }
        };
        if channels.is_empty() {
            return;
        }
        // A signed card shows only the daemon's words: a model's summary of it could
        // carry what an agent wrote into what the person signs.
        if let Some(coder) = coder.filter(|_| !signed) {
            let line = summarize(&coder, &msg, about.as_deref()).await;
            let mut j = journal.lock().unwrap_or_else(|e| e.into_inner());
            if let Ok(lines) = &line.lines {
                msg.summary = lines.iter().map(|l| super::redact(l, &secrets)).collect();
            }
            j.add(Line {
                goal: msg.goal.clone(),
                n: Some(n),
                ep,
                ..line.line
            });
        }
        let settled = || shared.answered.lock().unwrap_or_else(|e| e.into_inner()).contains(&n);
        for ch in channels {
            // Answered or withdrawn during the summary or an earlier send: nobody needs
            // to hear of it now.
            if settled() {
                break;
            }
            if signed && ch.kind() != channel::ChannelKind::Steery {
                continue;
            }
            if ch.kind() == channel::ChannelKind::Agent {
                super::agent::start(shared.clone(), &ch, &msg, secrets.clone());
                continue;
            }
            match channel::send(&ch, &msg).await {
                Ok(Some(s)) => {
                    // Settled while it was being sent: nothing to poll, no buttons; a
                    // watch that outlives its ask goes on.
                    let settled = settled();
                    if settled && !channel::outlives_ask(&s) {
                        if let Err(e) = channel::close(&ch, &s).await {
                            tracing::warn!(ask = n, channel = %ch.name, error = %format!("{e:#}"), "closing an ask failed");
                        }
                        continue;
                    }
                    let mut sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
                    let entry = sent.entry(n).or_default();
                    entry.service = service.clone();
                    entry.settled |= settled;
                    entry.sent.insert(ch.name.clone(), s);
                    save_sent(&sent);
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(service = %service, ask = n, channel = %ch.name, error = %format!("{e:#}"), "sending an ask failed"),
            }
        }
    });
    n
}

/// The card waits this long for its summary, then goes without one.
const SUMMARY_WAIT: Duration = Duration::from_secs(20);

/// The summary's journal line, and its lines when they can be used.
struct Summarized {
    lines: Result<Vec<String>, String>,
    line: Line,
}

/// The card as the person reads it goes to the model, never the journal's raw facts:
/// `msg` holds them redacted.
async fn summarize(coder: &Coder, msg: &Message, about: Option<&str>) -> Summarized {
    let ask = msg.ask.as_ref();
    let card = json!({
        "service": msg.service,
        "host": msg.host,
        "about": about,
        "title": msg.title,
        "lines": msg.text.lines().collect::<Vec<_>>(),
        "facts": msg.facts,
        "buttons": ask.map(|a| a.answers.iter().map(|w| a.label(w)).collect::<Vec<_>>()),
    });
    let start = Instant::now();
    let model = coder.model(0).to_string();
    let called = tokio::time::timeout(SUMMARY_WAIT, card::summary(coder, &card))
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "no answer in {} s, the card went without it",
                SUMMARY_WAIT.as_secs()
            ))
        });
    match called {
        Ok(s) => {
            let mut tally = Tally::default();
            tally.add(&s.reply);
            let text = match &s.lines {
                Ok(l) => format!("card summary by {model}: {}", l.join(" / ")),
                Err(e) => format!("card summary by {model} not used: {e}"),
            };
            Summarized {
                lines: s.lines,
                line: Line {
                    kind: Kind::Note,
                    text: Some(text),
                    cost_usd: Some(tally.cost_usd),
                    usage: Some(tally.usage(model)),
                    ms: super::ms(start),
                    ..Default::default()
                },
            }
        }
        Err(e) => Summarized {
            lines: Err(format!("{e:#}")),
            line: Line {
                kind: Kind::Note,
                text: Some(format!("card summary: {e:#}")),
                ms: super::ms(start),
                ..Default::default()
            },
        },
    }
}

/// Polls every answering channel for its open asks; answers go to `tx` with the
/// channel's name.
pub(crate) async fn poll_loop(
    shared: std::sync::Arc<Shared>,
    tx: mpsc::UnboundedSender<(channel::Answer, String)>,
) {
    loop {
        let channels: Vec<channel::Channel> = channel::ls()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.kind().answers())
            .collect();
        let mut set = JoinSet::new();
        {
            let sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
            let answered = shared.answered.lock().unwrap_or_else(|e| e.into_inner());
            for ch in channels {
                let open: Vec<(u64, Sent)> = sent
                    .iter()
                    .filter(|(n, s)| s.settled || !answered.contains(n))
                    .filter_map(|(n, s)| s.sent.get(&ch.name).map(|s| (*n, s.clone())))
                    .collect();
                if open.is_empty() {
                    continue;
                }
                set.spawn(async move {
                    let r = channel::poll(&ch, &open).await;
                    (ch.name, r)
                });
            }
        }
        if set.is_empty() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        let mut failed = false;
        while let Some(r) = set.join_next().await {
            match r {
                Ok((name, Ok(polled))) => {
                    for a in polled.answers {
                        let _ = tx.send((a, name.clone()));
                    }
                    if !polled.done.is_empty() {
                        stop_watching(&shared, &name, &polled.done);
                    }
                }
                Ok((name, Err(e))) => {
                    tracing::warn!(channel = %name, error = %format!("{e:#}"), "polling a channel failed");
                    failed = true;
                }
                Err(e) => {
                    tracing::error!(error = %e, "a channel poll ended");
                    failed = true;
                }
            }
        }
        if failed {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}

/// Channel `name` has no more to do for asks `done`: no answer comes on it.
fn stop_watching(shared: &Shared, name: &str, done: &[u64]) {
    let mut sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
    for n in done {
        if let Some(entry) = sent.get_mut(n) {
            entry.sent.remove(name);
            if entry.sent.is_empty() {
                sent.remove(n);
            }
        }
    }
    save_sent(&sent);
}
