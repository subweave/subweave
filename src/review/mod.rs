//! `subweave review`: checks a weave against the rules in `rules.toml`, some in code
//! (`code.rs`), the rest as questions to the decider. The elements, their states and
//! the question shapes follow the measured prototype; the wordings must not change.

mod code;

use std::cmp::Reverse;
use std::fmt::Write;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::Result;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::model::decider::{Answers, Decider, Question};
use crate::weave::expr::Expr;
use crate::weave::{
    ArgKind, Condition, FactSource, FactType, Rule as WeaveRule, Step, Weave, show_duration,
};

/// Decider requests in flight at once.
const IN_FLIGHT: usize = 12;
/// An action without WITHIN gets this long.
const WITHIN: Duration = Duration::from_secs(300);

#[derive(Debug, Deserialize)]
pub struct Rules {
    pub probe: Vec<Probe>,
    pub rule: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
pub struct Probe {
    pub id: String,
    pub ask: String,
}

#[derive(Debug, Deserialize)]
pub struct Rule {
    pub id: String,
    pub area: String,
    pub weight: u8,
    pub on: String,
    pub code: Option<String>,
    pub ask: Option<String>,
    pub choices: Option<IndexMap<String, String>>,
    pub pass: Option<String>,
    pub says: String,
    pub why: Option<String>,
    pub unless: Option<String>,
    pub when: Option<String>,
    #[serde(default)]
    pub fields: Vec<String>,
}

impl Rule {
    pub fn level(&self) -> &'static str {
        match self.weight {
            3 => "error",
            2 => "warning",
            _ => "note",
        }
    }

    fn question(&self, ask: &str) -> Question {
        match &self.choices {
            Some(c) => Question::Choice {
                criteria: c.clone(),
                instructions: ask.to_string(),
            },
            None => Question::Noul {
                instructions: ask.to_string(),
            },
        }
    }
}

/// The compiled-in rules; a broken `rules.toml` is a build mistake, caught on first use.
pub fn rules() -> &'static Rules {
    static RULES: LazyLock<Rules> = LazyLock::new(|| {
        let rules: Rules =
            toml::from_str(include_str!("rules.toml")).expect("src/review/rules.toml parses");
        for r in &rules.rule {
            for id in r.code.iter().chain(&r.unless) {
                assert!(
                    code::CHECKS.contains(&id.as_str()),
                    "rule {}: no code check {id}",
                    r.id
                );
            }
            assert!(
                r.code.is_some() || r.ask.is_some(),
                "rule {}: neither code nor ask",
                r.id
            );
            if let Some(p) = &r.when {
                assert!(
                    rules.probe.iter().any(|x| &x.id == p),
                    "rule {}: no probe {p}",
                    r.id
                );
            }
        }
        rules
    });
    &RULES
}

/// One thing a rule checks: a weave, an ARG, a command, an action pair…
#[derive(Debug, Clone)]
pub struct Element {
    pub key: String,
    pub line: u32,
    pub state: Map<String, Value>,
}

impl Element {
    fn str(&self, field: &str) -> &str {
        self.state.get(field).and_then(Value::as_str).unwrap_or("")
    }
}

#[derive(Debug, Serialize)]
pub struct Check {
    #[serde(serialize_with = "rule_id")]
    pub rule: &'static Rule,
    pub key: String,
    pub line: u32,
    pub pass: bool,
    /// The decider's probability the check is read from; none for a code check.
    pub p: Option<f64>,
    /// The decider request failed: the check counts as failing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_answer: bool,
}

fn rule_id<S: Serializer>(r: &&'static Rule, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&r.id)
}

