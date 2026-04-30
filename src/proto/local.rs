use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;

use parking_lot::Mutex;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc::unbounded_channel;

use super::ssh::{SessionCommand, SessionEvent, SessionHandle, TunnelStatusMap};

/// Spawn a local shell (e.g. bash/zsh) in a pseudo-terminal and return a
/// `SessionHandle` that speaks the same protocol as an SSH session.
pub fn spawn_local_shell(rt: &tokio::runtime::Handle) -> SessionHandle {
    let (event_tx, event_rx) = unbounded_channel::<SessionEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<SessionCommand>();
    let tunnels: TunnelStatusMap = Arc::new(Mutex::new(HashMap::new()));

    let event_tx_task = event_tx.clone();
    // Run the blocking PTY I/O on a dedicated thread (not a tokio task)
    // because portable-pty's reader/writer are synchronous.
    std::thread::Builder::new()
        .name("local-shell".into())
        .spawn(move || {
            if let Err(e) = run_local_shell(event_tx_task.clone(), cmd_rx) {
                let _ = event_tx_task.send(SessionEvent::Closed(Some(format!("{e:#}"))));
            } else {
                let _ = event_tx_task.send(SessionEvent::Closed(None));
            }
        })
        .expect("failed to spawn local-shell thread");

    let _ = rt; // kept for API symmetry with spawn_session

    SessionHandle {
        events: event_rx,
        commands: cmd_tx,
        tunnels,
    }
}

fn run_local_shell(
    event_tx: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
    mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<SessionCommand>,
) -> anyhow::Result<()> {
    let pty_system = NativePtySystem::default();
    let pair = pty_system.openpty(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let shell = default_shell();
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");

    let mut child = pair.slave.spawn_command(cmd)?;
    // Drop the slave side so reads on the master EOF when the child exits.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;

    // Reader thread: PTY output → SessionEvent::Output
    let ev_tx = event_tx.clone();
    let reader_handle = std::thread::Builder::new()
        .name("local-shell-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if ev_tx.send(SessionEvent::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!(?e, "local shell reader error");
                        break;
                    }
                }
            }
        })?;

    // We need the master handle alive for resize, wrap it in Arc<Mutex>.
    let master = Arc::new(Mutex::new(pair.master));

    // Command loop: SessionCommand → PTY input / resize
    loop {
        match cmd_rx.blocking_recv() {
            Some(SessionCommand::Input(data)) => {
                if writer.write_all(&data).is_err() {
                    break;
                }
            }
            Some(SessionCommand::Resize { cols, rows }) => {
                let m = master.lock();
                let _ = m.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
            Some(SessionCommand::Disconnect) | None => {
                break;
            }
        }
    }

    // Clean up
    drop(writer);
    drop(master);
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader_handle.join();

    Ok(())
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| {
        if cfg!(windows) {
            "cmd.exe".to_string()
        } else {
            "/bin/bash".to_string()
        }
    })
}
