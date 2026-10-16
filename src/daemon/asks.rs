//! Escalations: numbered daemon-wide, journaled, sent to every channel; answers come
//! from the inbox or from the answering channels, each polled by its own background task.
//! Where each went is an event on it, so its answer is read back from the fold.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::channel::card::DrillShown;
use crate::channel::{self, AskInfo, Card, Message};
use crate::model::coder::{Coder, Tally};
use crate::paths;
use crate::service::{self as svc, TASK};
use crate::weave::Class;

use super::{card, goals};
use super::journal::Journal;
use super::observe::Snapshot;
use super::service::Ctx;
use super::state::{Event, Kind, Sent};
use super::{Shared, lock};

impl Shared {
    /// The next escalation number, from `asks.next`: never one a removed service used,
    /// so a late answer to it reaches nothing.
    fn next_ask(&self) -> u64 {
        let _g = lock(&self.ask_lock);
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

/// Journals `e`, an answer or a withdraw of escalation `n`. Once it settles, its agent
/// process, if any, is killed, no channel polls for it, and each channel that showed
/// buttons takes them away. `false`: the escalation did not take it, and it is journaled
/// as refused.
pub(crate) fn settle(shared: &Shared, j: &mut Journal, e: Event) -> bool {
    let Some(n) = e.n() else { return false };
    let sent = j
        .state()
        .escalations
        .get(&n)
        .map(|x| x.sent().to_vec())
        .unwrap_or_default();
    if !j.add(e) {
        return false;
    }
    super::agent::kill(shared, n);
    close(n, sent);
    true
}

/// Takes the buttons away where escalation `n` went.
fn close(n: u64, sent: Vec<Sent>) {
    if sent.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let channels = channel::ls().unwrap_or_default();
        for s in sent {
            if let Some(ch) = channels.iter().find(|c| c.name == s.channel) {
                if let Err(e) = channel::close(ch, &s.handle).await {
                    tracing::warn!(ask = n, channel = %s.channel, error = %format!("{e:#}"), "closing an ask failed");
                }
            }
        }
    });
}

pub(crate) struct AskReq {
    pub goal: Option<String>,
    pub action: Option<String>,
    /// The rule that holds the action, as written: its `always`/`never` answers are kept.
    pub rule: Option<String>,
    pub card: Card,
    /// Only the person's signed answer on a Steery card settles it: it goes to no other
    /// channel, and no inbox answer is taken.
    pub signed: bool,
    /// The drill that failed, on a drill's escalation.
    pub drill: Option<String>,
}