#[derive(Debug, Serialize)]
pub struct Failed {
    pub rule: &'static str,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct Area {
    pub area: &'static str,
    pub score: u32,
    pub failed: Vec<Failed>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub name: String,
    pub score: u32,
    /// Rules with at least one check.
    pub rules: usize,
    pub cost_usd: f64,
    pub probes: IndexMap<&'static str, Option<f64>>,
    pub areas: Vec<Area>,
    pub checks: Vec<Check>,
    pub requests: usize,
    pub failed_requests: usize,
    /// The first failed request's error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One decider request: every asked rule of one element that shares a field set.
struct Job {
    state: Value,
    questions: IndexMap<String, Question>,
    rules: Vec<&'static Rule>,
    /// Index into the checks, one per rule.
    checks: Vec<usize>,
    within: Option<Duration>,
}

pub async fn review(w: &Weave, decider: Decider) -> Result<Report> {
    let rules = rules();
    let decider = Arc::new(decider);
    let els = elements(w);
    let about = w.about.clone().unwrap_or_default();

    let mut cost_usd = 0.0;
    let mut requests = 1;
    let mut failed_requests = 0;
    let mut error = None;

    let questions = rules
        .probe
        .iter()
        .map(|p| {
            (
                p.id.clone(),
                Question::Noul {
                    instructions: p.ask.clone(),
                },
            )
        })
        .collect();
    let mut probes: IndexMap<&'static str, Option<f64>> =
        rules.probe.iter().map(|p| (p.id.as_str(), None)).collect();
    match decider.ask(json!({ "about": about }), questions).await {
        Ok(a) => {
            cost_usd += a.cost_usd;
            for (id, p) in probes.iter_mut() {
                *p = a.noul(id);
            }
        }
        Err(e) => {
            failed_requests += 1;
            error = Some(format!("{e:#}"));
        }
    }

    let mut by_on: IndexMap<&str, Vec<&'static Rule>> = IndexMap::new();
    for r in &rules.rule {
        let live = r
            .when
            .as_ref()
            .is_none_or(|p| probes.get(p.as_str()).copied().flatten().unwrap_or(0.0) >= 0.5);
        if live {
            by_on.entry(r.on.as_str()).or_default().push(r);
        }
    }

    let mut checks = Vec::new();
    let mut jobs = Vec::new();
    for (on, rs) in &by_on {
        for el in els.get(*on).into_iter().flatten() {
            let mut groups: IndexMap<&[String], Vec<&'static Rule>> = IndexMap::new();
            for &r in rs {
                let result = if r.unless.as_deref().is_some_and(|u| code::check(u, w, el)) {
                    Some(true)
                } else if r.ask.is_none() {
                    r.code.as_deref().map(|c| code::check(c, w, el))
                } else {
                    groups.entry(r.fields.as_slice()).or_default().push(r);
                    None
                };
                if let Some(pass) = result {
                    checks.push(Check {
                        rule: r,
                        key: el.key.clone(),
                        line: el.line,
                        pass,
                        p: None,
                        no_answer: false,
                    });
                }
            }
            for (fields, grp) in groups {
                let mut state: Map<String, Value> = el
                    .state
                    .iter()
                    .filter(|(k, _)| fields.is_empty() || fields.contains(k))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                state.insert("service".into(), Value::String(about.clone()));
                let mut job = Job {
                    state: Value::Object(state),
                    questions: IndexMap::new(),
                    rules: Vec::new(),
                    checks: Vec::new(),
                    within: crate::weave::expr::duration(el.str("within")),
                };
                for r in grp {
                    let ask = r.ask.as_deref().unwrap_or_default();
                    job.questions.insert(r.id.clone(), r.question(ask));
                    job.rules.push(r);
                    job.checks.push(checks.len());
                    checks.push(Check {
                        rule: r,
                        key: el.key.clone(),
                        line: el.line,
                        pass: false,
                        p: None,
                        no_answer: true,
                    });
                }
                jobs.push(job);
            }
        }
    }

    let gate = Arc::new(Semaphore::new(IN_FLIGHT));
    let mut set = JoinSet::new();
    for (i, job) in jobs.iter_mut().enumerate() {
        let state = std::mem::take(&mut job.state);
        let questions = std::mem::take(&mut job.questions);
        let (decider, gate) = (decider.clone(), gate.clone());
        set.spawn(async move {
            let _permit = gate.acquire_owned().await;
            (i, decider.ask(state, questions).await)
        });
    }
    requests += jobs.len();
    while let Some(done) = set.join_next().await {
        let (i, answer) = done?;
        let job = &jobs[i];
        match answer {
            Ok(a) => {
                cost_usd += a.cost_usd;
                for (r, &c) in job.rules.iter().zip(&job.checks) {
                    if let Some((pass, p)) = judge(r, &a, job.within) {
                        checks[c].pass = pass;
                        checks[c].p = Some(p);
                        checks[c].no_answer = false;
                    }
                }
            }
            Err(e) => {
                failed_requests += 1;
                error.get_or_insert_with(|| format!("{e:#}"));
            }
        }
    }

    let mut areas: Vec<Area> = Vec::new();
    for r in &rules.rule {
        if areas.iter().any(|a| a.area == r.area) || !checks.iter().any(|c| c.rule.area == r.area) {
            continue;
        }
        let of: Vec<&Check> = checks.iter().filter(|c| c.rule.area == r.area).collect();
        let mut failed: Vec<Failed> = Vec::new();
        for c in of.iter().filter(|c| !c.pass) {
            match failed.iter_mut().find(|f| f.rule == c.rule.id) {
                Some(f) => f.count += 1,
                None => failed.push(Failed {
                    rule: &c.rule.id,
                    count: 1,
                }),
            }
        }
        areas.push(Area {
            area: &r.area,
            score: score(of.iter().copied()),
            failed,
        });
    }

    let mut ids: Vec<&str> = checks.iter().map(|c| c.rule.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    Ok(Report {
        name: w.name.clone(),
        score: score(checks.iter()),
        rules: ids.len(),
        cost_usd,
        probes,
        areas,
        checks,
        requests,
        failed_requests,
        error,
    })
}

/// A decider rule's outcome and the probability it reads, or `None` when the answer
/// has the wrong shape.
fn judge(r: &Rule, a: &Answers, within: Option<Duration>) -> Option<(bool, f64)> {
    if r.code.as_deref() == Some("within") {
        let (_, probs, _) = a.choice(&r.id)?;
        let within = within?.as_secs();
        let over: f64 = probs
            .iter()
            .filter(|(k, _)| lower(k).is_some_and(|l| l >= within))
            .map(|(_, p)| p)
            .sum();
        return Some((over < 0.5, over));
    }
    let pass = r.pass.as_deref().unwrap_or_default();
    if r.choices.is_some() {
        let (_, probs, _) = a.choice(&r.id)?;
        let p = probs.get(pass).copied().unwrap_or(0.0);
        Some((p >= 0.5, p))
    } else {
        let y = a.noul(&r.id)?;
        Some((if pass == "yes" { y >= 0.5 } else { y < 0.5 }, y))
    }
}

/// The lower bound of each `within` choice, in seconds.
fn lower(choice: &str) -> Option<u64> {
    Some(match choice {
        "seconds" => 0,
        "minutes" => 60,
        "hour" => 900,
        "hours" => 3600,
        "days" => 43200,
        _ => return None,
    })
}

/// Each rule counts by the share of its elements that pass, so a weave's size does not
/// weigh it; weighted, floored.
fn score<'a>(checks: impl Iterator<Item = &'a Check>) -> u32 {
    let mut per: IndexMap<&str, (u8, u32, u32)> = IndexMap::new();
    for c in checks {
        let e = per.entry(&c.rule.id).or_insert((c.rule.weight, 0, 0));
        e.1 += u32::from(c.pass);
        e.2 += 1;
    }
    let total: u32 = per.values().map(|(w, _, _)| u32::from(*w)).sum();
    if total == 0 {
        return 100;
    }
    let sum: f64 = per
        .values()
        .map(|&(w, pass, n)| f64::from(u32::from(w) * pass) / f64::from(n))
        .sum();
    (100.0 * sum / f64::from(total)) as u32
}

pub fn render(r: &Report) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}  {}/100   {} rules, {} checks, ${:.4}",
        r.name,
        r.score,
        r.rules,
        r.checks.len(),
        r.cost_usd
    );
    out.push('\n');
    let _ = writeln!(out, "{:<10} {:>5}  failed", "area", "score");
    for a in &r.areas {
        let failed: Vec<String> = a
            .failed
            .iter()
            .map(|f| format!("{} ({})", f.rule, f.count))
            .collect();
        let row = format!("{:<10} {:>5}  {}", a.area, a.score, failed.join(", "));
        let _ = writeln!(out, "{}", row.trim_end());
    }
    let mut failing: Vec<&Check> = r.checks.iter().filter(|c| !c.pass).collect();
    if failing.is_empty() {
        return out;
    }
    failing.sort_by_key(|c| (Reverse(c.rule.weight), c.line));
    let iw = failing.iter().map(|c| c.rule.id.len()).max().unwrap_or(0);
    let kw = failing
        .iter()
        .map(|c| head(&c.key, 40).chars().count())
        .max()
        .unwrap_or(0);
    let sw = failing
        .iter()
        .map(|c| c.rule.says.chars().count())
        .max()
        .unwrap_or(0);
    out.push('\n');
    for c in failing {
        let p = match c.p {
            Some(p) => format!("{p:.2}"),
            None if c.no_answer => "no answer".into(),
            None => String::new(),
        };
        let lead = format!(
            "Weavefile:{:<4} {:<8} {:<iw$}   {:<kw$}   ",
            c.line,
            c.rule.level(),
            c.rule.id,
            head(&c.key, 40)
        );
        let row = format!("{lead}{:<sw$}   {p}", c.rule.says);
        let _ = writeln!(out, "{}", row.trim_end());
        if let Some(why) = &c.rule.why {
            let _ = writeln!(out, "{:indent$}{why}", "", indent = lead.chars().count());
        }
    }
    out
}

