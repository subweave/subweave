//! Heartbeat: a GET to the target (healthchecks.io and the like), at most one a minute
//! per channel however often the daemon ticks.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};

use super::{Channel, client, http};

const EVERY: Duration = Duration::from_secs(60);

static LAST: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

pub(super) async fn send(ch: &Channel) -> Result<()> {
    let url = ch
        .target
        .as_deref()
        .ok_or_else(|| anyhow!("no URL; add the channel again"))?;
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if last
            .get(&ch.name)
            .is_some_and(|at| now.duration_since(*at) < EVERY)
        {
            return Ok(());
        }
        last.insert(ch.name.clone(), now);
    }
    http("heartbeat", client().get(url)).await?;
    Ok(())
}
