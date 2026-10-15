//! `subweave weave <service>`: the running Weavefile and its ARGs, what an agent reads
//! first. `--change` proposes a new Weavefile: the one way an agent changes it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

use crate::channel::{self, ChannelKind};
use crate::model::{ModelRef, decider::Decider};
use crate::plan::{self, Level};
use crate::service::{self, Bundle, Command, Spec};
use crate::weave::{self, change::Change};
use crate::paths;

#[derive(clap::Args)]
pub struct Args {
    pub service: String,
    /// Propose this Weavefile instead: a path, or - for stdin
    #[arg(long, value_name = "FILE")]
    change: Option<String>,
    /// Why, in a sentence; the journal and the person's card show it
    #[arg(long, requires = "change")]
    why: Option<String>,
    /// Run on this box, over ssh
    #[arg(long, value_name = "USER@HOST")]
    pub on: Option<String>,
}

pub async fn run(args: Args) -> Result<ExitCode> {
    let Some(file) = args.change else {
        print!("{}", show(&args.service)?.text());
        return Ok(ExitCode::SUCCESS);
    };
    let text = if file == "-" {
        let mut s = String::new();
        std::io::stdin()
            .lock()
            .read_to_string(&mut s)
            .context("read the Weavefile from stdin")?;
        s
    } else {
        std::fs::read_to_string(&file).with_context(|| format!("read {file}"))?
    };
    let why = args.why.unwrap_or_default();
    let proposed = propose(&args.service, &text, &why, super::by()).await?;
    print!("{}", proposed.text(&args.service));
    Ok(ExitCode::SUCCESS)
}

/// The running weave as `subweave weave` shows it.
#[derive(Serialize)]
pub(crate) struct Shown {
    pub weavefile: String,
    /// ARGs and slots, as the service runs with them.
    pub args: BTreeMap<String, String>,
    /// Where the weave came from: a library name like `postgres:0.5`, or a path.
    pub source: String,
}

impl Shown {
    fn text(&self) -> String {
        let mut out = self.weavefile.clone();
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\nARGs:\n");
        for (name, value) in &self.args {
            let _ = writeln!(out, "{name}={value}");
        }
        let _ = writeln!(out, "\nsource: {}", self.source);
        out
    }
}

pub(crate) fn show(service: &str) -> Result<Shown> {
    super::known(service)?;
    let spec = service::spec(service)?;
    let file = paths::weavefile(service);
    let weavefile =
        std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
    Ok(Shown {
        weavefile,
        args: spec.args,
        source: spec.weave,
    })
}

/// A change handed to the daemon.
#[derive(Serialize)]
pub(crate) struct Proposed {
    /// The rules change: it waits for the person's signed answer on Steery.
    pub signed: bool,
    #[serde(flatten)]
    pub change: Change,
}

impl Proposed {
    fn text(&self, service: &str) -> String {
        if !self.signed {
            return format!(
                "handed the change to {service} to the daemon: {}\n",
                self.change.summary()
            );
        }
        let mut out = format!(
            "the rules of {service} change: an escalation asks the person for a signed answer on Steery\n"
        );
        for l in self.change.diff() {
            let _ = writeln!(out, "{l}");
        }
        if !self.change.free.is_empty() {
            let _ = writeln!(out, "and: {}", self.change.summary());
        }
        out
    }
}

/// Checks `text` as the service's new Weavefile, then hands it to the daemon, which
/// applies it at once when only facts, goals, actions and skills change. The check is
/// `run`'s: it must parse, and the request check must pass with the service's ARGs and
/// SECRETs, with no request. `why` and `by` go in the journal.
pub(crate) async fn propose(service: &str, text: &str, why: &str, by: String) -> Result<Proposed> {
    let why = why.trim();
    if why.is_empty() {
        bail!("say why, in a sentence");
    }
    super::known(service)?;
    let spec = service::spec(service)?;
    if spec.stopped {
        bail!("{service} is stopped: start it first");
    }
    let file = paths::weavefile(service);
    let old_text =
        std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
    let lines = |problems: Vec<weave::Problem>| {
        problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    };
    let old = weave::parse(service, &old_text)
        .map_err(|p| anyhow!("the running Weavefile does not parse:\n{}", lines(p)))?;
    let new = weave::parse(service, text)
        .map_err(|p| anyhow!("the new Weavefile does not parse:\n{}", lines(p)))?;
    let composed = weave::compose::check(service, &new, service::weaves()?);
    if !composed.is_empty() {
        bail!(
            "the new Weavefile does not fit the other services on this box:\n{}",
            lines(composed)
        );
    }
    let decider = Decider::new(ModelRef::parse(&spec.decide)?)?;
    let plan = plan::plan(&new, "", &decider, Some(&spec.args)).await?;
    if plan.count(Level::Error) > 0 {
        bail!(
            "the request check refuses the new Weavefile:\n{}",
            plan::render(&plan)
        );
    }
    let have = super::tell::stored_secrets(service);
    let missing: Vec<&str> = plan
        .secrets
        .iter()
        .filter(|s| !have.contains_key(*s))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        bail!(
            "the new Weavefile needs SECRET {}, which this box does not hold: only subweave run at a terminal sets it",
            missing.join(", ")
        );
    }
    let change = weave::change::compare(&old_text, &old, text, &new);
    let signed = change.rules();
    let steery = channel::ls()?
        .iter()
        .any(|c| c.kind() == ChannelKind::Steery);
    if signed && !steery {
        bail!(
            "the rules of {service} change, and only a signed answer on Steery settles that: this box has no Steery channel"
        );
    }
    let bundle = Bundle {
        spec: Spec {
            args: plan.values,
            stopped: false,
            ..spec
        },
        weavefile: text.to_owned(),
        readme: std::fs::read_to_string(paths::readme(service)).ok(),
        secrets: BTreeMap::new(),
    };
    service::put(&Command::Change {
        bundle,
        base: old_text,
        why: why.to_owned(),
        by,
    })?;
    Ok(Proposed { signed, change })
}
