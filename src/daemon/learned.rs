//! Learned commands: a quoted FACT's command, written once by the coder and then run as
//! code; and the steps that made a goal hold, kept under `<action>/<goal>`.

use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use indexmap::IndexMap;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::model::coder::{Msg, Tally, Tool};
use crate::model::decider::Question;
use crate::paths;
use crate::service::{self as svc, Learned, LearnedFact};
use crate::weave::{self, Action, Fact, FactSource, FactType, Step, Weave};

use super::service::Ctx;
use super::state::{Event, JobKind, Kind};
use super::{ms, now, save_json, sh};

/// A note on a learn: no goal, no action.
fn note(text: String) -> Event {
    Kind::Note {
        goal: None,
        action: None,
        n: None,
        text,
    }
    .into()
}

const TRIES: usize = 3;
/// After a quoted fact could not be learned, the next try waits this long.
const RELEARN: Duration = Duration::from_secs(600);
/// A LINES command still running after this long follows; one that ended does not.
const FOLLOW_PROBE: Duration = Duration::from_secs(5);
/// The coder's reasoning effort for a learn. On the 14 number and text facts of
/// weaves/postgres, twice each on the test box: `low` passed 21 of 28 as the default did,
/// with 38k output tokens against 78k.
const LEARN_EFFORT: &str = "low";
const FACT_SYSTEM: &str = "You write one bash command that reads a fact on this box. The \
    command must only read: never install, write, delete, restart or refresh a cache. It runs \
    as `bash -eo pipefail -c <command>` with a 60 second limit, and the ARGs are environment \
    variables. The service may be down or not installed yet: test that first, on a unit, \
    container or process the fact, the ARGs or the skills name, and print the fact's value for \
    that case (`systemctl is-active -q <unit> || { echo 0; exit 0; }`). `Service` below is \
    Subweave's label for this weave, not a unit name. Then read the fact with no fallback: \
    when the service runs and the read fails, the command must fail too, so a wrong query \
    never reads as a healthy value. Answer by calling the tool `fact`.";

pub(crate) fn key(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// The key a quoted fact's learned command is kept under: its type and its text after
/// `$name` replacement. A changed type, text or ARG learns again.
fn fact_key(fact: &Fact, args: &std::collections::BTreeMap<String, String>) -> Option<String> {
    let FactSource::Quoted(text) = &fact.source else {
        return None;
    };
    Some(key(&format!("{}\n{}", fact.ty, weave::subst(text, args))))
}

/// Drops each learned fact command the weave no longer reads as learned: its fact left
/// the weave, or its type or text changed. Whether any was dropped.
pub(crate) fn prune(
    l: &mut Learned,
    w: &Weave,
    args: &std::collections::BTreeMap<String, String>,
) -> bool {
    let before = l.facts.len();
    l.facts.retain(|name, learned| {
        w.fact(name)
            .and_then(|f| fact_key(f, args))
            .is_some_and(|k| k == learned.key)
    });
    l.facts.len() != before
}

/// The key of an action's learned steps: its DO texts after `$name` replacement.
pub(crate) fn steps_key(ctx: &Ctx, a: &Action) -> String {
    let texts: Vec<String> = a
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Do { task, .. } => Some(weave::subst(task, &ctx.spec.args)),
            _ => None,
        })
        .collect();
    key(&texts.join("\n"))
}

pub(crate) fn save(service: &str, l: &Learned) {
    if let Err(e) = save_json(&paths::learned(service), l) {
        tracing::error!(service, error = %format!("{e:#}"), "cannot save learned.json");
    }
}

/// Drops a learned fact, an action's steps for every goal, or one `<action>/<goal>`.
pub(crate) fn forget(l: &mut Learned, name: &str) -> bool {
    let before = l.facts.len() + l.steps.len();
    l.facts.remove(name);
    let prefix = format!("{name}/");
    l.steps.retain(|k, _| k != name && !k.starts_with(&prefix));
    l.facts.len() + l.steps.len() != before
}

pub(crate) fn forget_file(service: &str, name: &str) -> Result<bool> {
    let mut l = svc::learned(service)?;
    let dropped = forget(&mut l, name);
    if dropped {
        save_json(&paths::learned(service), &l)?;
    }
    Ok(dropped)
}

