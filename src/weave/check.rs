//! Problems that need the whole file: names, references, cycles and expressions.

use std::collections::{BTreeMap, BTreeSet};

use super::Problem;
use super::ast::{ArgKind, Before, Condition, Rule, Weave};
use super::expr::{Expr, Op, Window};
use super::value::{Dim, FactType, Version};

pub fn check(w: &Weave, problems: &mut Vec<Problem>) {
    if w.from.is_none() {
        problems.push(Problem::new(
            0,
            "no FROM line: say the box's system, like FROM ubuntu:24.04",
        ));
    }
    names(w, problems);
    references(w, problems);
    cycles(w, problems);
    args(w, problems);
    expressions(w, problems);
    backticks(w, problems);
    known(w, problems);
}

/// Each KNOWN names a quoted goal over a LINES fact, and one kind of line once per goal.
fn known(w: &Weave, problems: &mut Vec<Problem>) {
    let mut seen: BTreeMap<(&str, String), u32> = BTreeMap::new();
    for k in &w.known {
        let Some(g) = w.goal(&k.goal) else {
            problems.push(Problem::new(k.line, format!("KNOWN {0}: no GOAL {0}", k.goal)));
            continue;
        };
        if w.judged_lines(g).next().is_none() {
            problems.push(Problem::new(
                k.line,
                format!(
                    "KNOWN {}: the goal judges no lines; KNOWN takes a quoted GOAL that names a \
                     LINES fact in backticks",
                    k.goal
                ),
            ));
            continue;
        }
        let kind = crate::daemon::memory::template(&k.sample);
        if let Some(first) = seen.insert((k.goal.as_str(), kind), k.line) {
            problems.push(Problem::new(
                k.line,
                format!("KNOWN {}: line {first} already says this kind of line", k.goal),
            ));
        }
    }
}

/// Each name once across ARGs, slots, SECRETs, FACTs, PARTs, GOALs, ACTIONs and SKILLs.
fn names(w: &Weave, problems: &mut Vec<Problem>) {
    let mut all: Vec<(&str, u32, &str)> = Vec::new();
    for a in &w.args {
        all.push((&a.name, a.line, "ARG"));
        if let ArgKind::Options(opts) = &a.kind {
            for o in opts {
                for s in &o.slots {
                    all.push((s, o.line, "slot"));
                }
            }
        }
    }
    all.extend(
        w.secrets
            .iter()
            .map(|s| (s.name.as_str(), s.line, "SECRET")),
    );
    all.extend(w.facts.iter().map(|f| (f.name.as_str(), f.line, "FACT")));
    all.extend(w.parts.iter().map(|p| (p.name.as_str(), p.line, "PART")));
    all.extend(w.goals.iter().map(|g| (g.name.as_str(), g.line, "GOAL")));
    all.extend(
        w.actions
            .iter()
            .map(|a| (a.name.as_str(), a.line, "ACTION")),
    );
    all.extend(w.skills.iter().map(|s| (s.name.as_str(), s.line, "SKILL")));
    all.sort_by_key(|(_, line, _)| *line);
    let mut seen: BTreeMap<&str, (u32, &str)> = BTreeMap::new();
    for (name, line, kind) in all {
        match seen.get(name) {
            Some((first, first_kind)) => problems.push(Problem::new(
                line,
                format!("`{name}` is already the name of the {first_kind} on line {first}"),
            )),
            None => {
                seen.insert(name, (line, kind));
            }
        }
    }
}

