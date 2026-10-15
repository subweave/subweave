//! Drills: a DRILL's BREAK fails its goal on purpose, and the normal loop must notice and
//! repair it, rules and escalations included. The loop journals `detected` and `fixed`
//! as its checks see them (`seen`); the drill's task runs BREAK, waits out FAILS WITHIN
//! and HOLDS WITHIN, runs RESTORE when the loop did not repair it, and escalates a drill
//! that failed: a weave defect.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::service::DRILL;
use crate::weave::{self, Drill, expr, show_duration};

use super::asks::AskReq;
use super::card;
use super::goals::{self, Scope};
use super::journal::Journal;
use super::observe;
use super::repair::command_output;
use super::service::Ctx;
use super::state::{DrillRun, DrillState, DrillStep, Event, Kind, ServiceState};
use super::{epoch, epoch_of, lock, sh};

/// A BREAK or a RESTORE runs at most this long.
const LIMIT: Duration = Duration::from_secs(10 * 60);
/// How often the drill reads the fold while it waits for the loop.
const POLL: Duration = Duration::from_secs(1);

/// How a drill ended.
enum End {
    Passed,
    /// BREAK failed: the drill tested nothing.
    Broken,
    /// The goal did not fail within FAILS WITHIN.
    Missed,
    /// The goal failed, and did not hold again within HOLDS WITHIN.
    Unfixed,
}

/// The drill that runs now and its label, as cards and messages name it.
pub(crate) fn running(ctx: &Ctx) -> Option<(String, String)> {
    let name = ctx.journal().state().drilling().map(|(n, _)| n.to_string())?;
    let label = match ctx.w.drill(&name) {
        Some(d) => label(ctx, d),
        None => name.clone(),
    };
    Some((name, label))
}

/// The drill's label with `$name` replaced.
pub(crate) fn label(ctx: &Ctx, d: &Drill) -> String {
    weave::subst(&d.label, &ctx.spec.args)
}

fn step(d: &Drill, step: DrillStep, ok: Option<bool>, text: Option<String>) -> Kind {
    ran(d, step, ok, text, None)
}

/// A step that ran a command: BREAK or RESTORE, with its output.
fn ran(d: &Drill, step: DrillStep, ok: Option<bool>, text: Option<String>, output: Option<String>) -> Kind {
    Kind::Drill {
        drill: d.name.clone(),
        goal: d.expect.clone(),
        step,
        ok,
        text,
        output,
    }
}

/// Seconds from `from` (RFC 3339) to now, as a duration.
fn since(from: Option<&str>) -> Option<Duration> {
    let t = epoch_of(from?)?;
    Some(Duration::from_secs((epoch() - t).max(0) as u64))
}

/// The step a check of `goal` gives the drill that runs, read from the fold the check
/// went into: the goal fails while the drill waits for that, or holds while it waits for
/// that. Journaled in the same lock as the read, so the drill's own timeout never races
/// it.
pub(crate) fn seen(st: &ServiceState, goal: &str) -> Option<Kind> {
    let (name, run) = st.drilling().filter(|(_, r)| r.goal == goal)?;
    let g = st.goal(goal)?;
    let step = if run.awaits_failure() && g.fails() {
        DrillStep::Detected
    } else if run.awaits_hold() && g.holds() {
        DrillStep::Fixed
    } else {
        return None;
    };
    let broke = match &run.state {
        DrillState::Running { broke, .. } => broke.as_deref(),
        DrillState::Ended { .. } => None,
    };
    Some(Kind::Drill {
        drill: name.to_string(),
        goal: goal.to_string(),
        step,
        ok: Some(true),
        text: since(broke).map(|d| format!("{} after BREAK", show_duration(d))),
        output: None,
    })
}

/// Why `d` may not start now, else `None`: a drill already runs, its goal does not hold,
/// or its IF is not true. Its IF's facts are read fresh.
pub(crate) async fn refused(ctx: &Ctx, d: &Drill) -> Option<String> {
    {
        let j = ctx.journal();
        let st = j.state();
        if let Some((other, _)) = st.drilling() {
            return Some(format!("drill {other} runs"));
        }
        if !st.goal(&d.expect).is_some_and(|g| g.holds()) {
            return Some(format!("{} does not hold now", d.expect));
        }
    }
    let e = d.when.as_ref()?;
    let names: BTreeSet<String> = e
        .names()
        .into_iter()
        .filter(|n| ctx.w.fact(n).is_some())
        .map(str::to_string)
        .collect();
    let span = goals::span_for(None, observe::read(ctx, &names, false).await);
    let scope = Scope { ctx, span };
    (expr::holds(e, &scope) != Some(true)).then(|| format!("its IF is not true: `{e}`"))
}

