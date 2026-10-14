//! `config()/dash.json`: where the dashboard listens and its password's argon2id hash.

use std::net::IpAddr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bind: String,
    pub port: u16,
    /// An argon2id PHC string; `None`: no login, loopback only.
    pub password: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: "127.0.0.1".into(),
            port: 7400,
            password: None,
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

    /// The bind address; one that is not loopback needs a password: the dashboard can
    /// run commands as root.
    pub fn addr(&self) -> Result<IpAddr> {
        let ip: IpAddr = self
            .bind
            .parse()
            .with_context(|| format!("{:?} is no IP address", self.bind))?;
        if !ip.is_loopback() && self.password.is_none() {
            bail!(
                "a dashboard on {ip} needs a password: subweave dash --password. It can run commands as root."
            );
        }
        Ok(ip)
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
