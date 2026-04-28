//! VNC client session using a pure-Rust RFB (Remote Framebuffer) implementation.
//!
//! Connects to a VNC server, receives framebuffer updates, and forwards
//! mouse/keyboard input from the egui UI.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use des::cipher::{BlockCipherEncrypt, KeyInit};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::core::connection::{AuthMethod, Connection};

/// A rectangular region of the framebuffer received from the server.
#[derive(Clone)]
pub struct VncBitmapRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
    /// Raw pixel data in RGBA (4 bytes per pixel).
    pub data: Vec<u8>,
}

/// Events sent from the VNC reader thread to the UI.
pub enum VncEvent {
    Connected { width: u16, height: u16 },
    Bitmap(VncBitmapRegion),
    Closed(Option<String>),
}

/// Commands sent from the UI to the VNC writer thread.
pub enum VncCommand {
    /// Pointer (mouse) event: position + button mask (RFB button mask).
    Pointer { x: u16, y: u16, button_mask: u8 },
    /// Key event: X11 keysym + pressed flag.
    Key { keysym: u32, pressed: bool },
    /// Request a full framebuffer update.
    Refresh,
    /// Graceful shutdown.
    Shutdown,
}

pub struct VncHandle {
    pub events: UnboundedReceiver<VncEvent>,
    pub commands: UnboundedSender<VncCommand>,
    shutdown: Arc<AtomicBool>,
}

impl Drop for VncHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.commands.send(VncCommand::Shutdown);
    }
}

pub fn spawn_vnc_session(
    rt: &tokio::runtime::Handle,
    conn: Connection,
) -> VncHandle {
    let (event_tx, event_rx) = unbounded_channel::<VncEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<VncCommand>();
    let shutdown = Arc::new(AtomicBool::new(false));

    let event_tx_task = event_tx.clone();
    let shutdown_task = shutdown.clone();
    rt.spawn(async move {
        let result = tokio::task::spawn_blocking(move || {
            run_vnc_session(conn, event_tx_task.clone(), cmd_rx, shutdown_task)
        })
        .await;

        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = event_tx.send(VncEvent::Closed(Some(format!("{e:#}"))));
            }
            Err(e) => {
                let _ = event_tx.send(VncEvent::Closed(Some(format!("task panicked: {e}"))));
            }
        }
    });

    VncHandle { events: event_rx, commands: cmd_tx, shutdown }
}

// ---------------------------------------------------------------------------
// RFB protocol constants
// ---------------------------------------------------------------------------

const RFB_VERSION_3_8: &[u8] = b"RFB 003.008\n";
const RFB_VERSION_3_7: &[u8] = b"RFB 003.007\n";
const RFB_VERSION_3_3: &[u8] = b"RFB 003.003\n";

// Security types
const SEC_NONE: u8 = 1;
const SEC_VNC_AUTH: u8 = 2;

// Client message types
const CLIENT_SET_PIXEL_FORMAT: u8 = 0;
const CLIENT_SET_ENCODINGS: u8 = 2;
const CLIENT_FB_UPDATE_REQUEST: u8 = 3;
const CLIENT_KEY_EVENT: u8 = 4;
const CLIENT_POINTER_EVENT: u8 = 5;

// Server message types
const SERVER_FB_UPDATE: u8 = 0;
const SERVER_SET_COLOUR_MAP: u8 = 1;
const SERVER_BELL: u8 = 2;
const SERVER_CUT_TEXT: u8 = 3;

// Encoding types
const ENCODING_RAW: i32 = 0;
const ENCODING_COPYRECT: i32 = 1;

// ---------------------------------------------------------------------------
// VNC (DES) authentication challenge-response
// ---------------------------------------------------------------------------

