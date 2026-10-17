//! A service's state: the events its journal holds and the fold over them. `apply` is the
//! one place state changes. The daemon applies each event as it journals it and replays
//! the journal on start; every reader folds the same events.

use std::collections::{BTreeMap, BTreeSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::channel;
use crate::model::Usage;
use crate::service::{DRILL, TASK, WEAVE};
use crate::weave::Class;

use super::epoch_of;
use super::memory::{Memory, Told, ToldRelease};

/// One journal line: when, what happened, who caused it, the episode it belongs to, and
/// what its step cost and took.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// RFC 3339, UTC.
    pub t: String,
    #[serde(flatten)]
    pub kind: Kind,
    /// Who acted, on events a person or an agent causes: `cli:<user>`, `dash@<ip>`, a
    /// channel's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The episode the event belongs to: one job run (a repair or a person's task) or one
    /// learn outside a job. Events outside every episode have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ep: Option<String>,
    /// On an episode's first event: the job it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<JobKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// What a model conversation used, on the event that ends it: a coder's note, a
    /// decider's `decide`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Wall time in milliseconds of the step the event ends: a command, a check's fact
    /// reads, a decision, a learn, a coder conversation, an action, a task's run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

impl From<Kind> for Event {
    fn from(kind: Kind) -> Event {
        Event {
            t: String::new(),
            kind,
            by: None,
            ep: None,
            job: None,
            cost_usd: None,
            usage: None,
            ms: None,
        }
    }
}

/// One line a LINES fact printed, at `t` (Unix milliseconds).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub t: i64,
    pub text: String,
}

