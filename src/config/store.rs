use anyhow::{Context, Result};
use directories::ProjectDirs;
use std::{fs, path::PathBuf};

use crate::config::secrets::{SecretKind, SecretStore};
use crate::core::connection::{AuthMethod, Connection, ConnectionStore};

const QUALIFIER: &str = "com";
const ORG: &str = "nexetry";
const APP: &str = "e-sh";
const FILE: &str = "connections.toml";

pub struct ConfigPaths {
    pub config_dir: PathBuf,
    pub connections_file: PathBuf,
}

impl ConfigPaths {
    pub fn discover() -> Result<Self> {
        let dirs = ProjectDirs::from(QUALIFIER, ORG, APP)
            .context("could not determine OS config directory")?;
        let config_dir = dirs.config_dir().to_path_buf();
        Ok(Self {
            connections_file: config_dir.join(FILE),
            config_dir,
        })
    }
}

pub fn load_connections(paths: &ConfigPaths) -> Result<ConnectionStore> {
    if !paths.connections_file.exists() {
        return Ok(ConnectionStore::default());
    }
    let text = fs::read_to_string(&paths.connections_file)
        .with_context(|| format!("reading {}", paths.connections_file.display()))?;
    let store: ConnectionStore = toml::from_str(&text)
        .with_context(|| format!("parsing {}", paths.connections_file.display()))?;
    Ok(store)
}

pub fn save_connections(
    paths: &ConfigPaths,
    store: &ConnectionStore,
    secrets: &mut SecretStore,
) -> Result<()> {
    fs::create_dir_all(&paths.config_dir)
        .with_context(|| format!("creating {}", paths.config_dir.display()))?;

    for conn in &store.connections {
        persist_secrets(conn, secrets);
    }

    let mut sanitized = store.clone();
    for conn in &mut sanitized.connections {
        sanitize_secrets(conn);
    }

    let text = toml::to_string_pretty(&sanitized).context("serializing connections")?;
    fs::write(&paths.connections_file, text)
        .with_context(|| format!("writing {}", paths.connections_file.display()))?;
    Ok(())
}

pub fn forget_secrets(conn: &Connection, secrets: &mut SecretStore) {
    secrets.forget(SecretKind::Password, conn.id);
    secrets.forget(SecretKind::Passphrase, conn.id);
}

pub fn hydrate_after_unlock(
    paths: &ConfigPaths,
    store: &mut ConnectionStore,
    secrets: &mut SecretStore,
) -> Result<()> {
    let mut needs_resave = false;
    for conn in &mut store.connections {
        if hydrate_secrets(conn, secrets) {
            needs_resave = true;
        }
    }
    if needs_resave {
        save_connections(paths, store, secrets)?;
    }
    Ok(())
}

fn hydrate_secrets(conn: &mut Connection, secrets: &mut SecretStore) -> bool {
    let mut migrated = false;
    match &mut conn.auth {
        AuthMethod::Password { password } => {
            if password.is_empty() {
                if let Some(stored) = secrets.fetch(SecretKind::Password, conn.id) {
                    *password = stored;
                }
            } else if let Err(err) = secrets.store(SecretKind::Password, conn.id, password) {
                tracing::warn!(?err, "failed to migrate plaintext password into secret store");
            } else {
                migrated = true;
            }
        }
        AuthMethod::PublicKey { passphrase, .. } => match passphrase {
            Some(value) if !value.is_empty() => {
                if let Err(err) = secrets.store(SecretKind::Passphrase, conn.id, value) {
                    tracing::warn!(?err, "failed to migrate plaintext passphrase into secret store");
                } else {
                    migrated = true;
                }
            }
            _ => {
                if let Some(stored) = secrets.fetch(SecretKind::Passphrase, conn.id) {
                    *passphrase = Some(stored);
                }
            }
        },
        AuthMethod::Agent => {}
    }
    migrated
}

fn persist_secrets(conn: &Connection, secrets: &mut SecretStore) {
    match &conn.auth {
        AuthMethod::Password { password } if !password.is_empty() => {
            if let Err(err) = secrets.store(SecretKind::Password, conn.id, password) {
                tracing::warn!(?err, "failed to write password to secret store");
            }
        }
        AuthMethod::Password { .. } => {
            secrets.forget(SecretKind::Password, conn.id);
        }
        AuthMethod::PublicKey { passphrase, .. } => match passphrase {
            Some(value) if !value.is_empty() => {
                if let Err(err) = secrets.store(SecretKind::Passphrase, conn.id, value) {
                    tracing::warn!(?err, "failed to write passphrase to secret store");
                }
            }
            _ => {
                secrets.forget(SecretKind::Passphrase, conn.id);
            }
        },
        AuthMethod::Agent => {
            secrets.forget(SecretKind::Password, conn.id);
            secrets.forget(SecretKind::Passphrase, conn.id);
        }
    }
}

fn sanitize_secrets(conn: &mut Connection) {
    match &mut conn.auth {
        AuthMethod::Password { password } => password.clear(),
        AuthMethod::PublicKey { passphrase, .. } => *passphrase = None,
        AuthMethod::Agent => {}
    }
}
