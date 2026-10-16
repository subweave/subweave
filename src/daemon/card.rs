//! The card on each escalation, built in code from the weave's plain words and the
//! service's fold: a question, the read set of the verdict, and buttons that name the
//! move. One function per kind of escalation. Never every fact: only those the failing
//! goal's condition reads, and those its action and the rule that holds it read.

use std::collections::BTreeSet;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::channel::card::{
    ActionShown, Button, Card, Check, DrillShown, GoalShown, Held, KindShown, Kinds, Known,
    PartShown, Read, Series, WindowShown,
};
use crate::model::coder::{Coder, Msg, Reply, Tool};
use crate::weave::{
    self, Action, Condition, Drill, Expr, FactSource, FactType, Goal, PartField, Rule, Step,
    show_duration,
};

use super::observe::{Snapshot, Span};
use super::service::{Ctx, no_park};
use super::{cap, goals};

const SUMMARY_SYSTEM: &str = "You write at most two plain lines at the top of a card on a \
    person's phone. The card is from Subweave, the daemon that keeps a service on a server \
    running. Subweave reached the card's verdict in code: its goal does not hold, and its \
    title is what it asks. Take the verdict as given: never judge or doubt whether the goal \
    holds, whether a line is trouble, or what the answer should be. Explain why the verdict \
    is what it is, from what the card shows: its facts, its window's lines and what is known \
    of them, its action and the rule that holds it. Each line is one short sentence of at \
    most 16 plain words, for a reader who does not know the service's internals: say what a \
    fact means, not its name. Never add a number, version, name or cause the card does not \
    state, and never say which button to press. When the card does not show why, save no \
    lines. Call save.";
const SUMMARY_EFFORT: &str = "low";
/// The most lines a summary has, and the most words a line keeps: Steery refuses a card
/// whose gist runs longer.
const SUMMARY_LINES: usize = 2;
const SUMMARY_WORDS: usize = 20;

/// The longest goal text in a title, and the longest command.
const SUBJECT: usize = 80;
const COMMAND: usize = 60;
const FACT: usize = 300;
/// The most kinds of line a card shows, the most reads of a fact over its window, and
/// the first lines of a part's notes.
const KINDS: usize = 8;
const SERIES: usize = 8;
const NOTES: usize = 3;

/// The lines a model wrote, or why they cannot be used, and its reply.
pub(crate) struct Summary {
    pub lines: Result<Vec<String>, String>,
    pub reply: Reply,
}

