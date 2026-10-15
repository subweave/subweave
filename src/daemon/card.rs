//! What a person reads on each ask: a question, plain lines and buttons that name the
//! move, written in code from the weave's plain words. One function per kind of ask.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::model::coder::{Coder, Msg, Reply, Tool};
use crate::weave::{self, Action, Condition, Goal, Rule, Step, show_duration};

use super::observe::Snapshot;
use super::service::{Ctx, no_park};
use super::{cap, goals};

const SUMMARY_SYSTEM: &str = "You write the two lines at the top of a card on a person's \
    phone. The card is a question from Subweave, the daemon that keeps a service on a server \
    running. Line 1 says what happened; line 2 says what it means for the person, or what \
    they decide. Each line is one short sentence of at most 16 plain words, for a reader \
    who does not know the service's internals: say what a fact means, not its name. Use \
    only what the card holds: never add a number, version, name or cause it does not \
    state, and never say which button to press. Call save with the two lines.";
const SUMMARY_EFFORT: &str = "low";
/// The most words a summary line keeps: Steery refuses a card whose gist runs longer.
const SUMMARY_WORDS: usize = 20;

/// The longest goal text in a title, and the longest command.
const SUBJECT: usize = 80;
const COMMAND: usize = 60;
const FACT: usize = 300;
/// The most kinds of line a card shows.
const KINDS: usize = 8;

pub(crate) struct Card {
    /// The question the buttons answer.
    pub title: String,
    /// Plain sentences in reading order: what does not hold, the facts now, what
    /// Subweave did or wants, why it asks.
    pub lines: Vec<String>,
    /// (answer word, button label), in button order.
    pub buttons: Vec<(String, String)>,
}

impl Card {
    pub fn words(&self) -> Vec<String> {
        self.buttons.iter().map(|(w, _)| w.clone()).collect()
    }

    pub fn labels(&self) -> BTreeMap<String, String> {
        self.buttons.iter().cloned().collect()
    }
}

/// The two lines a model wrote, or why they cannot be used, and its reply.
pub(crate) struct Summary {
    pub lines: Result<Vec<String>, String>,
    pub reply: Reply,
}

/// One call to the coder's first model: two plain lines for the top of `card`, a JSON of
/// the card as the person would read it, secrets already redacted.
pub(crate) async fn summary(coder: &Coder, card: &Value) -> Result<Summary> {
    let tool = Tool {
        name: "save",
        description: "Save the card's two lines.",
        parameters: json!({
            "type": "object",
            "properties": {
                "lines": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 2}
            },
            "required": ["lines"]
        }),
    };
    let reply = coder
        .send(
            0,
            None,
            Some(SUMMARY_EFFORT),
            SUMMARY_SYSTEM,
            &[Msg::User(card.to_string())],
            &[tool],
        )
        .await?;
    let lines = summary_lines(&reply);
    Ok(Summary { lines, reply })
}

fn summary_lines(reply: &Reply) -> Result<Vec<String>, String> {
    let call = reply
        .calls
        .iter()
        .find(|c| c.name == "save")
        .ok_or("the model called no save")?;
    let lines: Vec<String> = call.input["lines"]
        .as_array()
        .ok_or("save has no lines")?
        .iter()
        .filter_map(Value::as_str)
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() != 2 {
        return Err(format!("save has {} lines, not 2", lines.len()));
    }
    if let Some(l) = lines.iter().find(|l| l.split_whitespace().count() > SUMMARY_WORDS) {
        return Err(format!("a line is over {SUMMARY_WORDS} words: {}", short(l, 60)));
    }
    Ok(lines)
}

fn buttons(b: &[(&str, &str)]) -> Vec<(String, String)> {
    b.iter().map(|(w, l)| ((*w).to_string(), (*l).to_string())).collect()
}

/// `no` parks the goal, or the box, for `no_park`.
fn later(g: Option<&Goal>) -> String {
    format!("Leave it for {}", show_duration(no_park(g)))
}

