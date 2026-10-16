//! The card: what a person or an agent reads on an escalation or a message. The daemon
//! builds it once, from the weave and the service's fold, and keeps it in the escalation;
//! every channel renders this one model. It shows the read set of the verdict, never
//! every fact.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Card {
    /// The question the buttons answer, or a message's headline.
    pub title: String,
    /// The goal that does not hold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<GoalShown>,
    /// The facts the goal's condition reads (and the action's and its rule's), as now.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<Read>,
    /// The window the goal is judged over, when it is judged over one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowShown>,
    /// The parts whose facts it reads: their versions and the newer releases' notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartShown>,
    /// The action picked or held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<ActionShown>,
    /// The action an UNDO undoes, as its button says it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undoes: Option<String>,
    /// The drill that caused it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<DrillShown>,
    /// What else the reader needs: what was tried, what it asks. Plain sentences.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// A model's plain lines on why the verdict is what it is; none when it had none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summary: Vec<String>,
    /// The answers, in button order; none on a message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<Button>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalShown {
    pub name: String,
    /// What it keeps true, in the weave's words.
    pub sentence: String,
    pub check: Check,
}

/// How a goal is judged: an expression in code, or a question to a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Check {
    Code(String),
    Quoted(String),
}

/// A fact as it reads now, with its unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Read {
    pub fact: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowShown {
    /// `12s`, `15m`.
    pub over: String,
    /// The reads of each fact a window function takes, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Series>,
    /// Its lines grouped by kind, when it reads LINES facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Kinds>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub fact: String,
    pub values: Vec<String>,
    /// Older reads left out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Kinds {
    pub kinds: Vec<KindShown>,
    /// Kinds left out.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: usize,
}

/// One kind of line: lines that differ only in numbers, ids and quoted values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindShown {
    pub count: usize,
    /// Its newest line.
    pub sample: String,
    /// What it is, when the box knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known: Option<Known>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// `normal` or `trouble`.
    pub class: String,
    /// Who says so: `the weave`, `the model`, `cli:aram`.
    pub by: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartShown {
    pub name: String,
    pub running: String,
    pub newest: String,
    /// `yes`: a newer release fixes a security issue.
    pub security: String,
    /// The first lines of the newer releases' notes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionShown {
    pub name: String,
    /// Its button.
    pub label: String,
    /// What it does: its DO tasks, or the RUN it is.
    pub does: Vec<String>,
    /// Its effects; empty: it changes nothing.
    pub changes: Vec<String>,
    /// The rule that holds it, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<Held>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Held {
    /// As the box reads it: `ASK BEFORE CHANGES data`.
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub because: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrillShown {
    pub name: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    /// `yes`, `no`, `always`, `never` or an action's name.
    pub word: String,
    pub label: String,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// One line: a value with line breaks reads as one.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn stop(s: &str) -> &str {
    s.trim().trim_end_matches('.')
}

impl Card {
    /// A message's card: a headline and plain lines.
    pub fn note(title: impl Into<String>, lines: Vec<String>) -> Card {
        Card {
            title: title.into(),
            lines,
            ..Card::default()
        }
    }

    pub fn words(&self) -> Vec<String> {
        self.buttons.iter().map(|b| b.word.clone()).collect()
    }

    /// The button for `word`: its label, else the word.
    pub fn label<'a>(&'a self, word: &'a str) -> &'a str {
        self.buttons
            .iter()
            .find(|b| b.word == word)
            .map_or(word, |b| b.label.as_str())
    }

    /// Passes every text read from the box or written by a person through `f`.
    pub fn redact(&mut self, f: &dyn Fn(&str) -> String) {
        let mut each = |s: &mut String| *s = f(s);
        each(&mut self.title);
        for r in &mut self.reads {
            each(&mut r.value);
        }
        if let Some(w) = &mut self.window {
            w.values.iter_mut().flat_map(|s| &mut s.values).for_each(&mut each);
            if let Some(k) = &mut w.lines {
                k.kinds.iter_mut().for_each(|k| each(&mut k.sample));
            }
        }
        for p in &mut self.parts {
            p.notes.iter_mut().for_each(&mut each);
        }
        self.lines.iter_mut().for_each(&mut each);
        self.summary.iter_mut().for_each(&mut each);
    }

    /// The card in reading order, one plain sentence a line; the title and the summary
    /// are apart.
    pub fn sentences(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(d) = &self.drill {
            out.push(format!(
                "This is drill {} ({}): Subweave caused it on purpose, to test the weave.",
                d.name,
                stop(&d.label)
            ));
        }
        let over = self
            .window
            .as_ref()
            .map(|w| format!(" over the last {}", w.over))
            .unwrap_or_default();
        if let Some(g) = &self.goal {
            out.push(format!("Not true now: {}.", stop(&g.sentence)));
            out.push(match &g.check {
                Check::Code(e) => format!("Goal `{}` checks `{e}`{over}.", g.name),
                Check::Quoted(t) => format!("Goal `{}` asks a model{over}: {}.", g.name, stop(t)),
            });
        }
        for r in &self.reads {
            let v = one_line(&r.value);
            out.push(format!(
                "`{}` now: {}",
                r.fact,
                if v.is_empty() { "nothing" } else { &v }
            ));
        }
        if let Some(w) = &self.window {
            for s in &w.values {
                let older = if s.more > 0 { "…, " } else { "" };
                out.push(format!(
                    "`{}` in the window: {older}{}",
                    s.fact,
                    s.values.iter().map(|v| one_line(v)).collect::<Vec<_>>().join(", ")
                ));
            }
            if let Some(k) = &w.lines {
                if k.kinds.is_empty() {
                    out.push("No lines in the window.".into());
                }
                for x in &k.kinds {
                    let known = match &x.known {
                        Some(k) => format!("known {} ({})", k.class, k.by),
                        None => "not known yet".into(),
                    };
                    out.push(format!("{}× {known}: `{}`", x.count, one_line(&x.sample)));
                }
                if k.more > 0 {
                    out.push(format!("…and {} more kinds of line.", k.more));
                }
            }
        }
        for p in &self.parts {
            out.push(format!(
                "Part `{}` runs {}; the newest release is {}; a security release waits: {}.",
                p.name, p.running, p.newest, p.security
            ));
            if !p.notes.is_empty() {
                out.push(format!(
                    "Its newer releases' notes begin: {}",
                    p.notes.iter().map(|l| one_line(l)).collect::<Vec<_>>().join(" / ")
                ));
            }
        }
        if let Some(a) = &self.action {
            out.push(format!(
                "The action is “{}”: {}.",
                stop(&a.label),
                stop(&a.does.join("; then "))
            ));
            out.push(if a.changes.is_empty() {
                "It changes nothing.".into()
            } else {
                format!("It changes {}.", a.changes.join(", "))
            });
            if let Some(h) = &a.held {
                out.push(match &h.because {
                    Some(b) => format!("`{}` holds it: {}.", h.rule, stop(b)),
                    None => format!("`{}` holds it.", h.rule),
                });
            }
        }
        if let Some(u) = &self.undoes {
            out.push(format!("It undoes “{}”.", stop(u)));
        }
        out.extend(self.lines.iter().cloned());
        out
    }

    /// The summary, then the sentences: the card as plain text.
    pub fn text(&self) -> String {
        let mut out = String::new();
        if !self.summary.is_empty() {
            out.push_str(&self.summary.join("\n"));
            out.push_str("\n\n");
        }
        out.push_str(&self.sentences().join("\n"));
        out
    }
}
