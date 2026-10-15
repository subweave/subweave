//! Escalations: numbered daemon-wide, journaled, sent to every channel; answers come
//! from the inbox or from the answering channels, which one background task polls.
//! Where each went is an event on it, so its answer is read back from the fold.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::channel::{self, AskInfo, Message};
use crate::model::coder::{Coder, Tally};
use crate::paths;
use crate::service::{self as svc, TASK};
use crate::weave::Class;

use super::card::{self, Card};
use super::goals;
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
    pub facts: BTreeMap<String, String>,
    /// Only the person's signed answer on a Steery card settles it: it goes to no other
    /// channel, and no inbox answer is taken.
    pub signed: bool,
    /// The drill that failed, on a drill's escalation.
    pub drill: Option<String>,
}

/// Journals the escalation and sends it to every channel. One a person's job raised
/// carries their `by`: it waits for a person's answer and is never withdrawn. When a
/// channel will show it, the coder's first model writes its summary first, journaled
/// with what it cost. Its number, or `None` when the journal refused or could not write
/// it: nothing is sent then.
pub(crate) fn raise(ctx: &Ctx, req: AskReq) -> Option<u64> {
    let n = ctx.shared.next_ask();
    // A drill runs beside the repair slot: the person whose job runs there did not raise it.
    let by = lock(&ctx.by)
        .clone()
        .filter(|b| !b.is_empty() && req.drill.is_none());
    let answers = req.card.words();
    let labels = req.card.labels();
    // Raised while a drill runs: the card says the failure was caused on purpose.
    let mut title = req.card.title.clone();
    let mut body = req.card.lines.clone();
    if let Some((name, label)) = super::drill::running(ctx).filter(|_| req.drill.is_none()) {
        title = format!("Drill {name}: {title}");
        body.insert(
            0,
            format!("This is drill {name}: Subweave caused this on purpose ({label}), to test the weave."),
        );
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
    let (title, text, facts) = {
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
                text: title,
                body: body.join("\n"),
                facts: req.facts.clone(),
                answers: answers.clone(),
                labels: labels.clone(),
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
        (card.title, card.body, card.facts)
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
            let note = summarize(&coder, &msg, about.as_deref()).await;
            if let Ok(lines) = &note.lines {
                msg.summary = lines.iter().map(|l| super::redact(l, &secrets)).collect();
            }
            lock(&journal).add(Event {
                ep: ep.clone(),
                ..note.event(msg.goal.clone(), n)
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
            match channel::send(&ch, &msg).await {
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

/// The summary's lines when they can be used, and what its note says it took.
struct Summarized {
    lines: Result<Vec<String>, String>,
    text: String,
    tally: Option<(Tally, String)>,
    ms: Option<u64>,
}

impl Summarized {
    /// The note on escalation `n` that says what the summary came to.
    fn event(self, goal: Option<String>, n: u64) -> Event {
        let (cost_usd, usage) = match self.tally {
            Some((t, model)) => (Some(t.cost_usd), Some(t.usage(model))),
            None => (None, None),
        };
        Event {
            cost_usd,
            usage,
            ms: self.ms,
            ..Kind::Note {
                goal,
                action: None,
                n: Some(n),
                text: self.text,
            }
            .into()
        }
    }
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

/// Polls every answering channel for what the services' folds say it reads: the open
/// escalations sent to it and its watches. Answers go to `tx` with the channel's name.
pub(crate) async fn poll_loop(
    shared: Arc<Shared>,
    tx: mpsc::UnboundedSender<(channel::Answer, String)>,
) {
    loop {
        let channels: Vec<channel::Channel> = channel::ls()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.kind().answers())
            .collect();
        let mut read: BTreeMap<String, Vec<(u64, channel::Sent)>> = BTreeMap::new();
        for name in svc::names().unwrap_or_default() {
            let journal = shared.journal(&name);
            let j = lock(&journal);
            for (n, ch, handle) in j.state().polled() {
                read.entry(ch.to_string())
                    .or_default()
                    .push((n, handle.clone()));
            }
        }
        let mut set = JoinSet::new();
        for ch in channels {
            let Some(open) = read.remove(&ch.name) else {
                continue;
            };
            set.spawn(async move {
                let r = channel::poll(&ch, &open).await;
                (ch.name, r)
            });
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
                    for n in polled.done {
                        if let Some((_, journal)) = shared.find(n) {
                            lock(&journal).add(Kind::Unwatch {
                                n,
                                channel: name.clone(),
                            });
                        }
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
