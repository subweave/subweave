//! One service's loop: due goals → the facts they read → code conditions in code,
//! quoted ones in one decider request → journal and `status.json` → the first failing
//! goal starts a repair. Checks go on while a repair runs.

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
use crate::service::{
    self as svc, GoalState, GoalStatus, Kind, Learned, Line, Spec, Status, TASK,
};
use crate::weave::{self, Condition, Weave, expr, graph};

use super::asks::{self, AskReq};
use super::facts::{self, Follower, Values};
use super::goals::{self, Scope};
use super::journal::Journal;
use super::repair::{self, Forced, Job, Outcome, Repair, TaskJob};
use super::{Shared, learned, now, save_json};

/// A check line is written when the result or the facts change, or after this long.
const LINE_EVERY: Duration = Duration::from_secs(3600);
/// A goal failing again this soon after it held sends no new warning.
const QUIET: Duration = Duration::from_secs(600);
/// After `no`, a goal waits this long before the loop acts on it again.
const NO_PARK: i64 = 3600;
const CODE_EVERY: Duration = Duration::from_secs(60);
const QUOTED_EVERY: Duration = Duration::from_secs(300);

/// What the daemon hands a service's loop; `by` says who asked.
pub(crate) enum Ctl {
    Answer { n: u64, word: String, by: String },
    Do { action: String, by: String },
    Task { text: String, by: String },
    Forget { name: String, by: String },
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
    pub coder: Option<Coder>,
    pub boxinfo: Option<BoxInfo>,
    /// What this box lacks for FROM and NEEDS.
    pub misfit: Vec<String>,
    journal: Mutex<Journal>,
    learned: Mutex<Learned>,
    pub follow: Mutex<BTreeMap<String, Follower>>,
    /// Per goal, the facts its last check read: `PREV()`.
    prev: Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    pub learning: tokio::sync::Mutex<()>,
    pub learn_wait: Mutex<BTreeMap<String, Instant>>,
    /// `restart for running` while it runs.
    pub repair: Mutex<Option<String>>,
    /// The person whose job runs now (`do`, a task, an answer's re-run): the asks it
    /// raises carry it, and only a person answers them.
    pub by: Mutex<Option<String>>,
    /// The action whose WITHIN wait runs, and when its steps ended.
    pub ran: Mutex<Option<(String, Instant)>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
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
        let mut journal = Journal::open(name, secrets.clone());
        let decider = match ModelRef::parse(&spec.decide).and_then(Decider::new) {
            Ok(d) => Some(d),
            Err(e) => {
                journal.note(None, format!("no decider: {e:#}"));
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
            Ok(c) => Some(c),
            Err(e) => {
                journal.note(None, format!("no coder: {e:#}"));
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
        let learned = svc::learned(name).unwrap_or_default();
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
            journal: Mutex::new(journal),
            learned: Mutex::new(learned),
            follow: Mutex::new(BTreeMap::new()),
            prev: Mutex::new(BTreeMap::new()),
            learning: tokio::sync::Mutex::new(()),
            learn_wait: Mutex::new(BTreeMap::new()),
            repair: Mutex::new(None),
            by: Mutex::new(None),
            ran: Mutex::new(None),
        })
    }

    pub fn journal(&self) -> MutexGuard<'_, Journal> {
        lock(&self.journal)
    }

    pub fn learned(&self) -> MutexGuard<'_, Learned> {
        lock(&self.learned)
    }

    pub fn prev_of(&self, goal: &str) -> Option<BTreeMap<String, String>> {
        lock(&self.prev).get(goal).cloned()
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
        let title = self.redact(&title);
        self.journal().add(Line {
            kind: Kind::Message,
            goal: goal.clone(),
            text: Some(format!("{}: {title}", kind.as_str())),
            ..Default::default()
        });
        let msg = Message {
            kind,
            host: self.shared.host.clone(),
            service: self.name.clone(),
            goal,
            title,
            text: self.redact(&text),
            facts: facts
                .into_iter()
                .map(|(k, v)| (k, self.redact(&v)))
                .collect(),
            ask: None,
        };
        super::notify(msg, None);
    }

    pub fn ask(&self, req: AskReq) -> u64 {
        asks::raise(self, req)
    }
}