/// The first `n` characters.
fn head(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every bash text with where it is and its line: FACT and PART VERSION commands, RUN
/// scripts, and each ```` ```recipe ```` block of a skill.
pub fn commands(w: &Weave) -> Vec<(String, &str, u32)> {
    let mut out = Vec::new();
    for f in &w.facts {
        if let FactSource::Command(c) = &f.source
            && !c.is_empty()
        {
            out.push((format!("fact {}", f.name), c.as_str(), f.line));
        }
    }
    for p in &w.parts {
        out.push((format!("part {}", p.name), p.version.as_str(), p.line));
    }
    for a in &w.actions {
        for s in &a.steps {
            if let Step::Run { script, .. } = s {
                out.push((format!("action {}", a.name), script.as_str(), a.line));
            }
        }
    }
    for s in &w.skills {
        let mut rest = s.body.as_str();
        while let Some(at) = rest.find("```recipe\n") {
            let inner = &rest[at + "```recipe\n".len()..];
            let Some(end) = inner.find("```") else { break };
            out.push((format!("skill {}", s.name), &inner[..end], s.line));
            rest = &inner[end + 3..];
        }
    }
    out
}

/// Every code expression: code goal conditions, rule expressions, and the IFs of goals,
/// actions and secrets.
pub fn exprs(w: &Weave) -> Vec<&Expr> {
    let mut out = Vec::new();
    for g in &w.goals {
        if let Condition::Code(e) = &g.cond {
            out.push(e);
        }
    }
    for r in &w.rules {
        match r {
            WeaveRule::NeverIf { expr, .. } | WeaveRule::AskIf { expr, .. } => out.push(expr),
            WeaveRule::AskBefore { cond: Some(e), .. }
            | WeaveRule::NeverBefore { cond: Some(e), .. } => out.push(e),
            _ => {}
        }
    }
    out.extend(w.goals.iter().filter_map(|g| g.when.as_ref()));
    out.extend(w.actions.iter().filter_map(|a| a.when_if.as_ref()));
    out.extend(w.secrets.iter().filter_map(|s| s.cond.as_ref()));
    out
}

/// What the coder can be asked to do: each DO task of an action with FOR, and, when a
/// catch-all action has a DO step, making each goal hold again.
pub fn jobs(w: &Weave) -> Vec<String> {
    let mut out: Vec<String> = w
        .actions
        .iter()
        .filter(|a| !a.for_goals.is_empty())
        .flat_map(|a| a.steps.iter())
        .filter_map(|s| match s {
            Step::Do { task, .. } => Some(norm(task)),
            _ => None,
        })
        .collect();
    let catch_all_does = w
        .actions
        .iter()
        .any(|a| a.for_goals.is_empty() && a.steps.iter().any(|s| matches!(s, Step::Do { .. })));
    if catch_all_does {
        for g in &w.goals {
            out.push(format!(
                "make the goal `{}` hold again: {}",
                g.name,
                condition(&g.cond)
            ));
        }
    }
    out
}

fn condition(c: &Condition) -> String {
    match c {
        Condition::Code(e) => e.to_string(),
        Condition::Quoted(t) => norm(t),
    }
}

fn rule_kind(r: &WeaveRule) -> &'static str {
    match r {
        WeaveRule::Never { .. } | WeaveRule::NeverIf { .. } | WeaveRule::NeverBefore { .. } => {
            "NEVER"
        }
        _ => "ASK",
    }
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!("json!({{…}}) is an object"),
    }
}

