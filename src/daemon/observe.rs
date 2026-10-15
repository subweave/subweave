//! Fact reads kept as observations: each read is `(t, value)` in a ring per fact, and a
//! LINES fact's follower keeps each line with its time. A tick reads the facts its due
//! goals need once, then every goal checked in it reads the rings up to that snapshot.
//! Readers ask for a window; nothing is taken away.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value as Json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::task::JoinHandle;

use crate::weave::{Dim, Fact, FactSource, FactType, Value, Weave, Window};

use super::service::Ctx;
use super::state::{Event, Kind, Line};
use super::{Out, learned, lock, redact, sh};

/// A value as a check event shows it, read back: `Value::show` in reverse.
fn from_shown(ty: FactType, text: &str) -> Result<Value, String> {
    if let Some(why) = text.strip_prefix("broken read: ") {
        return Err(why.to_string());
    }
    match (ty, text) {
        (FactType::Bool, "yes") => Ok(Value::Bool(true)),
        (FactType::Bool, "no") => Ok(Value::Bool(false)),
        (FactType::Bool, _) => Err(format!("`{text}` is no BOOL")),
        _ => ty.parse(text),
    }
}

pub(crate) const FACT_LIMIT: Duration = Duration::from_secs(60);
/// A ring keeps at least this much, and longer for a longer window that reads it.
pub(crate) const KEEP: Duration = Duration::from_secs(3600);
/// A LINES ring keeps at most this many lines: a log that floods is cut from its oldest.
const LINES_MAX: usize = 10_000;
/// What a fact shows people and models at most.
const SHOWN: usize = 2000;

/// Unix milliseconds.
pub(crate) fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// One read of a fact, or one line of a LINES fact. `Err`: a broken read, and why.
#[derive(Debug, Clone)]
pub(crate) struct Obs {
    pub t: i64,
    pub value: Result<Value, String>,
}

/// The moment a tick read its facts: every goal checked in the tick reads up to it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Snapshot {
    pub at: i64,
}

/// A window, in Unix milliseconds: a fact's value is its newest read at `to`; window
/// functions read `[from, to]`, starting from the value the fact had at `from`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Span {
    pub from: i64,
    pub to: i64,
}

impl Snapshot {
    pub fn now() -> Snapshot {
        Snapshot { at: now_ms() }
    }

    pub fn over(self, d: Duration) -> Span {
        Span {
            from: self.at - d.as_millis() as i64,
            to: self.at,
        }
    }
}

impl Span {
    /// The window, starting no earlier than `t`.
    pub fn after(self, t: i64) -> Span {
        Span {
            from: self.from.max(t),
            to: self.to,
        }
    }
}

/// A fact's observations, oldest first.
struct Ring {
    ty: FactType,
    keep: i64,
    /// Since when the ring has observed: a window that starts earlier is not covered.
    since: i64,
    obs: VecDeque<Obs>,
}

impl Ring {
    fn push(&mut self, o: Obs) {
        let from = o.t - self.keep;
        self.obs.push_back(o);
        if self.ty == FactType::Lines {
            while self
                .obs
                .front()
                .is_some_and(|x| x.t < from || self.obs.len() > LINES_MAX)
            {
                self.obs.pop_front();
            }
        } else {
            // The newest read before `from` stays: it is the value at a window's start.
            while self.obs.get(1).is_some_and(|x| x.t <= from) {
                self.obs.pop_front();
            }
        }
    }

    /// The newest observation at or before `at`.
    fn newest(&self, at: i64) -> Option<&Obs> {
        let end = self.obs.partition_point(|o| o.t <= at);
        end.checked_sub(1).map(|i| &self.obs[i])
    }

    /// The observations in `s`, and for a value (not a line) the one before it: the value
    /// at the window's start.
    fn span(&self, s: Span) -> impl Iterator<Item = &Obs> {
        let start = self.obs.partition_point(|o| o.t < s.from);
        let end = self.obs.partition_point(|o| o.t <= s.to);
        let start = match self.ty {
            FactType::Lines => start,
            _ => start.saturating_sub(1),
        };
        self.obs.range(start..end.max(start))
    }

