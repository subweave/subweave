//! An episode: the journal lines one job run or one learn writes, all with its `ep`. The id
//! lives in the episode's tokio task, so lines other tasks write meanwhile (checks,
//! channel deliveries) never join it.

tokio::task_local! {
    static EP: String;
}

/// The episode the current task writes for, if any.
pub(crate) fn current() -> Option<String> {
    EP.try_with(Clone::clone).ok()
}

/// A new episode id.
pub(crate) fn new_id() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

/// Runs `fut` as a new episode, or as part of the one already running.
pub(crate) async fn scope<F: Future>(fut: F) -> F::Output {
    if current().is_some() {
        return fut.await;
    }
    within(new_id(), fut).await
}

/// Runs `fut` in episode `ep`: a person's task has its id from when it was queued.
pub(crate) async fn within<F: Future>(ep: String, fut: F) -> F::Output {
    EP.scope(ep, fut).await
}
