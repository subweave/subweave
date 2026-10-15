//! One service's loop: due goals → the facts they read → code conditions in code,
//! quoted ones in one decider request → journal, whose fold is the goals' state → the
//! first failing goal starts a repair. Checks go on while a repair runs.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::boxinfo::BoxInfo;
use crate::channel::{self, Message};
use crate::model::ModelRef;
use crate::model::coder::Coder;
use crate::model::decider::Decider;
use crate::paths;
use crate::service::{self as svc, DRILL, Learned, Spec, TASK, WEAVE};
use crate::weave::{self, Condition, Weave, expr, graph};

use super::asks::{self, AskReq};
use super::card;
use super::goals::{self, Scope, Verdict};
use super::memory::{self, Taught};
use super::observe::{self, Observer, Values};
use super::journal::Journal;
use super::repair::{self, Forced, Job, Outcome, Repair, TaskJob};
use super::state::{Event, GoalState, Kind};
use super::{Shared, learned, lock, now, save_json};

/// A check line is written when the result or the facts change, or after this long.
const LINE_EVERY: Duration = Duration::from_secs(3600);
/// A goal failing again this soon after it held sends no new warning.
const QUIET: Duration = Duration::from_secs(600);
/// After `no`, a goal waits at least this long before the loop acts on it again.
const NO_PARK: Duration = Duration::from_secs(3600);
/// A decider's `wait` leaves a goal alone until its next check, at least this long.
const MIN_WAIT: Duration = Duration::from_secs(60);
/// After a first failed check, the goal is checked again this soon, at most its EVERY.
pub(crate) const RECHECK: Duration = Duration::from_secs(30);
/// A due EVERY drill that could not start (its goal, its IF) is tried again this soon.
const DRILL_RETRY: Duration = Duration::from_secs(60);

/// How long a `no` leaves a goal alone: until its next check, at least `NO_PARK`; the
/// box's own asks, `NO_PARK`.
pub(crate) fn no_park(g: Option<&weave::Goal>) -> Duration {
    g.map_or(NO_PARK, |g| g.every().max(NO_PARK))
}

/// How soon a goal's first failed check is checked again. The window is time, so the
/// re-check still reads the lines the first check read.
fn recheck(g: &weave::Goal) -> Duration {
    RECHECK.min(g.every())
}

/// What the daemon hands a service's loop; `by` says who asked. An answer is in the
/// journal already, sent while the daemon still held it: the loop acts on it.
pub(crate) enum Ctl {
    Answer {
        n: u64,
        word: String,
        by: String,
    },
    Do { action: String, by: String },
    Task { text: String, by: String },
    Forget { name: String, by: String },
    /// Run one DRILL now.
    Drill { drill: String, by: String },
    /// A weave change whose rules differ: a signed ask decides it.
    Change {
        pending: PendingChange,
        change: weave::change::Change,
    },
}

/// A weave change as proposed, kept in `changes/<n>.json` until its signed answer:
/// `base` is the Weavefile it was written against.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingChange {
    pub bundle: svc::Bundle,
    pub base: String,
    pub why: String,
    pub by: String,
}

/// The person's tasks a loop runs or has queued, kept current for the daemon: when it
/// halts the loop, each gets its end line. A started one (`again`) has its start line.
pub(crate) type Tasks = Arc<Mutex<Vec<TaskJob>>>;

/// What a service's loop and its repair share.
pub(crate) struct Ctx {
    pub shared: Arc<Shared>,
    pub name: String,
    pub w: Weave,
    pub spec: Spec,
    /// ARGs, slots and SECRETs: the environment of every command.
    pub env: BTreeMap<String, String>,
    /// (name, value), longest value first.
    pub secrets: Vec<(String, String)>,
    pub decider: Option<Decider>,
    /// Shared with the task that writes an ask's summary.
    pub coder: Option<Arc<Coder>>,
    pub boxinfo: Option<BoxInfo>,
    /// What this box lacks for FROM and NEEDS.
    pub misfit: Vec<String>,
    journal: Arc<Mutex<Journal>>,
    learned: Mutex<Learned>,
    /// The rings of the facts' reads and lines.
    pub observe: Observer,
    /// The weave's KNOWN lines by goal and template.
    pub taught: Taught,
    pub learning: tokio::sync::Mutex<()>,
    pub learn_wait: Mutex<BTreeMap<String, Instant>>,
    /// The person whose job runs now (`do`, a task, an answer's re-run): the asks it
    /// raises carry it, and only a person answers them.
    pub by: Mutex<Option<String>>,
    /// The action whose WITHIN wait runs, and when its steps ended.
    pub ran: Mutex<Option<(String, Instant)>>,
}

