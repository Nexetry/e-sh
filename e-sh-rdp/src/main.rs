//! e-sh-rdp: IronRDP helper binary for embedded RDP viewer.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::io::{self, BufRead, Read, Write};
use std::net::SocketAddr;

#[derive(Deserialize, Clone)]
struct Params {
    host: String,
    port: u16,
    username: String,
    password: String,
    width: u16,
    height: u16,
    /// `"auto"` (default), `"ironrdp"`, or `"freerdp"`.
    #[serde(default = "default_backend")]
    backend: String,
    /// `"dynamic_resolution"` (default), `"smart_sizing"`, or `"static"`.
    #[serde(default = "default_resize_mode")]
    freerdp_resize_mode: String,
}

fn default_backend() -> String {
    "auto".into()
}

fn default_resize_mode() -> String {
    "dynamic_resolution".into()
}

const MSG_CONNECTED: u8 = 1;
const MSG_BITMAP: u8 = 2;
const MSG_CLOSED: u8 = 3;
const CMD_MOUSE_MOVE: u8 = 10;
const CMD_MOUSE_BUTTON: u8 = 11;
const CMD_KEY: u8 = 12;
const CMD_SHUTDOWN: u8 = 13;
const CMD_MOUSE_SCROLL: u8 = 14;

fn write_msg(out: &mut impl Write, t: u8, p: &[u8]) -> io::Result<()> {
    out.write_all(&[t])?;
    out.write_all(&(p.len() as u32).to_le_bytes())?;
    out.write_all(p)?;
    out.flush()
}

fn main() {
    // Default keeps the connector at debug so the actual PDU received during
    // CapabilitiesExchange is visible (see ironrdp_connector::connection_activation
    // "Received" log line). Override with IRONRDP_LOG=... to silence or expand.
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("IRONRDP_LOG")
                .unwrap_or_else(|_| "ironrdp_connector=debug,ironrdp=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run()) {
        let mut o = io::stdout().lock();
        let _ = write_msg(&mut o, MSG_CLOSED, format!("{e:#}").as_bytes());
    }
}

async fn run() -> Result<()> {
    let p: Params = {
        let mut l = String::new();
        io::stdin().lock().read_line(&mut l)?;
        serde_json::from_str(l.trim())?
    };
    let mut out = io::stdout().lock();

    match p.backend.as_str() {
        "freerdp" => {
            let r = freerdp_session(&p, &mut out).await;
            match &r {
                Ok(()) => { let _ = write_msg(&mut out, MSG_CLOSED, b"session ended"); }
                Err(e) => { let _ = write_msg(&mut out, MSG_CLOSED, format!("{e:#}").as_bytes()); }
            }
            r
        }
        "ironrdp" => {
            let r = session(&p, &mut out).await;
            match &r {
                Ok(()) => { let _ = write_msg(&mut out, MSG_CLOSED, b"session ended"); }
                Err(e) => { let _ = write_msg(&mut out, MSG_CLOSED, format!("{e:#}").as_bytes()); }
            }
            r
        }
        _ => {
            // "auto": try IronRDP first, fall back to FreeRDP on grd-style disconnect
            match session(&p, &mut out).await {
                Ok(()) => {
                    let _ = write_msg(&mut out, MSG_CLOSED, b"session ended");
                    Ok(())
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    if is_gfx_disconnect(&msg) {
                        eprintln!(
                            "[e-sh-rdp] IronRDP failed (likely GFX-only server like gnome-remote-desktop), \
                             retrying with FreeRDP: {msg}"
                        );
                        let r = freerdp_session(&p, &mut out).await;
                        match &r {
                            Ok(()) => { let _ = write_msg(&mut out, MSG_CLOSED, b"session ended"); }
                            Err(e2) => { let _ = write_msg(&mut out, MSG_CLOSED, format!("{e2:#}").as_bytes()); }
                        }
                        r
                    } else {
                        let _ = write_msg(&mut out, MSG_CLOSED, msg.as_bytes());
                        Err(e)
                    }
                }
            }
        }
    }
}

