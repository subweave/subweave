//! Code expressions: conditions after `GOAL`, `IF`, `NEVER IF` and `ASK IF`. A value
//! that is not known (a fact not read) makes what reads it unknown, not false.

use std::fmt;
use std::time::Duration;

use serde::{Serialize, Serializer};

use super::value::{self, Dim, Unit, Version};

pub use super::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Name(String),
    /// `85`, `85%`, `25h`: kept as written so it prints back the same.
    Num(f64, Option<Unit>),
    Str(String),
    Since(String),
    /// `MIN(disk)`: over the goal's window.
    Window(Window, String),
    Bin(Box<Expr>, Op, Box<Expr>),
    Not(Box<Expr>),
    Group(Box<Expr>),
}

/// A window function: over the observations of a fact in the goal's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Min,
    Max,
    Avg,
    /// The newest value minus the value at the window's start.
    Delta,
    /// Lines, or reads.
    Count,
}

const WINDOWS: &[(&str, Window)] = &[
    ("MIN", Window::Min),
    ("MAX", Window::Max),
    ("AVG", Window::Avg),
    ("DELTA", Window::Delta),
    ("COUNT", Window::Count),
];

impl Window {
    pub fn word(w: &str) -> Option<Window> {
        WINDOWS.iter().find(|(k, _)| *k == w).map(|(_, f)| *f)
    }

    pub fn as_str(self) -> &'static str {
        WINDOWS
            .iter()
            .find(|(_, f)| *f == self)
            .map(|(k, _)| *k)
            .expect("every window function is in WINDOWS")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Add => "+",
            Op::Sub => "-",
            Op::Mul => "*",
            Op::Div => "/",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Eq => "==",
            Op::Ne => "!=",
            Op::And => "AND",
            Op::Or => "OR",
        }
    }

    pub fn compares(self) -> bool {
        matches!(self, Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne)
    }
}

/// What an expression reads.
pub trait Env {
    /// A fact's newest value, or an ARG's or a slot's words (empty when unsaid). `None`: a
    /// fact with no value now.
    fn value(&self, name: &str) -> Option<Value>;
    fn since(&self, action: &str) -> Option<Duration>;
    /// `f` over the fact's observations in the goal's window; `None` with none to read.
    fn window(&self, f: Window, fact: &str) -> Option<Value>;
}

/// `90s`, `10m`, `25h`, `30d`.
pub fn duration(s: &str) -> Option<Duration> {
    if !s.is_ascii() {
        return None;
    }
    let (n, unit) = s.split_at(s.len().checked_sub(1)?);
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = n.parse().ok()?;
    let secs = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(n.checked_mul(secs)?))
}

/// Whether a condition holds; `None` when a value it needs is unknown. `AND` with one
/// false side is false, `OR` with one true side is true, whatever the other side.
pub fn holds(e: &Expr, env: &dyn Env) -> Option<bool> {
    match e {
        Expr::Name(n) => match env.value(n)? {
            Value::Bool(b) => Some(b),
            Value::Text(t) => Some(!t.is_empty()),
            Value::Num(..) | Value::Version(_) => None,
        },
        Expr::Group(inner) => holds(inner, env),
        Expr::Not(inner) => holds(inner, env).map(|b| !b),
        Expr::Bin(l, Op::And, r) => match holds(l, env) {
            Some(false) => Some(false),
            a => match (a, holds(r, env)?) {
                (_, false) => Some(false),
                (a, true) => a,
            },
        },
        Expr::Bin(l, Op::Or, r) => match holds(l, env) {
            Some(true) => Some(true),
            a => match (a, holds(r, env)?) {
                (_, true) => Some(true),
                (a, false) => a,
            },
        },
        Expr::Bin(l, op, r) if op.compares() => {
            Some(compare(&value(l, env)?, *op, &value(r, env)?))
        }
        _ => None,
    }
}

fn value(e: &Expr, env: &dyn Env) -> Option<Value> {
    match e {
        Expr::Name(n) => env.value(n),
        Expr::Num(n, unit) => Some(match unit {
            Some(u) => Value::Num(n * u.scale(), u.dim()),
            None => Value::Num(*n, Dim::Plain),
        }),
        Expr::Str(s) => Some(Value::Text(s.clone())),
        Expr::Since(a) => env.since(a).map(|d| Value::Num(d.as_secs_f64(), Dim::Time)),
        Expr::Window(f, n) => env.window(*f, n),
        Expr::Group(inner) => value(inner, env),
        Expr::Bin(l, op @ (Op::Add | Op::Sub | Op::Mul | Op::Div), r) => {
            arith(&value(l, env)?, *op, &value(r, env)?)
        }
        Expr::Bin(..) | Expr::Not(_) => holds(e, env).map(Value::Bool),
    }
}

