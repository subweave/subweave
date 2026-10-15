use std::collections::BTreeSet;
use std::time::Duration;

use serde::{Deserialize, Serialize, Serializer};

use super::expr::Expr;
use super::value::FactType;

#[derive(Debug, Clone, Serialize)]
pub struct Weave {
    pub name: String,
    pub about: Option<String>,
    pub from: Option<From>,
    pub needs: Needs,
    pub model: ModelHint,
    pub args: Vec<Arg>,
    pub secrets: Vec<Secret>,
    pub facts: Vec<Fact>,
    /// Components with releases; each adds its derived facts to `facts`.
    pub parts: Vec<Part>,
    pub goals: Vec<Goal>,
    pub actions: Vec<Action>,
    pub rules: Vec<Rule>,
    pub skills: Vec<Skill>,
    /// The author's verdicts on kinds of log lines.
    pub known: Vec<Known>,
    /// Failures the box causes on purpose, to prove a goal notices them and the loop
    /// repairs them.
    pub drills: Vec<Drill>,
}

/// `DRILL <name> "<label>" BREAK <command> EXPECT <goal> FAILS WITHIN <d> HOLDS WITHIN <d>
/// [EVERY <d>] [IF <expr>]`, and an optional `RESTORE <command>` line under it.
#[derive(Debug, Clone, Serialize)]
pub struct Drill {
    pub name: String,
    /// What it breaks, for a person: `DRILL cl-down "Stop the consensus client" …`.
    pub label: String,
    /// The command that causes the failure.
    #[serde(rename = "break")]
    pub breaks: String,
    /// The goal that must fail, then hold again.
    pub expect: String,
    /// From the end of BREAK, how soon the goal must fail.
    #[serde(serialize_with = "dur")]
    pub fails_within: Duration,
    /// From the end of BREAK, how soon it must hold again.
    #[serde(serialize_with = "dur")]
    pub holds_within: Duration,
    /// How often the daemon runs it on its own; `None`: only by hand.
    #[serde(serialize_with = "dur_opt")]
    pub every: Option<Duration>,
    /// A precondition: while false or unknown it does not run.
    pub when: Option<Expr>,
    /// Puts back what BREAK did, when the loop did not repair it.
    pub restore: Option<String>,
    pub line: u32,
    /// The last physical line of the DRILL and its RESTORE.
    #[serde(skip)]
    pub end: u32,
}

/// `KNOWN <goal> NORMAL|TROUBLE "<sample line>"`: lines of the sample's kind are routine
/// for the goal, or trouble.
#[derive(Debug, Clone, Serialize)]
pub struct Known {
    pub goal: String,
    pub class: Class,
    pub sample: String,
    pub line: u32,
}

/// What a kind of log line shows a goal: nothing that needs action, or trouble.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Class {
    Normal,
    Trouble,
}

