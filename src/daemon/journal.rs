//! A service's journal, `journal.jsonl`: one event per line, the only record of its
//! state. The daemon writes it through `Journal`, which folds each event as it writes
//! it; readers fold it with `Follower`. `state.json` caches a fold with the part of the
//! journal it covers.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

use super::state::{self, Escalation, Event, Kind, ServiceState};
use super::{now, redact, save_json};

/// The daemon writes `state.json` again after this many lines.
const SNAPSHOT_EVERY: u64 = 1000;

/// `state.json`: the fold of the journal's first `lines` lines, its first `offset`
/// bytes, by the daemon of `version`. Another version's is not read: its fold may differ.
#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: String,
    offset: u64,
    lines: u64,
    state: ServiceState,
}

/// The journal the daemon writes, with the fold of every line in it.
pub(crate) struct Journal {
    service: String,
    state: ServiceState,
    /// Bytes and lines of the journal file the state covers.
    offset: u64,
    lines: u64,
    /// Lines since `state.json` was written.
    unsaved: u64,
    /// Model spend not yet on a line; the next line carries it.
    pending: f64,
    secrets: Vec<(String, String)>,
    /// The service was removed: a writer still holding its journal writes nothing.
    closed: bool,
}

impl Journal {
    /// The journal replayed from its first line: a snapshot of an older daemon may fold
    /// differently. A line a crash cut short is ended first, so the daemon folds it as
    /// readers do and the next line starts on its own.
    pub fn open(service: &str) -> Journal {
        end_partial(&paths::journal(service));
        let mut f = Follower::start(service);
        f.read();
        let mut j = Journal {
            service: service.to_string(),
            state: f.state,
            offset: f.tail.pos,
            lines: f.tail.count,
            unsaved: f.tail.count,
            pending: 0.0,
            secrets: Vec::new(),
            closed: false,
        };
        if j.unsaved > 0 {
            j.save();
        }
        j
    }

    /// The service is gone: nothing more is written for it.
    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn state(&self) -> &ServiceState {
        &self.state
    }

    /// The values each line's text is redacted of, as (name, value), longest first.
    pub fn set_secrets(&mut self, secrets: Vec<(String, String)>) {
        self.secrets = secrets;
    }

    /// Journals `e` and folds it: the time and the running episode set (and its job, on
    /// its first line), secrets redacted, pending spend added. An event the state does
    /// not allow is journaled as refused, with why. `false`: refused, or not written.
    pub fn add(&mut self, e: impl Into<Event>) -> bool {
        if self.closed {
            return false;
        }
        let mut e = e.into();
        if e.t.is_empty() {
            e.t = now();
        }
        if e.ep.is_none() {
            if let Some((ep, job)) = super::episode::for_line() {
                e.ep = Some(ep);
                e.job = e.job.or(job);
            }
        }
        let secrets = &self.secrets;
        e.kind.redact(&|s| redact(s, secrets));
        // A line with a model's usage keeps that model's cost alone.
        if self.pending > 0.0 && e.usage.is_none() {
            e.cost_usd = Some(e.cost_usd.unwrap_or(0.0) + self.pending);
            self.pending = 0.0;
        }
        let applied = match state::apply(&mut self.state, &e) {
            Ok(()) => true,
            Err(why) => {
                e = Event {
                    kind: Kind::Refused {
                        why,
                        refused: Box::new(e.kind),
                    },
                    ..e
                };
                // A refusal changes nothing but the spend it carries.
                let _ = state::apply(&mut self.state, &e);
                false
            }
        };
        if !self.write(&e) {
            // The fold holds only what the file holds.
            *self = Journal {
                secrets: std::mem::take(&mut self.secrets),
                pending: self.pending,
                ..Journal::open(&self.service)
            };
            return false;
        }
        applied
    }

