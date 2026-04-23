pub mod asciicast;
pub mod config;
pub mod manifest;
pub mod sftp_log;
pub mod writer;

use std::time::Instant;
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

use crate::recording::manifest::RecordingKind;
use crate::recording::writer::RecorderCmd;

pub use config::{StartParams, start_recording};
pub use manifest::{ManifestStore, RecordingEntry, RecordingKind as Kind};

#[derive(Debug, Clone, Copy)]
pub enum SftpResult {
    Ok,
    Error,
}

pub struct Recorder {
    id: Uuid,
    sender: UnboundedSender<RecorderCmd>,
    kind: RecordingKind,
    started: Instant,
}

impl Recorder {
    pub(crate) fn new(
        id: Uuid,
        sender: UnboundedSender<RecorderCmd>,
        kind: RecordingKind,
        started: Instant,
    ) -> Self {
        Self { id, sender, kind, started }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn kind(&self) -> RecordingKind {
        self.kind
    }
}
