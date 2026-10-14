//! The box: what it is (`/api/box`) and how it does (`/api/vitals`). On Linux a sampler
//! reads `/proc` every 10 s, keeps 24 h of samples in memory and appends one-minute
//! means to `state()/dash/vitals.jsonl`, pruned past 30 days.

use std::collections::{BTreeMap, VecDeque};
use std::ffi::CString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ApiError, App};
use crate::boxinfo::BoxInfo;
use crate::daemon::{epoch, epoch_of, hostname, now, running};
use crate::{paths, service};

const EVERY: Duration = Duration::from_secs(10);
const KEEP: i64 = 24 * 3600;
const FILE_KEEP: i64 = 30 * 24 * 3600;
const PRUNE_EVERY: Duration = Duration::from_secs(24 * 3600);

/// One reading. `t` is RFC 3339; network in bytes per second.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Sample {
    pub t: String,
    pub cpu_pct: f64,
    pub mem_pct: f64,
    pub load1: f64,
    pub disk_pct: BTreeMap<String, f64>,
    pub net_rx_bps: f64,
    pub net_tx_bps: f64,
}

#[derive(Default)]
pub(crate) struct Vitals {
    /// The last 24 h of 10 s samples, oldest first.
    recent: Mutex<VecDeque<(i64, Sample)>>,
}

/// This box reads like Linux: `/proc` is there.
fn linux() -> bool {
    std::path::Path::new("/proc/stat").is_file()
}

fn file() -> std::path::PathBuf {
    paths::dash_state().join("vitals.jsonl")
}

/// CPU jiffies: (idle, total).
fn cpu() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let nums: Vec<u64> = stat
        .lines()
        .next()?
        .split_whitespace()
        .skip(1)
        .take(8)
        .filter_map(|n| n.parse().ok())
        .collect();
    if nums.len() < 5 {
        return None;
    }
    Some((nums[3] + nums[4], nums.iter().sum()))
}

/// `(total, available)` bytes from `/proc/meminfo`.
fn mem() -> Option<(f64, f64)> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |key: &str| -> Option<f64> {
        info.lines()
            .find_map(|l| l.strip_prefix(key))?
            .split_whitespace()
            .next()?
            .parse::<f64>()
            .ok()
            .map(|kb| kb * 1024.0)
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

fn load1() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Received and sent bytes of every interface but loopback.
fn net() -> Option<(u64, u64)> {
    let dev = std::fs::read_to_string("/proc/net/dev").ok()?;
    let mut rx = 0;
    let mut tx = 0;
    for l in dev.lines().skip(2) {
        let Some((name, rest)) = l.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<u64> = rest
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        if f.len() >= 9 {
            rx += f[0];
            tx += f[8];
        }
    }
    Some((rx, tx))
}

/// The mount points of real filesystems, one per device: block devices and ZFS, not
/// snaps' loop images. Elsewhere than Linux, `/`.
pub(crate) fn mounts() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string("/proc/mounts") else {
        return vec!["/".into()];
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 3 {
            continue;
        }
        let (dev, mount, fs) = (f[0], f[1], f[2]);
        let real = (dev.starts_with("/dev/") && !dev.starts_with("/dev/loop")) || fs == "zfs";
        if !real || fs == "squashfs" || !seen.insert(dev.to_string()) {
            continue;
        }
        out.push(unescape(mount));
    }
    out
}

