//! A DO task: the decider picks skills, their recipes run in code, and when the goal
//! still fails the coder works on it with `shell`, `done` and `ask`.

use std::collections::BTreeMap;
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::model::coder::{Msg, Tally, Tool};
use crate::model::decider::Question;
use crate::service::{Kind, Line};
use crate::weave::{self, Action, Condition, Goal, Rule, Skill};

use super::asks::AskReq;
use super::facts::{self, Values};
use super::goals;
use super::repair::{command, rule_text};
use super::service::Ctx;
use super::{cap, sh};

const RUN_LIMIT: Duration = Duration::from_secs(30 * 60);
const OUTPUT_CAP: usize = 8 * 1024;
const MAX_COMMANDS: usize = 40;
const MAX_CHECKS: u32 = 2;
/// Replies with no tool call before the model counts as failed.
const MAX_IDLE: u32 = 3;
const ERROR_KINDS: [(&str, &str); 5] = [
    (
        "missing package",
        "a program, library or package the command needs is not installed",
    ),
    (
        "permission",
        "access was denied: a file mode, an owner, a user or a missing sudo",
    ),
    (
        "network",
        "a host, port, DNS name or download could not be reached",
    ),
    (
        "config",
        "a setting, a file's content, a path or an argument is wrong",
    ),
    ("other", "none of these"),
];

pub(crate) enum DoResult {
    /// The goal held: the commands that made it, in order.
    Pass(Vec<String>),
    Asked,
    Failed(String),
}

enum End {
    Pass(Vec<String>),
    Asked,
    Next,
    Failed(String),
}

pub(crate) async fn do_task(
    ctx: &Ctx,
    goal: &Goal,
    a: &Action,
    task: &str,
    prior: &mut String,
    rules_ok: &[String],
) -> DoResult {
    let task = weave::subst(task, &ctx.spec.args);
    let values = facts::known(ctx, &facts::all(&ctx.w)).await;
    let skills = pick_skills(ctx, goal, &task, &values).await;
    let mut ran = Vec::new();
    'recipes: for s in &skills {
        for r in &s.recipes {
            let cmd = weave::subst(r, &ctx.spec.args);
            if !command(ctx, goal, a, &cmd, prior).await {
                break 'recipes;
            }
            ran.push(cmd);
        }
    }
    if !ran.is_empty() && goals::check_fresh(ctx, goal).await == Some(true) {
        return DoResult::Pass(ran);
    }
    if ctx.coder.is_none() {
        return DoResult::Failed(format!("{}: no coder for the task", a.name));
    }
    session(ctx, goal, a, &task, &skills, prior, rules_ok).await
}

/// One decider request, one choice per skill: `reached` at 0.5 or more picks it.
async fn pick_skills<'a>(ctx: &'a Ctx, goal: &Goal, task: &str, values: &Values) -> Vec<&'a Skill> {
    let Some(decider) = &ctx.decider else {
        return Vec::new();
    };
    if ctx.w.skills.is_empty() {
        return Vec::new();
    }
    let mut questions = IndexMap::new();
    for s in &ctx.w.skills {
        let when = weave::subst(&s.when, &ctx.spec.args);
        questions.insert(
            s.name.clone(),
            Question::Choice {
                criteria: IndexMap::from([
                    (
                        "reached".to_string(),
                        format!("the situation `{when}` comes up in this `job`"),
                    ),
                    (
                        "unreached".to_string(),
                        format!(
                            "the situation `{when}` does not come up in this `job`, so the coder does not need it"
                        ),
                    ),
                ]),
                instructions: format!(
                    "The coder gets this skill when the situation in `when` comes up in its job. Does the situation `{when}` come up in this `job`?"
                ),
            },
        );
    }
    let state = json!({
        "box": ctx.box_line(),
        "service": ctx.name,
        "job": task,
        "failing_goal": goals::describe(ctx, goal),
        "facts": facts::shown(values),
    });
    let answers = match decider.ask(state, questions).await {
        Ok(a) => a,
        Err(e) => {
            ctx.journal()
                .note(Some(&goal.name), format!("decider: {e:#}"));
            return Vec::new();
        }
    };
    let mut p = BTreeMap::new();
    let mut picked = Vec::new();
    for s in &ctx.w.skills {
        let reached = answers
            .choice(&s.name)
            .and_then(|(_, probs, _)| probs.get("reached").copied())
            .unwrap_or(0.0);
        p.insert(s.name.clone(), reached);
        if reached >= 0.5 {
            picked.push(s);
        }
    }
    let names: Vec<&str> = picked.iter().map(|s| s.name.as_str()).collect();
    ctx.journal().add(Line {
        kind: Kind::Decide,
        goal: Some(goal.name.clone()),
        text: Some(format!(
            "skills: {}",
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        )),
        p: Some(p),
        cost_usd: Some(answers.cost_usd),
        ..Default::default()
    });
    picked
}

