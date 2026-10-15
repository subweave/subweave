//! The request check: what one sentence asks of a weave, before anything runs.
//!
//! Two decider requests, then code. The question wordings were measured on real calls
//! (`evidence/`); change them only with a new measurement.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write;
use std::time::Duration;

use anyhow::{Result, bail};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::json;

use crate::model::decider::{Answers, Decider, Question};
use crate::weave::expr::{self, Env, Value};
use crate::weave::{ArgKind, Rule, Weave, graph};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Ask,
    Warning,
    Info,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Ask => "ask",
            Level::Warning => "warning",
            Level::Info => "info",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub level: Level,
    /// An ARG or slot, or `NEVER`, `NEVER IF`, `ASK`, `ASK IF`, `request`.
    pub subject: String,
    pub text: String,
    pub p: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub request: String,
    /// Every ARG and slot with a value, defaults included.
    pub values: BTreeMap<String, String>,
    pub findings: Vec<Finding>,
    /// Goals switched off by IF, and every goal after them.
    pub off: Vec<String>,
    /// SECRETs the daemon asks for once.
    pub secrets: Vec<String>,
    pub cost_usd: f64,
}

impl Plan {
    pub fn count(&self, level: Level) -> usize {
        self.findings.iter().filter(|f| f.level == level).count()
    }
}

const UNCOVERED: &str = "The service offers only these settings: {settings}. Besides choosing values for these settings, does the request ask for any feature, extension or behaviour the service would have to add? Request: \"{request}\"";
const FIT_TASK: &str = "Check each value a person gave against what the setting expects.";

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// An option's answer key: its label when that is a plain word, else `o<i>`.
fn option_key(label: &str, i: usize) -> String {
    let plain = !label.is_empty()
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if plain {
        label.to_string()
    } else {
        format!("o{i}")
    }
}

/// The first key with the highest probability.
fn argmax<'a>(p: impl Iterator<Item = (&'a String, f64)>) -> Option<(&'a String, f64)> {
    let mut best: Option<(&String, f64)> = None;
    for (k, v) in p {
        if best.is_none_or(|(_, b)| v > b) {
            best = Some((k, v));
        }
    }
    best
}

/// Values as code reads them: an empty or missing value is no value.
struct Values<'a>(&'a IndexMap<String, Option<String>>);

impl Values<'_> {
    fn get(&self, name: &str) -> Option<&str> {
        self.0
            .get(name)
            .and_then(|v| v.as_deref())
            .filter(|v| !v.is_empty())
    }
}

impl Env for Values<'_> {
    fn value(&self, name: &str) -> Option<Value> {
        self.get(name).map(|v| Value::Text(v.to_string()))
    }
    fn truthy(&self, name: &str) -> bool {
        self.get(name).is_some()
    }
    fn since(&self, _action: &str) -> Option<Duration> {
        None
    }
    fn prev(&self, _fact: &str) -> Option<Value> {
        None
    }
}

/// Every slot of one option ARG, sorted and once each.
fn arg_slots(opts: &[crate::weave::ArgOption]) -> Vec<&str> {
    let set: BTreeSet<&str> = opts
        .iter()
        .flat_map(|o| o.slots.iter().map(String::as_str))
        .collect();
    set.into_iter().collect()
}