/// What every command of a service runs with: its ARGs, slots and SECRETs as the
/// environment, and the values to redact, as (name, value), longest value first.
pub(crate) fn command_env(
    name: &str,
    spec: &Spec,
) -> (BTreeMap<String, String>, Vec<(String, String)>) {
    let secret_map: BTreeMap<String, String> = std::fs::read(paths::secrets(name))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut secrets: Vec<(String, String)> = secret_map
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    // The coder's shell can read the daemon's own files: its channel and provider keys
    // are redacted like SECRETs, though they never reach a command's environment.
    for c in channel::ls().unwrap_or_default() {
        if let Some(s) = c.secret.filter(|s| !s.is_empty()) {
            secrets.push((format!("channel {}", c.name), s));
        }
    }
    for p in crate::model::provider::all().unwrap_or_default() {
        if let Ok(Some(k)) = p.key().map(|k| k.filter(|k| !k.is_empty())) {
            secrets.push((format!("{} key", p.name), k));
        }
    }
    secrets.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    let mut env = spec.args.clone();
    env.extend(secret_map);
    (env, secrets)
}

impl Ctx {
    fn load(shared: Arc<Shared>, name: &str) -> Result<Ctx> {
        let spec = svc::spec(name)?;
        let text = std::fs::read_to_string(paths::weavefile(name))
            .with_context(|| format!("read {}", paths::weavefile(name).display()))?;
        let w = weave::parse(name, &text).map_err(|problems| {
            anyhow!(
                "{}",
                problems
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?;
        let (env, secrets) = command_env(name, &spec);
        let journal = shared.journal(name);
        lock(&journal).set_secrets(secrets.clone());
        let decider = match ModelRef::parse(&spec.decide).and_then(Decider::new) {
            Ok(d) => Some(d),
            Err(e) => {
                lock(&journal).note(None, format!("no decider: {e:#}"));
                None
            }
        };
        let coder = match spec
            .code
            .iter()
            .map(|c| ModelRef::parse(c))
            .collect::<Result<Vec<_>>>()
            .and_then(|m| Coder::new(&m))
        {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                lock(&journal).note(None, format!("no coder: {e:#}"));
                None
            }
        };
        let (boxinfo, misfit) = match BoxInfo::read() {
            Ok(info) => {
                let misfit = info.check(&w);
                (Some(info), misfit)
            }
            Err(e) => (None, vec![format!("cannot read this box: {e:#}")]),
        };
        // A learned fact command whose fact left the weave, or whose type or text changed,
        // is dropped.
        let mut learned = svc::learned(name).unwrap_or_default();
        if learned::prune(&mut learned, &w, &spec.args) {
            learned::save(name, &learned);
        }
        let observe = Observer::new(goals::keep(&w));
        let taught = memory::taught(&w);
        // State lives in the journal: the rings start as its check events left them.
        observe.rebuild(&w, &super::journal::events(name));
        Ok(Ctx {
            shared,
            name: name.to_string(),
            w,
            spec,
            env,
            secrets,
            decider,
            coder,
            boxinfo,
            misfit,
            journal,
            learned: Mutex::new(learned),
            observe,
            taught,
            learning: tokio::sync::Mutex::new(()),
            learn_wait: Mutex::new(BTreeMap::new()),
            by: Mutex::new(None),
            ran: Mutex::new(None),
        })
    }

    pub fn journal(&self) -> MutexGuard<'_, Journal> {
        lock(&self.journal)
    }

    /// The journal, for a task that outlives this borrow.
    pub fn journal_handle(&self) -> Arc<Mutex<Journal>> {
        self.journal.clone()
    }

    pub fn learned(&self) -> MutexGuard<'_, Learned> {
        lock(&self.learned)
    }

    pub fn redact(&self, text: &str) -> String {
        super::redact(text, &self.secrets)
    }

    /// `macos 26.5 aarch64, 10 CPUs, 34.4 GB RAM, 994 GB disk`.
    pub fn box_line(&self) -> String {
        match &self.boxinfo {
            Some(b) => format!(
                "{} {} {}, {} CPUs, {} GPUs, {:.1} GB RAM, {:.0} GB disk",
                b.os, b.version, b.arch, b.cpus, b.gpus, b.ram_gb, b.disk_gb
            ),
            None => "unknown".into(),
        }
    }

    /// Journals a message line and sends it to the channels.
    pub fn message(
        &self,
        kind: channel::Kind,
        goal: Option<String>,
        title: String,
        text: String,
        facts: BTreeMap<String, String>,
    ) {
        let title = match super::drill::running(self) {
            Some((name, _)) => format!("Drill {name}: {}", self.redact(&title)),
            None => self.redact(&title),
        };
        self.journal().add(Kind::Message {
            goal: goal.clone(),
            action: None,
            text: format!("{}: {title}", kind.as_str()),
        });
        let msg = Message {
            kind,
            host: self.shared.host.clone(),
            service: self.name.clone(),
            goal,
            title,
            text: self.redact(&text),
            summary: Vec::new(),
            facts: facts
                .into_iter()
                .map(|(k, v)| (k, self.redact(&v)))
                .collect(),
            ask: None,
        };
        super::notify(msg, None);
    }

    pub fn ask(&self, req: AskReq) -> Option<u64> {
        asks::raise(self, req)
    }

    /// Leaves `goal` alone until `until_ms`. The fold refuses it for a goal that holds.
    pub fn wait(&self, goal: &str, until_ms: i64) {
        self.journal().add(Kind::Wait {
            goal: goal.to_string(),
            until: super::rfc3339(until_ms),
        });
    }
}

