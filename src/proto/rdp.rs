//! RDP client session via the e-sh-rdp helper binary.
//!
//! Spawns the `e-sh-rdp` binary as a child process and communicates via
//! stdin/stdout pipes using a simple binary protocol.

use std::io::{Read, Write};
use std::process::{Command, Stdio};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::core::connection::{AuthMethod, Connection};

// Message types (helper -> app)
const MSG_CONNECTED: u8 = 1;
const MSG_BITMAP: u8 = 2;
const MSG_CLOSED: u8 = 3;

// Command types (app -> helper)
const CMD_MOUSE_MOVE: u8 = 10;
const CMD_MOUSE_BUTTON: u8 = 11;
const CMD_KEY: u8 = 12;
const CMD_SHUTDOWN: u8 = 13;
const CMD_MOUSE_SCROLL: u8 = 14;

#[derive(Clone)]
pub struct BitmapRegion {
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
    pub data: Vec<u8>,
    pub bpp: u16,
}

pub enum RdpEvent {
    Connected { width: u16, height: u16, external_window: bool },
    Bitmap(BitmapRegion),
    Closed(Option<String>),
}

pub enum RdpCommand {
    Mouse { x: u16, y: u16, button: RdpMouseButton, down: bool },
    MouseMove { x: u16, y: u16 },
    /// Scroll wheel: positive = up, negative = down
    Scroll { x: u16, y: u16, delta: i16 },
    Key { code: u16, pressed: bool },
    Shutdown,
}

#[derive(Clone, Copy)]
pub enum RdpMouseButton {
    Left,
    Right,
    Middle,
    None,
}

pub struct RdpHandle {
    pub events: UnboundedReceiver<RdpEvent>,
    pub commands: UnboundedSender<RdpCommand>,
}

pub fn spawn_rdp_session(
    rt: &tokio::runtime::Handle,
    conn: Connection,
) -> RdpHandle {
    let (event_tx, event_rx) = unbounded_channel::<RdpEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<RdpCommand>();

    let event_tx_task = event_tx.clone();
    rt.spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            run_rdp_helper(conn, event_tx_task.clone(), cmd_rx)
        }).await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = event_tx.send(RdpEvent::Closed(Some(format!("{e:#}"))));
            }
            Err(e) => {
                let _ = event_tx.send(RdpEvent::Closed(Some(format!("task panicked: {e}"))));
            }
        }
    });

    RdpHandle { events: event_rx, commands: cmd_tx }
}

fn find_helper_binary() -> Option<std::path::PathBuf> {
    // Look next to the current executable first
    if let Ok(exe) = std::env::current_exe() {
        let dir = exe.parent()?;
        let helper = dir.join("e-sh-rdp");
        if helper.exists() { return Some(helper); }
        // Also check in target/debug or target/release during development
        let target_debug = dir.join("../e-sh-rdp");
        if target_debug.exists() { return Some(target_debug); }
    }
    // Fall back to workspace target directory (development)
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    for profile in &["debug", "release"] {
        // Helper has its own target dir since it's not a workspace member
        let p = std::path::PathBuf::from(manifest_dir)
            .join(format!("e-sh-rdp/target/{profile}/e-sh-rdp"));
        if p.exists() { return Some(p); }
        // Also check the main target dir in case it was copied there
        let p = std::path::PathBuf::from(manifest_dir)
            .join(format!("target/{profile}/e-sh-rdp"));
        if p.exists() { return Some(p); }
    }
    None
}