/// At most `n` characters, `…` where it was cut.
fn short(s: &str, n: usize) -> String {
    let s = s.trim().trim_end_matches('.');
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// What the goal keeps true, in the weave's words: its sentence.
pub(crate) fn subject(ctx: &Ctx, g: &Goal) -> String {
    weave::subst(&g.sentence, &ctx.spec.args)
}

/// An action's button: its label, `$name` replaced, else its name.
pub(crate) fn label(ctx: &Ctx, a: &Action) -> String {
    match &a.label {
        Some(l) => weave::subst(l, &ctx.spec.args),
        None => a.name.clone(),
    }
}

/// Why a rule holds an action, for a person: its `BECAUSE`, else the rule as written.
fn why(ctx: &Ctx, r: &Rule) -> String {
    match r.because() {
        Some(b) => weave::subst(b, &ctx.spec.args),
        None => weave::subst(&r.text(), &ctx.spec.args),
    }
}

/// A goal that does not hold: what it keeps true and how it is checked, then each fact
/// it reads as it is now. A goal that judges lines shows its window's kinds of line
/// instead of their text, each with what is known of it.
pub(crate) fn goal_lines(ctx: &Ctx, g: &Goal, now: &BTreeMap<String, String>) -> Vec<String> {
    let window = if ctx.w.windowed(g) {
        format!(" over the last {}", weave::show_duration(g.over()))
    } else {
        String::new()
    };
    let mut out = vec![
        format!("Not true now: {}.", subject(ctx, g).trim_end_matches('.')),
        match &g.cond {
            Condition::Code(e) => format!("Goal `{}` checks `{e}`{window}.", g.name),
            Condition::Quoted(t) => format!(
                "Goal `{}` asks a model{window}: {}.",
                g.name,
                weave::subst(t, &ctx.spec.args).trim_end_matches('.')
            ),
        },
    ];
    let judged: Vec<&str> = ctx.w.judged_lines(g).collect();
    for f in ctx.w.reads(g) {
        if judged.contains(&f.as_str()) {
            continue;
        }
        if let Some(v) = now.get(&f) {
            let v = v.trim();
            out.push(format!(
                "`{f}` now: {}",
                cap(if v.is_empty() { "nothing" } else { v }, FACT)
            ));
        }
    }
    if !judged.is_empty() {
        let kinds = goals::kinds(ctx, g, goals::span(g, Snapshot::now()));
        if kinds.is_empty() {
            out.push("No lines in the window.".into());
        }
        for k in kinds.iter().take(KINDS) {
            out.push(format!("{}× {}: `{}`", k.count, k.known_words(), cap(&k.sample, FACT)));
        }
        if kinds.len() > KINDS {
            out.push(format!("…and {} more kinds of line.", kinds.len() - KINDS));
        }
    }
    out
}

/// `TRIES` used up.
pub(crate) fn tries(
    ctx: &Ctx,
    g: &Goal,
    n: u32,
    per: Duration,
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = goal_lines(ctx, g, now);
    lines.push(format!(
        "Subweave tried {n} times in {}, and it still does not hold.",
        show_duration(per)
    ));
    Card {
        title: format!("Try again to make this true: {}?", short(&subject(ctx, g), SUBJECT)),
        lines,
        buttons: buttons(&[("yes", "Try again"), ("no", &later(Some(g)))]),
    }
}

/// No action fits the goal: a rule refuses each one that does, or the `IF` of each one
/// written for it is not true now, which says what that fix would need.
pub(crate) fn no_fit(
    ctx: &Ctx,
    g: &Goal,
    refused: &[(&Action, &Rule, bool)],
    off: &[&Action],
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = goal_lines(ctx, g, now);
    for (a, r, kept) in refused {
        lines.push(if *kept {
            format!("{} is off: you answered Never run it for it before.", label(ctx, a))
        } else {
            format!("{} is refused: {}.", label(ctx, a), why(ctx, r))
        });
    }
    for a in off {
        if let Some(e) = &a.when_if {
            lines.push(format!("{} would fit, but it needs `{e}`, which is not true now.", label(ctx, a)));
        }
    }
    lines.push(if refused.is_empty() && off.is_empty() {
        "No action in the weave can fix this, so it needs you.".into()
    } else {
        "No other action can fix this, so it needs you.".into()
    });
    Card {
        title: format!("Can you make this true: {}?", short(&subject(ctx, g), SUBJECT)),
        lines,
        buttons: buttons(&[("yes", "Fixed it, check again"), ("no", &later(Some(g)))]),
    }
}

/// The undo of `of` failed or could not run, for `why`.
pub(crate) fn undo_failed(
    ctx: &Ctx,
    g: &Goal,
    of: &str,
    undo: &Action,
    why: &str,
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = goal_lines(ctx, g, now);
    lines.push(format!(
        "{} did not help or broke what held, so Subweave tried to undo it: {}.",
        action_name(ctx, of),
        label(ctx, undo)
    ));
    lines.push(format!("The undo failed: {}.", why.trim_end_matches('.')));
    Card {
        title: format!("Undo failed. Can you make this true: {}?", short(&subject(ctx, g), SUBJECT)),
        lines,
        buttons: buttons(&[("yes", "Fixed it, check again"), ("no", &later(Some(g)))]),
    }
}

/// Several actions fit and the decider did not pick one: `why` says why not.
pub(crate) fn which(
    ctx: &Ctx,
    g: &Goal,
    cands: &[&Action],
    why: &str,
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = goal_lines(ctx, g, now);
    lines.push(why.to_string());
    for a in cands {
        let when = weave::subst(&a.when, &ctx.spec.args);
        let when = when.trim().trim_end_matches('.');
        lines.push(if when.is_empty() {
            format!("{}.", label(ctx, a))
        } else {
            format!("{}: right when {when}.", label(ctx, a))
        });
    }
    let mut b: Vec<(String, String)> =
        cands.iter().map(|a| (a.name.clone(), label(ctx, a))).collect();
    b.push(("no".into(), later(Some(g))));
    Card {
        title: format!("Which fix for: {}?", short(&subject(ctx, g), SUBJECT)),
        lines,
        buttons: b,
    }
}

/// An `ASK BEFORE` or `ASK IF` holds the action the loop picked.
pub(crate) fn gate(
    ctx: &Ctx,
    g: &Goal,
    a: &Action,
    rule: &Rule,
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = goal_lines(ctx, g, now);
    let dos: Vec<String> = a
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Do { task, .. } => Some(weave::subst(task, &ctx.spec.args)),
            _ => None,
        })
        .collect();
    lines.push(if dos.is_empty() {
        format!("To fix it, Subweave wants to run `{}`.", a.name)
    } else {
        format!("To fix it, Subweave wants to {}.", dos.join("; then ").trim_end_matches('.'))
    });
    if let Rule::AskBefore { on: weave::Before::Changes(_), .. } = rule {
        lines.push(format!("It changes {}.", weave::show_effects(a.effects())));
    }
    lines.push(format!("The weave asks you first: {}.", why(ctx, rule)));
    Card {
        title: match &a.label {
            Some(_) => format!("{}?", label(ctx, a).trim_end_matches(['.', '?'])),
            None => format!("Run `{}`?", a.name),
        },
        lines,
        buttons: buttons(&[
            ("yes", "Run it"),
            ("no", &later(Some(g))),
            ("always", "Always run it"),
            ("never", "Never run it"),
        ]),
    }
}

