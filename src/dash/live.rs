//! `/api/events`: one tailer reads every service's `journal.jsonl` and `status.json`
//! every 500 ms (as `log -f` does) and fans each change out to every client.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use serde_json::json;
use tokio::sync::broadcast;

use super::App;
use super::read::Numbered;
use crate::cli::log::Tail;
use crate::daemon::now;
use crate::paths;
use crate::service::{self, Line, Status};

const EVERY: Duration = Duration::from_millis(500);
const KEEP_ALIVE: Duration = Duration::from_secs(15);
/// Events a slow client may fall behind by; past that its stream ends and it reconnects.
const BACKLOG: usize = 4096;

/// An event's name and its JSON, serialized once for every client.
pub(crate) type Ev = (&'static str, Arc<str>);

pub(crate) struct Live {
    tx: broadcast::Sender<Ev>,
}

impl Live {
    pub fn new() -> Live {
        Live {
            tx: broadcast::channel(BACKLOG).0,
        }
    }

    pub fn send(&self, name: &'static str, data: &serde_json::Value) {
        // No client listening is no error.
        let _ = self.tx.send((name, data.to_string().into()));
    }
}

struct Watched {
    journal: Tail,
    status: Option<Vec<u8>>,
}

/// Services there at the start are read from their end; a service that appears later,
/// from its first line.
pub(crate) async fn tail(app: Arc<App>) {
    let mut watched: BTreeMap<String, Watched> = BTreeMap::new();
    let mut first = true;
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tick.tick().await;
        let names = service::names().unwrap_or_default();
        watched.retain(|n, _| names.contains(n));
        for name in names {
            let w = watched.entry(name.clone()).or_insert_with(|| {
                let mut journal = Tail::start(paths::journal(&name));
                let mut status = None;
                if first {
                    journal.numbered();
                    status = std::fs::read(paths::status(&name)).ok();
                }
                Watched { journal, status }
            });
            let (from, lines) = w.journal.numbered();
            for (k, raw) in lines.iter().enumerate() {
                let Ok(line) = serde_json::from_str::<Line>(raw) else {
                    continue;
                };
                let i = from + k as u64;
                app.live.send(
                    "journal",
                    &json!({"service": name, "line": Numbered { i, line: &line }}),
                );
            }
            let bytes = std::fs::read(paths::status(&name)).ok();
            if bytes != w.status {
                if let Some(status) = bytes
                    .as_deref()
                    .and_then(|b| serde_json::from_slice::<Status>(b).ok())
                {
                    app.live
                        .send("status", &json!({"service": name, "status": status}));
                }
                w.status = bytes;
            }
        }
        first = false;
    }
}

pub(crate) async fn events(
    State(app): State<Arc<App>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = app.live.tx.subscribe();
    let hello = Event::default()
        .event("hello")
        .data(json!({"t": now()}).to_string());
    let rest = stream::unfold(rx, |mut rx| async move {
        match rx.recv().await {
            Ok((name, data)) => Some((Event::default().event(name).data(&*data), rx)),
            // Behind or closed: the stream ends and the client reconnects and reloads.
            Err(_) => None,
        }
    });
    let all = stream::once(async { hello }).chain(rest).map(Ok);
    Sse::new(all).keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
}
