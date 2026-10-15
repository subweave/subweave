//! A service's journal: one JSON line per event, kept in memory while its loop runs.

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Duration;

use crate::paths;
use crate::service::{self as svc, Kind, Line, TASK, WEAVE};

use super::{epoch, epoch_of, now, redact};

pub(crate) struct Journal {
    service: String,
    lines: Vec<Line>,
    /// Model spend not yet on a line; the next line carries it.
    pending: f64,
    secrets: Vec<(String, String)>,
}

impl Journal {
    pub fn open(service: &str, secrets: Vec<(String, String)>) -> Journal {
        let lines = svc::journal(service).unwrap_or_else(|e| {
            tracing::error!(service, error = %format!("{e:#}"), "cannot read the journal");
            Vec::new()
        });
        Journal {
            service: service.to_string(),
            lines,
            pending: 0.0,
            secrets,
        }
    }

    /// Writes a line: the time and the running episode set (and its job, on its first
    /// line), secrets redacted, pending spend added.
    pub fn add(&mut self, mut l: Line) -> Line {
        if l.t.is_empty() {
            l.t = now();
        }
        if l.ep.is_none() {
            if let Some((ep, job)) = super::episode::for_line() {
                l.ep = Some(ep);
                l.job = l.job.or(job);
            }
        }
        for s in [&mut l.text, &mut l.body, &mut l.output].into_iter().flatten() {
            *s = redact(s, &self.secrets);
        }
        if let Some(f) = &mut l.facts {
            for v in f.values_mut() {
                *v = redact(v, &self.secrets);
            }
        }
        // A line with a model's usage keeps that model's cost alone.
        if self.pending > 0.0 && l.usage.is_none() {
            l.cost_usd = Some(l.cost_usd.unwrap_or(0.0) + self.pending);
            self.pending = 0.0;
        }
        append(&self.service, &l);
        self.lines.push(l.clone());
        l
    }

    pub fn note(&mut self, goal: Option<&str>, text: impl Into<String>) {
        self.add(Line {
            kind: Kind::Note,
            goal: goal.map(str::to_string),
            text: Some(text.into()),
            ..Default::default()
        });
    }

    /// A note the person caused; `by` says who.
    pub fn note_by(&mut self, goal: Option<&str>, text: impl Into<String>, by: &str) {
        self.add(Line {
            kind: Kind::Note,
            goal: goal.map(str::to_string),
            text: Some(text.into()),
            by: Some(by.to_string()).filter(|b| !b.is_empty()),
            ..Default::default()
        });
    }

    /// Spend that goes on the next line.
    pub fn spend(&mut self, cost: f64) {
        self.pending += cost;
    }

    /// The sum of `cost_usd` in this month's lines, and the spend not written yet.
    pub fn month_spend(&self) -> f64 {
        let month = &now()[..7];
        self.lines
            .iter()
            .filter(|l| l.t.starts_with(month))
            .filter_map(|l| l.cost_usd)
            .sum::<f64>()
            + self.pending
    }

    /// Time since `action` last passed.
    pub fn since_pass(&self, action: &str) -> Option<Duration> {
        let l = self.lines.iter().rev().find(|l| {
            l.kind == Kind::Action && l.ok == Some(true) && l.action.as_deref() == Some(action)
        })?;
        let secs = epoch() - epoch_of(&l.t)?;
        Some(Duration::from_secs(secs.max(0) as u64))
    }

    /// `action` lines for `goal` within the last `per`.
    pub fn tries(&self, goal: &str, per: Duration) -> u32 {
        let from = epoch() - per.as_secs() as i64;
        self.lines
            .iter()
            .filter(|l| l.kind == Kind::Action && l.goal.as_deref() == Some(goal))
            .filter(|l| epoch_of(&l.t).is_some_and(|t| t >= from))
            .count() as u32
    }

    pub fn ask(&self, n: u64) -> Option<&Line> {
        self.lines
            .iter()
            .find(|l| l.kind == Kind::Ask && l.n == Some(n))
    }

    /// The rule an ask was raised for: its `gate` line.
    pub fn gate(&self, n: u64) -> Option<&Line> {
        self.lines
            .iter()
            .find(|l| l.kind == Kind::Gate && l.n == Some(n))
    }

    /// The lines of the person's task that raised ask `n`, from its start line on.
    pub fn task_lines(&self, n: u64) -> Option<&[Line]> {
        task_lines(&self.lines, n)
    }

    pub fn answered(&self, n: u64) -> bool {
        self.lines
            .iter()
            .any(|l| l.kind == Kind::Answer && l.n == Some(n))
    }

    /// Whether a person's task waits for the answer to its ask.
    pub fn task_waits(&self) -> bool {
        svc::open_asks(&self.lines)
            .iter()
            .any(|l| l.action.as_deref() == Some(TASK))
    }

