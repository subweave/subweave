//! The two model roles: decide and code. The run picks the models; a Weavefile hints.

pub mod coder;
pub mod decider;
pub mod provider;

use std::fmt;

use anyhow::{Result, bail};
use serde::Serialize;

use crate::weave::ModelHint;

pub const DEFAULT_DECIDE: &str = "typesafe/jev-1.13";
pub const DEFAULT_CODE: &str = "deepseek/deepseek-v4-flash-0731";

/// `id[@provider]`; the provider defaults to `openrouter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelRef {
    pub id: String,
    pub provider: String,
}

impl ModelRef {
    pub fn parse(s: &str) -> Result<ModelRef> {
        let (id, provider) = match s.rsplit_once('@') {
            Some((id, p)) => (id, p),
            None => (s, provider::OPENROUTER),
        };
        if id.is_empty() || provider.is_empty() || id.contains(char::is_whitespace) {
            bail!("`{s}` is no model: use <id> or <id>@<provider>");
        }
        Ok(ModelRef {
            id: id.to_string(),
            provider: provider.to_string(),
        })
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.provider == provider::OPENROUTER {
            f.write_str(&self.id)
        } else {
            write!(f, "{}@{}", self.id, self.provider)
        }
    }
}

/// The decider: the `--decide` flag, else the weave's `MODEL DECIDE`, else the default.
pub fn decide_model(flag: Option<&str>, hint: &ModelHint) -> Result<ModelRef> {
    ModelRef::parse(flag.or(hint.decide.as_deref()).unwrap_or(DEFAULT_DECIDE))
}

/// The coder chain, in THEN order: the `--code` flags, else the weave's `MODEL CODE`,
/// else the default.
pub fn code_models(flags: &[String], hint: &ModelHint) -> Result<Vec<ModelRef>> {
    let ids: Vec<&str> = if !flags.is_empty() {
        flags.iter().map(String::as_str).collect()
    } else if !hint.code.is_empty() {
        hint.code.iter().map(String::as_str).collect()
    } else {
        vec![DEFAULT_CODE]
    };
    ids.into_iter().map(ModelRef::parse).collect()
}