fn references(w: &Weave, problems: &mut Vec<Problem>) {
    for g in &w.goals {
        for a in &g.after {
            if w.goal(a).is_none() {
                problems.push(Problem::new(g.line, format!("AFTER {a}: no GOAL {a}")));
            }
        }
    }
    for act in &w.actions {
        for g in &act.for_goals {
            if w.goal(g).is_none() {
                problems.push(Problem::new(act.line, format!("FOR {g}: no GOAL {g}")));
            }
        }
        if let Some(u) = &act.undo {
            if w.action(u).is_none() {
                problems.push(Problem::new(act.line, format!("UNDO {u}: no ACTION {u}")));
            } else if *u == act.name {
                problems.push(Problem::new(act.line, format!("ACTION {u}: UNDO names itself")));
            }
        }
    }
    for r in &w.rules {
        if let Rule::AskBefore { on: Before::Action(action), line, .. }
        | Rule::NeverBefore { on: Before::Action(action), line, .. } = r
        {
            if w.action(action).is_none() {
                problems.push(Problem::new(
                    *line,
                    format!("{}: no ACTION {action}", r.text()),
                ));
            }
        }
    }
}

/// One problem per AFTER cycle, on the line of the goal where the walk closes it.
fn cycles(w: &Weave, problems: &mut Vec<Problem>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    fn visit(
        w: &Weave,
        i: usize,
        marks: &mut [Mark],
        stack: &mut Vec<usize>,
        problems: &mut Vec<Problem>,
    ) {
        marks[i] = Mark::Open;
        stack.push(i);
        for a in &w.goals[i].after {
            let Some(j) = w.goals.iter().position(|g| &g.name == a) else {
                continue;
            };
            match marks[j] {
                Mark::New => visit(w, j, marks, stack, problems),
                Mark::Open => {
                    let from = stack.iter().position(|&k| k == j).unwrap_or(0);
                    let mut path: Vec<&str> = stack[from..]
                        .iter()
                        .map(|&k| w.goals[k].name.as_str())
                        .collect();
                    path.push(&w.goals[j].name);
                    problems.push(Problem::new(
                        w.goals[i].line,
                        format!("AFTER cycle: {}", path.join(" → ")),
                    ));
                }
                Mark::Done => {}
            }
        }
        stack.pop();
        marks[i] = Mark::Done;
    }
    let mut marks = vec![Mark::New; w.goals.len()];
    let mut stack = Vec::new();
    for i in 0..w.goals.len() {
        if marks[i] == Mark::New {
            visit(w, i, &mut marks, &mut stack, problems);
        }
    }
}

fn args(w: &Weave, problems: &mut Vec<Problem>) {
    for a in &w.args {
        if a.level == super::ast::Level::Must && a.default.is_some() {
            problems.push(Problem::new(
                a.line,
                format!("ARG {}: a MUST ARG has no default", a.name),
            ));
        }
        if let ArgKind::Options(opts) = &a.kind {
            if opts.is_empty() {
                problems.push(Problem::new(
                    a.line,
                    format!(
                        "ARG {} has no quoted description and no options under it",
                        a.name
                    ),
                ));
            } else if let Some(d) = &a.default {
                if !opts.iter().any(|o| &o.label == d) {
                    let labels: Vec<&str> = opts.iter().map(|o| o.label.as_str()).collect();
                    problems.push(Problem::new(
                        a.line,
                        format!(
                            "ARG {}: default `{d}` is not one of {}",
                            a.name,
                            labels.join(", ")
                        ),
                    ));
                }
            }
        }
    }
}

