//! The Weavefile format: `WEAVEFILE.md` in the repository.

pub mod ast;
pub mod check;
pub mod expr;
pub mod graph;
pub mod lex;
pub mod parse;
pub mod source;

use std::collections::BTreeMap;
use std::fmt;

pub use ast::*;
pub use expr::{Env, Expr, Value};

/// One thing wrong with a Weavefile. Line 0 is the file as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub line: u32,
    pub message: String,
}

impl Problem {
    pub fn new(line: u32, message: impl Into<String>) -> Self {
        Problem {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 {
            write!(f, "Weavefile: {}", self.message)
        } else {
            write!(f, "Weavefile:{}: {}", self.line, self.message)
        }
    }
}

/// Parses and checks a Weavefile. Every problem found comes back, in line order.
pub fn parse(name: &str, text: &str) -> Result<Weave, Vec<Problem>> {
    let (w, mut problems) = parse::parse(name, text);
    check::check(&w, &mut problems);
    if problems.is_empty() {
        Ok(w)
    } else {
        problems.sort_by_key(|p| p.line);
        problems.dedup();
        Err(problems)
    }
}

/// Replaces `$name` and `${name}` for each name in `args` (ARGs and slots); every
/// other `$` stays as written.
pub fn subst(text: &str, args: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, used) = if let Some(inner) = after.strip_prefix('{') {
            match inner.find('}') {
                Some(end) => (&inner[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after
                .bytes()
                .position(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
                .unwrap_or(after.len());
            (&after[..end], end)
        };
        match args.get(name) {
            Some(v) if !name.is_empty() => {
                out.push_str(v);
                rest = &after[used..];
            }
            _ => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