/// An ARG's words read as a number of kind `dim`: a bare number takes its base unit.
fn as_num(t: &str, dim: Dim) -> Option<(f64, Dim)> {
    value::number(t, dim.base()).filter(|(_, d)| *d == dim)
}

fn num(v: &Value, dim: Dim) -> Option<(f64, Dim)> {
    match v {
        Value::Num(n, d) => Some((*n, *d)),
        Value::Text(t) => as_num(t, dim),
        Value::Bool(_) | Value::Version(_) => None,
    }
}

fn arith(a: &Value, op: Op, b: &Value) -> Option<Value> {
    // ARG words take the other side's kind when added or subtracted, none when scaling.
    let hint = |other: &Value| match (op, other) {
        (Op::Add | Op::Sub, Value::Num(_, d)) => *d,
        _ => Dim::Plain,
    };
    let (x, d) = num(a, hint(b))?;
    let (y, e) = num(b, hint(a))?;
    let n = match op {
        Op::Add if d == e => Value::Num(x + y, d),
        Op::Sub if d == e => Value::Num(x - y, d),
        Op::Mul if e == Dim::Plain => Value::Num(x * y, d),
        Op::Mul if d == Dim::Plain => Value::Num(x * y, e),
        Op::Div if y == 0.0 => return None,
        Op::Div if e == Dim::Plain => Value::Num(x / y, d),
        Op::Div if d == e => Value::Num(x / y, Dim::Plain),
        _ => return None,
    };
    Some(n)
}

fn ord<T: PartialOrd>(x: T, op: Op, y: T) -> bool {
    match op {
        Op::Lt => x < y,
        Op::Le => x <= y,
        Op::Gt => x > y,
        Op::Ge => x >= y,
        Op::Eq => x == y,
        Op::Ne => x != y,
        _ => false,
    }
}

/// Numbers of one kind compare as numbers, versions as semver. An ARG's words take the
/// other side's type. Anything else only compares `==` and `!=`, as text.
pub fn compare(a: &Value, op: Op, b: &Value) -> bool {
    match (a, b) {
        (Value::Num(x, d), Value::Num(y, e)) => d == e && ord(x, op, y),
        (Value::Version(x), Value::Version(y)) => ord(x, op, y),
        (Value::Num(_, d), Value::Text(t)) | (Value::Text(t), Value::Num(_, d)) => {
            match as_num(t, *d) {
                Some((n, d)) if matches!(a, Value::Num(..)) => compare(a, op, &Value::Num(n, d)),
                Some((n, d)) => compare(&Value::Num(n, d), op, b),
                None => text(a, op, b),
            }
        }
        (Value::Version(_), Value::Text(t)) | (Value::Text(t), Value::Version(_)) => {
            match Version::parse(t) {
                Some(v) if matches!(a, Value::Version(_)) => compare(a, op, &Value::Version(v)),
                Some(v) => compare(&Value::Version(v), op, b),
                None => text(a, op, b),
            }
        }
        (Value::Text(x), Value::Text(y)) => match (value::number(x, None), value::number(y, None)) {
            (Some((p, d)), Some((q, e))) if d == e => ord(p, op, q),
            _ => text(a, op, b),
        },
        _ => text(a, op, b),
    }
}

fn text(a: &Value, op: Op, b: &Value) -> bool {
    match op {
        Op::Eq => a.to_string() == b.to_string(),
        Op::Ne => a.to_string() != b.to_string(),
        _ => false,
    }
}

impl Expr {
    /// Every node, parents first.
    pub fn walk<'a>(&'a self, f: &mut impl FnMut(&'a Expr)) {
        f(self);
        match self {
            Expr::Bin(l, _, r) => {
                l.walk(f);
                r.walk(f);
            }
            Expr::Not(e) | Expr::Group(e) => e.walk(f),
            _ => {}
        }
    }

