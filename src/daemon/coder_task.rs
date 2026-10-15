//! A DO task: the decider picks skills, their recipes run in code, and when the goal
//! still fails the coder works on it with `shell`, `done` and `ask`. A person's task is
//! a DO task with no goal: `done` ends it.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::model::coder::{Msg, Tally, Tool};
use crate::model::decider::Question;
use crate::paths;
use crate::weave::{self, Condition, Goal, Rule, Skill};

use super::asks::AskReq;
use super::card;
use super::observe::{self, Values};
use super::goals;
use super::repair::{command, command_output};
use super::service::Ctx;
use super::state::{Event, Kind};
use super::{cap, cap_to, ms, sh};

const RUN_LIMIT: Duration = Duration::from_secs(30 * 60);
const OUTPUT_CAP: usize = 8 * 1024;
/// Output files kept per service, the newest.
const OUTPUT_FILES: usize = 200;
const MAX_COMMANDS: usize = 40;
const MAX_CHECKS: u32 = 2;
/// Replies with no tool call before the model counts as failed.
const MAX_IDLE: u32 = 3;
/// Message text past which old tool results are masked. On the box journal (2026-10-14)
/// the messages of the last request were about 1.7k tokens in each session that passed
/// and 5.4k in the longest flash session that made progress, against 25k and 45k in the
/// two that wandered. 48 KiB, about 12k tokens, masks no session that passed.
const MASK_AT: usize = 48 * 1024;
/// Tool results that masking leaves whole, the newest.
const MASK_KEEP: usize = 4;
/// Shell commands between two progress judgements, counted over the model chain.
const JUDGE_EVERY: usize = 10;
/// The progress judge's criteria; `closer` under 0.5 ends the session. Replayed on the
/// box journal (2026-10-14) at each 10th command: the two `quiet` fixes that ran 40
/// commands each scored 0.10 to 0.19 at command 10; the sessions that passed scored 0.92
/// to 1.0 at their last command. "Do or test what the task asks" keeps a read-only task
/// closer; "change or test what the goal needs" put one at 0.26.
const PROGRESS: [(&str, &str); 5] = [
    (
        "closer",
        "they do or test what the task asks, and each builds on what the ones before found",
    ),
    (
        "reading",
        "they read logs, files or settings around the problem, but do and test nothing the task asks",
    ),
    (
        "repeating",
        "they repeat earlier reads or tries with small changes",
    ),
    ("waiting", "they sleep or wait for something to happen"),
    (
        "elsewhere",
        "they work on something other than the task: another part of the box, or the daemon that runs the coder",
    ),
];
/// Every box's rule, after the weave's: the person and `--on` reach the box over SSH. A
/// firewall command alone cannot show whether SSH stays allowed, so the rule names the
/// exception. On 20 commands "cut off SSH access to this box" passed `ufw enable` (0.47);
/// this text refused it (0.69) and passed each command that allowed SSH in the same change.
const BOX_ACCESS: &str = "cut off SSH access to this box, or risk it: a firewall change that \
    does not allow SSH in the same command, stopping sshd, or taking the network down";
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

/// What a DO task works for: an action's goal, or a person's task (`action` is `task`,
/// no goal). Every line it writes names both.
#[derive(Clone, Copy)]
pub(crate) struct Work<'a> {
    pub goal: Option<&'a Goal>,
    pub action: &'a str,
}

impl Work<'_> {
    pub fn goal_name(&self) -> Option<String> {
        self.goal.map(|g| g.name.clone())
    }

    /// For lines that name no action in an action's run: a task's lines all say `task`.
    fn task_action(&self) -> Option<String> {
        self.goal.is_none().then(|| self.action.to_string())
    }

    fn note(&self, ctx: &Ctx, text: String) {
        ctx.journal().add(Kind::Note {
            goal: self.goal_name(),
            action: self.task_action(),
            n: None,
            text,
        });
    }
}

pub(crate) enum DoResult {
    /// The goal held, or the person's task is done: the commands that made it, in order,
    /// and the coder's summary of a task.
    Pass {
        commands: Vec<String>,
        summary: Option<String>,
    },
    Asked,
    Failed(String),
}

enum End {
    Pass(Vec<String>, Option<String>),
    Asked,
    Next,
    Failed(String),
}

