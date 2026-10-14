//! What the CLI and the daemon share on disk. The CLI writes only `inbox/`; the daemon writes
//! everything under `services/` and is the only reader of `inbox/`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;

/// The `action` of every journal line of a person's task; no ACTION may take the name.
pub const TASK: &str = "task";
/// The `text` of the `answer` line that closes an ask whose goal holds again.
pub const WITHDRAWN: &str = "withdrawn";

/// `services/<name>/service.json`: how a service was asked for, as the daemon keeps it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Spec {
    pub name: String,
    /// The weave as the person named it: `postgres:0.5`, or a path on the machine that ran `run`.
    pub weave: String,
    /// The last request said, by `run` or `tell`.
    pub request: String,
    /// ARGs and slots after the request check. An ARG with no value is absent.
    pub args: BTreeMap<String, String>,
    /// `<id>[@<provider>]`.
    pub decide: String,
    /// The coder chain, `THEN` order.
    pub code: Vec<String>,
    /// RFC 3339.
    pub created: String,
    pub stopped: bool,
}

/// Everything `run` hands the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub spec: Spec,
    pub weavefile: String,
    pub readme: Option<String>,
    /// SECRET name → value. The daemon writes them to `secrets.json`, mode 0600.
    pub secrets: BTreeMap<String, String>,
}

/// One file in `inbox/`. `by`: who sent it, `cli:<user>` or `dash@<ip>`; lines it
/// causes carry it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Command {
    /// A new service, or a service run again under the same name (its journal and learned
    /// commands stay).
    Run {
        bundle: Bundle,
        #[serde(default)]
        by: String,
    },
    /// New ARGs, after the request check; `decide`/`code` replace the models when given.
    /// `secrets`: SECRETs the new ARGs turn on, merged into `secrets.json`.
    Tell {
        service: String,
        request: String,
        args: BTreeMap<String, String>,
        secrets: BTreeMap<String, String>,
        decide: Option<String>,
        code: Option<Vec<String>>,
        #[serde(default)]
        by: String,
    },
    /// Run one action now, through the same rules.
    Do {
        service: String,
        action: String,
        #[serde(default)]
        by: String,
    },
    /// A task in words for the coder, with no goal, through the same rules.
    Task {
        service: String,
        text: String,
        by: String,
    },
    /// `word`: `yes`, `no`, `always`, `never` or an action name.
    Answer {
        n: u64,
        word: String,
        by: String,
    },
    Stop {
        service: String,
        #[serde(default)]
        by: String,
    },
    Start {
        service: String,
        #[serde(default)]
        by: String,
    },
    Rm {
        service: String,
        #[serde(default)]
        by: String,
    },
    /// Drop a learned fact command or learned steps (`<action>` drops every goal's steps).
    Forget {
        service: String,
        name: String,
        #[serde(default)]
        by: String,
    },
}

/// One line of `journal.jsonl`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Line {
    /// RFC 3339, UTC.
    pub t: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// `check`: the facts the goal read. `ask`: the facts shown to the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<BTreeMap<String, String>>,
    /// `decide`: the probability of each choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<BTreeMap<String, f64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// `ask` and `answer`: the ask's number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u64>,
    /// `ask`: the words that answer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<Vec<String>>,
    /// `ask`: what it says beyond its title, as the channels get it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `command`: stdout and stderr, secrets redacted, the last 4 KB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Who acted, on lines the person causes: `cli:<user>`, `dash@<ip>`, a channel's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The episode the line belongs to: one job run (a repair or a person's task) or one
    /// learn outside a job. Lines outside every episode have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ep: Option<String>,
    /// What a model conversation used, on the line that ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// `id[@provider]`.
    pub model: String,
    pub calls: u32,
    /// Input tokens over every call; `cached` of them read from the provider's cache.
    pub input: u64,
    pub cached: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A goal checked: `ok` holds or not; no `ok` is unknown.
    #[default]
    Check,
    /// The decider chose: `text` the choice, `p` the probabilities.
    Decide,
    /// A rule refused or held an action: `text` the rule.
    Gate,
    /// An action ended: `ok` the goal held within its time.
    Action,
    /// One shell command: `text` the command, `ok` exit 0.
    Command,
    /// A fact command or steps learned: `text` what.
    Learn,
    Ask,
    /// `text` the word, `action` set when the word named one.
    Answer,
    /// A message sent to the channels: `text` its title.
    Message,
    Note,
}