    fn write(&mut self, e: &Event) -> bool {
        let path = paths::journal(&self.service);
        let written = (|| -> anyhow::Result<u64> {
            let mut bytes = serde_json::to_vec(e)?;
            bytes.push(b'\n');
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)?
                .write_all(&bytes)?;
            Ok(bytes.len() as u64)
        })();
        let len = match written {
            Ok(len) => len,
            Err(err) => {
                tracing::error!(path = %path.display(), error = %format!("{err:#}"), "cannot write a journal line");
                return false;
            }
        };
        self.offset += len;
        self.lines += 1;
        self.unsaved += 1;
        if self.unsaved >= SNAPSHOT_EVERY {
            self.save();
        }
        super::trace::line(&self.service, e);
        true
    }

    /// `state.json`, from the fold.
    fn save(&mut self) {
        let snap = Snapshot {
            version: env!("CARGO_PKG_VERSION").to_string(),
            offset: self.offset,
            lines: self.lines,
            state: self.state.clone(),
        };
        match save_json(&paths::snapshot(&self.service), &snap) {
            Ok(()) => self.unsaved = 0,
            Err(e) => {
                tracing::error!(service = %self.service, error = %format!("{e:#}"), "cannot write state.json")
            }
        }
    }

    pub fn note(&mut self, goal: Option<&str>, text: impl Into<String>) {
        self.note_by(goal, text, "");
    }

    /// A note someone caused; `by`, when not empty, says who.
    pub fn note_by(&mut self, goal: Option<&str>, text: impl Into<String>, by: &str) {
        self.add(Event {
            by: Some(by.to_string()).filter(|b| !b.is_empty()),
            ..Kind::Note {
                goal: goal.map(str::to_string),
                action: None,
                n: None,
                text: text.into(),
            }
            .into()
        });
    }

    /// Spend that goes on the next line.
    pub fn spend(&mut self, cost: f64) {
        self.pending += cost;
    }
}

/// Ends the file's last line when a crash cut it before its newline.
fn end_partial(path: &Path) {
    let cut = (|| -> std::io::Result<bool> {
        let mut f = std::fs::File::open(path)?;
        let len = f.metadata()?.len();
        if len == 0 {
            return Ok(false);
        }
        f.seek(SeekFrom::Start(len - 1))?;
        let mut last = [0u8; 1];
        f.read_exact(&mut last)?;
        Ok(last[0] != b'\n')
    })();
    let ended = match cut {
        Ok(false) => return,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Ok(true) => std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .and_then(|mut f| f.write_all(b"\n")),
        Err(e) => Err(e),
    };
    if let Err(e) = ended {
        tracing::error!(path = %path.display(), error = %e, "cannot end a cut journal line");
    }
}

/// A service's journal as a reader follows it: the fold of every line read so far.
pub(crate) struct Follower {
    tail: Tail,
    pub state: ServiceState,
}

impl Follower {
    /// Nothing read yet: the journal from its first line.
    pub fn start(service: &str) -> Follower {
        Follower {
            tail: Tail::start(paths::journal(service)),
            state: ServiceState::default(),
        }
    }

    /// The journal read to its end now, from the daemon's `state.json` on when it covers
    /// a part of this journal.
    pub fn open(service: &str) -> Follower {
        let path = paths::journal(service);
        let len = std::fs::metadata(&path).map_or(0, |m| m.len());
        let snap = std::fs::read(paths::snapshot(service))
            .ok()
            .and_then(|b| serde_json::from_slice::<Snapshot>(&b).ok())
            .filter(|s| s.version == env!("CARGO_PKG_VERSION") && s.offset <= len);
        let mut f = match snap {
            Some(s) => Follower {
                tail: Tail {
                    path,
                    pos: s.offset,
                    count: s.lines,
                },
                state: s.state,
            },
            None => Follower::start(service),
        };
        f.read();
        f
    }

    /// The lines written since the last read, each event with its index, folded into
    /// `state`. `true`: the journal was cut shorter, and is read again from its first
    /// line into a fresh state.
    pub fn read(&mut self) -> (bool, Vec<(u64, Event)>) {
        let expected = self.tail.count;
        let (first, lines) = self.tail.numbered();
        let cut = first < expected;
        if cut {
            self.state = ServiceState::default();
        }
        let mut out = Vec::with_capacity(lines.len());
        for (k, raw) in lines.iter().enumerate() {
            let Ok(e) = serde_json::from_str::<Event>(raw) else {
                continue;
            };
            // The daemon journaled what it refused: replayed, it changes nothing.
            let _ = state::apply(&mut self.state, &e);
            out.push((first + k as u64, e));
        }
        (cut, out)
    }
}

/// A service's state now, folded from its journal.
pub(crate) fn read(service: &str) -> ServiceState {
    Follower::open(service).state
}

