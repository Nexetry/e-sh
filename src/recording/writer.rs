use anyhow::{Context, Result};
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

use crate::recording::manifest::{ManifestStore, RecordingEntry};

const FLUSH_BYTES: usize = 64 * 1024;
const FLUSH_INTERVAL_MS: u64 = 250;

pub enum RecorderCmd {
    Bytes(Vec<u8>),
    Finish,
}

pub struct WriterConfig {
    pub recording_id: Uuid,
    pub recordings_dir: PathBuf,
    pub entry: RecordingEntry,
    pub started: Instant,
    pub header: Vec<u8>,
}

pub struct WriterHandle {
    pub sender: UnboundedSender<RecorderCmd>,
    pub join: tokio::task::JoinHandle<()>,
}

pub fn spawn_file_writer(cfg: WriterConfig) -> Result<WriterHandle> {
    std::fs::create_dir_all(&cfg.recordings_dir)
        .with_context(|| format!("creating {}", cfg.recordings_dir.display()))?;
    let file_path = cfg.recordings_dir.join(&cfg.entry.file);
    let file = std::fs::File::create(&file_path)
        .with_context(|| format!("creating {}", file_path.display()))?;
    let buf = BufWriter::new(file);
    let gz: Box<dyn Write + Send> = Box::new(GzEncoder::new(buf, Compression::default()));
    Ok(spawn_with_writer(cfg, gz))
}

pub fn spawn_with_writer(cfg: WriterConfig, writer: Box<dyn Write + Send>) -> WriterHandle {
    let (tx, rx) = unbounded_channel();
    let sender = tx.clone();
    let join = tokio::spawn(async move {
        let cfg = cfg;
        if let Err(err) = run_writer(cfg, rx, writer).await {
            tracing::error!(?err, "recording writer task ended with error");
        }
    });
    WriterHandle { sender, join }
}

async fn run_writer(
    cfg: WriterConfig,
    mut rx: UnboundedReceiver<RecorderCmd>,
    writer: Box<dyn Write + Send>,
) -> Result<()> {
    let writer = Arc::new(parking_lot::Mutex::new(Some(writer)));
    let mut bytes_since_flush: usize = 0;
    let mut total_bytes: u64 = 0;
    let mut last_flush = Instant::now();
    let mut partial = false;

    if !cfg.header.is_empty() {
        let h = cfg.header.clone();
        let w = writer.clone();
        let res = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(w) = w.lock().as_mut() {
                w.write_all(&h)?;
            }
            Ok(())
        })
        .await;
        match res {
            Ok(Ok(())) => {
                total_bytes += cfg.header.len() as u64;
                bytes_since_flush += cfg.header.len();
            }
            Ok(Err(err)) => {
                tracing::error!(?err, "recording header write failed; marking partial");
                partial = true;
            }
            Err(err) => {
                tracing::error!(?err, "recording header join failed; marking partial");
                partial = true;
            }
        }
    }

    let mut finished_clean = false;
    loop {
        let timeout = Duration::from_millis(FLUSH_INTERVAL_MS);
        let cmd = tokio::time::timeout(timeout, rx.recv()).await;
        match cmd {
            Ok(Some(RecorderCmd::Bytes(buf))) => {
                if partial {
                    continue;
                }
                let len = buf.len();
                let w = writer.clone();
                let res = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                    if let Some(w) = w.lock().as_mut() {
                        w.write_all(&buf)?;
                    }
                    Ok(())
                })
                .await;
                match res {
                    Ok(Ok(())) => {
                        total_bytes += len as u64;
                        bytes_since_flush += len;
                        if bytes_since_flush >= FLUSH_BYTES {
                            flush_writer(&writer).await;
                            bytes_since_flush = 0;
                            last_flush = Instant::now();
                        }
                    }
                    Ok(Err(err)) => {
                        tracing::error!(?err, "mid-session write failed; marking partial");
                        partial = true;
                    }
                    Err(err) => {
                        tracing::error!(?err, "writer join failed; marking partial");
                        partial = true;
                    }
                }
            }
            Ok(Some(RecorderCmd::Finish)) => {
                finished_clean = !partial;
                break;
            }
            Ok(None) => {
                break;
            }
            Err(_) => {
                if bytes_since_flush > 0 && last_flush.elapsed() >= timeout {
                    flush_writer(&writer).await;
                    bytes_since_flush = 0;
                    last_flush = Instant::now();
                }
            }
        }
    }

    close_writer(&writer).await;
    let ended_at = chrono::Utc::now().to_rfc3339();
    let duration_ms = cfg.started.elapsed().as_millis() as u64;
    let recording_id = cfg.recording_id;
    let recordings_dir = cfg.recordings_dir.clone();
    let partial_final = partial || !finished_clean;

    let _ = tokio::task::spawn_blocking(move || -> Result<()> {
        let mut store = ManifestStore::load(&recordings_dir)?;
        store.update(recording_id, &recordings_dir, |entry| {
            entry.ended_at = Some(ended_at);
            entry.duration_ms = Some(duration_ms);
            entry.bytes_captured = total_bytes;
            entry.partial = partial_final;
        })?;
        Ok(())
    })
    .await;

    Ok(())
}