/// When the loop checks a goal and what it said about it last; where the goal stands,
/// and how long it is left alone, is the journal's fold.
#[derive(Default)]
struct Track {
    /// `None`: checked at once, as a goal blocked by one it comes AFTER is once they hold.
    last_check: Option<Instant>,
    /// The last check line written: its result, facts and when.
    line: Option<(Option<bool>, BTreeMap<String, String>, Instant)>,
    warned: bool,
    recovered: Option<Instant>,
    /// When a goal whose check failed once is checked again.
    again: Option<Instant>,
}

/// Whether a goal is checked now, by the goals it comes AFTER and the other services it
/// reads.
enum Gate {
    Go,
    /// A goal it comes AFTER is off.
    Off(String),
    /// Another service it comes AFTER or reads does not run, and why.
    Unknown(String),
    /// A goal it comes AFTER does not hold.
    Waiting(String),
}

struct Loop {
    ctx: Arc<Ctx>,
    tracks: BTreeMap<String, Track>,
    announce: bool,
    repairs: JoinSet<(Option<String>, Outcome)>,
    running: Option<String>,
    /// The person's task in the slot.
    task: Option<TaskJob>,
    pending: VecDeque<Job>,
    tasks: Tasks,
    ctl: mpsc::UnboundedReceiver<Ctl>,
    /// The drill that runs, beside the repair slot: the loop repairs what it breaks.
    drill: JoinSet<()>,
    /// Drills asked for by hand, with who asked.
    drills_asked: VecDeque<(String, String)>,
    /// When a due EVERY drill was last refused: it is tried again after `DRILL_RETRY`.
    drill_refused: BTreeMap<String, Instant>,
    /// When the loop started (epoch seconds): an EVERY drill that never ran counts from it.
    born: i64,
}

pub(crate) async fn run(
    shared: Arc<Shared>,
    name: String,
    ctl: mpsc::UnboundedReceiver<Ctl>,
    announce: bool,
    tasks: Tasks,
) {
    let ctx = match Ctx::load(shared.clone(), &name) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            shared.note(&name, &format!("cannot start: {e:#}"), "");
            tracing::error!(service = %name, error = %format!("{e:#}"), "cannot start");
            return;
        }
    };
    shared.registry.join(&ctx);
    let drill = {
        let mut j = ctx.journal();
        // What ran when the daemon stopped runs no more.
        let drill = j.state().drilling().map(|(n, _)| n.to_string());
        for e in j.state().interrupted("the daemon stopped") {
            j.add(e);
        }
        let goals: Vec<String> = graph::order(&ctx.w)
            .iter()
            .map(|g| g.name.clone())
            .collect();
        if !j.state().goals.keys().eq(goals.iter()) {
            j.add(Kind::Goals { goals });
        }
        drill
    };
    // What a drill's BREAK did does not outlive the daemon that ran it.
    if let Some(d) = drill.as_deref().and_then(|n| ctx.w.drill(n)) {
        super::drill::stopped(ctx.journal_handle(), ctx.env.clone(), d.clone());
    }
    let tracks = ctx
        .w
        .goals
        .iter()
        .map(|g| (g.name.clone(), Track::default()))
        .collect();
    let mut l = Loop {
        ctx,
        tracks,
        announce,
        repairs: JoinSet::new(),
        running: None,
        task: None,
        pending: VecDeque::new(),
        tasks,
        ctl,
        drill: JoinSet::new(),
        drills_asked: VecDeque::new(),
        drill_refused: BTreeMap::new(),
        born: super::epoch(),
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => l.tick().await,
            Some(c) = l.ctl.recv() => l.control(c),
            Some(r) = l.repairs.join_next(), if !l.repairs.is_empty() => l.repaired(r),
            Some(_) = l.drill.join_next(), if !l.drill.is_empty() => {}
        }
        l.keep_tasks();
    }
}

impl Loop {
    fn keep_tasks(&self) {
        let mut kept = lock(&self.tasks);
        let queued = self.pending.iter().any(|j| matches!(j, Job::Task(_)));
        if kept.is_empty() && self.task.is_none() && !queued {
            return;
        }
        *kept = self
            .task
            .iter()
            .cloned()
            .chain(self.pending.iter().filter_map(|j| match j {
                Job::Task(t) => Some(t.clone()),
                Job::Repair(_) => None,
            }))
            .collect();
    }

