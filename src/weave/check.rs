//! Problems that need the whole file: names, references, cycles and expressions.

use std::collections::{BTreeMap, BTreeSet};

use super::Problem;
use super::ast::{ArgKind, Condition, Rule, Weave};
use super::expr::Expr;

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
}

/// Each name once across ARGs, slots, SECRETs, FACTs, GOALs, ACTIONs and SKILLs.
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
    }
    for r in &w.rules {
        if let Rule::AskBefore { action, line, .. } = r {
            if w.action(action).is_none() {
                problems.push(Problem::new(
                    *line,
                    format!("ASK BEFORE {action}: no ACTION {action}"),
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

fn expressions(w: &Weave, problems: &mut Vec<Problem>) {
    let values: BTreeSet<&str> = w
        .args
        .iter()
        .map(|a| a.name.as_str())
        .chain(w.slots())
        .chain(w.facts.iter().map(|f| f.name.as_str()))
        .collect();
    let mut exprs: Vec<(u32, &Expr)> = Vec::new();
    for s in &w.secrets {
        exprs.extend(s.cond.iter().map(|e| (s.line, e)));
    }
    for g in &w.goals {
        if let Condition::Code(e) = &g.cond {
            exprs.push((g.line, e));
        }
        exprs.extend(g.when.iter().map(|e| (g.line, e)));
    }
    for a in &w.actions {
        exprs.extend(a.when_if.iter().map(|e| (a.line, e)));
    }
    for r in &w.rules {
        match r {
            Rule::NeverIf { expr, line } | Rule::AskIf { expr, line } => exprs.push((*line, expr)),
            Rule::AskBefore {
                cond: Some(e),
                line,
                ..
            } => exprs.push((*line, e)),
            _ => {}
        }
    }
    for (line, e) in exprs {
        e.walk(&mut |node| match node {
            Expr::Name(n) => {
                if !values.contains(n.as_str()) {
                    problems.push(Problem::new(line, format!("`{n}` is no ARG, slot or FACT")));
                } else if w.fact(n).is_some_and(|f| f.follow) {
                    problems.push(Problem::new(
                        line,
                        format!("`{n}` is a FOLLOW fact: only a quoted condition reads it"),
                    ));
                }
            }
            Expr::Since(a) if w.action(a).is_none() => {
                problems.push(Problem::new(line, format!("SINCE({a}): no ACTION {a}")));
            }
            Expr::Prev(f) => match w.fact(f) {
                None => problems.push(Problem::new(line, format!("PREV({f}): no FACT {f}"))),
                Some(fact) if fact.follow => problems.push(Problem::new(
                    line,
                    format!("`{f}` is a FOLLOW fact: only a quoted condition reads it"),
                )),
                Some(_) => {}
            },
            _ => {}
        });
    }
}

/// A quoted GOAL says which facts it reads with backticked names.
fn backticks(w: &Weave, problems: &mut Vec<Problem>) {
    for g in &w.goals {
        let Condition::Quoted(text) = &g.cond else {
            continue;
        };
        for (k, part) in text.split('`').enumerate() {
            if k % 2 == 0 || k == text.split('`').count() - 1 {
                continue;
            }
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
