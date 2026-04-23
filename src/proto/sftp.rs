use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use parking_lot::Mutex;

use anyhow::{Context, Result, anyhow};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

use crate::core::connection::Connection;
use crate::proto::ssh::{HostKeyContext, connect_and_authenticate};

#[derive(Debug, Clone)]
pub struct SftpEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    pub permissions: Option<u32>,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub enum TransferDirection {
    Upload,
    Download,
}

pub enum SftpCommand {
    Connect,
    ListDir { path: String },
    Realpath { path: String },
    Mkdir { path: String },
    Rmdir { path: String },
    Remove { path: String },
    Rename { from: String, to: String },
    Upload { id: Uuid, local: PathBuf, remote: String },
    Download { id: Uuid, remote: String, local: PathBuf },
    CancelTransfer { id: Uuid },
    Disconnect,
}

pub enum SftpEvent {
    Connected { home: String },
    DirListing { path: String, entries: Vec<SftpEntry> },
    OperationOk { message: String },
    OperationError { message: String },
    TransferStarted {
        id: Uuid,
        direction: TransferDirection,
        label: String,
        total: Option<u64>,
    },
    TransferProgress { id: Uuid, bytes: u64, total: Option<u64> },
    TransferDone { id: Uuid },
    TransferFailed { id: Uuid, error: String },
    Closed(Option<String>),
}

pub struct SftpHandle {
    pub events: UnboundedReceiver<SftpEvent>,
    pub commands: UnboundedSender<SftpCommand>,
}

pub fn spawn_sftp_session(
    rt: &tokio::runtime::Handle,
    chain: Vec<Connection>,
    host_keys: HostKeyContext,
) -> SftpHandle {
    let (event_tx, event_rx) = unbounded_channel::<SftpEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<SftpCommand>();

    let event_tx_task = event_tx.clone();
    rt.spawn(async move {
        if let Err(e) = run_sftp_session(chain, host_keys, event_tx_task.clone(), cmd_rx).await {
            let _ = event_tx_task.send(SftpEvent::Closed(Some(format!("{e:#}"))));
        } else {
            let _ = event_tx_task.send(SftpEvent::Closed(None));
        }
    });

    SftpHandle {
        events: event_rx,
        commands: cmd_tx,
    }
}

