//! Encrypted secret store.
//!
//! On-disk format (v2):
//!
//! ```toml
//! version = 2
//! recipient = "age1..."                 # x25519 public key
//! wrapped_master_key = "<base64>"       # x25519 secret key, age-encrypted
//!                                       # under a scrypt-derived passphrase
//! [entries]
//! "password:<uuid>" = "<base64>"        # entry, age-encrypted to `recipient`
//! ```
//!
//! scrypt is a memory-hard KDF and intentionally slow, so it is run exactly
//! once per session — at unlock time — to recover the x25519 master key.
//! Per-entry encrypt/decrypt then uses cheap asymmetric crypto.
//!
//! Legacy v1 files (no `version` field, entries scrypt-encrypted directly) are
//! migrated transparently on first unlock.

use age::secrecy::{ExposeSecret, SecretString};
use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};
use uuid::Uuid;

const FILE: &str = "secrets.enc.toml";
const CURRENT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SecretKind {
    Password,
    Passphrase,
}

impl SecretKind {
    fn tag(self) -> &'static str {
        match self {
            SecretKind::Password => "password",
            SecretKind::Passphrase => "passphrase",
        }
    }
}

fn entry_key(kind: SecretKind, conn_id: Uuid) -> String {
    format!("{}:{}", kind.tag(), conn_id)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct OnDiskV2 {
    version: u32,
    recipient: String,
    wrapped_master_key: String,
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
struct OnDiskV1 {
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

pub struct SecretStore {
    path: PathBuf,
    identity: age::x25519::Identity,
    recipient: age::x25519::Recipient,
    wrapped_master_key: String,
    entries: BTreeMap<String, String>,
}

impl SecretStore {
    pub fn path_for(config_dir: &Path) -> PathBuf {
        config_dir.join(FILE)
    }

    pub fn file_exists(config_dir: &Path) -> bool {
        Self::path_for(config_dir).exists()
    }

    pub fn create(config_dir: &Path, passphrase: SecretString) -> Result<Self> {
        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public();
        let wrapped_master_key = wrap_master_key(&identity, passphrase)
            .context("wrapping fresh master key")?;
        let store = Self {
            path: Self::path_for(config_dir),
            identity,
            recipient,
            wrapped_master_key,
            entries: BTreeMap::new(),
        };
        store.save()?;
        Ok(store)
    }

    pub fn open(config_dir: &Path, passphrase: SecretString) -> Result<Self> {
        let path = Self::path_for(config_dir);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

        let probe: VersionProbe = toml::from_str(&text)
            .with_context(|| format!("probing {}", path.display()))?;

        match probe.version {
            Some(v) if v == CURRENT_VERSION => Self::open_v2(path, passphrase, &text),
            Some(v) => Err(anyhow!(
                "unsupported secret-store version {v} (expected {CURRENT_VERSION})"
            )),
            None => Self::open_v1_and_migrate(path, passphrase, &text),
        }
    }

    fn open_v2(path: PathBuf, passphrase: SecretString, text: &str) -> Result<Self> {
        let on_disk: OnDiskV2 = toml::from_str(text)
            .with_context(|| format!("parsing {}", path.display()))?;

        let wrapped = B64
            .decode(on_disk.wrapped_master_key.as_bytes())
            .context("decoding wrapped master key")?;
        let identity = unwrap_master_key(&wrapped, passphrase)?;
        let recipient = age::x25519::Recipient::from_str(&on_disk.recipient)
            .map_err(|e| anyhow!("parsing stored recipient: {e}"))?;

        // Refuse to proceed if the unwrapped private key doesn't match the
        // stored public key — file is corrupt or hand-edited and we'd
        // otherwise encrypt to a recipient we can't decrypt.
        if identity.to_public().to_string() != recipient.to_string() {
            return Err(anyhow!(
                "secret store corrupt: wrapped master key does not match stored recipient"
            ));
        }

        Ok(Self {
            path,
            identity,
            recipient,
            wrapped_master_key: on_disk.wrapped_master_key,
            entries: on_disk.entries,
        })
    }

    fn open_v1_and_migrate(
        path: PathBuf,
        passphrase: SecretString,
        text: &str,
    ) -> Result<Self> {
        let legacy: OnDiskV1 = toml::from_str(text)
            .with_context(|| format!("parsing legacy {}", path.display()))?;

        let mut plaintexts: BTreeMap<String, String> = BTreeMap::new();
        for (k, b64) in legacy.entries {
            let ct = B64
                .decode(b64.as_bytes())
                .with_context(|| format!("decoding ciphertext for {k}"))?;
            let scrypt_id = age::scrypt::Identity::new(passphrase.clone());
            let pt = age::decrypt(&scrypt_id, &ct)
                .map_err(|e| anyhow!("decryption failed (wrong master password?): {e}"))?;
            let pt = String::from_utf8(pt).context("secret is not valid UTF-8")?;
            plaintexts.insert(k, pt);
        }

        let identity = age::x25519::Identity::generate();
        let recipient = identity.to_public();
        let wrapped_master_key = wrap_master_key(&identity, passphrase)
            .context("wrapping master key during v1 -> v2 migration")?;

        let mut entries: BTreeMap<String, String> = BTreeMap::new();
        for (k, pt) in plaintexts {
            let ct = encrypt_to_recipient(&recipient, pt.as_bytes())
                .with_context(|| format!("re-encrypting entry {k} during migration"))?;
            entries.insert(k, B64.encode(ct));
        }

        let store = Self {
            path,
            identity,
            recipient,
            wrapped_master_key,
            entries,
        };
        store.save().context("persisting migrated v2 secret store")?;
        tracing::info!("migrated secret store to v2 format");
        Ok(store)
    }

    pub fn fetch(&self, kind: SecretKind, conn_id: Uuid) -> Option<String> {
        let key = entry_key(kind, conn_id);
        let b64 = self.entries.get(&key)?;
        let ct = match B64.decode(b64.as_bytes()) {
            Ok(b) => b,
            Err(err) => {
                tracing::warn!(?err, "failed to base64-decode entry {key}");
                return None;
            }
        };
        match decrypt_with_identity(&self.identity, &ct) {
            Ok(pt) => Some(pt),
            Err(err) => {
                tracing::warn!(?err, "failed to decrypt entry {key}");
                None
            }
        }
    }

    pub fn store(&mut self, kind: SecretKind, conn_id: Uuid, secret: &str) -> Result<()> {
        let key = entry_key(kind, conn_id);
        let ct = encrypt_to_recipient(&self.recipient, secret.as_bytes())
            .with_context(|| format!("encrypting entry {key}"))?;
        self.entries.insert(key, B64.encode(ct));
        self.save()
    }

    pub fn forget(&mut self, kind: SecretKind, conn_id: Uuid) {
        if self.forget_no_save(kind, conn_id)
            && let Err(err) = self.save()
        {
            tracing::warn!(?err, "failed persisting secret store after forget");
        }
    }

    pub fn forget_no_save(&mut self, kind: SecretKind, conn_id: Uuid) -> bool {
        self.entries.remove(&entry_key(kind, conn_id)).is_some()
    }

    pub fn save(&self) -> Result<()> {
        let on_disk = OnDiskV2 {
            version: CURRENT_VERSION,
            recipient: self.recipient.to_string(),
            wrapped_master_key: self.wrapped_master_key.clone(),
            entries: self.entries.clone(),
        };
        let text = toml::to_string_pretty(&on_disk).context("serializing secret store")?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&self.path, text)
            .with_context(|| format!("writing {}", self.path.display()))?;
        Ok(())
    }
}

fn wrap_master_key(identity: &age::x25519::Identity, passphrase: SecretString) -> Result<String> {
    let key_str = identity.to_string();
    let mut recipient = age::scrypt::Recipient::new(passphrase);
    // Pin scrypt work factor to ~1s on commodity hardware. age's autotune can
    // land at log_n high enough to make unlock take tens of seconds on some
    // machines; the master password lives only in human memory so 2^17 is the
    // sweet spot between brute-force resistance and unlock latency.
    recipient.set_work_factor(17);
    let encryptor = age::Encryptor::with_recipients(
        std::iter::once(&recipient as &dyn age::Recipient),
    )
    .map_err(|e| anyhow!("creating scrypt encryptor: {e}"))?;

    let mut wrapped: Vec<u8> = Vec::new();
    {
        let mut writer = encryptor
            .wrap_output(&mut wrapped)
            .map_err(|e| anyhow!("wrapping master key: {e}"))?;
        writer
            .write_all(key_str.expose_secret().as_bytes())
            .context("writing master key bytes")?;
        writer.finish().map_err(|e| anyhow!("finishing wrap: {e}"))?;
    }
    Ok(B64.encode(wrapped))
}

fn unwrap_master_key(wrapped: &[u8], passphrase: SecretString) -> Result<age::x25519::Identity> {
    let mut scrypt_id = age::scrypt::Identity::new(passphrase);
    // The wrapped key is our own file, not adversary input, so accept whatever
    // work factor age chose at encrypt time. age's default cap (target + 4) can
    // reject our own files on machines where target tuning lands near the edge.
    scrypt_id.set_max_work_factor(30);

    let decryptor = age::Decryptor::new(wrapped)
        .map_err(|e| anyhow!("opening wrapped key: {e}"))?;
    if !decryptor.is_scrypt() {
        return Err(anyhow!(
            "wrapped master key is not scrypt-encrypted (file may be corrupt)"
        ));
    }
    let mut reader = decryptor
        .decrypt(std::iter::once(&scrypt_id as &dyn age::Identity))
        .map_err(|e| anyhow!("decryption failed (wrong master password?): {e}"))?;
    let mut key_str = String::new();
    reader
        .read_to_string(&mut key_str)
        .context("reading unwrapped key")?;
    age::x25519::Identity::from_str(&key_str)
        .map_err(|e| anyhow!("parsing unwrapped x25519 key: {e}"))
}

fn encrypt_to_recipient(recipient: &age::x25519::Recipient, plaintext: &[u8]) -> Result<Vec<u8>> {
    let encryptor = age::Encryptor::with_recipients(
        std::iter::once(recipient as &dyn age::Recipient),
    )
    .map_err(|e| anyhow!("creating x25519 encryptor: {e}"))?;
    let mut ct = Vec::new();
    {
        let mut writer = encryptor
            .wrap_output(&mut ct)
            .map_err(|e| anyhow!("wrap_output: {e}"))?;
        writer.write_all(plaintext).context("writing plaintext")?;
        writer.finish().map_err(|e| anyhow!("finish encrypt: {e}"))?;
    }
    Ok(ct)
}

fn decrypt_with_identity(
    identity: &age::x25519::Identity,
    ciphertext: &[u8],
) -> Result<String> {
    let decryptor = age::Decryptor::new(ciphertext)
        .map_err(|e| anyhow!("opening entry: {e}"))?;
    let mut reader = decryptor
        .decrypt(std::iter::once(identity as &dyn age::Identity))
        .map_err(|e| anyhow!("entry decrypt: {e}"))?;
    let mut pt = String::new();
    reader
        .read_to_string(&mut pt)
        .context("reading entry plaintext")?;
    Ok(pt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn pw(s: &str) -> SecretString {
        SecretString::from(s.to_string())
    }

    #[test]
    fn create_store_and_round_trip() {
        let dir = tempdir().unwrap();
        let mut store = SecretStore::create(dir.path(), pw("hunter2hunter2")).unwrap();
        let id = Uuid::new_v4();
        store.store(SecretKind::Password, id, "s3cret").unwrap();
        assert_eq!(store.fetch(SecretKind::Password, id).as_deref(), Some("s3cret"));

        drop(store);
        let store = SecretStore::open(dir.path(), pw("hunter2hunter2")).unwrap();
        assert_eq!(store.fetch(SecretKind::Password, id).as_deref(), Some("s3cret"));
    }

    #[test]
    fn wrong_password_fails_to_open() {
        let dir = tempdir().unwrap();
        SecretStore::create(dir.path(), pw("correctpassword")).unwrap();
        let res = SecretStore::open(dir.path(), pw("wrongpassword"));
        assert!(res.is_err());
    }

    #[test]
    fn forget_removes_entry() {
        let dir = tempdir().unwrap();
        let mut store = SecretStore::create(dir.path(), pw("hunter2hunter2")).unwrap();
        let id = Uuid::new_v4();
        store.store(SecretKind::Password, id, "s3cret").unwrap();
        assert!(store.fetch(SecretKind::Password, id).is_some());
        store.forget(SecretKind::Password, id);
        assert!(store.fetch(SecretKind::Password, id).is_none());

        drop(store);
        let store = SecretStore::open(dir.path(), pw("hunter2hunter2")).unwrap();
        assert!(store.fetch(SecretKind::Password, id).is_none());
    }

    #[test]
    fn migrates_v1_to_v2() {
        let dir = tempdir().unwrap();
        let path = SecretStore::path_for(dir.path());
        let id = Uuid::new_v4();
        let key = entry_key(SecretKind::Password, id);

        let recipient = age::scrypt::Recipient::new(pw("legacypw"));
        let ct = age::encrypt(&recipient, b"legacysecret").unwrap();
        let mut entries = BTreeMap::new();
        entries.insert(key.clone(), B64.encode(ct));

        let mut text = String::from("[entries]\n");
        for (k, v) in &entries {
            text.push_str(&format!("\"{k}\" = \"{v}\"\n"));
        }
        std::fs::write(&path, text).unwrap();

        let store = SecretStore::open(dir.path(), pw("legacypw")).unwrap();
        assert_eq!(store.fetch(SecretKind::Password, id).as_deref(), Some("legacysecret"));

        let on_disk_text = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk_text.contains("version = 2"));
        assert!(on_disk_text.contains("wrapped_master_key"));
        assert!(on_disk_text.contains("recipient"));

        drop(store);
        let store = SecretStore::open(dir.path(), pw("legacypw")).unwrap();
        assert_eq!(store.fetch(SecretKind::Password, id).as_deref(), Some("legacysecret"));
    }
}