/// Journals the escalation with its card and sends it to every channel. One a person's
/// job raised carries their `by`: it waits for a person's answer and is never withdrawn.
/// When a channel will show it, the coder's first model writes its summary first,
/// journaled with what it cost. Its number, or `None` when the journal refused or could
/// not write it: nothing is sent then. Whether it is still open is read after the
/// summary and before each send, throw and agent: once answered or withdrawn, nothing
/// more goes out.
pub(crate) fn raise(ctx: &Ctx, req: AskReq) -> Option<u64> {
    let n = ctx.shared.next_ask();
    // A drill runs beside the repair slot: the person whose job runs there did not raise it.
    let by = lock(&ctx.by)
        .clone()
        .filter(|b| !b.is_empty() && req.drill.is_none());
    let mut card = req.card;
    // Raised while a drill runs: the card says the failure was caused on purpose.
    if let Some((name, label)) = super::drill::running(ctx).filter(|_| req.drill.is_none()) {
        card.title = format!("Drill {name}: {}", card.title);
        card.drill = Some(DrillShown { name, label });
    }
    // The kinds of line its goal's window holds that nobody knows normal: an answer may
    // teach them.
    let lines: Vec<String> = match req.goal.as_deref().and_then(|g| ctx.w.goal(g)) {
        Some(g) => goals::kinds(ctx, g, goals::span(g, Snapshot::now()))
            .into_iter()
            .filter(|k| !k.is(Class::Normal))
            .map(|k| k.sample)
            .collect(),
        None => Vec::new(),
    };
    let card = {
        let mut j = ctx.journal();
        if let (Some(rule), Some(action)) = (&req.rule, &req.action) {
            j.add(Event {
                by: by.clone(),
                ..Kind::Gate {
                    goal: req.goal.clone(),
                    action: action.clone(),
                    text: rule.clone(),
                    ok: false,
                    p: BTreeMap::new(),
                    n: Some(n),
                }
                .into()
            });
        }
        let raised = j.add(Event {
            by,
            ..Kind::Ask {
                n,
                goal: req.goal.clone(),
                action: req.action.clone(),
                rule: req.rule.clone(),
                card,
                signed: req.signed,
                lines,
                drill: req.drill.clone(),
            }
            .into()
        });
        // The card as journaled: secrets redacted.
        let card = match j.state().escalations.get(&n) {
            Some(e) if raised => e.card.clone(),
            _ => return None,
        };
        j.add(Kind::Message {
            goal: req.goal.clone(),
            action: req.action.clone().filter(|a| a == TASK),
            text: format!("ask: #{n} {}", card.title),
        });
        card
    };
    let mut msg = Message {
        kind: channel::Kind::Ask,
        host: ctx.shared.host.clone(),
        service: ctx.name.clone(),
        goal: req.goal,
        card,
        ask: Some(AskInfo {
            n,
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
            let note = summarize(&coder, &msg, about.as_deref()).await;
            let lines: Vec<String> = match &note.lines {
                Ok(l) => l.iter().map(|l| super::redact(l, &secrets)).collect(),
                Err(_) => Vec::new(),
            };
            msg.card.summary = lines.clone();
            lock(&journal).add(Event {
                ep: ep.clone(),
                ..note.event(msg.goal.clone(), n, lines)
            });
        }
        let open = || {
            lock(&journal)
                .state()
                .escalations
                .get(&n)
                .is_some_and(|e| e.is_open())
        };
        for ch in channels {
            // Answered or withdrawn during the summary or an earlier send: nobody needs
            // to hear of it now.
            if !open() {
                break;
            }
            if signed && ch.kind() != channel::ChannelKind::Steery {
                continue;
            }
            if ch.kind() == channel::ChannelKind::Agent {
                super::agent::start(shared.clone(), &ch, &msg, secrets.clone());
                continue;
            }
            match channel::send(&ch, &msg, &open).await {
                Ok(Some(handle)) => {
                    // A watch outlives its escalation; any other handle is read for the
                    // answer, and refused once the escalation settled while it was sent:
                    // it loses its buttons then.
                    let channel = ch.name.clone();
                    if channel::outlives_ask(&handle) {
                        lock(&journal).add(Kind::Watch { n, channel, handle });
                    } else if !lock(&journal).add(Kind::Sent {
                        n,
                        channel: channel.clone(),
                        handle: handle.clone(),
                    }) {
                        close(n, vec![Sent { channel, handle }]);
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(service = %service, ask = n, channel = %ch.name, error = %format!("{e:#}"), "sending an ask failed"),
            }
        }
    });
    Some(n)
}

/// The card waits this long for its summary, then goes without one.
const SUMMARY_WAIT: Duration = Duration::from_secs(20);

/// The summary's lines when they can be used, and what its event says it took.
struct Summarized {
    lines: Result<Vec<String>, String>,
    text: String,
    tally: Option<(Tally, String)>,
    ms: Option<u64>,
}

impl Summarized {
    /// The summary event of escalation `n`: `lines` as the card shows them, none when it
    /// goes without.
    fn event(self, goal: Option<String>, n: u64, lines: Vec<String>) -> Event {
        let (cost_usd, usage) = match self.tally {
            Some((t, model)) => (Some(t.cost_usd), Some(t.usage(model))),
            None => (None, None),
        };
        Event {
            cost_usd,
            usage,
            ms: self.ms,
            ..Kind::Summary {
                n,
                goal,
                lines,
                text: self.text,
            }
            .into()
        }
    }
}

/// The card as the person reads it goes to the model, never the journal's raw facts:
/// `msg` holds them redacted. Its buttons stay out: the model explains the verdict, it
/// does not weigh the answers.
async fn summarize(coder: &Coder, msg: &Message, about: Option<&str>) -> Summarized {
    let shown = Card {
        buttons: Vec::new(),
        summary: Vec::new(),
        ..msg.card.clone()
    };
    let card = json!({
        "service": msg.service,
        "host": msg.host,
        "about": about,
        "card": shown,
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
                Ok(_) => format!("card summary by {model}"),
                Err(e) => format!("card summary by {model} not used: {e}"),
            };
            Summarized {
                lines: s.lines,
                text,
                tally: Some((tally, model)),
                ms: super::ms(start),
            }
        }
        Err(e) => Summarized {
            lines: Err(format!("{e:#}")),
            text: format!("card summary: {e:#}"),
            tally: None,
            ms: super::ms(start),
        },
    }
}

/// Polls every answering channel, each in its own loop: one channel's long poll never
/// holds up another's next round. Answers go to `tx` with the channel's name.
pub(crate) async fn poll_loop(
    shared: Arc<Shared>,
    tx: mpsc::UnboundedSender<(channel::Answer, String)>,
) {
    let mut polling: BTreeMap<String, JoinHandle<()>> = BTreeMap::new();
    loop {
        polling.retain(|_, h| !h.is_finished());
        for ch in channel::ls().unwrap_or_default() {
            if ch.kind().answers() && !polling.contains_key(&ch.name) {
                let task = poll_channel(shared.clone(), tx.clone(), ch.name.clone());
                polling.insert(ch.name, tokio::spawn(task));
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// One answering channel's rounds, each for what the services' folds say it reads: the
/// open escalations sent to it and its watches. Ends once the channel is removed.
async fn poll_channel(
    shared: Arc<Shared>,
    tx: mpsc::UnboundedSender<(channel::Answer, String)>,
    name: String,
) {
    loop {
        let Some(ch) = channel::ls()
            .unwrap_or_default()
            .into_iter()
            .find(|c| c.name == name && c.kind().answers())
        else {
            return;
        };
        let mut open: Vec<(u64, channel::Sent)> = Vec::new();
        for service in svc::names().unwrap_or_default() {
            let journal = shared.journal(&service);
            let j = lock(&journal);
            open.extend(
                j.state()
                    .polled()
                    .filter(|(_, c, _)| *c == name)
                    .map(|(n, _, handle)| (n, handle.clone())),
            );
        }
        if open.is_empty() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        match channel::poll(&ch, &open).await {
            Ok(polled) => {
                for a in polled.answers {
                    let _ = tx.send((a, name.clone()));
                }
                for n in polled.done {
                    if let Some((_, journal)) = shared.find(n) {
                        lock(&journal).add(Kind::Unwatch {
                            n,
                            channel: name.clone(),
                        });
                    }
                }
            }
            Err(e) => {
                tracing::warn!(channel = %name, error = %format!("{e:#}"), "polling a channel failed");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}