async fn run_sftp_session(
    chain: Vec<Connection>,
    host_keys: HostKeyContext,
    events: UnboundedSender<SftpEvent>,
    mut commands: UnboundedReceiver<SftpCommand>,
) -> Result<()> {
    let handle = connect_and_authenticate(&chain, host_keys)
        .await
        .context("establishing ssh session for sftp")?;

    let channel = handle
        .channel_open_session()
        .await
        .context("opening session channel for sftp")?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .context("requesting sftp subsystem")?;
    let sftp = Arc::new(
        SftpSession::new(channel.into_stream())
            .await
            .context("initializing sftp session")?,
    );

    let home = sftp
        .canonicalize(".")
        .await
        .unwrap_or_else(|_| "/".to_string());
    let _ = events.send(SftpEvent::Connected { home: home.clone() });
    let _ = list_and_emit(&sftp, &home, &events).await;

    let cancels: Arc<Mutex<HashMap<Uuid, Arc<AtomicBool>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    while let Some(cmd) = commands.recv().await {
        match cmd {
            SftpCommand::Connect => {
                let _ = list_and_emit(&sftp, &home, &events).await;
            }
            SftpCommand::ListDir { path } => {
                let _ = list_and_emit(&sftp, &path, &events).await;
            }
            SftpCommand::Realpath { path } => match sftp.canonicalize(&path).await {
                Ok(resolved) => {
                    let _ = list_and_emit(&sftp, &resolved, &events).await;
                }
                Err(e) => {
                    let _ = events.send(SftpEvent::OperationError {
                        message: format!("realpath {path}: {e}"),
                    });
                }
            },
            SftpCommand::Mkdir { path } => match sftp.create_dir(&path).await {
                Ok(_) => {
                    let _ = events.send(SftpEvent::OperationOk {
                        message: format!("mkdir {path}"),
                    });
                }
                Err(e) => {
                    let _ = events.send(SftpEvent::OperationError {
                        message: format!("mkdir {path}: {e}"),
                    });
                }
            },
            SftpCommand::Rmdir { path } => match sftp.remove_dir(&path).await {
                Ok(_) => {
                    let _ = events.send(SftpEvent::OperationOk {
                        message: format!("rmdir {path}"),
                    });
                }
                Err(e) => {
                    let _ = events.send(SftpEvent::OperationError {
                        message: format!("rmdir {path}: {e}"),
                    });
                }
            },
            SftpCommand::Remove { path } => match sftp.remove_file(&path).await {
                Ok(_) => {
                    let _ = events.send(SftpEvent::OperationOk {
                        message: format!("rm {path}"),
                    });
                }
                Err(e) => {
                    let _ = events.send(SftpEvent::OperationError {
                        message: format!("rm {path}: {e}"),
                    });
                }
            },
            SftpCommand::Rename { from, to } => match sftp.rename(&from, &to).await {
                Ok(_) => {
                    let _ = events.send(SftpEvent::OperationOk {
                        message: format!("mv {from} -> {to}"),
                    });
                }
                Err(e) => {
                    let _ = events.send(SftpEvent::OperationError {
                        message: format!("mv {from} -> {to}: {e}"),
                    });
                }
            },
            SftpCommand::Upload { id, local, remote } => {
                let sftp = sftp.clone();
                let events = events.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                cancels.lock().insert(id, cancel.clone());
                let cancels_done = cancels.clone();
                tokio::spawn(async move {
                    let res = do_upload(sftp, id, local, remote, events.clone(), cancel.clone())
                        .await;
                    cancels_done.lock().remove(&id);
                    if let Err(e) = res {
                        let msg = if cancel.load(Ordering::Relaxed) {
                            "cancelled".to_string()
                        } else {
                            format!("{e:#}")
                        };
                        let _ = events.send(SftpEvent::TransferFailed { id, error: msg });
                    }
                });
            }
            SftpCommand::Download { id, remote, local } => {
                let sftp = sftp.clone();
                let events = events.clone();
                let cancel = Arc::new(AtomicBool::new(false));
                cancels.lock().insert(id, cancel.clone());
                let cancels_done = cancels.clone();
                tokio::spawn(async move {
                    let res = do_download(sftp, id, remote, local, events.clone(), cancel.clone())
                        .await;
                    cancels_done.lock().remove(&id);
                    if let Err(e) = res {
                        let msg = if cancel.load(Ordering::Relaxed) {
                            "cancelled".to_string()
                        } else {
                            format!("{e:#}")
                        };
                        let _ = events.send(SftpEvent::TransferFailed { id, error: msg });
                    }
                });
            }
            SftpCommand::CancelTransfer { id } => {
                if let Some(flag) = cancels.lock().get(&id).cloned() {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            SftpCommand::Disconnect => {
                break;
            }
        }
    }

    let _ = sftp.close().await;
    Ok(())
}

async fn list_and_emit(
    sftp: &SftpSession,
    path: &str,
    events: &UnboundedSender<SftpEvent>,
) -> Result<()> {
    match sftp.read_dir(path).await {
        Ok(read_dir) => {
            let mut entries: Vec<SftpEntry> = read_dir
                .map(|e| {
                    let meta = e.metadata();
                    let ft = meta.file_type();
                    SftpEntry {
                        name: e.file_name(),
                        is_dir: ft.is_dir(),
                        is_symlink: ft.is_symlink(),
                        size: meta.len(),
                        permissions: None,
                        modified: meta.modified().ok(),
                    }
                })
                .collect();
            entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });
            let _ = events.send(SftpEvent::DirListing {
                path: path.to_string(),
                entries,
            });
            Ok(())
        }
        Err(e) => {
            let _ = events.send(SftpEvent::OperationError {
                message: format!("ls {path}: {e}"),
            });
            Err(anyhow!("read_dir failed: {e}"))
        }
    }
}

