//! Model providers: a name, an endpoint, a protocol and a key.

use std::fmt;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths;

pub const OPENROUTER: &str = "openrouter";
const OPENROUTER_ENDPOINT: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    #[serde(rename = "openai")]
    OpenAi,
    #[serde(rename = "anthropic")]
    Anthropic,
}

impl Protocol {
    pub fn as_str(self) -> &'static str {
        match self {
            Protocol::OpenAi => "openai",
            Protocol::Anthropic => "anthropic",
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    pub endpoint: String,
    pub protocol: Protocol,
}

/// Where a provider's key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    File,
    None,
}

impl Provider {
    pub fn openrouter() -> Provider {
        Provider {
            name: OPENROUTER.to_string(),
            endpoint: OPENROUTER_ENDPOINT.to_string(),
            protocol: Protocol::OpenAi,
        }
    }

    /// `<endpoint>/<path>`, with one slash between them.
    pub fn url(&self, path: &str) -> String {
        format!("{}/{}", self.endpoint.trim_end_matches('/'), path)
    }

    /// The key: env `<NAME>_API_KEY`, else `config/keys/<name>`, else none.
    pub fn key(&self) -> Result<Option<String>> {
        Ok(key_with_source(&self.name)?.0)
    }

    pub fn key_source(&self) -> Result<KeySource> {
        Ok(key_with_source(&self.name)?.1)
    }
}

/// `gpu-1` reads `GPU_1_API_KEY`.
pub fn env_var(name: &str) -> String {
    format!("{}_API_KEY", name.to_uppercase().replace('-', "_"))
}

fn key_with_source(name: &str) -> Result<(Option<String>, KeySource)> {
    if let Ok(k) = std::env::var(env_var(name)) {
        if !k.trim().is_empty() {
            return Ok((Some(k.trim().to_string()), KeySource::Env));
        }
    }
    let file = paths::key(name);
    match std::fs::read_to_string(&file) {
        Ok(k) if !k.trim().is_empty() => Ok((Some(k.trim().to_string()), KeySource::File)),
        Ok(_) => Ok((None, KeySource::None)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, KeySource::None)),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

/// The providers added with `provider add`, without the built-in one.
fn added() -> Result<Vec<Provider>> {
    let file = paths::providers();
    match std::fs::read_to_string(&file) {
        Ok(text) => {
            serde_json::from_str(&text).with_context(|| format!("reading {}", file.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
    }
}

fn save(list: &[Provider]) -> Result<()> {
    let file = paths::providers();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(list)? + "\n";
    std::fs::write(&file, text).with_context(|| format!("writing {}", file.display()))
}

/// Every provider: `openrouter` first, then the added ones.
pub fn all() -> Result<Vec<Provider>> {
    let mut list = vec![Provider::openrouter()];
    list.extend(added()?);
    Ok(list)
}

pub fn get(name: &str) -> Result<Provider> {
    all()?
        .into_iter()
        .find(|p| p.name == name)
        .with_context(|| {
            format!("no provider {name}: add it with subweave provider add {name} <endpoint>")
        })
}

/// Adds a provider, or changes the one with that name.
pub fn add(p: Provider) -> Result<()> {
    if p.name == OPENROUTER {
        bail!("openrouter is built in");
    }
    let valid = !p.name.is_empty()
        && p.name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !valid {
        bail!("a provider name takes lowercase letters, digits, `-` and `_`");
    }
    if !(p.endpoint.starts_with("https://") || p.endpoint.starts_with("http://")) {
        bail!("the endpoint is an http:// or https:// URL");
    }
    let mut list = added()?;
    match list.iter_mut().find(|q| q.name == p.name) {
        Some(q) => *q = p,
        None => list.push(p),
    }
    save(&list)
}

pub fn remove(name: &str) -> Result<()> {
    if name == OPENROUTER {
        bail!("openrouter is built in");
    }
    let mut list = added()?;
    let before = list.len();
    list.retain(|p| p.name != name);
    if list.len() == before {
        bail!("no provider {name}");
    }
    save(&list)?;
    match std::fs::remove_file(paths::key(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", paths::key(name).display())),
    }
}

/// Writes `config/keys/<provider>` with mode 0600.
pub fn store_key(name: &str, key: &str) -> Result<std::path::PathBuf> {
    get(name)?;
    let dir = paths::keys();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("setting the mode of {}", dir.display()))?;
    let file = paths::key(name);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&file)
        .with_context(|| format!("writing {}", file.display()))?;
    // The mode applies only to a new file; an older one may be wider.
    f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    f.write_all(key.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(file)
}