/// Heuristic: detect disconnects caused by servers that require the GFX
/// pipeline (e.g. gnome-remote-desktop). These typically disconnect during
/// or right after CapabilitiesExchange / Consumed with "UserRequested" or
/// "disconnect provider ultimatum".
fn is_gfx_disconnect(msg: &str) -> bool {
    let m = msg.to_lowercase();
    // grd sends a disconnect provider ultimatum with reason UserRequested
    // right after capabilities exchange when GFX is not advertised.
    (m.contains("userrequested") || m.contains("user requested"))
        || (m.contains("disconnect") && m.contains("ultimatum"))
        || (m.contains("consumed") && m.contains("disconnect"))
        || (m.contains("connect_finalize") && m.contains("consumed"))
}

async fn session(p: &Params, out: &mut impl Write) -> Result<()> {
    use ironrdp::connector::*;
    use ironrdp::graphics::image_processing::PixelFormat;
    use ironrdp::session::{ActiveStage, ActiveStageOutput, image::DecodedImage};
    use ironrdp_tokio::*;
    use tokio::net::TcpStream;

    let addr = format!("{}:{}", p.host, p.port);
    let sock: SocketAddr = tokio::net::lookup_host(&addr)
        .await?
        .next()
        .ok_or_else(|| anyhow::anyhow!("DNS failed for {addr}"))?;

    let config = Config {
        desktop_size: DesktopSize {
            width: p.width,
            height: p.height,
        },
        desktop_scale_factor: 0,
        // Offer both SSL (TLS-only) and HYBRID/HYBRID_EX (CredSSP/NLA).
        // Some servers (e.g. xrdp configured with security_layer=negotiate or
        // hybrid, Windows with "Require NLA") refuse SSL-only and respond with
        // FailureCode(5) SSL_NOT_ALLOWED_BY_SERVER. CredSSP uses in-process
        // NTLM for username/password credentials and does not require a
        // working NetworkClient.
        enable_tls: true,
        enable_credssp: true,
        credentials: Credentials::UsernamePassword {
            username: p.username.clone(),
            password: p.password.clone().into(),
        },
        domain: None,
        client_build: 0,
        client_name: "e-sh".into(),
        keyboard_type: ironrdp::pdu::gcc::KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0,
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig {
            lossy_compression: true,
            color_depth: 32,
            codecs: ironrdp::pdu::rdp::capability_sets::client_codecs_capabilities(&[])
                .unwrap_or_default(),
        }),
        dig_product_id: String::new(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".into(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        platform: ironrdp::pdu::rdp::capability_sets::MajorPlatformType::UNSPECIFIED,
        hardware_id: None,
        request_data: None,
        pointer_software_rendering: true,
        enable_server_pointer: false,
        autologon: false,
        enable_audio_playback: false,
        license_cache: None,
        performance_flags: Default::default(),
        timezone_info: Default::default(),
        compression_type: None,
        multitransport_flags: None,
    };

    let sn = ServerName::new(&p.host);

    let mut conn = ClientConnector::new(config, sock);

    let tcp = TcpStream::connect(&addr).await.context("TCP connect")?;
    let mut f: Framed<TokioStream<TcpStream>> = Framed::new(tcp);
    let upgrade = connect_begin(&mut f, &mut conn)
        .await
        .map_err(|e| anyhow::anyhow!("connect_begin: {e}"))?;

    let tcp_inner = f.into_inner_no_leftover();
    let (tls, cert) = ironrdp_tls::upgrade(tcp_inner, &p.host)
        .await
        .context("TLS")?;
    let spk = ironrdp_tls::extract_tls_server_public_key(&cert)
        .map(|k| k.to_vec())
        .unwrap_or_default();
    let mut f2 = Framed::<TokioStream<_>>::new(tls);
    let up = mark_as_upgraded(upgrade, &mut conn);

    struct NoNet;
    impl NetworkClient for NoNet {
        fn send(
            &mut self,
            _: &ironrdp::connector::sspi::generator::NetworkRequest,
        ) -> impl std::future::Future<Output = ConnectorResult<Vec<u8>>> {
            async { Err(general_err!("not supported")) }
        }
    }

    let cr = finalize_xrdp_compat(up, conn, &mut f2, &mut NoNet, sn, spk).await?;
    let mut framed = f2;

    let w = cr.desktop_size.width;
    let h = cr.desktop_size.height;
    let mut buf4 = Vec::with_capacity(5);
    buf4.extend_from_slice(&w.to_le_bytes());
    buf4.extend_from_slice(&h.to_le_bytes());
    buf4.push(0); // external_window = false (embedded via IronRDP)
    write_msg(out, MSG_CONNECTED, &buf4)?;

    let mut stage = ActiveStage::new(cr);
    let mut img = DecodedImage::new(PixelFormat::BgrX32, w, h);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    tokio::task::spawn_blocking(move || {
        let mut si = io::stdin().lock();
        loop {
            let mut hdr = [0u8; 5];
            if si.read_exact(&mut hdr).is_err() {
                break;
            }
            let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
            let mut pl = vec![0u8; len];
            if len > 0 && si.read_exact(&mut pl).is_err() {
                break;
            }
            let mut m = vec![hdr[0]];
            m.extend_from_slice(&pl);
            if tx.send(m).is_err() {
                break;
            }
            if hdr[0] == CMD_SHUTDOWN {
                break;
            }
        }
    });

    // Persistent input database — tracks mouse position and button state
    // across events so the RDP server receives correct coordinates.
    let mut input_db = ironrdp::input::Database::new();

    // Use tokio::select! so input commands and server PDUs are processed
    // concurrently — neither side blocks the other.
    loop {
        // Drain all pending input commands first (non-blocking)
        while let Ok(c) = rx.try_recv() {
            if !c.is_empty() {
                if c[0] == CMD_SHUTDOWN {
                    do_cmd(&c, &mut stage, &mut img, &mut framed, &mut input_db).await?;
                    return Ok(());
                }
                do_cmd(&c, &mut stage, &mut img, &mut framed, &mut input_db).await?;
            }
        }

        // Wait for either a server PDU or an input command, whichever comes first
        tokio::select! {
            cmd = rx.recv() => {
                match cmd {
                    Some(c) if !c.is_empty() => {
                        if c[0] == CMD_SHUTDOWN {
                            do_cmd(&c, &mut stage, &mut img, &mut framed, &mut input_db).await?;
                            return Ok(());
                        }
                        do_cmd(&c, &mut stage, &mut img, &mut framed, &mut input_db).await?;
                    }
                    None => {
                        // Parent closed the pipe — exit gracefully
                        return Ok(());
                    }
                    _ => {}
                }
            }

            pdu_result = framed.read_pdu() => {
                let (action, data) = pdu_result
                    .map_err(|e| anyhow::anyhow!("read: {e}"))?;
                for o in stage
                    .process(&mut img, action, &data)
                    .map_err(|e| anyhow::anyhow!("process: {e}"))?
                {
                    match o {
                        ActiveStageOutput::ResponseFrame(d) => {
                            framed
                                .write_all(&d)
                                .await
                                .map_err(|e| anyhow::anyhow!("write: {e}"))?;
                        }
                        ActiveStageOutput::GraphicsUpdate(region) => {
                            let reg_x = region.left;
                            let reg_y = region.top;
                            let reg_w = region.right.saturating_sub(region.left).saturating_add(1);
                            let reg_h = region.bottom.saturating_sub(region.top).saturating_add(1);

                            let stride = w as usize * 4;
                            let mut b = Vec::with_capacity(10 + reg_w as usize * reg_h as usize * 4);
                            b.extend_from_slice(&reg_x.to_le_bytes());
                            b.extend_from_slice(&reg_y.to_le_bytes());
                            b.extend_from_slice(&reg_w.to_le_bytes());
                            b.extend_from_slice(&reg_h.to_le_bytes());
                            b.extend_from_slice(&32u16.to_le_bytes());

                            let px = img.data();
                            for row in reg_y..(reg_y + reg_h) {
                                let start = row as usize * stride + reg_x as usize * 4;
                                let end = start + reg_w as usize * 4;
                                if end <= px.len() {
                                    b.extend_from_slice(&px[start..end]);
                                }
                            }
                            write_msg(out, MSG_BITMAP, &b)?;
                        }
                        ActiveStageOutput::Terminate(r) => {
                            write_msg(out, MSG_CLOSED, format!("{r:?}").as_bytes())?;
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FreeRDP backend
// ---------------------------------------------------------------------------

/// Locate a usable FreeRDP client binary on the system.
///
/// On macOS the X11-based `xfreerdp` requires XQuartz, so we prefer the
/// SDL-based `sdl-freerdp3` / `sdl-freerdp` which work natively.
fn find_freerdp_binary() -> Option<std::path::PathBuf> {
    // Order matters: prefer SDL clients (work without X11 on macOS),
    // then fall back to xfreerdp variants.
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["sdl-freerdp3", "sdl-freerdp", "xfreerdp3", "xfreerdp"]
    } else {
        &["xfreerdp3", "xfreerdp", "sdl-freerdp3", "sdl-freerdp"]
    };

    for name in candidates {
        // Check well-known paths first (child processes may not inherit full PATH)
        for dir in &[
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/snap/bin",
        ] {
            let p = std::path::PathBuf::from(dir).join(name);
            if p.exists() {
                return Some(p);
            }
        }
        // Fall back to PATH lookup
        if let Ok(output) = std::process::Command::new("which")
            .arg(name)
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Some(std::path::PathBuf::from(path));
                }
            }
        }
    }
    None
}

/// Build the xfreerdp command-line arguments.
fn build_freerdp_args(p: &Params, is_v3: bool) -> Vec<String> {
    let mut args = Vec::new();

    // Connection target
    args.push(format!("/v:{}:{}", p.host, p.port));
    args.push(format!("/u:{}", p.username));
    args.push(format!("/p:{}", p.password));

    // Resolution / scaling mode (these are mutually exclusive in FreeRDP)
    match p.freerdp_resize_mode.as_str() {
        "smart_sizing" => {
            args.push(format!("/size:{}x{}", p.width, p.height));
            args.push("/smart-sizing".into());
        }
        "static" => {
            args.push(format!("/size:{}x{}", p.width, p.height));
        }
        _ => {
            // "dynamic_resolution" — session resizes with the window
            args.push("/dynamic-resolution".into());
        }
    }

    // Graphics pipeline (required for gnome-remote-desktop)
    args.push("/gfx".into());
    args.push("/gdi:sw".into());
    args.push(format!("/bpp:{}", 32));

    // Security: accept certificates automatically
    if is_v3 {
        args.push("/cert:ignore".into());
    } else {
        args.push("/cert-ignore".into());
        args.push("/cert-tofu".into());
    }

    // NLA (CredSSP) — gnome-remote-desktop requires it
    args.push("/sec:nla".into());

    // Disable audio to reduce complexity
    args.push("/audio-mode:none".into());

    args
}

/// Detect whether the binary is FreeRDP 3.x by checking its version output.
fn is_freerdp_v3(bin: &std::path::Path) -> bool {
    if let Ok(output) = std::process::Command::new(bin)
        .arg("--version")
        .output()
    {
        let ver = String::from_utf8_lossy(&output.stdout);
        let ver_err = String::from_utf8_lossy(&output.stderr);
        ver.contains("version 3.") || ver_err.contains("version 3.")
            || bin.file_name().map_or(false, |n| {
                let s = n.to_string_lossy();
                s.contains("freerdp3") || s.contains("freerdp-3")
            })
    } else {
        bin.file_name().map_or(false, |n| {
            let s = n.to_string_lossy();
            s.contains("freerdp3") || s.contains("freerdp-3")
        })
    }
}

/// FreeRDP backend: spawns xfreerdp as a child process.
///
/// Since FreeRDP manages its own window and input, we report Connected
/// once the process starts and Closed when it exits. Mouse/keyboard
/// commands from the egui side are ignored (FreeRDP handles its own input
/// in its own window).
async fn freerdp_session(p: &Params, out: &mut impl Write) -> Result<()> {
    let bin = find_freerdp_binary()
        .ok_or_else(|| anyhow::anyhow!(
            "FreeRDP not found. Install it with: brew install freerdp (macOS) \
             or apt install freerdp2-x11 (Linux). \
             On macOS the SDL client (sdl-freerdp) is preferred over xfreerdp."
        ))?;

    let v3 = is_freerdp_v3(&bin);
    let args = build_freerdp_args(p, v3);

    eprintln!("[e-sh-rdp] Launching FreeRDP: {} {}", bin.display(), args.join(" "));

    let mut child = std::process::Command::new(&bin)
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn FreeRDP ({bin:?}): {e}"))?;

    // Report connected immediately — FreeRDP opens its own window
    let mut buf4 = Vec::with_capacity(5);
    buf4.extend_from_slice(&p.width.to_le_bytes());
    buf4.extend_from_slice(&p.height.to_le_bytes());
    buf4.push(1); // external_window = true (FreeRDP manages its own window)
    write_msg(out, MSG_CONNECTED, &buf4)?;

    // Drain stdin commands in a background thread (discard them — FreeRDP
    // handles its own input). This prevents the parent from blocking on
    // a full pipe.
    let stdin_drain = tokio::task::spawn_blocking(|| {
        let mut si = io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            match si.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // Check for shutdown command
                    if buf[..n].contains(&CMD_SHUTDOWN) {
                        break;
                    }
                }
            }
        }
    });

    // Collect stderr for error reporting
    let stderr_handle = child.stderr.take();
    let stderr_thread = std::thread::spawn(move || {
        let mut output = String::new();
        if let Some(mut stderr) = stderr_handle {
            let _ = stderr.read_to_string(&mut output);
        }
        output
    });

    // Wait for FreeRDP to exit
    let status = child.wait().context("waiting for FreeRDP")?;
    stdin_drain.abort();

    let stderr_output = stderr_thread.join().unwrap_or_default();
    if !stderr_output.is_empty() {
        eprintln!("[e-sh-rdp] FreeRDP stderr:\n{stderr_output}");
    }

    if status.success() {
        Ok(())
    } else {
        let code = status.code().unwrap_or(-1);
        let hint = if stderr_output.contains("ERRCONNECT_CONNECT_TRANSPORT_FAILED")
            || stderr_output.contains("unable to connect")
        {
            " (connection refused — is the RDP server running?)"
        } else if stderr_output.contains("LOGON_FAILED")
            || stderr_output.contains("ERRCONNECT_LOGON_FAILURE")
        {
            " (authentication failed — check username/password)"
        } else {
            ""
        };
        Err(anyhow::anyhow!(
            "FreeRDP exited with code {code}{hint}"
        ))
    }
}