    fn window(&self, f: Window, s: Span) -> Option<Value> {
        if f == Window::Count {
            let start = self.obs.partition_point(|o| o.t < s.from);
            let end = self.obs.partition_point(|o| o.t <= s.to);
            let n = self.obs.range(start..end.max(start)).filter(|o| o.value.is_ok()).count();
            return Some(Value::Num(n as f64, Dim::Plain));
        }
        let mut nums = self.span(s).filter_map(|o| match &o.value {
            Ok(Value::Num(n, d)) => Some((*n, *d)),
            _ => None,
        });
        let (first, dim) = nums.next()?;
        let (mut min, mut max, mut sum, mut count, mut last) = (first, first, first, 1.0, first);
        for (n, _) in nums {
            min = min.min(n);
            max = max.max(n);
            sum += n;
            count += 1.0;
            last = n;
        }
        let n = match f {
            Window::Min => min,
            Window::Max => max,
            Window::Avg => sum / count,
            Window::Delta => last - first,
            Window::Count => unreachable!("counted above"),
        };
        Some(Value::Num(n, dim))
    }

    /// What the fact shows over `s`: its lines in it, else its newest value.
    fn show(&self, s: Span) -> Option<Seen> {
        if self.ty == FactType::Lines {
            let lines: Vec<&str> = self
                .span(s)
                .filter_map(|o| match &o.value {
                    Ok(Value::Text(l)) => Some(l.as_str()),
                    _ => None,
                })
                .collect();
            return Some(Seen {
                text: super::last(&lines.join("\n"), SHOWN).to_string(),
                json: None,
            });
        }
        let o = self.newest(s.to)?;
        Some(match &o.value {
            Err(why) => Seen {
                text: format!("broken read: {why}"),
                json: Some(Json::String(format!("broken read: {why}"))),
            },
            Ok(v) => {
                let text = super::cap(&v.show(self.ty), SHOWN);
                let json = match (self.ty, v) {
                    (FactType::Number(None), Value::Num(n, _)) => Some(serde_json::json!(n)),
                    (FactType::Bool | FactType::Number(_) | FactType::Version, _) => {
                        Some(Json::String(text.clone()))
                    }
                    (FactType::Text | FactType::Lines, _) => None,
                };
                Seen { text, json }
            }
        })
    }
}

/// A fact as one snapshot shows it to people and models.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Seen {
    pub text: String,
    /// Under `facts` in a decider request; `None`: program output.
    pub json: Option<Json>,
}

pub(crate) type Values = BTreeMap<String, Seen>;

pub(crate) fn shown(values: &Values) -> BTreeMap<String, String> {
    values.iter().map(|(k, v)| (k.clone(), v.text.clone())).collect()
}

/// `BOOL`, `NUMBER` and `VERSION` values go under `facts`, `TEXT` and `LINES` under
/// `program_output`.
pub(crate) fn for_decider(values: &Values) -> (Json, Json) {
    let mut facts = Map::new();
    let mut output = Map::new();
    for (k, v) in values {
        match &v.json {
            Some(j) => facts.insert(k.clone(), j.clone()),
            None => output.insert(k.clone(), Json::String(v.text.clone())),
        };
    }
    (Json::Object(facts), Json::Object(output))
}

/// A LINES fact's long-lived child, restarted 10 s after it exits; its lines go to the
/// fact's ring, which outlives the child.
struct Follower {
    command: String,
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
        ring: Arc<Mutex<Ring>>,
    ) -> Follower {
        let pid = Arc::new(AtomicU32::new(0));
        let (p, cmd) = (pid.clone(), command.clone());
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
                            let line = redact(&plain(&line), &secrets);
                            lock(&ring).push(Obs {
                                t: now_ms(),
                                value: Ok(Value::Text(line)),
                            });
                        }
                    }
                    let _ = child.wait().await;
                    p.store(0, Ordering::Relaxed);
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        });
        Follower { command, pid, task }
    }
}

/// A service's rings and followers.
pub(crate) struct Observer {
    rings: Mutex<BTreeMap<String, Arc<Mutex<Ring>>>>,
    followers: Mutex<BTreeMap<String, Follower>>,
    /// Per fact, how long its ring keeps: its longest window, at least `KEEP`.
    keep: BTreeMap<String, Duration>,
    /// Per LINES fact, the time of the newest line a check event holds.
    journaled: Mutex<BTreeMap<String, i64>>,
    /// Held while a PART is read.
    pub parts: tokio::sync::Mutex<()>,
}

impl Observer {
    pub fn new(keep: BTreeMap<String, Duration>) -> Observer {
        Observer {
            rings: Mutex::new(BTreeMap::new()),
            followers: Mutex::new(BTreeMap::new()),
            keep,
            journaled: Mutex::new(BTreeMap::new()),
            parts: tokio::sync::Mutex::new(()),
        }
    }

