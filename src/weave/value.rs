//! Typed values: fact types, units, versions, and how a fact's printed text parses.

use std::cmp::Ordering;
use std::fmt;

use serde::{Serialize, Serializer};

/// What a fact's value is: `FACT <name> <TYPE> …`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactType {
    /// Exit 0.
    Bool,
    Number(Option<Unit>),
    Text,
    Version,
    /// A stream: each printed line is kept with its time.
    Lines,
}

impl FactType {
    /// `BOOL`, `NUMBER`, … : the type word, without the unit.
    pub fn word(w: &str) -> Option<FactType> {
        Some(match w {
            "BOOL" => FactType::Bool,
            "NUMBER" => FactType::Number(None),
            "TEXT" => FactType::Text,
            "VERSION" => FactType::Version,
            "LINES" => FactType::Lines,
            _ => return None,
        })
    }

    /// Parses printed text by the type. LINES never comes here: each line is text.
    pub fn parse(self, text: &str) -> Result<Value, String> {
        let t = text.trim();
        match self {
            FactType::Bool => Err("a BOOL is an exit status, not text".into()),
            FactType::Number(unit) => number(t, unit)
                .map(|(n, dim)| Value::Num(n, dim))
                .ok_or_else(|| match unit {
                    Some(u) => format!("not a number in {u}: `{}`", short(t)),
                    None => format!("not a number: `{}`", short(t)),
                }),
            FactType::Text | FactType::Lines => Ok(Value::Text(t.to_string())),
            FactType::Version => Version::parse(t)
                .map(Value::Version)
                .ok_or_else(|| format!("not a version: `{}`", short(t))),
        }
    }
}

fn short(t: &str) -> &str {
    t.char_indices().nth(60).map_or(t, |(i, _)| &t[..i])
}

impl fmt::Display for FactType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FactType::Bool => f.write_str("BOOL"),
            FactType::Number(None) => f.write_str("NUMBER"),
            FactType::Number(Some(u)) => write!(f, "NUMBER {u}"),
            FactType::Text => f.write_str("TEXT"),
            FactType::Version => f.write_str("VERSION"),
            FactType::Lines => f.write_str("LINES"),
        }
    }
}

impl Serialize for FactType {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// What a number measures. Values keep the base unit: seconds, bytes, percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dim {
    Plain,
    Time,
    Bytes,
    Percent,
}

impl Dim {
    /// The unit a bare number of this kind is read in.
    pub fn base(self) -> Option<Unit> {
        match self {
            Dim::Plain => None,
            Dim::Time => Some(Unit::S),
            Dim::Bytes => Some(Unit::B),
            Dim::Percent => Some(Unit::Pct),
        }
    }
}

impl fmt::Display for Dim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Dim::Plain => "a number with no unit",
            Dim::Time => "a duration",
            Dim::Bytes => "a size",
            Dim::Percent => "a percentage",
        })
    }
}

/// A unit a fact or a literal carries: `85%`, `60s`, `1GB`. KB is 1024 B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Ms,
    S,
    M,
    H,
    D,
    Pct,
    B,
    Kb,
    Mb,
    Gb,
    Tb,
}

const UNITS: &[(&str, Unit)] = &[
    ("ms", Unit::Ms),
    ("s", Unit::S),
    ("m", Unit::M),
    ("h", Unit::H),
    ("d", Unit::D),
    ("%", Unit::Pct),
    ("B", Unit::B),
    ("KB", Unit::Kb),
    ("MB", Unit::Mb),
    ("GB", Unit::Gb),
    ("TB", Unit::Tb),
];

impl Unit {
    pub fn parse(s: &str) -> Option<Unit> {
        UNITS.iter().find(|(w, _)| *w == s).map(|(_, u)| *u)
    }