/// Perform VNC DES challenge-response authentication (RFB §7.2.2).
///
/// The server sends a 16-byte random challenge; we encrypt it with the user's
/// password (truncated/padded to 8 bytes, each byte bit-reversed) using
/// single-DES in ECB mode and return the 16-byte ciphertext.
///
/// # Security note
///
/// Single-DES is a weak cipher and the 8-byte password limit is poor by modern
/// standards.  However, this is the *only* authentication handshake defined by
/// the core RFB protocol (security-type 2).  Every conforming VNC client must
/// implement it exactly this way — the key derivation, bit-reversal, and
/// algorithm are all mandated by the spec and cannot be substituted.
///
/// For stronger transport security, prefer connecting through an SSH tunnel or
/// using a VNC server that supports VeNCrypt (TLS-wrapped RFB).
///
/// CodeQL: "Use of a broken or weak cryptographic algorithm" — intentional,
/// protocol-mandated.  See RFC 6143 §7.2.2.
fn vnc_auth_response(challenge: &[u8; 16], password: &str) -> [u8; 16] {
    // Prepare the key: take first 8 bytes of password, pad with zeros,
    // and reverse the bits of each byte (required by the RFB spec).
    let mut key = [0u8; 8]; // CodeQL: not a hard-coded secret — zero-padding per RFB spec
    for (i, &b) in password.as_bytes().iter().take(8).enumerate() {
        key[i] = reverse_bits(b);
    }

    // CodeQL: DES is weak but mandated by the VNC/RFB protocol (RFC 6143 §7.2.2).
    let cipher = des::Des::new_from_slice(&key).expect("DES key is 8 bytes");

    let mut response = [0u8; 16];
    // Encrypt each 8-byte block of the challenge with single-DES ECB.
    let mut block0: des::cipher::Array<u8, _> = challenge[0..8].try_into().unwrap();
    cipher.encrypt_block(&mut block0);
    response[0..8].copy_from_slice(&block0);

    let mut block1: des::cipher::Array<u8, _> = challenge[8..16].try_into().unwrap();
    cipher.encrypt_block(&mut block1);
    response[8..16].copy_from_slice(&block1);

    response
}

/// Reverse the bits of a single byte.
///
/// The RFB spec requires each byte of the DES key to be bit-reversed before
/// use.  The constants here (shift amounts, mask `1`) are arithmetic, not
/// cryptographic secrets.
fn reverse_bits(b: u8) -> u8 {
    let mut r = 0u8;
    for i in 0..8 {
        r |= ((b >> i) & 1) << (7 - i);
    }
    r
}

// ---------------------------------------------------------------------------
// RFB session logic
// ---------------------------------------------------------------------------