    /// The rings as the journal's check events left them: the values they show and the
    /// lines new in each. The time the daemon was down is not watched: a ring has
    /// observed only since now, so a window over it is not whole.
    pub fn rebuild(&self, w: &Weave, events: &[Event]) {
        let longest = self.keep.values().copied().max().unwrap_or(KEEP).max(KEEP);
        let from = now_ms() - longest.as_millis() as i64;
        for e in events {
            let Kind::Check { facts, lines, .. } = &e.kind else {
                continue;
            };
            let Some(t) = e.t.parse::<jiff::Timestamp>().ok().map(|t| t.as_millisecond()) else {
                continue;
            };
            if t < from {
                continue;
            }
            for (name, text) in facts {
                if let Some(f) = w.fact(name).filter(|f| f.ty != FactType::Lines) {
                    self.push(f, Obs { t, value: from_shown(f.ty, text) });
                }
            }
            for (name, ls) in lines {
                let Some(f) = w.fact(name).filter(|f| f.ty == FactType::Lines) else {
                    continue;
                };
                let ring = self.ring(f);
                let mut r = lock(&ring);
                for l in ls {
                    r.push(Obs {
                        t: l.t,
                        value: Ok(Value::Text(l.text.clone())),
                    });
                }
                if let Some(last) = ls.last() {
                    lock(&self.journaled).insert(name.clone(), last.t);
                }
            }
        }
    }

    /// Per LINES fact named, its lines up to `to` that no check event holds yet; from now
    /// on, one does.
    pub fn new_lines(&self, w: &Weave, names: &BTreeSet<String>, to: i64) -> BTreeMap<String, Vec<Line>> {
        let mut journaled = lock(&self.journaled);
        let mut out = BTreeMap::new();
        for n in names {
            if w.fact(n).is_none_or(|f| f.ty != FactType::Lines) {
                continue;
            }
            let Some(ring) = self.get(n) else { continue };
            let after = journaled.get(n).copied().unwrap_or(i64::MIN);
            let new: Vec<Line> = lock(&ring)
                .obs
                .iter()
                .filter(|o| o.t > after && o.t <= to)
                .filter_map(|o| match &o.value {
                    Ok(Value::Text(l)) => Some(Line { t: o.t, text: l.clone() }),
                    _ => None,
                })
                .collect();
            if let Some(last) = new.last() {
                journaled.insert(n.clone(), last.t);
                out.insert(n.clone(), new);
            }
        }
        out
    }

    fn ring(&self, f: &Fact) -> Arc<Mutex<Ring>> {
        lock(&self.rings)
            .entry(f.name.clone())
            .or_insert_with(|| {
                let keep = self.keep.get(&f.name).copied().unwrap_or(KEEP).max(KEEP);
                Arc::new(Mutex::new(Ring {
                    ty: f.ty,
                    keep: keep.as_millis() as i64,
                    since: now_ms(),
                    obs: VecDeque::new(),
                }))
            })
            .clone()
    }

    fn get(&self, name: &str) -> Option<Arc<Mutex<Ring>>> {
        lock(&self.rings).get(name).cloned()
    }

    fn push(&self, f: &Fact, o: Obs) {
        lock(&self.ring(f)).push(o);
    }

    /// Starts the fact's follower, or a new one when its command changed.
    fn follow(&self, f: &Fact, command: String, ctx: &Ctx) {
        let mut followers = lock(&self.followers);
        if followers.get(&f.name).is_some_and(|x| x.command == command) {
            return;
        }
        let ring = self.ring(f);
        followers.insert(
            f.name.clone(),
            Follower::start(command, ctx.env.clone(), ctx.secrets.clone(), ring),
        );
    }

    /// The fact's newest value at `at`; `None` with none, or a broken read.
    pub fn value(&self, name: &str, at: i64) -> Option<Value> {
        let ring = self.get(name)?;
        let r = lock(&ring);
        r.newest(at)?.value.clone().ok()
    }

    pub fn window(&self, name: &str, f: Window, s: Span) -> Option<Value> {
        let ring = self.get(name)?;
        let r = lock(&ring);
        r.window(f, s)
    }

    /// A LINES fact's lines in `s`, oldest first.
    pub fn lines(&self, name: &str, s: Span) -> Vec<String> {
        let Some(ring) = self.get(name) else {
            return Vec::new();
        };
        let r = lock(&ring);
        r.span(s)
            .filter_map(|o| match &o.value {
                Ok(Value::Text(l)) if r.ty == FactType::Lines => Some(l.clone()),
                _ => None,
            })
            .collect()
    }

