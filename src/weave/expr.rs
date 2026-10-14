//! Code expressions: conditions after `GOAL`, `IF`, `NEVER IF` and `ASK IF`.

use std::fmt;
use std::time::Duration;

use serde::{Serialize, Serializer};

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Name(String),
    Num(f64),
    /// `25h`: kept as written so it prints back the same.
    Dur(u64, char),
    Str(String),
    Since(String),
    Prev(String),
    Bin(Box<Expr>, Op, Box<Expr>),
    Not(Box<Expr>),
    Group(Box<Expr>),
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    Num(f64),
    Dur(Duration),
    Bool(bool),
}

/// What an expression reads. Facts, ARGs and slots answer `value`; `truthy` is a
/// bare name: a command fact that exits 0, or an ARG that is not empty.
pub trait Env {
    fn value(&self, name: &str) -> Option<Value>;
    fn truthy(&self, name: &str) -> bool;
    fn since(&self, action: &str) -> Option<Duration>;
    fn prev(&self, fact: &str) -> Option<Value>;
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

impl Value {
    /// Text that reads as a number is a number.
    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Text(t) => {
                let t = t.trim();
                let first = t.bytes().next()?;
                if !(first.is_ascii_digit() || matches!(first, b'-' | b'+' | b'.')) {
                    return None;
                }
                t.parse::<f64>().ok().filter(|n| n.is_finite())
            }
            _ => None,
        }
    }

    /// Text that reads as a duration is a duration.
    pub fn dur(&self) -> Option<Duration> {
        match self {
            Value::Dur(d) => Some(*d),
            Value::Text(t) => duration(t.trim()),
            _ => None,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Text(t) => !t.is_empty(),
            Value::Num(n) => *n != 0.0,
            Value::Dur(d) => !d.is_zero(),
            Value::Bool(b) => *b,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Text(t) => f.write_str(t),
            Value::Num(n) => write!(f, "{n}"),
            Value::Dur(d) => f.write_str(&super::ast::show_duration(*d)),
            Value::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// The value of an expression: a comparison or a logic expression gives `Bool`, a
/// missing value gives empty text. Use [`holds`] for a condition.
pub fn eval(e: &Expr, env: &dyn Env) -> Value {
    value(e, env).unwrap_or(Value::Text(String::new()))
}

/// Whether a condition holds. A bare name asks `Env::truthy`.
pub fn holds(e: &Expr, env: &dyn Env) -> bool {
    match e {
        Expr::Name(n) => env.truthy(n),
        Expr::Group(inner) => holds(inner, env),
        Expr::Not(inner) => !holds(inner, env),
        Expr::Bin(l, Op::And, r) => holds(l, env) && holds(r, env),
        Expr::Bin(l, Op::Or, r) => holds(l, env) || holds(r, env),
        Expr::Bin(l, op @ (Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne), r) => {
            match (value(l, env), value(r, env)) {
                (Some(a), Some(b)) => compare(&a, *op, &b),
                _ => false,
            }
        }
        _ => value(e, env).is_some_and(|v| v.truthy()),
    }
}

fn value(e: &Expr, env: &dyn Env) -> Option<Value> {
    match e {
        Expr::Name(n) => env.value(n),
        Expr::Num(n) => Some(Value::Num(*n)),
        Expr::Dur(n, unit) => duration(&format!("{n}{unit}")).map(Value::Dur),
        Expr::Str(s) => Some(Value::Text(s.clone())),
        Expr::Since(a) => env.since(a).map(Value::Dur),
        Expr::Prev(f) => env.prev(f),
        Expr::Group(inner) => value(inner, env),
        Expr::Bin(l, op @ (Op::Add | Op::Sub | Op::Mul | Op::Div), r) => {
            arith(&value(l, env)?, *op, &value(r, env)?)
        }
        Expr::Bin(..) | Expr::Not(_) => Some(Value::Bool(holds(e, env))),
    }
}

fn arith(a: &Value, op: Op, b: &Value) -> Option<Value> {
    if let (Some(x), Some(y)) = (a.num(), b.num()) {
        let n = match op {
            Op::Add => x + y,
            Op::Sub => x - y,
            Op::Mul => x * y,
            Op::Div if y != 0.0 => x / y,
            _ => return None,
        };
        return Some(Value::Num(n));
    }
    match (a.dur(), b.dur(), a.num(), b.num()) {
        (Some(x), Some(y), ..) => match op {
            Op::Add => Some(Value::Dur(x + y)),
            Op::Sub => x.checked_sub(y).map(Value::Dur),
            _ => None,
        },
        (Some(x), None, _, Some(y)) => match op {
            Op::Mul => Duration::try_from_secs_f64(x.as_secs_f64() * y)
                .ok()
                .map(Value::Dur),
            Op::Div if y != 0.0 => Duration::try_from_secs_f64(x.as_secs_f64() / y)
                .ok()
                .map(Value::Dur),
            _ => None,
        },
        (None, Some(y), Some(x), _) if op == Op::Mul => {
            Duration::try_from_secs_f64(y.as_secs_f64() * x)
                .ok()
                .map(Value::Dur)
        }
        _ => None,
    }
}

/// Two numbers compare as numbers, two durations as durations; otherwise only
/// `==` and `!=` hold, on the text.
pub fn compare(a: &Value, op: Op, b: &Value) -> bool {
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
    if let (Some(x), Some(y)) = (a.num(), b.num()) {
        return ord(x, op, y);
    }
    if let (Some(x), Some(y)) = (a.dur(), b.dur()) {
        return ord(x, op, y);
    }
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

    /// The names it reads, in order, without `SINCE` and `PREV` arguments.
    pub fn names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.walk(&mut |e| {
            if let Expr::Name(n) = e {
                out.push(n.as_str());
            }
        });
        out
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
            Expr::Num(n) => write!(f, "{n}"),
            Expr::Dur(n, unit) => write!(f, "{n}{unit}"),
            Expr::Str(s) => write!(f, "'{s}'"),
            Expr::Since(a) => write!(f, "SINCE({a})"),
            Expr::Prev(a) => write!(f, "PREV({a})"),
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
    Num(f64),
    Dur(u64, char),
    Str(String),
    Op(Op),
    Not,
    Since,
    Prev,
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
                let unit = b.get(i).copied();
                let word_follows = |j: usize| {
                    b.get(j)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
                };
                if matches!(unit, Some(b's' | b'm' | b'h' | b'd')) && !word_follows(i + 1) {
                    let n = digits.parse::<u64>().map_err(|_| {
                        format!("`{digits}{}` is no duration", unit.unwrap() as char)
                    })?;
                    out.push(Tok::Dur(n, unit.unwrap() as char));
                    i += 1;
                } else if word_follows(i) {
                    let end = (i..b.len()).find(|&j| !word_follows(j)).unwrap_or(b.len());
                    return Err(format!("`{}` is no number or duration", &src[start..end]));
                } else {
                    let n = digits
                        .parse::<f64>()
                        .map_err(|_| format!("`{digits}` is no number"))?;
                    out.push(Tok::Num(n));
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                    i += 1;
                }
                let w = &src[start..i];
                out.push(match w {
                    "AND" => Tok::Op(Op::And),
                    "OR" => Tok::Op(Op::Or),
                    "NOT" => Tok::Not,
                    "SINCE" => Tok::Since,
                    "PREV" => Tok::Prev,
                    _ => Tok::Name(w.to_string()),
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
        if let Some(Tok::Op(op @ (Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne))) =
            self.peek().cloned()
        {
            self.at += 1;
            let r = self.sum()?;
            if let Some(Tok::Op(next @ (Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::Eq | Op::Ne))) =
                self.peek()
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
            Some(Tok::Name(n)) => Ok(Expr::Name(n)),
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Dur(n, u)) => Ok(Expr::Dur(n, u)),
            Some(Tok::Str(s)) => Ok(Expr::Str(s)),
            Some(Tok::Open) => {
                let e = self.or()?;
                match self.next() {
                    Some(Tok::Close) => Ok(Expr::Group(Box::new(e))),
                    _ => Err("a `(` has no closing `)`".into()),
                }
            }
            Some(f @ (Tok::Since | Tok::Prev)) => {
                let word = if f == Tok::Since { "SINCE" } else { "PREV" };
                let (Some(Tok::Open), Some(Tok::Name(n)), Some(Tok::Close)) =
                    (self.next(), self.next(), self.next())
                else {
                    return Err(format!("{word} takes one name: {word}(name)"));
                };
                Ok(if f == Tok::Since {
                    Expr::Since(n)
                } else {
                    Expr::Prev(n)
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
        Tok::Num(n) => format!("`{n}`"),
        Tok::Dur(n, u) => format!("`{n}{u}`"),
        Tok::Str(s) => format!("`'{s}'`"),
        Tok::Op(op) => format!("`{}`", op.as_str()),
        Tok::Not => "`NOT`".into(),
        Tok::Since => "`SINCE`".into(),
        Tok::Prev => "`PREV`".into(),
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
