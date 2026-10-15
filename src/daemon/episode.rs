//! An episode: the journal lines one job run or one learn writes, all with its `ep`. The id
//! lives in the episode's tokio task, so lines other tasks write meanwhile (checks,
//! channel deliveries) never join it. Its first line says what job it runs.

use std::cell::Cell;

use crate::service::JobKind;

struct Ep {
    id: String,
    kind: JobKind,
    /// A line carries the kind already.
    marked: Cell<bool>,
}

tokio::task_local! {
    static EP: Ep;
}

/// The episode the current task writes for, if any.
pub(crate) fn current() -> Option<String> {
    EP.try_with(|e| e.id.clone()).ok()
}

/// The episode a new line joins: its id, and its kind when the line is its first.
pub(crate) fn for_line() -> Option<(String, Option<JobKind>)> {
    EP.try_with(|e| (e.id.clone(), (!e.marked.replace(true)).then_some(e.kind)))
        .ok()
}

/// A new episode id.
pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

/// Runs `fut` as a new episode of `kind`, or as part of the one already running.
pub(crate) async fn scope<F: Future>(kind: JobKind, fut: F) -> F::Output {
    if current().is_some() {
        return fut.await;
    }
    let ep = Ep {
        id: new_id(),
        kind,
        marked: Cell::new(false),
    };
    EP.scope(ep, fut).await
}

/// Runs a run of a person's task in its episode `ep`: its id and its first line, which
/// says `task`, are from when it was queued.
pub(crate) async fn task<F: Future>(ep: String, fut: F) -> F::Output {
    let ep = Ep {
        id: ep,
        kind: JobKind::Task,
        marked: Cell::new(true),
    };
    EP.scope(ep, fut).await
}