/// Runs drill `name` from BREAK to its end; `by` the person or agent who asked, none
/// when its EVERY started it.
pub(crate) async fn run(ctx: Arc<Ctx>, name: String, by: Option<String>) {
    let Some(d) = ctx.w.drill(&name).cloned() else {
        return;
    };
    let add = |k: Kind| {
        ctx.journal().add(Event {
            by: by.clone(),
            ..k.into()
        })
    };
    // Refused while another drill runs: the fold allows one at a time.
    if !add(step(&d, DrillStep::Started, None, Some(label(&ctx, &d)))) {
        return;
    }
    let o = sh(&d.breaks, &ctx.env, LIMIT).await;
    add(ran(&d, DrillStep::Broke, Some(o.ok()), Some(d.breaks.clone()), command_output(&ctx, &o)));
    let end = if !o.ok() {
        End::Broken
    } else {
        let broke = Instant::now();
        if !waited(&ctx, &d, broke + d.fails_within, DrillRun::awaits_failure, DrillStep::Detected, d.fails_within).await {
            End::Missed
        } else if !waited(&ctx, &d, broke + d.holds_within, DrillRun::awaits_hold, DrillStep::Fixed, d.holds_within).await {
            End::Unfixed
        } else {
            End::Passed
        }
    };
    let passed = matches!(end, End::Passed);
    // What BREAK did must not outlive the drill: the loop did not repair it.
    let restored = match d.restore.as_deref().filter(|_| !passed) {
        Some(r) => {
            let o = sh(r, &ctx.env, LIMIT).await;
            add(ran(&d, DrillStep::Restored, Some(o.ok()), Some(r.to_string()), command_output(&ctx, &o)));
            Some(o.ok())
        }
        None => None,
    };
    let label = label(&ctx, &d);
    let text = match end {
        End::Passed => {
            let (broke, detected) = match ctx.journal().state().drills.get(&d.name).map(|r| &r.state) {
                Some(DrillState::Running { broke, detected, .. }) => (broke.clone(), detected.clone()),
                _ => (None, None),
            };
            let noticed = match (epoch_of(broke.as_deref().unwrap_or_default()), detected.as_deref().and_then(epoch_of)) {
                (Some(b), Some(t)) => show_duration(Duration::from_secs((t - b).max(0) as u64)),
                _ => "?".into(),
            };
            let held = since(broke.as_deref()).map_or_else(|| "?".into(), show_duration);
            format!("passed: {} noticed in {noticed} and held again in {held}", d.expect)
        }
        End::Broken => format!("BREAK failed: exit {}", o.code.map_or("killed".into(), |c| c.to_string())),
        End::Missed => format!("{} did not notice {label}", d.expect),
        End::Unfixed => format!("{} did not recover from {label}", d.expect),
    };
    add(step(&d, DrillStep::Ended, Some(passed), Some(text)));
    if matches!(end, End::Missed | End::Unfixed) {
        escalate(&ctx, &d, matches!(end, End::Missed), restored).await;
    }
}

/// Waits until the loop journals what the drill awaits, or `until`; then journals the
/// step as not seen, in the same lock as the read. Whether the loop saw it.
async fn waited(
    ctx: &Ctx,
    d: &Drill,
    until: Instant,
    awaits: fn(&DrillRun) -> bool,
    s: DrillStep,
    within: Duration,
) -> bool {
    loop {
        {
            let mut j = ctx.journal();
            if !j.state().drills.get(&d.name).is_some_and(awaits) {
                return true;
            }
            if Instant::now() >= until {
                let text = format!("not within {}", show_duration(within));
                j.add(step(d, s, Some(false), Some(text)));
                return false;
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

/// A drill that failed is a weave defect: its goal's check misses the failure, or its
/// actions do not repair it. A yes runs the drill again.
async fn escalate(ctx: &Ctx, d: &Drill, missed: bool, restored: Option<bool>) {
    let values = goals::known(ctx, ctx.w.goal(&d.expect)).await;
    let reads = ctx.w.goal(&d.expect).map(|g| ctx.w.reads(g)).unwrap_or_default();
    let shown: BTreeMap<String, String> = observe::shown(&values)
        .into_iter()
        .filter(|(n, _)| reads.contains(n))
        .collect();
    ctx.ask(AskReq {
        goal: None,
        action: Some(DRILL.to_string()),
        rule: None,
        signed: false,
        drill: Some(d.name.clone()),
        card: card::drill_failed(ctx, d, missed, restored, &shown),
        facts: shown,
    });
}

/// A drill its loop stopped while it ran (its end is journaled already): its RESTORE
/// runs anyway, in the background, so what BREAK did does not outlive it. `d`: the drill
/// as the weave has it now, if it still does.
pub(crate) fn stopped(journal: Arc<Mutex<Journal>>, env: BTreeMap<String, String>, d: Drill) {
    let Some(r) = d.restore.clone() else {
        return;
    };
    tokio::spawn(async move {
        let o = sh(&r, &env, LIMIT).await;
        let output = Some(super::last(&o.both(), 4 * 1024).to_string()).filter(|s| !s.is_empty());
        lock(&journal).add(ran(&d, DrillStep::Restored, Some(o.ok()), Some(r), output));
    });
}
