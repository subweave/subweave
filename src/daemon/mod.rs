//! `subweave daemon`: consumes the inbox, keeps one loop task per service and polls the
//! answering channels for open asks. `WEAVEFILE.md` "The loop" says what a loop does.

mod asks;
mod coder_task;
mod facts;
mod goals;
mod inbox;
mod journal;
mod learned;
mod repair;
mod service;

use std::collections::{BTreeMap, BTreeSet};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::channel::{self, Message};
use crate::paths;
use crate::service::{self as svc, Command, Kind, Line};

use service::Ctl;

/// What every service task shares: the ask counter and where each ask was sent.
pub(crate) struct Shared {
    pub host: String,
    ask_lock: Mutex<()>,
    sent: Mutex<BTreeMap<u64, asks::SentAsk>>,
    /// Asks settled in this run, so a second answer is ignored.
    answered: Mutex<BTreeSet<u64>>,
}

struct Task {
    handle: JoinHandle<()>,
    ctl: mpsc::UnboundedSender<Ctl>,
}

struct Daemon {
    shared: Arc<Shared>,
    tasks: BTreeMap<String, Task>,
    /// Services whose task ended on an error; they start again on their next command.
    broken: BTreeSet<String>,
}

/// Runs every service in the foreground until SIGINT or SIGTERM.
pub async fn run() -> Result<()> {
    for dir in [paths::inbox(), paths::services()] {
        std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let shared = Arc::new(Shared {
        host: hostname(),
        ask_lock: Mutex::new(()),
        sent: Mutex::new(asks::load_sent()),
        answered: Mutex::new(BTreeSet::new()),
    });
    let (answers_tx, mut answers_rx) = mpsc::unbounded_channel();
    tokio::spawn(asks::poll_loop(shared.clone(), answers_tx));
    let mut d = Daemon {
        shared,
        tasks: BTreeMap::new(),
        broken: BTreeSet::new(),
    };
    eprintln!(
        "subweave daemon: state {}, config {}",
        paths::state().display(),
        paths::config().display()
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
                d.answer(answer.n, &answer.word, &answer.by, Some(from)).await;
                continue;
            }
            _ = tokio::signal::ctrl_c() => break,
            _ = term.recv() => break,
        }
        for (path, cmd) in inbox::take() {
            match cmd {
                Ok(cmd) => d.apply(cmd).await,
                Err(e) => eprintln!("inbox {}: {e:#}", path.display()),
            }
            if let Err(e) = std::fs::remove_file(&path) {
                eprintln!("remove {}: {e}", path.display());
            }
        }
        d.keep();
        heartbeat();
    }
    for (_, t) in std::mem::take(&mut d.tasks) {
        t.handle.abort();
        let _ = t.handle.await;
    }
    Ok(())
}

impl Daemon {
    /// Starts a task for every service that is not stopped and has none.
    fn keep(&mut self) {
        let names = match svc::names() {
            Ok(n) => n,
            Err(e) => {
                eprintln!("{e:#}");
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
                eprintln!("{name}: its loop ended; it starts again on its next command");
            }
            if self.broken.contains(&name) {
                continue;
            }
            match svc::spec(&name) {
                Ok(spec) if !spec.stopped => self.start(&name, false),
                Ok(_) => {}
                Err(e) => {
                    eprintln!("{name}: {e:#}");
                    self.broken.insert(name);
                }
            }
        }
    }

    fn start(&mut self, name: &str, announce: bool) {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(service::run(
            self.shared.clone(),
            name.to_string(),
            rx,
            announce,
        ));
        self.tasks
            .insert(name.to_string(), Task { handle, ctl: tx });
    }

    async fn halt(&mut self, name: &str) {
        if let Some(t) = self.tasks.remove(name) {
            t.handle.abort();
            let _ = t.handle.await;
        }
    }

