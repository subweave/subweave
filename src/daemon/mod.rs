//! `subweave daemon`: consumes the inbox, keeps one loop task per service and polls the
//! answering channels for open escalations. `WEAVEFILE.md` "The loop" says what a loop
//! does.

mod agent;
mod asks;
mod card;
mod coder_task;
mod episode;
mod goals;
mod inbox;
pub(crate) mod journal;
mod learned;
pub(crate) mod memory;
mod observe;
mod repair;
mod service;
pub(crate) mod state;
mod trace;

use std::collections::{BTreeMap, BTreeSet};
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::channel::{self, Message};
use crate::paths;
use crate::service::{self as svc, Command, TASK};

use journal::Journal;
use service::{Ctl, PendingChange, Tasks};
use state::{Act, Event, Kind};

pub(crate) use service::command_env;

/// What every service task shares: the ask counter, the journals and the agent channels'
/// processes.
pub(crate) struct Shared {
    pub host: String,
    ask_lock: Mutex<()>,
    /// Each service's journal with its fold, opened on first use: every writer goes
    /// through it.
    journals: Mutex<BTreeMap<String, Arc<Mutex<Journal>>>>,
    /// The agent process working on each escalation, by number: a withdrawn one kills it.
    agents: Mutex<BTreeMap<u64, u32>>,
    /// Per service, held while its agent runs: one agent at a time.
    agent_turns: Mutex<BTreeMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl Shared {
    /// A service's journal, replayed on first use.
    pub fn journal(&self, service: &str) -> Arc<Mutex<Journal>> {
        lock(&self.journals)
            .entry(service.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(Journal::open(service))))
            .clone()
    }

    /// The service that raised escalation `n`, and its journal.
    pub fn find(&self, n: u64) -> Option<(String, Arc<Mutex<Journal>>)> {
        svc::names().ok()?.into_iter().find_map(|name| {
            let j = self.journal(&name);
            let has = lock(&j).state().escalations.contains_key(&n);
            has.then_some((name, j))
        })
    }

    /// A note in a service's journal; `by`, when not empty, says who caused it.
    pub fn note(&self, service: &str, text: &str, by: &str) {
        lock(&self.journal(service)).note_by(None, text, by);
    }

    /// A removed service's journal is gone with it: a task that still holds it writes
    /// nothing more, and one made again starts afresh.
    fn forget(&self, service: &str) {
        let gone = lock(&self.journals).remove(service);
        if let Some(j) = gone {
            lock(&j).close();
        }
    }
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Task {
    handle: JoinHandle<()>,
    ctl: mpsc::UnboundedSender<Ctl>,
    /// The person's tasks it runs or has queued.
    tasks: Tasks,
}

struct Daemon {
    shared: Arc<Shared>,
    tasks: BTreeMap<String, Task>,
    /// Services whose task ended on an error; they start again on their next command.
    broken: BTreeSet<String>,
}

/// Runs every service in the foreground until SIGINT or SIGTERM.
pub async fn run() -> Result<()> {
    crate::logs::init();
    trace::init();
    for dir in [paths::inbox(), paths::services()] {
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    }
    // Two daemons on one state would each run every service: every check, repair and ask
    // twice. The lock lives as long as this process, crash or not.
    let _lock = lock_state()?;
    save_json(
        &paths::daemon(),
        &Running {
            pid: std::process::id(),
            since: now(),
        },
    )?;
    let shared = Arc::new(Shared {
        host: hostname(),
        ask_lock: Mutex::new(()),
        journals: Mutex::new(BTreeMap::new()),
        agents: Mutex::new(BTreeMap::new()),
        agent_turns: Mutex::new(BTreeMap::new()),
    });
    let (answers_tx, mut answers_rx) = mpsc::unbounded_channel();
    tokio::spawn(asks::poll_loop(shared.clone(), answers_tx));
    let mut d = Daemon {
        shared,
        tasks: BTreeMap::new(),
        broken: BTreeSet::new(),
    };
    tracing::info!(
        state = %paths::state().display(),
        config = %paths::config().display(),
        "subweave daemon started"
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("listening for SIGTERM")?;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            Some((answer, from)) = answers_rx.recv() => {
                let answer: channel::Answer = answer;
                d.answer(answer.n, &answer.word, &answer.by, None, None, Some(from)).await;
                continue;
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = term.recv() => break,
        }
        for (path, cmd) in inbox::take() {
            match cmd {
                Ok(cmd) => d.apply(cmd).await,
                Err(e) => tracing::warn!(path = %path.display(), error = %format!("{e:#}"), "unreadable inbox command"),
            }
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::error!(path = %path.display(), error = %e, "cannot remove an inbox command");
            }
        }
        d.keep();
        heartbeat();
    }
    for (_, t) in std::mem::take(&mut d.tasks) {
        t.handle.abort();
        let _ = t.handle.await;
    }
    let _ = std::fs::remove_file(paths::daemon());
    let _ = tokio::task::spawn_blocking(trace::shutdown).await;
    Ok(())
}

