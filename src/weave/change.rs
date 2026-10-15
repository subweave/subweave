//! What a new Weavefile changes, read from the parsed weaves, never the text. FACT, GOAL,
//! ACTION, SKILL and KNOWN items an agent may change alone; every other item is a rule,
//! and a change to one waits for the person's signed answer.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use super::Weave;

#[derive(Debug, Clone, Default, Serialize)]
pub struct Change {
    /// `GOAL running changed`, `FACT disk added`: the facts, goals, actions and skills.
    pub free: Vec<String>,
    /// The Weavefile lines of each rule item only the old weave has, in file order.
    pub removed: Vec<String>,
    /// The Weavefile lines of each rule item only the new weave has, in file order.
    pub added: Vec<String>,
    /// Whether anything but facts, goals, actions and skills differs, read from the whole
    /// weave: a kind of item `removed` and `added` do not list yet still counts.
    #[serde(skip)]
    rules: bool,
}

impl Change {
    /// Whether the rules differ: the change waits for the person's signed answer.
    pub fn rules(&self) -> bool {
        self.rules
    }

    /// `GOAL running changed, FACT disk added`, or what says nothing of them changed.
    pub fn summary(&self) -> String {
        if self.free.is_empty() {
            "no fact, goal, action, skill or KNOWN line changed".into()
        } else {
            self.free.join(", ")
        }
    }

    /// The rule lines that differ, `- ` removed and `+ ` added.
    pub fn diff(&self) -> Vec<String> {
        let minus = self.removed.iter().map(|l| format!("- {l}"));
        minus.chain(self.added.iter().map(|l| format!("+ {l}"))).collect()
    }
}

/// `old` and `new`, parsed from `old_text` and `new_text`.
pub fn compare(old_text: &str, old: &Weave, new_text: &str, new: &Weave) -> Change {
    let mut change = Change::default();
    for (what, o, n) in [
        ("FACT", named(&old.facts, |f| &f.name), named(&new.facts, |f| &f.name)),
        ("PART", named(&old.parts, |p| &p.name), named(&new.parts, |p| &p.name)),
        ("GOAL", named(&old.goals, |g| &g.name), named(&new.goals, |g| &g.name)),
        ("ACTION", named(&old.actions, |a| &a.name), named(&new.actions, |a| &a.name)),
        ("SKILL", named(&old.skills, |s| &s.name), named(&new.skills, |s| &s.name)),
        ("KNOWN", known(old), known(new)),
    ] {
        for name in o.keys().chain(n.keys()).collect::<BTreeSet<_>>() {
            let how = match (o.get(name), n.get(name)) {
                (Some(a), Some(b)) if a == b => continue,
                (Some(_), Some(_)) => "changed",
                (Some(_), None) => "removed",
                (None, _) => "added",
            };
            change.free.push(format!("{what} {name} {how}"));
        }
    }
    let (o, n) = (rules(old_text, old), rules(new_text, new));
    let ok: Vec<&Value> = o.iter().map(|(k, _)| k).collect();
    let nk: Vec<&Value> = n.iter().map(|(k, _)| k).collect();
    for (k, lines) in &o {
        if !nk.contains(&k) {
            change.removed.extend(lines.iter().cloned());
        }
    }
    for (k, lines) in &n {
        if !ok.contains(&k) {
            change.added.extend(lines.iter().cloned());
        }
    }
    change.rules = rest(old) != rest(new);
    change
}

/// The weave without its facts, goals, actions, skills and KNOWN lines and without line
/// numbers, each list in a fixed order: moving an item is no change. An action's
/// `CHANGES` stays: it decides which rules by effect gate the action, so saying less
/// than every effect is a rule change.
fn rest(w: &Weave) -> Value {
    let mut v = unlined(w);
    if let Value::Object(map) = &mut v {
        for free in ["facts", "parts", "goals", "actions", "skills", "known"] {
            map.remove(free);
        }
        let changes: Vec<Value> = w
            .actions
            .iter()
            .filter_map(|a| Some(serde_json::json!([a.name, a.changes.as_ref()?])))
            .collect();
        map.insert("changes".into(), Value::Array(changes));
        for item in map.values_mut() {
            if let Value::Array(items) = item {
                items.sort_by_cached_key(Value::to_string);
            }
        }
    }
    v
}

/// Each item by name, as its JSON without line numbers.
fn named<T: Serialize>(items: &[T], name: impl Fn(&T) -> &String) -> BTreeMap<String, Value> {
    items
        .iter()
        .map(|i| (name(i).clone(), unlined(i)))
        .collect()
}

/// Each KNOWN line by its goal and sample: `quiet "<line>"`.
fn known(w: &Weave) -> BTreeMap<String, Value> {
    w.known
        .iter()
        .map(|k| (format!("{} {:?}", k.goal, k.sample), unlined(k)))
        .collect()
}

/// Every rule item (ABOUT, FROM, NEEDS, MODEL, each ARG, SECRET, NEVER, ASK and DRILL) as
/// its JSON without line numbers, with the Weavefile lines that write it, in file order. A
/// DRILL breaks the service on purpose: only the person adds or changes one.
fn rules(text: &str, w: &Weave) -> Vec<(Value, Vec<String>)> {
    let lines: Vec<&str> = text.lines().collect();
    let keyword = |word: &str| -> Vec<String> {
        lines
            .iter()
            .filter(|l| l.trim_start().split_whitespace().next() == Some(word))
            .map(|l| l.trim().to_owned())
            .collect()
    };
    // From `first` to `last` (1-based), and on while a line ends with `\`.
    let span = |first: u32, last: u32| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut i = first.max(1) as usize - 1;
        while i < lines.len() {
            let go_on = out.last().is_some_and(|l| l.ends_with('\\'));
            if i >= last as usize && !go_on {
                break;
            }
            out.push(lines[i].trim().to_owned());
            i += 1;
        }
        out
    };
    let mut items = Vec::new();
    if let Some(about) = &w.about {
        items.push((Value::String(format!("ABOUT {about}")), keyword("ABOUT")));
    }
    if let Some(from) = &w.from {
        items.push((unlined(from), span(from.line, from.line)));
    }
    if w.needs.line > 0 {
        items.push((unlined(&w.needs), span(w.needs.line, w.needs.line)));
    }
    if w.model.decide.is_some() || !w.model.code.is_empty() {
        items.push((unlined(&w.model), keyword("MODEL")));
    }
    for a in &w.args {
        let last = match &a.kind {
            super::ArgKind::Options(opts) => opts.iter().map(|o| o.line).max().unwrap_or(a.line),
            super::ArgKind::Value { .. } => a.line,
        };
        items.push((unlined(a), span(a.line, last.max(a.line))));
    }
    for s in &w.secrets {
        items.push((unlined(s), span(s.line, s.line)));
    }
    for r in &w.rules {
        items.push((unlined(r), span(r.line(), r.line())));
    }
    for d in &w.drills {
        items.push((unlined(d), span(d.line, d.end)));
    }
    items
}

/// `item` as JSON, every `line` field taken out: moving an item is no change.
fn unlined<T: Serialize>(item: &T) -> Value {
    let mut v = serde_json::to_value(item).unwrap_or(Value::Null);
    strip(&mut v);
    v
}

fn strip(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.remove("line");
            map.values_mut().for_each(strip);
        }
        Value::Array(items) => items.iter_mut().for_each(strip),
        _ => {}
    }
}
