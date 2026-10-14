//! Commands on another box: the CLI there does the work, this one forwards over ssh.
//!
//! Before a command, the box gets this exact `subweave` version; `run` also sends the
//! weave directory, the models' keys and the SECRETs set here. `boxes.json` remembers
//! which box runs each service, so later commands take the name alone.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::model::{ModelRef, provider};
use crate::paths;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const RELEASES: &str = "https://github.com/subweave/subweave/releases/download";
const INSTALLED: &str = "/usr/local/bin/subweave";

/// `ssh` that never prompts for a password; `tty` gives the remote side a terminal for
/// prompts.
pub fn ssh(host: &str, tty: bool) -> Command {
    let mut c = Command::new("ssh");
    c.args(["-o", "BatchMode=yes"]);
    if tty {
        c.arg("-t");
    }
    c.arg(host);
    c
}

/// One argument for a POSIX shell.
pub fn quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@,+%".contains(&b));
    if plain {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// `subweave <args>` as one shell line.
pub fn command_line(args: &[String]) -> String {
    let mut line = String::from("subweave");
    for a in args {
        line.push(' ');
        line.push_str(&quote(a));
    }
    line
}

/// `subweave <args>` on the box, with a terminal when this one has one; the exit code.
pub fn run(host: &str, args: &[String]) -> Result<i32> {
    run_line(host, &command_line(args))
}

pub fn run_line(host: &str, line: &str) -> Result<i32> {
    let status = ssh(host, std::io::stdin().is_terminal())
        .arg(line)
        .status()
        .context("running ssh")?;
    Ok(status.code().unwrap_or(1))
}

/// A line on the box, without a terminal; its stdout.
fn output(host: &str, line: &str) -> Result<String> {
    let out = ssh(host, false)
        .arg(line)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .context("running ssh")?;
    if !out.status.success() {
        bail!("ssh {host} {line:?} failed");
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Makes sure the box runs this exact `subweave` version, installing it when not.
pub async fn ensure(host: &str) -> Result<()> {
    let said = output(host, "uname -sm; subweave version 2>/dev/null || true")?;
    let mut lines = said.lines();
    let uname = lines.next().unwrap_or_default();
    let (os, arch) = uname.split_once(' ').unwrap_or((uname, ""));
    if os != "Linux" {
        bail!("{host} runs {os}; subweave runs on Linux boxes");
    }
    let target = match arch.trim() {
        "x86_64" | "amd64" => "x86_64-unknown-linux-musl",
        "aarch64" | "arm64" => "aarch64-unknown-linux-musl",
        other => bail!("{host} is {other}; subweave runs on x86_64 and aarch64"),
    };
    let want = format!("subweave {VERSION}");
    if lines.any(|l| l.trim() == want) {
        return Ok(());
    }
    let bin = binary(target).await?;
    eprintln!("installing subweave {VERSION} on {host}");
    let tmp = format!("/tmp/subweave-{}", uuid::Uuid::now_v7());
    let status = Command::new("scp")
        .args(["-q", "-o", "BatchMode=yes"])
        .arg(&bin)
        .arg(format!("{host}:{tmp}"))
        .status()
        .context("running scp")?;
    if !status.success() {
        bail!("scp to {host} failed");
    }
    output(
        host,
        &format!("install -m 755 {tmp} {INSTALLED} && rm -f {tmp}"),
    )
    .with_context(|| format!("installing {INSTALLED} on {host}"))?;
    Ok(())
}

/// This version's binary for `target`: from `~/.cache/subweave/bin`, else downloaded from
/// the release and checked against its sha256.
async fn binary(target: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    let path = Path::new(&home)
        .join(".cache/subweave/bin")
        .join(VERSION)
        .join(target)
        .join("subweave");
    if path.is_file() {
        return Ok(path);
    }
    let url = format!("{RELEASES}/v{VERSION}/subweave-{target}");
    eprintln!("downloading {url}");
    let client = reqwest::Client::new();
    let get = |url: String| {
        let client = client.clone();
        async move {
            let resp = client
                .get(&url)
                .send()
                .await
                .with_context(|| format!("fetching {url}"))?;
            if !resp.status().is_success() {
                bail!("{url}: {}", resp.status());
            }
            Ok(resp.bytes().await?)
        }
    };
    let bytes = get(url.clone()).await?;
    let sum = get(format!("{url}.sha256")).await?;
    let want = String::from_utf8_lossy(&sum)
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let got = hex::encode(Sha256::digest(&bytes));
    if got != want {
        bail!("{url}: sha256 is {got}, the release says {want}");
    }
    let dir = path.parent().context("cache path")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    std::fs::write(&path, &bytes).with_context(|| format!("write {}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

/// Copies a directory to `/tmp/subweave-<uuid>/<its name>/` on the box. Gives that path
/// and the `/tmp/subweave-<uuid>` to remove later.
pub fn upload_dir(host: &str, dir: &Path) -> Result<(String, String)> {
    let dir = std::fs::canonicalize(dir).with_context(|| format!("read {}", dir.display()))?;
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .context("a weave directory needs a name")?;
    let tmp = format!("/tmp/subweave-{}", uuid::Uuid::now_v7());
    let remote = format!("{tmp}/{name}");
    let mut tar = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("-C")
        .arg(&dir)
        // macOS tar adds its extended attributes, which GNU tar on the box warns about.
        .args(["--no-xattrs", "-cf", "-", "."])
        .stdout(Stdio::piped())
        .spawn()
        .context("running tar")?;
    let stdout = tar.stdout.take().context("tar's output")?;
    let status = ssh(host, false)
        .arg(format!(
            "mkdir -p {q} && tar -xf - -C {q}",
            q = quote(&remote)
        ))
        .stdin(stdout)
        .status()
        .context("running ssh")?;
    let tarred = tar.wait()?;
    if !status.success() || !tarred.success() {
        bail!("copying {} to {host} failed", dir.display());
    }
    Ok((remote, tmp))
}

/// Gives the box `vars` in a file only this user reads, for the next line to source and
/// delete: no value appears in a command line. Gives the file's path.
pub fn send_env(host: &str, vars: &[(String, String)]) -> Result<String> {
    let path = format!("/tmp/subweave-{}.env", uuid::Uuid::now_v7());
    let body: String = vars
        .iter()
        .map(|(k, v)| format!("export {k}={}\n", quote(v)))
        .collect();
    let mut child = ssh(host, false)
        .arg(format!("umask 077 && cat > {}", quote(&path)))
        .stdin(Stdio::piped())
        .spawn()
        .context("running ssh")?;
    child
        .stdin
        .take()
        .context("ssh's stdin")?
        .write_all(body.as_bytes())?;
    if !child.wait()?.success() {
        bail!("sending the secrets to {host} failed");
    }
    Ok(path)
}

/// Gives the box each model's provider (when added here) and its key (when this machine
/// has one), with `subweave provider add` and `subweave key` on the box's stdin.
pub fn send_keys(host: &str, models: &[ModelRef]) -> Result<()> {
    let mut done = Vec::new();
    for m in models {
        if done.contains(&m.provider) {
            continue;
        }
        done.push(m.provider.clone());
        let p = provider::get(&m.provider)?;
        if p.name != provider::OPENROUTER {
            output(
                host,
                &command_line(&[
                    "provider".into(),
                    "add".into(),
                    p.name.clone(),
                    p.endpoint.clone(),
                    "--protocol".into(),
                    p.protocol.as_str().into(),
                ]),
            )?;
        }
        let Some(key) = p.key()? else {
            continue;
        };
        let mut child = ssh(host, false)
            .arg(command_line(&["key".into(), p.name.clone()]))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .context("running ssh")?;
        child
            .stdin
            .take()
            .context("ssh's stdin")?
            .write_all(key.as_bytes())?;
        if !child.wait()?.success() {
            bail!("storing the {} key on {host} failed", p.name);
        }
        eprintln!("copied the {} key to {host}", p.name);
    }
    Ok(())
}

/// `boxes.json`: service → `user@host`.
pub fn boxes() -> Result<BTreeMap<String, String>> {
    let file = paths::boxes();
    match std::fs::read(&file) {
        Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parse {}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("read {}", file.display())),
    }
}

fn save(boxes: &BTreeMap<String, String>) -> Result<()> {
    let file = paths::boxes();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(boxes)? + "\n";
    std::fs::write(&file, text).with_context(|| format!("write {}", file.display()))
}

/// The box that runs `service`, when this machine sent it there.
pub fn host_of(service: &str) -> Result<Option<String>> {
    Ok(boxes()?.remove(service))
}

pub fn remember(service: &str, host: &str) -> Result<()> {
    let mut b = boxes()?;
    if b.get(service).map(String::as_str) != Some(host) {
        b.insert(service.to_string(), host.to_string());
        save(&b)?;
    }
    Ok(())
}

pub fn forget(service: &str) -> Result<()> {
    let mut b = boxes()?;
    if b.remove(service).is_some() {
        save(&b)?;
    }
    Ok(())
}