struct Track {
    state: GoalState,
    since: Option<String>,
    last_check: Option<Instant>,
    /// Off or waiting because of a goal it comes AFTER: checked as soon as they hold.
    blocked: bool,
    /// The last check line written: its result, facts and when.
    line: Option<(Option<bool>, BTreeMap<String, String>, Instant)>,
    warned: bool,
    recovered: Option<Instant>,
    wait_until: Option<Instant>,
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
    status_at: Option<Instant>,
    dirty: bool,
}

pub(crate) async fn run(
    shared: Arc<Shared>,
    name: String,
    mut ctl: mpsc::UnboundedReceiver<Ctl>,
    announce: bool,
    tasks: Tasks,
) {
    let ctx = match Ctx::load(shared, &name) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            super::journal::note(&name, &format!("cannot start: {e:#}"), "");
            tracing::error!(service = %name, error = %format!("{e:#}"), "cannot start");
            return;
        }
    };
    let old: BTreeMap<String, GoalStatus> = svc::status(&name)
        .ok()
        .flatten()
        .map(|s| s.goals.into_iter().map(|g| (g.name.clone(), g)).collect())
        .unwrap_or_default();
    let tracks = ctx
        .w
        .goals
        .iter()
        .map(|g| {
            let since = old.get(&g.name).and_then(|o| o.since.clone());
            (
                g.name.clone(),
                Track {
                    state: old.get(&g.name).map_or(GoalState::Unknown, |o| o.state),
                    since,
                    last_check: None,
                    blocked: false,
                    line: None,
                    warned: false,
                    recovered: None,
                    wait_until: None,
                },
            )
        })
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
        status_at: None,
        dirty: true,
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => l.tick().await,
            Some(c) = ctl.recv() => l.control(c),
            Some(r) = l.repairs.join_next(), if !l.repairs.is_empty() => l.repaired(r),
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
        let due: BTreeSet<&str> = order
            .iter()
            .filter(|g| {
                let t = &self.tracks[&g.name];
                let parents_hold = g.after.iter().all(|a| {
                    self.tracks
                        .get(a)
                        .is_some_and(|p| p.state == GoalState::Holds)
                });
                let every = g.every.unwrap_or(match g.cond {
                    Condition::Code(_) => CODE_EVERY,
                    Condition::Quoted(_) => QUOTED_EVERY,
                });
                parents_hold
                    && (t.blocked
                        || t.last_check
                            .is_none_or(|at| now.duration_since(at) >= every))
            })
            .map(|g| g.name.as_str())
            .collect();
        if !due.is_empty() {
            let mut needed = BTreeSet::new();
            for g in order.iter().filter(|g| due.contains(g.name.as_str())) {
                needed.extend(goals::needs(w, g));
            }
            let read_start = Instant::now();
            let values = facts::read(&ctx, &needed, true).await;
            let read_ms = super::ms(read_start);
            let mut quoted = Vec::new();
            for g in &order {
                let after = |s: GoalState| {
                    g.after
                        .iter()
                        .any(|a| self.tracks.get(a).is_some_and(|p| p.state == s))
                };
                if after(GoalState::Off) {
                    self.set(&g.name, GoalState::Off, true);
                    continue;
                }
                let parents_hold = g.after.iter().all(|a| {
                    self.tracks
                        .get(a)
                        .is_some_and(|p| p.state == GoalState::Holds)
                });
                if !parents_hold {
                    self.set(&g.name, GoalState::Waiting, true);
                    continue;
                }
                if !due.contains(g.name.as_str()) {
                    continue;
                }
                self.tracks
                    .get_mut(&g.name)
                    .expect("a track per goal")
                    .last_check = Some(now);
                let prev = ctx.prev_of(&g.name);
                let scope = Scope {
                    ctx: &ctx,
                    facts: &values,
                    prev: prev.as_ref(),
                };
                // A fact this box cannot read yet (a quoted fact not learned) leaves the
                // goal unknown: a repair cannot make an unread fact hold.
                let unread = goals::needs(w, g)
                    .into_iter()
                    .any(|n| !values.contains_key(&n));
                if unread {
                    self.record(g, None, &values, read_ms);
                    continue;
                }
                if g.when.as_ref().is_some_and(|e| !expr::holds(e, &scope)) {
                    self.set(&g.name, GoalState::Off, false);
                    continue;
                }
                match &g.cond {
                    Condition::Code(e) => {
                        let ok = expr::holds(e, &scope);
                        self.record(g, Some(ok), &values, read_ms);
                    }
                    Condition::Quoted(_) => quoted.push(*g),
                }
            }
            if !quoted.is_empty() {
                let decide_start = Instant::now();
                let results = goals::quoted(&ctx, &quoted, &values).await;
                let took = read_ms.zip(super::ms(decide_start)).map(|(r, d)| r + d);
                for g in quoted {
                    self.record(g, results.get(&g.name).copied().flatten(), &values, took);
                }
            }
            self.dirty = true;
        }
        if self.announce
            && self
                .tracks
                .values()
                .all(|t| matches!(t.state, GoalState::Holds | GoalState::Off))
            && self.running.is_none()
        {
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
        if self.dirty
            || self
                .status_at
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(30))
        {
            self.write_status();
        }
    }

    /// Sets a goal's state without a check (off, waiting).
    fn set(&mut self, goal: &str, state: GoalState, blocked: bool) {
        let t = self.tracks.get_mut(goal).expect("a track per goal");
        t.blocked = blocked;
        if t.state != state {
            t.state = state;
            t.since = Some(now());
            self.dirty = true;
        }
    }

    /// A check's result: journal line when it says something new, messages on change.
    /// `ms`: how long its facts (and a quoted goal's decider) took.
    fn record(&mut self, g: &weave::Goal, ok: Option<bool>, values: &Values, ms: Option<u64>) {
        let ctx = self.ctx.clone();
        let read: Values = goals::needs(&ctx.w, g)
            .into_iter()
            .filter_map(|n| values.get(&n).map(|v| (n, v.clone())))
            .collect();
        let shown = facts::shown(&read);
        lock(&ctx.prev).insert(
            g.name.clone(),
            read.iter()
                .map(|(k, v)| (k.clone(), v.text.clone()))
                .collect(),
        );
        let t = self.tracks.get_mut(&g.name).expect("a track per goal");
        t.blocked = false;
        let write = match &t.line {
            None => true,
            Some((was, facts, at)) => *was != ok || *facts != shown || at.elapsed() >= LINE_EVERY,
        };
        if write {
            ctx.journal().add(Line {
                kind: Kind::Check,
                goal: Some(g.name.clone()),
                ok,
                facts: Some(shown.clone()),
                ms,
                ..Default::default()
            });
            t.line = Some((ok, shown, Instant::now()));
        }
        let state = match ok {
            Some(true) => GoalState::Holds,
            Some(false) => GoalState::Fails,
            None => GoalState::Unknown,
        };
        let was = t.state;
        if was != state {
            t.state = state;
            t.since = Some(now());
        }
        if state == GoalState::Holds && was != GoalState::Holds {
            t.wait_until = None;
            if was == GoalState::Fails {
                t.recovered = Some(Instant::now());
            }
            if t.warned {
                t.warned = false;
                ctx.message(
                    channel::Kind::Info,
                    Some(g.name.clone()),
                    format!("{} holds again", g.name),
                    goals::describe(&ctx, g),
                    facts::shown(&read),
                );
            }
        }
        if state == GoalState::Holds {
            self.withdraw(&g.name);
        }
    }

    /// Asks for `goal` the loop raised are moot once it holds: each gets a `withdrawn`
    /// answer, its channel message loses its buttons, and the channels hear it.
    /// Checked on every pass, not only on the change, so an ask left open by an older
    /// daemon closes too.
    fn withdraw(&self, goal: &str) {
        let ctx = &self.ctx;
        let open = ctx.journal().open_for(goal);
        for n in open {
            if !lock(&ctx.shared.answered).insert(n) {
                continue;
            }
            ctx.journal().add(Line {
                kind: Kind::Answer,
                goal: Some(goal.to_string()),
                text: Some(svc::WITHDRAWN.to_string()),
                n: Some(n),
                by: Some(format!("daemon: {goal} holds again")),
                ..Default::default()
            });
            asks::withdraw(&ctx.shared, n);
            ctx.message(
                channel::Kind::Info,
                Some(goal.to_string()),
                format!("#{n} withdrawn"),
                format!("{goal} holds again; ask #{n} is closed."),
                BTreeMap::new(),
            );
        }
    }

    /// Goals the loop leaves alone now: an open ask, a `no` within the hour, a `wait`.
    fn parked(&self) -> (BTreeSet<String>, bool) {
        let j = self.ctx.journal();
        let nowe = super::epoch();
        let mut goals = BTreeSet::new();
        let mut box_parked = false;
        for (goal, (_, answer)) in j.latest_asks() {
            let park = match &answer {
                None => true,
                Some((word, at)) => word == "no" && nowe - at < NO_PARK,
            };
            if park {
                match goal {
                    Some(g) => {
                        goals.insert(g);
                    }
                    None => box_parked = true,
                }
            }
        }
        for (name, t) in &self.tracks {
            if t.wait_until.is_some_and(|u| u > Instant::now()) {
                goals.insert(name.clone());
            }
        }
        (goals, box_parked)
    }

    fn start_repair(&mut self) {
        if self.running.is_some() {
            return;
        }
        let ctx = self.ctx.clone();
        let (parked, box_parked) = self.parked();
        // One person's task at a time: a queued one waits while another waits for its
        // answer, so a task's lines run from its start line to its end.
        let task_waits = ctx.journal().task_waits();
        let next = self
            .pending
            .iter()
            .position(|j| !(task_waits && matches!(j, Job::Task(_))));
        let mut job = match next.and_then(|i| self.pending.remove(i)) {
            Some(j) => j,
            None => {
                let Some(g) = graph::order(&ctx.w).into_iter().find(|g| {
                    self.tracks[&g.name].state == GoalState::Fails && !parked.contains(&g.name)
                }) else {
                    return;
                };
                Job::Repair(Repair {
                    goal: g.name.clone(),
                    forced: None,
                    ignore_tries: false,
                    by: None,
                })
            }
        };
        if !ctx.misfit.is_empty() && !ctx.journal().box_approved() {
            if !box_parked {
                ctx.journal().note(
                    None,
                    format!("this box does not fit: {}", ctx.misfit.join("; ")),
                );
                ctx.ask(AskReq {
                    goal: None,
                    action: None,
                    rule: None,
                    title: "this box does not fit the weave".into(),
                    text: format!("{}. Answer yes to go on anyway.", ctx.misfit.join(". ")),
                    facts: BTreeMap::new(),
                    answers: vec!["yes".into(), "no".into()],
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
                    ctx.journal().add(t.line(None, t.text.clone()));
                    t.again = true;
                }
                self.task = Some(t.clone());
                TASK.to_string()
            }
        });
        self.dirty = true;
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
        let facts = t.line.as_ref().map(|l| l.1.clone()).unwrap_or_default();
        let what = ctx
            .w
            .goal(goal)
            .map(|g| goals::describe(&ctx, g))
            .unwrap_or_default();
        ctx.message(
            channel::Kind::Warning,
            Some(goal.to_string()),
            format!("{goal} fails"),
            format!("{what}\nThe loop acts on it."),
            facts,
        );
    }

    fn repaired(&mut self, r: Result<(Option<String>, Outcome), tokio::task::JoinError>) {
        self.running = None;
        self.task = None;
        self.dirty = true;
        match r {
            Ok((goal, outcome)) => {
                if let Some(t) = goal.as_ref().and_then(|g| self.tracks.get_mut(g)) {
                    // Check again at once rather than at the next EVERY.
                    t.last_check = None;
                    if let Outcome::Wait = outcome {
                        let every = self
                            .ctx
                            .w
                            .goal(goal.as_deref().unwrap_or_default())
                            .and_then(|g| g.every)
                            .unwrap_or(CODE_EVERY)
                            .max(CODE_EVERY);
                        t.wait_until = Some(Instant::now() + every);
                    }
                }
            }
            Err(e) => self.ctx.journal().note(None, format!("repair ended: {e}")),
        }
    }

    fn control(&mut self, c: Ctl) {
        let ctx = self.ctx.clone();
        match c {
            Ctl::Answer { n, word, by } => {
                let (ask, rule) = {
                    let j = ctx.journal();
                    if j.answered(n) {
                        // Withdrawn while the answer was on its way.
                        return;
                    }
                    (j.ask(n).cloned(), j.gate(n).and_then(|g| g.text.clone()))
                };
                let Some(ask) = ask else { return };
                if ask.action.as_deref() == Some(TASK) {
                    self.answer_task(n, &word, &by);
                    return;
                }
                let action = ctx.w.action(&word).map(|a| a.name.clone());
                ctx.journal().add(Line {
                    kind: Kind::Answer,
                    goal: ask.goal.clone(),
                    action: action.clone(),
                    text: Some(word.clone()),
                    n: Some(n),
                    by: Some(by.clone()),
                    ..Default::default()
                });
                let Some(goal) = ask.goal.clone() else {
                    return;
                };
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
                    ("yes" | "always", None) => Some(match ask.action.clone() {
                        Some(a) => Repair {
                            goal,
                            forced: Some(Forced {
                                action: a,
                                approved: true,
                                rules_ok: rule.into_iter().collect(),
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
                let state = |g: &str| self.tracks.get(g).map(|t| t.state);
                let order = graph::order(&ctx.w);
                let goal = a
                    .for_goals
                    .iter()
                    .find(|g| state(g) == Some(GoalState::Fails))
                    .or(a.for_goals.first())
                    .cloned()
                    .or_else(|| {
                        order
                            .iter()
                            .find(|g| state(&g.name) == Some(GoalState::Fails))
                            .or(order.first())
                            .map(|g| g.name.clone())
                    });
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
        }
    }

    /// An answer to an ask a person's task raised: yes runs the task again with every
    /// rule the person passed for it and what its earlier runs did; any other word ends
    /// it.
    fn answer_task(&mut self, n: u64, word: &str, by: &str) {
        let ctx = self.ctx.clone();
        let mut j = ctx.journal();
        j.add(Line {
            kind: Kind::Answer,
            action: Some(TASK.to_string()),
            text: Some(word.to_string()),
            n: Some(n),
            by: Some(by.to_string()),
            ..Default::default()
        });
        let Some(lines) = j.task_lines(n) else { return };
        let mut job = TaskJob::new(
            lines[0].text.clone().unwrap_or_default(),
            lines[0].by.clone().unwrap_or_default(),
            lines[0].ep.clone().unwrap_or_else(super::episode::new_id),
        );
        job.again = true;
        if !matches!(word, "yes" | "always") {
            let end = job.line(Some(false), format!("ended: #{n} answered {word}"));
            j.add(end);
            return;
        }
        for l in lines {
            match l.kind {
                Kind::Answer if l.action.as_deref() == Some(TASK) => {
                    if matches!(l.text.as_deref(), Some("yes" | "always")) {
                        if let Some(rule) = l.n.and_then(|m| j.gate(m)).and_then(|g| g.text.clone()) {
                            job.rules_ok.push(rule);
                        }
                    }
                }
                Kind::Command if l.action.as_deref() == Some(TASK) => {
                    let cmd = l.text.as_deref().unwrap_or_default();
                    let state = if l.ok == Some(true) { "passed" } else { "failed" };
                    job.prior.push_str(&format!(
                        "$ {cmd}\n{}\n[{state}]\n",
                        l.output.as_deref().unwrap_or_default()
                    ));
                    if l.ok == Some(true) {
                        job.done.push(cmd.to_string());
                    }
                }
                _ => {}
            }
        }
        if !job.prior.is_empty() {
            job.prior.insert_str(0, "Earlier runs of this task, before the person's answer:\n");
        }
        drop(j);
        self.pending.push_front(Job::Task(job));
    }

    fn write_status(&mut self) {
        let ctx = &self.ctx;
        let (asks, spend) = {
            let j = ctx.journal();
            (j.open_asks(), j.month_spend())
        };
        let status = Status {
            service: ctx.name.clone(),
            t: now(),
            goals: graph::order(&ctx.w)
                .iter()
                .map(|g| {
                    let t = &self.tracks[&g.name];
                    GoalStatus {
                        name: g.name.clone(),
                        state: t.state,
                        since: t.since.clone(),
                    }
                })
                .collect(),
            repair: lock(&ctx.repair).clone(),
            asks,
            spend_month_usd: spend,
            decide: ctx.spec.decide.clone(),
            code: ctx.spec.code.clone(),
            stopped: false,
        };
        if let Err(e) = save_json(&paths::status(&ctx.name), &status) {
            tracing::error!(service = %ctx.name, error = %format!("{e:#}"), "cannot write status.json");
        }
        self.status_at = Some(Instant::now());
        self.dirty = false;
    }
}