/// `services/<name>/learned.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Learned {
    /// Fact name → the command this box learned for it.
    pub facts: BTreeMap<String, LearnedFact>,
    /// `<action>/<goal>` → the commands that made the goal hold.
    pub steps: BTreeMap<String, LearnedSteps>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedFact {
    pub command: String,
    /// sha256 hex of the fact's quoted text after `$name` replacement: a changed ARG learns again.
    pub key: String,
    pub t: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedSteps {
    pub commands: Vec<String>,
    /// sha256 hex of the action's DO texts after `$name` replacement.
    pub key: String,
    pub t: String,
}

/// `services/<name>/status.json`, rewritten by the daemon after every tick.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Status {
    pub service: String,
    pub t: String,
    /// Graph order.
    pub goals: Vec<GoalStatus>,
    /// The action running now and its goal, as `restart for running`.
    pub repair: Option<String>,
    /// Open asks, by number.
    pub asks: Vec<u64>,
    pub spend_month_usd: f64,
    pub decide: String,
    pub code: Vec<String>,
    pub stopped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalStatus {
    pub name: String,
    pub state: GoalState,
    /// When the goal last changed state.
    pub since: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalState {
    Holds,
    Fails,
    /// Not checked yet, or a quoted goal the decider could not answer.
    Unknown,
    /// Its IF is false, or a goal it comes AFTER is off.
    Off,
    /// A goal it comes AFTER does not hold.
    Waiting,
}

/// Drop a command into the daemon's inbox, atomically.
pub fn put(cmd: &Command) -> Result<()> {
    let dir = paths::inbox();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let id = uuid::Uuid::now_v7();
    let tmp = dir.join(format!(".{id}.tmp"));
    let mut f = std::fs::File::create(&tmp).with_context(|| format!("write {}", tmp.display()))?;
    f.write_all(&serde_json::to_vec(cmd)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join(format!("{id}.json")))?;
    Ok(())
}

/// Every line of a service's journal; lines that do not parse are skipped.
pub fn journal(service: &str) -> Result<Vec<Line>> {
    read_lines(&paths::journal(service))
}

fn read_lines(path: &Path) -> Result<Vec<Line>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect())
}

/// Asks in a journal that no `answer` line settled yet, oldest first.
pub fn open_asks(lines: &[Line]) -> Vec<&Line> {
    let answered: std::collections::BTreeSet<u64> = lines
        .iter()
        .filter(|l| l.kind == Kind::Answer)
        .filter_map(|l| l.n)
        .collect();
    lines
        .iter()
        .filter(|l| l.kind == Kind::Ask && l.n.is_some_and(|n| !answered.contains(&n)))
        .collect()
}

/// Every line of a service's journal with its index among the file's non-blank lines;
/// lines that do not parse are skipped, and keep their index.
pub fn journal_numbered(service: &str) -> Result<Vec<(u64, Line)>> {
    let path = paths::journal(service);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
    };
    Ok(numbered_lines(&text)
        .enumerate()
        .filter_map(|(i, l)| Some((i as u64, serde_json::from_str(l).ok()?)))
        .collect())
}

/// The lines of a journal that count: every line but blank ones. A line's index among
/// them is its `i` in the dashboard.
pub fn numbered_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().filter(|l| !l.trim().is_empty())
}

/// An open ask, as `subweave inbox` and the dashboard show it.
#[derive(Debug, Clone, Serialize)]
pub struct Ask {
    pub n: u64,
    pub service: String,
    pub goal: Option<String>,
    pub action: Option<String>,
    /// The rule that holds the action, from the ask's `gate` line.
    pub rule: Option<String>,
    pub title: String,
    pub text: String,
    pub facts: BTreeMap<String, String>,
    pub answers: Vec<String>,
    pub t: String,
}

/// A service's open asks, oldest first. The rule reads as the box reads it, `$name`
/// replaced; the gate line keeps it as written, the key of `always` and `never`.
pub fn asks(service: &str, lines: &[Line]) -> Vec<Ask> {
    let args = spec(service).map(|s| s.args).unwrap_or_default();
    open_asks(lines)
        .into_iter()
        .filter_map(|l| {
            let n = l.n?;
            let rule = lines
                .iter()
                .find(|g| g.kind == Kind::Gate && g.n == Some(n))
                .and_then(|g| g.text.as_deref())
                .map(|t| crate::weave::subst(t, &args));
            Some(Ask {
                n,
                service: service.to_string(),
                goal: l.goal.clone(),
                action: l.action.clone(),
                rule,
                title: l.text.clone().unwrap_or_default(),
                text: l.body.clone().unwrap_or_default(),
                facts: l.facts.clone().unwrap_or_default(),
                answers: l.answers.clone().unwrap_or_default(),
                t: l.t.clone(),
            })
        })
        .collect()
}

/// Every open ask on this box, by number.
pub fn all_asks() -> Result<Vec<Ask>> {
    let mut out = Vec::new();
    for name in names()? {
        out.extend(asks(&name, &journal(&name)?));
    }
    out.sort_by_key(|a| a.n);
    Ok(out)
}

/// Every service the daemon keeps, by name.
pub fn names() -> Result<Vec<String>> {
    let dir = paths::services();
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("read {}", dir.display())),
    };
    for e in entries {
        let e = e?;
        if e.path().join("service.json").is_file() {
            out.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    out.sort();
    Ok(out)
}

pub fn spec(service: &str) -> Result<Spec> {
    read_json(&paths::service_json(service))
}

pub fn status(service: &str) -> Result<Option<Status>> {
    let p = paths::status(service);
    if !p.exists() {
        return Ok(None);
    }
    read_json(&p).map(Some)
}

pub fn learned(service: &str) -> Result<Learned> {
    let p = paths::learned(service);
    if !p.exists() {
        return Ok(Learned::default());
    }
    read_json(&p)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}
