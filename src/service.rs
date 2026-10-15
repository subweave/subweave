//! What the CLI and the daemon share on disk. The CLI writes only `inbox/`; the daemon writes
//! everything under `services/` and is the only reader of `inbox/`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::weave::{self, Class, Weave};

/// The `action` of every journal line of a person's task; no ACTION may take the name.
pub const TASK: &str = "task";
/// The `action` of the ask and answer lines of a weave change; no ACTION may take the name.
pub const WEAVE: &str = "weave";
/// The `action` of a failed drill's escalation; no ACTION may take the name.
pub const DRILL: &str = "drill";

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
    /// Run one DRILL now.
    Drill {
        service: String,
        drill: String,
        by: String,
    },
    /// `word`: `yes`, `no`, `always`, `never` or an action name. `note`: the answerer's
    /// words, journaled as a note by `by` after the answer line. `known`: what the
    /// escalation's lines are, taught to the box with the answer.
    Answer {
        n: u64,
        word: String,
        by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        known: Option<Class>,
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
    /// Lines like `line` are `class` for `goal`; none: the box forgets what it knew of
    /// them.
    Known {
        service: String,
        goal: String,
        line: String,
        class: Option<Class>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        by: String,
    },
    /// Release `version` of PART `part` is a security fix, or not.
    Release {
        service: String,
        part: String,
        version: String,
        security: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        by: String,
    },
    /// A new Weavefile an agent proposed, checked by `weave --change` and bundled as `run`
    /// bundles a service run again. `base`: the Weavefile it was written against; the
    /// daemon drops the change once the running one differs. It applies at once when the
    /// rules stay as they are; otherwise a signed ask decides, and the daemon applies it
    /// on that ask's signed yes.
    Change {
        bundle: Bundle,
        base: String,
        why: String,
        by: String,
    },
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

/// The weave each service the daemon keeps runs, by name, stopped ones too: what other
/// services' references name. One whose Weavefile does not parse runs nothing.
pub fn weaves() -> Result<BTreeMap<String, Weave>> {
    Ok(names()?
        .into_iter()
        .filter_map(|name| {
            let text = std::fs::read_to_string(paths::weavefile(&name)).ok()?;
            let w = weave::parse(&name, &text).ok()?;
            Some((name, w))
        })
        .collect())
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