/// An open escalation as readers show it. Its rule reads as the box reads it, `$name`
/// replaced; the escalation keeps it as written, the key of `always` and `never`.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Ask {
    pub n: u64,
    pub service: String,
    pub goal: Option<String>,
    pub action: Option<String>,
    pub rule: Option<String>,
    pub title: String,
    pub text: String,
    pub facts: BTreeMap<String, String>,
    pub answers: Vec<String>,
    /// Answer word → button label.
    pub labels: BTreeMap<String, String>,
    /// Only the person's signed answer on a Steery card settles it.
    pub signed: bool,
    pub t: String,
    /// Lines of the kinds its goal's window held that nobody knows normal: an answer
    /// with `known` says what they are.
    pub lines: Vec<String>,
}

/// A service's open escalations, oldest first.
pub(crate) fn asks(service: &str, st: &ServiceState) -> Vec<Ask> {
    let args = crate::service::spec(service)
        .map(|s| s.args)
        .unwrap_or_default();
    st.open()
        .map(|e| Ask {
            n: e.n,
            service: service.to_string(),
            goal: e.goal.clone(),
            action: e.action.clone(),
            rule: e.rule.as_deref().map(|r| crate::weave::subst(r, &args)),
            title: e.card.title.clone(),
            text: e.card.body.clone(),
            facts: e.card.facts.clone(),
            answers: e.card.answers.clone(),
            labels: e.card.labels.clone(),
            signed: e.signed,
            t: e.raised.clone(),
            lines: e.lines.clone(),
        })
        .collect()
}

/// Every open escalation on this box, by number.
pub(crate) fn all_asks() -> Result<Vec<Ask>> {
    let mut out = Vec::new();
    for name in crate::service::names()? {
        out.extend(asks(&name, &read(&name)));
    }
    out.sort_by_key(|a| a.n);
    Ok(out)
}

/// Escalation `n` on this box, open or settled, with its service.
pub(crate) fn escalation(n: u64) -> Result<Option<(String, Escalation)>> {
    for name in crate::service::names()? {
        if let Some(e) = read(&name).escalations.remove(&n) {
            return Ok(Some((name, e)));
        }
    }
    Ok(None)
}

/// Every event of a service's journal with its index among the file's non-blank lines;
/// lines that do not parse are skipped, and keep their index.
pub(crate) fn numbered(service: &str) -> Vec<(u64, Event)> {
    Follower::start(service).read().1
}

/// Every event of a service's journal; lines that do not parse are skipped.
pub(crate) fn events(service: &str) -> Vec<Event> {
    numbered(service).into_iter().map(|(_, e)| e).collect()
}

/// The lines of a journal that count: every line but blank ones. A line's index among
/// them is its `i` in the dashboard.
pub(crate) fn numbered_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim().is_empty())
}

/// Complete new lines of a growing file.
pub(crate) struct Tail {
    path: PathBuf,
    pos: u64,
    /// Lines read so far, when read from the start: the index of the next line.
    count: u64,
}

impl Tail {
    pub fn start(path: PathBuf) -> Tail {
        Tail {
            path,
            pos: 0,
            count: 0,
        }
    }

    /// From the file's end now: only lines written later.
    pub fn end(path: PathBuf) -> Tail {
        let pos = std::fs::metadata(&path).map_or(0, |m| m.len());
        Tail {
            path,
            pos,
            count: 0,
        }
    }

    pub fn lines(&mut self) -> Vec<String> {
        self.numbered().1
    }

    /// The new lines and the 0-based index of the first, for a tail made by `start`.
    pub fn numbered(&mut self) -> (u64, Vec<String>) {
        let lines = self.read().unwrap_or_default();
        let first = self.count;
        self.count += lines.len() as u64;
        (first, lines)
    }

    fn read(&mut self) -> Result<Vec<String>> {
        let mut f = match std::fs::File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("read {}", self.path.display())),
        };
        if f.metadata()?.len() < self.pos {
            self.pos = 0;
            self.count = 0;
        }
        f.seek(SeekFrom::Start(self.pos))?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        // A line still being written waits for its newline.
        let Some(end) = buf.iter().rposition(|&b| b == b'\n') else {
            return Ok(Vec::new());
        };
        self.pos += end as u64 + 1;
        Ok(numbered_lines(&String::from_utf8_lossy(&buf[..end]))
            .map(String::from)
            .collect())
    }
}