/// `daemon.json`: the running daemon's pid and when it started.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Running {
    pub pid: u32,
    pub since: String,
}

/// The daemon on this box, while its process lives.
pub(crate) fn running() -> Option<Running> {
    let r: Running = serde_json::from_slice(&std::fs::read(paths::daemon()).ok()?).ok()?;
    let pid = libc::pid_t::try_from(r.pid).ok()?;
    // SAFETY: signal 0 only checks that the process exists; it sends nothing.
    let alive = unsafe { libc::kill(pid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    alive.then_some(r)
}

/// `daemon.lock` in the state, held with `flock` until this process ends.
fn lock_state() -> Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    let path = paths::state().join("daemon.lock");
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    // SAFETY: flock on a descriptor this function owns; the File keeps it open.
    if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let by = running().map_or(String::new(), |r| format!(": pid {}", r.pid));
        anyhow::bail!("a daemon already runs on {}{by}", paths::state().display());
    }
    Ok(f)
}

impl Daemon {
    /// Starts a task for every service that is not stopped and has none.
    fn keep(&mut self) {
        let names = match svc::names() {
            Ok(n) => n,
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "cannot list the services");
                return;
            }
        };
        for name in names {
            if let Some(t) = self.tasks.get(&name) {
                if !t.handle.is_finished() {
                    continue;
                }
                self.tasks.remove(&name);
                self.broken.insert(name.clone());
                tracing::warn!(service = %name, "its loop ended; it starts again on its next command");
            }
            if self.broken.contains(&name) {
                continue;
            }
            match svc::spec(&name) {
                Ok(spec) if !spec.stopped => self.start(&name, false),
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(service = %name, error = %format!("{e:#}"), "cannot read the service");
                    self.broken.insert(name);
                }
            }
        }
    }

    fn start(&mut self, name: &str, announce: bool) {
        let (tx, rx) = mpsc::unbounded_channel();
        let tasks = Tasks::default();
        let handle = tokio::spawn(service::run(
            self.shared.clone(),
            name.to_string(),
            rx,
            announce,
            tasks.clone(),
        ));
        self.tasks.insert(
            name.to_string(),
            Task {
                handle,
                ctl: tx,
                tasks,
            },
        );
    }

    /// Ends a service's loop, if one runs. What the journal says runs gets an end line
    /// saying `why`, and so does each person's task the loop had queued; with no `why`
    /// (rm) the journal goes anyway.
    async fn halt(&mut self, name: &str, why: Option<&str>) {
        let queued = match self.tasks.remove(name) {
            Some(t) => {
                t.handle.abort();
                let _ = t.handle.await;
                std::mem::take(&mut *lock(&t.tasks))
            }
            None => Default::default(),
        };
        let Some(why) = why else { return };
        let journal = self.shared.journal(name);
        let mut j = lock(&journal);
        // A task the slot never took has no start line yet.
        for job in queued.iter().filter(|job| !job.again) {
            j.add(job.start());
            j.add(job.end(false, format!("ended: {why}")));
        }
        for e in j.state().interrupted(why) {
            j.add(e);
        }
    }

    /// Writes a service's bundle and starts its loop again; `note` journals it, by `by`.
    async fn run(&mut self, bundle: svc::Bundle, note: &str, by: &str) {
        let name = bundle.spec.name.clone();
        if !valid_name(&name) {
            tracing::warn!(service = %name, "run: no service name");
            return;
        }
        self.halt(&name, Some("the service restarted for new settings"))
            .await;
        if let Err(e) = write_bundle(&bundle) {
            tracing::error!(service = %name, error = %format!("{e:#}"), "run: cannot write the service");
            return;
        }
        self.shared.note(&name, note, by);
        self.broken.remove(&name);
        self.start(&name, true);
    }

    /// A weave change applies as a run again when its rules stay as they are; otherwise
    /// the service's loop raises a signed ask, whose signed yes applies it
    /// (`apply_change`).
    async fn change(&mut self, pending: PendingChange) {
        let name = pending.bundle.spec.name.clone();
        if !valid_name(&name) {
            tracing::warn!(service = %name, "weave change: no service name");
            return;
        }
        let by = pending.by.clone();
        let (bundle, change) = match rebase(&name, &pending) {
            Ok(r) => r,
            Err(why) => {
                let text = format!("weave change by {by} dropped: {why}");
                self.shared.note(&name, &text, &by);
                return;
            }
        };
        if !change.rules() {
            let note = format!("weave changed by {by}: {}; {}", pending.why, change.summary());
            self.run(bundle, &note, &by).await;
            return;
        }
        match self.tasks.get(&name) {
            Some(t) => {
                let _ = t.ctl.send(Ctl::Change { pending, change });
            }
            None => {
                let text = format!("weave change by {by} dropped: the service is stopped");
                self.shared.note(&name, &text, &by);
            }
        }
    }

    /// The signed yes to weave change `n`, journaled, by `by`: the loop stops, and the
    /// change the loop kept applies to the service as it is now. One that no longer fits
    /// is dropped, and the loop starts again.
    async fn apply_change(&mut self, service: &str, n: u64, by: &str) {
        let running = self.tasks.contains_key(service);
        self.halt(service, Some("the service restarted for new settings"))
            .await;
        let path = paths::change(service, n);
        let kept: Option<PendingChange> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let _ = std::fs::remove_file(&path);
        let dropped = match kept {
            None => "nothing kept to apply".to_string(),
            Some(_) if !running => "the service is stopped; propose it again once it runs".into(),
            Some(p) => match rebase(service, &p) {
                Ok((bundle, change)) => {
                    let text = format!(
                        "weave changed by {}: {}; {}; signed yes on #{n} by {by}",
                        p.by,
                        p.why,
                        change.summary()
                    );
                    self.run(bundle, &text, &p.by).await;
                    return;
                }
                Err(why) => why,
            },
        };
        let text = format!("weave change #{n} dropped: {dropped}");
        self.shared.note(service, &text, by);
        if running {
            self.start(service, false);
        }
    }

    async fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Run { bundle, by } => {
                let note = format!("run {}: {}", bundle.spec.weave, bundle.spec.request);
                self.run(bundle, &note, &by).await;
            }
            Command::Change {
                bundle,
                base,
                why,
                by,
            } => {
                self.change(PendingChange {
                    bundle,
                    base,
                    why,
                    by,
                })
                .await
            }
            Command::Tell {
                service,
                request,
                args,
                secrets,
                decide,
                code,
                by,
            } => {
                let Ok(mut spec) = svc::spec(&service) else {
                    tracing::warn!(service = %service, "tell: no such service");
                    return;
                };
                self.halt(&service, Some("the service restarted for new settings"))
                    .await;
                spec.request = request.clone();
                spec.args = args;
                if let Some(d) = decide {
                    spec.decide = d;
                }
                if let Some(c) = code {
                    spec.code = c;
                }
                let saved = save_json(&paths::service_json(&service), &spec).and_then(|()| {
                    let mut all = read_secrets(&service);
                    all.extend(secrets);
                    write_secrets(&service, &all)
                });
                if let Err(e) = saved {
                    tracing::error!(service = %service, error = %format!("{e:#}"), "tell: cannot save");
                    return;
                }
                self.shared.note(&service, &format!("tell: {request}"), &by);
                self.broken.remove(&service);
                if !spec.stopped {
                    self.start(&service, true);
                }
            }
            Command::Do {
                service,
                action,
                by,
            } => match self.tasks.get(&service) {
                Some(t) => {
                    let _ = t.ctl.send(Ctl::Do { action, by });
                }
                None if svc::spec(&service).is_ok() => {
                    let text = format!("do {action}: the service is stopped");
                    self.shared.note(&service, &text, &by);
                }
                None => tracing::warn!(service = %service, action = %action, "do: no such service"),
            },
            Command::Task { service, text, by } => match self.tasks.get(&service) {
                Some(t) => {
                    let _ = t.ctl.send(Ctl::Task { text, by });
                }
                None if svc::spec(&service).is_ok() => {
                    let text = format!("task {text:?}: the service is stopped");
                    self.shared.note(&service, &text, &by);
                }
                None => tracing::warn!(service = %service, "task: no such service"),
            },
            Command::Answer {
                n,
                word,
                by,
                note,
                known,
            } => self.answer(n, &word, &by, note, known, None).await,
            Command::Stop { service, by } => {
                self.halt(&service, Some("the service stopped")).await;
                if let Err(e) = set_stopped(&service, true) {
                    tracing::error!(service = %service, error = %format!("{e:#}"), "stop failed");
                    return;
                }
                self.shared.note(&service, "stopped", &by);
            }
            Command::Start { service, by } => {
                if let Err(e) = set_stopped(&service, false) {
                    tracing::error!(service = %service, error = %format!("{e:#}"), "start failed");
                    return;
                }
                self.broken.remove(&service);
                self.shared.note(&service, "started", &by);
            }
            Command::Rm { service, by: _ } => {
                self.halt(&service, None).await;
                self.broken.remove(&service);
                if valid_name(&service) {
                    if let Err(e) = std::fs::remove_dir_all(paths::service(&service)) {
                        tracing::error!(service = %service, error = %e, "rm failed");
                    }
                    self.shared.forget(&service);
                }
            }
            Command::Forget { service, name, by } => match self.tasks.get(&service) {
                Some(t) => {
                    let _ = t.ctl.send(Ctl::Forget { name, by });
                }
                None => match learned::forget_file(&service, &name) {
                    Ok(true) => self.shared.note(&service, &format!("forgot {name}"), &by),
                    Ok(false) => {
                        let text = format!("forget: nothing learned for {name}");
                        self.shared.note(&service, &text, &by);
                    }
                    Err(e) => tracing::error!(service = %service, error = %format!("{e:#}"), "forget failed"),
                },
            },
            Command::Known {
                service,
                goal,
                line,
                class,
                why,
                by,
            } => {
                if svc::spec(&service).is_err() {
                    tracing::warn!(service = %service, "known: no such service");
                    return;
                }
                lock(&self.shared.journal(&service)).add(Event {
                    by: Some(by).filter(|b| !b.is_empty()),
                    ..Kind::Known {
                        goal,
                        text: line,
                        class,
                        why,
                    }
                    .into()
                });
            }
        }
    }

    /// The first answer to an open escalation settles it; an answer it does not take,
    /// after it settled or unsigned where it needs a signature, is journaled as refused.
    /// `from` is the channel it came on; `note`, when given, follows the answer as a note
    /// by `by`. `known` teaches the box what the escalation's lines are, before the loop
    /// acts on the answer.
    async fn answer(
        &mut self,
        n: u64,
        word: &str,
        by: &str,
        note: Option<String>,
        known: Option<crate::weave::Class>,
        from: Option<String>,
    ) {
        let Some((service, journal)) = self.shared.find(n) else {
            tracing::warn!(ask = n, "answer: no such escalation");
            return;
        };
        // A Steery channel reads only answers signed by a pinned signer.
        let signed = from.as_deref().is_some_and(|name| {
            channel::ls()
                .unwrap_or_default()
                .iter()
                .any(|c| c.name == name && c.kind() == channel::ChannelKind::Steery)
        });
        let ctl = self.tasks.get(&service).map(|t| t.ctl.clone());
        let esc = {
            let mut j = lock(&journal);
            let Some(esc) = j.state().escalations.get(&n).cloned() else {
                return;
            };
            let action = match esc.action.as_deref() {
                Some(a @ (TASK | svc::WEAVE)) => Some(a.to_string()),
                _ => answers_action(&service, word),
            };
            let answer = Kind::Answer {
                n,
                goal: esc.goal.clone(),
                action,
                text: word.to_string(),
                signed,
            };
            let answered = asks::settle(
                &self.shared,
                &mut j,
                Event {
                    by: Some(by.to_string()),
                    ..answer.into()
                },
            );
            if !answered {
                return;
            }
            if let (Some(class), Some(goal)) = (known, &esc.goal) {
                for line in &esc.lines {
                    j.add(Event {
                        by: Some(by.to_string()),
                        ..Kind::Known {
                            goal: goal.clone(),
                            text: line.clone(),
                            class: Some(class),
                            why: note.clone(),
                        }
                        .into()
                    });
                }
            }
            if let Some(text) = note {
                j.note_by(esc.goal.as_deref(), text, by);
            }
            // Sent before the loop can read the escalation settled: it takes the answer
            // before it starts a repair of its own for the goal.
            let weave_yes = esc.action.as_deref() == Some(svc::WEAVE) && word == "yes";
            if let Some(ctl) = ctl.as_ref().filter(|_| !weave_yes) {
                let _ = ctl.send(Ctl::Answer {
                    n,
                    word: word.to_string(),
                    by: by.to_string(),
                });
            }
            esc
        };
        let weave = esc.action.as_deref() == Some(svc::WEAVE);
        match ctl {
            // A signed yes to a weave change: the daemon applies what the loop kept.
            _ if weave && word == "yes" => self.apply_change(&service, n, by).await,
            // The loop has the answer.
            Some(_) => {}
            None if weave => {
                let _ = std::fs::remove_file(paths::change(&service, n));
            }
            // No loop runs the task again: it ends here.
            None => {
                let mut j = lock(&journal);
                let ended = j.state().task_of(n).map(|(ep, t)| Event {
                    by: t.by.clone(),
                    ep: Some(ep.to_string()),
                    ..Kind::Action(Act::Ended {
                        goal: None,
                        action: TASK.to_string(),
                        ok: false,
                        text: Some(format!(
                            "ended: #{n} answered {word} while the service was stopped"
                        )),
                    })
                    .into()
                });
                if let Some(e) = ended {
                    j.add(e);
                }
            }
        }
        let msg = Message {
            kind: channel::Kind::Info,
            host: self.shared.host.clone(),
            service: service.clone(),
            goal: esc.goal.clone(),
            title: format!("#{n} answered {word} by {by}"),
            text: format!("Ask #{n} is settled: {word}, by {by}."),
            summary: Vec::new(),
            facts: BTreeMap::new(),
            ask: None,
        };
        notify(msg, from);
    }
}