fn run_vnc_session(
    conn: Connection,
    event_tx: UnboundedSender<VncEvent>,
    cmd_rx: UnboundedReceiver<VncCommand>,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let addr = format!("{}:{}", conn.host, conn.port);
    let mut stream = TcpStream::connect(&addr)
        .map_err(|e| anyhow::anyhow!("failed to connect to {addr}: {e}"))?;
    stream.set_nodelay(true).ok();

    // --- Protocol version handshake ---
    let mut version_buf = [0u8; 12];
    stream.read_exact(&mut version_buf)?;

    // We always respond with 3.8 (or 3.7 / 3.3 if server is older)
    let server_version = std::str::from_utf8(&version_buf).unwrap_or("");
    let client_version = if server_version.starts_with("RFB 003.008") {
        RFB_VERSION_3_8
    } else if server_version.starts_with("RFB 003.007") {
        RFB_VERSION_3_7
    } else {
        RFB_VERSION_3_3
    };
    stream.write_all(client_version)?;
    stream.flush()?;

    let use_38 = client_version == RFB_VERSION_3_8;

    // --- Security handshake ---
    let password = match &conn.auth {
        AuthMethod::Password { password } => password.clone(),
        _ => String::new(),
    };

    if use_38 {
        // RFB 3.8: server sends list of security types
        let mut num_types = [0u8; 1];
        stream.read_exact(&mut num_types)?;
        if num_types[0] == 0 {
            // Connection failed — read reason
            let reason = read_reason_string(&mut stream)?;
            return Err(anyhow::anyhow!("server refused connection: {reason}"));
        }
        let mut types = vec![0u8; num_types[0] as usize];
        stream.read_exact(&mut types)?;

        // Prefer VNC auth if password is available, otherwise None
        let chosen = if !password.is_empty() && types.contains(&SEC_VNC_AUTH) {
            SEC_VNC_AUTH
        } else if types.contains(&SEC_NONE) {
            SEC_NONE
        } else if types.contains(&SEC_VNC_AUTH) {
            SEC_VNC_AUTH
        } else {
            return Err(anyhow::anyhow!(
                "no supported security type (server offered: {types:?})"
            ));
        };

        stream.write_all(&[chosen])?;
        stream.flush()?;

        if chosen == SEC_VNC_AUTH {
            do_vnc_auth(&mut stream, &password)?;
        }

        // Read SecurityResult
        let mut result = [0u8; 4];
        stream.read_exact(&mut result)?;
        let code = u32::from_be_bytes(result);
        if code != 0 {
            let reason = read_reason_string(&mut stream).unwrap_or_else(|_| "authentication failed".into());
            return Err(anyhow::anyhow!("authentication failed: {reason}"));
        }
    } else {
        // RFB 3.3: server picks the security type
        let mut sec_type = [0u8; 4];
        stream.read_exact(&mut sec_type)?;
        let chosen = u32::from_be_bytes(sec_type);
        match chosen {
            0 => {
                let reason = read_reason_string(&mut stream)?;
                return Err(anyhow::anyhow!("server refused connection: {reason}"));
            }
            1 => { /* None — no auth needed */ }
            2 => {
                do_vnc_auth(&mut stream, &password)?;
                // 3.3 has no SecurityResult for None, but does for VNC auth
                let mut result = [0u8; 4];
                stream.read_exact(&mut result)?;
                if u32::from_be_bytes(result) != 0 {
                    return Err(anyhow::anyhow!("VNC authentication failed"));
                }
            }
            _ => {
                return Err(anyhow::anyhow!("unsupported security type: {chosen}"));
            }
        }
    }

    // --- ClientInit ---
    // shared-flag = 1 (allow other clients)
    stream.write_all(&[1])?;
    stream.flush()?;

    // --- ServerInit ---
    let mut server_init = [0u8; 24];
    stream.read_exact(&mut server_init)?;
    let fb_width = u16::from_be_bytes([server_init[0], server_init[1]]);
    let fb_height = u16::from_be_bytes([server_init[2], server_init[3]]);
    // Pixel format: bytes 4..19
    let _bpp = server_init[4];
    let _depth = server_init[5];
    let _big_endian = server_init[6];
    let _true_colour = server_init[7];
    // Read server name
    let name_len = u32::from_be_bytes([server_init[20], server_init[21], server_init[22], server_init[23]]) as usize;
    let mut _name_buf = vec![0u8; name_len];
    stream.read_exact(&mut _name_buf)?;

    // --- Set pixel format to 32-bit RGBX ---
    {
        let mut msg = [0u8; 20];
        msg[0] = CLIENT_SET_PIXEL_FORMAT;
        // padding: 1..4
        // pixel format starts at byte 4
        msg[4] = 32;  // bits-per-pixel
        msg[5] = 24;  // depth
        msg[6] = 0;   // big-endian = false
        msg[7] = 1;   // true-colour = true
        // red-max = 255
        msg[8] = 0; msg[9] = 255;
        // green-max = 255
        msg[10] = 0; msg[11] = 255;
        // blue-max = 255
        msg[12] = 0; msg[13] = 255;
        // red-shift = 0, green-shift = 8, blue-shift = 16
        msg[14] = 0;   // red-shift
        msg[15] = 8;   // green-shift
        msg[16] = 16;  // blue-shift
        // padding: 17..20
        stream.write_all(&msg)?;
        stream.flush()?;
    }

    // --- Set encodings (Raw + CopyRect) ---
    {
        let mut msg = [0u8; 12];
        msg[0] = CLIENT_SET_ENCODINGS;
        // padding: 1
        // number of encodings: 2
        msg[2] = 0; msg[3] = 2;
        // CopyRect
        let cr = ENCODING_COPYRECT.to_be_bytes();
        msg[4..8].copy_from_slice(&cr);
        // Raw
        let raw = ENCODING_RAW.to_be_bytes();
        msg[8..12].copy_from_slice(&raw);
        stream.write_all(&msg)?;
        stream.flush()?;
    }

    let _ = event_tx.send(VncEvent::Connected {
        width: fb_width,
        height: fb_height,
    });

    // Request initial full framebuffer update
    send_fb_update_request(&mut stream, false, 0, 0, fb_width, fb_height)?;

    // Clone stream for the writer thread
    let writer_stream = stream.try_clone()?;
    let cmd_rx = std::sync::Mutex::new(cmd_rx);

    let writer_thread = std::thread::spawn(move || {
        vnc_writer_loop(writer_stream, cmd_rx, fb_width, fb_height);
    });

    // Reader loop
    //
    // We use a short read timeout ONLY for the 1-byte message-type probe.
    // Once we know a message is arriving, we switch back to blocking mode
    // so that `read_exact` on the payload never fails with EAGAIN mid-read.
    let probe_timeout = Some(std::time::Duration::from_millis(100));
    let mut framebuffer = vec![0u8; fb_width as usize * fb_height as usize * 4];
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Probe for the next message type byte with a timeout
        stream.set_read_timeout(probe_timeout).ok();
        let mut msg_type = [0u8; 1];
        match stream.read(&mut msg_type) {
            Ok(0) => {
                let _ = event_tx.send(VncEvent::Closed(Some("server closed connection".into())));
                break;
            }
            Ok(_) => {
                // Got data — switch to blocking for the rest of this message
                stream.set_read_timeout(None).ok();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock
                   || e.kind() == std::io::ErrorKind::TimedOut => {
                // Timeout — re-request an incremental update and keep waiting
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                // Switch to blocking briefly to send the request
                stream.set_read_timeout(None).ok();
                let _ = send_fb_update_request(&mut stream, true, 0, 0, fb_width, fb_height);
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                let _ = event_tx.send(VncEvent::Closed(Some("server closed connection".into())));
                break;
            }
            Err(e) => {
                let _ = event_tx.send(VncEvent::Closed(Some(format!("read error: {e}"))));
                break;
            }
        }

        match msg_type[0] {
            SERVER_FB_UPDATE => {
                // padding: 1 byte
                let mut header = [0u8; 3];
                stream.read_exact(&mut header)?;
                let num_rects = u16::from_be_bytes([header[1], header[2]]);

                for _ in 0..num_rects {
                    let mut rect_header = [0u8; 12];
                    stream.read_exact(&mut rect_header)?;
                    let x = u16::from_be_bytes([rect_header[0], rect_header[1]]);
                    let y = u16::from_be_bytes([rect_header[2], rect_header[3]]);
                    let w = u16::from_be_bytes([rect_header[4], rect_header[5]]);
                    let h = u16::from_be_bytes([rect_header[6], rect_header[7]]);
                    let encoding = i32::from_be_bytes([
                        rect_header[8], rect_header[9], rect_header[10], rect_header[11],
                    ]);

                    match encoding {
                        ENCODING_RAW => {
                            let pixel_count = w as usize * h as usize;
                            let byte_count = pixel_count * 4; // 32bpp
                            let mut pixel_data = vec![0u8; byte_count];
                            stream.read_exact(&mut pixel_data)?;

                            // Convert from server pixel format (RGBX) to RGBA
                            let mut rgba = vec![0u8; pixel_count * 4];
                            for i in 0..pixel_count {
                                let si = i * 4;
                                rgba[si] = pixel_data[si];       // R
                                rgba[si + 1] = pixel_data[si + 1]; // G
                                rgba[si + 2] = pixel_data[si + 2]; // B
                                rgba[si + 3] = 255;                // A
                            }

                            // Blit into local framebuffer
                            blit_rect(&mut framebuffer, fb_width, x, y, w, h, &rgba);

                            let _ = event_tx.send(VncEvent::Bitmap(VncBitmapRegion {
                                x, y, width: w, height: h, data: rgba,
                            }));
                        }
                        ENCODING_COPYRECT => {
                            let mut src = [0u8; 4];
                            stream.read_exact(&mut src)?;
                            let src_x = u16::from_be_bytes([src[0], src[1]]);
                            let src_y = u16::from_be_bytes([src[2], src[3]]);

                            // Copy from framebuffer
                            let rgba = copy_rect(
                                &framebuffer, fb_width, fb_height,
                                src_x, src_y, w, h,
                            );
                            blit_rect(&mut framebuffer, fb_width, x, y, w, h, &rgba);

                            let _ = event_tx.send(VncEvent::Bitmap(VncBitmapRegion {
                                x, y, width: w, height: h, data: rgba,
                            }));
                        }
                        _ => {
                            // Unknown encoding — skip (we can't know the size, so bail)
                            let _ = event_tx.send(VncEvent::Closed(Some(
                                format!("unsupported encoding: {encoding}")
                            )));
                            break;
                        }
                    }
                }

                // Request incremental update
                send_fb_update_request(&mut stream, true, 0, 0, fb_width, fb_height)?;
            }
            SERVER_SET_COLOUR_MAP => {
                // Skip: padding(1) + first-colour(2) + num-colours(2)
                let mut header = [0u8; 5];
                stream.read_exact(&mut header)?;
                let num = u16::from_be_bytes([header[3], header[4]]) as usize;
                let mut _colours = vec![0u8; num * 6];
                stream.read_exact(&mut _colours)?;
            }
            SERVER_BELL => {
                // No payload
            }
            SERVER_CUT_TEXT => {
                let mut header = [0u8; 7];
                stream.read_exact(&mut header)?;
                let len = u32::from_be_bytes([header[3], header[4], header[5], header[6]]) as usize;
                let mut _text = vec![0u8; len];
                stream.read_exact(&mut _text)?;
            }
            other => {
                let _ = event_tx.send(VncEvent::Closed(Some(
                    format!("unknown server message type: {other}")
                )));
                break;
            }
        }
    }

    let _ = stream.shutdown(std::net::Shutdown::Both);
    let _ = writer_thread.join();
    Ok(())
}