pub(crate) async fn do_task(
    ctx: &Ctx,
    work: Work<'_>,
    task: &str,
    prior: &mut String,
    rules_ok: &[String],
    done: &[String],
) -> DoResult {
    let task = weave::subst(task, &ctx.spec.args);
    // The goal is judged over the part of its window since this work began: lines from
    // before it do not count against it. The WITHIN wait judges it again afterwards.
    let since = observe::now_ms();
    let values = goals::known(ctx, work.goal).await;
    let skills = pick_skills(ctx, Some(work), &task, Some(&values)).await;
    let mut ran = Vec::new();
    'recipes: for s in &skills {
        for r in &s.recipes {
            let cmd = weave::subst(r, &ctx.spec.args);
            if done.contains(&cmd) {
                continue;
            }
            if !command(ctx, work, &cmd, prior).await {
                break 'recipes;
            }
            ran.push(cmd);
        }
    }
    if let Some(goal) = work.goal {
        if !ran.is_empty() && goals::check_fresh(ctx, goal, Some(since)).await == Some(true) {
            return DoResult::Pass {
                commands: ran,
                summary: None,
            };
        }
    }
    if ctx.coder.is_none() {
        return DoResult::Failed(format!("{}: no coder for the task", work.action));
    }
    // No recipe ran: the facts just read still hold for the first coder model.
    let fresh = ran.is_empty().then_some(values);
    session(ctx, work, &task, &skills, prior, rules_ok, fresh, since).await
}