/// Every code expression is typed: units, versions and LINES are checked here, before
/// anything runs. A goal's condition and IF may read its window; nothing else may.
fn expressions(w: &Weave, problems: &mut Vec<Problem>) {
    let words: BTreeSet<&str> = w.args.iter().map(|a| a.name.as_str()).chain(w.slots()).collect();
    let typer = |windows| Typer {
        w,
        words: &words,
        windows,
    };
    let mut conds: Vec<(u32, &Expr, bool)> = Vec::new();
    for s in &w.secrets {
        conds.extend(s.cond.iter().map(|e| (s.line, e, false)));
    }
    for g in &w.goals {
        if let Condition::Code(e) = &g.cond {
            conds.push((g.line, e, true));
        }
        conds.extend(g.when.iter().map(|e| (g.line, e, true)));
        if g.over.is_some() && !w.windowed(g) {
            problems.push(Problem::new(
                g.line,
                format!(
                    "GOAL {}: OVER needs a window to judge: a LINES fact, or MIN, MAX, AVG, \
                     DELTA or COUNT",
                    g.name
                ),
            ));
        }
    }
    for a in &w.actions {
        conds.extend(a.when_if.iter().map(|e| (a.line, e, false)));
    }
    for r in &w.rules {
        match r {
            Rule::NeverIf { expr, line, .. } | Rule::AskIf { expr, line, .. } => {
                conds.push((*line, expr, false))
            }
            Rule::AskBefore { cond: Some(e), line, .. }
            | Rule::NeverBefore { cond: Some(e), line, .. } => conds.push((*line, e, false)),
            _ => {}
        }
    }
    for (line, e, windows) in conds {
        if let Err(msg) = typer(windows).cond(e) {
            problems.push(Problem::new(line, msg));
        }
    }
}

/// An expression's type. ARG and slot words are `Any`: they take the other side's type.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Ty {
    Bool,
    Num(Dim),
    Text,
    Version,
    Any,
}

impl std::fmt::Display for Ty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ty::Bool => f.write_str("a BOOL"),
            Ty::Num(d) => write!(f, "{d}"),
            Ty::Text => f.write_str("a TEXT"),
            Ty::Version => f.write_str("a VERSION"),
            Ty::Any => f.write_str("an ARG"),
        }
    }
}

struct Typer<'a> {
    w: &'a Weave,
    words: &'a BTreeSet<&'a str>,
    /// Window functions are allowed: a goal's condition or IF.
    windows: bool,
}

