//! Memory: a judgment the box made, or was told, kept as data so code applies it next
//! time. A log line's kind is its template; each kind is known normal or trouble by a
//! person's or agent's word, the weave's KNOWN, or two agreeing model verdicts. An action
//! that fixed a goal failing on a known trouble kind twice runs for it without the
//! decider. A PART's release is a security fix or not by a person's or agent's word, or
//! the model's one verdict on its notes. The state lives in the journal's fold
//! (`state.rs`), from `verdict`, `known`, `release` and `action` events.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::weave::{Class, Weave};

/// Model verdicts in a row that make a kind known.
pub const AGREE: usize = 2;
/// A model verdict at or above this p (that the line shows trouble) says trouble…
pub const TROUBLE_P: f64 = 0.8;
/// …and at or below this, normal.
pub const NORMAL_P: f64 = 0.2;
/// Passes of one action for (goal, known trouble kind) after which it runs without the
/// decider.
pub const CHOICE_PASSES: u32 = 2;
/// A model verdict at or above this p says a release is a security fix; one verdict per
/// release decides.
pub const SECURITY_P: f64 = 0.5;

/// What a template shows where a value was.
const MASK: &str = "<*>";

/// A line's template: the line with every value masked, so lines of one kind share it.
/// Masked: quoted values, any word with a digit (numbers, times, hex, UUIDs, IPs,
/// durations, a path's numeric parts), hex words of 8 letters or more, base64 runs, and a
/// month or weekday name before a time. A word is letters, digits and `_`, joined by `.`,
/// `:` or `-` between two of them. Runs of spaces read as one.
pub fn template(line: &str) -> String {
    let c: Vec<char> = line.trim().chars().collect();
    let n = c.len();
    let word = |x: char| x.is_alphanumeric() || x == '_';
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < n {
        let x = c[i];
        if x.is_whitespace() {
            while i < n && c[i].is_whitespace() {
                i += 1;
            }
            out.push(' ');
            continue;
        }
        // `"…"` always; `'…'` only outside a word, so `doesn't` stays.
        let quote = x == '"' || (x == '\'' && (i == 0 || !word(c[i - 1])));
        if quote && let Some(end) = closing(&c, i) {
            out.push_str(MASK);
            i = end + 1;
            continue;
        }
        if !word(x) {
            out.push(x);
            i += 1;
            continue;
        }
        let b64 = base64_end(&c, i);
        if b64 > i {
            out.push_str(MASK);
            i = b64;
            continue;
        }
        let start = i;
        while i < n
            && (word(c[i])
                || (matches!(c[i], '.' | ':' | '-') && i + 1 < n && word(c[i + 1]) && word(c[i - 1])))
        {
            i += 1;
        }
        let w: String = c[start..i].iter().collect();
        let next = next_word(&c, i);
        let date = (is_month(&w) && next.starts_with(|d: char| d.is_ascii_digit()))
            || (is_weekday(&w)
                && (is_month(&next) || next.starts_with(|d: char| d.is_ascii_digit())));
        if date || variable(&w) {
            out.push_str(MASK);
        } else {
            out.push_str(&w);
        }
    }
    out.trim_end().to_string()
}

/// The index of the quote closing the one at `open`, on the same line.
fn closing(c: &[char], open: usize) -> Option<usize> {
    let q = c[open];
    let mut k = open + 1;
    while k < c.len() {
        match c[k] {
            '\\' => k += 1,
            x if x == q => {
                let ends_word = q == '"'
                    || c.get(k + 1).is_none_or(|y| !(y.is_alphanumeric() || *y == '_'));
                if ends_word {
                    return Some(k);
                }
            }
            _ => {}
        }
        k += 1;
    }
    None
}