/// Every kind of element a rule can check (`on`), each with its key, line and state.
pub fn elements(w: &Weave) -> IndexMap<&'static str, Vec<Element>> {
    let mut els: IndexMap<&'static str, Vec<Element>> = IndexMap::new();
    let mut add = |on: &'static str, key: String, line: u32, state: Value| {
        els.entry(on).or_default().push(Element {
            key,
            line,
            state: obj(state),
        });
    };

    let goals: Vec<Value> = w
        .goals
        .iter()
        .map(|g| {
            let fixes: Vec<String> = w
                .actions
                .iter()
                .filter(|a| a.for_goals.contains(&g.name))
                .map(|a| format!("{}: {}", a.name, norm(&a.when)))
                .collect();
            let mut m = Map::new();
            m.insert("name".into(), json!(g.name));
            m.insert("condition".into(), json!(condition(&g.cond)));
            if let Some(e) = &g.when {
                m.insert("if".into(), json!(e.to_string()));
            }
            m.insert("fixed_by".into(), json!(fixes));
            Value::Object(m)
        })
        .collect();
    let rules: Vec<String> = w
        .rules
        .iter()
        .map(|r| match r {
            WeaveRule::Never { text, .. } | WeaveRule::Ask { text, .. } => {
                format!("{} \"{}\"", rule_kind(r), norm(text))
            }
            WeaveRule::AskBefore { .. } | WeaveRule::NeverBefore { .. } => r.text(),
            WeaveRule::NeverIf { expr, .. } | WeaveRule::AskIf { expr, .. } => {
                format!("{} IF {expr}", rule_kind(r))
            }
        })
        .collect();
    let actions: Vec<String> = w
        .actions
        .iter()
        .map(|a| format!("{}: {}", a.name, norm(&a.when)))
        .collect();
    add(
        "weave",
        w.name.clone(),
        1,
        json!({
            "about": w.about.clone().unwrap_or_default(),
            "goals": goals,
            "actions": actions,
            "rules": rules,
        }),
    );

    for a in &w.args {
        add("arg", a.name.clone(), a.line, json!({ "name": a.name }));
        match &a.kind {
            ArgKind::Value { what } => {
                add(
                    "arg-value",
                    a.name.clone(),
                    a.line,
                    json!({
                        "name": a.name,
                        "level": a.level.as_str(),
                        "what": norm(what),
                        "default": a.default,
                    }),
                );
                if let Some(d) = &a.default {
                    add(
                        "arg-defaulted",
                        a.name.clone(),
                        a.line,
                        json!({ "name": a.name, "default": d, "meaning": norm(what) }),
                    );
                }
            }
            ArgKind::Options(opts) => {
                let mut slots: Vec<&str> = opts
                    .iter()
                    .flat_map(|o| o.slots.iter().map(String::as_str))
                    .collect();
                slots.sort_unstable();
                slots.dedup();
                for s in &slots {
                    add("arg", s.to_string(), a.line, json!({ "name": s }));
                }
                let options: Map<String, Value> = opts
                    .iter()
                    .map(|o| (o.label.clone(), json!(o.meaning)))
                    .collect();
                add(
                    "arg-options",
                    a.name.clone(),
                    a.line,
                    json!({ "name": a.name, "options": options, "slots": slots }),
                );
                if let Some(d) = &a.default {
                    let meaning = opts
                        .iter()
                        .find(|o| &o.label == d)
                        .map_or("", |o| o.meaning.as_str());
                    add(
                        "arg-defaulted",
                        a.name.clone(),
                        a.line,
                        json!({ "name": a.name, "default": d, "meaning": meaning }),
                    );
                }
            }
        }
    }

    let cmds = commands(w);
    let mut where_texts: Vec<(String, &str, u32)> = cmds.clone();
    for s in &w.skills {
        where_texts.push((format!("skill {}", s.name), s.body.as_str(), s.line));
    }
    for a in &w.actions {
        for s in &a.steps {
            if let Step::Do { task, .. } = s {
                where_texts.push((format!("action {}", a.name), task.as_str(), a.line));
            }
        }
    }
    for s in &w.secrets {
        add("secret", s.name.clone(), s.line, json!({ "name": s.name }));
        let mut seen: Vec<&str> = Vec::new();
        for (place, text, line) in &where_texts {
            if code::dollar_ref(text, &s.name) && !seen.contains(&place.as_str()) {
                seen.push(place);
                add(
                    "secret-text",
                    format!("{} in {place}", s.name),
                    *line,
                    json!({ "secret": s.name, "text": text }),
                );
            }
        }
    }

    for f in &w.facts {
        if matches!(f.source, FactSource::Part { .. }) {
            continue;
        }
        add("fact", f.name.clone(), f.line, json!({ "name": f.name }));
        match &f.source {
            FactSource::Command(c) | FactSource::Quoted(c) if f.ty == FactType::Lines => add(
                "fact-follow",
                f.name.clone(),
                f.line,
                json!({ "name": f.name, "source": norm(c) }),
            ),
            FactSource::Quoted(what) => {
                let st = json!({ "name": f.name, "what": norm(what) });
                add("fact-quoted", f.name.clone(), f.line, st.clone());
                match f.ty {
                    FactType::Number(_) => add("fact-number", f.name.clone(), f.line, st),
                    FactType::Bool => add("fact-exit", f.name.clone(), f.line, st),
                    _ => {}
                }
            }
            FactSource::Command(c) => add(
                "fact-command",
                f.name.clone(),
                f.line,
                json!({ "name": f.name, "command": c }),
            ),
            FactSource::Part { .. } => {}
        }
    }
    for (place, c, line) in &cmds {
        add(
            "command",
            place.clone(),
            *line,
            json!({ "where": place, "command": c }),
        );
    }

    for g in &w.goals {
        add("goal", g.name.clone(), g.line, json!({ "name": g.name }));
        if let Condition::Quoted(t) = &g.cond {
            add(
                "goal-quoted",
                g.name.clone(),
                g.line,
                json!({ "name": g.name, "condition": norm(t) }),
            );
        }
    }

    for a in &w.actions {
        let steps: Vec<String> = a
            .steps
            .iter()
            .map(|s| match s {
                Step::Run { script, .. } => format!("RUN: {script}"),
                Step::Do { task, .. } => format!("DO: {}", norm(task)),
                Step::Add { content, .. } => format!("ADD {}", head(content, 60)),
            })
            .collect();
        add(
            "action",
            a.name.clone(),
            a.line,
            json!({
                "name": a.name,
                "when": norm(&a.when),
                "steps": steps,
                "within": show_duration(a.within.unwrap_or(WITHIN)),
            }),
        );
        for s in &a.steps {
            match s {
                Step::Do { task, .. } if !a.for_goals.is_empty() => add(
                    "do",
                    a.name.clone(),
                    a.line,
                    json!({ "action": a.name, "task": norm(task) }),
                ),
                Step::Run { script, .. } => add(
                    "run",
                    a.name.clone(),
                    a.line,
                    json!({ "action": a.name, "script": script }),
                ),
                _ => {}
            }
        }
        if a.for_goals.is_empty() {
            let task: Vec<String> = a
                .steps
                .iter()
                .filter_map(|s| match s {
                    Step::Do { task, .. } => Some(norm(task)),
                    _ => None,
                })
                .collect();
            add(
                "action-catch-all",
                a.name.clone(),
                a.line,
                json!({ "name": a.name, "when": norm(&a.when), "task": task.join(" ") }),
            );
        }
    }
    for g in &w.goals {
        let acts: Vec<_> = w
            .actions
            .iter()
            .filter(|a| a.for_goals.contains(&g.name))
            .collect();
        for (i, x) in acts.iter().enumerate() {
            for y in &acts[i + 1..] {
                add(
                    "action-pair",
                    format!("{} / {} for {}", x.name, y.name, g.name),
                    y.line,
                    json!({ "goal": g.name, "a": norm(&x.when), "b": norm(&y.when) }),
                );
            }
        }
    }
    for r in &w.rules {
        if let WeaveRule::Never { text, line } | WeaveRule::Ask { text, line } = r {
            add(
                "rule-quoted",
                format!("{} \"{}\"", rule_kind(r), head(text, 40)),
                *line,
                json!({ "kind": rule_kind(r), "text": norm(text) }),
            );
        }
    }
    let boxname = w.from.as_ref().map(ToString::to_string).unwrap_or_default();
    let jobs = jobs(w);
    for s in &w.skills {
        add(
            "skill",
            s.name.clone(),
            s.line,
            json!({
                "name": s.name,
                "box": boxname,
                "when": s.when,
                "body": s.body,
                "jobs": jobs,
            }),
        );
    }
    els
}