/// `word` when it names an action of the service's weave.
fn answers_action(service: &str, word: &str) -> Option<String> {
    let text = std::fs::read_to_string(paths::weavefile(service)).ok()?;
    let w = crate::weave::parse(service, &text).ok()?;
    w.action(word).map(|a| a.name.clone())
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// A weave change on the service as it is now: its spec and README as they stand, its
/// ARGs as they stand plus those only the new weave adds, and what it changes. Refused,
/// with why, when the running Weavefile is no longer the one the change was written
/// against, or a Weavefile does not parse.
fn rebase(
    name: &str,
    p: &PendingChange,
) -> std::result::Result<(svc::Bundle, crate::weave::change::Change), String> {
    let current = std::fs::read_to_string(paths::weavefile(name)).unwrap_or_default();
    if current != p.base {
        return Err(
            "the Weavefile changed since it was read: read it again and propose again".into(),
        );
    }
    let spec = svc::spec(name).map_err(|e| format!("{e:#}"))?;
    let (Ok(old), Ok(new)) = (
        crate::weave::parse(name, &current),
        crate::weave::parse(name, &p.bundle.weavefile),
    ) else {
        return Err("a Weavefile does not parse".into());
    };
    let change = crate::weave::change::compare(&current, &old, &p.bundle.weavefile, &new);
    let mut args = p.bundle.spec.args.clone();
    args.extend(spec.args.clone());
    let bundle = svc::Bundle {
        spec: svc::Spec { args, ..spec },
        weavefile: p.bundle.weavefile.clone(),
        readme: std::fs::read_to_string(paths::readme(name)).ok(),
        secrets: BTreeMap::new(),
    };
    Ok((bundle, change))
}

fn write_bundle(b: &svc::Bundle) -> Result<()> {
    let name = &b.spec.name;
    let dir = paths::service(name);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::write(paths::weavefile(name), &b.weavefile).context("write the Weavefile")?;
    match &b.readme {
        Some(r) => std::fs::write(paths::readme(name), r).context("write README.md")?,
        None => {
            let _ = std::fs::remove_file(paths::readme(name));
        }
    }
    let mut spec = b.spec.clone();
    spec.stopped = false;
    save_json(&paths::service_json(name), &spec)?;
    // A run again keeps the SECRETs it did not ask for.
    let mut secrets = read_secrets(name);
    secrets.extend(b.secrets.clone());
    write_secrets(name, &secrets)
}

fn read_secrets(service: &str) -> BTreeMap<String, String> {
    std::fs::read(paths::secrets(service))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_secrets(service: &str, secrets: &BTreeMap<String, String>) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(secrets)?;
    bytes.push(b'\n');
    channel::write_private(&paths::secrets(service), &bytes)
}

fn set_stopped(service: &str, stopped: bool) -> Result<()> {
    let mut spec = svc::spec(service)?;
    spec.stopped = stopped;
    save_json(&paths::service_json(service), &spec)
}

/// Writes JSON atomically: a temporary file, then a rename.
pub(crate) fn save_json<T: serde::Serialize>(path: &std::path::Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("write {}", path.display()))
}

