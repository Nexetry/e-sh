use anyhow::Result;
use std::path::Path;
use std::time::Instant;
use uuid::Uuid;

use crate::core::connection::Connection;
use crate::recording::asciicast;
use crate::recording::manifest::{ManifestStore, RecordingEntry, RecordingKind};
use crate::recording::writer::{RecorderCmd, WriterConfig, spawn_file_writer};
use crate::recording::{Recorder, SftpResult};

pub struct StartParams<'a> {
    pub conn: &'a Connection,
    pub recordings_dir: &'a Path,
    pub kind: RecordingKind,
    pub width: u16,
    pub height: u16,
    pub term: &'a str,
}

pub fn start_recording(params: StartParams<'_>) -> Result<Recorder> {
    let id = Uuid::new_v4();
    let file = format!("{}{}", id, params.kind.file_suffix());
    let started_at = chrono::Utc::now().to_rfc3339();
    let entry = RecordingEntry {
        id,
        connection_id: Some(params.conn.id),
        connection_name: params.conn.name.clone(),
        kind: params.kind,
        started_at,
        ended_at: None,
        duration_ms: None,
        file: file.clone(),
        bytes_captured: 0,
        partial: false,
        notes: String::new(),
    };

    let mut store = ManifestStore::load(params.recordings_dir)?;
    store.append(entry.clone(), params.recordings_dir)?;

    let header = match params.kind {
        RecordingKind::Ssh => asciicast::encode_header(
            params.width,
            params.height,
            chrono::Utc::now().timestamp(),
            params.term,
            &params.conn.name,
        ),
        RecordingKind::Sftp => Vec::new(),
    };

    let cfg = WriterConfig {
        recording_id: id,
        recordings_dir: params.recordings_dir.to_path_buf(),
        entry,
        started: Instant::now(),
        header,
    };
    let handle = spawn_file_writer(cfg)?;
    Ok(Recorder::new(id, handle.sender, params.kind, Instant::now()))
}

impl Recorder {
    pub fn ssh_output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let t = self.started.elapsed().as_secs_f64();
        let event = asciicast::encode_event(t, "o", bytes);
        let _ = self.sender.send(RecorderCmd::Bytes(event));
    }

    pub fn sftp_event(&self, op: &str, result: SftpResult, extra: serde_json::Value) {
        let t = self.started.elapsed().as_secs_f64();
        let event = crate::recording::sftp_log::encode_event(
            t,
            op,
            match result {
                SftpResult::Ok => crate::recording::sftp_log::SftpResult::Ok,
                SftpResult::Error => crate::recording::sftp_log::SftpResult::Error,
            },
            extra,
        );
        let _ = self.sender.send(RecorderCmd::Bytes(event));
    }

    pub fn finish(self) {
        let _ = self.sender.send(RecorderCmd::Finish);
    }

    pub fn finish_shared(&self) {
        let _ = self.sender.send(RecorderCmd::Finish);
    }
}
