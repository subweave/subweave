//! What a weave names on the other services of its box: `<service>:<goal>` after AFTER
//! and `<service>:<fact>` in expressions, checked against the weaves the box runs.

use std::collections::{BTreeMap, BTreeSet};

use super::Problem;
use super::ast::Weave;
use super::check;
use super::expr::Expr;

/// One reference to another service, as written, on its line: `AFTER box:room`,
/// `box:disk`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ref {
    pub line: u32,
    pub text: String,
}

/// Every reference to another service, in line order, each once per line.
pub fn refs(w: &Weave) -> Vec<Ref> {
    let mut out = BTreeSet::new();
    for g in &w.goals {
        for a in g.after.iter().filter(|a| a.service.is_some()) {
            out.insert(Ref {
                line: g.line,
                text: format!("AFTER {a}"),
            });
        }
    }
    for (line, e, _) in check::conditions(w) {
        e.walk(&mut |x| {
            if let Expr::Other(..) = x {
                out.insert(Ref {
                    line,
                    text: x.to_string(),
                });
            }
        });
    }
    out.into_iter().collect()
}

/// What is wrong with `w` run as service `name` on a box whose services run `all`, by
/// name (the one named `name`, if any, is what `w` replaces): a service, goal or fact it
/// names that is not there, a type, and a cycle of AFTERs across services.
pub fn check(name: &str, w: &Weave, mut all: BTreeMap<String, Weave>) -> Vec<Problem> {
    all.insert(name.to_string(), w.clone());
    let mut problems = Vec::new();
    for g in &w.goals {
        for a in &g.after {
            let Some(s) = &a.service else { continue };
            let why = if s == name {
                format!("{s} is this service: write AFTER {}", a.goal)
            } else {
                match all.get(s) {
                    None => format!("no service {s} on this box"),
                    Some(o) if o.goal(&a.goal).is_none() => format!("{s} has no GOAL {}", a.goal),
                    Some(_) => continue,
                }
            };
            problems.push(Problem::new(g.line, format!("AFTER {a}: {why}")));
        }
    }
    for (line, e, _) in check::conditions(w) {
        e.walk(&mut |x| {
            if let Expr::Other(s, f) = x
                && s == name
            {
                problems.push(Problem::new(line, format!("`{x}`: {s} is this service: write {f}")));
            }
        });
    }
    check::others(w, &all, &mut problems);
    cycles(name, &all, &mut problems);
    problems.sort_by_key(|p| p.line);
    problems.dedup();
    problems
}

/// One problem per AFTER cycle through a goal of `name`, on that goal's line. A cycle
/// within one weave is the weave's own check's to find.
fn cycles(name: &str, all: &BTreeMap<String, Weave>, problems: &mut Vec<Problem>) {
    type Node<'a> = (&'a str, &'a str);
    /// The goals the goal `(service, goal)` comes AFTER, on the box.
    fn next<'a>(all: &'a BTreeMap<String, Weave>, s: &str, g: &str) -> Vec<Node<'a>> {
        let Some(goal) = all.get(s).and_then(|w| w.goal(g)) else {
            return Vec::new();
        };
        goal.after
            .iter()
            .filter_map(|a| {
                let (k, w) = all.get_key_value(a.service.as_deref().unwrap_or(s))?;
                Some((k.as_str(), w.goal(&a.goal)?.name.as_str()))
            })
            .collect()
    }
    let Some((own, w)) = all.get_key_value(name) else {
        return;
    };
    for g in &w.goals {
        let start: Node = (own.as_str(), g.name.as_str());
        // Depth first from the goal until a path leads back to it.
        let mut stack: Vec<(Node, Vec<Node>)> = vec![(start, vec![start])];
        let mut seen: BTreeSet<Node> = BTreeSet::new();
        'walk: while let Some(((s, at), path)) = stack.pop() {
            for n in next(all, s, at) {
                if n == start {
                    if path.iter().all(|(s, _)| *s == own.as_str()) {
                        continue;
                    }
                    let shown: Vec<String> = path
                        .iter()
                        .chain([&start])
                        .map(|(s, g)| format!("{s}:{g}"))
                        .collect();
                    problems.push(Problem::new(
                        g.line,
                        format!("AFTER cycle across services: {}", shown.join(" → ")),
                    ));
                    break 'walk;
                }
                if seen.insert(n) {
                    let mut p = path.clone();
                    p.push(n);
                    stack.push((n, p));
                }
            }
        }
    }
}