    /// The names it reads, in order, without `SINCE` and window function arguments.
    pub fn names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Name(n) = e {
                out.push(n.as_str());
            }
        });
        out
    }

    /// It reads a window: a window function.
    pub fn windowed(&self) -> bool {
        let mut found = false;
        self.walk(&mut |e| found |= matches!(e, Expr::Window(..)));
        found
    }

    /// Prints the expression with each name that `lookup` knows replaced by its value:
    /// `version < 14` with version 12 is `12 < 14`.
    pub fn show_with(&self, lookup: &dyn Fn(&str) -> Option<String>) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(lookup))
            .expect("writing to a String");
        s
    }

    fn write(
        &self,
        f: &mut dyn fmt::Write,
        lookup: Option<&dyn Fn(&str) -> Option<String>>,
    ) -> fmt::Result {
        match self {
            Expr::Name(n) => match lookup.and_then(|l| l(n)) {
                Some(v) => f.write_str(&v),
                None => f.write_str(n),
            },
            Expr::Num(n, None) => write!(f, "{n}"),
            Expr::Num(n, Some(u)) => write!(f, "{n}{u}"),
            Expr::Str(s) => write!(f, "'{s}'"),
            Expr::Since(a) => write!(f, "SINCE({a})"),
            Expr::Window(w, a) => write!(f, "{}({a})", w.as_str()),
            Expr::Bin(l, op, r) => {
                l.write(f, lookup)?;
                write!(f, " {} ", op.as_str())?;
                r.write(f, lookup)
            }
            Expr::Not(e) => {
                f.write_str("NOT ")?;
                e.write(f, lookup)
            }
            Expr::Group(e) => {
                f.write_str("(")?;
                e.write(f, lookup)?;
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, None)
    }
}