/// The learned command of a quoted fact while its key holds.
pub(crate) fn learned_command(ctx: &Ctx, fact: &Fact) -> Option<String> {
    let k = fact_key(fact, &ctx.spec.args)?;
    ctx.learned()
        .facts
        .get(&fact.name)
        .filter(|l| l.key == k)
        .map(|l| l.command.clone())
}

/// The command for a quoted fact: the learned one while its key holds, else a new one
/// from the coder. `None` when none is learned and none can be now.
pub(crate) async fn fact_command(ctx: &Ctx, fact: &Fact) -> Option<String> {
    if !matches!(fact.source, FactSource::Quoted(_)) {
        return None;
    }
    if let Some(c) = learned_command(ctx, fact) {
        return Some(c);
    }
    let _one = ctx.learning.lock().await;
    if let Some(c) = learned_command(ctx, fact) {
        return Some(c);
    }
    super::episode::scope(JobKind::Learn, learn_and_keep(ctx, fact, None)).await
}

/// A learned command whose read broke on this box (`why`): forgotten, and learned again
/// with its error, at most once per `RELEARN` for the same fact.
pub(crate) async fn relearn(
    ctx: &Ctx,
    fact: &Fact,
    command: &str,
    failed: &super::Out,
    why: &str,
) -> Option<String> {
    let _one = ctx.learning.lock().await;
    let now_learned = ctx.learned().facts.get(&fact.name).cloned();
    let Some(was) = now_learned.filter(|l| l.command == command) else {
        // Another check learned it again meanwhile.
        return learned_command(ctx, fact);
    };
    let code = why;
    let output = ctx.redact(&super::cap(&failed.both(), 1000));
    ctx.journal().add(Kind::Learn {
        goal: None,
        action: None,
        ok: false,
        text: format!("fact {}: `{command}` failed on this box ({code})", fact.name),
        output: Some(output.clone()),
    });
    {
        let mut l = ctx.learned();
        l.facts.remove(&fact.name);
        save(&ctx.name, &l);
    }
    let age = super::epoch() - super::epoch_of(&was.t).unwrap_or(0);
    if age < RELEARN.as_secs() as i64 {
        let wait = RELEARN - Duration::from_secs(age.max(0) as u64);
        ctx.learn_wait
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fact.name.clone(), Instant::now() + wait);
        return None;
    }
    let hint = format!(
        "The last command for this fact failed on this box ({code}):\n{command}\nIt printed:\n{output}\nWrite one that works here."
    );
    super::episode::scope(JobKind::Learn, learn_and_keep(ctx, fact, Some(&hint))).await
}

/// Learns a quoted fact's command and keeps it. The caller holds `ctx.learning`.
async fn learn_and_keep(ctx: &Ctx, fact: &Fact, hint: Option<&str>) -> Option<String> {
    let FactSource::Quoted(text) = &fact.source else {
        return None;
    };
    let text = weave::subst(text, &ctx.spec.args);
    let k = fact_key(fact, &ctx.spec.args)?;
    if ctx
        .learn_wait
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&fact.name)
        .is_some_and(|at| *at > Instant::now())
    {
        return None;
    }
    let mut cost = 0.0;
    let mut tally = Tally::default();
    let start = Instant::now();
    let result = learn(ctx, fact, &text, hint, &mut cost, &mut tally).await;
    if tally.calls > 0 {
        let model = ctx
            .coder
            .as_ref()
            .map(|c| c.model(0).to_string())
            .unwrap_or_default();
        let text = format!(
            "coder {model} for fact {}: {}",
            fact.name,
            tally.describe(FACT_SYSTEM.chars().count())
        );
        ctx.journal().add(Event {
            cost_usd: Some(tally.cost_usd),
            usage: Some(tally.usage(model)),
            ms: ms(start),
            ..note(text)
        });
        cost -= tally.cost_usd;
    }
    match result {
        Ok(command) => {
            {
                let mut l = ctx.learned();
                l.facts.insert(
                    fact.name.clone(),
                    LearnedFact {
                        command: command.clone(),
                        key: k,
                        t: now(),
                    },
                );
                save(&ctx.name, &l);
            }
            ctx.journal().add(Event {
                cost_usd: Some(cost),
                ms: ms(start),
                ..Kind::Learn {
                    goal: None,
                    action: None,
                    ok: true,
                    text: format!("fact {}: {command}", fact.name),
                    output: None,
                }
                .into()
            });
            ctx.message(
                crate::channel::Kind::Info,
                None,
                format!("learned fact {}", fact.name),
                format!("This box reads `{}` with: {command}", fact.name),
                Default::default(),
            );
            Some(command)
        }
        Err(e) => {
            ctx.journal().add(Event {
                cost_usd: (cost > 0.0).then_some(cost),
                ms: ms(start),
                ..note(format!("cannot learn fact {}: {e:#}", fact.name))
            });
            ctx.learn_wait
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(fact.name.clone(), Instant::now() + RELEARN);
            None
        }
    }
}