    async fn tick(&mut self) {
        let ctx = self.ctx.clone();
        let w = &ctx.w;
        let order = graph::order(w);
        let now = Instant::now();
        // Goals that may not be checked now say why on every tick, even with nothing due:
        // a goal that waits on another service is not due until that service's goal holds.
        let mut go = BTreeSet::new();
        for g in &order {
            match self.gate(g) {
                Gate::Go => {
                    go.insert(g.name.as_str());
                }
                Gate::Off(a) => self.off(&g.name, format!("it comes after {a}, which is off"), true),
                Gate::Unknown(why) => self.unknown(&g.name, why),
                Gate::Waiting(a) => self.waiting(&g.name, a),
            }
        }
        let due: BTreeSet<&str> = order
            .iter()
            .filter(|g| {
                let t = &self.tracks[&g.name];
                go.contains(g.name.as_str())
                    && (t.again.is_some_and(|at| now >= at)
                        || t.last_check
                            .is_none_or(|at| now.duration_since(at) >= g.every()))
            })
            .map(|g| g.name.as_str())
            .collect();
        if !due.is_empty() {
            let mut needed = BTreeSet::new();
            for g in order.iter().filter(|g| due.contains(g.name.as_str())) {
                needed.extend(w.reads(g));
            }
            // One snapshot per tick: every goal checked in it reads the rings up to it.
            let read_start = Instant::now();
            let snap = observe::read(&ctx, &needed, true).await;
            let read_ms = super::ms(read_start);
            let mut quoted = Vec::new();
            for g in &order {
                if !due.contains(g.name.as_str()) {
                    continue;
                }
                // A goal checked earlier in this tick may have stopped this one.
                match self.gate(g) {
                    Gate::Go => {}
                    Gate::Off(a) => {
                        self.off(&g.name, format!("it comes after {a}, which is off"), true);
                        continue;
                    }
                    Gate::Unknown(why) => {
                        self.unknown(&g.name, why);
                        continue;
                    }
                    Gate::Waiting(a) => {
                        self.waiting(&g.name, a);
                        continue;
                    }
                }
                self.tracks
                    .get_mut(&g.name)
                    .expect("a track per goal")
                    .last_check = Some(now);
                let span = goals::span(g, snap);
                let scope = Scope { ctx: &ctx, span };
                // An IF that cannot be judged yet leaves the goal on, as `plan` shows it.
                if g.when.as_ref().and_then(|e| expr::holds(e, &scope)) == Some(false) {
                    self.off(&g.name, "its IF is false".into(), false);
                    continue;
                }
                let mut values = ctx.observe.values(&w.reads(g), span);
                values.extend(ctx.shared.registry.seen(&w.reads_other(g)));
                match &g.cond {
                    // A fact with no value (a quoted fact not learned, a broken read) leaves
                    // the goal unknown, unless the rest of the condition decides it.
                    Condition::Code(e) => {
                        let v = Verdict {
                            ok: expr::holds(e, &scope),
                            p: None,
                        };
                        let v = self.unseen(g, span, v);
                        self.record(g, v, values, span, read_ms);
                    }
                    Condition::Quoted(_) => quoted.push((*g, span, values)),
                }
            }
            if !quoted.is_empty() {
                let decide_start = Instant::now();
                let asked: Vec<_> = quoted.iter().map(|(g, s, _)| (*g, *s)).collect();
                let results = goals::quoted(&ctx, &asked).await;
                let took = read_ms.zip(super::ms(decide_start)).map(|(r, d)| r + d);
                for (g, span, values) in quoted {
                    let v = results.get(&g.name).copied().unwrap_or(Verdict { ok: None, p: None });
                    let v = self.unseen(g, span, v);
                    self.record(g, v, values, span, took);
                }
            }
        }
        let settled = {
            let j = ctx.journal();
            j.state().goals.values().all(|g| g.holds() || g.off())
        };
        if self.announce && settled && self.running.is_none() {
            self.announce = false;
            ctx.message(
                channel::Kind::Info,
                None,
                format!("{} is up: every goal holds", ctx.name),
                format!("The request \"{}\" is done.", ctx.spec.request),
                BTreeMap::new(),
            );
        }
        self.start_repair();
        self.start_drill().await;
    }

    /// Starts a drill beside the repair slot, one at a time: one asked for by hand first,
    /// else an EVERY drill that is due. An EVERY drill starts only while every goal holds
    /// or is off, no escalation is open and the repair slot is free; any drill only while
    /// its goal holds and its IF is true.
    async fn start_drill(&mut self) {
        let ctx = self.ctx.clone();
        if !self.drill.is_empty() {
            for (name, by) in self.drills_asked.drain(..) {
                ctx.journal().note_by(None, format!("drill {name}: another drill runs"), &by);
            }
            return;
        }
        let (name, by) = match self.drills_asked.pop_front() {
            Some((name, by)) => (name, Some(by)),
            None => match self.due_drill() {
                Some(name) => (name, None),
                None => return,
            },
        };
        let Some(d) = ctx.w.drill(&name) else {
            if let Some(by) = &by {
                ctx.journal().note_by(None, format!("drill: no drill {name}"), by);
            }
            return;
        };
        if let Some(why) = super::drill::refused(&ctx, d).await {
            match &by {
                Some(by) => ctx
                    .journal()
                    .note_by(Some(&d.expect), format!("drill {name} does not run: {why}"), by),
                None => {
                    self.drill_refused.insert(name, Instant::now());
                }
            }
            return;
        }
        self.drill.spawn(super::drill::run(ctx.clone(), name, by));
    }