/// What happened. A reader shows `text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Kind {
    /// The goals of the weave the loop runs, in graph order.
    Goals { goals: Vec<String> },
    /// A goal checked: `ok` holds or not, none unknown; `facts` the values it read (not
    /// LINES); `lines` per LINES fact the lines new since the fact's last check event;
    /// `p` how sure the decider was that a quoted goal holds. The facts' rings are
    /// rebuilt from these on start.
    Check {
        goal: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<bool>,
        facts: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        lines: BTreeMap<String, Vec<Line>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        p: Option<f64>,
    },
    /// The model judged one line of a quoted goal's window: `p` that lines of its kind
    /// show trouble the goal must act on.
    Verdict {
        goal: String,
        text: String,
        p: f64,
    },
    /// A person or an agent (`by`) said lines of `text`'s kind are `class` for `goal`;
    /// none: the box forgets what it knew of them.
    Known {
        goal: String,
        text: String,
        class: Option<Class>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// Whether release `text` of PART `part` fixes a security issue or is high priority:
    /// the model's verdict (`p`, no `by`), or a person's or an agent's word (`by`).
    Release {
        part: String,
        #[serde(rename = "text")]
        version: String,
        security: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        p: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// The loop leaves a failing goal alone `until` (RFC 3339), across a restart too: the
    /// decider chose to wait, or a fix held and its window has yet to pass.
    Wait { goal: String, until: String },
    /// A goal does not apply: its IF is false, or a goal it comes AFTER is off.
    Off {
        goal: String,
        #[serde(rename = "text")]
        why: String,
    },
    /// A goal waits for `on`, a goal it comes AFTER that does not hold: one of this
    /// weave's, or `<service>:<goal>` of another service.
    Waiting { goal: String, on: String },
    /// A goal cannot be judged: another service it reads or comes AFTER does not run.
    Unknown {
        goal: String,
        #[serde(rename = "text")]
        why: String,
    },
    /// The decider chose: `text` the choice, `p` the probabilities.
    Decide {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        p: BTreeMap<String, f64>,
    },
    /// A rule refused an action or a command, held it for escalation `n`, or (`ok`) let
    /// it pass on the person's `always`: `text` the rule, and the command after a colon;
    /// `p` how sure the decider was that a quoted rule covers the command.
    Gate {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        action: String,
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        ok: bool,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        p: BTreeMap<String, f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        n: Option<u64>,
    },
    /// An action or a person's task starts or ends.
    Action(Act),
    /// One shell command or ADD: `text` the command, `ok` exit 0.
    Command {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        action: String,
        text: String,
        ok: bool,
        /// stdout and stderr, secrets redacted, the last 4 KB.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// A fact command or steps learned, or (not `ok`) a learned command that failed.
    Learn {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        ok: bool,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// An escalation raised with the card everyone reads; `rule` the rule that holds its
    /// action, as written.
    Ask {
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rule: Option<String>,
        card: channel::Card,
        /// Only the person's signed answer on a Steery card settles it.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        signed: bool,
        /// The window's lines of the kinds not known normal: an answer may teach them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lines: Vec<String>,
        /// The drill that failed, on a drill's escalation: a yes runs it again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drill: Option<String>,
    },
    /// The summary model's lines for escalation `n`'s card; none when the card goes
    /// without them, and `text` says why.
    Summary {
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        lines: Vec<String>,
        text: String,
    },
    /// Escalation `n` answered with the word `text`; `action` the action the word names,
    /// or `task` and `weave` for those escalations. `signed`: a signature from a pinned
    /// signer on Steery carries it.
    Answer {
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        text: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        signed: bool,
    },
    /// Escalation `n` closed with no answer: nobody needs to answer it now.
    Withdraw {
        n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(rename = "text")]
        why: String,
    },
    /// Escalation `n` went to `channel`; `handle` is what the channel reads its answer by.
    Sent {
        n: u64,
        channel: String,
        handle: channel::Sent,
    },
    /// `channel` watches `handle`, sent with escalation `n`, for the service's own sake
    /// whatever becomes of the escalation: a Steery connect.
    Watch {
        n: u64,
        channel: String,
        handle: channel::Sent,
    },
    /// `channel` has nothing more to read for escalation `n`.
    Unwatch { n: u64, channel: String },
    /// An event the state did not allow, kept with why.
    Refused {
        #[serde(rename = "text")]
        why: String,
        refused: Box<Kind>,
    },
    /// A message sent to the channels: `text` its kind and title.
    Message {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        text: String,
    },
    /// A step of drill `drill`, which breaks the service to prove `goal` fails and holds
    /// again: `ok` how the step went, `text` what it ran or how long it took, `output` a
    /// command's.
    Drill {
        drill: String,
        goal: String,
        step: DrillStep,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// `n`: the escalation a card summary or an agent's run is for.
    Note {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        n: Option<u64>,
        text: String,
    },
}

/// An action's or a person's task's start or end; an end has `ok`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Act {
    /// `ok`: the goal held within the action's WITHIN, or the task is done. A task's
    /// `text` is the coder's summary or why it ended.
    Ended {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        action: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
    /// An action cut off before its WITHIN said whether it worked, because its loop
    /// `stopped`: no outcome, so no try.
    Stopped {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        action: String,
        stopped: String,
    },
    /// A task's `text` is the person's words; `within` the seconds an action's goal has
    /// to hold.
    Started {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        goal: Option<String>,
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        within: Option<u64>,
        /// Lines of the known trouble kinds the goal fails on: a pass is remembered for
        /// them.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        on: Vec<String>,
        /// The action's UNDO, which this run arms: it runs when the goal does not hold
        /// within `within`, or a goal that held before fails then. None for an undo.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undo: Option<String>,
        /// This run undoes that action.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        undoes: Option<String>,
    },
}

/// A drill's steps, in order. `detected` and `fixed` carry `ok: false` when the goal did
/// not fail, or hold again, within the drill's time; `ended` carries its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DrillStep {
    Started,
    Broke,
    Detected,
    Fixed,
    Restored,
    Ended,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// What an episode runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    /// A failing goal's repair.
    Repair,
    /// A person's task in words.
    Task,
    /// A quoted fact learned outside a job.
    Learn,
}

impl Event {
    pub fn goal(&self) -> Option<&str> {
        self.kind.goal()
    }

    pub fn action(&self) -> Option<&str> {
        self.kind.action()
    }

    pub fn text(&self) -> Option<&str> {
        self.kind.text()
    }

    pub fn ok(&self) -> Option<bool> {
        self.kind.ok()
    }

    pub fn n(&self) -> Option<u64> {
        self.kind.n()
    }
}

impl Kind {
    /// As the journal spells it.
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Goals { .. } => "goals",
            Kind::Check { .. } => "check",
            Kind::Verdict { .. } => "verdict",
            Kind::Known { .. } => "known",
            Kind::Release { .. } => "release",
            Kind::Off { .. } => "off",
            Kind::Waiting { .. } => "waiting",
            Kind::Unknown { .. } => "unknown",
            Kind::Wait { .. } => "wait",
            Kind::Decide { .. } => "decide",
            Kind::Gate { .. } => "gate",
            Kind::Action(_) => "action",
            Kind::Command { .. } => "command",
            Kind::Learn { .. } => "learn",
            Kind::Ask { .. } => "ask",
            Kind::Summary { .. } => "summary",
            Kind::Answer { .. } => "answer",
            Kind::Withdraw { .. } => "withdraw",
            Kind::Sent { .. } => "sent",
            Kind::Watch { .. } => "watch",
            Kind::Unwatch { .. } => "unwatch",
            Kind::Refused { .. } => "refused",
            Kind::Message { .. } => "message",
            Kind::Note { .. } => "note",
            Kind::Drill { .. } => "drill",
        }
    }

    pub fn goal(&self) -> Option<&str> {
        match self {
            Kind::Check { goal, .. }
            | Kind::Verdict { goal, .. }
            | Kind::Known { goal, .. }
            | Kind::Off { goal, .. }
            | Kind::Waiting { goal, .. }
            | Kind::Unknown { goal, .. }
            | Kind::Wait { goal, .. }
            | Kind::Drill { goal, .. } => Some(goal),
            Kind::Decide { goal, .. }
            | Kind::Gate { goal, .. }
            | Kind::Command { goal, .. }
            | Kind::Learn { goal, .. }
            | Kind::Ask { goal, .. }
            | Kind::Summary { goal, .. }
            | Kind::Answer { goal, .. }
            | Kind::Withdraw { goal, .. }
            | Kind::Message { goal, .. }
            | Kind::Note { goal, .. }
            | Kind::Action(
                Act::Ended { goal, .. } | Act::Stopped { goal, .. } | Act::Started { goal, .. },
            ) => goal.as_deref(),
            Kind::Refused { refused, .. } => refused.goal(),
            Kind::Goals { .. }
            | Kind::Release { .. }
            | Kind::Sent { .. }
            | Kind::Watch { .. }
            | Kind::Unwatch { .. } => None,
        }
    }

    pub fn action(&self) -> Option<&str> {
        match self {
            Kind::Gate { action, .. }
            | Kind::Command { action, .. }
            | Kind::Action(
                Act::Ended { action, .. }
                | Act::Stopped { action, .. }
                | Act::Started { action, .. },
            ) => Some(action),
            Kind::Decide { action, .. }
            | Kind::Learn { action, .. }
            | Kind::Ask { action, .. }
            | Kind::Answer { action, .. }
            | Kind::Message { action, .. }
            | Kind::Note { action, .. } => action.as_deref(),
            Kind::Refused { refused, .. } => refused.action(),
            Kind::Summary { .. } => None,
            Kind::Goals { .. }
            | Kind::Check { .. }
            | Kind::Verdict { .. }
            | Kind::Known { .. }
            | Kind::Release { .. }
            | Kind::Off { .. }
            | Kind::Waiting { .. }
            | Kind::Unknown { .. }
            | Kind::Wait { .. }
            | Kind::Withdraw { .. }
            | Kind::Sent { .. }
            | Kind::Watch { .. }
            | Kind::Unwatch { .. }
            | Kind::Drill { .. } => None,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Kind::Decide { text, .. }
            | Kind::Gate { text, .. }
            | Kind::Command { text, .. }
            | Kind::Learn { text, .. }
            | Kind::Summary { text, .. }
            | Kind::Answer { text, .. }
            | Kind::Message { text, .. }
            | Kind::Note { text, .. }
            | Kind::Verdict { text, .. }
            | Kind::Known { text, .. }
            | Kind::Release { version: text, .. }
            | Kind::Off { why: text, .. }
            | Kind::Withdraw { why: text, .. }
            | Kind::Refused { why: text, .. }
            | Kind::Waiting { on: text, .. }
            | Kind::Unknown { why: text, .. }
            | Kind::Sent { channel: text, .. }
            | Kind::Watch { channel: text, .. }
            | Kind::Unwatch { channel: text, .. }
            | Kind::Action(Act::Stopped { stopped: text, .. }) => Some(text),
            Kind::Ask { card, .. } => Some(&card.title),
            Kind::Action(Act::Ended { text, .. } | Act::Started { text, .. })
            | Kind::Drill { text, .. } => text.as_deref(),
            Kind::Goals { .. } | Kind::Check { .. } => None,
            Kind::Wait { until: text, .. } => Some(text),
        }
    }

    /// Held, passed or exit 0; `Some(false)` the opposite; `None` neither, or unknown.
    pub fn ok(&self) -> Option<bool> {
        match self {
            Kind::Check { ok, .. } => *ok,
            Kind::Gate { ok, .. } => ok.then_some(true),
            Kind::Action(Act::Ended { ok, .. }) | Kind::Command { ok, .. } => Some(*ok),
            Kind::Learn { ok, .. } => (!ok).then_some(false),
            Kind::Drill { ok, .. } => *ok,
            _ => None,
        }
    }

    /// The escalation the event is about.
    pub fn n(&self) -> Option<u64> {
        match self {
            Kind::Ask { n, .. }
            | Kind::Summary { n, .. }
            | Kind::Answer { n, .. }
            | Kind::Withdraw { n, .. }
            | Kind::Sent { n, .. }
            | Kind::Watch { n, .. }
            | Kind::Unwatch { n, .. } => Some(*n),
            Kind::Note { n, .. } | Kind::Gate { n, .. } => *n,
            Kind::Refused { refused, .. } => refused.n(),
            _ => None,
        }
    }

    /// `check`: the facts the goal read.
    pub fn facts(&self) -> Option<&BTreeMap<String, String>> {
        match self {
            Kind::Check { facts, .. } => Some(facts),
            _ => None,
        }
    }

    pub fn output(&self) -> Option<&str> {
        match self {
            Kind::Command { output, .. }
            | Kind::Learn { output, .. }
            | Kind::Drill { output, .. } => output.as_deref(),
            _ => None,
        }
    }

    /// Passes every text a program or a person wrote through `f`: what the journal
    /// redacts.
    pub fn redact(&mut self, f: &dyn Fn(&str) -> String) {
        let mut each = |s: &mut String| *s = f(s);
        match self {
            Kind::Check { facts, lines, .. } => {
                facts.values_mut().for_each(&mut each);
                lines
                    .values_mut()
                    .flatten()
                    .for_each(|l| each(&mut l.text));
            }
            Kind::Ask { card, lines, .. } => {
                card.redact(f);
                lines.iter_mut().for_each(&mut each);
            }
            Kind::Summary { lines, text, .. } => {
                lines.iter_mut().for_each(&mut each);
                each(text);
            }
            Kind::Verdict { text, .. } => each(text),
            Kind::Known { text, why, .. } => {
                each(text);
                why.iter_mut().for_each(&mut each);
            }
            Kind::Release { why, .. } => why.iter_mut().for_each(&mut each),
            Kind::Command { text, output, .. } | Kind::Learn { text, output, .. } => {
                each(text);
                output.iter_mut().for_each(&mut each);
            }
            Kind::Decide { text, .. }
            | Kind::Gate { text, .. }
            | Kind::Message { text, .. }
            | Kind::Note { text, .. }
            | Kind::Off { why: text, .. }
            | Kind::Withdraw { why: text, .. }
            | Kind::Refused { why: text, .. }
            | Kind::Action(Act::Stopped { stopped: text, .. }) => each(text),
            Kind::Action(Act::Ended { text, .. }) => text.iter_mut().for_each(&mut each),
            Kind::Drill { text, output, .. } => {
                text.iter_mut().for_each(&mut each);
                output.iter_mut().for_each(&mut each);
            }
            Kind::Action(Act::Started { text, on, .. }) => {
                text.iter_mut().for_each(&mut each);
                on.iter_mut().for_each(&mut each);
            }
            Kind::Goals { .. }
            | Kind::Waiting { .. }
            | Kind::Unknown { .. }
            | Kind::Wait { .. }
            | Kind::Answer { .. }
            | Kind::Sent { .. }
            | Kind::Watch { .. }
            | Kind::Unwatch { .. } => {}
        }
    }
}

