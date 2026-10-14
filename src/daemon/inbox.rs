//! `inbox/<uuidv7>.json`: commands from the CLI, oldest first (UUIDv7 names sort by time).

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::paths;
use crate::service::Command;

/// Every command waiting, oldest first; the caller deletes each file after applying it.
pub(crate) fn take() -> Vec<(PathBuf, Result<Command>)> {
    let Ok(entries) = std::fs::read_dir(paths::inbox()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".json") && !n.starts_with('.'))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let cmd = std::fs::read(&p)
                .with_context(|| format!("read {}", p.display()))
                .and_then(|b| serde_json::from_slice(&b).context("not a command"));
            (p, cmd)
        })
        .collect()
}