    /// The first EVERY drill whose EVERY has passed since it last started (or since the
    /// loop started, for one that never ran), while the service is quiet: every goal holds
    /// or is off, no escalation is open, and nothing runs or waits in the repair slot.
    fn due_drill(&self) -> Option<String> {
        let ctx = &self.ctx;
        if ctx.w.drills.iter().all(|d| d.every.is_none())
            || self.running.is_some()
            || !self.pending.is_empty()
        {
            return None;
        }
        let j = ctx.journal();
        let st = j.state();
        if st.open().next().is_some() || !st.goals.values().all(|g| g.holds() || g.off()) {
            return None;
        }
        let now = super::epoch();
        ctx.w
            .drills
            .iter()
            .filter(|d| {
                self.drill_refused
                    .get(&d.name)
                    .is_none_or(|at| at.elapsed() >= DRILL_RETRY)
            })
            .find(|d| {
                let Some(every) = d.every else { return false };
                let last = st
                    .drills
                    .get(&d.name)
                    .and_then(|r| super::epoch_of(&r.started))
                    .unwrap_or(self.born);
                now - last >= every.as_secs() as i64
            })
            .map(|d| d.name.clone())
    }

    /// A windowed goal that holds over a window the daemon has not watched whole (it
    /// started within it) is unknown: what it did not see may have been there. A failure
    /// stands: what it saw was there.
    fn unseen(&self, g: &weave::Goal, span: observe::Span, v: Verdict) -> Verdict {
        let ctx = &self.ctx;
        let partial = ctx.w.windowed(g) && !ctx.observe.covers(&ctx.w.reads(g), span.from);
        if v.ok == Some(true) && partial {
            return Verdict { ok: None, ..v };
        }
        v
    }

    /// A goal that does not apply now, for `why`; `blocked` when a goal it comes AFTER is
    /// off.
    fn off(&mut self, goal: &str, why: String, blocked: bool) {
        let state = GoalState::Off { why: why.clone() };
        let e = Kind::Off {
            goal: goal.to_string(),
            why,
        };
        self.unchecked(goal, state, e, blocked);
        self.withdraw(goal, "no longer applies: its IF, or a parent's, is false");
    }

    /// Whether `g` may be checked now: the goals it comes AFTER hold (one of this
    /// weave's may have failed one check) and the other services it reads run. A goal of
    /// another service that does not hold is that service's to fix.
    fn gate(&self, g: &weave::Goal) -> Gate {
        let ctx = &self.ctx;
        let (mut off, mut waiting) = (None, None);
        {
            let j = ctx.journal();
            let st = j.state();
            for a in g.after.iter().filter_map(|a| a.local()) {
                if st.goal(a).is_some_and(GoalState::off) {
                    off.get_or_insert_with(|| a.to_string());
                } else if !st.clears(a) {
                    waiting.get_or_insert_with(|| a.to_string());
                }
            }
        }
        // Read with this service's journal unlocked: two loops never hold each other's.
        let reg = &ctx.shared.registry;
        let mut gone = None;
        for a in &g.after {
            let Some(s) = &a.service else { continue };
            match reg.goal(s, &a.goal) {
                Err(why) => {
                    gone.get_or_insert(why);
                }
                Ok(st) if st.off() => {
                    off.get_or_insert_with(|| a.to_string());
                }
                Ok(st) if !st.holds() => {
                    waiting.get_or_insert_with(|| a.to_string());
                }
                Ok(_) => {}
            }
        }
        if let Some(a) = off {
            return Gate::Off(a);
        }
        if let Some(why) = gone.or_else(|| reg.gone(ctx.w.reads_other(g).into_iter().map(|(s, _)| s))) {
            return Gate::Unknown(why);
        }
        match waiting {
            Some(a) => Gate::Waiting(a),
            None => Gate::Go,
        }
    }

    /// A goal that waits `on` a goal it comes AFTER: its escalations are moot.
    fn waiting(&mut self, goal: &str, on: String) {
        let why = format!("waits on {on}");
        let state = GoalState::Waiting { on: on.clone() };
        let e = Kind::Waiting {
            goal: goal.to_string(),
            on,
        };
        self.unchecked(goal, state, e, true);
        self.withdraw(goal, &why);
    }

    /// A goal that cannot be judged: another service it comes AFTER or reads does not
    /// run, for `why`.
    fn unknown(&mut self, goal: &str, why: String) {
        let state = GoalState::Unknown {
            why: Some(why.clone()),
        };
        let e = Kind::Unknown {
            goal: goal.to_string(),
            why: why.clone(),
        };
        self.unchecked(goal, state, e, true);
        self.withdraw(goal, &format!("cannot be judged: {why}"));
    }

    /// A goal not checked now, journaled as `e` when it was not in `state` yet: a failed
    /// check before it no longer counts toward two in a row. One `blocked` by a goal it
    /// comes AFTER is checked as soon as they hold.
    fn unchecked(&mut self, goal: &str, state: GoalState, e: Kind, blocked: bool) {
        let t = self.tracks.get_mut(goal).expect("a track per goal");
        if blocked {
            t.last_check = None;
        }
        t.again = None;
        let mut j = self.ctx.journal();
        if j.state().goal(goal) != Some(&state) {
            j.add(e);
        }
    }