/// Sends a message to every channel but `except`, in the background.
pub(crate) fn notify(msg: Message, except: Option<String>) {
    tokio::spawn(async move {
        let channels = match channel::ls() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %format!("{e:#}"), "cannot read the channels");
                return;
            }
        };
        for ch in channels {
            if except.as_deref() == Some(ch.name.as_str()) {
                continue;
            }
            if let Err(e) = channel::send(&ch, &msg).await {
                tracing::warn!(service = %msg.service, goal = msg.goal.as_deref(), channel = %ch.name, error = %format!("{e:#}"), "sending a message failed");
            }
        }
    });
}

/// A ping to every heartbeat channel; the channel module keeps it to one a minute.
fn heartbeat() {
    tokio::spawn(async {
        let Ok(channels) = channel::ls() else { return };
        let host = hostname();
        for ch in channels
            .iter()
            .filter(|c| c.kind() == channel::ChannelKind::Heartbeat)
        {
            let msg = Message {
                kind: channel::Kind::Info,
                host: host.clone(),
                service: "subweave".into(),
                goal: None,
                title: "tick".into(),
                text: String::new(),
                summary: Vec::new(),
                facts: BTreeMap::new(),
                ask: None,
            };
            if let Err(e) = channel::send(ch, &msg).await {
                tracing::warn!(channel = %ch.name, error = %format!("{e:#}"), "heartbeat failed");
            }
        }
    });
}