impl Class {
    /// `normal` or `trouble`, as the CLI and the journal spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Normal => "normal",
            Class::Trouble => "trouble",
        }
    }

    /// `NORMAL` or `TROUBLE`, as a Weavefile spells it.
    pub fn keyword(self) -> &'static str {
        match self {
            Class::Normal => "NORMAL",
            Class::Trouble => "TROUBLE",
        }
    }

    pub fn parse(word: &str) -> Option<Class> {
        match word {
            "normal" | "NORMAL" => Some(Class::Normal),
            "trouble" | "TROUBLE" => Some(Class::Trouble),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct From {
    pub os: String,
    /// `None`: any version of `os`.
    pub version: Option<String>,
    pub line: u32,
}

impl std::fmt::Display for From {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.version {
            Some(v) => write!(f, "{}:{v}", self.os),
            None => f.write_str(&self.os),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Needs {
    pub cpu: Option<u32>,
    pub gpu: Option<u32>,
    pub ram_gb: Option<f64>,
    pub disk_gb: Option<f64>,
    pub line: u32,
}

/// `MODEL CODE a THEN b` gives `code: [a, b]`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelHint {
    pub decide: Option<String>,
    pub code: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Level {
    Must,
    Should,
    May,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Must => "MUST",
            Level::Should => "SHOULD",
            Level::May => "MAY",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Arg {
    pub name: String,
    pub default: Option<String>,
    pub level: Level,
    pub kind: ArgKind,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ArgKind {
    Value { what: String },
    Options(Vec<ArgOption>),
}

/// `last <blocks> blocks` has slots `["blocks"]`.
#[derive(Debug, Clone, Serialize)]
pub struct ArgOption {
    pub label: String,
    pub meaning: String,
    pub slots: Vec<String>,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Secret {
    pub name: String,
    pub what: String,
    pub cond: Option<Expr>,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: FactType,
    pub source: FactSource,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FactSource {
    Command(String),
    Quoted(String),
    /// `<part>.<field>`: derived from a PART's releases and running version.
    Part { part: String, field: PartField },
}

/// `PART <name> <source> VERSION <command>`: a component with releases.
#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub name: String,
    pub source: PartSource,
    /// Prints the running version.
    pub version: String,
    pub line: u32,
}

/// Where a part's releases are listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartSource {
    /// `github:<org>/<repo>`: its GitHub releases, with their notes.
    Github { org: String, repo: String },
    /// `docker:<image>`: its image tags, which carry no notes.
    Docker { image: String },
    /// `apt:<package>`: the versions apt can install, with their changelog entries.
    Apt { package: String },
}

impl PartSource {
    pub fn parse(s: &str) -> Result<PartSource, String> {
        const FORM: &str = "github:<org>/<repo>, docker:<image> or apt:<package>";
        let (kind, rest) = s.split_once(':').ok_or_else(|| format!("`{s}` is no source: {FORM}"))?;
        let word = |w: &str| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        match kind {
            "github" => match rest.split_once('/') {
                Some((org, repo)) if word(org) && word(repo) => Ok(PartSource::Github {
                    org: org.into(),
                    repo: repo.into(),
                }),
                _ => Err(format!("`{s}`: github:<org>/<repo>, like github:sigp/lighthouse")),
            },
            "docker" if !rest.is_empty() && rest.split(['/', ':']).all(word) && !rest.contains('@') => {
                Ok(PartSource::Docker { image: rest.into() })
            }
            "docker" => Err(format!("`{s}`: docker:<image>, like docker:sigp/lighthouse")),
            "apt" if word(rest) => Ok(PartSource::Apt { package: rest.into() }),
            "apt" => Err(format!("`{s}`: apt:<package>, like apt:openssl")),
            _ => Err(format!("`{s}` is no source: {FORM}")),
        }
    }
}

impl std::fmt::Display for PartSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartSource::Github { org, repo } => write!(f, "github:{org}/{repo}"),
            PartSource::Docker { image } => write!(f, "docker:{image}"),
            PartSource::Apt { package } => write!(f, "apt:{package}"),
        }
    }
}

impl Serialize for PartSource {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// The facts a PART derives, `<part>.<field>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PartField {
    /// The version the VERSION command prints.
    Running,
    /// The newest release.
    Newest,
    /// Releases newer than the running one.
    Behind,
    /// A release newer than the running one fixes a security issue or is high priority.
    Security,
    /// The notes of the releases newer than the running one, trimmed.
    Notes,
}

impl PartField {
    pub const ALL: [PartField; 5] = [
        PartField::Running,
        PartField::Newest,
        PartField::Behind,
        PartField::Security,
        PartField::Notes,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PartField::Running => "running",
            PartField::Newest => "newest",
            PartField::Behind => "behind",
            PartField::Security => "security",
            PartField::Notes => "notes",
        }
    }

    pub fn ty(self) -> FactType {
        match self {
            PartField::Running | PartField::Newest => FactType::Version,
            PartField::Behind => FactType::Number(None),
            PartField::Security => FactType::Bool,
            PartField::Notes => FactType::Text,
        }
    }
}

/// What an action changes: a blast radius every weave shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// The service restarts: a brief outage.
    Restart,
    /// Settings.
    Config,
    /// The installed release.
    Version,
    /// Stored data.
    Data,
    /// Exposure: listeners, firewall.
    Network,
}

impl Effect {
    pub const ALL: [Effect; 5] = [
        Effect::Restart,
        Effect::Config,
        Effect::Version,
        Effect::Data,
        Effect::Network,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Restart => "restart",
            Effect::Config => "config",
            Effect::Version => "version",
            Effect::Data => "data",
            Effect::Network => "network",
        }
    }

    pub fn parse(w: &str) -> Option<Effect> {
        Effect::ALL.into_iter().find(|e| e.as_str() == w)
    }
}

