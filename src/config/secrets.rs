use age::secrecy::SecretString;
use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const FILE: &str = "secrets.enc.toml";

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
struct OnDisk {
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

pub struct SecretStore {
    path: PathBuf,
    passphrase: SecretString,
    plaintext: BTreeMap<String, String>,
}

impl SecretStore {
    pub fn path_for(config_dir: &Path) -> PathBuf {
        config_dir.join(FILE)
    }

    pub fn file_exists(config_dir: &Path) -> bool {
        Self::path_for(config_dir).exists()
    }

    pub fn create(config_dir: &Path, passphrase: SecretString) -> Result<Self> {
        let store = Self {
            path: Self::path_for(config_dir),
            passphrase,
            plaintext: BTreeMap::new(),
        };
        store.save()?;
        Ok(store)
    }

    pub fn open(config_dir: &Path, passphrase: SecretString) -> Result<Self> {
        let path = Self::path_for(config_dir);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let on_disk: OnDisk =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let mut plaintext = BTreeMap::new();
        for (k, b64) in on_disk.entries {
            let ct = B64
                .decode(b64.as_bytes())
                .with_context(|| format!("decoding ciphertext for {k}"))?;
            let identity = age::scrypt::Identity::new(passphrase.clone());
            let pt = age::decrypt(&identity, &ct)
                .map_err(|e| anyhow!("decryption failed (wrong master password?): {e}"))?;
            let pt = String::from_utf8(pt).context("secret is not valid UTF-8")?;
            plaintext.insert(k, pt);
        }
        Ok(Self {
            path,
            passphrase,
            plaintext,
        })
    }

    pub fn fetch(&self, kind: SecretKind, conn_id: Uuid) -> Option<String> {
        self.plaintext.get(&entry_key(kind, conn_id)).cloned()
    }

    pub fn store(&mut self, kind: SecretKind, conn_id: Uuid, secret: &str) -> Result<()> {
        self.plaintext
            .insert(entry_key(kind, conn_id), secret.to_string());
        self.save()
    }

    pub fn forget(&mut self, kind: SecretKind, conn_id: Uuid) {
        if self.plaintext.remove(&entry_key(kind, conn_id)).is_some()
            && let Err(err) = self.save()
        {
            tracing::warn!(?err, "failed persisting secret store after forget");
        }
    }

    fn save(&self) -> Result<()> {
        let mut entries: BTreeMap<String, String> = BTreeMap::new();
        for (k, pt) in &self.plaintext {
            let recipient = age::scrypt::Recipient::new(self.passphrase.clone());
            let ct =
                age::encrypt(&recipient, pt.as_bytes()).map_err(|e| anyhow!("encrypt: {e}"))?;
            entries.insert(k.clone(), B64.encode(ct));
        }
        let on_disk = OnDisk { entries };
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