/// `hint`: why the last learned command for this fact was dropped, for the coder.
async fn learn(
    ctx: &Ctx,
    fact: &Fact,
    text: &str,
    hint: Option<&str>,
    cost: &mut f64,
    tally: &mut Tally,
) -> Result<String> {
    let coder = ctx.coder.as_ref().ok_or_else(|| anyhow!("no coder"))?;
    let decider = ctx
        .decider
        .as_ref()
        .ok_or_else(|| anyhow!("no decider to check the command"))?;
    let reads = match fact.ty {
        FactType::Lines => {
            "lines: it keeps running and prints new lines as they come, never old ones \
             (`tail -n 0 -F`, `journalctl -f -n 0`)"
                .to_string()
        }
        FactType::Bool => "its exit status: exit 0 when it holds, non-zero when not".into(),
        FactType::Number(Some(u)) => format!("a number in {u}: print one number and nothing else"),
        FactType::Number(None) => "a number: print one number and nothing else".into(),
        FactType::Version => "a version like 1.2.3: print it and nothing else".into(),
        FactType::Text => "the text it prints".into(),
    };
    // What every learn of this service shares comes first, so the provider's prompt
    // cache can hold it; only the skills the decider picks for this fact go along.
    let mut user = format!("Box: {}\nService: {}", ctx.box_line(), ctx.name);
    if !ctx.spec.args.is_empty() {
        user.push_str("\nARGs:");
        for (k, v) in &ctx.spec.args {
            user.push_str(&format!("\n  {k}={v}"));
        }
    }
    for s in super::coder_task::pick_skills(ctx, None, text, None).await {
        user.push_str(&format!(
            "\n\nSkill {} (when {}):\n{}",
            s.name,
            s.when,
            weave::subst(&s.body, &ctx.spec.args)
        ));
    }
    user.push_str(&format!(
        "\n\nFact `{}`: {text}\nThe weave reads it as {reads}.",
        fact.name
    ));
    if let Some(h) = hint {
        user.push_str(&format!("\n\n{h}"));
    }
    let tools = [Tool {
        name: "fact",
        description: "The one read-only bash command that prints the fact.",
        parameters: json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"],
        }),
    }];
    let mut msgs = vec![Msg::User(user)];
    let mut last = String::new();
    let sid = super::coder_task::session_id(0);
    for _ in 0..TRIES {
        let reply = coder
            .send(0, sid.as_deref(), Some(LEARN_EFFORT), FACT_SYSTEM, &msgs, &tools)
            .await?;
        *cost += reply.cost_usd;
        tally.add(&reply);
        let call = reply.calls.iter().find(|c| c.name == "fact").cloned();
        msgs.push(Msg::Assistant {
            text: reply.text,
            calls: reply.calls.clone(),
        });
        let Some(call) = call else {
            msgs.push(Msg::User("Call the tool `fact` with the command.".into()));
            last = "no command".into();
            continue;
        };
        let command = call.input["command"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let answer = |m: &mut Vec<Msg>, s: String| {
            let mut rs: Vec<(String, String)> = reply
                .calls
                .iter()
                .map(|c| (c.id.clone(), "ignored".to_string()))
                .collect();
            if let Some(r) = rs.iter_mut().find(|(id, _)| *id == call.id) {
                r.1 = s;
            }
            m.push(Msg::Results(rs));
        };
        if command.is_empty() {
            answer(&mut msgs, "The command is empty.".into());
            last = "an empty command".into();
            continue;
        }
        // "hides" catches a read that prints a fallback when it fails: a query of a column
        // that does not exist read 0 every five minutes as "no idle transaction". On 16
        // commands it sorted 15 (#111-#126); the miss refused a `|| true` on a dpkg read.
        let mut q = IndexMap::new();
        q.insert(
            "changes".to_string(),
            Question::Noul {
                instructions: "Does this command change anything on the box: install, write, \
                    delete, restart, or refresh a cache?"
                    .into(),
            },
        );
        if !matches!(fact.ty, FactType::Bool | FactType::Lines) {
            q.insert(
                "hides".to_string(),
                Question::Noul {
                    instructions: "Does this command swallow the errors of its main read with \
                        `|| echo`, `2>/dev/null`, `|| R=0` or a similar fallback that prints a \
                        value, rather than only guarding for the service being down or not \
                        installed? A guard on a unit, container or process that neither `fact` \
                        nor `args` names is such a fallback: it can print the value forever."
                        .into(),
                },
            );
        }
        let a = decider
            .ask(
                json!({"fact": text, "args": ctx.spec.args, "command": ctx.redact(&command)}),
                q,
            )
            .await?;
        *cost += a.cost_usd;
        let p = a.noul("changes").unwrap_or(1.0);
        if p >= 0.5 {
            answer(
                &mut msgs,
                format!(
                    "Refused: that command changes the box ({p:.2}). Write one that only reads."
                ),
            );
            last = format!("`{command}` changes the box ({p:.2})");
            continue;
        }
        let p = a.noul("hides").unwrap_or(0.0);
        if p >= 0.5 {
            answer(
                &mut msgs,
                format!(
                    "Refused: when the read fails, that command still prints a value ({p:.2}). \
                     Test first that the service is installed and running, print the value for \
                     that case, then read with no fallback."
                ),
            );
            last = format!("`{command}` prints a value when its read fails ({p:.2})");
            continue;
        }
        if fact.ty == FactType::Lines {
            let o = sh(&command, &ctx.env, FOLLOW_PROBE).await;
            if let Some(c) = o.code {
                let shown = ctx.redact(&super::cap(&o.both(), 2000));
                answer(
                    &mut msgs,
                    format!(
                        "It ended with exit {c} within {}s and printed this:\n{shown}\nA LINES \
                         command keeps running and prints new lines as they come.",
                        FOLLOW_PROBE.as_secs()
                    ),
                );
                last = format!("`{command}` ended with exit {c}: it does not follow");
                continue;
            }
        } else if fact.ty != FactType::Bool {
            let o = sh(&command, &ctx.env, Duration::from_secs(60)).await;
            let printed = o.stdout.trim();
            let parsed = if o.ok() {
                fact.ty.parse(printed)
            } else {
                Err("it exited non-zero".to_string())
            };
            if let Err(why) = parsed {
                let code = o.code.map_or("killed".to_string(), |c| format!("exit {c}"));
                let shown = ctx.redact(&super::cap(&o.both(), 2000));
                answer(
                    &mut msgs,
                    format!(
                        "It ended with {code} and printed this:\n{shown}\nThat is no {} value: \
                         {why}. The weave reads it as {reads}.",
                        fact.ty
                    ),
                );
                last = format!("`{command}` printed no {} value ({code})", fact.ty);
                continue;
            }
            // A command can read the right thing in the wrong form: an HTTP status for a
            // 0/1 flag, bytes for GB. Measured on 14 cases, all sorted at 0.5.
            let mut q = IndexMap::new();
            q.insert(
                "fits".to_string(),
                Question::Noul {
                    instructions: "Is `printed` a value that `fact` can have, in the form \
                        `fact` describes? Judge the form and the scale, not whether the box \
                        is healthy."
                        .into(),
                },
            );
            let shown = ctx.redact(&super::cap(printed, 500));
            let a = decider
                .ask(json!({"fact": text, "printed": shown}), q)
                .await?;
            *cost += a.cost_usd;
            let p = a.noul("fits").unwrap_or(0.0);
            if p < 0.5 {
                answer(
                    &mut msgs,
                    format!(
                        "It printed `{shown}`, which is not a value of the fact in the form it \
                         describes ({p:.2}). Print exactly that form."
                    ),
                );
                last = format!("`{command}` printed `{shown}`, not the fact's form ({p:.2})");
                continue;
            }
        }
        return Ok(command);
    }
    bail!("{TRIES} tries, the last {last}")
}