    async fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Run { bundle } => {
                let name = bundle.spec.name.clone();
                if !valid_name(&name) {
                    eprintln!("run: {name:?} is no service name");
                    return;
                }
                self.halt(&name).await;
                if let Err(e) = write_bundle(&bundle) {
                    eprintln!("run {name}: {e:#}");
                    return;
                }
                journal::note(
                    &name,
                    &format!("run {}: {}", bundle.spec.weave, bundle.spec.request),
                );
                self.broken.remove(&name);
                self.start(&name, true);
            }
            Command::Tell {
                service,
                request,
                args,
                secrets,
                decide,
                code,
            } => {
                let Ok(mut spec) = svc::spec(&service) else {
                    eprintln!("tell: no service {service}");
                    return;
                };
                self.halt(&service).await;
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
                    eprintln!("tell {service}: {e:#}");
                    return;
                }
                journal::note(&service, &format!("tell: {request}"));
                self.broken.remove(&service);
                if !spec.stopped {
                    self.start(&service, true);
                }
            }
            Command::Do { service, action } => match self.tasks.get(&service) {
                Some(t) => {
                    let _ = t.ctl.send(Ctl::Do(action));
                }
                None if svc::spec(&service).is_ok() => {
                    journal::note(&service, &format!("do {action}: the service is stopped"))
                }
                None => eprintln!("do: no service {service}"),
            },
            Command::Answer { n, word, by } => self.answer(n, &word, &by, None).await,
            Command::Stop { service } => {
                self.halt(&service).await;
                if let Err(e) = set_stopped(&service, true) {
                    eprintln!("stop {service}: {e:#}");
                    return;
                }
                journal::note(&service, "stopped");
            }
            Command::Start { service } => {
                if let Err(e) = set_stopped(&service, false) {
                    eprintln!("start {service}: {e:#}");
                    return;
                }
                self.broken.remove(&service);
                journal::note(&service, "started");
            }
            Command::Rm { service } => {
                self.halt(&service).await;
                self.broken.remove(&service);
                if valid_name(&service) {
                    if let Err(e) = std::fs::remove_dir_all(paths::service(&service)) {
                        eprintln!("rm {service}: {e}");
                    }
                }
            }
            Command::Forget { service, name } => match self.tasks.get(&service) {
                Some(t) => {
                    let _ = t.ctl.send(Ctl::Forget(name));
                }
                None => match learned::forget_file(&service, &name) {
                    Ok(true) => journal::note(&service, &format!("forgot {name}")),
                    Ok(false) => {
                        journal::note(&service, &format!("forget: nothing learned for {name}"))
                    }
                    Err(e) => eprintln!("forget {service}: {e:#}"),
                },
            },
        }
    }

    /// The first answer to an open ask wins; `from` is the channel it came on.
    async fn answer(&mut self, n: u64, word: &str, by: &str, from: Option<String>) {
        if self
            .shared
            .answered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&n)
        {
            return;
        }
        let Some((service, ask)) = find_ask(n) else {
            eprintln!("answer: no open ask #{n}");
            return;
        };
        let answers = ask.answers.clone().unwrap_or_default();
        if !answers.iter().any(|a| a == word) {
            eprintln!("answer #{n}: {word:?} is not one of {}", answers.join(", "));
            return;
        }
        self.shared
            .answered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(n);
        asks::forget_sent(&self.shared, n);
        match self.tasks.get(&service) {
            Some(t) => {
                let _ = t.ctl.send(Ctl::Answer {
                    n,
                    word: word.to_string(),
                });
            }
            None => journal::append(
                &paths::journal(&service),
                &Line {
                    t: now(),
                    kind: Kind::Answer,
                    goal: ask.goal.clone(),
                    action: answers_action(&service, word),
                    text: Some(word.to_string()),
                    n: Some(n),
                    ..Default::default()
                },
            ),
        }
        let msg = Message {
            kind: channel::Kind::Info,
            host: self.shared.host.clone(),
            service: service.clone(),
            goal: ask.goal.clone(),
            title: format!("#{n} answered {word} by {by}"),
            text: format!("Ask #{n} is settled: {word}, by {by}."),
            facts: BTreeMap::new(),
            ask: None,
        };
        notify(msg, from);
    }
}

/// The service and ask line of open ask `n`.
fn find_ask(n: u64) -> Option<(String, Line)> {
    for name in svc::names().ok()? {
        let lines = svc::journal(&name).unwrap_or_default();
        if let Some(ask) = svc::open_asks(&lines).into_iter().find(|l| l.n == Some(n)) {
            return Some((name, ask.clone()));
        }
    }
    None
}

/// `word` when it names an action of the service's weave.
fn answers_action(service: &str, word: &str) -> Option<String> {
    let text = std::fs::read_to_string(paths::weavefile(service)).ok()?;
    let w = crate::weave::parse(service, &text).ok()?;
    w.action(word).map(|a| a.name.clone())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
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
    write_secrets(name, &b.secrets)
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
    save_json(&paths::service_json(service), &spec)?;
    if let Ok(Some(mut status)) = svc::status(service) {
        status.stopped = stopped;
        status.repair = None;
        status.t = now();
        save_json(&paths::status(service), &status)?;
    }
    Ok(())
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
                eprintln!("channels: {e:#}");
                return;
            }
        };
        for ch in channels {
            if except.as_deref() == Some(ch.name.as_str()) {
                continue;
            }
            if let Err(e) = channel::send(&ch, &msg).await {
                eprintln!("{e:#}");
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
            .filter(|c| c.kind == channel::ChannelKind::Heartbeat)
        {
            let msg = Message {
                kind: channel::Kind::Info,
                host: host.clone(),
                service: "subweave".into(),
                goal: None,
                title: "tick".into(),
                text: String::new(),
                facts: BTreeMap::new(),
                ask: None,
            };
            if let Err(e) = channel::send(ch, &msg).await {
                eprintln!("{e:#}");
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

/// Seconds since the epoch of an RFC 3339 time.
pub(crate) fn epoch_of(t: &str) -> Option<i64> {
    t.parse::<jiff::Timestamp>().ok().map(|ts| ts.as_second())
}

pub(crate) fn epoch() -> i64 {
    jiff::Timestamp::now().as_second()
}

fn hostname() -> String {
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

/// At most `max` bytes: the head and the tail, with what was cut said between.
pub(crate) fn cap(text: &str, max: usize) -> String {
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
    format!(
        "{}\n…[{} bytes cut]…\n{}",
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

/// Runs `script` in `bash -eo pipefail -c` with `env` added, killed after `limit`.
pub(crate) async fn sh(script: &str, env: &BTreeMap<String, String>, limit: Duration) -> Out {
    let mut cmd = tokio::process::Command::new("bash");
    cmd.args(["-eo", "pipefail", "-c", script])
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
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
                // SAFETY: kill has no memory preconditions; the group is the command's own.
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
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