/// Everything the journal says about a service now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceState {
    /// The goals of the weave the loop runs, in graph order.
    pub goals: IndexMap<String, GoalState>,
    /// Every escalation the service raised, by number.
    pub escalations: BTreeMap<u64, Escalation>,
    /// What channels watch for the service's own sake.
    pub watches: Vec<Watch>,
    /// The person's tasks by episode, from start to end, also while one waits for an
    /// answer.
    pub tasks: BTreeMap<String, Task>,
    /// Per goal, the outcomes of the actions run for it, oldest first, at most `OUTCOMES`.
    pub outcomes: BTreeMap<String, Vec<Outcome>>,
    /// An action running for a goal that is not failing, with the goal: one a person
    /// runs by hand (`do`) on a goal that was not failing, or one still running after its
    /// goal held, turned off or waits. A failing goal's run is its `Failing` attempt.
    pub outside: Option<(String, Attempt)>,
    /// Per action, when it last passed.
    pub passed: BTreeMap<String, String>,
    /// Per goal, until when (RFC 3339) the loop leaves it alone: the decider chose to
    /// wait, or a fix held and its window has yet to pass. A check that holds ends it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub waits: BTreeMap<String, String>,
    pub spend: Spend,
    /// What the box knows of kinds of log lines, and which actions fixed them.
    #[serde(default)]
    pub memory: Memory,
    /// The newest action with an UNDO that ended, until another action starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<Undoable>,
    /// Each drill's newest run, by name: at most one runs at a time.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drills: BTreeMap<String, DrillRun>,
}

/// Outcomes kept per goal: more than any TRIES counts and the decider's 10.
const OUTCOMES: usize = 100;

/// A drill's run. Times are RFC 3339.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrillRun {
    pub goal: String,
    pub started: String,
    pub state: DrillState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum DrillState {
    /// `step`: the newest step, and how it went; `broke` and `detected` when those steps
    /// passed.
    Running {
        step: DrillStep,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        broke: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detected: Option<String>,
    },
    /// `ok`: the goal failed and held again in time. `text` says how it went.
    Ended { ok: bool, text: String, t: String },
}