/// `restart, version`: effects as a Weavefile lists them.
pub fn show_effects(es: &[Effect]) -> String {
    es.iter().map(|e| e.as_str()).collect::<Vec<_>>().join(", ")
}

#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub name: String,
    /// What it keeps true, for a person: `GOAL current "reth runs its newest release" …`.
    pub sentence: String,
    pub after: Vec<GoalRef>,
    #[serde(serialize_with = "dur_opt")]
    pub every: Option<Duration>,
    /// The window it is judged over; `None`: its EVERY.
    #[serde(serialize_with = "dur_opt")]
    pub over: Option<Duration>,
    #[serde(serialize_with = "tries_opt")]
    pub tries: Option<(u32, Duration)>,
    pub cond: Condition,
    /// The trailing `IF`.
    pub when: Option<Expr>,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Condition {
    Code(Expr),
    Quoted(String),
}

/// A goal AFTER names: one of this weave's, or `<service>:<goal>`, a goal of another
/// service on the same box.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GoalRef {
    pub service: Option<String>,
    pub goal: String,
}

impl GoalRef {
    /// The goal's name when it is this weave's.
    pub fn local(&self) -> Option<&str> {
        self.service.is_none().then_some(self.goal.as_str())
    }
}

impl std::fmt::Display for GoalRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.service {
            Some(s) => write!(f, "{s}:{}", self.goal),
            None => f.write_str(&self.goal),
        }
    }
}

impl Serialize for GoalRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub name: String,
    /// The button that runs it: `ACTION fence "Make JSON-RPC local only" …`.
    pub label: Option<String>,
    pub for_goals: Vec<String>,
    #[serde(serialize_with = "dur_opt")]
    pub within: Option<Duration>,
    /// `CHANGES`: `None` unsaid, which counts as every effect; empty: `CHANGES nothing`.
    pub changes: Option<Vec<Effect>>,
    /// `UNDO`: the action that puts back what this one changed.
    pub undo: Option<String>,
    /// The trailing `IF`: a precondition. While false or unknown the action is no fix.
    pub when_if: Option<Expr>,
    pub when: String,
    pub steps: Vec<Step>,
    pub line: u32,
}

impl Action {
    /// What it changes: its `CHANGES`, else every effect.
    pub fn effects(&self) -> &[Effect] {
        self.changes.as_deref().unwrap_or(&Effect::ALL)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Step {
    Run {
        script: String,
        line: u32,
    },
    Do {
        task: String,
        line: u32,
    },
    Add {
        path: String,
        content: String,
        line: u32,
    },
}

/// What a `BEFORE` rule gates: one action, or every action with one of the effects.
#[derive(Debug, Clone, PartialEq)]
pub enum Before {
    Action(String),
    Changes(Vec<Effect>),
}

impl Before {
    pub fn gates(&self, a: &Action) -> bool {
        match self {
            Before::Action(n) => *n == a.name,
            Before::Changes(es) => a.effects().iter().any(|e| es.contains(e)),
        }
    }
}

impl std::fmt::Display for Before {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Before::Action(a) => f.write_str(a),
            Before::Changes(es) => write!(f, "CHANGES {}", show_effects(es)),
        }
    }
}