    /// The fact has something to show over `s`: a follower runs (no line is a value too),
    /// or its newest read is not broken.
    pub fn known(&self, name: &str, s: Span) -> bool {
        self.get(name).is_some_and(|ring| {
            let r = lock(&ring);
            r.ty == FactType::Lines || r.newest(s.to).is_some_and(|o| o.value.is_ok())
        })
    }

    /// Every fact named has observed since `from`: a window from there is whole.
    pub fn covers(&self, names: &BTreeSet<String>, from: i64) -> bool {
        names
            .iter()
            .all(|n| self.get(n).is_some_and(|r| lock(&r).since <= from))
    }

    /// The facts named, as `s` shows them. A fact never read is left out.
    pub fn values(&self, names: &BTreeSet<String>, s: Span) -> Values {
        names
            .iter()
            .filter_map(|n| {
                let ring = self.get(n)?;
                let seen = lock(&ring).show(s)?;
                Some((n.clone(), seen))
            })
            .collect()
    }
}

/// What a read gives a fact of type `ty`. A learned command that exits non-zero is a
/// broken read; an author's command is judged by what it prints.
fn parse(ty: FactType, o: &Out, learned: bool, ctx: &Ctx) -> Result<Value, String> {
    let code = || o.code.map_or("killed".to_string(), |c| format!("exit {c}"));
    match ty {
        FactType::Bool => match o.code {
            Some(c) => Ok(Value::Bool(c == 0)),
            None => Err(format!("killed after {}s", FACT_LIMIT.as_secs())),
        },
        _ if o.code.is_none() || (learned && !o.ok()) => Err(code()),
        _ => ty.parse(&plain(&ctx.redact(o.stdout.trim()))),
    }
}

/// Reads the facts named, once each, into their rings, and returns the snapshot every
/// goal of this tick reads. A quoted fact with no learned command is learned first when
/// `learn`; one that cannot be learned has no value. A LINES fact starts its follower.
pub(crate) async fn read(ctx: &Ctx, names: &BTreeSet<String>, learn: bool) -> Snapshot {
    // A PART is read once for all its facts.
    let mut parts: BTreeSet<&str> = BTreeSet::new();
    for name in names {
        let Some(fact) = ctx.w.fact(name) else {
            continue;
        };
        let is_learned = matches!(fact.source, FactSource::Quoted(_));
        let command = match &fact.source {
            FactSource::Command(c) => Some(c.clone()),
            FactSource::Quoted(_) if learn => learned::fact_command(ctx, fact).await,
            FactSource::Quoted(_) => learned::learned_command(ctx, fact),
            FactSource::Part { part, .. } => {
                parts.insert(part);
                None
            }
        };
        let Some(mut command) = command else {
            continue;
        };
        if fact.ty == FactType::Lines {
            ctx.observe.follow(fact, command, ctx);
            continue;
        }
        let mut o = sh(&command, &ctx.env, FACT_LIMIT).await;
        let mut value = parse(fact.ty, &o, is_learned, ctx);
        // A learned read that breaks is learned again with its error, and read once more.
        let broken = value.as_ref().err().filter(|_| is_learned && learn).cloned();
        if let Some(why) = broken
            && let Some(c) = learned::relearn(ctx, fact, &command, &o, &why).await
        {
            command = c;
            o = sh(&command, &ctx.env, FACT_LIMIT).await;
            value = parse(fact.ty, &o, is_learned, ctx);
        }
        ctx.observe.push(
            fact,
            Obs {
                t: now_ms(),
                value,
            },
        );
    }
    for p in parts.into_iter().filter_map(|p| ctx.w.part(p)) {
        let t = now_ms();
        for (field, value) in super::parts::read(ctx, p).await {
            if let Some(fact) = ctx.w.fact(&format!("{}.{}", p.name, field.as_str())) {
                ctx.observe.push(fact, Obs { t, value });
            }
        }
    }
    Snapshot::now()
}

/// Text without terminal escapes: colour codes in a program's logs are noise to people
/// and tokens to models.
fn plain(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // CSI (`ESC [`) runs to a final byte in @..~; OSC (`ESC ]`) to BEL or `ESC \`;
        // any other escape is two characters.
        match chars.next() {
            Some('[') => {
                while chars.next().is_some_and(|c| !('@'..='~').contains(&c)) {}
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