/// The end of a base64 run starting at `i`: 16 or more of `A-Za-z0-9+/` with a digit,
/// a capital and a small letter, then any `=`; `i` when there is none.
fn base64_end(c: &[char], i: usize) -> usize {
    let mut k = i;
    let (mut digit, mut upper, mut lower) = (false, false, false);
    while k < c.len() && (c[k].is_ascii_alphanumeric() || c[k] == '+' || c[k] == '/') {
        digit |= c[k].is_ascii_digit();
        upper |= c[k].is_ascii_uppercase();
        lower |= c[k].is_ascii_lowercase();
        k += 1;
    }
    if k - i < 16 || !(digit && upper && lower) {
        return i;
    }
    while k < c.len() && c[k] == '=' {
        k += 1;
    }
    k
}

/// The word after the spaces at `i`.
fn next_word(c: &[char], mut i: usize) -> String {
    while i < c.len() && c[i] == ' ' {
        i += 1;
    }
    c[i..]
        .iter()
        .take_while(|x| x.is_alphanumeric())
        .collect()
}

fn variable(w: &str) -> bool {
    w.bytes().any(|b| b.is_ascii_digit()) || (w.len() >= 8 && w.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_month(w: &str) -> bool {
    const MONTHS: [&str; 12] = [
        "january", "february", "march", "april", "may", "june", "july", "august", "september",
        "october", "november", "december",
    ];
    named(w, &MONTHS)
}

fn is_weekday(w: &str) -> bool {
    const DAYS: [&str; 7] = [
        "monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday",
    ];
    named(w, &DAYS)
}

/// `w` is one of `names` or its first three letters, in any case.
fn named(w: &str, names: &[&str]) -> bool {
    let l = w.to_ascii_lowercase();
    names.iter().any(|n| l == *n || (l.len() == 3 && n.starts_with(&l)))
}

/// The weave's KNOWN lines: per goal, per template.
pub type Taught = BTreeMap<String, BTreeMap<String, Class>>;

pub fn taught(w: &Weave) -> Taught {
    let mut out = Taught::new();
    for k in &w.known {
        out.entry(k.goal.clone())
            .or_default()
            .insert(template(&k.sample), k.class);
    }
    out
}

/// Who knows a kind's class, in order of authority.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A person or an agent said so on this box.
    Told { by: Option<String> },
    /// The weave's KNOWN.
    Weave,
    /// `AGREE` model verdicts in a row.
    Model,
}

impl Source {
    /// `cli:aram`, `the weave`, `the model`.
    pub fn who(&self) -> String {
        match self {
            Source::Told { by } => by.clone().unwrap_or_else(|| "someone".into()),
            Source::Weave => "the weave".into(),
            Source::Model => "the model".into(),
        }
    }
}

/// What the box knows of kinds of lines and of releases, folded from the journal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Memory {
    /// Per goal, per template.
    pub goals: BTreeMap<String, BTreeMap<String, Entry>>,
    /// Per PART, per release version: whether it fixes a security issue.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub releases: BTreeMap<String, BTreeMap<String, Release>>,
}

/// One release of a part: the model's verdict on its notes, and a person's word.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Release {
    /// The model's p that the notes show a security fix or a high priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told: Option<ToldRelease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToldRelease {
    pub security: bool,
    pub by: Option<String>,
    pub why: Option<String>,
    pub t: String,
}

/// One kind of line for one goal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entry {
    /// The newest line of it the box judged or was told about.
    pub sample: String,
    /// What a person or an agent said it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub told: Option<Told>,
    /// The model's newest verdicts, the p that it shows trouble, oldest first, at most
    /// `AGREE`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verdicts: Vec<f64>,
    /// Per action, its passes in a row for the goal failing on this kind.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub choices: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Told {
    pub class: Class,
    pub by: Option<String>,
    pub why: Option<String>,
    pub t: String,
}

impl Memory {
    fn entry(&mut self, goal: &str, line: &str) -> &mut Entry {
        self.goals
            .entry(goal.to_string())
            .or_default()
            .entry(template(line))
            .or_default()
    }