fn run_rdp_helper(
    conn: Connection,
    event_tx: UnboundedSender<RdpEvent>,
    cmd_rx: UnboundedReceiver<RdpCommand>,
) -> anyhow::Result<()> {
    let helper_path = find_helper_binary()
        .ok_or_else(|| anyhow::anyhow!(
            "e-sh-rdp helper binary not found. Build it with: cargo build -p e-sh-rdp"
        ))?;

    let password = match &conn.auth {
        AuthMethod::Password { password } => password.clone(),
        _ => String::new(),
    };

    let params = serde_json::json!({
        "host": conn.host,
        "port": conn.port,
        "username": conn.username,
        "password": password,
        "width": conn.rdp_width,
        "height": conn.rdp_height,
        "backend": match conn.rdp_backend {
            crate::core::connection::RdpBackend::Auto => "auto",
            crate::core::connection::RdpBackend::Ironrdp => "ironrdp",
            crate::core::connection::RdpBackend::Freerdp => "freerdp",
        },
        "freerdp_resize_mode": match conn.freerdp_resize_mode {
            crate::core::connection::FreeRdpResizeMode::DynamicResolution => "dynamic_resolution",
            crate::core::connection::FreeRdpResizeMode::SmartSizing => "smart_sizing",
            crate::core::connection::FreeRdpResizeMode::Static => "static",
        },
        "rdp_security_mode": match conn.rdp_security_mode {
            crate::core::connection::RdpSecurityMode::Negotiate => "negotiate",
            crate::core::connection::RdpSecurityMode::Nla => "nla",
            crate::core::connection::RdpSecurityMode::Tls => "tls",
            crate::core::connection::RdpSecurityMode::Rdp => "rdp",
        },
    });

    let mut child = Command::new(&helper_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn e-sh-rdp: {e}"))?;

    let mut child_stdin = child.stdin.take().unwrap();
    let mut child_stdout = child.stdout.take().unwrap();

    // Send connection params
    let params_str = format!("{}\n", params);
    child_stdin.write_all(params_str.as_bytes())?;
    child_stdin.flush()?;

    // Spawn a thread to forward commands to the helper's stdin
    let cmd_rx = std::sync::Mutex::new(cmd_rx);
    let stdin_thread = std::thread::spawn(move || {
        let rx = cmd_rx;
        loop {
            let cmd = {
                let mut guard = rx.lock().unwrap();
                guard.blocking_recv()
            };
            let Some(cmd) = cmd else { break; };
            let msg = encode_command(&cmd);
            if child_stdin.write_all(&msg).is_err() { break; }
            if child_stdin.flush().is_err() { break; }
            if matches!(cmd, RdpCommand::Shutdown) { break; }
        }
    });

    // Read messages from the helper's stdout
    loop {
        let mut header = [0u8; 5];
        match child_stdout.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                let _ = event_tx.send(RdpEvent::Closed(Some("helper process exited".into())));
                break;
            }
            Err(e) => {
                let _ = event_tx.send(RdpEvent::Closed(Some(format!("read error: {e}"))));
                break;
            }
        }

        let msg_type = header[0];
        let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
        let mut payload = vec![0u8; len];
        if len > 0 {
            child_stdout.read_exact(&mut payload)?;
        }

        match msg_type {
            MSG_CONNECTED if payload.len() >= 4 => {
                let width = u16::from_le_bytes([payload[0], payload[1]]);
                let height = u16::from_le_bytes([payload[2], payload[3]]);
                let external_window = payload.get(4).copied().unwrap_or(0) != 0;
                let _ = event_tx.send(RdpEvent::Connected { width, height, external_window });
            }
            MSG_BITMAP if payload.len() >= 10 => {
                let left = u16::from_le_bytes([payload[0], payload[1]]);
                let top = u16::from_le_bytes([payload[2], payload[3]]);
                let width = u16::from_le_bytes([payload[4], payload[5]]);
                let height = u16::from_le_bytes([payload[6], payload[7]]);
                let bpp = u16::from_le_bytes([payload[8], payload[9]]);
                let data = payload[10..].to_vec();
                let _ = event_tx.send(RdpEvent::Bitmap(BitmapRegion {
                    left, top, width, height, data, bpp,
                }));
            }
            MSG_CLOSED => {
                let reason = String::from_utf8_lossy(&payload).to_string();
                let _ = event_tx.send(RdpEvent::Closed(Some(reason)));
                break;
            }
            _ => {}
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = stdin_thread.join();
    Ok(())
}

fn encode_command(cmd: &RdpCommand) -> Vec<u8> {
    match cmd {
        RdpCommand::MouseMove { x, y } => {
            let mut buf = vec![CMD_MOUSE_MOVE];
            buf.extend_from_slice(&4u32.to_le_bytes());
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&y.to_le_bytes());
            buf
        }
        RdpCommand::Mouse { x, y, button, down } => {
            let btn: u8 = match button {
                RdpMouseButton::Left => 0,
                RdpMouseButton::Right => 1,
                RdpMouseButton::Middle => 2,
                RdpMouseButton::None => 255,
            };
            let mut buf = vec![CMD_MOUSE_BUTTON];
            buf.extend_from_slice(&6u32.to_le_bytes());
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&y.to_le_bytes());
            buf.push(btn);
            buf.push(if *down { 1 } else { 0 });
            buf
        }
        RdpCommand::Key { code, pressed } => {
            let mut buf = vec![CMD_KEY];
            buf.extend_from_slice(&3u32.to_le_bytes());
            buf.extend_from_slice(&code.to_le_bytes());
            buf.push(if *pressed { 1 } else { 0 });
            buf
        }
        RdpCommand::Scroll { x, y, delta } => {
            let mut buf = vec![CMD_MOUSE_SCROLL];
            buf.extend_from_slice(&6u32.to_le_bytes());
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&y.to_le_bytes());
            buf.extend_from_slice(&delta.to_le_bytes());
            buf
        }
        RdpCommand::Shutdown => {
            let mut buf = vec![CMD_SHUTDOWN];
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf
        }
    }
}