fn do_vnc_auth(stream: &mut TcpStream, password: &str) -> anyhow::Result<()> {
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let response = vnc_auth_response(&challenge, password);
    stream.write_all(&response)?;
    stream.flush()?;
    Ok(())
}

fn read_reason_string(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut len_buf = [0u8; 4];
    if stream.read_exact(&mut len_buf).is_err() {
        return Ok(String::new());
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1024 * 64 {
        return Ok("(reason too long)".into());
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

fn send_fb_update_request(
    stream: &mut TcpStream,
    incremental: bool,
    x: u16, y: u16, w: u16, h: u16,
) -> anyhow::Result<()> {
    let mut msg = [0u8; 10];
    msg[0] = CLIENT_FB_UPDATE_REQUEST;
    msg[1] = if incremental { 1 } else { 0 };
    msg[2..4].copy_from_slice(&x.to_be_bytes());
    msg[4..6].copy_from_slice(&y.to_be_bytes());
    msg[6..8].copy_from_slice(&w.to_be_bytes());
    msg[8..10].copy_from_slice(&h.to_be_bytes());
    stream.write_all(&msg)?;
    stream.flush()?;
    Ok(())
}

fn vnc_writer_loop(
    mut stream: TcpStream,
    cmd_rx: std::sync::Mutex<UnboundedReceiver<VncCommand>>,
    fb_width: u16,
    fb_height: u16,
) {
    loop {
        let cmd = {
            let mut guard = cmd_rx.lock().unwrap();
            guard.blocking_recv()
        };
        let Some(cmd) = cmd else { break };
        match cmd {
            VncCommand::Pointer { x, y, button_mask } => {
                let mut msg = [0u8; 6];
                msg[0] = CLIENT_POINTER_EVENT;
                msg[1] = button_mask;
                msg[2..4].copy_from_slice(&x.to_be_bytes());
                msg[4..6].copy_from_slice(&y.to_be_bytes());
                if stream.write_all(&msg).is_err() { break; }
                let _ = stream.flush();
            }
            VncCommand::Key { keysym, pressed } => {
                let mut msg = [0u8; 8];
                msg[0] = CLIENT_KEY_EVENT;
                msg[1] = if pressed { 1 } else { 0 };
                // padding: 2..4
                msg[4..8].copy_from_slice(&keysym.to_be_bytes());
                if stream.write_all(&msg).is_err() { break; }
                let _ = stream.flush();
            }
            VncCommand::Refresh => {
                let _ = send_fb_update_request(&mut stream, false, 0, 0, fb_width, fb_height);
            }
            VncCommand::Shutdown => {
                break;
            }
        }
    }
}

fn blit_rect(
    framebuffer: &mut [u8],
    fb_width: u16,
    x: u16, y: u16, w: u16, h: u16,
    data: &[u8],
) {
    let fbw = fb_width as usize;
    for row in 0..h as usize {
        let dst_y = y as usize + row;
        for col in 0..w as usize {
            let dst_x = x as usize + col;
            let src_idx = (row * w as usize + col) * 4;
            let dst_idx = (dst_y * fbw + dst_x) * 4;
            if src_idx + 4 <= data.len() && dst_idx + 4 <= framebuffer.len() {
                framebuffer[dst_idx..dst_idx + 4].copy_from_slice(&data[src_idx..src_idx + 4]);
            }
        }
    }
}

fn copy_rect(
    framebuffer: &[u8],
    fb_width: u16, _fb_height: u16,
    src_x: u16, src_y: u16, w: u16, h: u16,
) -> Vec<u8> {
    let fbw = fb_width as usize;
    let mut data = vec![0u8; w as usize * h as usize * 4];
    for row in 0..h as usize {
        let sy = src_y as usize + row;
        for col in 0..w as usize {
            let sx = src_x as usize + col;
            let src_idx = (sy * fbw + sx) * 4;
            let dst_idx = (row * w as usize + col) * 4;
            if src_idx + 4 <= framebuffer.len() && dst_idx + 4 <= data.len() {
                data[dst_idx..dst_idx + 4].copy_from_slice(&framebuffer[src_idx..src_idx + 4]);
            }
        }
    }
    data
}
