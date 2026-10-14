//! Where a weave comes from: a local directory, or `name[:tag]` in the repository.

use std::path::Path;

use anyhow::{Context, Result, bail};

const RAW: &str = "https://raw.githubusercontent.com/subweave/subweave";

/// Returns the weave's name and its Weavefile's text.
///
/// A path to a directory holding `Weavefile`, or to the file itself, names the weave
/// after the directory. Otherwise `name[:tag]` is read from the repository at the git
/// tag `<name>-<tag>`, or `main` without a tag.
pub async fn resolve(spec: &str) -> Result<(String, String)> {
    let path = Path::new(spec);
    if path.exists() {
        let file = if path.is_dir() {
            path.join("Weavefile")
        } else {
            path.to_path_buf()
        };
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let dir =
            std::fs::canonicalize(&file).with_context(|| format!("reading {}", file.display()))?;
        let name = dir
            .parent()
            .and_then(|d| d.file_name())
            .and_then(|n| n.to_str())
            .context("a Weavefile needs a directory named after the weave")?
            .to_string();
        return Ok((name, text));
    }
    let (name, tag) = match spec.split_once(':') {
        Some((n, t)) => (n, Some(t)),
        None => (spec, None),
    };
    let valid = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    };
    if !valid(name) || !tag.is_none_or(valid) {
        bail!("no weave at {spec}: give a directory, or a name like postgres or postgres:0.5");
    }
    let git_ref = match tag {
        Some(t) => format!("{name}-{t}"),
        None => "main".to_string(),
    };
    let url = format!("{RAW}/{git_ref}/examples/{name}/Weavefile");
    let mut req = reqwest::Client::new().get(&url);
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        req = req.bearer_auth(token);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("no weave {spec} in github.com/subweave/subweave ({url})");
    }
    if !resp.status().is_success() {
        bail!("fetching {url}: HTTP {}", resp.status());
    }
    let text = resp
        .text()
        .await
        .with_context(|| format!("fetching {url}"))?;
    Ok((name.to_string(), text))
}