fn tools() -> [Tool; 3] {
    [
        Tool {
            name: "shell",
            description: "Run one bash command on the box (bash -eo pipefail -c, 30 minute limit, as the daemon's user). Say why in purpose.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "purpose": {"type": "string", "description": "what this command is for, in one line"},
                },
                "required": ["command", "purpose"],
            }),
        },
        Tool {
            name: "done",
            description: "The goal should hold now. commands: the commands that made the fix, in order, exactly as you ran them; cause: one line on what was wrong.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "commands": {"type": "array", "items": {"type": "string"}},
                    "cause": {"type": "string"},
                },
                "required": ["commands", "cause"],
            }),
        },
        Tool {
            name: "ask",
            description: "Only the person can fix this (a key, a payment, a decision). Ends the task.",
            parameters: json!({
                "type": "object",
                "properties": {"question": {"type": "string"}},
                "required": ["question"],
            }),
        },
    ]
}

fn system_prompt(
    ctx: &Ctx,
    goal: &Goal,
    task: &str,
    values: &Values,
    skills: &[&Skill],
    prior: &str,
) -> String {
    let mut s = format!(
        "You keep the service `{}` running on this box. You work through tools: `shell` runs \
         one bash command at a time, `done` says the task is finished, `ask` hands it to the \
         person. Commands must not wait for input. Start a long-running program in the \
         background with its output redirected to a file (nohup … >file 2>&1 &), or the \
         command never ends.\n",
        ctx.name
    );
    if let Some(about) = &ctx.w.about {
        s.push_str(&format!("About the service: {about}\n"));
    }
    s.push_str(&format!("\nTask: {task}\n"));
    s.push_str(&format!(
        "The goal that must hold: {}\n",
        goals::describe(ctx, goal)
    ));
    if let Condition::Code(_) = goal.cond {
        s.push_str("It is checked in code with the facts below.\n");
    }
    if !ctx.spec.args.is_empty() {
        s.push_str("\nARGs (also environment variables of every command):\n");
        for (k, v) in &ctx.spec.args {
            s.push_str(&format!("  {k}={v}\n"));
        }
    }
    if !ctx.secrets.is_empty() {
        s.push_str("\nSECRETs, environment variables whose values you never see:");
        for (k, _) in &ctx.secrets {
            s.push_str(&format!(" ${k}"));
        }
        s.push('\n');
    }
    let rules: Vec<String> = ctx
        .w
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::Never { text, .. } => {
                Some(format!("  NEVER {}", weave::subst(text, &ctx.spec.args)))
            }
            Rule::Ask { text, .. } => Some(format!(
                "  ASK the person first: {}",
                weave::subst(text, &ctx.spec.args)
            )),
            _ => None,
        })
        .collect();
    if !rules.is_empty() {
        s.push_str("\nRules:\n");
        s.push_str(&rules.join("\n"));
        s.push('\n');
    }
    s.push_str(&format!("\nBox: {}\n", ctx.box_line()));
    if !values.is_empty() {
        s.push_str("\nFacts now:\n");
        for (k, v) in values {
            s.push_str(&format!("  {k}: {}\n", v.show()));
        }
        for f in &ctx.w.facts {
            if let weave::FactSource::Command(c) = &f.source {
                s.push_str(&format!("  ({} is read with: {c})\n", f.name));
            }
        }
    }
    for sk in skills {
        s.push_str(&format!(
            "\nSkill {}:\n{}\n",
            sk.name,
            weave::subst(&sk.body, &ctx.spec.args)
        ));
    }
    if !prior.is_empty() {
        s.push_str(&format!("\nEarlier output:\n{}\n", cap(prior, 8 * 1024)));
    }
    s
}