/// RFC 3339 in UTC, to the second.
pub(crate) fn now() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// RFC 3339 in UTC, to the second, of Unix milliseconds `ms`.
pub(crate) fn rfc3339(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .unwrap_or(jiff::Timestamp::MAX)
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Seconds since the epoch of an RFC 3339 time.
pub(crate) fn epoch_of(t: &str) -> Option<i64> {
    t.parse::<jiff::Timestamp>().ok().map(|ts| ts.as_second())
}

/// Milliseconds since `start`: a line's `ms`.
pub(crate) fn ms(start: Instant) -> Option<u64> {
    Some(u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
}

pub(crate) fn epoch() -> i64 {
    jiff::Timestamp::now().as_second()
}

pub(crate) fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname writes at most that much.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    match std::str::from_utf8(&buf[..end]) {
        Ok(h) if rc == 0 && !h.is_empty() => h.to_string(),
        _ => "localhost".into(),
    }
}

/// Replaces each secret's exact value with `[NAME]`. `secrets` holds (name, value),
/// longest value first.
pub(crate) fn redact(text: &str, secrets: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (name, value) in secrets {
        if !value.is_empty() && out.contains(value.as_str()) {
            out = out.replace(value.as_str(), &format!("[{name}]"));
        }
    }
    out
}

/// The last `max` bytes of `text`, from a character boundary.
pub(crate) fn last(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut from = text.len() - max;
    while !text.is_char_boundary(from) {
        from += 1;
    }
    &text[from..]
}

/// At most `max` bytes: the head and the tail, with what was cut said between.
pub(crate) fn cap(text: &str, max: usize) -> String {
    cap_to(text, max, None)
}

/// `cap`, whose cut marker names `file`, where the whole text is kept.
pub(crate) fn cap_to(text: &str, max: usize, file: Option<&std::path::Path>) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let half = max / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    let whole = match file {
        Some(f) => format!("; all of it is in {}", f.display()),
        None => String::new(),
    };
    format!(
        "{}\n…[{} bytes cut{whole}]…\n{}",
        &text[..head],
        tail - head,
        &text[tail..]
    )
}