impl DrillRun {
    pub fn running(&self) -> bool {
        matches!(self.state, DrillState::Running { .. })
    }

    /// BREAK ran and the goal has not failed yet, nor run out of time to.
    pub fn awaits_failure(&self) -> bool {
        matches!(
            self.state,
            DrillState::Running { step: DrillStep::Broke, ok: true, .. }
        )
    }

    /// The goal failed and has not held again yet, nor run out of time to.
    pub fn awaits_hold(&self) -> bool {
        matches!(
            self.state,
            DrillState::Running { step: DrillStep::Detected, ok: true, .. }
        )
    }
}

/// A drill's newest run, with its name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LastDrill {
    pub name: String,
    #[serde(flatten)]
    pub run: DrillRun,
}

impl LastDrill {
    /// `cl-down passed`, `cl-down failed` or `cl-down runs`.
    pub fn short(&self) -> String {
        let how = match &self.run.state {
            DrillState::Running { .. } => "runs",
            DrillState::Ended { ok: true, .. } => "passed",
            DrillState::Ended { ok: false, .. } => "failed",
        };
        format!("{} {how}", self.name)
    }

    /// Why it failed, when it did.
    pub fn failure(&self) -> Option<&str> {
        match &self.run.state {
            DrillState::Ended { ok: false, text, .. } => Some(text),
            _ => None,
        }
    }
}

/// Where a goal stands. Times are RFC 3339.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum GoalState {
    /// Its IF is false, or a goal it comes AFTER is off.
    Off {
        why: String,
    },
    /// `on`, a goal it comes AFTER, does not hold: one of this weave's, or
    /// `<service>:<goal>`, whose failure is that service's.
    Waiting {
        on: String,
    },
    /// Not checked yet, a fact it reads is not read, or the decider gave no verdict;
    /// `why` when another service it reads or comes AFTER does not run.
    Unknown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    Holds {
        since: String,
    },
    /// A check failed; one more failed check in a row makes it fail.
    Suspect {
        since: String,
        fails: u32,
    },
    /// `attempt`: the action running for it now.
    #[serde(rename = "fails")]
    Failing {
        since: String,
        attempt: Option<Attempt>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    pub action: String,
    pub started: String,
    /// Seconds the goal has to hold.
    pub within: u64,
    /// The episode it runs in.
    pub ep: Option<String>,
    /// Lines of the known trouble kinds the goal failed on when it started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub on: Vec<String>,
    /// The action's UNDO, armed by this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<String>,
    /// The other goals that held when it started.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub held: Vec<String>,
}

/// An action with an UNDO that ended: a goal in `held` that fails within `within`
/// seconds of `ended` makes the loop run `undo`, and a run of `undo` is the undo of
/// `action`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Undoable {
    pub action: String,
    pub undo: String,
    pub goal: String,
    /// The action passed: its goal held within its WITHIN.
    pub ok: bool,
    /// The other goals that held when it started.
    pub held: Vec<String>,
    pub ended: String,
    pub within: u64,
}

