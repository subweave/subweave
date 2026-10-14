//! Reading facts: a command in `bash -eo pipefail` with a 60 s limit, a learned command
//! for a quoted fact, or the lines a FOLLOW child printed since the last read.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value as Json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinHandle;

use crate::weave::{FactSource, Weave};

use super::service::Ctx;
use super::{learned, redact, sh};

const FACT_LIMIT: Duration = Duration::from_secs(60);
const FOLLOW_LINES: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FactValue {
    /// Trimmed stdout, secrets redacted.
    pub text: String,
    /// The exit status; `None` for FOLLOW facts and killed commands.
    pub code: Option<i32>,
}

impl FactValue {
    /// How people and models see it: the text, or for a command that printed
    /// nothing, `yes` (exit 0) or `no`.
    pub fn show(&self) -> String {
        if self.text.is_empty() && self.code.is_some() {
            if self.code == Some(0) { "yes" } else { "no" }.to_string()
        } else {
            super::cap(&self.text, 2000)
        }
    }
}

pub(crate) type Values = BTreeMap<String, FactValue>;

pub(crate) fn shown(values: &Values) -> BTreeMap<String, String> {
    values.iter().map(|(k, v)| (k.clone(), v.show())).collect()
}

/// Numbers and exit results go under `facts`, other text under `program_output`.
pub(crate) fn for_decider(values: &Values) -> (Json, Json) {
    let mut facts = Map::new();
    let mut output = Map::new();
    for (k, v) in values {
        let n = crate::weave::Value::Text(v.text.clone()).num();
        match n {
            Some(n) => {
                facts.insert(k.clone(), serde_json::json!(n));
            }
            None if v.text.is_empty() => {
                facts.insert(k.clone(), Json::String(v.show()));
            }
            None => {
                output.insert(k.clone(), Json::String(v.show()));
            }
        }
    }
    (Json::Object(facts), Json::Object(output))
}

struct FollowBuf {
    pending: VecDeque<String>,
    last: String,
}

/// A FOLLOW fact's long-lived child, restarted 10 s after it exits.
pub(crate) struct Follower {
    pub command: String,
    buf: Arc<Mutex<FollowBuf>>,
    pid: Arc<AtomicU32>,
    task: JoinHandle<()>,
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.task.abort();
        let pid = self.pid.load(Ordering::Relaxed);
        if pid != 0 {
            super::kill_group(pid);
        }
    }
}

impl Follower {
    fn start(
        command: String,
        env: BTreeMap<String, String>,
        secrets: Vec<(String, String)>,
    ) -> Follower {
        let buf = Arc::new(Mutex::new(FollowBuf {
            pending: VecDeque::new(),
            last: String::new(),
        }));
        let pid = Arc::new(AtomicU32::new(0));
        let (b, p, cmd) = (buf.clone(), pid.clone(), command.clone());
        let task = tokio::spawn(async move {
            loop {
                let child = super::bash(&cmd, &env)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn();
                if let Ok(mut child) = child {
                    p.store(child.id().unwrap_or(0), Ordering::Relaxed);
                    if let Some(out) = child.stdout.take() {
                        let mut lines = BufReader::new(out).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let mut g = b.lock().unwrap_or_else(|e| e.into_inner());
                            g.pending.push_back(redact(&line, &secrets));
                            while g.pending.len() > FOLLOW_LINES {
                                g.pending.pop_front();
                            }
                        }
                    }
                    let _ = child.wait().await;
                    p.store(0, Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        Follower {
            command,
            buf,
            pid,
            task,
        }
    }

    /// The lines printed since the last read; `consume: false` leaves them for the
    /// next check and also shows the lines the last check read.
    fn read(&self, consume: bool) -> String {
        let mut g = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        let pending: Vec<String> = g.pending.iter().cloned().collect();
        if consume {
            g.pending.clear();
            g.last = pending.join("\n");
            g.last.clone()
        } else if pending.is_empty() {
            g.last.clone()
        } else if g.last.is_empty() {
            pending.join("\n")
        } else {
            format!("{}\n{}", g.last, pending.join("\n"))
        }
    }
}

/// The FACT names an expression reads, `PREV()` included.
pub(crate) fn expr_facts(w: &Weave, e: &crate::weave::Expr, out: &mut BTreeSet<String>) {
    e.walk(&mut |x| match x {
        crate::weave::Expr::Name(n) | crate::weave::Expr::Prev(n) if w.fact(n).is_some() => {
            out.insert(n.clone());
        }
        _ => {}
    });
}

/// The FACT names a quoted text names in backticks.
pub(crate) fn quoted_facts(w: &Weave, text: &str, out: &mut BTreeSet<String>) {
    for (k, part) in text.split('`').enumerate() {
        if k % 2 == 1 && w.fact(part).is_some() {
            out.insert(part.to_string());
        }
    }
}

pub(crate) fn all(w: &Weave) -> BTreeSet<String> {
    w.facts.iter().map(|f| f.name.clone()).collect()
}

/// Reads the facts named. A quoted fact with no learned command is learned first, when
/// a coder is reachable; one that cannot be learned has no value. `consume` takes the
/// FOLLOW lines (a check); a repair only looks.
pub(crate) async fn read(ctx: &Ctx, names: &BTreeSet<String>, consume: bool) -> Values {
    read_with(ctx, names, consume, true).await
}

/// Reads the facts named that have a command now, for the state a model sees: a quoted
/// fact not learned yet has no value. Only checks learn, so a repair never waits on a
/// coder call for each fact its goal does not read.
pub(crate) async fn known(ctx: &Ctx, names: &BTreeSet<String>) -> Values {
    read_with(ctx, names, false, false).await
}

async fn read_with(ctx: &Ctx, names: &BTreeSet<String>, consume: bool, learn: bool) -> Values {
    let mut out = Values::new();
    for name in names {
        let Some(fact) = ctx.w.fact(name) else {
            continue;
        };
        let mut command = match &fact.source {
            FactSource::Command(c) => c.clone(),
            FactSource::Quoted(_) => {
                let c = if learn {
                    learned::fact_command(ctx, fact).await
                } else {
                    learned::learned_command(ctx, fact)
                };
                match c {
                    Some(c) => c,
                    None => continue,
                }
            }
        };
        if fact.follow {
            let text = {
                let mut follow = ctx.follow.lock().unwrap_or_else(|e| e.into_inner());
                let stale = follow.get(name).is_none_or(|f| f.command != command);
                if stale {
                    follow.insert(
                        name.clone(),
                        Follower::start(command, ctx.env.clone(), ctx.secrets.clone()),
                    );
                }
                follow[name].read(consume)
            };
            out.insert(name.clone(), FactValue { text, code: None });
            continue;
        }
        let mut o = sh(&command, &ctx.env, FACT_LIMIT).await;
        // A learned number or text that fails is a broken read, not a value: a query of a
        // column that does not exist. Learn it again with the error, and read once more.
        if !o.ok() && learn && learned::exits_zero(&ctx.w, fact) {
            match learned::relearn(ctx, fact, &command, &o).await {
                Some(c) => {
                    command = c;
                    o = sh(&command, &ctx.env, FACT_LIMIT).await;
                }
                None => continue,
            }
            if !o.ok() {
                continue;
            }
        }
        out.insert(
            name.clone(),
            FactValue {
                text: ctx.redact(o.stdout.trim()),
                code: o.code,
            },
        );
    }
    out
}