impl Serialize for Before {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rule {
    Never {
        text: String,
        line: u32,
    },
    NeverIf {
        expr: Expr,
        because: Option<String>,
        line: u32,
    },
    /// `NEVER BEFORE <action> | CHANGES <effects>`: refuses what it gates.
    NeverBefore {
        on: Before,
        cond: Option<Expr>,
        because: Option<String>,
        line: u32,
    },
    Ask {
        text: String,
        line: u32,
    },
    /// `ASK BEFORE <action> | CHANGES <effects>`: the person says yes first.
    AskBefore {
        on: Before,
        cond: Option<Expr>,
        because: Option<String>,
        line: u32,
    },
    AskIf {
        expr: Expr,
        because: Option<String>,
        line: u32,
    },
}

impl Rule {
    pub fn line(&self) -> u32 {
        match self {
            Rule::Never { line, .. }
            | Rule::NeverIf { line, .. }
            | Rule::NeverBefore { line, .. }
            | Rule::Ask { line, .. }
            | Rule::AskBefore { line, .. }
            | Rule::AskIf { line, .. } => *line,
        }
    }

    /// A code rule's `BECAUSE`: why it asks or refuses, for a person.
    pub fn because(&self) -> Option<&str> {
        match self {
            Rule::NeverIf { because, .. }
            | Rule::NeverBefore { because, .. }
            | Rule::AskBefore { because, .. }
            | Rule::AskIf { because, .. } => because.as_deref(),
            Rule::Never { .. } | Rule::Ask { .. } => None,
        }
    }

    /// As written, without BECAUSE: the key its `always` and `never` answers are kept
    /// under.
    pub fn text(&self) -> String {
        let before = |kw: &str, on: &Before, cond: &Option<Expr>| match cond {
            Some(c) => format!("{kw} BEFORE {on} IF {c}"),
            None => format!("{kw} BEFORE {on}"),
        };
        match self {
            Rule::Never { text, .. } => format!("NEVER \"{text}\""),
            Rule::NeverIf { expr, .. } => format!("NEVER IF {expr}"),
            Rule::NeverBefore { on, cond, .. } => before("NEVER", on, cond),
            Rule::Ask { text, .. } => format!("ASK \"{text}\""),
            Rule::AskBefore { on, cond, .. } => before("ASK", on, cond),
            Rule::AskIf { expr, .. } => format!("ASK IF {expr}"),
        }
    }
}

/// `recipes` holds the commands of the body's ```` ```recipe ```` blocks, one command
/// each, in order; a command continued with `\` stays one entry.
#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub when: String,
    pub body: String,
    pub recipes: Vec<String>,
    pub line: u32,
}

impl Weave {
    pub fn arg(&self, name: &str) -> Option<&Arg> {
        self.args.iter().find(|a| a.name == name)
    }

    pub fn goal(&self, name: &str) -> Option<&Goal> {
        self.goals.iter().find(|g| g.name == name)
    }