async fn session(
    ctx: &Ctx,
    goal: &Goal,
    a: &Action,
    task: &str,
    skills: &[&Skill],
    prior: &str,
    rules_ok: &[String],
) -> DoResult {
    let coder = ctx.coder.as_ref().expect("checked by the caller");
    let tools = tools();
    let mut commands = 0usize;
    let mut ok_cmds: Vec<String> = Vec::new();
    let mut history: Vec<String> = Vec::new();
    for i in 0..coder.len() {
        let values = facts::known(ctx, &facts::all(&ctx.w)).await;
        let system = system_prompt(ctx, goal, task, &values, skills, prior);
        let mut first = "Make the goal hold.".to_string();
        if !history.is_empty() {
            first.push_str(&format!(
                " Another model worked on this and the goal did not hold. Its commands:\n{}",
                cap(&history.join("\n"), 8 * 1024)
            ));
        }
        let mut msgs = vec![Msg::User(first)];
        let mut checks = 0;
        let mut idle = 0;
        let mut tally = Tally::default();
        let end = loop {
            if commands >= MAX_COMMANDS {
                break End::Failed(format!("{MAX_COMMANDS} commands without a pass"));
            }
            let reply = match coder.send(i, &system, &msgs, &tools).await {
                Ok(r) => r,
                Err(e) => {
                    ctx.journal().note(Some(&goal.name), format!("{e:#}"));
                    break End::Next;
                }
            };
            tally.add(&reply);
            msgs.push(Msg::Assistant {
                text: reply.text.clone(),
                calls: reply.calls.clone(),
            });
            if reply.calls.is_empty() {
                idle += 1;
                if idle >= MAX_IDLE {
                    break End::Next;
                }
                msgs.push(Msg::User(
                    "Call one of the tools: shell, done or ask.".into(),
                ));
                continue;
            }
            let mut results = Vec::new();
            let mut end = None;
            for c in &reply.calls {
                if end.is_some() {
                    results.push((c.id.clone(), "not run: the task ended".to_string()));
                    continue;
                }
                let out = match c.name.as_str() {
                    "shell" => {
                        commands += 1;
                        match shell(ctx, goal, a, &c.input, rules_ok, &mut ok_cmds).await {
                            Ok((cmd, out)) => {
                                history.push(cmd);
                                out
                            }
                            Err(()) => {
                                end = Some(End::Asked);
                                String::new()
                            }
                        }
                    }
                    "done" => match goals::check_fresh(ctx, goal).await {
                        Some(true) => {
                            let named: Vec<String> = c.input["commands"]
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|v| v.as_str())
                                        .map(|s| s.trim().to_string())
                                        .collect()
                                })
                                .unwrap_or_default();
                            // A named command is kept as it ran: the coder often names
                            // a part of a chained command, or a command with a check added.
                            let mut kept: Vec<String> = Vec::new();
                            for n in &named {
                                let ran = ok_cmds.iter().find(|o| o.trim() == n).or_else(|| {
                                    ok_cmds
                                        .iter()
                                        .find(|o| o.contains(n.as_str()) || n.contains(o.trim()))
                                });
                                if let Some(o) = ran {
                                    if !kept.contains(o) {
                                        kept.push(o.clone());
                                    }
                                }
                            }
                            if let Some(cause) = c.input["cause"].as_str() {
                                ctx.journal()
                                    .note(Some(&goal.name), format!("coder: {cause}"));
                            }
                            end = Some(End::Pass(kept));
                            String::new()
                        }
                        _ => {
                            checks += 1;
                            if checks >= MAX_CHECKS {
                                end = Some(End::Next);
                            }
                            let now = facts::read(ctx, &goals::needs(&ctx.w, goal), false).await;
                            let shown: Vec<String> = now
                                .iter()
                                .map(|(k, v)| format!("{k}: {}", v.show()))
                                .collect();
                            format!(
                                "The goal still does not hold. Facts now:\n{}",
                                shown.join("\n")
                            )
                        }
                    },
                    "ask" => {
                        let q = c.input["question"]
                            .as_str()
                            .unwrap_or("the coder needs the person")
                            .to_string();
                        ctx.ask(AskReq {
                            goal: Some(goal.name.clone()),
                            action: Some(a.name.clone()),
                            rule: None,
                            title: q,
                            text: format!(
                                "{}\nThe coder says only you can fix this. Answer yes once it is fixed to run {} again.",
                                goals::describe(ctx, goal),
                                a.name
                            ),
                            facts: facts::shown(&values),
                            answers: vec!["yes".into(), "no".into()],
                        });
                        end = Some(End::Asked);
                        String::new()
                    }
                    other => format!("There is no tool {other}."),
                };
                results.push((c.id.clone(), out));
            }
            if let Some(e) = end {
                break e;
            }
            msgs.push(Msg::Results(results));
        };
        ctx.journal().add(Line {
            kind: Kind::Note,
            goal: Some(goal.name.clone()),
            action: Some(a.name.clone()),
            text: Some(format!(
                "coder {}: {}, {commands} commands so far",
                coder.model(i),
                tally.describe(system.chars().count())
            )),
            cost_usd: Some(tally.cost_usd),
            ..Default::default()
        });
        match end {
            End::Pass(cmds) => return DoResult::Pass(cmds),
            End::Asked => return DoResult::Asked,
            End::Failed(why) => return DoResult::Failed(format!("{}: {why}", a.name)),
            End::Next => {}
        }
    }
    DoResult::Failed(format!("{}: every coder model failed the task", a.name))
}

