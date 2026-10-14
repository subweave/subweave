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

/// Runs `fut` as a new episode, or as part of the one already running.
pub(crate) async fn scope<F: Future>(fut: F) -> F::Output {
    if current().is_some() {
        return fut.await;
    }
    EP.scope(uuid::Uuid::now_v7().simple().to_string(), fut).await
}