// ---------------------------------------------------------------------------
// IronRDP input command handling
// ---------------------------------------------------------------------------

async fn do_cmd<S: ironrdp_tokio::FramedWrite + ironrdp_tokio::FramedRead>(
    c: &[u8],
    stage: &mut ironrdp::session::ActiveStage,
    img: &mut ironrdp::session::image::DecodedImage,
    f: &mut ironrdp_tokio::Framed<S>,
    input_db: &mut ironrdp::input::Database,
) -> Result<()> {
    use ironrdp::input::{MouseButton, Operation};
    use ironrdp::pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
    use ironrdp_tokio::FramedWrite as _;

    match c[0] {
        CMD_KEY if c.len() >= 4 => {
            let sc = u16::from_le_bytes([c[1], c[2]]);
            let mut fl = KeyboardFlags::empty();
            if c[3] == 0 {
                fl |= KeyboardFlags::RELEASE;
            }
            let ev = FastPathInputEvent::KeyboardEvent(fl, sc as u8);
            for o in stage
                .process_fastpath_input(img, &[ev])
                .map_err(|e| anyhow::anyhow!("{e}"))?
            {
                if let ironrdp::session::ActiveStageOutput::ResponseFrame(d) = o {
                    f.write_all(&d).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
        }
        CMD_MOUSE_MOVE if c.len() >= 5 => {
            let x = u16::from_le_bytes([c[1], c[2]]);
            let y = u16::from_le_bytes([c[3], c[4]]);
            stage.update_mouse_pos(x, y);
            let ops = [Operation::MouseMove(ironrdp::input::MousePosition { x, y })];
            let evs: Vec<_> = input_db.apply(ops.into_iter()).to_vec();
            if !evs.is_empty() {
                for o in stage
                    .process_fastpath_input(img, &evs)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                {
                    if let ironrdp::session::ActiveStageOutput::ResponseFrame(d) = o {
                        f.write_all(&d).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                }
            }
        }
        CMD_MOUSE_BUTTON if c.len() >= 6 => {
            let x = u16::from_le_bytes([c[1], c[2]]);
            let y = u16::from_le_bytes([c[3], c[4]]);
            stage.update_mouse_pos(x, y);
            let btn = match c[5] {
                0 => MouseButton::Left,
                1 => MouseButton::Right,
                _ => MouseButton::Middle,
            };
            let down = c.get(6).copied().unwrap_or(1) != 0;

            // Send a move first to ensure position is current, then the button event
            let move_op = Operation::MouseMove(ironrdp::input::MousePosition { x, y });
            let btn_op = if down {
                Operation::MouseButtonPressed(btn)
            } else {
                Operation::MouseButtonReleased(btn)
            };
            let evs: Vec<_> = input_db.apply([move_op, btn_op].into_iter()).to_vec();
            if !evs.is_empty() {
                for o in stage
                    .process_fastpath_input(img, &evs)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                {
                    if let ironrdp::session::ActiveStageOutput::ResponseFrame(d) = o {
                        f.write_all(&d).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                }
            }
        }
        CMD_MOUSE_SCROLL if c.len() >= 7 => {
            let x = u16::from_le_bytes([c[1], c[2]]);
            let y = u16::from_le_bytes([c[3], c[4]]);
            let delta = i16::from_le_bytes([c[5], c[6]]);
            stage.update_mouse_pos(x, y);

            let move_op = Operation::MouseMove(ironrdp::input::MousePosition { x, y });
            let scroll_op = if delta > 0 {
                Operation::WheelRotations(ironrdp::input::WheelRotations {
                    is_vertical: true,
                    rotation_units: delta,
                })
            } else {
                Operation::WheelRotations(ironrdp::input::WheelRotations {
                    is_vertical: true,
                    rotation_units: delta,
                })
            };
            let evs: Vec<_> = input_db.apply([move_op, scroll_op].into_iter()).to_vec();
            if !evs.is_empty() {
                for o in stage
                    .process_fastpath_input(img, &evs)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                {
                    if let ironrdp::session::ActiveStageOutput::ResponseFrame(d) = o {
                        f.write_all(&d).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                    }
                }
            }
        }
        CMD_SHUTDOWN => {
            for o in stage
                .graceful_shutdown()
                .map_err(|e| anyhow::anyhow!("{e}"))?
            {
                if let ironrdp::session::ActiveStageOutput::ResponseFrame(d) = o {
                    f.write_all(&d).await.map_err(|e| anyhow::anyhow!("{e}"))?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Custom replacement for `ironrdp_tokio::connect_finalize` that handles the
/// xrdp interop quirk: xrdp sends a `ServerDeactivateAll` PDU as the very
/// first IO-channel message during CapabilitiesExchange (right after the
/// server skips license exchange), then sends `ServerDemandActive`. The
/// stock IronRDP connector errors out with
/// "unexpected Share Control Pdu (expected ServerDemandActive)".
///
/// We drive `ClientConnector` step-by-step ourselves and silently drop the
/// stray `ServerDeactivateAll` while the connector is in the
/// CapabilitiesExchange state, so the next read picks up the real
/// `ServerDemandActive` and the handshake completes normally.
async fn finalize_xrdp_compat<S>(
    _upgraded: ironrdp_tokio::Upgraded,
    mut connector: ironrdp::connector::ClientConnector,
    framed: &mut ironrdp_tokio::Framed<S>,
    network_client: &mut impl ironrdp_tokio::NetworkClient,
    server_name: ironrdp::connector::ServerName,
    server_public_key: Vec<u8>,
) -> Result<ironrdp::connector::ConnectionResult>
where
    S: ironrdp_tokio::FramedRead + ironrdp_tokio::FramedWrite,
{
    use ironrdp::connector::credssp::CredsspSequence;
    use ironrdp::connector::sspi::generator::GeneratorState;
    use ironrdp::connector::{
        ClientConnectorState, ConnectorResult, Sequence, State, legacy,
    };
    use ironrdp::pdu::rdp::headers::ShareControlPdu;
    use ironrdp::core::WriteBuf;
    use ironrdp_tokio::FramedWrite as _;

    let mut buf = WriteBuf::new();

    // ---- CredSSP ----------------------------------------------------------
    if connector.should_perform_credssp() {
        let selected_protocol = match connector.state {
            ClientConnectorState::Credssp { selected_protocol, .. } => selected_protocol,
            _ => return Err(anyhow::anyhow!("invalid connector state for CredSSP")),
        };

        let (mut sequence, mut ts_request) = CredsspSequence::init(
            connector.config.credentials.clone(),
            connector.config.domain.as_deref(),
            selected_protocol,
            server_name,
            server_public_key,
            None,
        )
        .map_err(|e| anyhow::anyhow!("CredsspSequence::init: {e}"))?;

        loop {
            // Drive the (possibly-suspending) sspi generator using `network_client`.
            let client_state = {
                let mut gen = sequence.process_ts_request(ts_request);
                let mut state = gen.start();
                loop {
                    match state {
                        GeneratorState::Suspended(req) => {
                            let resp = network_client
                                .send(&req)
                                .await
                                .map_err(|e| anyhow::anyhow!("CredSSP network: {e}"))?;
                            state = gen.resume(Ok(resp));
                        }
                        GeneratorState::Completed(cs) => {
                            break cs.map_err(|e| {
                                anyhow::anyhow!("CredSSP processing: {e}")
                            })?;
                        }
                    }
                }
            };

            buf.clear();
            let written = sequence
                .handle_process_result(client_state, &mut buf)
                .map_err(|e| anyhow::anyhow!("CredSSP handle: {e}"))?;
            if let Some(n) = written.size() {
                framed
                    .write_all(&buf[..n])
                    .await
                    .map_err(|e| anyhow::anyhow!("CredSSP write: {e}"))?;
            }

            let Some(hint) = sequence.next_pdu_hint() else { break };
            let pdu = framed
                .read_by_hint(hint)
                .await
                .map_err(|e| anyhow::anyhow!("CredSSP read: {e}"))?;
            match sequence
                .decode_server_message(&pdu)
                .map_err(|e| anyhow::anyhow!("CredSSP decode: {e}"))?
            {
                Some(next) => ts_request = next,
                None => break,
            }
        }

        connector.mark_credssp_as_done();
    }

    // ---- Remaining sequence with xrdp DeactivateAll workaround ------------
    loop {
        if let ClientConnectorState::Connected { .. } = connector.state {
            break;
        }

        let result: ConnectorResult<()> = async {
            let next_hint = connector.next_pdu_hint();
            let input: Vec<u8> = if let Some(hint) = next_hint {
                let pdu = framed
                    .read_by_hint(hint)
                    .await
                    .map_err(|e| ironrdp::connector::custom_err!("read", e))?;

                // xrdp workaround: in CapabilitiesExchange, drop a stray
                // ServerDeactivateAll so the connector can wait for the real
                // ServerDemandActive that xrdp sends right after.
                if matches!(connector.state, ClientConnectorState::CapabilitiesExchange { .. }) {
                    if let Ok(sdi) = legacy::decode_send_data_indication(&pdu) {
                        if let Ok(share) = legacy::decode_share_control(sdi) {
                            if matches!(share.pdu, ShareControlPdu::ServerDeactivateAll(_)) {
                                eprintln!(
                                    "[e-sh-rdp] xrdp compat: dropping stray ServerDeactivateAll \
                                     before ServerDemandActive"
                                );
                                return Ok(());
                            }
                        }
                    }
                }

                pdu.to_vec()
            } else {
                Vec::new()
            };

            buf.clear();
            let written = connector.step(&input, &mut buf)?;
            if let Some(n) = written.size() {
                framed
                    .write_all(&buf[..n])
                    .await
                    .map_err(|e| ironrdp::connector::custom_err!("write", e))?;
            }
            Ok(())
        }
        .await;

        result.map_err(|e| {
            anyhow::anyhow!(
                "connect_finalize ({}): {e}",
                connector.state.name()
            )
        })?;
    }

    match std::mem::replace(
        &mut connector.state,
        ClientConnectorState::Consumed,
    ) {
        ClientConnectorState::Connected { result } => Ok(result),
        _ => Err(anyhow::anyhow!("connector ended in non-Connected state")),
    }
}