    /// A check's result: an event when it changes the goal's state, says something new,
    /// holds lines no event holds yet, or the decider judged it (its `p`); messages on
    /// change. `values`: what it read over `span`, the ring's window. `ms`: how long its
    /// facts (and a quoted goal's decider) took.
    fn record(
        &mut self,
        g: &weave::Goal,
        v: Verdict,
        values: Values,
        span: observe::Span,
        ms: Option<u64>,
    ) {
        let ctx = self.ctx.clone();
        let ok = v.ok;
        let shown = observe::shown(&values);
        // Not LINES: the event holds its lines apart. Another service's facts, under
        // `<service>:<fact>`, show what the check read of it.
        let kept: BTreeMap<String, String> = shown
            .iter()
            .filter(|(n, _)| ctx.w.fact(n).is_none_or(|f| f.ty != weave::FactType::Lines))
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        let t = self.tracks.get_mut(&g.name).expect("a track per goal");
        let (was, state) = {
            let mut j = ctx.journal();
            let was = j.state().goal(&g.name).cloned().unwrap_or(GoalState::Unknown { why: None });
            let changes = was.after_check(ok, &now()) != was;
            let says_new = match &t.line {
                None => true,
                Some((said, facts, at)) => {
                    *said != ok || *facts != kept || at.elapsed() >= LINE_EVERY
                }
            };
            let lines = ctx.observe.new_lines(&ctx.w, &ctx.w.reads(g), span.to);
            if changes || says_new || v.p.is_some() || !lines.is_empty() {
                j.add(Event {
                    ms,
                    ..Kind::Check {
                        goal: g.name.clone(),
                        ok,
                        facts: kept.clone(),
                        lines,
                        p: v.p,
                    }
                    .into()
                });
                t.line = Some((ok, kept, Instant::now()));
            }
            // A drill that waits for this goal to fail, or to hold again, sees it here.
            if let Some(e) = super::drill::seen(j.state(), &g.name) {
                j.add(e);
            }
            let state = j.state().goal(&g.name).cloned().unwrap_or(GoalState::Unknown { why: None });
            (was, state)
        };
        // One failed check is not yet a failing goal: no warning, repair or ask until a
        // second check fails too, soon after. It reads the same window, moved on by then.
        t.again = (ok == Some(false) && matches!(state, GoalState::Suspect { .. }))
            .then(|| Instant::now() + recheck(g));
        if state.holds() && !was.holds() {
            if was.fails() {
                t.recovered = Some(Instant::now());
            }
            if t.warned {
                t.warned = false;
                ctx.message(
                    channel::Kind::Info,
                    Some(g.name.clone()),
                    format!("True again: {}", card::subject(&ctx, g)),
                    format!("Goal `{}` holds again.", g.name),
                    shown,
                );
            }
        }
        if state.holds() {
            self.withdraw(&g.name, "holds again");
        }
    }

    /// Escalations for `goal` the loop raised are moot once it holds or turns off (`why`):
    /// each is withdrawn, its channel messages lose their buttons, and the channels hear
    /// it. Checked on every pass, not only on the change.
    fn withdraw(&self, goal: &str, why: &str) {
        let ctx = &self.ctx;
        let open = ctx.journal().state().open_for(goal);
        for n in open {
            let withdraw = Kind::Withdraw {
                n,
                goal: Some(goal.to_string()),
                why: format!("{goal} {why}"),
            };
            if !asks::settle(&ctx.shared, &mut ctx.journal(), withdraw.into()) {
                continue;
            }
            ctx.message(
                channel::Kind::Info,
                Some(goal.to_string()),
                format!("#{n} withdrawn"),
                format!("{goal} {why}; ask #{n} is closed."),
                BTreeMap::new(),
            );
        }
    }

    /// Goals the loop leaves alone now: an open escalation, a recent `no` (`no_park`), a
    /// `wait`.
    fn parked(&self) -> (BTreeSet<String>, bool) {
        let w = &self.ctx.w;
        self.ctx.journal().state().parked(super::epoch(), |g| {
            no_park(g.and_then(|g| w.goal(g))).as_secs() as i64
        })
    }