/// What a shell command did. `code` is `None` when it was killed or timed out.
pub(crate) struct Out {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Out {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout and stderr, as one text.
    pub fn both(&self) -> String {
        let (o, e) = (self.stdout.trim_end(), self.stderr.trim_end());
        match (o.is_empty(), e.is_empty()) {
            (_, true) => o.to_string(),
            (true, false) => e.to_string(),
            (false, false) => format!("{o}\n{e}"),
        }
    }
}

/// `bash -eo pipefail -c <script>` with `env` added, stdin closed, in a process group
/// of its own so that `kill_group` ends what it started too.
pub(crate) fn bash(script: &str, env: &BTreeMap<String, String>) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.args(["-eo", "pipefail", "-c", script])
        .envs(env)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    cmd
}

/// Kills the process group a `bash` command leads.
pub(crate) fn kill_group(pid: u32) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: kill has no memory preconditions; the group is the command's own.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
}

/// Runs `script` in `bash -eo pipefail -c` with `env` added, killed after `limit`.
pub(crate) async fn sh(script: &str, env: &BTreeMap<String, String>, limit: Duration) -> Out {
    let mut cmd = bash(script, env);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Out {
                code: None,
                stdout: String::new(),
                stderr: format!("bash: {e}"),
            };
        }
    };
    let pid = child.id();
    let (out, mut out_task) = drain(child.stdout.take());
    let (err, mut err_task) = drain(child.stderr.take());
    let mut timed_out = false;
    let code = match tokio::time::timeout(limit, child.wait()).await {
        Ok(Ok(status)) => status.code(),
        Ok(Err(_)) => None,
        Err(_) => {
            timed_out = true;
            if let Some(pid) = pid {
                kill_group(pid);
            }
            let _ = child.kill().await;
            None
        }
    };
    // A command that started a server in the background leaves it holding the pipes:
    // take what came within a second and leave the readers draining.
    let grace = Duration::from_secs(1);
    let _ = tokio::time::timeout(grace, &mut out_task).await;
    let _ = tokio::time::timeout(grace, &mut err_task).await;
    let take = |b: &Arc<Mutex<Vec<u8>>>| {
        String::from_utf8_lossy(&b.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    };
    let mut stderr = take(&err);
    if timed_out {
        stderr.push_str(&format!("\n[killed after {}s]", limit.as_secs()));
    }
    Out {
        code,
        stdout: take(&out),
        stderr,
    }
}

/// Reads a pipe into a shared buffer (first MiB kept) until it closes.
fn drain<R: AsyncRead + Unpin + Send + 'static>(
    pipe: Option<R>,
) -> (Arc<Mutex<Vec<u8>>>, JoinHandle<()>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let b = buf.clone();
    let task = tokio::spawn(async move {
        let Some(mut p) = pipe else { return };
        let mut chunk = [0u8; 8192];
        loop {
            match p.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut g = b.lock().unwrap_or_else(|e| e.into_inner());
                    if g.len() < 1 << 20 {
                        g.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    });
    (buf, task)
}