async fn do_upload(
    sftp: Arc<SftpSession>,
    id: Uuid,
    local: PathBuf,
    remote: String,
    events: UnboundedSender<SftpEvent>,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let meta = tokio::fs::metadata(&local)
        .await
        .with_context(|| format!("stat local {}", local.display()))?;
    let is_dir = meta.is_dir();
    let label = format!(
        "{}{} -> {}",
        local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        if is_dir { "/" } else { "" },
        remote
    );
    let total = if is_dir {
        Some(walk_local_total(&local).await)
    } else {
        Some(meta.len())
    };
    let _ = events.send(SftpEvent::TransferStarted {
        id,
        direction: TransferDirection::Upload,
        label,
        total,
    });

    let mut sent: u64 = 0;
    if is_dir {
        upload_dir_rec(&sftp, id, &local, &remote, total, &mut sent, &events, &cancel).await?;
    } else {
        upload_file(&sftp, id, &local, &remote, total, &mut sent, &events, &cancel).await?;
    }
    let _ = events.send(SftpEvent::TransferDone { id });
    Ok(())
}

async fn upload_file(
    sftp: &SftpSession,
    id: Uuid,
    local: &PathBuf,
    remote: &str,
    total: Option<u64>,
    sent: &mut u64,
    events: &UnboundedSender<SftpEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow!("cancelled"));
    }
    let mut local_file = tokio::fs::File::open(local)
        .await
        .with_context(|| format!("opening local {}", local.display()))?;
    let mut remote_file = sftp
        .open_with_flags(
            remote.to_string(),
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .with_context(|| format!("creating remote {remote}"))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        let n = local_file.read(&mut buf).await.context("local read")?;
        if n == 0 {
            break;
        }
        remote_file
            .write_all(&buf[..n])
            .await
            .context("remote write")?;
        *sent += n as u64;
        let _ = events.send(SftpEvent::TransferProgress {
            id,
            bytes: *sent,
            total,
        });
    }
    remote_file.shutdown().await.context("closing remote file")?;
    Ok(())
}

fn upload_dir_rec<'a>(
    sftp: &'a SftpSession,
    id: Uuid,
    local: &'a PathBuf,
    remote: &'a str,
    total: Option<u64>,
    sent: &'a mut u64,
    events: &'a UnboundedSender<SftpEvent>,
    cancel: &'a Arc<AtomicBool>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        let _ = sftp.create_dir(remote.to_string()).await;
        let mut rd = tokio::fs::read_dir(local)
            .await
            .with_context(|| format!("reading local dir {}", local.display()))?;
        while let Some(entry) = rd.next_entry().await.context("dir iter")? {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("cancelled"));
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let child_local = entry.path();
            let child_remote = if remote.ends_with('/') {
                format!("{remote}{name}")
            } else {
                format!("{remote}/{name}")
            };
            let ft = entry.file_type().await.context("file_type")?;
            if ft.is_dir() {
                upload_dir_rec(
                    sftp,
                    id,
                    &child_local,
                    &child_remote,
                    total,
                    sent,
                    events,
                    cancel,
                )
                .await?;
            } else if ft.is_file() {
                upload_file(
                    sftp,
                    id,
                    &child_local,
                    &child_remote,
                    total,
                    sent,
                    events,
                    cancel,
                )
                .await?;
            }
        }
        Ok(())
    })
}

async fn ensure_local_dir(path: &PathBuf) -> Result<()> {
    match tokio::fs::metadata(path).await {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(anyhow!(
                "local path {} exists and is not a directory",
                path.display()
            ));
        }
        Err(_) => {}
    }
    match tokio::fs::create_dir_all(path).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            match tokio::fs::metadata(path).await {
                Ok(m) if m.is_dir() => Ok(()),
                _ => Err(anyhow!(
                    "mkdir local {} failed: AlreadyExists but not a directory",
                    path.display()
                )),
            }
        }
        Err(e) => Err(anyhow!("mkdir local {}: {e}", path.display())),
    }
}

async fn walk_local_total(root: &PathBuf) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.clone()];
    while let Some(p) = stack.pop() {
        let Ok(mut rd) = tokio::fs::read_dir(&p).await else {
            continue;
        };
        while let Ok(Some(entry)) = rd.next_entry().await {
            let Ok(ft) = entry.file_type().await else {
                continue;
            };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if let Ok(m) = entry.metadata().await {
                    total += m.len();
                }
            }
        }
    }
    total
}

