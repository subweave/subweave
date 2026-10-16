//! The outpost's life in the daemon: held while a Steery channel exists, from the box's
//! first one, with one `holder` per daemon start. A changed channel starts it again; a
//! revoked key or another daemon's hold stops it until the channel changes.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::watch;

use super::{Shared, notify};
use crate::channel::steery::outpost::{self, Config, Ended, Holder, Stop, Tools};
use crate::channel::{self, Card, Message};
use crate::cli::mcp;
use crate::service as svc;

/// How often the channels are read for a change.
const LOOK: Duration = Duration::from_secs(2);

/// Hold the box's outpost until `stop` says why the daemon stops.
pub(crate) async fn keep(shared: Arc<Shared>, mut stop: Stop) {
    let holder = Arc::new(Holder::new());
    // The channel whose outpost ended for good: it is not held again while it stays.
    let mut ended_for: Option<Config> = None;
    loop {
        if stop.borrow().is_some() {
            return;
        }
        let Some(cfg) = outpost::config().filter(|c| ended_for.as_ref() != Some(c)) else {
            tokio::select! {
                () = tokio::time::sleep(LOOK) => {}
                _ = stop.changed() => return,
            }
            continue;
        };
        let (tell, told) = watch::channel(None);
        let mut run = tokio::spawn({
            let (cfg, holder) = (cfg.clone(), holder.clone());
            async move { outpost::run(&cfg, &holder, tools(), told).await }
        });
        let ended = loop {
            tokio::select! {
                ended = &mut run => break ended,
                () = tokio::time::sleep(LOOK) => {
                    if outpost::config().as_ref() != Some(&cfg) {
                        tell.send_replace(Some("the Steery channel changed"));
                    }
                }
                _ = stop.changed() => {
                    tell.send_replace(Some(stop.borrow().unwrap_or("the daemon stopped")));
                    let _ = (&mut run).await;
                    return;
                }
            }
        };
        let name = &cfg.name;
        match ended {
            Ok(Ended::Stopped) => {}
            Ok(Ended::Revoked(why)) => {
                tracing::warn!(outpost = %name, %why, "outpost: Steery revoked this box's key");
                notify(
                    Message {
                        kind: channel::Kind::Warning,
                        host: shared.host.clone(),
                        service: "subweave".into(),
                        goal: None,
                        card: Card::note(
                            "Steery revoked this box's key",
                            vec![format!(
                                "The outpost {name} stopped: {why}. Add the Steery channel again with a new key."
                            )],
                        ),
                        ask: None,
                    },
                    None,
                );
                ended_for = Some(cfg);
            }
            Ok(Ended::HeldElsewhere) => {
                warn(&shared, &format!("outpost {name} stopped: another daemon holds it"));
                ended_for = Some(cfg);
            }
            Ok(Ended::Refused(why)) => {
                warn(&shared, &format!("outpost {name} refused: {why}"));
                ended_for = Some(cfg);
            }
            Err(e) => {
                tracing::error!(outpost = %name, error = %e, "outpost: its task failed");
                tokio::time::sleep(LOOK).await;
            }
        }
    }
}

/// A warning in the daemon's log and every service's journal.
fn warn(shared: &Shared, text: &str) {
    tracing::warn!("{text}");
    for service in svc::names().unwrap_or_default() {
        shared.note(&service, text, "");
    }
}

/// `subweave mcp`'s tools, each result redacted as the journal redacts.
fn tools() -> Tools {
    Tools {
        list: mcp::tools(),
        policy: mcp::policy,
        call: Arc::new(|tool, args, by| {
            Box::pin(async move {
                let mut result = mcp::call_tool(tool, args, by).await;
                redact(&mut result, &secrets());
                result
            })
        }),
    }
}

/// Every service's SECRETs with the channel and provider keys, longest first.
fn secrets() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = Vec::new();
    for name in svc::names().unwrap_or_default() {
        if let Ok(spec) = svc::spec(&name) {
            all.extend(super::command_env(&name, &spec).1);
        }
    }
    all.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.1.cmp(&b.1)));
    all.dedup_by(|a, b| a.1 == b.1);
    all
}

fn redact(value: &mut Value, secrets: &[(String, String)]) {
    match value {
        Value::String(text) => *text = super::redact(text, secrets),
        Value::Array(items) => items.iter_mut().for_each(|v| redact(v, secrets)),
        Value::Object(fields) => fields.values_mut().for_each(|v| redact(v, secrets)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