/// `/proc/mounts` writes a space as `\040`.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = b
            .get(i + 1..i + 4)
            .filter(|d| b[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)));
        match octal {
            Some(d) => {
                let n = d.iter().fold(0u32, |n, c| n * 8 + u32::from(c - b'0'));
                out.push(n as u8);
                i += 4;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `(size, used, available)` bytes of the filesystem at `mount`.
pub(crate) fn disk(mount: &str) -> Option<(u64, u64, u64)> {
    let c = CString::new(std::path::Path::new(mount).as_os_str().as_bytes()).ok()?;
    // SAFETY: `c` is a valid NUL-terminated path; statvfs only writes into `st`.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let f = st.f_frsize as u64;
    let size = st.f_blocks as u64 * f;
    let used = (st.f_blocks as u64).saturating_sub(st.f_bfree as u64) * f;
    Some((size, used, st.f_bavail as u64 * f))
}

/// Used share as `df` counts it: of what is used plus what a user may still take.
fn disk_pct() -> BTreeMap<String, f64> {
    mounts()
        .into_iter()
        .filter_map(|m| {
            let (_, used, avail) = disk(&m)?;
            let all = used + avail;
            (all > 0).then(|| (m, used as f64 * 100.0 / all as f64))
        })
        .collect()
}

/// Samples every 10 s while the dashboard runs; nothing elsewhere than Linux.
pub(crate) async fn sample(app: Arc<App>) {
    if !linux() {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(paths::dash_state()) {
        eprintln!("create {}: {e}", paths::dash_state().display());
    }
    prune();
    let mut pruned = Instant::now();
    let mut prev = (cpu(), net(), Instant::now());
    let mut minute: Vec<Sample> = Vec::new();
    let mut tick = tokio::time::interval(EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await;
    loop {
        tick.tick().await;
        let (c, n, at) = (cpu(), net(), Instant::now());
        let secs = at.duration_since(prev.2).as_secs_f64().max(0.001);
        let cpu_pct = match (prev.0, c) {
            (Some((i0, t0)), Some((i1, t1))) if t1 > t0 => {
                100.0 * (1.0 - (i1.saturating_sub(i0)) as f64 / (t1 - t0) as f64)
            }
            _ => 0.0,
        };
        let (net_rx_bps, net_tx_bps) = match (prev.1, n) {
            (Some((r0, s0)), Some((r1, s1))) => (
                r1.saturating_sub(r0) as f64 / secs,
                s1.saturating_sub(s0) as f64 / secs,
            ),
            _ => (0.0, 0.0),
        };
        prev = (c, n, at);
        let mem_pct = mem().map_or(0.0, |(total, avail)| 100.0 * (1.0 - avail / total));
        let t = epoch();
        let s = Sample {
            t: now(),
            cpu_pct: round1(cpu_pct),
            mem_pct: round1(mem_pct),
            load1: load1().unwrap_or(0.0),
            disk_pct: disk_pct().into_iter().map(|(k, v)| (k, round1(v))).collect(),
            net_rx_bps: net_rx_bps.round(),
            net_tx_bps: net_tx_bps.round(),
        };
        app.live
            .send("vitals", &serde_json::to_value(&s).unwrap_or(Value::Null));
        {
            let mut recent = app.vitals.recent.lock().unwrap_or_else(|e| e.into_inner());
            recent.push_back((t, s.clone()));
            while recent.front().is_some_and(|(at, _)| t - at > KEEP) {
                recent.pop_front();
            }
        }
        if minute
            .first()
            .and_then(|m| epoch_of(&m.t))
            .is_some_and(|m0| m0 / 60 != t / 60)
        {
            append(&mean(&minute));
            minute.clear();
        }
        minute.push(s);
        if pruned.elapsed() >= PRUNE_EVERY {
            prune();
            pruned = Instant::now();
        }
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// One sample: the mean of each value, at the first sample's minute.
fn mean(samples: &[Sample]) -> Sample {
    let n = samples.len().max(1) as f64;
    let avg = |f: &dyn Fn(&Sample) -> f64| samples.iter().map(f).sum::<f64>() / n;
    let mut disk: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    for s in samples {
        for (m, v) in &s.disk_pct {
            let e = disk.entry(m.clone()).or_default();
            e.0 += v;
            e.1 += 1.0;
        }
    }
    let t = samples
        .first()
        .and_then(|s| epoch_of(&s.t))
        .map(|t| t / 60 * 60)
        .and_then(|t| jiff::Timestamp::from_second(t).ok())
        .map(|t| t.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(now);
    Sample {
        t,
        cpu_pct: round1(avg(&|s| s.cpu_pct)),
        mem_pct: round1(avg(&|s| s.mem_pct)),
        load1: (avg(&|s| s.load1) * 100.0).round() / 100.0,
        disk_pct: disk
            .into_iter()
            .map(|(m, (sum, k))| (m, round1(sum / k)))
            .collect(),
        net_rx_bps: avg(&|s| s.net_rx_bps).round(),
        net_tx_bps: avg(&|s| s.net_tx_bps).round(),
    }
}

fn append(s: &Sample) {
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file())?;
        let mut line = serde_json::to_vec(s)?;
        line.push(b'\n');
        f.write_all(&line)
    };
    if let Err(e) = write() {
        eprintln!("write {}: {e}", file().display());
    }
}

/// The file's samples from `from` on.
fn read_file(from: i64) -> Vec<(i64, Sample)> {
    let Ok(text) = std::fs::read_to_string(file()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Sample>(l).ok())
        .filter_map(|s| Some((epoch_of(&s.t)?, s)))
        .filter(|(t, _)| *t >= from)
        .collect()
}

/// Drops the file's samples older than 30 days.
fn prune() {
    let keep = read_file(epoch() - FILE_KEEP);
    let mut bytes = Vec::new();
    for (_, s) in &keep {
        if let Ok(l) = serde_json::to_vec(s) {
            bytes.extend(l);
            bytes.push(b'\n');
        }
    }
    let tmp = file().with_extension("jsonl.tmp");
    if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, file())) {
        eprintln!("prune {}: {e}", file().display());
    }
}

#[derive(Deserialize)]
pub(crate) struct Range {
    range: Option<String>,
}

pub(crate) async fn get(
    State(app): State<Arc<App>>,
    Query(q): Query<Range>,
) -> Result<Json<Value>, ApiError> {
    let (span, step) = match q.range.as_deref().unwrap_or("1h") {
        "1h" => (3600, 10),
        "24h" => (24 * 3600, 60),
        "7d" => (7 * 24 * 3600, 600),
        "30d" => (30 * 24 * 3600, 3600),
        other => return Err(ApiError::bad(format!("range {other:?}: 1h, 24h, 7d or 30d"))),
    };
    if !linux() {
        return Ok(Json(json!({
            "step_s": step, "t": [], "cpu_pct": [], "mem_pct": [], "load1": [],
            "disk_pct": {}, "net_rx_bps": [], "net_tx_bps": [],
        })));
    }
    let now = epoch();
    let from = now - span;
    let mut samples: Vec<(i64, Sample)> = app
        .vitals
        .recent
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(t, _)| *t >= from)
        .cloned()
        .collect();
    // Memory holds the time since the dashboard started; the file, minutes before it.
    if span > 3600 || samples.is_empty() {
        let first = samples.first().map_or(i64::MAX, |(t, _)| *t);
        let mut older: Vec<(i64, Sample)> =
            read_file(from).into_iter().filter(|(t, _)| *t < first).collect();
        older.append(&mut samples);
        samples = older;
    }
    Ok(Json(buckets(&samples, now, span, step)))
}

/// Means per step, `null` for a step with no sample; steps end at `now`.
fn buckets(samples: &[(i64, Sample)], now: i64, span: i64, step: i64) -> Value {
    let n = (span / step) as usize;
    let end = now / step * step + step;
    let start = end - n as i64 * step;
    let mut sums: BTreeMap<String, (Vec<f64>, Vec<u32>)> = BTreeMap::new();
    let mut add = |key: String, k: usize, v: f64| {
        let e = sums
            .entry(key)
            .or_insert_with(|| (vec![0.0; n], vec![0; n]));
        e.0[k] += v;
        e.1[k] += 1;
    };
    for (t, s) in samples {
        if *t < start || *t >= end {
            continue;
        }
        let k = ((t - start) / step) as usize;
        add("cpu_pct".into(), k, s.cpu_pct);
        add("mem_pct".into(), k, s.mem_pct);
        add("load1".into(), k, s.load1);
        add("net_rx_bps".into(), k, s.net_rx_bps);
        add("net_tx_bps".into(), k, s.net_tx_bps);
        for (m, v) in &s.disk_pct {
            add(format!("disk:{m}"), k, *v);
        }
    }
    let series = |key: &str| -> Value {
        match sums.get(key) {
            Some((sum, count)) => (0..n)
                .map(|k| {
                    (count[k] > 0)
                        .then(|| (sum[k] / f64::from(count[k]) * 100.0).round() / 100.0)
                        .map_or(Value::Null, |v| json!(v))
                })
                .collect(),
            None => Value::Array(vec![Value::Null; n]),
        }
    };
    let t: Vec<String> = (0..n)
        .filter_map(|k| jiff::Timestamp::from_second(start + k as i64 * step).ok())
        .map(|ts| ts.strftime("%Y-%m-%dT%H:%M:%SZ").to_string())
        .collect();
    let disks: serde_json::Map<String, Value> = sums
        .keys()
        .filter_map(|k| k.strip_prefix("disk:"))
        .map(|m| (m.to_string(), series(&format!("disk:{m}"))))
        .collect();
    json!({
        "step_s": step,
        "t": t,
        "cpu_pct": series("cpu_pct"),
        "mem_pct": series("mem_pct"),
        "load1": series("load1"),
        "disk_pct": disks,
        "net_rx_bps": series("net_rx_bps"),
        "net_tx_bps": series("net_tx_bps"),
    })
}

fn kernel() -> String {
    // SAFETY: utsname is plain data that uname fills.
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } != 0 {
        return String::new();
    }
    // SAFETY: uname writes a NUL-terminated string into `release`.
    unsafe { std::ffi::CStr::from_ptr(u.release.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn uptime_s() -> Option<u64> {
    if let Ok(t) = std::fs::read_to_string("/proc/uptime") {
        return t.split_whitespace().next()?.parse::<f64>().ok().map(|s| s as u64);
    }
    // macOS: `{ sec = 1728…, usec = … } …`.
    let out = std::process::Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let sec: i64 = text
        .split("sec = ")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    u64::try_from(epoch() - sec).ok()
}

pub(crate) async fn box_info() -> Result<Json<Value>, ApiError> {
    let info = BoxInfo::read().map_err(ApiError::internal)?;
    let disks: Vec<Value> = mounts()
        .into_iter()
        .filter_map(|m| {
            let (size, used, _) = disk(&m)?;
            Some(json!({"mount": m, "size_bytes": size, "used_bytes": used}))
        })
        .collect();
    let mut spend = 0.0;
    for name in service::names().map_err(ApiError::internal)? {
        if let Ok(Some(s)) = service::status(&name) {
            spend += s.spend_month_usd;
        }
    }
    let daemon = match running() {
        Some(r) => json!({"active": true, "since": r.since}),
        None => json!({"active": false}),
    };
    Ok(Json(json!({
        "host": hostname(),
        "os": format!("{} {}", info.os, info.version),
        "kernel": kernel(),
        "cpus": info.cpus,
        "mem_bytes": (info.ram_gb * 1e9).round() as u64,
        "disks": disks,
        "uptime_s": uptime_s(),
        "version": env!("CARGO_PKG_VERSION"),
        "daemon": daemon,
        "spend_month_usd": spend,
        "asks_open": service::all_asks().map_err(ApiError::internal)?.len(),
    })))
}