async fn do_download(
    sftp: Arc<SftpSession>,
    id: Uuid,
    remote: String,
    local: PathBuf,
    events: UnboundedSender<SftpEvent>,
    cancel: Arc<AtomicBool>,
) -> Result<()> {
    let meta = sftp
        .metadata(remote.clone())
        .await
        .with_context(|| format!("stat remote {remote}"))?;
    let is_dir = meta.is_dir();
    let label = format!(
        "{}{} -> {}",
        remote,
        if is_dir { "/" } else { "" },
        local.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
    );
    let total = if is_dir {
        Some(walk_remote_total(&sftp, &remote).await)
    } else {
        Some(meta.len())
    };
    let _ = events.send(SftpEvent::TransferStarted {
        id,
        direction: TransferDirection::Download,
        label,
        total,
    });

    let mut got: u64 = 0;
    if is_dir {
        download_dir_rec(&sftp, id, &remote, &local, total, &mut got, &events, &cancel).await?;
    } else {
        download_file(&sftp, id, &remote, &local, total, &mut got, &events, &cancel).await?;
    }
    let _ = events.send(SftpEvent::TransferDone { id });
    Ok(())
}

async fn download_file(
    sftp: &SftpSession,
    id: Uuid,
    remote: &str,
    local: &PathBuf,
    total: Option<u64>,
    got: &mut u64,
    events: &UnboundedSender<SftpEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        return Err(anyhow!("cancelled"));
    }
    if let Some(parent) = local.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut remote_file = sftp
        .open(remote.to_string())
        .await
        .with_context(|| format!("opening remote {remote}"))?;
    let mut local_file = tokio::fs::File::create(local)
        .await
        .with_context(|| format!("creating local {}", local.display()))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        let n = remote_file.read(&mut buf).await.context("remote read")?;
        if n == 0 {
            break;
        }
        local_file
            .write_all(&buf[..n])
            .await
            .context("local write")?;
        *got += n as u64;
        let _ = events.send(SftpEvent::TransferProgress {
            id,
            bytes: *got,
            total,
        });
    }
    local_file.shutdown().await.context("closing local file")?;
    Ok(())
}

fn download_dir_rec<'a>(
    sftp: &'a SftpSession,
    id: Uuid,
    remote: &'a str,
    local: &'a PathBuf,
    total: Option<u64>,
    got: &'a mut u64,
    events: &'a UnboundedSender<SftpEvent>,
    cancel: &'a Arc<AtomicBool>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("cancelled"));
        }
        ensure_local_dir(local).await?;
        let read_dir = sftp
            .read_dir(remote.to_string())
            .await
            .with_context(|| format!("read_dir remote {remote}"))?;
        let entries: Vec<_> = read_dir.collect();
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(anyhow!("cancelled"));
            }
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child_remote = if remote.ends_with('/') {
                format!("{remote}{name}")
            } else {
                format!("{remote}/{name}")
            };
            let child_local = local.join(&name);
            let ft = entry.metadata().file_type();
            if ft.is_dir() {
                download_dir_rec(
                    sftp,
                    id,
                    &child_remote,
                    &child_local,
                    total,
                    got,
                    events,
                    cancel,
                )
                .await?;
            } else if ft.is_symlink() {
                continue;
            } else {
                download_file(
                    sftp,
                    id,
                    &child_remote,
                    &child_local,
                    total,
                    got,
                    events,
                    cancel,
                )
                .await?;
            }
        }
        Ok(())
    })
}

async fn walk_remote_total(sftp: &SftpSession, root: &str) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_string()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = sftp.read_dir(p.clone()).await else {
            continue;
        };
        for entry in rd {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let child = if p.ends_with('/') {
                format!("{p}{name}")
            } else {
                format!("{p}/{name}")
            };
            let meta = entry.metadata();
            let ft = meta.file_type();
            if ft.is_dir() {
                stack.push(child);
            } else if ft.is_file() {
                total += meta.len();
            }
        }
    }
    total
}