impl Serialize for Expr {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Num(f64, Option<Unit>),
    Str(String),
    Op(Op),
    Not,
    Since,
    Win(Window),
    Open,
    Close,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let two = src.get(i..i + 2).unwrap_or("");
        let op2 = match two {
            "<=" => Some(Op::Le),
            ">=" => Some(Op::Ge),
            "==" => Some(Op::Eq),
            "!=" => Some(Op::Ne),
            _ => None,
        };
        if let Some(op) = op2 {
            out.push(Tok::Op(op));
            i += 2;
            continue;
        }
        let one = match c {
            b'<' => Some(Tok::Op(Op::Lt)),
            b'>' => Some(Tok::Op(Op::Gt)),
            b'+' => Some(Tok::Op(Op::Add)),
            b'-' => Some(Tok::Op(Op::Sub)),
            b'*' => Some(Tok::Op(Op::Mul)),
            b'/' => Some(Tok::Op(Op::Div)),
            b'(' => Some(Tok::Open),
            b')' => Some(Tok::Close),
            _ => None,
        };
        if let Some(t) = one {
            out.push(t);
            i += 1;
            continue;
        }
        match c {
            b'=' => return Err("`=` compares only as `==`".into()),
            b'"' => {
                return Err(
                    "double quotes mean model; a string in code takes single quotes".into(),
                );
            }
            b'\'' => {
                let end = src[i + 1..]
                    .find('\'')
                    .ok_or("a string has no closing `'`")?;
                out.push(Tok::Str(src[i + 1..i + 1 + end].to_string()));
                i += end + 2;
            }
            b'0'..=b'9' | b'.' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                let digits = &src[start..i];
                let n = digits
                    .parse::<f64>()
                    .map_err(|_| format!("`{digits}` is no number"))?;
                let unit_end = if b.get(i) == Some(&b'%') {
                    i + 1
                } else {
                    (i..b.len())
                        .find(|&j| !(b[j].is_ascii_alphanumeric() || b[j] == b'_'))
                        .unwrap_or(b.len())
                };
                let unit = match &src[i..unit_end] {
                    "" => None,
                    u => Some(Unit::parse(u).ok_or_else(|| {
                        format!(
                            "`{}` is no number: units are %, ms, s, m, h, d, B, KB, MB, GB and TB",
                            &src[start..unit_end]
                        )
                    })?),
                };
                out.push(Tok::Num(n, unit));
                i = unit_end;
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                // `lighthouse.running`: a PART's fact.
                while i < b.len()
                    && (b[i].is_ascii_alphanumeric()
                        || b[i] == b'_'
                        || (b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_lowercase)))
                {
                    i += 1;
                }
                let w = &src[start..i];
                out.push(match w {
                    "AND" => Tok::Op(Op::And),
                    "OR" => Tok::Op(Op::Or),
                    "NOT" => Tok::Not,
                    "SINCE" => Tok::Since,
                    w => match Window::word(w) {
                        Some(f) => Tok::Win(f),
                        None => Tok::Name(w.to_string()),
                    },
                });
            }
            _ => {
                let ch = src[i..].chars().next().unwrap_or('?');
                return Err(format!("`{ch}` has no meaning in an expression"));
            }
        }
    }
    Ok(out)
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.at).cloned();
        self.at += 1;
        t
    }

    fn or(&mut self) -> Result<Expr, String> {
        let mut l = self.and()?;
        while self.peek() == Some(&Tok::Op(Op::Or)) {
            self.at += 1;
            l = Expr::Bin(Box::new(l), Op::Or, Box::new(self.and()?));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, String> {
        let mut l = self.not()?;
        while self.peek() == Some(&Tok::Op(Op::And)) {
            self.at += 1;
            l = Expr::Bin(Box::new(l), Op::And, Box::new(self.not()?));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, String> {
        if self.peek() == Some(&Tok::Not) {
            self.at += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, String> {
        let l = self.sum()?;
        if let Some(Tok::Op(op)) = self.peek().cloned()
            && op.compares()
        {
            self.at += 1;
            let r = self.sum()?;
            if let Some(Tok::Op(next)) = self.peek()
                && next.compares()
            {
                return Err(format!(
                    "`{}` after a comparison: join comparisons with AND or OR",
                    next.as_str()
                ));
            }
            return Ok(Expr::Bin(Box::new(l), op, Box::new(r)));
        }
        Ok(l)
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut l = self.prod()?;
        while let Some(Tok::Op(op @ (Op::Add | Op::Sub))) = self.peek().cloned() {
            self.at += 1;
            l = Expr::Bin(Box::new(l), op, Box::new(self.prod()?));
        }
        Ok(l)
    }

    fn prod(&mut self) -> Result<Expr, String> {
        let mut l = self.atom()?;
        while let Some(Tok::Op(op @ (Op::Mul | Op::Div))) = self.peek().cloned() {
            self.at += 1;
            l = Expr::Bin(Box::new(l), op, Box::new(self.atom()?));
        }
        Ok(l)
    }

    fn atom(&mut self) -> Result<Expr, String> {
        match self.next() {
            Some(Tok::Name(n)) if self.peek() == Some(&Tok::Open) => Err(format!(
                "`{n}` is no function: SINCE, MIN, MAX, AVG, DELTA and COUNT are"
            )),
            Some(Tok::Name(n)) => Ok(Expr::Name(n)),
            Some(Tok::Num(n, u)) => Ok(Expr::Num(n, u)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Open) => {
                let e = self.or()?;
                match self.next() {
                    Some(Tok::Close) => Ok(Expr::Group(Box::new(e))),
                    _ => Err("a `(` has no closing `)`".into()),
                }
            }
            Some(f @ (Tok::Since | Tok::Win(_))) => {
                let word = match f {
                    Tok::Win(w) => w.as_str(),
                    _ => "SINCE",
                };
                let (Some(Tok::Open), Some(Tok::Name(n)), Some(Tok::Close)) =
                    (self.next(), self.next(), self.next())
                else {
                    return Err(format!("{word} takes one name: {word}(name)"));
                };
                Ok(match f {
                    Tok::Win(w) => Expr::Window(w, n),
                    _ => Expr::Since(n),
                })
            }
            Some(t) => Err(format!("expected a value, found {}", show_tok(&t))),
            None => Err("the expression ends early".into()),
        }
    }
}

fn show_tok(t: &Tok) -> String {
    match t {
        Tok::Name(n) => format!("`{n}`"),
        Tok::Num(n, None) => format!("`{n}`"),
        Tok::Num(n, Some(u)) => format!("`{n}{u}`"),
        Tok::Str(s) => format!("`'{s}'`"),
        Tok::Op(op) => format!("`{}`", op.as_str()),
        Tok::Not => "`NOT`".into(),
        Tok::Since => "`SINCE`".into(),
        Tok::Win(w) => format!("`{}`", w.as_str()),
        Tok::Open => "`(`".into(),
        Tok::Close => "`)`".into(),
    }
}

/// Parses a code expression; the error says what is wrong in words.
pub fn parse(src: &str) -> Result<Expr, String> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err("an empty expression".into());
    }
    let mut p = Parser { toks, at: 0 };
    let e = p.or()?;
    match p.peek() {
        None => Ok(e),
        Some(t) => Err(format!("unexpected {} in `{}`", show_tok(t), src.trim())),
    }
}