/// One decider request, one choice per skill: `reached` at 0.5 or more picks it. `work` is
/// none for a learn, whose job is the fact's text; `values`, the facts the decider sees.
pub(crate) async fn pick_skills<'a>(
    ctx: &'a Ctx,
    work: Option<Work<'_>>,
    job: &str,
    values: Option<&Values>,
) -> Vec<&'a Skill> {
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
    let mut state = json!({
        "box": ctx.box_line(),
        "service": ctx.name,
        "job": job,
    });
    if let Some(values) = values {
        state["facts"] = json!(observe::shown(values));
    }
    if let Some(goal) = work.and_then(|w| w.goal) {
        state["failing_goal"] = json!(goals::describe(ctx, goal));
    }
    let start = Instant::now();
    let answers = match decider.ask(state, questions).await {
        Ok(a) => a,
        Err(e) => {
            match work {
                Some(w) => w.note(ctx, format!("decider: {e:#}")),
                None => ctx.journal().note(None, format!("decider: {e:#}")),
            }
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
    ctx.journal().add(Event {
        cost_usd: Some(answers.cost_usd),
        usage: Some(decider.usage(&answers)),
        ms: ms(start),
        ..Kind::Decide {
            goal: work.and_then(|w| w.goal_name()),
            action: work.and_then(|w| w.task_action()),
            text: format!(
                "skills: {}",
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            ),
            p,
        }
        .into()
    });
    picked
}

fn tools(work: Work<'_>) -> [Tool; 3] {
    let done = match work.goal {
        Some(_) => Tool {
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
        None => Tool {
            name: "done",
            description: "The task is done. commands: the commands that did it, in order, exactly as you ran them; summary: one line on what you did and what came of it.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "commands": {"type": "array", "items": {"type": "string"}},
                    "summary": {"type": "string"},
                },
                "required": ["commands", "summary"],
            }),
        },
    };
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
        done,
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
    work: Work<'_>,
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
    match work.goal {
        Some(goal) => {
            s.push_str(&format!("\nTask: {task}\n"));
            s.push_str(&format!(
                "The goal that must hold: {}\n",
                goals::describe(ctx, goal)
            ));
            if let Condition::Code(_) = goal.cond {
                s.push_str("It is checked in code with the facts below.\n");
            }
        }
        None => s.push_str(&format!(
            "\nTask from the person: {task}\nDo this task and change nothing it does not \
             need, then call `done` with a one-line summary.\n"
        )),
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
    let mut rules: Vec<String> = ctx
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
    rules.push(format!("  NEVER {BOX_ACCESS}"));
    s.push_str("\nRules:\n");
    s.push_str(&rules.join("\n"));
    s.push('\n');
    s.push_str(&format!("\nBox: {}\n", ctx.box_line()));
    if !values.is_empty() {
        s.push_str("\nFacts now:\n");
        for (k, v) in values {
            s.push_str(&format!("  {k}: {}\n", v.text));
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

/// The coder's `session_id`: one per episode and chain model, so each model's provider
/// stays the same for the conversation and its prompt cache holds.
pub(crate) fn session_id(i: usize) -> Option<String> {
    super::episode::current().map(|ep| format!("{ep}-{i}"))
}

async fn session(
    ctx: &Ctx,
    work: Work<'_>,
    task: &str,
    skills: &[&Skill],
    prior: &str,
    rules_ok: &[String],
    mut fresh: Option<Values>,
    since: i64,
) -> DoResult {
    let coder = ctx.coder.as_ref().expect("checked by the caller");
    let tools = tools(work);
    let mut commands = 0usize;
    let mut ok_cmds: Vec<String> = Vec::new();
    let mut tried: Vec<Ran> = Vec::new();
    let mut start: Option<BTreeMap<String, String>> = None;
    let mut judged = 0usize;
    for i in 0..coder.len() {
        let values = match fresh.take() {
            Some(v) => v,
            None => goals::known(ctx, work.goal).await,
        };
        let start = &*start.get_or_insert_with(|| observe::shown(&values));
        let system = system_prompt(ctx, work, task, &values, skills, prior);
        let mut first = match work.goal {
            Some(_) => "Make the goal hold.",
            None => "Do the task.",
        }
        .to_string();
        if !tried.is_empty() {
            let commands: Vec<&str> = tried.iter().map(|r| r.command.as_str()).collect();
            first.push_str(&format!(
                " Another model worked on this and it is not done. Its commands:\n{}",
                cap(&commands.join("\n"), 8 * 1024)
            ));
        }
        let mut msgs = vec![Msg::User(first)];
        let mut checks = 0;
        let mut idle = 0;
        let mut tally = Tally::default();
        let started = Instant::now();
        let sid = session_id(i);
        let mut files: HashMap<String, PathBuf> = HashMap::new();
        let end = loop {
            if commands >= MAX_COMMANDS {
                break End::Failed(format!("{MAX_COMMANDS} commands without a pass"));
            }
            let reply = match coder.send(i, sid.as_deref(), None, &system, &msgs, &tools).await {
                Ok(r) => r,
                Err(e) => {
                    work.note(ctx, format!("{e:#}"));
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
                        match shell(ctx, work, &c.input, rules_ok, &mut ok_cmds).await {
                            Ok(ran) => {
                                if let Some(f) = &ran.file {
                                    files.insert(c.id.clone(), f.clone());
                                }
                                let out = ran.result.clone();
                                tried.push(ran);
                                out
                            }
                            Err(()) => {
                                end = Some(End::Asked);
                                String::new()
                            }
                        }
                    }
                    "done" => {
                        let held = match work.goal {
                            Some(goal) => {
                                goals::check_fresh(ctx, goal, Some(since)).await == Some(true)
                            }
                            None => true,
                        };
                        if held {
                            let kept = kept_commands(&c.input, &ok_cmds);
                            let summary = match work.goal {
                                Some(_) => {
                                    if let Some(cause) = c.input["cause"].as_str() {
                                        work.note(ctx, format!("coder: {cause}"));
                                    }
                                    None
                                }
                                None => c.input["summary"]
                                    .as_str()
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty()),
                            };
                            end = Some(End::Pass(kept, summary));
                            String::new()
                        } else {
                            checks += 1;
                            if checks >= MAX_CHECKS {
                                end = Some(End::Next);
                            }
                            let goal = work.goal.expect("a task with no goal is held");
                            let now = goals::known(ctx, Some(goal)).await;
                            let reads = ctx.w.reads(goal);
                            let shown: Vec<String> = now
                                .iter()
                                .filter(|(k, _)| reads.contains(*k))
                                .map(|(k, v)| format!("{k}: {}", v.text))
                                .collect();
                            format!(
                                "The goal still does not hold. Facts now:\n{}",
                                shown.join("\n")
                            )
                        }
                    }
                    "ask" => {
                        let q = c.input["question"]
                            .as_str()
                            .unwrap_or("the coder needs the person")
                            .to_string();
                        let shown = observe::shown(&values);
                        ctx.ask(AskReq {
                            goal: work.goal_name(),
                            action: Some(work.action.to_string()),
                            rule: None,
                            signed: false,
                            card: card::coder_asks(ctx, work.goal, work.action, task, &q, &shown),
                            facts: shown,
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
            if commands >= judged + JUDGE_EVERY {
                judged = commands - commands % JUDGE_EVERY;
                if judge(ctx, work, task, start, &tried, commands).await {
                    break End::Asked;
                }
            }
            msgs.push(Msg::Results(results));
            mask(&mut msgs, &files);
        };
        ctx.journal().add(Event {
            cost_usd: Some(tally.cost_usd),
            usage: Some(tally.usage(coder.model(i).to_string())),
            ms: ms(started),
            ..Kind::Note {
                goal: work.goal_name(),
                action: Some(work.action.to_string()),
                n: None,
                text: format!(
                    "coder {}: {}, {commands} commands so far",
                    coder.model(i),
                    tally.describe(system.chars().count())
                ),
            }
            .into()
        });
        match end {
            End::Pass(commands, summary) => return DoResult::Pass { commands, summary },
            End::Asked => return DoResult::Asked,
            End::Failed(why) => return DoResult::Failed(format!("{}: {why}", work.action)),
            End::Next => {}
        }
    }
    DoResult::Failed(format!(
        "{}: every coder model failed the task",
        work.action
    ))
}

/// The text of a conversation's messages, in bytes.
fn size(msgs: &[Msg]) -> usize {
    msgs.iter()
        .map(|m| match m {
            Msg::User(t) => t.len(),
            Msg::Assistant { text, calls } => {
                text.len()
                    + calls
                        .iter()
                        .map(|c| c.name.len() + c.input.to_string().len())
                        .sum::<usize>()
            }
            Msg::Results(rs) => rs.iter().map(|(_, r)| r.len()).sum(),
        })
        .sum()
}

/// Past `MASK_AT`, every tool result but the newest `MASK_KEEP` becomes its first line and
/// the file of its output. All at once: the provider's cached prefix breaks only once each
/// time the conversation grows past the budget again.
fn mask(msgs: &mut [Msg], files: &HashMap<String, PathBuf>) {
    if size(msgs) <= MASK_AT {
        return;
    }
    let results = msgs.iter_mut().rev().filter_map(|m| match m {
        Msg::Results(rs) => Some(rs),
        _ => None,
    });
    for (id, content) in results.flat_map(|rs| rs.iter_mut().rev()).skip(MASK_KEEP) {
        let first = content.lines().next().unwrap_or_default();
        let stub = match files.get(id.as_str()) {
            Some(f) => format!("{first} [output hidden; it is in {}]", f.display()),
            None => format!("{first} [hidden]"),
        };
        if stub.len() < content.len() {
            *content = stub;
        }
    }
}

/// At most `max` bytes of the start of `text`.
fn head(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut at = max;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    format!("{}…", &text[..at])
}

/// One decider request on the last `JUDGE_EVERY` commands: does the coder get closer to
/// the end of its task? When not, the session ends with an ask to the person, and this is
/// true. A failed request lets the session go on.
async fn judge(
    ctx: &Ctx,
    work: Work<'_>,
    task: &str,
    start: &BTreeMap<String, String>,
    tried: &[Ran],
    commands: usize,
) -> bool {
    let Some(decider) = &ctx.decider else {
        return false;
    };
    let short = |facts: &BTreeMap<String, String>| -> BTreeMap<String, String> {
        facts.iter().map(|(k, v)| (k.clone(), head(v, 300))).collect()
    };
    let now = observe::shown(&goals::known(ctx, work.goal).await);
    let from = tried.len().saturating_sub(JUDGE_EVERY);
    let last: Vec<Value> = tried[from..]
        .iter()
        .enumerate()
        .map(|(k, r)| {
            let first = r.result.lines().next().unwrap_or_default();
            let exit = first
                .strip_prefix("exit ")
                .and_then(|e| e.split(';').next())
                .and_then(|e| e.parse::<i64>().ok())
                .map_or_else(|| json!(first), |c| json!(c));
            json!({
                "n": from + k + 1,
                "command": head(&ctx.redact(&r.command), 300),
                "purpose": head(&ctx.redact(&r.purpose), 300),
                "exit": exit,
            })
        })
        .collect();
    let mut state = json!({
        "task": task,
        "facts_at_start": short(start),
        "facts_now": short(&now),
        "commands_run": commands,
        "last_commands": last,
    });
    if let Some(goal) = work.goal {
        state["goal"] = json!(goals::describe(ctx, goal));
    }
    let criteria: IndexMap<String, String> = PROGRESS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let questions = IndexMap::from([(
        "progress".to_string(),
        Question::Choice {
            criteria,
            instructions:
                "The coder ran `last_commands` for its `task`. Which describes these commands best?"
                    .into(),
        },
    )]);
    let started = Instant::now();
    let a = match decider.ask(state, questions).await {
        Ok(a) => a,
        Err(e) => {
            work.note(ctx, format!("decider: {e:#}"));
            return false;
        }
    };
    let Some((choice, probs, _)) = a.choice("progress") else {
        return false;
    };
    let closer = probs.get("closer").copied().unwrap_or(0.0);
    ctx.journal().add(Event {
        cost_usd: Some(a.cost_usd),
        usage: Some(decider.usage(&a)),
        ms: ms(started),
        ..Kind::Decide {
            goal: work.goal_name(),
            action: Some(work.action.to_string()),
            text: format!("progress after {commands} commands: {choice}"),
            p: probs.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        }
        .into()
    });
    if closer >= 0.5 {
        return false;
    }
    let why = PROGRESS
        .iter()
        .find(|(k, _)| *k == choice)
        .map_or(choice, |(_, v)| *v);
    let lines: Vec<String> = tried[from..]
        .iter()
        .map(|r| {
            let what = if r.purpose.is_empty() {
                head(&r.command, 120)
            } else {
                r.purpose.clone()
            };
            let first = r.result.lines().next().unwrap_or_default();
            ctx.redact(&format!("{what} ({first})"))
        })
        .collect();
    let mut tried_lines = vec![format!(
        "After {commands} commands the coder does not get closer: {}.",
        why.trim_end_matches('.')
    )];
    if !lines.is_empty() {
        tried_lines.push("Its last commands:".into());
        tried_lines.extend(lines);
    }
    ctx.ask(AskReq {
        goal: work.goal_name(),
        action: Some(work.action.to_string()),
        rule: None,
        signed: false,
        card: card::coder_stuck(ctx, work.goal, work.action, task, tried_lines, &now),
        facts: now,
    });
    true
}

/// The commands `done` names, each as it ran: the coder often names a part of a chained
/// command, or a command with a check added.
fn kept_commands(input: &Value, ok_cmds: &[String]) -> Vec<String> {
    let named: Vec<String> = input["commands"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .collect()
        })
        .unwrap_or_default();
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
    kept
}

/// One `shell` call: what the coder sent, what it read back, and the file that keeps the
/// whole output of a command that ran.
struct Ran {
    command: String,
    purpose: String,
    result: String,
    file: Option<PathBuf>,
}

/// One coder command: gated by the quoted rules, run, journaled. `Err(())` when an ask
/// was raised and the session ends.
async fn shell(
    ctx: &Ctx,
    work: Work<'_>,
    input: &Value,
    rules_ok: &[String],
    ok_cmds: &mut Vec<String>,
) -> Result<Ran, ()> {
    let command = input["command"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    let purpose = input["purpose"].as_str().unwrap_or_default().to_string();
    let (result, file) = if command.is_empty() {
        ("shell needs a command.".to_string(), None)
    } else {
        gate_and_run(ctx, work, &command, &purpose, rules_ok, ok_cmds).await?
    };
    Ok(Ran {
        command,
        purpose,
        result,
        file,
    })
}

/// What the coder reads of a command, and the file of its whole output.
async fn gate_and_run(
    ctx: &Ctx,
    work: Work<'_>,
    command: &str,
    purpose: &str,
    rules_ok: &[String],
    ok_cmds: &mut Vec<String>,
) -> Result<(String, Option<PathBuf>), ()> {
    let access = Rule::Never {
        text: BOX_ACCESS.into(),
        line: 0,
    };
    let quoted: Vec<(&Rule, bool, String)> = ctx
        .w
        .rules
        .iter()
        .chain(std::iter::once(&access))
        .filter_map(|r| match r {
            Rule::Never { text, .. } => Some((r, true, weave::subst(text, &ctx.spec.args))),
            Rule::Ask { text, .. } => Some((r, false, weave::subst(text, &ctx.spec.args))),
            _ => None,
        })
        .collect();
    let Some(decider) = &ctx.decider else {
        return Ok(("Refused: no decider to check the rules.".into(), None));
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
    let state = json!({"command": ctx.redact(command), "purpose": ctx.redact(purpose)});
    let answers = match decider.ask(state, questions).await {
        Ok(x) => x,
        Err(e) => {
            work.note(ctx, format!("decider: {e:#}"));
            return Ok(("Refused: the rule check failed; try again.".into(), None));
        }
    };
    ctx.journal().spend(answers.cost_usd);
    for (i, (r, never, _)) in quoted.iter().enumerate() {
        let p = answers.noul(&format!("rule{i}")).unwrap_or(1.0);
        if p < 0.5 {
            continue;
        }
        let rule = r.text();
        let kept = ctx.journal().state().kept(&rule);
        let gate = |ok: bool| Kind::Gate {
            goal: work.goal_name(),
            action: work.action.to_string(),
            text: format!("{rule}: {}", ctx.redact(command)),
            ok,
            p: BTreeMap::from([("yes".to_string(), p)]),
            n: None,
        };
        if *never || kept == Some("never") {
            ctx.journal().add(gate(false));
            return Ok((
                format!("Refused by the rule {rule} ({p:.2}). Do it another way, or call ask."),
                None,
            ));
        }
        if kept == Some("always") {
            ctx.journal().add(gate(true));
            continue;
        }
        if rules_ok.iter().any(|ok| *ok == rule) {
            continue;
        }
        let values = goals::known(ctx, work.goal).await;
        ctx.ask(AskReq {
            goal: work.goal_name(),
            action: Some(work.action.to_string()),
            rule: Some(rule.clone()),
            signed: false,
            card: card::command(ctx, work.goal, work.action, command, purpose, &rule),
            facts: observe::shown(&values),
        });
        return Err(());
    }
    let start = Instant::now();
    let o = sh(command, &ctx.env, RUN_LIMIT).await;
    let took = ms(start);
    let all = ctx.redact(&o.both());
    let file = save_output(ctx, &all);
    let shown = cap_to(&all, OUTPUT_CAP, file.as_deref());
    ctx.journal().add(Event {
        ms: took,
        ..Kind::Command {
            goal: work.goal_name(),
            action: work.action.to_string(),
            text: command.to_string(),
            ok: o.ok(),
            output: command_output(ctx, &o),
        }
        .into()
    });
    if o.ok() {
        ok_cmds.push(command.to_string());
        return Ok((format!("exit 0\n{shown}"), file));
    }
    let code = o.code.map_or("killed".to_string(), |c| format!("exit {c}"));
    let kind = classify(ctx, command, &shown).await;
    let line = match kind {
        Some(k) => format!("{code}; the error looks like: {k}\n{shown}"),
        None => format!("{code}\n{shown}"),
    };
    Ok((line, file))
}

/// Keeps a coder command's whole output, redacted, as `output/<episode>-<n>.txt` with mode
/// 0600, and only the newest `OUTPUT_FILES` files of the service. `None` when it cannot.
fn save_output(ctx: &Ctx, text: &str) -> Option<PathBuf> {
    let dir = paths::output(&ctx.name);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .ok()?;
    let ep = super::episode::current()
        .unwrap_or_else(|| uuid::Uuid::now_v7().simple().to_string());
    let prefix = format!("{ep}-");
    let mut n = 0u32;
    let mut files: Vec<(SystemTime, PathBuf)> = Vec::new();
    for e in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if let Some(k) = name
            .strip_prefix(&prefix)
            .and_then(|r| r.strip_suffix(".txt"))
            .and_then(|k| k.parse::<u32>().ok())
        {
            n = n.max(k);
        }
        if let Ok(t) = e.metadata().and_then(|m| m.modified()) {
            files.push((t, e.path()));
        }
    }
    let path = dir.join(format!("{prefix}{}.txt", n + 1));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .ok()?;
    f.write_all(text.as_bytes()).ok()?;
    if files.len() >= OUTPUT_FILES {
        files.sort();
        for (_, old) in &files[..=files.len() - OUTPUT_FILES] {
            let _ = std::fs::remove_file(old);
        }
    }
    Some(path)
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