    fn start_repair(&mut self) {
        if self.running.is_some() {
            return;
        }
        let ctx = self.ctx.clone();
        let (parked, box_parked) = self.parked();
        let failing = {
            let j = ctx.journal();
            graph::order(&ctx.w)
                .into_iter()
                .find(|g| {
                    j.state().goal(&g.name).is_some_and(GoalState::fails)
                        && !parked.contains(&g.name)
                })
                .map(|g| g.name.clone())
        };
        // The daemon queues an answer before the journal shows it: one the state read
        // above shows is queued now, and its job goes before the loop's own repair.
        while let Ok(c) = self.ctl.try_recv() {
            self.control(c);
        }
        // One person's task at a time: a queued one waits while another waits for its
        // answer, so a task's lines run from its start line to its end.
        let task_waits = ctx.journal().state().task_waits();
        let next = self
            .pending
            .iter()
            .position(|j| !(task_waits && matches!(j, Job::Task(_))));
        let mut job = match next.and_then(|i| self.pending.remove(i)) {
            Some(j) => j,
            None => {
                // A goal that held before an action with an UNDO and fails within its
                // WITHIN: the undo runs first, through the rules.
                let regressed = {
                    let j = ctx.journal();
                    j.state()
                        .regressed(super::epoch())
                        .filter(|(_, g)| !parked.contains(*g))
                        .map(|(u, g)| (u.action.clone(), u.undo.clone(), g.to_string()))
                };
                if let Some((action, undo, goal)) = regressed {
                    ctx.journal().note(
                        Some(&goal),
                        format!("{goal} held before {action} and fails since: {undo} undoes it"),
                    );
                    Job::Repair(Repair {
                        goal,
                        forced: Some(Forced {
                            action: undo,
                            approved: false,
                            rules_ok: Vec::new(),
                        }),
                        ignore_tries: true,
                        by: None,
                    })
                } else {
                    let Some(goal) = failing else {
                        return;
                    };
                    Job::Repair(Repair {
                        goal,
                        forced: None,
                        ignore_tries: false,
                        by: None,
                    })
                }
            }
        };
        let box_approved = ctx.journal().state().box_approved();
        if !ctx.misfit.is_empty() && !box_approved {
            if !box_parked {
                ctx.journal().note(
                    None,
                    format!("this box does not fit: {}", ctx.misfit.join("; ")),
                );
                ctx.ask(AskReq {
                    goal: None,
                    action: None,
                    rule: None,
                    signed: false,
                    drill: None,
                    card: card::misfit(&ctx),
                    facts: BTreeMap::new(),
                });
            }
            if job.by_person() {
                // Asked for by the person: kept. An automatic job is made again.
                self.pending.push_front(job);
            }
            return;
        }
        self.running = Some(match &mut job {
            Job::Repair(r) => {
                if r.by.is_none() {
                    self.warn(&r.goal);
                }
                r.goal.clone()
            }
            Job::Task(t) => {
                if !t.again {
                    ctx.journal().add(t.start());
                    t.again = true;
                }
                self.task = Some(t.clone());
                TASK.to_string()
            }
        });
        self.repairs.spawn(repair::run(ctx, job));
        // Before any await: a halt must see the start line just written.
        self.keep_tasks();
    }

    /// The warning when a goal starts failing and the loop acts.
    fn warn(&mut self, goal: &str) {
        let ctx = self.ctx.clone();
        let t = self.tracks.get_mut(goal).expect("a track per goal");
        if t.warned || t.recovered.is_some_and(|r| r.elapsed() < QUIET) {
            return;
        }
        t.warned = true;
        let Some(g) = ctx.w.goal(goal) else { return };
        // The window as the ring holds it now, LINES included: not one check line.
        let span = goals::span(g, observe::Snapshot::now());
        let facts = observe::shown(&ctx.observe.values(&ctx.w.reads(g), span));
        let mut lines = card::goal_lines(&ctx, g, &facts);
        lines.push("Subweave is fixing it, and asks you only if it needs you.".into());
        ctx.message(
            channel::Kind::Warning,
            Some(goal.to_string()),
            format!("Fixing: {}", card::subject(&ctx, g)),
            lines.join("\n"),
            facts,
        );
    }

    fn repaired(&mut self, r: Result<(Option<String>, Outcome), tokio::task::JoinError>) {
        self.running = None;
        self.task = None;
        match r {
            Ok((goal, outcome)) => {
                let Some(goal) = goal.filter(|g| self.tracks.contains_key(g)) else {
                    return;
                };
                // Check again at once rather than at the next EVERY.
                self.tracks.get_mut(&goal).expect("a track per goal").last_check = None;
                if let Outcome::Wait = outcome {
                    let every = self.ctx.w.goal(&goal).map_or(MIN_WAIT, |g| g.every().max(MIN_WAIT));
                    self.ctx.wait(&goal, observe::now_ms() + every.as_millis() as i64);
                }
            }
            Err(e) => self.ctx.journal().note(None, format!("repair ended: {e}")),
        }
    }

