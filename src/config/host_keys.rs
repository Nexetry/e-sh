use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use tokio::sync::oneshot;

use crate::config::store::ConfigPaths;

const HOST_KEYS_FILE: &str = "host_keys.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostKeyEntry {
    pub algorithm: String,
    pub fingerprint: String,
    #[serde(default)]
    pub first_seen: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct HostKeyStore {
    #[serde(default)]
    pub hosts: BTreeMap<String, HostKeyEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyVerdict {
    KnownMatch,
    NewHost {
        algorithm: String,
        fingerprint: String,
    },
    Mismatch {
        expected: HostKeyEntry,
        actual_algorithm: String,
        actual_fingerprint: String,
    },
}

impl HostKeyStore {
    pub fn host_keys_path(paths: &ConfigPaths) -> PathBuf {
        paths.config_dir.join(HOST_KEYS_FILE)
    }

    pub fn load(paths: &ConfigPaths) -> Result<Self> {
        let path = Self::host_keys_path(paths);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let store: Self = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(store)
    }

    pub fn save(&self, paths: &ConfigPaths) -> Result<()> {
        fs::create_dir_all(&paths.config_dir)
            .with_context(|| format!("creating {}", paths.config_dir.display()))?;
        let path = Self::host_keys_path(paths);
        let text = toml::to_string_pretty(self).context("serializing host keys")?;
        fs::write(&path, text)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn host_id(host: &str, port: u16) -> String {
        format!("{host}:{port}")
    }

    pub fn check(&self, host: &str, port: u16, algorithm: &str, fingerprint: &str) -> HostKeyVerdict {
        let id = Self::host_id(host, port);
        match self.hosts.get(&id) {
            None => HostKeyVerdict::NewHost {
                algorithm: algorithm.to_string(),
                fingerprint: fingerprint.to_string(),
            },
            Some(entry) if entry.fingerprint == fingerprint && entry.algorithm == algorithm => {
                HostKeyVerdict::KnownMatch
            }
            Some(entry) => HostKeyVerdict::Mismatch {
                expected: entry.clone(),
                actual_algorithm: algorithm.to_string(),
                actual_fingerprint: fingerprint.to_string(),
            },
        }
    }

    pub fn insert(&mut self, host: &str, port: u16, algorithm: String, fingerprint: String) {
        let id = Self::host_id(host, port);
        let first_seen = current_timestamp();
        self.hosts.insert(
            id,
            HostKeyEntry {
                algorithm,
                fingerprint,
                first_seen,
            },
        );
    }
}

fn current_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("epoch:{secs}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyPromptKind {
    NewHost,
    Mismatch { expected: HostKeyEntry },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    AcceptAndSave,
    AcceptOnce,
    Reject,
}

pub struct HostKeyPrompt {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub kind: HostKeyPromptKind,
    pub responder: oneshot::Sender<HostKeyDecision>,
}
