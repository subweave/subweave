//! What this machine is: system, size, and whether a weave fits it.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::paths;
use crate::weave::Weave;

/// RAM and disk count as enough within this share of NEEDS: a 4 GB box shows less than
/// 4 GB to its system, and a 40 GB disk less than 40 GB after the filesystem.
const SIZE_SLACK: f64 = 0.9;

#[derive(Debug, Clone, Serialize)]
pub struct BoxInfo {
    /// `ubuntu`, `debian`, `macos`.
    pub os: String,
    /// `24.04`.
    pub version: String,
    pub arch: String,
    pub cpus: u32,
    pub gpus: u32,
    pub ram_gb: f64,
    /// Free plus used space of the box's largest disk filesystem: a weave may keep its
    /// data on any mount, like `/mnt/data`, not only where the state directory is.
    pub disk_gb: f64,
}

impl BoxInfo {
    pub fn read() -> Result<BoxInfo> {
        let (os, version, cpus, gpus, ram_gb) = system()?;
        Ok(BoxInfo {
            os,
            version,
            arch: std::env::consts::ARCH.to_string(),
            cpus,
            gpus,
            ram_gb,
            disk_gb: largest_disk_gb()?,
        })
    }

    /// What this box lacks for the weave's FROM and NEEDS, one line each. FROM fits the
    /// box's system at that version, or a version under it (`macos:26` fits 26.5).
    pub fn check(&self, w: &Weave) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(from) = &w.from {
            let fits = from.os == self.os
                && (self.version == from.version
                    || self.version.starts_with(&format!("{}.", from.version)));
            if !fits {
                out.push(format!(
                    "this box runs {} {}; the weave needs {}:{}",
                    self.os, self.version, from.os, from.version
                ));
            }
        }
        let n = &w.needs;
        if let Some(cpu) = n.cpu.filter(|c| *c > self.cpus) {
            out.push(format!(
                "this box has {} CPUs; the weave needs {cpu}",
                self.cpus
            ));
        }
        if let Some(gpu) = n.gpu.filter(|g| *g > self.gpus) {
            out.push(format!(
                "this box has {} GPUs; the weave needs {gpu}",
                self.gpus
            ));
        }
        if let Some(ram) = n.ram_gb.filter(|r| self.ram_gb < r * SIZE_SLACK) {
            out.push(format!(
                "this box has {:.1} GB RAM; the weave needs {ram} GB",
                self.ram_gb
            ));
        }
        if let Some(disk) = n.disk_gb.filter(|d| self.disk_gb < d * SIZE_SLACK) {
            out.push(format!(
                "this box's largest disk is {:.0} GB; the weave needs {disk} GB",
                self.disk_gb
            ));
        }
        out
    }
}

/// What this box lacks for the weave's FROM and NEEDS; a box it cannot read is one line.
pub fn check(w: &Weave) -> Vec<String> {
    match BoxInfo::read() {
        Ok(info) => info.check(w),
        Err(e) => vec![format!("cannot read this box: {e:#}")],
    }
}

#[cfg(target_os = "linux")]
fn system() -> Result<(String, String, u32, u32, f64)> {
    let release = std::fs::read_to_string("/etc/os-release").context("reading /etc/os-release")?;
    let field = |key: &str| {
        release
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").context("reading /proc/cpuinfo")?;
    let mut cpus = cpuinfo
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count() as u32;
    if cpus == 0 {
        cpus = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    }
    let meminfo = std::fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?;
    let kb: f64 = meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .context("no MemTotal in /proc/meminfo")?;
    let gpus = std::fs::read_dir("/dev")
        .map(|d| {
            d.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .and_then(|n| n.strip_prefix("nvidia"))
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                })
                .count() as u32
        })
        .unwrap_or(0);
    Ok((
        field("ID"),
        field("VERSION_ID"),
        cpus,
        gpus,
        kb * 1024.0 / 1e9,
    ))
}

#[cfg(target_os = "macos")]
fn system() -> Result<(String, String, u32, u32, f64)> {
    let version = run("sw_vers", &["-productVersion"])?;
    let cpus: u32 = run("sysctl", &["-n", "hw.ncpu"])?
        .parse()
        .context("reading hw.ncpu")?;
    let bytes: f64 = run("sysctl", &["-n", "hw.memsize"])?
        .parse()
        .context("reading hw.memsize")?;
    Ok(("macos".to_string(), version, cpus, 0, bytes / 1e9))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system() -> Result<(String, String, u32, u32, f64)> {
    bail!("Subweave reads only Linux and macOS boxes")
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        bail!("{cmd} {} exited with {}", args.join(" "), out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The largest filesystem on a block device: `/proc/mounts` sources under `/dev/`.
#[cfg(target_os = "linux")]
fn largest_disk_gb() -> Result<f64> {
    let mounts = std::fs::read_to_string("/proc/mounts").context("reading /proc/mounts")?;
    let largest = mounts
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let source = f.next()?;
            let at = f.next()?;
            source.starts_with("/dev/").then_some(at)
        })
        // /proc/mounts escapes a space in a mount point as \040.
        .filter_map(|at| disk_gb(Path::new(&at.replace("\\040", " "))).ok())
        .fold(0.0, f64::max);
    if largest > 0.0 { Ok(largest) } else { disk_gb(&paths::state()) }
}

/// macOS keeps one data volume: the one that holds the state directory.
#[cfg(not(target_os = "linux"))]
fn largest_disk_gb() -> Result<f64> {
    disk_gb(&paths::state())
}

/// The nearest existing directory at or above `dir`: the state directory may not exist yet.
fn disk_gb(dir: &Path) -> Result<f64> {
    let existing = dir
        .ancestors()
        .find(|p| p.exists())
        .unwrap_or(Path::new("/"));
    let c = CString::new(existing.as_os_str().as_bytes())?;
    // SAFETY: `c` is a valid NUL-terminated path; statvfs only writes into `st`.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        bail!(
            "statvfs {}: {}",
            existing.display(),
            std::io::Error::last_os_error()
        );
    }
    let frsize = st.f_frsize as f64;
    let used = (st.f_blocks as f64 - st.f_bfree as f64) * frsize;
    let free = st.f_bavail as f64 * frsize;
    Ok((used + free) / 1e9)
}