async fn flush_writer(writer: &Arc<parking_lot::Mutex<Option<Box<dyn Write + Send>>>>) {
    let w = writer.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Some(w) = w.lock().as_mut() {
            let _ = w.flush();
        }
    })
    .await;
}

async fn close_writer(writer: &Arc<parking_lot::Mutex<Option<Box<dyn Write + Send>>>>) {
    let w = writer.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let taken = w.lock().take();
        if let Some(mut w) = taken {
            let _ = w.flush();
            drop(w);
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::io::{ErrorKind, Read};
    use tempfile::TempDir;

    fn sample_cfg(dir: &std::path::Path, kind: crate::recording::manifest::RecordingKind) -> WriterConfig {
        let id = Uuid::new_v4();
        let file = format!("{}{}", id, kind.file_suffix());
        let entry = RecordingEntry {
            id,
            connection_id: None,
            connection_name: "t".into(),
            kind,
            started_at: chrono::Utc::now().to_rfc3339(),
            ended_at: None,
            duration_ms: None,
            file: file.clone(),
            bytes_captured: 0,
            partial: false,
            notes: String::new(),
        };
        let mut store = ManifestStore::default();
        store.append(entry.clone(), dir).unwrap();
        WriterConfig {
            recording_id: id,
            recordings_dir: dir.to_path_buf(),
            entry,
            started: Instant::now(),
            header: Vec::new(),
        }
    }

    #[tokio::test]
    async fn finish_closes_cleanly() {
        let tmp = TempDir::new().unwrap();
        let cfg = sample_cfg(tmp.path(), crate::recording::manifest::RecordingKind::Ssh);
        let id = cfg.recording_id;
        let file = tmp.path().join(&cfg.entry.file);
        let h = spawn_file_writer(cfg).unwrap();
        h.sender.send(RecorderCmd::Bytes(b"hello world\n".to_vec())).unwrap();
        h.sender.send(RecorderCmd::Finish).unwrap();
        h.join.await.unwrap();

        let data = std::fs::read(&file).unwrap();
        let mut dec = GzDecoder::new(&data[..]);
        let mut out = String::new();
        dec.read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello world\n");

        let store = ManifestStore::load(tmp.path()).unwrap();
        let entry = store.entries.iter().find(|e| e.id == id).unwrap();
        assert!(entry.ended_at.is_some());
        assert!(entry.duration_ms.is_some());
        assert!(!entry.partial);
        assert_eq!(entry.bytes_captured, 12);
    }

    #[tokio::test]
    async fn writer_flushes_every_64kb() {
        let tmp = TempDir::new().unwrap();
        let cfg = sample_cfg(tmp.path(), crate::recording::manifest::RecordingKind::Ssh);
        let file = tmp.path().join(&cfg.entry.file);
        let h = spawn_file_writer(cfg).unwrap();
        let chunk = vec![b'x'; 70 * 1024];
        h.sender.send(RecorderCmd::Bytes(chunk)).unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        let size = std::fs::metadata(&file).unwrap().len();
        assert!(size > 0, "file should be flushed after crossing 64KB boundary");

        h.sender.send(RecorderCmd::Finish).unwrap();
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn writer_flushes_every_250ms() {
        let tmp = TempDir::new().unwrap();
        let cfg = sample_cfg(tmp.path(), crate::recording::manifest::RecordingKind::Ssh);
        let file = tmp.path().join(&cfg.entry.file);
        let h = spawn_file_writer(cfg).unwrap();
        h.sender.send(RecorderCmd::Bytes(b"tiny".to_vec())).unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;

        let size = std::fs::metadata(&file).unwrap().len();
        assert!(size > 0, "file should be flushed after 250ms timer");

        h.sender.send(RecorderCmd::Finish).unwrap();
        h.join.await.unwrap();
    }

    struct BrokenWriter;
    impl Write for BrokenWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(ErrorKind::BrokenPipe, "mock broken"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn mid_session_io_error_sets_partial() {
        let tmp = TempDir::new().unwrap();
        let cfg = sample_cfg(tmp.path(), crate::recording::manifest::RecordingKind::Ssh);
        let id = cfg.recording_id;
        let h = spawn_with_writer(cfg, Box::new(BrokenWriter));
        h.sender.send(RecorderCmd::Bytes(b"x".to_vec())).unwrap();
        h.sender.send(RecorderCmd::Finish).unwrap();
        h.join.await.unwrap();

        let store = ManifestStore::load(tmp.path()).unwrap();
        let entry = store.entries.iter().find(|e| e.id == id).unwrap();
        assert!(entry.partial, "BrokenPipe mid-session must mark partial");
        assert!(entry.ended_at.is_some());
    }
}