    /// The open asks for `goal` the loop raised on its own: none a person's job raised,
    /// which carry `by`.
    pub fn open_for(&self, goal: &str) -> Vec<u64> {
        svc::open_asks(&self.lines)
            .iter()
            .filter(|l| l.goal.as_deref() == Some(goal) && l.by.is_none())
            .filter_map(|l| l.n)
            .collect()
    }

    /// The newest `always` or `never` the person gave for this rule and action.
    pub fn kept(&self, rule: &str, action: &str) -> Option<&'static str> {
        self.lines.iter().rev().find_map(|l| {
            if l.kind != Kind::Answer {
                return None;
            }
            let word = match l.text.as_deref() {
                Some("always") => "always",
                Some("never") => "never",
                _ => return None,
            };
            let g = self.gate(l.n?)?;
            (g.text.as_deref() == Some(rule) && g.action.as_deref() == Some(action)).then_some(word)
        })
    }

    pub fn open_asks(&self) -> Vec<u64> {
        svc::open_asks(&self.lines)
            .iter()
            .filter_map(|l| l.n)
            .collect()
    }

    /// Per goal (`None`: the box), its newest ask and that ask's answer with its time.
    pub fn latest_asks(&self) -> BTreeMap<Option<String>, (u64, Option<(String, i64)>)> {
        let mut out: BTreeMap<Option<String>, (u64, Option<(String, i64)>)> = BTreeMap::new();
        let mut answers: BTreeMap<u64, (String, i64)> = BTreeMap::new();
        for l in &self.lines {
            match l.kind {
                // A person's task or a weave change parks no goal and not the box.
                Kind::Ask if !matches!(l.action.as_deref(), Some(TASK | WEAVE)) => {
                    if let Some(n) = l.n {
                        out.insert(l.goal.clone(), (n, None));
                    }
                }
                Kind::Answer => {
                    if let (Some(n), Some(w)) = (l.n, &l.text) {
                        answers
                            .entry(n)
                            .or_insert((w.clone(), epoch_of(&l.t).unwrap_or(0)));
                    }
                }
                _ => {}
            }
        }
        for (n, a) in out.values_mut() {
            *a = answers.get(n).cloned();
        }
        out
    }

    /// Whether the person said yes to a box that does not fit the weave.
    pub fn box_approved(&self) -> bool {
        self.lines.iter().any(|l| {
            l.kind == Kind::Answer
                && l.goal.is_none()
                && matches!(l.text.as_deref(), Some("yes" | "always"))
                && l.n.is_some_and(|n| {
                    self.ask(n)
                        .is_some_and(|a| a.goal.is_none() && a.action.is_none())
                        && self.gate(n).is_none()
                })
        })
    }

    /// The newest action outcomes for `goal`, short, for the decider. Only outcomes:
    /// with the failed checks and the asks beside them, the decider (jev-1.13) chose
    /// `ask` over a fitting restart after one recurrence, and repeat failures are TRIES'
    /// job, counted in code.
    pub fn recent(&self, goal: &str, k: usize) -> Vec<String> {
        let mut out: Vec<String> = self
            .lines
            .iter()
            .rev()
            .filter(|l| l.kind == Kind::Action && l.goal.as_deref() == Some(goal))
            .filter_map(|l| {
                let hm = l.t.get(11..16).unwrap_or("");
                let outcome = if l.ok == Some(true) {
                    "passed"
                } else {
                    "failed"
                };
                Some(format!("{hm} {}: {outcome}", l.action.as_deref()?))
            })
            .take(k)
            .collect();
        out.reverse();
        out
    }
}

/// The lines from the start of the person's task that raised ask `n` (the newest start
/// line before it) to the end. Tasks do not interleave: a queued task waits while one
/// waits for its answer.
pub(crate) fn task_lines(lines: &[Line], n: u64) -> Option<&[Line]> {
    let at = lines
        .iter()
        .position(|l| l.kind == Kind::Ask && l.n == Some(n))?;
    let start = lines[..at].iter().rposition(|l| {
        l.kind == Kind::Action && l.action.as_deref() == Some(TASK) && l.ok.is_none()
    })?;
    Some(&lines[start..])
}

/// Appends one line to a service's journal; the one place lines are written, so the trace
/// of the line's episode gets it too.
pub(crate) fn append(service: &str, l: &Line) {
    let path = paths::journal(service);
    let write = || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        let mut bytes = serde_json::to_vec(l)?;
        bytes.push(b'\n');
        f.write_all(&bytes)
    };
    if let Err(e) = write() {
        tracing::error!(path = %path.display(), error = %e, "cannot write a journal line");
    }
    super::trace::line(service, l);
}

/// A note in the journal of a service whose loop is not running; `by`, when not empty,
/// says who caused it.
pub(crate) fn note(service: &str, text: &str, by: &str) {
    append(
        service,
        &Line {
            t: now(),
            kind: Kind::Note,
            text: Some(text.to_string()),
            by: Some(by.to_string()).filter(|b| !b.is_empty()),
            ..Default::default()
        },
    );
}
