//! The services whose loops run in this daemon, for what other services' weaves read of
//! them: a goal's state (its journal's fold) and a fact's newest observation (its ring).
//! A loop joins when it starts and leaves when the daemon halts it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, Weak};

use crate::service as svc;
use crate::weave::Value;

use super::lock;
use super::observe::{self, Seen, Span};
use super::service::Ctx;
use super::state::GoalState;

#[derive(Default)]
pub(crate) struct Registry {
    loops: Mutex<BTreeMap<String, Weak<Ctx>>>,
}

impl Registry {
    pub fn join(&self, ctx: &Arc<Ctx>) {
        lock(&self.loops).insert(ctx.name.clone(), Arc::downgrade(ctx));
    }

    pub fn leave(&self, service: &str) {
        lock(&self.loops).remove(service);
    }

    /// The service's loop, or why it does not run: stopped, not on this box, or not
    /// started (yet, or its loop ended on an error).
    fn get(&self, service: &str) -> Result<Arc<Ctx>, String> {
        if let Some(ctx) = lock(&self.loops).get(service).and_then(Weak::upgrade) {
            return Ok(ctx);
        }
        Err(match svc::spec(service) {
            Err(_) => format!("{service} is not on this box"),
            Ok(s) if s.stopped => format!("{service} is stopped"),
            Ok(_) => format!("{service} does not run"),
        })
    }

    /// Why one of `services` cannot be read now, the first that cannot.
    pub fn gone<'a>(&self, services: impl IntoIterator<Item = &'a str>) -> Option<String> {
        services.into_iter().find_map(|s| self.get(s).err())
    }

    /// Where the goal stands on its service; `Err` why that service does not run.
    pub fn goal(&self, service: &str, goal: &str) -> Result<GoalState, String> {
        let ctx = self.get(service)?;
        let state = ctx.journal().state().goal(goal).cloned();
        Ok(state.unwrap_or(GoalState::Unknown { why: None }))
    }

    /// The fact's newest observation on its service; `None` with none, or when that
    /// service does not run.
    pub fn value(&self, service: &str, fact: &str) -> Option<Value> {
        self.get(service).ok()?.observe.value(fact, observe::now_ms())
    }

    /// The facts named, as (service, fact), as their services show them now, under
    /// `<service>:<fact>`; one with nothing to show is left out.
    pub fn seen(&self, facts: &BTreeSet<(&str, &str)>) -> BTreeMap<String, Seen> {
        let now = observe::now_ms();
        let span = Span { from: now, to: now };
        let mut out = BTreeMap::new();
        for (s, f) in facts {
            let Ok(ctx) = self.get(s) else { continue };
            let one = BTreeSet::from([f.to_string()]);
            if let Some(seen) = ctx.observe.values(&one, span).remove(*f) {
                out.insert(format!("{s}:{f}"), seen);
            }
        }
        out
    }
}