/// Checks `request` against the weave. With `current` (a running service's ARGs, for
/// `tell`), an unsaid ARG keeps its current value and is not reported, and each quoted
/// ASK compares the ARGs now with the ARGs after instead of reading the request.
pub async fn plan(
    w: &Weave,
    request: &str,
    decider: &Decider,
    current: Option<&BTreeMap<String, String>>,
) -> Result<Plan> {
    // A comma inside a word joins a list ("10.0.4.11,10.0.4.12"); one at its end ends a clause.
    let words: Vec<&str> = request
        .split_whitespace()
        .map(|x| x.trim_end_matches([',', ';', '.']))
        .filter(|x| !x.is_empty())
        .collect();
    let word_opts: IndexMap<String, String> = words
        .iter()
        .enumerate()
        .map(|(i, x)| (format!("w{i}"), format!("the word `{x}`")))
        .collect();
    let mut cost = 0.0;

    // 1. What the request says.
    let mut q1: IndexMap<String, Question> = IndexMap::new();
    let mut opt_keys: HashMap<&str, IndexMap<String, &crate::weave::ArgOption>> = HashMap::new();
    for a in &w.args {
        match &a.kind {
            ArgKind::Value { what } => {
                let mut criteria = word_opts.clone();
                criteria.insert("none".into(), "the request gives no value for it".into());
                q1.insert(
                    format!("slot.{}", a.name),
                    Question::Choice {
                        criteria,
                        instructions: format!(
                            "Which word of the request gives the value of `{}`: {what}? Pick a word even when its value looks wrong for the setting. Pick `none` when the request does not mention this setting.",
                            a.name
                        ),
                    },
                );
            }
            ArgKind::Options(opts) => {
                let mut criteria = IndexMap::new();
                let keys = opt_keys.entry(a.name.as_str()).or_default();
                for (i, o) in opts.iter().enumerate() {
                    let k = option_key(&o.label, i);
                    let text = if k == o.label {
                        o.meaning.clone()
                    } else {
                        format!("`{}`: {}", o.label, o.meaning)
                    };
                    criteria.insert(k.clone(), text);
                    keys.insert(k, o);
                }
                criteria.insert(
                    "other".into(),
                    "the request says something about this setting that no option above fits"
                        .into(),
                );
                criteria.insert(
                    "unsaid".into(),
                    "the request does not mention this setting".into(),
                );
                q1.insert(
                    format!("arg.{}", a.name),
                    Question::Choice {
                        criteria,
                        instructions: format!(
                            "Which option does the request choose for `{}`?",
                            a.name
                        ),
                    },
                );
                for s in arg_slots(opts) {
                    let tag = format!("<{s}>");
                    let Some(o) = opts.iter().find(|o| o.label.contains(&tag)) else {
                        continue;
                    };
                    let mut criteria = word_opts.clone();
                    criteria.insert("none".into(), "no word gives it".into());
                    q1.insert(
                        format!("slot.{s}"),
                        Question::Choice {
                            criteria,
                            instructions: format!(
                                "Which word of the request gives `{tag}` in `{}`: {}? Pick a word even when its value looks wrong. Pick `none` when no word gives it.",
                                o.label, o.meaning
                            ),
                        },
                    );
                }
            }
        }
    }
    let nevers: Vec<&str> = w
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::Never { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    for (i, text) in nevers.iter().enumerate() {
        q1.insert(
            format!("never.{i}"),
            Question::Noul {
                instructions: format!(
                    "Does the request ask for this, which the service forbids: {text}?"
                ),
            },
        );
    }
    let settings = w
        .args
        .iter()
        .map(|a| match &a.kind {
            ArgKind::Value { what } => format!("{}: {what}", a.name),
            ArgKind::Options(opts) => format!(
                "{}: {}",
                a.name,
                opts.iter()
                    .map(|o| o.meaning.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
        .collect::<Vec<_>>()
        .join("; ");
    q1.insert(
        "uncovered".into(),
        Question::Noul {
            instructions: UNCOVERED
                .replace("{settings}", &settings)
                .replace("{request}", request),
        },
    );
    let about = w.about.as_deref().unwrap_or(&w.name);
    // No words say nothing: every setting is unsaid, without a decider call.
    let a1 = if words.is_empty() {
        Answers {
            answers: IndexMap::new(),
            usage: Default::default(),
            cost_usd: 0.0,
        }
    } else {
        decider
            .ask(
                json!({"service": about, "request": request, "words": words}),
                q1,
            )
            .await?
    };
    cost += a1.cost_usd;

    // 2. Code: each word fills one setting; the most confident slot keeps it.
    let slot_answers: Vec<(&str, &IndexMap<String, f64>)> = a1
        .answers
        .keys()
        .filter_map(|k| {
            let name = k.strip_prefix("slot.")?;
            let (_, p, _) = a1.choice(k)?;
            Some((name, p))
        })
        .collect();
    let mut owner: HashMap<&str, usize> = HashMap::new();
    for (idx, (_, p)) in slot_answers.iter().enumerate() {
        let Some((c, v)) = argmax(p.iter().map(|(k, v)| (k, *v))) else {
            continue;
        };
        if !c.starts_with('w') {
            continue;
        }
        let wins = match owner.get(c.as_str()) {
            None => true,
            Some(&o) => v > slot_answers[o].1.get(c).copied().unwrap_or(0.0),
        };
        if wins {
            owner.insert(c.as_str(), idx);
        }
    }
    let mut picked: HashMap<&str, (Option<String>, f64)> = HashMap::new();
    for (idx, (name, p)) in slot_answers.iter().enumerate() {
        let kept: Vec<(&String, f64)> = p
            .iter()
            .filter(|(c, _)| {
                !(c.starts_with('w') && owner.get(c.as_str()).is_some_and(|&o| o != idx))
            })
            .map(|(c, v)| (c, *v))
            .collect();
        let total: f64 = kept.iter().map(|(_, v)| v).sum();
        let total = if total == 0.0 { 1.0 } else { total };
        let Some((c, v)) = argmax(kept.into_iter()) else {
            continue;
        };
        let word = c
            .strip_prefix('w')
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| words.get(i))
            .map(|x| x.to_string());
        picked.insert(name, (word, round2(v / total)));
    }
    let pick = |name: &str| picked.get(name).cloned().unwrap_or((None, 0.0));

    let mut vals: IndexMap<String, Option<String>> = IndexMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut fits: IndexMap<String, (String, String)> = IndexMap::new();
    let finding = |level, subject: &str, text: String, p: Option<f64>| Finding {
        level,
        subject: subject.to_string(),
        text,
        p,
    };
    for a in &w.args {
        match &a.kind {
            ArgKind::Value { what } => {
                let (v, _) = pick(&a.name);
                if let Some(v) = &v {
                    fits.insert(a.name.clone(), (v.clone(), what.clone()));
                }
                vals.insert(a.name.clone(), v);
            }
            ArgKind::Options(opts) => {
                let key = format!("arg.{}", a.name);
                let (c, confidence) = match a1.choice(&key) {
                    Some((c, _, p)) => (c, p),
                    None if words.is_empty() => ("unsaid", 1.0),
                    None => bail!("decider: no choice for {key}"),
                };
                let p = round2(confidence);
                if c == "other" || c == "unsaid" {
                    vals.insert(a.name.clone(), None);
                    if c == "other" {
                        let labels: Vec<&str> = opts.iter().map(|o| o.label.as_str()).collect();
                        findings.push(finding(
                            Level::Error,
                            &a.name,
                            format!(
                                "the request says something no option fits: {}",
                                labels.join(", ")
                            ),
                            Some(p),
                        ));
                    }
                    continue;
                }
                let Some(o) = opt_keys.get(a.name.as_str()).and_then(|k| k.get(c)) else {
                    bail!("decider: `{c}` is no option of {}", a.name);
                };
                let mut v = o.label.clone();
                for s in arg_slots(opts) {
                    let (sv, sp) = pick(s);
                    vals.insert(s.to_string(), sv.clone());
                    let tag = format!("<{s}>");
                    if o.label.contains(&tag) {
                        match sv {
                            None => findings.push(finding(
                                Level::Error,
                                s,
                                format!("`{}` needs {tag}: the request gives no value", o.label),
                                Some(sp),
                            )),
                            Some(sv) => {
                                v = v.replace(&tag, &sv);
                                fits.insert(s.to_string(), (sv, o.meaning.clone()));
                            }
                        }
                    }
                }
                if p < 0.5 {
                    findings.push(finding(
                        Level::Ask,
                        &a.name,
                        format!("which option? the decider leans to `{v}`"),
                        Some(p),
                    ));
                } else {
                    findings.push(finding(
                        Level::Info,
                        &a.name,
                        format!("{v}: {}", o.meaning),
                        Some(p),
                    ));
                }
                vals.insert(a.name.clone(), Some(v));
            }
        }
    }

    // 3. Whether the values fit.
    let asks: Vec<&str> = w
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::Ask { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let mut q2: IndexMap<String, Question> = fits
        .iter()
        .map(|(n, (v, what))| {
            (
                format!("fit.{n}"),
                Question::Noul {
                    instructions: format!("Is the value `{v}` a valid example of this: {what}?"),
                },
            )
        })
        .collect();
    if current.is_none() {
        for (i, text) in asks.iter().enumerate() {
            q2.insert(
                format!("ask.{i}"),
                Question::Noul {
                    instructions: format!(
                        "Does the request ask for this, which needs the person's yes first: {text}? Request: \"{request}\""
                    ),
                },
            );
        }
    }
    let a2 = if q2.is_empty() {
        None
    } else {
        let a = decider.ask(json!({"task": FIT_TASK}), q2).await?;
        cost += a.cost_usd;
        Some(a)
    };
    let noul2 = |key: &str| a2.as_ref().and_then(|a| a.noul(key)).unwrap_or(0.0);
    for (n, (v, what)) in &fits {
        let f = round2(noul2(&format!("fit.{n}")));
        if f < 0.5 {
            vals.insert(n.clone(), None);
            findings.push(finding(
                Level::Error,
                n,
                format!("`{v}` is not {what}"),
                Some(f),
            ));
        } else if w.arg(n).is_some() {
            findings.push(finding(Level::Info, n, v.clone(), Some(f)));
        }
    }

    // 4. Code: unsaid ARGs take their level; NEVER IF and ASK IF read the values.
    for a in &w.args {
        let said = vals.get(&a.name).is_some_and(|v| v.is_some());
        if said || findings.iter().any(|f| f.subject == a.name) {
            continue;
        }
        if let Some(now) = current.and_then(|c| c.get(&a.name)) {
            vals.insert(a.name.clone(), Some(now.clone()));
            if let ArgKind::Options(opts) = &a.kind {
                for s in arg_slots(opts) {
                    let v = current.and_then(|c| c.get(s)).cloned();
                    vals.insert(s.to_string(), v);
                }
            }
            continue;
        }
        if let Some(d) = &a.default {
            vals.insert(a.name.clone(), Some(d.clone()));
        }
        let meaning = match (&a.kind, &a.default) {
            (ArgKind::Options(opts), Some(d)) => opts
                .iter()
                .find(|o| &o.label == d)
                .map(|o| o.meaning.as_str())
                .unwrap_or(""),
            _ => "",
        };
        let (level, text) = match a.level {
            crate::weave::Level::Must => (
                Level::Error,
                match &a.kind {
                    ArgKind::Value { what } => format!("unsaid: say {what}"),
                    ArgKind::Options(opts) => format!(
                        "unsaid: say one of {}",
                        opts.iter()
                            .map(|o| o.label.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                },
            ),
            level => (
                if level == crate::weave::Level::Should {
                    Level::Warning
                } else {
                    Level::Info
                },
                match &a.default {
                    None => "unsaid".to_string(),
                    Some(d) if meaning.is_empty() => format!("unsaid, default {d}"),
                    Some(d) => format!("unsaid, default {d}: {meaning}"),
                },
            ),
        };
        findings.push(finding(level, &a.name, text, None));
    }

    let env = Values(&vals);
    for r in &w.rules {
        let (level, subject, e) = match r {
            Rule::NeverIf { expr, .. } => (Level::Error, "NEVER IF", expr),
            Rule::AskIf { expr, .. } => (Level::Ask, "ASK IF", expr),
            _ => continue,
        };
        if expr::holds(e, &env) {
            let shown = e.show_with(&|n| env.get(n).map(String::from));
            findings.push(finding(level, subject, shown, None));
        }
    }
    for (i, text) in nevers.iter().enumerate() {
        let p = round2(a1.noul(&format!("never.{i}")).unwrap_or(0.0));
        if p >= 0.5 {
            findings.push(finding(
                Level::Error,
                "NEVER",
                format!("\"{text}\""),
                Some(p),
            ));
        }
    }
    let after: BTreeMap<&str, &str> = vals
        .iter()
        .filter_map(|(k, v)| Some((k.as_str(), v.as_deref()?)))
        .collect();
    let changed = |now: &BTreeMap<String, String>| {
        after.len() != now.len()
            || now
                .iter()
                .any(|(k, v)| after.get(k.as_str()) != Some(&v.as_str()))
    };
    let ask_p: Vec<f64> = match current {
        None => (0..asks.len())
            .map(|i| noul2(&format!("ask.{i}")))
            .collect(),
        Some(now) if !asks.is_empty() && changed(now) => {
            let q3: IndexMap<String, Question> = asks
                .iter()
                .enumerate()
                .map(|(i, text)| {
                    (
                        format!("ask.{i}"),
                        Question::Noul {
                            instructions: format!(
                                "Rule: ask the person {text}. Does applying `args_after` in place of `args_now` trigger this rule?"
                            ),
                        },
                    )
                })
                .collect();
            let a3 = decider
                .ask(
                    json!({"service": about, "request": request, "args_now": now, "args_after": after}),
                    q3,
                )
                .await?;
            cost += a3.cost_usd;
            (0..asks.len())
                .map(|i| a3.noul(&format!("ask.{i}")).unwrap_or(0.0))
                .collect()
        }
        Some(_) => Vec::new(),
    };
    for (text, p) in asks.iter().zip(ask_p) {
        let p = round2(p);
        if p >= 0.5 {
            findings.push(finding(Level::Ask, "ASK", format!("\"{text}\""), Some(p)));
        }
    }
    let u = round2(a1.noul("uncovered").unwrap_or(0.0));
    if u >= 0.5 {
        findings.push(finding(
            Level::Warning,
            "request",
            "asks for something no ARG covers".into(),
            Some(u),
        ));
    }

    // Goals switched off by IF, and every goal after them.
    let off = graph::off(w, &env);
    let off_in_order: Vec<&str> = w
        .goals
        .iter()
        .filter(|g| off.contains(&g.name))
        .map(|g| g.name.as_str())
        .collect();
    for f in &mut findings {
        if f.text != "unsaid" {
            continue;
        }
        let hit = w.goals.iter().any(|g| {
            off.contains(&g.name)
                && g.when
                    .as_ref()
                    .is_some_and(|e| e.names().contains(&f.subject.as_str()))
        });
        if hit {
            f.text = format!("unsaid, so off: {}", off_in_order.join(", "));
        }
    }
    findings.sort_by_key(|f| (f.level, w.args.iter().position(|a| a.name == f.subject)));

    let secrets = w
        .secrets
        .iter()
        .filter(|s| s.cond.as_ref().is_none_or(|e| expr::holds(e, &env)))
        .map(|s| s.name.clone())
        .collect();
    let values = vals
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), v.clone()?)))
        .collect();
    Ok(Plan {
        request: request.to_string(),
        values,
        findings,
        off: off_in_order.into_iter().map(String::from).collect(),
        secrets,
        cost_usd: cost,
    })
}

/// The findings as `subweave plan` prints them, then the secrets and one summary line.
pub fn render(plan: &Plan) -> String {
    let mut out = String::new();
    let width = plan
        .findings
        .iter()
        .map(|f| f.subject.chars().count())
        .max()
        .unwrap_or(0);
    for f in &plan.findings {
        let _ = write!(
            out,
            "{:8} {:width$}  {}",
            f.level.as_str(),
            f.subject,
            f.text
        );
        if let Some(p) = f.p {
            let _ = write!(out, "   {p:.2}");
        }
        out.push('\n');
    }
    if !plan.findings.is_empty() {
        out.push('\n');
    }
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let errors = plan.count(Level::Error);
    if errors > 0 {
        let _ = writeln!(out, "{}: nothing runs.", plural(errors, "error"));
        return out;
    }
    if !plan.secrets.is_empty() {
        let _ = writeln!(out, "secrets asked once: {}", plan.secrets.join(", "));
    }
    let asks = plan.count(Level::Ask);
    let warnings = plan.count(Level::Warning);
    if asks > 0 {
        let _ = writeln!(out, "{}: run waits for the person.", plural(asks, "ask"));
    } else if warnings > 0 {
        let _ = writeln!(out, "{}: run asks to go on.", plural(warnings, "warning"));
    } else {
        out.push_str("ready to run.\n");
    }
    out
}