    fn control(&mut self, c: Ctl) {
        let ctx = self.ctx.clone();
        match c {
            Ctl::Answer { n, word, by } => {
                let esc = ctx.journal().state().escalations.get(&n).cloned();
                let Some(esc) = esc else { return };
                if esc.action.as_deref() == Some(TASK) {
                    self.answer_task(n, &word);
                    return;
                }
                if esc.action.as_deref() == Some(DRILL) {
                    // Yes drills again, most often after a weave change; any other word
                    // only settles it.
                    if let Some(d) = esc.drill.filter(|_| word == "yes") {
                        self.drills_asked.push_back((d, by));
                    }
                    return;
                }
                if esc.action.as_deref() == Some(WEAVE) {
                    // Any word but the yes the daemon applies drops the change.
                    let _ = std::fs::remove_file(paths::change(&ctx.name, n));
                    ctx.journal()
                        .note(None, format!("weave change #{n} dropped: answered {word}"));
                    return;
                }
                let Some(goal) = esc.goal.clone() else {
                    return;
                };
                let action = ctx.w.action(&word).map(|a| a.name.clone());
                let by = Some(by);
                let job = match (word.as_str(), action) {
                    (_, Some(a)) => Some(Repair {
                        goal,
                        forced: Some(Forced {
                            action: a,
                            approved: true,
                            rules_ok: Vec::new(),
                        }),
                        ignore_tries: true,
                        by,
                    }),
                    ("yes" | "always", None) => Some(match esc.action.clone() {
                        Some(a) => Repair {
                            goal,
                            forced: Some(Forced {
                                action: a,
                                approved: true,
                                rules_ok: esc.rule.into_iter().collect(),
                            }),
                            ignore_tries: true,
                            by,
                        },
                        None => Repair {
                            goal,
                            forced: None,
                            ignore_tries: true,
                            by,
                        },
                    }),
                    _ => None,
                };
                if let Some(job) = job {
                    self.pending.push_front(Job::Repair(job));
                }
            }
            Ctl::Task { text, by } => {
                self.pending.push_back(Job::Task(TaskJob::new(
                    text,
                    by,
                    super::episode::new_id(),
                )));
            }
            Ctl::Do { action, by } => {
                let Some(a) = ctx.w.action(&action) else {
                    ctx.journal().note(None, format!("do: no action {action}"));
                    return;
                };
                let goal = {
                    let j = ctx.journal();
                    let fails = |g: &str| j.state().goal(g).is_some_and(GoalState::fails);
                    let order = graph::order(&ctx.w);
                    a.for_goals
                        .iter()
                        .find(|g| fails(g))
                        .or(a.for_goals.first())
                        .cloned()
                        .or_else(|| {
                            order
                                .iter()
                                .find(|g| fails(&g.name))
                                .or(order.first())
                                .map(|g| g.name.clone())
                        })
                };
                let Some(goal) = goal else {
                    ctx.journal()
                        .note(None, format!("do {action}: the weave has no goal"));
                    return;
                };
                ctx.journal()
                    .note_by(Some(&goal), format!("do {action}: asked by hand"), &by);
                self.pending.push_back(Job::Repair(Repair {
                    goal,
                    forced: Some(Forced {
                        action,
                        approved: false,
                        rules_ok: Vec::new(),
                    }),
                    ignore_tries: true,
                    by: Some(by),
                }));
            }
            Ctl::Drill { drill, by } => self.drills_asked.push_back((drill, by)),
            Ctl::Forget { name, by } => {
                let dropped = {
                    let mut l = ctx.learned();
                    let d = learned::forget(&mut l, &name);
                    if d {
                        learned::save(&ctx.name, &l);
                    }
                    d
                };
                ctx.journal().note_by(
                    None,
                    if dropped {
                        format!("forgot {name}")
                    } else {
                        format!("forget: nothing learned for {name}")
                    },
                    &by,
                );
            }
            Ctl::Change { pending, change } => {
                let raised = ctx.ask(AskReq {
                    goal: None,
                    action: Some(WEAVE.to_string()),
                    rule: None,
                    card: card::weave_change(&ctx, &pending.by, &pending.why, &change),
                    facts: BTreeMap::new(),
                    signed: true,
                    drill: None,
                });
                let Some(n) = raised else { return };
                // What a signed yes applies.
                let path = paths::change(&ctx.name, n);
                let kept = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .map_err(anyhow::Error::from)
                    .and_then(|()| save_json(&path, &pending));
                if let Err(e) = kept {
                    ctx.journal()
                        .note(None, format!("weave change #{n} cannot be kept: {e:#}"));
                }
            }
        }
    }

    /// An answer to an escalation a person's task raised, journaled already: yes runs the
    /// task again with every rule the person passed for it and what its earlier runs did;
    /// any other word ends it.
    fn answer_task(&mut self, n: u64, word: &str) {
        let ctx = self.ctx.clone();
        let mut j = ctx.journal();
        let Some((ep, task)) = j.state().task_of(n).map(|(ep, t)| (ep.to_string(), t.clone()))
        else {
            return;
        };
        let mut job = TaskJob::new(task.text, task.by.unwrap_or_default(), ep);
        job.again = true;
        if !matches!(word, "yes" | "always") {
            j.add(job.end(false, format!("ended: #{n} answered {word}")));
            return;
        }
        job.rules_ok = task.approved;
        for ran in &task.commands {
            let state = if ran.ok { "passed" } else { "failed" };
            job.prior.push_str(&format!(
                "$ {}\n{}\n[{state}]\n",
                ran.command,
                ran.output.as_deref().unwrap_or_default()
            ));
            if ran.ok {
                job.done.push(ran.command.clone());
            }
        }
        if !job.prior.is_empty() {
            job.prior.insert_str(0, "Earlier runs of this task, before the person's answer:\n");
        }
        drop(j);
        self.pending.push_front(Job::Task(job));
    }
}