/// What a coder ask is for: the goal's lines and the action the coder runs, or the
/// person's task.
fn work_lines(
    ctx: &Ctx,
    goal: Option<&Goal>,
    action: &str,
    task: &str,
    now: &BTreeMap<String, String>,
) -> Vec<String> {
    match goal {
        Some(g) => {
            let mut out = goal_lines(ctx, g, now);
            out.push(format!("The coder runs {} to fix it.", action_name(ctx, action)));
            out
        }
        None => vec![format!("Your task: {}", short(task, 300))],
    }
}

/// An action by name, as its button says it.
fn action_name(ctx: &Ctx, action: &str) -> String {
    match ctx.w.action(action) {
        Some(a) if a.label.is_some() => format!("“{}”", label(ctx, a)),
        _ => format!("`{action}`"),
    }
}

/// `no` leaves a goal alone for a while and ends a task.
fn no_label(goal: Option<&Goal>) -> String {
    match goal {
        Some(_) => later(goal),
        None => "End the task".into(),
    }
}

/// The coder says only the person can do this; `question` is its own words.
pub(crate) fn coder_asks(
    ctx: &Ctx,
    goal: Option<&Goal>,
    action: &str,
    task: &str,
    question: &str,
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = work_lines(ctx, goal, action, task, now);
    lines.push("The coder says only you can do this.".into());
    Card {
        title: question.to_string(),
        lines,
        buttons: buttons(&[("yes", "Done, try again"), ("no", &no_label(goal))]),
    }
}