impl GoalState {
    /// As `ps`, the dashboard and the metrics name it.
    pub fn name(&self) -> &'static str {
        match self {
            GoalState::Off { .. } => "off",
            GoalState::Waiting { .. } => "waiting",
            GoalState::Unknown { .. } => "unknown",
            GoalState::Holds { .. } => "holds",
            GoalState::Suspect { .. } => "suspect",
            GoalState::Failing { .. } => "fails",
        }
    }

    pub fn since(&self) -> Option<&str> {
        match self {
            GoalState::Holds { since }
            | GoalState::Suspect { since, .. }
            | GoalState::Failing { since, .. } => Some(since),
            GoalState::Off { .. } | GoalState::Waiting { .. } | GoalState::Unknown { .. } => None,
        }
    }

    pub fn holds(&self) -> bool {
        matches!(self, GoalState::Holds { .. })
    }

    pub fn fails(&self) -> bool {
        matches!(self, GoalState::Failing { .. })
    }

    pub fn off(&self) -> bool {
        matches!(self, GoalState::Off { .. })
    }

    /// The state after a check at `t` with verdict `ok`. A goal fails at the second
    /// failed check in a row; a check with no verdict makes it unknown, except that it
    /// neither counts toward two in a row nor resets them.
    pub fn after_check(&self, ok: Option<bool>, t: &str) -> GoalState {
        match (ok, self) {
            (None, GoalState::Suspect { .. })
            | (Some(true), GoalState::Holds { .. })
            | (Some(false), GoalState::Failing { .. }) => self.clone(),
            (None, _) => GoalState::Unknown { why: None },
            (Some(true), _) => GoalState::Holds { since: t.into() },
            (Some(false), GoalState::Suspect { .. }) => GoalState::Failing {
                since: t.into(),
                attempt: None,
            },
            (Some(false), _) => GoalState::Suspect {
                since: t.into(),
                fails: 1,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Escalation {
    pub n: u64,
    pub goal: Option<String>,
    pub action: Option<String>,
    /// The rule that holds the action, as written: the key of its `always` and `never`.
    pub rule: Option<String>,
    pub card: channel::Card,
    pub signed: bool,
    /// The person whose job raised it: only a person answers it, and it is never
    /// withdrawn.
    pub by: Option<String>,
    /// The episode that raised it: a person's task waits in it for the answer.
    pub ep: Option<String>,
    pub raised: String,
    pub state: EscalationState,
    /// The window's lines of the kinds not known normal, when it was raised: an answer
    /// may say what they are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// The drill that failed, on a drill's escalation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum EscalationState {
    /// `sent`: where it went, each with what its channel reads the answer by.
    Open {
        sent: Vec<Sent>,
    },
    Answered {
        word: String,
        by: Option<String>,
        t: String,
    },
    Withdrawn {
        why: String,
        t: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sent {
    pub channel: String,
    pub handle: channel::Sent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watch {
    pub n: u64,
    pub channel: String,
    pub handle: channel::Sent,
}

/// A person's task, by its episode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// The person's words.
    pub text: String,
    pub by: Option<String>,
    pub started: String,
    /// What its runs did, for a run after the person's answer.
    pub commands: Vec<Ran>,
    /// Quoted ASK rules the person passed for its commands.
    pub approved: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ran {
    pub command: String,
    pub ok: bool,
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub t: String,
    pub action: String,
    pub ok: bool,
}

/// Model spend in the month of the newest event that cost something.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Spend {
    /// `YYYY-MM`.
    pub month: String,
    pub usd: f64,
}

impl Escalation {
    pub fn is_open(&self) -> bool {
        matches!(self.state, EscalationState::Open { .. })
    }

    /// About the box itself, not a goal, action or rule: a box that does not fit the weave.
    pub fn on_box(&self) -> bool {
        self.goal.is_none() && self.action.is_none() && self.rule.is_none()
    }

    /// Where it went, while it is open.
    pub fn sent(&self) -> &[Sent] {
        match &self.state {
            EscalationState::Open { sent } => sent,
            EscalationState::Answered { .. } | EscalationState::Withdrawn { .. } => &[],
        }
    }

    /// Why an event that needs it open is refused, when it is not.
    pub fn settled(&self) -> Option<String> {
        let n = self.n;
        match &self.state {
            EscalationState::Open { .. } => None,
            EscalationState::Answered { word, by, t } => Some(format!(
                "#{n} was answered {word} by {} at {t}",
                by.as_deref().unwrap_or("someone")
            )),
            EscalationState::Withdrawn { why, t } => {
                Some(format!("#{n} was withdrawn at {t}: {why}"))
            }
        }
    }
}

fn goal_mut<'a>(s: &'a mut ServiceState, goal: &str) -> Result<&'a mut GoalState, String> {
    s.goals
        .get_mut(goal)
        .ok_or_else(|| format!("the weave has no goal {goal}"))
}

/// Puts `goal` in state `new`. An action still running for it when it stops failing
/// runs on, kept in `outside`.
fn set_goal(s: &mut ServiceState, goal: &str, new: GoalState) -> Result<(), String> {
    let g = goal_mut(s, goal)?;
    let running = match g {
        GoalState::Failing { attempt, .. } if !matches!(new, GoalState::Failing { .. }) => {
            attempt.take()
        }
        _ => None,
    };
    *g = new;
    if let Some(a) = running {
        s.outside = Some((goal.to_string(), a));
    }
    Ok(())
}

/// The attempt of `action` for `goal`, wherever it is kept.
fn attempt_of<'a>(s: &'a ServiceState, goal: &str, action: &str) -> Option<&'a Attempt> {
    let failing = match s.goals.get(goal) {
        Some(GoalState::Failing {
            attempt: Some(a), ..
        }) => Some(a),
        _ => None,
    };
    failing
        .into_iter()
        .chain(s.outside.iter().filter(|(g, _)| g == goal).map(|(_, a)| a))
        .find(|a| a.action == action)
}

/// `action` no longer runs for `goal`, wherever its attempt is kept.
fn end_attempt(s: &mut ServiceState, goal: &str, action: &str) {
    if let Some(GoalState::Failing { attempt, .. }) = s.goals.get_mut(goal) {
        if attempt.as_ref().is_some_and(|a| a.action == action) {
            *attempt = None;
        }
    }
    if s.outside
        .as_ref()
        .is_some_and(|(g, a)| g == goal && a.action == action)
    {
        s.outside = None;
    }
}

/// The open escalation `n`, or why an event about it is refused.
fn open_mut(s: &mut ServiceState, n: u64) -> Result<&mut Escalation, String> {
    let e = s
        .escalations
        .get_mut(&n)
        .ok_or_else(|| format!("no escalation #{n}"))?;
    match e.settled() {
        Some(why) => Err(why),
        None => Ok(e),
    }
}

/// A step of drill `name` at `t`. One drill runs at a time; `detected` comes only while
/// its goal may still fail, `fixed` only while it may still hold again.
fn drill_step(
    s: &mut ServiceState,
    t: &str,
    name: &str,
    goal: &str,
    step: DrillStep,
    ok: bool,
    text: &Option<String>,
) -> Result<(), String> {
    if step == DrillStep::Started {
        if let Some((other, _)) = s.drilling() {
            return Err(format!("drill {other} runs"));
        }
        s.drills.insert(
            name.to_string(),
            DrillRun {
                goal: goal.to_string(),
                started: t.to_string(),
                state: DrillState::Running {
                    step,
                    ok: true,
                    broke: None,
                    detected: None,
                },
            },
        );
        return Ok(());
    }
    // A drill its loop stopped ended at once; its RESTORE runs after, in the background.
    if step == DrillStep::Restored && s.drills.get(name).is_some_and(|r| !r.running()) {
        return Ok(());
    }
    let run = s
        .drills
        .get_mut(name)
        .filter(|r| r.running())
        .ok_or_else(|| format!("drill {name} does not run"))?;
    match step {
        DrillStep::Detected if !run.awaits_failure() => {
            return Err(format!("drill {name} does not wait for {goal} to fail"));
        }
        DrillStep::Fixed if !run.awaits_hold() => {
            return Err(format!("drill {name} does not wait for {goal} to hold"));
        }
        _ => {}
    }
    if step == DrillStep::Ended {
        run.state = DrillState::Ended {
            ok,
            text: text.clone().unwrap_or_default(),
            t: t.to_string(),
        };
        return Ok(());
    }
    if let DrillState::Running {
        step: at,
        ok: was,
        broke,
        detected,
    } = &mut run.state
    {
        match step {
            DrillStep::Broke if ok => *broke = Some(t.to_string()),
            DrillStep::Detected if ok => *detected = Some(t.to_string()),
            _ => {}
        }
        *at = step;
        *was = ok;
    }
    Ok(())
}

/// Applies `e` to the state. An event the state does not allow changes nothing and says
/// why. The same events in the same order give the same state.
pub fn apply(s: &mut ServiceState, e: &Event) -> Result<(), String> {
    match &e.kind {
        Kind::Goals { goals } => {
            let mut old = std::mem::take(&mut s.goals);
            s.goals = goals
                .iter()
                .map(|g| {
                    let was = old.shift_remove(g).unwrap_or(GoalState::Unknown { why: None });
                    (g.clone(), was)
                })
                .collect();
        }
        Kind::Check { goal, ok, .. } => {
            let new = goal_mut(s, goal)?.after_check(*ok, &e.t);
            if new.holds() {
                s.waits.remove(goal);
            }
            set_goal(s, goal, new)?;
        }
        Kind::Verdict { goal, text, p } => {
            goal_mut(s, goal)?;
            s.memory.verdict(goal, text, *p);
        }
        Kind::Known {
            goal,
            text,
            class,
            why,
        } => {
            goal_mut(s, goal)?;
            let told = class.map(|class| Told {
                class,
                by: e.by.clone(),
                why: why.clone(),
                t: e.t.clone(),
            });
            s.memory.told(goal, text, told);
        }
        Kind::Release {
            part,
            version,
            security,
            p,
            why,
        } => match &e.by {
            Some(by) => s.memory.release_told(
                part,
                version,
                ToldRelease {
                    security: *security,
                    by: Some(by.clone()),
                    why: why.clone(),
                    t: e.t.clone(),
                },
            ),
            None => s.memory.release_judged(part, version, p.unwrap_or(if *security { 1.0 } else { 0.0 })),
        },
        Kind::Wait { goal, until } => {
            // A hold ends a wait: one after it would park the goal's next failure.
            if goal_mut(s, goal)?.holds() {
                return Err(format!("{goal} holds: nothing to wait for"));
            }
            s.waits.insert(goal.clone(), until.clone());
        }
        Kind::Off { goal, why } => set_goal(s, goal, GoalState::Off { why: why.clone() })?,
        Kind::Waiting { goal, on } => set_goal(s, goal, GoalState::Waiting { on: on.clone() })?,
        Kind::Unknown { goal, why } => {
            set_goal(s, goal, GoalState::Unknown { why: Some(why.clone()) })?
        }
        Kind::Action(Act::Started {
            goal,
            action,
            text,
            within,
            on,
            undo,
            undoes: _,
        }) => {
            if action == TASK {
                if let Some(ep) = &e.ep {
                    s.tasks.insert(
                        ep.clone(),
                        Task {
                            text: text.clone().unwrap_or_default(),
                            by: e.by.clone(),
                            started: e.t.clone(),
                            commands: Vec::new(),
                            approved: Vec::new(),
                        },
                    );
                }
            } else if let Some(goal) = goal {
                // A newer action makes a regression no longer the older one's.
                s.undo = None;
                let held = s
                    .goals
                    .iter()
                    .filter(|(g, st)| *g != goal && st.holds())
                    .map(|(g, _)| g.clone())
                    .collect();
                let attempt = Attempt {
                    action: action.clone(),
                    started: e.t.clone(),
                    within: within.unwrap_or_default(),
                    ep: e.ep.clone(),
                    on: on.clone(),
                    undo: undo.clone(),
                    held,
                };
                match s.goals.get_mut(goal) {
                    Some(GoalState::Failing { attempt: a, .. }) => *a = Some(attempt),
                    _ => s.outside = Some((goal.clone(), attempt)),
                }
            }
        }
        Kind::Action(Act::Ended {
            goal, action, ok, ..
        }) => {
            if action == TASK {
                if let Some(ep) = &e.ep {
                    s.tasks.remove(ep);
                }
            } else {
                if let Some(goal) = goal {
                    let all = s.outcomes.entry(goal.clone()).or_default();
                    all.push(Outcome {
                        t: e.t.clone(),
                        action: action.clone(),
                        ok: *ok,
                    });
                    if all.len() > OUTCOMES {
                        all.remove(0);
                    }
                    let attempt = attempt_of(s, goal, action).cloned();
                    let on = attempt.as_ref().map(|a| a.on.clone()).unwrap_or_default();
                    s.memory.ran(goal, &on, action, *ok);
                    if let Some(a) = attempt {
                        if let Some(undo) = a.undo {
                            s.undo = Some(Undoable {
                                action: action.clone(),
                                undo,
                                goal: goal.clone(),
                                ok: *ok,
                                held: a.held,
                                ended: e.t.clone(),
                                within: a.within,
                            });
                        }
                    }
                    end_attempt(s, goal, action);
                }
                if *ok {
                    s.passed.insert(action.clone(), e.t.clone());
                }
            }
        }
        Kind::Action(Act::Stopped { goal, action, .. }) => {
            if let Some(goal) = goal {
                end_attempt(s, goal, action);
            }
        }
        Kind::Command {
            action,
            text,
            ok,
            output,
            ..
        } => {
            let task = e.ep.as_ref().filter(|_| action == TASK);
            if let Some(task) = task.and_then(|ep| s.tasks.get_mut(ep)) {
                task.commands.push(Ran {
                    command: text.clone(),
                    ok: *ok,
                    output: output.clone(),
                });
            }
        }
        Kind::Ask {
            n,
            goal,
            action,
            rule,
            card,
            signed,
            lines,
            drill,
        } => {
            if s.escalations.contains_key(n) {
                return Err(format!("#{n} was raised already"));
            }
            s.escalations.insert(
                *n,
                Escalation {
                    n: *n,
                    goal: goal.clone(),
                    action: action.clone(),
                    rule: rule.clone(),
                    card: card.clone(),
                    signed: *signed,
                    by: e.by.clone(),
                    ep: e.ep.clone(),
                    raised: e.t.clone(),
                    state: EscalationState::Open { sent: Vec::new() },
                    lines: lines.clone(),
                    drill: drill.clone(),
                },
            );
            // A repair that asks has stopped: nothing runs for the goal until the answer.
            if let Some(goal) = goal {
                if let Some(GoalState::Failing { attempt, .. }) = s.goals.get_mut(goal) {
                    *attempt = None;
                }
                if s.outside.as_ref().is_some_and(|(g, _)| g == goal) {
                    s.outside = None;
                }
            }
        }
        Kind::Answer {
            n, text, signed, ..
        } => {
            let esc = open_mut(s, *n)?;
            if esc.signed && !signed {
                return Err(format!("#{n}: only a signed answer on Steery settles this"));
            }
            let words = esc.card.words();
            if !words.contains(text) {
                return Err(format!("#{n} takes {}, not {text:?}", words.join(", ")));
            }
            esc.state = EscalationState::Answered {
                word: text.clone(),
                by: e.by.clone(),
                t: e.t.clone(),
            };
            let passed =
                esc.action.as_deref() == Some(TASK) && matches!(text.as_str(), "yes" | "always");
            let rule = esc.rule.clone().filter(|_| passed);
            let ep = esc.ep.clone();
            if let (Some(rule), Some(task)) = (rule, ep.and_then(|ep| s.tasks.get_mut(&ep))) {
                task.approved.push(rule);
            }
        }
        // Whatever became of the escalation meanwhile: the card it went with.
        Kind::Summary { n, lines, .. } => {
            s.escalations
                .get_mut(n)
                .ok_or_else(|| format!("no escalation #{n}"))?
                .card
                .summary = lines.clone();
        }
        Kind::Withdraw { n, why, .. } => {
            open_mut(s, *n)?.state = EscalationState::Withdrawn {
                why: why.clone(),
                t: e.t.clone(),
            };
        }
        Kind::Sent { n, channel, handle } => {
            if let EscalationState::Open { sent } = &mut open_mut(s, *n)?.state {
                sent.push(Sent {
                    channel: channel.clone(),
                    handle: handle.clone(),
                });
            }
        }
        Kind::Watch { n, channel, handle } => s.watches.push(Watch {
            n: *n,
            channel: channel.clone(),
            handle: handle.clone(),
        }),
        Kind::Unwatch { n, channel } => {
            s.watches.retain(|w| !(w.n == *n && w.channel == *channel));
            if let Some(Escalation {
                state: EscalationState::Open { sent },
                ..
            }) = s.escalations.get_mut(n)
            {
                sent.retain(|x| x.channel != *channel);
            }
        }
        Kind::Drill {
            drill,
            goal,
            step,
            ok,
            text,
            ..
        } => drill_step(s, &e.t, drill, goal, *step, ok.unwrap_or(true), text)?,
        Kind::Decide { .. }
        | Kind::Gate { .. }
        | Kind::Learn { .. }
        | Kind::Refused { .. }
        | Kind::Message { .. }
        | Kind::Note { .. } => {}
    }
    if let Some(usd) = e.cost_usd {
        s.spend.add(&e.t, usd);
    }
    Ok(())
}

impl Spend {
    fn add(&mut self, t: &str, usd: f64) {
        let month = t.get(..7).unwrap_or_default();
        if month == self.month {
            self.usd += usd;
        } else if month > self.month.as_str() {
            self.month = month.to_string();
            self.usd = usd;
        }
    }
}

impl ServiceState {
    pub fn goal(&self, name: &str) -> Option<&GoalState> {
        self.goals.get(name)
    }

    /// The drill that runs now, by name.
    pub fn drilling(&self) -> Option<(&str, &DrillRun)> {
        self.drills
            .iter()
            .find(|(_, r)| r.running())
            .map(|(n, r)| (n.as_str(), r))
    }

    /// The drill that started last: what `ps` and the dashboard show.
    pub fn last_drill(&self) -> Option<LastDrill> {
        self.drills
            .iter()
            .max_by(|a, b| a.1.started.cmp(&b.1.started))
            .map(|(n, r)| LastDrill {
                name: n.clone(),
                run: r.clone(),
            })
    }

    /// A goal AFTER `goal` may be checked: `goal` holds, or failed only one check, which
    /// is not failing yet.
    pub fn clears(&self, goal: &str) -> bool {
        self.goal(goal)
            .is_some_and(|g| matches!(g, GoalState::Holds { .. } | GoalState::Suspect { .. }))
    }

    pub fn open(&self) -> impl Iterator<Item = &Escalation> {
        self.escalations.values().filter(|e| e.is_open())
    }

    /// The open escalations for `goal` the loop raised on its own: none a person's job
    /// raised.
    pub fn open_for(&self, goal: &str) -> Vec<u64> {
        self.open()
            .filter(|e| e.goal.as_deref() == Some(goal) && e.by.is_none())
            .map(|e| e.n)
            .collect()
    }

    /// The open escalations about the box itself the loop raised: none a person's job
    /// raised.
    pub fn open_box(&self) -> Vec<u64> {
        self.open()
            .filter(|e| e.on_box() && e.by.is_none())
            .map(|e| e.n)
            .collect()
    }

    /// A person's task waits for the answer to its escalation.
    pub fn task_waits(&self) -> bool {
        self.open().any(|e| e.action.as_deref() == Some(TASK))
    }

    /// The tasks that run, not waiting for an answer, with their episodes.
    pub fn tasks_running(&self) -> impl Iterator<Item = (&str, &Task)> {
        self.tasks
            .iter()
            .filter(|(ep, _)| {
                !self
                    .open()
                    .any(|e| e.action.as_deref() == Some(TASK) && e.ep.as_ref() == Some(*ep))
            })
            .map(|(ep, t)| (ep.as_str(), t))
    }

    /// The task that raised escalation `n`, with its episode.
    pub fn task_of(&self, n: u64) -> Option<(&str, &Task)> {
        let ep = self.escalations.get(&n)?.ep.as_deref()?;
        self.tasks.get_key_value(ep).map(|(ep, t)| (ep.as_str(), t))
    }

    /// Goals the loop leaves alone at `now` (epoch seconds), and whether it leaves the
    /// box alone: the newest escalation of each is open, or was answered `no` less than
    /// `park(goal)` seconds ago. A person's task and a weave change park nothing.
    pub fn parked(&self, now: i64, park: impl Fn(Option<&str>) -> i64) -> (BTreeSet<String>, bool) {
        let mut newest: BTreeMap<Option<&str>, &Escalation> = BTreeMap::new();
        for e in self.escalations.values() {
            if !matches!(e.action.as_deref(), Some(TASK | WEAVE | DRILL)) {
                newest.insert(e.goal.as_deref(), e);
            }
        }
        let mut goals = BTreeSet::new();
        let mut box_parked = false;
        for (goal, e) in newest {
            let parked = match &e.state {
                EscalationState::Open { .. } => true,
                EscalationState::Answered { word, t, .. } => {
                    word == "no" && epoch_of(t).is_some_and(|at| now - at < park(goal))
                }
                EscalationState::Withdrawn { .. } => false,
            };
            match (parked, goal) {
                (false, _) => {}
                (true, Some(g)) => {
                    goals.insert(g.to_string());
                }
                (true, None) => box_parked = true,
            }
        }
        for (g, until) in &self.waits {
            if epoch_of(until).is_some_and(|u| u > now) {
                goals.insert(g.clone());
            }
        }
        (goals, box_parked)
    }

    /// Actions run for `goal` that ended at or after `from` (epoch seconds).
    pub fn tries(&self, goal: &str, from: i64) -> u32 {
        self.outcomes.get(goal).map_or(0, |all| {
            all.iter()
                .filter(|o| epoch_of(&o.t).is_some_and(|t| t >= from))
                .count() as u32
        })
    }

    /// The newest `k` action outcomes for `goal`, oldest first, short, for the decider.
    /// Only outcomes: with the failed checks and the asks beside them, the decider
    /// (jev-1.13) chose `ask` over a fitting restart after one recurrence, and repeat
    /// failures are TRIES' job, counted in code.
    pub fn recent(&self, goal: &str, k: usize) -> Vec<String> {
        let all = self.outcomes.get(goal).map_or(&[][..], Vec::as_slice);
        all[all.len().saturating_sub(k)..]
            .iter()
            .map(|o| {
                let hm = o.t.get(11..16).unwrap_or("");
                let outcome = if o.ok { "passed" } else { "failed" };
                format!("{hm} {}: {outcome}", o.action)
            })
            .collect()
    }

    /// When `action` last passed.
    pub fn passed(&self, action: &str) -> Option<&str> {
        self.passed.get(action).map(String::as_str)
    }

    /// The action a run of `action` undoes: the newest action with an UNDO names it.
    pub fn undoing(&self, action: &str) -> Option<&Undoable> {
        self.undo.as_ref().filter(|u| u.undo == action)
    }

    /// A goal that held before the newest action with an UNDO passed, and fails within
    /// its WITHIN after it (`now` in epoch seconds): the undo to run, and that goal.
    pub fn regressed(&self, now: i64) -> Option<(&Undoable, &str)> {
        let u = self.undo.as_ref().filter(|u| u.ok)?;
        let until = epoch_of(&u.ended)? + u.within as i64;
        if now > until {
            return None;
        }
        let g = u.held.iter().find(|g| self.goal(g).is_some_and(GoalState::fails))?;
        Some((u, g.as_str()))
    }

    /// The newest `always` or `never` the person gave for this rule: it covers every
    /// action the rule holds.
    pub fn kept(&self, rule: &str) -> Option<&'static str> {
        self.escalations
            .values()
            .filter(|e| e.rule.as_deref() == Some(rule))
            .filter_map(|e| match &e.state {
                EscalationState::Answered { word, t, .. } => match word.as_str() {
                    "always" => Some((t, e.n, "always")),
                    "never" => Some((t, e.n, "never")),
                    _ => None,
                },
                EscalationState::Open { .. } | EscalationState::Withdrawn { .. } => None,
            })
            .max()
            .map(|(_, _, w)| w)
    }

    /// The person said yes to a box that does not fit the weave.
    pub fn box_approved(&self) -> bool {
        self.escalations.values().any(|e| {
            e.on_box()
                && matches!(&e.state, EscalationState::Answered { word, .. } if word == "yes" || word == "always")
        })
    }

    /// What runs now: `restart for running`, or `task: <the person's words>`.
    pub fn running(&self) -> Option<String> {
        if let Some((goal, a)) = self.attempts().next() {
            return Some(format!("{} for {goal}", a.action));
        }
        let (_, task) = self.tasks_running().next()?;
        let short = task.text.split_whitespace().collect::<Vec<_>>().join(" ");
        Some(match short.char_indices().nth(80) {
            Some((i, _)) => format!("task: {}…", &short[..i]),
            None => format!("task: {short}"),
        })
    }

    /// The events that end what the state says runs and nothing runs any more, since its
    /// loop stopped for `why`: an action is stopped with no outcome, a task ends failed.
    /// A task that waits for its answer goes on after it.
    pub fn interrupted(&self, why: &str) -> Vec<Event> {
        let mut out: Vec<Event> = self
            .attempts()
            .map(|(goal, a)| Event {
                ep: a.ep.clone(),
                ..Kind::Action(Act::Stopped {
                    goal: Some(goal.clone()),
                    action: a.action.clone(),
                    stopped: why.to_string(),
                })
                .into()
            })
            .collect();
        if let Some((drill, run)) = self.drilling() {
            out.push(
                Kind::Drill {
                    drill: drill.to_string(),
                    goal: run.goal.clone(),
                    step: DrillStep::Ended,
                    ok: Some(false),
                    text: Some(format!("stopped: {why}")),
                    output: None,
                }
                .into(),
            );
        }
        for (ep, t) in self.tasks_running() {
            out.push(Event {
                by: t.by.clone(),
                ep: Some(ep.to_string()),
                ..Kind::Action(Act::Ended {
                    goal: None,
                    action: TASK.to_string(),
                    ok: false,
                    text: Some(format!("ended: {why}")),
                })
                .into()
            });
        }
        out
    }

    /// The actions running, with their goals.
    fn attempts(&self) -> impl Iterator<Item = (&String, &Attempt)> {
        self.goals
            .iter()
            .filter_map(|(goal, g)| match g {
                GoalState::Failing {
                    attempt: Some(a), ..
                } => Some((goal, a)),
                _ => None,
            })
            .chain(self.outside.as_ref().map(|(goal, a)| (goal, a)))
    }

    /// Model spend in `month` (`YYYY-MM`).
    pub fn spend_in(&self, month: &str) -> f64 {
        if self.spend.month == month {
            self.spend.usd
        } else {
            0.0
        }
    }

    /// Every handle a channel reads: those of the open escalations, and the watches.
    pub fn polled(&self) -> impl Iterator<Item = (u64, &str, &channel::Sent)> {
        self.open()
            .flat_map(|e| {
                e.sent()
                    .iter()
                    .map(move |s| (e.n, s.channel.as_str(), &s.handle))
            })
            .chain(
                self.watches
                    .iter()
                    .map(|w| (w.n, w.channel.as_str(), &w.handle)),
            )
    }
}
