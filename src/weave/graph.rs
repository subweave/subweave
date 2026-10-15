//! The goal graph that AFTER makes.

use std::collections::{BTreeMap, BTreeSet};

use super::ast::{Goal, Weave, show_duration};
use super::expr::{self, Env};

/// Goals in check order: a goal after every goal of this weave it names in AFTER, file
/// order within a level. Goals on an AFTER cycle are left out (the check refuses such a
/// file).
pub fn order(w: &Weave) -> Vec<&Goal> {
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    let mut out = Vec::with_capacity(w.goals.len());
    loop {
        let level: Vec<&Goal> = w
            .goals
            .iter()
            .filter(|g| !placed.contains(g.name.as_str()))
            .filter(|g| g.after.iter().filter_map(|a| a.local()).all(|a| placed.contains(a)))
            .collect();
        if level.is_empty() {
            return out;
        }
        placed.extend(level.iter().map(|g| g.name.as_str()));
        out.extend(level);
    }
}

/// Goals that are off: their IF is false, or a goal of this weave they come AFTER is
/// off. An IF that cannot be judged yet leaves the goal on.
pub fn off(w: &Weave, env: &dyn Env) -> BTreeSet<String> {
    let mut off: BTreeSet<String> = w
        .goals
        .iter()
        .filter(|g| g.when.as_ref().is_some_and(|e| expr::holds(e, env) == Some(false)))
        .map(|g| g.name.clone())
        .collect();
    loop {
        let more: Vec<String> = w
            .goals
            .iter()
            .filter(|g| {
                !off.contains(&g.name)
                    && g.after.iter().filter_map(|a| a.local()).any(|a| off.contains(a))
            })
            .map(|g| g.name.clone())
            .collect();
        if more.is_empty() {
            return off;
        }
        off.extend(more);
    }
}

/// The graph as a tree, each goal under its first AFTER goal of this weave:
///
/// ```text
/// installed
/// ├─ configured
/// │  └─ running
/// │     ├─ archiving   IF bucket
/// ```
pub fn tree(w: &Weave, off: &BTreeSet<String>) -> String {
    let mut kids: BTreeMap<&str, Vec<&Goal>> = BTreeMap::new();
    let mut roots = Vec::new();
    for g in &w.goals {
        match parent(w, g) {
            Some(p) => kids.entry(p).or_default().push(g),
            None => roots.push(g),
        }
    }
    let mut out = String::new();
    fn walk(
        w: &Weave,
        g: &Goal,
        prefix: &str,
        last: bool,
        top: bool,
        kids: &BTreeMap<&str, Vec<&Goal>>,
        off: &BTreeSet<String>,
        out: &mut String,
        depth: usize,
    ) {
        let mut tags = Vec::new();
        let p = parent(w, g);
        let rest: Vec<String> = g
            .after
            .iter()
            .filter(|a| a.local() != p || p.is_none())
            .map(ToString::to_string)
            .collect();
        if !rest.is_empty() {
            let word = if p.is_some() { "also after" } else { "after" };
            tags.push(format!("{word} {}", rest.join(", ")));
        }
        if let Some(o) = g.over {
            tags.push(format!("OVER {}", show_duration(o)));
        }
        if let Some(e) = &g.when {
            tags.push(format!("IF {e}"));
        }
        if off.contains(&g.name) {
            tags.push("off".to_string());
        }
        if !top {
            out.push_str(prefix);
            out.push_str(if last { "└─ " } else { "├─ " });
        }
        out.push_str(&g.name);
        if !tags.is_empty() {
            out.push_str("   ");
            out.push_str(&tags.join("; "));
        }
        out.push('\n');
        // A cycle cannot loop forever: no tree is deeper than the number of goals.
        if depth > kids.values().map(Vec::len).sum::<usize>() {
            return;
        }
        let children = kids.get(g.name.as_str()).map(Vec::as_slice).unwrap_or(&[]);
        let next = if top {
            String::new()
        } else {
            format!("{prefix}{}", if last { "   " } else { "│  " })
        };
        for (i, k) in children.iter().enumerate() {
            walk(
                w,
                k,
                &next,
                i + 1 == children.len(),
                false,
                kids,
                off,
                out,
                depth + 1,
            );
        }
    }
    for r in roots {
        walk(w, r, "", true, true, &kids, off, &mut out, 0);
    }
    out
}

/// The goal a tree shows `g` under: the first goal of this weave it comes AFTER.
fn parent<'a>(w: &Weave, g: &'a Goal) -> Option<&'a str> {
    g.after.iter().filter_map(|a| a.local()).find(|a| w.goal(a).is_some())
}