/// The coder does not get closer: `tried` says how far it got and its last commands.
pub(crate) fn coder_stuck(
    ctx: &Ctx,
    goal: Option<&Goal>,
    action: &str,
    task: &str,
    tried: Vec<String>,
    now: &BTreeMap<String, String>,
) -> Card {
    let mut lines = work_lines(ctx, goal, action, task, now);
    lines.extend(tried);
    Card {
        title: match goal {
            Some(g) => format!("Can you make this true: {}?", short(&subject(ctx, g), SUBJECT)),
            None => format!("Can you help with this task: {}?", short(task, SUBJECT)),
        },
        lines,
        buttons: buttons(&[("yes", "Done, try again"), ("no", &no_label(goal))]),
    }
}

/// A quoted `ASK` holds a command the coder wants to run.
pub(crate) fn command(
    ctx: &Ctx,
    goal: Option<&Goal>,
    action: &str,
    command: &str,
    purpose: &str,
    rule: &str,
) -> Card {
    let mut lines = match goal {
        Some(g) => vec![
            format!("To make this true: {}.", subject(ctx, g).trim_end_matches('.')),
            format!("The coder runs {} to fix it.", action_name(ctx, action)),
        ],
        None => vec!["For your task.".to_string()],
    };
    lines.push(format!(
        "The coder wants to run `{command}` to {}.",
        purpose.trim_end_matches('.')
    ));
    lines.push(format!("The weave asks you first: {}.", weave::subst(rule, &ctx.spec.args)));
    Card {
        title: format!("Let the coder run `{}`?", short(command, COMMAND)),
        lines,
        buttons: buttons(&[
            ("yes", "Let it run"),
            ("no", &no_label(goal)),
            ("always", "Always allow it"),
            ("never", "Never allow it"),
        ]),
    }
}

/// The box lacks what FROM and NEEDS ask for.
pub(crate) fn misfit(ctx: &Ctx) -> Card {
    Card {
        title: format!("Run {} on a box that does not fit?", ctx.name),
        lines: ctx
            .misfit
            .iter()
            .map(|m| format!("{}.", m.trim_end_matches('.')))
            .collect(),
        buttons: buttons(&[("yes", "Run it anyway"), ("no", &later(None))]),
    }
}

/// An agent's new Weavefile changes more than facts, goals, actions and skills.
pub(crate) fn weave_change(
    ctx: &Ctx,
    by: &str,
    why: &str,
    change: &crate::weave::change::Change,
) -> Card {
    let mut lines = vec![
        format!("{by} proposes a new Weavefile: {}.", why.trim_end_matches('.')),
        "A rule change needs your signed answer.".to_owned(),
    ];
    lines.extend(change.removed.iter().map(|l| format!("Removed: {l}")));
    lines.extend(change.added.iter().map(|l| format!("Added: {l}")));
    if !change.free.is_empty() {
        lines.push(format!("Also: {}.", change.summary()));
    }
    Card {
        title: format!("Change the rules of {}?", ctx.name),
        lines,
        buttons: buttons(&[("yes", "Apply it"), ("no", "Keep the rules")]),
    }
}
