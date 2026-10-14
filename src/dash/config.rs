//! `config()/dash.json`: where the dashboard listens, the host names it answers to, and
//! its password's argon2id hash.

use std::net::IpAddr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bind: String,
    pub port: u16,
    /// An argon2id PHC string. The dashboard does not start without one.
    pub password: Option<String>,
    /// Names the `Host` header may carry besides the bind address, loopback names and
    /// the box's own IP addresses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: "127.0.0.1".into(),
            port: 7400,
            password: None,
            hosts: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Config> {
        let path = paths::dash();
        match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("parse {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let mut bytes = serde_json::to_vec_pretty(self)?;
        bytes.push(b'\n');
        crate::channel::write_private(&paths::dash(), &bytes)
    }

    pub fn addr(&self) -> Result<IpAddr> {
        self.bind
            .parse()
            .with_context(|| format!("{:?} is no IP address", self.bind))
    }

    /// The password hash, or why the dashboard cannot start: it can run commands as root.
    pub fn need_password(&self) -> Result<&str> {
        match self.password.as_deref() {
            Some(p) => Ok(p),
            None => bail!("the dashboard has no password: set one first: subweave dash --password"),
        }
    }
}

/// The argon2id PHC string of a password.
pub fn hash(password: &str) -> Result<String> {
    use argon2::password_hash::{PasswordHasher, SaltString, rand_core::OsRng};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("hashing the password: {e}"))
}

pub fn verify(password: &str, phc: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(phc).is_ok_and(|h| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &h)
            .is_ok()
    })
}
