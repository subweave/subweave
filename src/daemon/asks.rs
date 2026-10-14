//! Asks: numbered daemon-wide, journaled, sent to every channel; answers come from the
//! inbox or from the answering channels, which one background task polls.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::channel::{self, AskInfo, Message, Sent};
use crate::paths;
use crate::service::{Kind, Line, TASK};

use super::Shared;
use super::service::Ctx;

/// Where one ask went, so its answer can be read back. Kept in `asks.sent.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct SentAsk {
    pub service: String,
    /// Channel name → what it needs to find the answer.
    pub sent: BTreeMap<String, Sent>,
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
        eprintln!("{e:#}");
    }
}

pub(crate) fn forget_sent(shared: &Shared, n: u64) -> Option<SentAsk> {
    let mut sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
    let gone = sent.remove(&n);
    if gone.is_some() {
        save_sent(&sent);
    }
    gone
}

/// An ask nobody needs to answer now: no channel polls for it, and each channel that
/// showed buttons takes them away.
pub(crate) fn withdraw(shared: &Shared, n: u64) {
    let Some(gone) = forget_sent(shared, n) else {
        return;
    };
    tokio::spawn(async move {
        let channels = channel::ls().unwrap_or_default();
        for (name, sent) in gone.sent {
            if let Some(ch) = channels.iter().find(|c| c.name == name) {
                if let Err(e) = channel::close(ch, &sent).await {
                    eprintln!("ask #{n}: {e:#}");
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
            eprintln!("write {}: {e}", path.display());
        }
        n
    }
}

pub(crate) struct AskReq {
    pub goal: Option<String>,
    pub action: Option<String>,
    /// The rule that holds the action, as written: its `always`/`never` answers are kept.
    pub rule: Option<String>,
    pub title: String,
    pub text: String,
    pub facts: BTreeMap<String, String>,
    pub answers: Vec<String>,
}

/// Journals the ask (and its rule as a `gate` line), sends it to every channel.
pub(crate) fn raise(ctx: &Ctx, req: AskReq) -> u64 {
    let n = ctx.shared.next_ask();
    let (title, text, facts) = {
        let mut j = ctx.journal();
        if let Some(rule) = &req.rule {
            j.add(Line {
                kind: Kind::Gate,
                goal: req.goal.clone(),
                action: req.action.clone(),
                text: Some(rule.clone()),
                n: Some(n),
                ..Default::default()
            });
        }
        let l = j.add(Line {
            kind: Kind::Ask,
            goal: req.goal.clone(),
            action: req.action.clone(),
            text: Some(req.title.clone()),
            facts: Some(req.facts.clone()),
            n: Some(n),
            answers: Some(req.answers.clone()),
            body: Some(req.text.clone()),
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
    let msg = Message {
        kind: channel::Kind::Ask,
        host: ctx.shared.host.clone(),
        service: ctx.name.clone(),
        goal: req.goal,
        title,
        text,
        facts,
        ask: Some(AskInfo {
            n,
            answers: req.answers,
        }),
    };
    let shared = ctx.shared.clone();
    let service = ctx.name.clone();
    tokio::spawn(async move {
        let channels = match channel::ls() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("channels: {e:#}");
                return;
            }
        };
        for ch in channels {
            match channel::send(&ch, &msg).await {
                Ok(Some(s)) => {
                    // Settled while it was being sent: nothing to poll, no buttons.
                    if shared.answered.lock().unwrap_or_else(|e| e.into_inner()).contains(&n) {
                        if let Err(e) = channel::close(&ch, &s).await {
                            eprintln!("ask #{n}: {e:#}");
                        }
                        continue;
                    }
                    let mut sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
                    let entry = sent.entry(n).or_default();
                    entry.service = service.clone();
                    entry.sent.insert(ch.name.clone(), s);
                    save_sent(&sent);
                }
                Ok(None) => {}
                Err(e) => eprintln!("{e:#}"),
            }
        }
    });
    n
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
            .filter(|c| c.kind.answers())
            .collect();
        let mut set = JoinSet::new();
        {
            let sent = shared.sent.lock().unwrap_or_else(|e| e.into_inner());
            let answered = shared.answered.lock().unwrap_or_else(|e| e.into_inner());
            for ch in channels {
                let open: Vec<(u64, Sent)> = sent
                    .iter()
                    .filter(|(n, _)| !answered.contains(n))
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
                Ok((name, Ok(answers))) => {
                    for a in answers {
                        let _ = tx.send((a, name.clone()));
                    }
                }
                Ok((_, Err(e))) => {
                    eprintln!("{e:#}");
                    failed = true;
                }
                Err(e) => {
                    eprintln!("poll: {e}");
                    failed = true;
                }
            }
        }
        if failed {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }
}
