//! Every file Subweave reads or writes lives under one of two directories.
//!
//! | | root on Linux | otherwise | override |
//! |---|---|---|---|
//! | config | `/etc/subweave` | `~/.config/subweave` | `SUBWEAVE_CONFIG` |
//! | state | `/var/lib/subweave` | `~/.local/state/subweave` | `SUBWEAVE_STATE` |

use std::path::PathBuf;

fn system_wide() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    cfg!(target_os = "linux") && unsafe { libc::geteuid() } == 0
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Written by the CLI, read by the daemon.
pub fn config() -> PathBuf {
    if let Some(dir) = std::env::var_os("SUBWEAVE_CONFIG") {
        return PathBuf::from(dir);
    }
    if system_wide() {
        PathBuf::from("/etc/subweave")
    } else {
        home().join(".config/subweave")
    }
}

/// Written only by the daemon, except `inbox/`.
pub fn state() -> PathBuf {
    if let Some(dir) = std::env::var_os("SUBWEAVE_STATE") {
        return PathBuf::from(dir);
    }
    if system_wide() {
        PathBuf::from("/var/lib/subweave")
    } else {
        home().join(".local/state/subweave")
    }
}

/// Providers added with `subweave provider add`; `openrouter` is built in.
pub fn providers() -> PathBuf {
    config().join("providers.json")
}

pub fn keys() -> PathBuf {
    config().join("keys")
}

/// A provider's key, mode 0600.
pub fn key(provider: &str) -> PathBuf {
    keys().join(provider)
}

/// Mode 0600.
pub fn channels() -> PathBuf {
    config().join("channels.json")
}

/// On the person's computer: which box runs each service.
pub fn boxes() -> PathBuf {
    config().join("boxes.json")
}

/// The dashboard's bind address, port and password hash; mode 0600.
pub fn dash() -> PathBuf {
    config().join("dash.json")
}

/// What the dashboard keeps: `vitals.jsonl`.
pub fn dash_state() -> PathBuf {
    state().join("dash")
}

/// The running daemon's pid and start time.
pub fn daemon() -> PathBuf {
    state().join("daemon.json")
}

/// Commands from the CLI, one `<uuidv7>.json` each; the daemon consumes and deletes them.
pub fn inbox() -> PathBuf {
    state().join("inbox")
}

/// The daemon-wide ask counter.
pub fn asks_next() -> PathBuf {
    state().join("asks.next")
}

pub fn services() -> PathBuf {
    state().join("services")
}

pub fn service(name: &str) -> PathBuf {
    services().join(name)
}

pub fn weavefile(service: &str) -> PathBuf {
    self::service(service).join("Weavefile")
}

pub fn readme(service: &str) -> PathBuf {
    self::service(service).join("README.md")
}

pub fn service_json(service: &str) -> PathBuf {
    self::service(service).join("service.json")
}

/// Mode 0600.
pub fn secrets(service: &str) -> PathBuf {
    self::service(service).join("secrets.json")
}

pub fn journal(service: &str) -> PathBuf {
    self::service(service).join("journal.jsonl")
}

pub fn learned(service: &str) -> PathBuf {
    self::service(service).join("learned.json")
}

/// The coder's command output, whole and redacted: `<episode>-<n>.txt`, mode 0600.
pub fn output(service: &str) -> PathBuf {
    self::service(service).join("output")
}

pub fn status(service: &str) -> PathBuf {
    self::service(service).join("status.json")
}
