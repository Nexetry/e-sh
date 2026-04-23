use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordingKind {
    Ssh,
    Sftp,
}

impl RecordingKind {
    pub fn label(self) -> &'static str {
        match self {
            RecordingKind::Ssh => "SSH",
            RecordingKind::Sftp => "SFTP",
        }
    }

    pub fn file_suffix(self) -> &'static str {
        match self {
            RecordingKind::Ssh => ".cast.gz",
            RecordingKind::Sftp => ".sftp.jsonl.gz",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingEntry {
    pub id: Uuid,
    #[serde(default)]
    pub connection_id: Option<Uuid>,
    pub connection_name: String,
    pub kind: RecordingKind,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    pub file: String,
    #[serde(default)]
    pub bytes_captured: u64,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ManifestStore {
    #[serde(default, rename = "recording")]
    pub entries: Vec<RecordingEntry>,
}

impl ManifestStore {
    pub fn manifest_path(recordings_dir: &Path) -> PathBuf {
        recordings_dir.join("recordings.toml")
    }

    pub fn load(recordings_dir: &Path) -> Result<Self> {
        let path = Self::manifest_path(recordings_dir);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let store: ManifestStore = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(store)
    }

    pub fn save(&self, recordings_dir: &Path) -> Result<()> {
        fs::create_dir_all(recordings_dir)
            .with_context(|| format!("creating {}", recordings_dir.display()))?;
        let path = Self::manifest_path(recordings_dir);
        let tmp = path.with_extension("toml.tmp");
        let text = toml::to_string_pretty(self).context("serializing recordings manifest")?;
        fs::write(&tmp, text)
            .with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn append(&mut self, entry: RecordingEntry, recordings_dir: &Path) -> Result<()> {
        self.entries.push(entry);
        self.save(recordings_dir)
    }

    pub fn update<F>(&mut self, id: Uuid, recordings_dir: &Path, f: F) -> Result<bool>
    where
        F: FnOnce(&mut RecordingEntry),
    {
        let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) else {
            return Ok(false);
        };
        f(entry);
        self.save(recordings_dir)?;
        Ok(true)
    }

    pub fn delete(&mut self, id: Uuid, recordings_dir: &Path) -> Result<bool> {
        let Some(idx) = self.entries.iter().position(|e| e.id == id) else {
            return Ok(false);
        };
        let entry = self.entries.remove(idx);
        let file_path = recordings_dir.join(&entry.file);
        if file_path.exists() {
            if let Err(err) = fs::remove_file(&file_path) {
                tracing::warn!(
                    ?err,
                    path = %file_path.display(),
                    "failed to remove recording file; manifest row still pruned",
                );
            }
        } else {
            tracing::warn!(
                path = %file_path.display(),
                "recording file already missing on delete",
            );
        }
        self.save(recordings_dir)?;
        Ok(true)
    }

    pub fn list(&self) -> &[RecordingEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_entry(kind: RecordingKind, ended: bool, partial: bool) -> RecordingEntry {
        RecordingEntry {
            id: Uuid::new_v4(),
            connection_id: Some(Uuid::new_v4()),
            connection_name: "test-conn".into(),
            kind,
            started_at: "2026-04-24T00:00:00Z".into(),
            ended_at: if ended { Some("2026-04-24T00:05:00Z".into()) } else { None },
            duration_ms: if ended { Some(300_000) } else { None },
            file: format!("{}{}", Uuid::new_v4(), kind.file_suffix()),
            bytes_captured: 4096,
            partial,
            notes: String::new(),
        }
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let store = ManifestStore::load(tmp.path()).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let mut store = ManifestStore::default();
        let e1 = sample_entry(RecordingKind::Ssh, true, false);
        let e2 = sample_entry(RecordingKind::Sftp, false, false);
        let e3 = sample_entry(RecordingKind::Ssh, true, true);
        store.entries.push(e1.clone());
        store.entries.push(e2.clone());
        store.entries.push(e3.clone());
        store.save(tmp.path()).unwrap();

        let loaded = ManifestStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.entries[0], e1);
        assert_eq!(loaded.entries[1], e2);
        assert_eq!(loaded.entries[2], e3);
    }

    #[test]
    fn append_updates_file_atomically() {
        let tmp = TempDir::new().unwrap();
        let mut store = ManifestStore::default();
        store
            .append(sample_entry(RecordingKind::Ssh, true, false), tmp.path())
            .unwrap();
        store
            .append(sample_entry(RecordingKind::Sftp, true, false), tmp.path())
            .unwrap();

        let reloaded = ManifestStore::load(tmp.path()).unwrap();
        assert_eq!(reloaded.entries.len(), 2);
        assert!(!ManifestStore::manifest_path(tmp.path())
            .with_extension("toml.tmp")
            .exists());
    }

    #[test]
    fn delete_removes_row_and_file() {
        let tmp = TempDir::new().unwrap();
        let mut store = ManifestStore::default();
        let entry = sample_entry(RecordingKind::Ssh, true, false);
        let file_path = tmp.path().join(&entry.file);
        fs::create_dir_all(tmp.path()).unwrap();
        fs::write(&file_path, b"fake gzip").unwrap();
        store.append(entry.clone(), tmp.path()).unwrap();

        assert!(file_path.exists());
        let removed = store.delete(entry.id, tmp.path()).unwrap();
        assert!(removed);
        assert!(!file_path.exists());
        assert!(store.entries.is_empty());

        let missing = sample_entry(RecordingKind::Ssh, true, false);
        store.append(missing.clone(), tmp.path()).unwrap();
        let removed2 = store.delete(missing.id, tmp.path()).unwrap();
        assert!(removed2);
        assert!(store.entries.is_empty());
    }

    #[test]
    fn update_mutates_in_place() {
        let tmp = TempDir::new().unwrap();
        let mut store = ManifestStore::default();
        let mut entry = sample_entry(RecordingKind::Ssh, false, false);
        entry.ended_at = None;
        entry.duration_ms = None;
        let id = entry.id;
        store.append(entry, tmp.path()).unwrap();

        let updated = store
            .update(id, tmp.path(), |e| {
                e.ended_at = Some("2026-04-24T00:10:00Z".into());
                e.duration_ms = Some(600_000);
                e.bytes_captured = 12_345;
            })
            .unwrap();
        assert!(updated);

        let reloaded = ManifestStore::load(tmp.path()).unwrap();
        assert_eq!(reloaded.entries.len(), 1);
        assert_eq!(reloaded.entries[0].ended_at.as_deref(), Some("2026-04-24T00:10:00Z"));
        assert_eq!(reloaded.entries[0].duration_ms, Some(600_000));
        assert_eq!(reloaded.entries[0].bytes_captured, 12_345);
    }

    #[test]
    fn legacy_connection_without_record_sessions_deserializes() {
        let legacy = r#"
id = "00000000-0000-0000-0000-000000000001"
name = "legacy"
protocol = "ssh"
host = "example.com"
port = 22
username = "root"
"#;
        let conn: crate::core::connection::Connection =
            toml::from_str(legacy).expect("legacy connection must parse");
        assert!(!conn.record_sessions, "missing field defaults to false");
    }
}