/// One coder command: gated by the quoted rules, run, journaled. `Ok((command, what the
/// coder reads))`, or `Err(())` when an ask was raised and the session ends.
async fn shell(
    ctx: &Ctx,
    goal: &Goal,
    a: &Action,
    input: &Value,
    rules_ok: &[String],
    ok_cmds: &mut Vec<String>,
) -> Result<(String, String), ()> {
    let command = input["command"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    let purpose = input["purpose"].as_str().unwrap_or_default().to_string();
    if command.is_empty() {
        return Ok((command, "shell needs a command.".into()));
    }
    let quoted: Vec<(&Rule, bool, String)> = ctx
        .w
        .rules
        .iter()
        .filter_map(|r| match r {
            Rule::Never { text, .. } => Some((r, true, weave::subst(text, &ctx.spec.args))),
            Rule::Ask { text, .. } => Some((r, false, weave::subst(text, &ctx.spec.args))),
            _ => None,
        })
        .collect();
    if !quoted.is_empty() {
        let Some(decider) = &ctx.decider else {
            return Ok((command, "Refused: no decider to check the rules.".into()));
        };
        let mut questions = IndexMap::new();
        // "Is running this command a case of" judges the act. "Does it do this forbidden
        // thing" let shared words win: a read-only curl hit "accept connections from the
        // internet" at 0.75. On 25 commands, 5 rules each: 0 wrong (true >= 0.58, false
        // <= 0.34), against 5 wrong for the old question.
        for (i, (_, _, text)) in quoted.iter().enumerate() {
            let act = text.strip_prefix("before ").unwrap_or(text);
            questions.insert(
                format!("rule{i}"),
                Question::Noul {
                    instructions: format!("Is running this command a case of: {act}?"),
                },
            );
        }
        let state = json!({"command": ctx.redact(&command), "purpose": ctx.redact(&purpose)});
        let answers = match decider.ask(state, questions).await {
            Ok(x) => x,
            Err(e) => {
                ctx.journal()
                    .note(Some(&goal.name), format!("decider: {e:#}"));
                return Ok((command, "Refused: the rule check failed; try again.".into()));
            }
        };
        ctx.journal().spend(answers.cost_usd);
        for (i, (r, never, _)) in quoted.iter().enumerate() {
            let p = answers.noul(&format!("rule{i}")).unwrap_or(1.0);
            if p < 0.5 {
                continue;
            }
            let rule = rule_text(r);
            let kept = ctx.journal().kept(&rule, &a.name);
            if *never || kept == Some("never") {
                ctx.journal().add(Line {
                    kind: Kind::Gate,
                    goal: Some(goal.name.clone()),
                    action: Some(a.name.clone()),
                    text: Some(format!("{rule}: {}", ctx.redact(&command))),
                    p: Some(BTreeMap::from([("yes".to_string(), p)])),
                    ..Default::default()
                });
                return Ok((
                    command,
                    format!("Refused by the rule {rule} ({p:.2}). Do it another way, or call ask."),
                ));
            }
            if kept == Some("always") || rules_ok.iter().any(|ok| *ok == rule) {
                continue;
            }
            let values = facts::known(ctx, &facts::all(&ctx.w)).await;
            ctx.ask(AskReq {
                goal: Some(goal.name.clone()),
                action: Some(a.name.clone()),
                rule: Some(rule.clone()),
                title: format!("{} for {}: run `{command}`?", a.name, goal.name),
                text: format!(
                    "The coder wants to run `{command}` to {purpose}.\n{rule} holds ({p:.2}). Answer yes to let the task go on."
                ),
                facts: facts::shown(&values),
                answers: vec!["yes".into(), "no".into(), "always".into(), "never".into()],
            });
            return Err(());
        }
    }
    let o = sh(&command, &ctx.env, RUN_LIMIT).await;
    let shown = ctx.redact(&cap(&o.both(), OUTPUT_CAP));
    ctx.journal().add(Line {
        kind: Kind::Command,
        goal: Some(goal.name.clone()),
        action: Some(a.name.clone()),
        text: Some(command.clone()),
        ok: Some(o.ok()),
        ..Default::default()
    });
    if o.ok() {
        ok_cmds.push(command.clone());
        return Ok((command, format!("exit 0\n{shown}")));
    }
    let code = o.code.map_or("killed".to_string(), |c| format!("exit {c}"));
    let kind = classify(ctx, &command, &shown).await;
    let line = match kind {
        Some(k) => format!("{code}; the error looks like: {k}\n{shown}"),
        None => format!("{code}\n{shown}"),
    };
    Ok((command, line))
}

/// The decider sorts a failed command's error.
async fn classify(ctx: &Ctx, command: &str, output: &str) -> Option<String> {
    let decider = ctx.decider.as_ref()?;
    let criteria: IndexMap<String, String> = ERROR_KINDS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut questions = IndexMap::new();
    questions.insert(
        "kind".to_string(),
        Question::Choice {
            criteria,
            instructions:
                "The command in `command` failed with `output`. What kind of error is it?".into(),
        },
    );
    let state = json!({"command": ctx.redact(command), "output": output});
    match decider.ask(state, questions).await {
        Ok(a) => {
            ctx.journal().spend(a.cost_usd);
            a.choice("kind").map(|(c, _, _)| c.to_string())
        }
        Err(_) => None,
    }
}