/// One call to the coder's first model: at most two plain lines that explain the card's
/// verdict, from `card`, the card as the person reads it, secrets already redacted.
pub(crate) async fn summary(coder: &Coder, card: &Value) -> Result<Summary> {
    let tool = Tool {
        name: "save",
        description: "Save the card's lines: none when the card does not show why.",
        parameters: json!({
            "type": "object",
            "properties": {
                "lines": {"type": "array", "items": {"type": "string"}, "maxItems": SUMMARY_LINES}
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
    if lines.is_empty() {
        return Err("the model saw no why in the card".into());
    }
    if lines.len() > SUMMARY_LINES {
        return Err(format!("save has {} lines, not at most {SUMMARY_LINES}", lines.len()));
    }
    if let Some(l) = lines.iter().find(|l| l.split_whitespace().count() > SUMMARY_WORDS) {
        return Err(format!("a line is over {SUMMARY_WORDS} words: {}", short(l, 60)));
    }
    Ok(lines)
}

fn buttons(b: &[(&str, &str)]) -> Vec<Button> {
    b.iter()
        .map(|(w, l)| Button {
            word: (*w).to_string(),
            label: (*l).to_string(),
        })
        .collect()
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

/// A code rule's condition: what it reads.
fn rule_cond(r: &Rule) -> Option<&Expr> {
    match r {
        Rule::AskBefore { cond, .. } | Rule::NeverBefore { cond, .. } => cond.as_ref(),
        Rule::AskIf { expr, .. } | Rule::NeverIf { expr, .. } => Some(expr),
        Rule::Never { .. } | Rule::Ask { .. } => None,
    }
}

/// An action as a card shows it; `held` the rule that holds it.
fn action(ctx: &Ctx, a: &Action, held: Option<&Rule>) -> ActionShown {
    let dos: Vec<String> = a
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Do { task, .. } => Some(weave::subst(task, &ctx.spec.args)),
            _ => None,
        })
        .collect();
    ActionShown {
        name: a.name.clone(),
        label: label(ctx, a),
        does: if dos.is_empty() {
            vec![format!("it runs `{}`", a.name)]
        } else {
            dos
        },
        changes: a.effects().iter().map(|e| e.as_str().to_string()).collect(),
        held: held.map(|r| Held {
            rule: weave::subst(&r.text(), &ctx.spec.args),
            because: r.because().map(|b| weave::subst(b, &ctx.spec.args)),
        }),
    }
}

/// A fact's value as a person reads it: a NUMBER with its unit.
fn shown(ctx: &Ctx, fact: &str, text: &str) -> String {
    let text = cap(text.trim(), FACT);
    match ctx.w.fact(fact).map(|f| f.ty) {
        Some(FactType::Number(Some(u))) if !text.starts_with("broken read") => format!("{text}{u}"),
        _ => text.to_string(),
    }
}

/// A card on goal `g` and the read set of its verdict: the facts its condition reads,
/// those `a`'s WHEN names in backticks and those `rule`'s IF reads, as now. A fact a
/// window function takes shows its reads over the window; a LINES fact its window's
/// lines by kind, with what is known of each and who knows it; a part's facts show the
/// part: running, newest, security and the first lines of its notes.
fn about(ctx: &Ctx, g: &Goal, a: Option<&Action>, rule: Option<&Rule>) -> Card {
    let span = goals::span(g, Snapshot::now());
    let mut names = ctx.w.reads(g);
    if let Some(a) = a {
        names.extend(
            weave::backticked(&a.when)
                .filter(|n| ctx.w.fact(n).is_some())
                .map(str::to_string),
        );
    }
    if let Some(e) = rule.and_then(rule_cond) {
        e.walk(&mut |x| {
            if let Expr::Name(n) | Expr::Window(_, n) = x
                && ctx.w.fact(n).is_some()
            {
                names.insert(n.clone());
            }
        });
    }
    // The facts a window function of the goal takes.
    let mut windowed = BTreeSet::new();
    if let Condition::Code(e) = &g.cond {
        e.walk(&mut |x| {
            if let Expr::Window(_, n) = x {
                windowed.insert(n.clone());
            }
        });
    }
    let values = ctx.observe.values(&names, span);
    let mut reads = Vec::new();
    let mut series = Vec::new();
    let mut lines = Vec::new();
    let mut parts = BTreeSet::new();
    for n in &names {
        let Some(f) = ctx.w.fact(n) else { continue };
        match (&f.source, f.ty) {
            (FactSource::Part { part, .. }, _) => {
                parts.insert(part.clone());
            }
            (_, FactType::Lines) => lines.push(n.as_str()),
            _ if windowed.contains(n) => {
                let all = ctx.observe.series(n, span);
                let more = all.len().saturating_sub(SERIES);
                series.push(Series {
                    fact: n.clone(),
                    values: all[more..].iter().map(|v| shown(ctx, n, v)).collect(),
                    more,
                });
            }
            _ => {
                if let Some(v) = values.get(n) {
                    reads.push(Read {
                        fact: n.clone(),
                        value: shown(ctx, n, &v.text),
                    });
                }
            }
        }
    }
    for (fact, seen) in ctx.shared.registry.seen(&ctx.w.reads_other(g)) {
        reads.push(Read {
            fact,
            value: cap(seen.text.trim(), FACT).to_string(),
        });
    }
    let window = (ctx.w.windowed(g) || !lines.is_empty()).then(|| WindowShown {
        over: show_duration(g.over()),
        values: series,
        lines: (!lines.is_empty()).then(|| kinds(ctx, g, &lines, span)),
    });
    Card {
        goal: Some(GoalShown {
            name: g.name.clone(),
            sentence: subject(ctx, g),
            check: match &g.cond {
                Condition::Code(e) => Check::Code(e.to_string()),
                Condition::Quoted(t) => Check::Quoted(weave::subst(t, &ctx.spec.args)),
            },
        }),
        reads,
        window,
        parts: parts.iter().map(|p| part(ctx, p, span)).collect(),
        ..Card::default()
    }
}

/// The window's lines of `facts` by kind, with what the box knows of each.
fn kinds(ctx: &Ctx, g: &Goal, facts: &[&str], span: Span) -> Kinds {
    let all = goals::kinds_of(ctx, g, facts.iter().copied(), span);
    Kinds {
        more: all.len().saturating_sub(KINDS),
        kinds: all
            .into_iter()
            .take(KINDS)
            .map(|k| KindShown {
                count: k.count,
                sample: cap(&k.sample, FACT).to_string(),
                known: k.known.map(|(c, s)| Known {
                    class: c.as_str().to_string(),
                    by: s.who(),
                }),
            })
            .collect(),
    }
}

/// A part as its derived facts read now.
fn part(ctx: &Ctx, name: &str, span: Span) -> PartShown {
    let field = |f: PartField| format!("{name}.{}", f.as_str());
    let names: BTreeSet<String> = [PartField::Running, PartField::Newest, PartField::Security, PartField::Notes]
        .into_iter()
        .map(field)
        .collect();
    let values = ctx.observe.values(&names, span);
    let get = |f: PartField| {
        values
            .get(&field(f))
            .map_or_else(|| "unknown".to_string(), |v| cap(v.text.trim(), FACT).to_string())
    };
    let notes = values
        .get(&field(PartField::Notes))
        .map(|v| {
            v.text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .take(NOTES)
                .map(|l| cap(l, FACT).to_string())
                .collect()
        })
        .unwrap_or_default();
    PartShown {
        name: name.to_string(),
        running: get(PartField::Running),
        newest: get(PartField::Newest),
        security: get(PartField::Security),
        notes,
    }
}

/// The warning when a goal starts failing and the loop acts.
pub(crate) fn fixing(ctx: &Ctx, g: &Goal) -> Card {
    Card {
        title: format!("Fixing: {}", subject(ctx, g)),
        lines: vec!["Subweave is fixing it, and asks you only if it needs you.".into()],
        ..about(ctx, g, None, None)
    }
}

/// `TRIES` used up.
pub(crate) fn tries(ctx: &Ctx, g: &Goal, n: u32, per: Duration) -> Card {
    Card {
        title: format!("Try again to make this true: {}?", short(&subject(ctx, g), SUBJECT)),
        lines: vec![format!(
            "Subweave tried {n} times in {}, and it still does not hold.",
            show_duration(per)
        )],
        buttons: buttons(&[("yes", "Try again"), ("no", &later(Some(g)))]),
        ..about(ctx, g, None, None)
    }
}

/// No action fits the goal: a rule refuses each one that does, or the `IF` of each one
/// written for it is not true now, which says what that fix would need.
pub(crate) fn no_fit(
    ctx: &Ctx,
    g: &Goal,
    refused: &[(&Action, &Rule, bool)],
    off: &[&Action],
) -> Card {
    let mut lines = Vec::new();
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
        ..about(ctx, g, None, None)
    }
}

/// The undo of `of` failed or could not run, for `why`.
pub(crate) fn undo_failed(ctx: &Ctx, g: &Goal, of: &str, undo: &Action, why: &str) -> Card {
    Card {
        title: format!("Undo failed. Can you make this true: {}?", short(&subject(ctx, g), SUBJECT)),
        action: Some(action(ctx, undo, None)),
        undoes: Some(action_name(ctx, of)),
        lines: vec![
            format!("{} did not help or broke what held.", action_name(ctx, of)),
            format!("The undo failed: {}.", why.trim_end_matches('.')),
        ],
        buttons: buttons(&[("yes", "Fixed it, check again"), ("no", &later(Some(g)))]),
        ..about(ctx, g, Some(undo), None)
    }
}

/// Several actions fit and the decider did not pick one: `why` says why not.
pub(crate) fn which(ctx: &Ctx, g: &Goal, cands: &[&Action], why: &str) -> Card {
    let mut lines = vec![why.to_string()];
    for a in cands {
        let when = weave::subst(&a.when, &ctx.spec.args);
        let when = when.trim().trim_end_matches('.');
        lines.push(if when.is_empty() {
            format!("{}.", label(ctx, a))
        } else {
            format!("{}: right when {when}.", label(ctx, a))
        });
    }
    let mut b: Vec<Button> = cands
        .iter()
        .map(|a| Button {
            word: a.name.clone(),
            label: label(ctx, a),
        })
        .collect();
    b.push(Button {
        word: "no".into(),
        label: later(Some(g)),
    });
    Card {
        title: format!("Which fix for: {}?", short(&subject(ctx, g), SUBJECT)),
        lines,
        buttons: b,
        ..about(ctx, g, None, None)
    }
}

/// An `ASK BEFORE` or `ASK IF` holds the action the loop picked; `undoes` the action it
/// is the UNDO of, when it is one.
pub(crate) fn gate(ctx: &Ctx, g: &Goal, a: &Action, rule: &Rule, undoes: Option<&str>) -> Card {
    Card {
        title: match &a.label {
            Some(_) => format!("{}?", label(ctx, a).trim_end_matches(['.', '?'])),
            None => format!("Run `{}`?", a.name),
        },
        action: Some(action(ctx, a, Some(rule))),
        undoes: undoes.map(|u| action_name(ctx, u)),
        buttons: buttons(&[
            ("yes", "Run it"),
            ("no", &later(Some(g))),
            ("always", "Always run it"),
            ("never", "Never run it"),
        ]),
        ..about(ctx, g, Some(a), Some(rule))
    }
}

/// What a coder ask is about: the goal and the action the coder runs for it, or the
/// person's task.
fn work(ctx: &Ctx, goal: Option<&Goal>, act: &str, task: &str) -> Card {
    match goal {
        Some(g) => {
            let a = ctx.w.action(act);
            Card {
                action: a.map(|a| action(ctx, a, None)),
                lines: vec![format!("The coder runs {} to fix it.", action_name(ctx, act))],
                ..about(ctx, g, a, None)
            }
        }
        None => Card::note("", vec![format!("Your task: {}", short(task, 300))]),
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
) -> Card {
    let mut card = work(ctx, goal, action, task);
    card.title = question.to_string();
    card.lines.push("The coder says only you can do this.".into());
    card.buttons = buttons(&[("yes", "Done, try again"), ("no", &no_label(goal))]);
    card
}

/// The coder does not get closer: `tried` says how far it got and its last commands.
pub(crate) fn coder_stuck(
    ctx: &Ctx,
    goal: Option<&Goal>,
    action: &str,
    task: &str,
    tried: Vec<String>,
) -> Card {
    let mut card = work(ctx, goal, action, task);
    card.title = match goal {
        Some(g) => format!("Can you make this true: {}?", short(&subject(ctx, g), SUBJECT)),
        None => format!("Can you help with this task: {}?", short(task, SUBJECT)),
    };
    card.lines.extend(tried);
    card.buttons = buttons(&[("yes", "Done, try again"), ("no", &no_label(goal))]);
    card
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
    let mut card = match goal {
        Some(_) => work(ctx, goal, action, ""),
        None => Card::note("", vec!["For your task.".into()]),
    };
    card.title = format!("Let the coder run `{}`?", short(command, COMMAND));
    card.lines.push(format!(
        "The coder wants to run `{command}` to {}.",
        purpose.trim_end_matches('.')
    ));
    card.lines.push(format!("The weave asks you first: {}.", weave::subst(rule, &ctx.spec.args)));
    card.buttons = buttons(&[
        ("yes", "Let it run"),
        ("no", &no_label(goal)),
        ("always", "Always allow it"),
        ("never", "Never allow it"),
    ]);
    card
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
        ..Card::default()
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
        ..Card::default()
    }
}

/// A drill failed: its goal did not notice what BREAK did (`missed`), or the loop did not
/// make it hold again in time. `restored`: whether RESTORE passed; none when the drill
/// has none. A weave defect, for whoever fixes the weave.
pub(crate) fn drill_failed(ctx: &Ctx, d: &Drill, missed: bool, restored: Option<bool>) -> Card {
    let label = super::drill::label(ctx, d);
    let mut lines = vec![format!("BREAK ran `{}`.", short(&d.breaks, COMMAND))];
    lines.push(if missed {
        format!(
            "Its goal did not fail within {}: its check does not see this failure.",
            show_duration(d.fails_within)
        )
    } else {
        format!(
            "Its goal failed, but did not hold again within {} of the break: no action repaired it in time.",
            show_duration(d.holds_within)
        )
    });
    lines.push(match (&d.restore, restored) {
        (Some(r), Some(true)) => format!("RESTORE `{}` put it back.", short(r, COMMAND)),
        (Some(r), _) => format!("RESTORE `{}` failed: what BREAK did may still be in place.", short(r, COMMAND)),
        (None, _) => "The drill has no RESTORE: what BREAK did may still be in place.".into(),
    });
    lines.push(if missed {
        "This is a weave defect: fix the goal's check so it fails on this, then drill again.".into()
    } else {
        "This is a weave defect: fix the actions for this goal, or the drill's HOLDS WITHIN, then drill again.".into()
    });
    let verb = if missed { "did not notice" } else { "did not recover from" };
    let base = match ctx.w.goal(&d.expect) {
        Some(g) => about(ctx, g, None, None),
        None => Card::default(),
    };
    Card {
        title: format!("Drill: `{}` {verb} `{}`. Drill again?", d.expect, short(&label, SUBJECT)),
        drill: Some(DrillShown {
            name: d.name.clone(),
            label,
        }),
        lines,
        buttons: buttons(&[("yes", "Drill again"), ("no", "Leave it")]),
        ..base
    }
}