    pub fn action(&self, name: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.name == name)
    }

    pub fn fact(&self, name: &str) -> Option<&Fact> {
        self.facts.iter().find(|f| f.name == name)
    }

    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == name)
    }

    pub fn drill(&self, name: &str) -> Option<&Drill> {
        self.drills.iter().find(|d| d.name == name)
    }

    /// Named by another action's UNDO.
    pub fn undoes(&self, name: &str) -> bool {
        self.actions.iter().any(|a| a.undo.as_deref() == Some(name))
    }

    /// Every slot of every option ARG, in file order.
    pub fn slots(&self) -> impl Iterator<Item = &str> {
        self.args
            .iter()
            .flat_map(|a| match &a.kind {
                ArgKind::Value { .. } => (&[] as &[ArgOption]).iter(),
                ArgKind::Options(o) => o.iter(),
            })
            .flat_map(|o| o.slots.iter().map(String::as_str))
    }

    /// A goal judged over a window: its condition reads a window function, or a LINES
    /// fact in backticks.
    pub fn windowed(&self, g: &Goal) -> bool {
        match &g.cond {
            Condition::Code(e) => e.windowed(),
            Condition::Quoted(_) => self.judged_lines(g).next().is_some(),
        }
    }

    /// The LINES facts a quoted goal names in backticks: its window's lines are judged one
    /// kind at a time, and KNOWN lines apply to it.
    pub fn judged_lines<'a>(&'a self, g: &'a Goal) -> impl Iterator<Item = &'a str> {
        let text = match &g.cond {
            Condition::Quoted(t) => t.as_str(),
            Condition::Code(_) => "",
        };
        backticked(text).filter(|n| self.fact(n).is_some_and(|f| f.ty == FactType::Lines))
    }

    /// The facts of this weave a goal reads: its condition's, window function arguments
    /// and backticked names included, and its IF's.
    pub fn reads(&self, g: &Goal) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let code = match &g.cond {
            Condition::Code(e) => Some(e),
            Condition::Quoted(t) => {
                for n in backticked(t).filter(|n| self.fact(n).is_some()) {
                    out.insert(n.to_string());
                }
                None
            }
        };
        for e in code.into_iter().chain(&g.when) {
            e.walk(&mut |x| match x {
                Expr::Name(n) | Expr::Window(_, n) if self.fact(n).is_some() => {
                    out.insert(n.clone());
                }
                _ => {}
            });
        }
        out
    }

    /// The facts of other services a goal reads, as (service, fact): its condition's and
    /// its IF's.
    pub fn reads_other<'a>(&self, g: &'a Goal) -> BTreeSet<(&'a str, &'a str)> {
        let mut out = BTreeSet::new();
        let code = match &g.cond {
            Condition::Code(e) => Some(e),
            Condition::Quoted(_) => None,
        };
        for e in code.into_iter().chain(&g.when) {
            e.walk(&mut |x| {
                if let Expr::Other(s, f) = x {
                    out.insert((s.as_str(), f.as_str()));
                }
            });
        }
        out
    }
}

/// How often a goal is checked with no EVERY: code is cheap, a quoted goal asks a model.
const CODE_EVERY: Duration = Duration::from_secs(60);
const QUOTED_EVERY: Duration = Duration::from_secs(300);

impl Goal {
    pub fn every(&self) -> Duration {
        self.every.unwrap_or(match self.cond {
            Condition::Code(_) => CODE_EVERY,
            Condition::Quoted(_) => QUOTED_EVERY,
        })
    }

    /// The window it is judged over: its OVER, else its EVERY.
    pub fn over(&self) -> Duration {
        self.over.unwrap_or_else(|| self.every())
    }
}

/// The names a quoted text gives in backticks: `` `logs` ``.
pub fn backticked(text: &str) -> impl Iterator<Item = &str> {
    let ticks = text.matches('`').count();
    text.split('`')
        .enumerate()
        .filter(move |(k, _)| k % 2 == 1 && *k < ticks)
        .map(|(_, p)| p)
}

/// `5m`, `25h`, `30d`, `90s`: the largest unit that divides the duration.
pub fn show_duration(d: Duration) -> String {
    let s = d.as_secs();
    if s != 0 && s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s != 0 && s % 3_600 == 0 {
        format!("{}h", s / 3_600)
    } else if s != 0 && s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

fn dur<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&show_duration(*d))
}

fn dur_opt<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
    match d {
        Some(d) => s.serialize_str(&show_duration(*d)),
        None => s.serialize_none(),
    }
}

fn tries_opt<S: Serializer>(t: &Option<(u32, Duration)>, s: S) -> Result<S::Ok, S::Error> {
    match t {
        Some((n, per)) => s.collect_str(&format_args!("{n} PER {}", show_duration(*per))),
        None => s.serialize_none(),
    }
}