impl Typer<'_> {
    /// `e` is a condition: a BOOL fact, an ARG, a comparison, or those joined.
    fn cond(&self, e: &Expr) -> Result<(), String> {
        match e {
            Expr::Group(x) | Expr::Not(x) => self.cond(x),
            Expr::Bin(l, Op::And | Op::Or, r) => {
                self.cond(l)?;
                self.cond(r)
            }
            Expr::Bin(_, op, _) if op.compares() => self.ty(e).map(drop),
            Expr::Name(n) => match self.ty(e)? {
                Ty::Bool | Ty::Any => Ok(()),
                t => Err(format!("`{n}` is {t}, not a condition: compare it")),
            },
            _ => {
                self.ty(e)?;
                Err(format!("`{e}` is a value, not a condition: compare it"))
            }
        }
    }

    fn ty(&self, e: &Expr) -> Result<Ty, String> {
        Ok(match e {
            Expr::Name(n) => match self.w.fact(n) {
                Some(f) => match f.ty {
                    FactType::Bool => Ty::Bool,
                    FactType::Number(u) => Ty::Num(u.map_or(Dim::Plain, |u| u.dim())),
                    FactType::Text => Ty::Text,
                    FactType::Version => Ty::Version,
                    FactType::Lines => {
                        return Err(format!(
                            "`{n}` is LINES: COUNT({n}) counts its lines in the goal's window, \
                             and a quoted condition reads them"
                        ));
                    }
                },
                None if self.words.contains(n.as_str()) => Ty::Any,
                None => return Err(format!("`{n}` is no ARG, slot or FACT")),
            },
            Expr::Num(_, u) => Ty::Num(u.map_or(Dim::Plain, |u| u.dim())),
            Expr::Str(_) => Ty::Text,
            Expr::Since(a) => {
                if self.w.action(a).is_none() {
                    return Err(format!("SINCE({a}): no ACTION {a}"));
                }
                Ty::Num(Dim::Time)
            }
            Expr::Window(f, n) => {
                let call = format!("{}({n})", f.as_str());
                if !self.windows {
                    return Err(format!(
                        "`{call}` reads a goal's window: only a GOAL's condition and IF take it"
                    ));
                }
                let Some(fact) = self.w.fact(n) else {
                    return Err(format!("`{call}`: no FACT {n}"));
                };
                match (f, fact.ty) {
                    (Window::Count, _) => Ty::Num(Dim::Plain),
                    (_, FactType::Number(u)) => Ty::Num(u.map_or(Dim::Plain, |u| u.dim())),
                    (_, t) => {
                        return Err(format!(
                            "`{call}`: {n} is {t}; MIN, MAX, AVG and DELTA take a NUMBER"
                        ));
                    }
                }
            }
            Expr::Group(x) => self.ty(x)?,
            Expr::Not(_) | Expr::Bin(_, Op::And | Op::Or, _) => {
                self.cond(e)?;
                Ty::Bool
            }
            Expr::Bin(l, op, r) if op.compares() => {
                self.compare(e, l, *op, r)?;
                Ty::Bool
            }
            Expr::Bin(l, op, r) => match (self.ty(l)?, self.ty(r)?) {
                (Ty::Any, _) | (_, Ty::Any) => Ty::Any,
                (Ty::Num(a), Ty::Num(b)) => {
                    let t = match op {
                        Op::Add | Op::Sub if a == b => Some(a),
                        Op::Mul if b == Dim::Plain => Some(a),
                        Op::Mul if a == Dim::Plain => Some(b),
                        Op::Div if b == Dim::Plain => Some(a),
                        Op::Div if a == b => Some(Dim::Plain),
                        _ => None,
                    };
                    match t {
                        Some(d) => Ty::Num(d),
                        None => {
                            return Err(format!(
                                "`{e}`: `{l}` is {a} and `{r}` is {b}: `{}` does not join them",
                                op.as_str()
                            ));
                        }
                    }
                }
                (a, b) => {
                    return Err(format!(
                        "`{e}`: arithmetic takes numbers, and this has {a} and {b}"
                    ));
                }
            },
        })
    }

    fn compare(&self, e: &Expr, l: &Expr, op: Op, r: &Expr) -> Result<(), String> {
        let (a, b) = (self.ty(l)?, self.ty(r)?);
        for (side, t) in [(l, a), (r, b)] {
            if t == Ty::Bool {
                return Err(format!("`{side}` is a BOOL: use it bare or with NOT"));
            }
        }
        let version_literal = |side: &Expr| match side {
            Expr::Str(s) if Version::parse(s).is_none() => {
                Err(format!("`{e}`: '{s}' is no version, like '1.2.3'"))
            }
            Expr::Str(_) => Ok(()),
            _ => Err(format!("`{e}`: a VERSION compares with a VERSION or a quoted version")),
        };
        match (a, b) {
            (Ty::Any, _) | (_, Ty::Any) | (Ty::Version, Ty::Version) => Ok(()),
            (Ty::Num(x), Ty::Num(y)) if x == y => Ok(()),
            (Ty::Text, Ty::Text) if matches!(op, Op::Eq | Op::Ne) => Ok(()),
            (Ty::Text, Ty::Text) => Err(format!("`{e}`: TEXT compares only with == and !=")),
            (Ty::Version, Ty::Text) => version_literal(r),
            (Ty::Text, Ty::Version) => version_literal(l),
            (a, b) => Err(format!(
                "`{e}`: `{l}` is {a} and `{r}` is {b}, which do not compare"
            )),
        }
    }
}

/// A quoted GOAL says which facts it reads with backticked names.
fn backticks(w: &Weave, problems: &mut Vec<Problem>) {
    for g in &w.goals {
        let Condition::Quoted(text) = &g.cond else {
            continue;
        };
        for part in super::ast::backticked(text) {
            let known =
                w.fact(part).is_some() || w.arg(part).is_some() || w.slots().any(|s| s == part);
            if !known {
                problems.push(Problem::new(
                    g.line,
                    format!("GOAL {}: `{part}` is no FACT or ARG", g.name),
                ));
            }
        }
    }
}