    /// The model judged `line` for `goal`: `p` that it shows trouble.
    pub fn verdict(&mut self, goal: &str, line: &str, p: f64) {
        let e = self.entry(goal, line);
        e.sample = line.to_string();
        e.verdicts.push(p);
        if e.verdicts.len() > AGREE {
            e.verdicts.remove(0);
        }
    }

    /// A person or an agent said what lines like `line` are for `goal`; `None` forgets
    /// everything the box holds of them: what it was told, the verdicts, the choices.
    pub fn told(&mut self, goal: &str, line: &str, told: Option<Told>) {
        match told {
            Some(t) => {
                let e = self.entry(goal, line);
                e.sample = line.to_string();
                e.told = Some(t);
            }
            None => {
                if let Some(kinds) = self.goals.get_mut(goal) {
                    kinds.remove(&template(line));
                    if kinds.is_empty() {
                        self.goals.remove(goal);
                    }
                }
            }
        }
    }

    /// `action` ran for `goal` failing on the kinds of the lines `on`: a pass counts
    /// toward running it without the decider, a failure drops that.
    pub fn ran(&mut self, goal: &str, on: &[String], action: &str, ok: bool) {
        for line in on {
            let e = self.entry(goal, line);
            if e.sample.is_empty() {
                e.sample = line.clone();
            }
            if ok {
                *e.choices.entry(action.to_string()).or_default() += 1;
            } else {
                e.choices.remove(action);
            }
        }
    }

    /// What lines of `template` are for `goal`, and who says so, by authority: told on
    /// this box, then the weave's KNOWN, then the model.
    pub fn class(&self, taught: &Taught, goal: &str, template: &str) -> Option<(Class, Source)> {
        let e = self.goals.get(goal).and_then(|k| k.get(template));
        if let Some(t) = e.and_then(|e| e.told.as_ref()) {
            return Some((t.class, Source::Told { by: t.by.clone() }));
        }
        if let Some(c) = taught.get(goal).and_then(|k| k.get(template)) {
            return Some((*c, Source::Weave));
        }
        let v = &e?.verdicts;
        if v.len() < AGREE {
            None
        } else if v.iter().all(|p| *p >= TROUBLE_P) {
            Some((Class::Trouble, Source::Model))
        } else if v.iter().all(|p| *p <= NORMAL_P) {
            Some((Class::Normal, Source::Model))
        } else {
            None
        }
    }

    /// The action that passed `CHOICE_PASSES` times in a row for `goal` failing on lines
    /// of `template`, the one with the most passes.
    pub fn choice(&self, goal: &str, template: &str) -> Option<(&str, u32)> {
        let e = self.goals.get(goal)?.get(template)?;
        e.choices
            .iter()
            .filter(|(_, n)| **n >= CHOICE_PASSES)
            .max_by_key(|(_, n)| **n)
            .map(|(a, n)| (a.as_str(), *n))
    }

    fn release(&mut self, part: &str, version: &str) -> &mut Release {
        self.releases
            .entry(part.to_string())
            .or_default()
            .entry(version.to_string())
            .or_default()
    }

    /// The model judged release `version` of `part`: `p` that it is a security fix.
    pub fn release_judged(&mut self, part: &str, version: &str, p: f64) {
        self.release(part, version).p = Some(p);
    }

    /// A person or an agent said whether release `version` of `part` is a security fix.
    pub fn release_told(&mut self, part: &str, version: &str, told: ToldRelease) {
        self.release(part, version).told = Some(told);
    }

    /// Whether release `version` of `part` is a security fix, and who says so: told on
    /// this box, then the model's one verdict.
    pub fn security(&self, part: &str, version: &str) -> Option<(bool, Source)> {
        let r = self.releases.get(part)?.get(version)?;
        if let Some(t) = &r.told {
            return Some((t.security, Source::Told { by: t.by.clone() }));
        }
        r.p.map(|p| (p >= SECURITY_P, Source::Model))
    }
}