    pub fn as_str(self) -> &'static str {
        UNITS
            .iter()
            .find(|(_, u)| *u == self)
            .map(|(w, _)| *w)
            .expect("every unit is in UNITS")
    }

    pub fn dim(self) -> Dim {
        match self {
            Unit::Ms | Unit::S | Unit::M | Unit::H | Unit::D => Dim::Time,
            Unit::Pct => Dim::Percent,
            Unit::B | Unit::Kb | Unit::Mb | Unit::Gb | Unit::Tb => Dim::Bytes,
        }
    }

    /// One of this unit in the base unit of its kind.
    pub fn scale(self) -> f64 {
        match self {
            Unit::Ms => 0.001,
            Unit::S | Unit::Pct | Unit::B => 1.0,
            Unit::M => 60.0,
            Unit::H => 3_600.0,
            Unit::D => 86_400.0,
            Unit::Kb => 1024.0,
            Unit::Mb => 1024.0 * 1024.0,
            Unit::Gb => 1024.0 * 1024.0 * 1024.0,
            Unit::Tb => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        }
    }
}

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `85`, `85%`, `1.5GB`, `250ms`: the value in the base unit and its kind. A bare number
/// is in `unit`; a written unit must be of the same kind.
pub fn number(text: &str, unit: Option<Unit>) -> Option<(f64, Dim)> {
    let t = text.trim();
    let end = t
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && matches!(c, '-' | '+'))))
        .map_or(t.len(), |(i, _)| i);
    let n: f64 = t[..end].parse().ok().filter(|n: &f64| n.is_finite())?;
    let written = t[end..].trim_start();
    let u = if written.is_empty() {
        unit
    } else {
        let u = Unit::parse(written)?;
        if unit.is_some_and(|want| want.dim() != u.dim()) {
            return None;
        }
        Some(u)
    };
    Some(match u {
        Some(u) => (n * u.scale(), u.dim()),
        None => (n, Dim::Plain),
    })
}

/// A release: numbers joined by dots, an optional `-` pre-release and `+` build. A
/// leading `v` is dropped. Compares as semver; missing parts count as 0.
#[derive(Debug, Clone)]
pub struct Version {
    parts: Vec<u64>,
    pre: Vec<String>,
    text: String,
}

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let text = s.trim();
        let body = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let (core, rest) = match body.find(['-', '+']) {
            Some(i) => (&body[..i], &body[i..]),
            None => (body, ""),
        };
        let parts = core
            .split('.')
            .map(|p| {
                (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| p.parse::<u64>().ok())
                    .flatten()
            })
            .collect::<Option<Vec<u64>>>()?;
        let pre_text = rest
            .strip_prefix('-')
            .map(|p| p.split('+').next().unwrap_or_default());
        let pre = match pre_text {
            Some(p) if p.is_empty() => return None,
            Some(p) => p.split('.').map(str::to_string).collect(),
            None => Vec::new(),
        };
        Some(Version {
            parts,
            pre,
            text: text.to_string(),
        })
    }

    /// The version as written without its leading `v`: one release's key whichever way
    /// a source or a person spells it.
    pub fn bare(&self) -> &str {
        self.text.strip_prefix(['v', 'V']).unwrap_or(&self.text)
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let n = self.parts.len().max(other.parts.len());
        let part = |v: &Version, i: usize| v.parts.get(i).copied().unwrap_or(0);
        for i in 0..n {
            match part(self, i).cmp(&part(other, i)) {
                Ordering::Equal => {}
                o => return o,
            }
        }
        // A release ranks above its pre-releases; pre-release fields compare as numbers
        // when both are numbers.
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (a, b) in self.pre.iter().zip(&other.pre) {
            let o = match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => a.cmp(b),
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// A value an expression reads or makes. Numbers are in their kind's base unit.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Num(f64, Dim),
    Text(String),
    Version(Version),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(true) => f.write_str("yes"),
            Value::Bool(false) => f.write_str("no"),
            Value::Num(n, _) => write!(f, "{n}"),
            Value::Text(t) => f.write_str(t),
            Value::Version(v) => write!(f, "{v}"),
        }
    }
}

impl Value {
    /// A fact's value as people and models read it, in the fact's own unit: `85` for a
    /// `NUMBER %` that read 85.
    pub fn show(&self, ty: FactType) -> String {
        match (self, ty) {
            (Value::Num(n, _), FactType::Number(Some(u))) => format!("{}", n / u.scale()),
            _ => self.to_string(),
        }
    }
}
