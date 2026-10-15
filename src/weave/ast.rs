use std::time::Duration;

use serde::{Serialize, Serializer};

use super::expr::Expr;

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
    pub goals: Vec<Goal>,
    pub actions: Vec<Action>,
    pub rules: Vec<Rule>,
    pub skills: Vec<Skill>,
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
    pub follow: bool,
    pub source: FactSource,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FactSource {
    Command(String),
    Quoted(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub name: String,
    pub after: Vec<String>,
    #[serde(serialize_with = "dur_opt")]
    pub every: Option<Duration>,
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

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    pub name: String,
    pub for_goals: Vec<String>,
    #[serde(serialize_with = "dur_opt")]
    pub within: Option<Duration>,
    /// The trailing `IF`.
    pub when_if: Option<Expr>,
    pub when: String,
    pub steps: Vec<Step>,
    pub line: u32,
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

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Rule {
    Never {
        text: String,
        line: u32,
    },
    NeverIf {
        expr: Expr,
        line: u32,
    },
    Ask {
        text: String,
        line: u32,
    },
    AskBefore {
        action: String,
        cond: Option<Expr>,
        line: u32,
    },
    AskIf {
        expr: Expr,
        line: u32,
    },
}

impl Rule {
    pub fn line(&self) -> u32 {
        match self {
            Rule::Never { line, .. }
            | Rule::NeverIf { line, .. }
            | Rule::Ask { line, .. }
            | Rule::AskBefore { line, .. }
            | Rule::AskIf { line, .. } => *line,
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
