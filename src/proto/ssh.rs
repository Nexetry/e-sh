use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use parking_lot::Mutex;
use russh::client::{self, Handle, Handler, Msg, Session};
use russh::keys::agent::client::AgentClient;
use russh::keys::ssh_key::HashAlg;
use russh::keys::{PrivateKeyWithHashAlg, load_secret_key};
use russh::{Channel, ChannelMsg, Disconnect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::oneshot;

use crate::config::host_keys::{
    HostKeyDecision, HostKeyPrompt, HostKeyPromptKind, HostKeyStore, HostKeyVerdict,
};
use crate::config::store::ConfigPaths;
use crate::core::connection::{AuthMethod, Connection, Tunnel, TunnelKind};

pub enum SessionEvent {
    Output(Vec<u8>),
    Closed(Option<String>),
}

pub enum SessionCommand {
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Disconnect,
}

#[derive(Debug, Clone)]
pub enum TunnelStatusKind {
    Pending,
    Listening { bound_port: u16 },
    Failed { error: String },
    Disabled,
}

#[derive(Debug, Clone)]
pub struct TunnelStatus {
    pub tunnel_id: uuid::Uuid,
    pub kind: TunnelKind,
    pub listen_address: String,
    pub listen_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    pub status: TunnelStatusKind,
}

impl TunnelStatus {
    pub fn describe(&self) -> String {
        match self.kind {
            TunnelKind::Local => format!(
                "L {}:{} -> {}:{}",
                self.listen_address, self.listen_port, self.remote_host, self.remote_port
            ),
            TunnelKind::Remote => format!(
                "R {}:{} -> {}:{}",
                self.listen_address, self.listen_port, self.remote_host, self.remote_port
            ),
            TunnelKind::Dynamic => {
                format!("D {}:{} (SOCKS5)", self.listen_address, self.listen_port)
            }
        }
    }
}

pub type TunnelStatusMap = Arc<Mutex<HashMap<uuid::Uuid, TunnelStatus>>>;

pub struct SessionHandle {
    pub events: UnboundedReceiver<SessionEvent>,
    pub commands: UnboundedSender<SessionCommand>,
    pub tunnels: TunnelStatusMap,
}

#[derive(Clone)]
pub struct HostKeyContext {
    pub store: Arc<Mutex<HostKeyStore>>,
    pub paths: Arc<ConfigPaths>,
    pub prompts: UnboundedSender<HostKeyPrompt>,
}

pub struct Client {
    host: String,
    port: u16,
    host_keys: HostKeyContext,
    remote_forwards: Arc<Mutex<HashMap<u32, (String, u16)>>>,
    x11_target: Option<X11Target>,
}

#[derive(Clone, Debug)]
pub(crate) enum X11Target {
    #[cfg(unix)]
    UnixSocket(PathBuf),
    Tcp {
        host: String,
        port: u16,
    },
}

impl X11Target {
    pub(crate) fn from_display() -> Option<Self> {
        let display = std::env::var("DISPLAY").ok()?;
        let display = display.trim();
        if display.is_empty() {
            return None;
        }
        // Forms: ":0", ":0.0", "host:0", "host:0.0", "/tmp/.X11-unix/X0"
        if display.starts_with('/') {
            #[cfg(unix)]
            {
                return Some(X11Target::UnixSocket(PathBuf::from(display)));
            }
            #[cfg(not(unix))]
            {
                return None;
            }
        }
        let (host, tail) = display.rsplit_once(':')?;
        let display_num: u16 = tail.split('.').next()?.parse().ok()?;
        if host.is_empty() {
            #[cfg(unix)]
            {
                let sock = PathBuf::from(format!("/tmp/.X11-unix/X{display_num}"));
                if sock.exists() {
                    return Some(X11Target::UnixSocket(sock));
                }
                return Some(X11Target::Tcp {
                    host: "127.0.0.1".into(),
                    port: 6000 + display_num,
                });
            }
            #[cfg(not(unix))]
            {
                return Some(X11Target::Tcp {
                    host: "127.0.0.1".into(),
                    port: 6000 + display_num,
                });
            }
        }
        Some(X11Target::Tcp {
            host: host.to_string(),
            port: 6000 + display_num,
        })
    }
}

impl Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &russh::keys::ssh_key::PublicKey,
    ) -> Result<bool, Self::Error> {
        let algorithm = server_public_key.algorithm().as_str().to_string();
        let fingerprint = server_public_key
            .fingerprint(HashAlg::Sha256)
            .to_string();

        let verdict = {
            let guard = self.host_keys.store.lock();
            guard.check(&self.host, self.port, &algorithm, &fingerprint)
        };

        let kind = match verdict {
            HostKeyVerdict::KnownMatch => return Ok(true),
            HostKeyVerdict::NewHost { .. } => HostKeyPromptKind::NewHost,
            HostKeyVerdict::Mismatch { expected, .. } => HostKeyPromptKind::Mismatch { expected },
        };

        let (tx, rx) = oneshot::channel();
        let prompt = HostKeyPrompt {
            host: self.host.clone(),
            port: self.port,
            algorithm: algorithm.clone(),
            fingerprint: fingerprint.clone(),
            kind: kind.clone(),
            responder: tx,
        };
        if self.host_keys.prompts.send(prompt).is_err() {
            tracing::error!("host key prompt channel closed - rejecting connection");
            return Ok(false);
        }

        let decision = match rx.await {
            Ok(d) => d,
            Err(_) => {
                tracing::error!("host key prompt cancelled - rejecting connection");
                return Ok(false);
            }
        };

        match decision {
            HostKeyDecision::Reject => {
                tracing::warn!(
                    host = %self.host,
                    port = self.port,
                    %algorithm,
                    %fingerprint,
                    "user rejected host key"
                );
                Ok(false)
            }
            HostKeyDecision::AcceptOnce => {
                tracing::info!(
                    host = %self.host,
                    port = self.port,
                    %algorithm,
                    %fingerprint,
                    "user accepted host key once (not persisted)"
                );
                Ok(true)
            }
            HostKeyDecision::AcceptAndSave => {
                let mut guard = self.host_keys.store.lock();
                guard.insert(
                    &self.host,
                    self.port,
                    algorithm.clone(),
                    fingerprint.clone(),
                );
                if let Err(e) = guard.save(&self.host_keys.paths) {
                    tracing::warn!(error = %e, "failed persisting host key");
                }
                tracing::info!(
                    host = %self.host,
                    port = self.port,
                    %algorithm,
                    %fingerprint,
                    "user accepted and saved host key"
                );
                Ok(true)
            }
        }
    }

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let lookup = {
            let map = self.remote_forwards.lock();
            map.get(&connected_port)
                .cloned()
                .or_else(|| map.get(&0).cloned())
        };
        let Some((target_host, target_port)) = lookup else {
            tracing::warn!(
                bound_address = %connected_address,
                bound_port = connected_port,
                "no remote-forward target registered for inbound channel; dropping"
            );
            return Ok(());
        };
        tokio::spawn(async move {
            let mut local = match TcpStream::connect((target_host.as_str(), target_port)).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        target = %format!("{target_host}:{target_port}"),
                        "remote forward target dial failed"
                    );
                    return;
                }
            };
            let mut remote = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
        });
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(target) = self.x11_target.clone() else {
            tracing::warn!("server opened X11 channel but no local X server target is configured");
            return Ok(());
        };
        tokio::spawn(async move {
            let mut remote = channel.into_stream();
            match target {
                #[cfg(unix)]
                X11Target::UnixSocket(path) => {
                    match tokio::net::UnixStream::connect(&path).await {
                        Ok(mut local) => {
                            let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, path = %path.display(), "x11 local unix connect failed");
                        }
                    }
                }
                X11Target::Tcp { host, port } => {
                    match TcpStream::connect((host.as_str(), port)).await {
                        Ok(mut local) => {
                            let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, target = %format!("{host}:{port}"), "x11 local tcp connect failed");
                        }
                    }
                }
            }
        });
        Ok(())
    }
}

pub fn spawn_session(
    rt: &tokio::runtime::Handle,
    chain: Vec<Connection>,
    host_keys: HostKeyContext,
    recorder: Option<crate::recording::Recorder>,
) -> SessionHandle {
    let (event_tx, event_rx) = unbounded_channel::<SessionEvent>();
    let (cmd_tx, cmd_rx) = unbounded_channel::<SessionCommand>();

    let tunnels: TunnelStatusMap = Arc::new(Mutex::new(HashMap::new()));
    if let Some(target) = chain.last() {
        let mut map = tunnels.lock();
        for t in &target.tunnels {
            map.insert(
                t.id,
                TunnelStatus {
                    tunnel_id: t.id,
                    kind: t.kind,
                    listen_address: t.listen_address.clone(),
                    listen_port: t.listen_port,
                    remote_host: t.remote_host.clone(),
                    remote_port: t.remote_port,
                    status: if t.enabled {
                        TunnelStatusKind::Pending
                    } else {
                        TunnelStatusKind::Disabled
                    },
                },
            );
        }
    }

    let event_tx_task = event_tx.clone();
    let tunnels_task = tunnels.clone();
    rt.spawn(async move {
        if let Err(e) = run_session(chain, host_keys, event_tx_task.clone(), cmd_rx, tunnels_task, recorder).await {
            let _ = event_tx_task.send(SessionEvent::Closed(Some(format!("{e:#}"))));
        } else {
            let _ = event_tx_task.send(SessionEvent::Closed(None));
        }
    });

    SessionHandle {
        events: event_rx,
        commands: cmd_tx,
        tunnels,
    }
}

pub async fn connect_and_authenticate(
    chain: &[Connection],
    host_keys: HostKeyContext,
) -> Result<Handle<Client>> {
    let (handle, _) = establish_session(chain, host_keys).await?;
    Ok(handle)
}

pub(crate) async fn establish_session(
    chain: &[Connection],
    host_keys: HostKeyContext,
) -> Result<(Handle<Client>, Arc<Mutex<HashMap<u32, (String, u16)>>>)> {
    if chain.is_empty() {
        return Err(anyhow!("empty connection chain"));
    }

    let target = chain.last().expect("chain non-empty");
    let mut cfg = client::Config::default();
    if target.keepalive_secs > 0 {
        cfg.keepalive_interval = Some(Duration::from_secs(target.keepalive_secs as u64));
        cfg.keepalive_max = 0; // don't disconnect on missed replies — just keep pinging
    }
    let config = Arc::new(cfg);

    let remote_forwards: Arc<Mutex<HashMap<u32, (String, u16)>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let target_x11 = if target.x11_forwarding {
        X11Target::from_display()
    } else {
        None
    };
    let last_idx = chain.len() - 1;

    let head = &chain[0];
    let head_client = Client {
        host: head.host.clone(),
        port: head.port,
        host_keys: host_keys.clone(),
        remote_forwards: remote_forwards.clone(),
        x11_target: if last_idx == 0 { target_x11.clone() } else { None },
    };
    let mut current = client::connect(config.clone(), (head.host.as_str(), head.port), head_client)
        .await
        .with_context(|| format!("connecting to {}:{}", head.host, head.port))?;
    authenticate(&mut current, head)
        .await
        .with_context(|| format!("authenticating to {}:{}", head.host, head.port))?;

    for (i, hop) in chain.iter().enumerate().skip(1) {
        let channel = current
            .channel_open_direct_tcpip(hop.host.clone(), hop.port as u32, "127.0.0.1", 0)
            .await
            .with_context(|| format!("opening tunnel to {}:{}", hop.host, hop.port))?;
        let stream = channel.into_stream();
        let hop_client = Client {
            host: hop.host.clone(),
            port: hop.port,
            host_keys: host_keys.clone(),
            remote_forwards: remote_forwards.clone(),
            x11_target: if i == last_idx { target_x11.clone() } else { None },
        };
        let mut next = client::connect_stream(config.clone(), stream, hop_client)
            .await
            .with_context(|| format!("ssh handshake to {}:{} via jump", hop.host, hop.port))?;
        authenticate(&mut next, hop)
            .await
            .with_context(|| format!("authenticating to {}:{} via jump", hop.host, hop.port))?;
        current = next;
    }

    Ok((current, remote_forwards))
}

async fn run_session(
    chain: Vec<Connection>,
    host_keys: HostKeyContext,
    events: UnboundedSender<SessionEvent>,
    mut commands: UnboundedReceiver<SessionCommand>,
    tunnel_statuses: TunnelStatusMap,
    mut recorder: Option<crate::recording::Recorder>,
) -> Result<()> {
    let target = chain
        .last()
        .cloned()
        .ok_or_else(|| anyhow!("empty connection chain"))?;

    if let Some(script) = target.before_script.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let _ = events.send(SessionEvent::Output(
            b"\r\n[e-sh] running before_script...\r\n".to_vec(),
        ));
        run_local_script(script, &target, true)
            .await
            .with_context(|| "before_script failed; aborting connection")?;
    }

    let session = establish_session(&chain, host_keys).await?;
    let (session, remote_forwards) = session;

    let mut channel = session
        .channel_open_session()
        .await
        .context("opening session channel")?;

    channel
        .request_pty(false, "xterm-256color", 80, 24, 0, 0, &[])
        .await
        .context("requesting PTY")?;

    if target.x11_forwarding {
        let cookie = random_x11_cookie();
        if let Err(e) = channel
            .request_x11(false, false, "MIT-MAGIC-COOKIE-1", cookie, 0)
            .await
        {
            tracing::warn!(error = %e, "x11 forwarding request failed; continuing without");
            let _ = events.send(SessionEvent::Output(
                format!("\r\n[e-sh] X11 forwarding request failed: {e}\r\n").into_bytes(),
            ));
        }
    }

    channel
        .request_shell(false)
        .await
        .context("requesting shell")?;

    for cmd in &target.remote_commands {
        let trimmed = cmd.trim_end_matches('\n');
        if trimmed.is_empty() {
            continue;
        }
        let mut line = trimmed.to_string();
        line.push('\n');
        if let Err(e) = channel.data(line.as_bytes()).await {
            tracing::warn!(error = %e, "failed to send remote_command to shell");
            break;
        }
    }

    if let Some(script) = target
        .after_connect_script
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let script_owned = script.to_string();
        let target_for_env = target.clone();
        tokio::spawn(async move {
            if let Err(e) = run_local_script(&script_owned, &target_for_env, false).await {
                tracing::warn!(error = %e, "after_connect_script failed");
            }
        });
    }

    let session_arc = Arc::new(session);

    let mut tunnel_tasks = Vec::new();
    for tunnel in target.tunnels.iter().filter(|t| t.enabled).cloned() {
        let tunnel_id = tunnel.id;
        match start_tunnel(session_arc.clone(), remote_forwards.clone(), tunnel.clone()).await {
            Ok((handle, bound_port)) => {
                if let Some(s) = tunnel_statuses.lock().get_mut(&tunnel_id) {
                    s.status = TunnelStatusKind::Listening { bound_port };
                }
                tunnel_tasks.push(handle);
            }
            Err(e) => {
                let err_str = format!("{e:#}");
                tracing::warn!(error = %e, kind = ?tunnel.kind, "failed to start tunnel");
                if let Some(s) = tunnel_statuses.lock().get_mut(&tunnel_id) {
                    s.status = TunnelStatusKind::Failed { error: err_str.clone() };
                }
                let _ = events.send(SessionEvent::Output(
                    format!("\r\n[e-sh] tunnel {} failed: {err_str}\r\n", tunnel.describe()).into_bytes(),
                ));
            }
        }
    }

    let result: Result<()> = loop {
        tokio::select! {
            cmd = commands.recv() => {
                match cmd {
                    Some(SessionCommand::Input(bytes)) => {
                        if channel.data(&bytes[..]).await.is_err() {
                            break Ok(());
                        }
                    }
                    Some(SessionCommand::Resize { cols, rows }) => {
                        let _ = channel.window_change(cols as u32, rows as u32, 0, 0).await;
                    }
                    Some(SessionCommand::Disconnect) | None => {
                        let _ = session_arc.disconnect(Disconnect::ByApplication, "client closing", "en").await;
                        break Ok(());
                    }
                }
            }
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if let Some(r) = recorder.as_ref() {
                            r.ssh_output(&data);
                        }
                        if events.send(SessionEvent::Output(data.to_vec())).is_err() {
                            break Ok(());
                        }
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if let Some(r) = recorder.as_ref() {
                            r.ssh_output(&data);
                        }
                        if events.send(SessionEvent::Output(data.to_vec())).is_err() {
                            break Ok(());
                        }
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | Some(ChannelMsg::ExitStatus { .. }) | None => {
                        break Ok(());
                    }
                    _ => {}
                }
            }
        }
    };

    for h in tunnel_tasks {
        h.abort();
    }

    if let Some(r) = recorder.take() {
        r.finish();
    }

    if let Some(script) = target
        .after_close_script
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let script_owned = script.to_string();
        let target_for_env = target.clone();
        tokio::spawn(async move {
            if let Err(e) = run_local_script(&script_owned, &target_for_env, false).await {
                tracing::warn!(error = %e, "after_close_script failed");
            }
        });
    }

    result
}

fn random_x11_cookie() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mut out = String::with_capacity(32);
    let mut state = nanos ^ (pid.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    for _ in 0..16 {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let byte = ((state >> 64) & 0xFF) as u8;
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

async fn run_local_script(
    script: &str,
    conn: &Connection,
    blocking: bool,
) -> Result<()> {
    use tokio::process::Command;
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.arg("/C").arg(script);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    };
    cmd.env("ESH_HOST", &conn.host)
        .env("ESH_PORT", conn.port.to_string())
        .env("ESH_USER", &conn.username)
        .env("ESH_NAME", &conn.name);
    if blocking {
        let status = cmd.status().await.context("spawning local script")?;
        if !status.success() {
            return Err(anyhow!("local script exited with {status}"));
        }
        Ok(())
    } else {
        let _ = cmd.spawn().context("spawning local script")?;
        Ok(())
    }
}

pub(crate) async fn authenticate(session: &mut Handle<Client>, conn: &Connection) -> Result<()> {
    let user = &conn.username;
    let authed = match &conn.auth {
        AuthMethod::Password { password } => {
            session
                .authenticate_password(user, password)
                .await
                .context("password authentication")?
                .success()
        }
        AuthMethod::PublicKey { path, passphrase } => {
            let resolved = expand_path(path)?;
            let key = load_secret_key(&resolved, passphrase.as_deref())
                .with_context(|| format!("loading key {}", resolved.display()))?;
            let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            session
                .authenticate_publickey(user, key_with_alg)
                .await
                .context("public key authentication")?
                .success()
        }
        AuthMethod::Agent => authenticate_with_agent(session, user).await?,
    };

    if !authed {
        return Err(anyhow!("authentication failed"));
    }
    Ok(())
}

#[cfg(unix)]
async fn authenticate_with_agent(session: &mut Handle<Client>, user: &str) -> Result<bool> {
    let mut agent = AgentClient::connect_env()
        .await
        .context("connecting to SSH agent (is SSH_AUTH_SOCK set?)")?;
    try_agent_identities(session, user, &mut agent).await
}

#[cfg(windows)]
async fn authenticate_with_agent(session: &mut Handle<Client>, user: &str) -> Result<bool> {
    // Prefer OpenSSH-for-Windows named pipe; fall back to Pageant.
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    match AgentClient::connect_named_pipe(OPENSSH_PIPE).await {
        Ok(mut agent) => try_agent_identities(session, user, &mut agent).await,
        Err(openssh_err) => match AgentClient::connect_pageant().await {
            Ok(mut agent) => try_agent_identities(session, user, &mut agent).await,
            Err(pageant_err) => Err(anyhow!(
                "connecting to SSH agent failed (OpenSSH pipe: {openssh_err}; Pageant: {pageant_err})"
            )),
        },
    }
}

#[cfg(not(any(unix, windows)))]
async fn authenticate_with_agent(_session: &mut Handle<Client>, _user: &str) -> Result<bool> {
    Err(anyhow!("SSH agent authentication is not supported on this platform"))
}

async fn try_agent_identities<S>(
    session: &mut Handle<Client>,
    user: &str,
    agent: &mut AgentClient<S>,
) -> Result<bool>
where
    S: russh::keys::agent::client::AgentStream + Unpin + Send + 'static,
{
    let identities = agent
        .request_identities()
        .await
        .context("requesting identities from SSH agent")?;
    if identities.is_empty() {
        return Err(anyhow!(
            "SSH agent has no identities loaded (try `ssh-add`)"
        ));
    }
    let mut last_err: Option<String> = None;
    let mut succeeded = false;
    for identity in identities {
        let public = identity.public_key().into_owned();
        let fp = public.fingerprint(HashAlg::Sha256).to_string();
        tracing::info!(fingerprint = %fp, "trying agent identity");
        match session
            .authenticate_publickey_with(user, public, None, agent)
            .await
        {
            Ok(result) => {
                if result.success() {
                    succeeded = true;
                    break;
                } else {
                    last_err = Some(format!("server rejected identity {fp}"));
                }
            }
            Err(e) => {
                last_err = Some(format!("agent sign error for {fp}: {e}"));
            }
        }
    }
    if !succeeded {
        return Err(anyhow!(
            "agent authentication failed: {}",
            last_err.unwrap_or_else(|| "no identity accepted".into())
        ));
    }
    Ok(true)
}

async fn start_tunnel(
    session: Arc<Handle<Client>>,
    remote_forwards: Arc<Mutex<HashMap<u32, (String, u16)>>>,
    tunnel: Tunnel,
) -> Result<(tokio::task::JoinHandle<()>, u16)> {
    match tunnel.kind {
        TunnelKind::Local => {
            let listener = TcpListener::bind((tunnel.listen_address.as_str(), tunnel.listen_port))
                .await
                .with_context(|| {
                    format!(
                        "binding local forward {}:{}",
                        tunnel.listen_address, tunnel.listen_port
                    )
                })?;
            tracing::info!(target = %tunnel.describe(), "local tunnel listening");
            let bound = listener.local_addr().map(|a| a.port()).unwrap_or(tunnel.listen_port);
            let remote_host = tunnel.remote_host.clone();
            let remote_port = tunnel.remote_port;
            let handle = tokio::spawn(async move {
                loop {
                    let (mut local, peer) = match listener.accept().await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(error = %e, "local tunnel accept failed");
                            break;
                        }
                    };
                    let session = session.clone();
                    let remote_host = remote_host.clone();
                    tokio::spawn(async move {
                        let channel = match session
                            .channel_open_direct_tcpip(
                                remote_host,
                                remote_port as u32,
                                peer.ip().to_string(),
                                peer.port() as u32,
                            )
                            .await
                        {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(error = %e, "local tunnel channel open failed");
                                return;
                            }
                        };
                        let mut remote = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
                    });
                }
            });
            Ok((handle, bound))
        }
        TunnelKind::Remote => {
            let bound_port = session
                .tcpip_forward(tunnel.listen_address.clone(), tunnel.listen_port as u32)
                .await
                .with_context(|| {
                    format!(
                        "requesting remote forward {}:{}",
                        tunnel.listen_address, tunnel.listen_port
                    )
                })?;
            let effective_port = if bound_port == 0 {
                tunnel.listen_port as u32
            } else {
                bound_port
            };
            {
                let mut map = remote_forwards.lock();
                map.insert(
                    effective_port,
                    (tunnel.remote_host.clone(), tunnel.remote_port),
                );
                if tunnel.listen_port as u32 != effective_port {
                    map.insert(
                        tunnel.listen_port as u32,
                        (tunnel.remote_host.clone(), tunnel.remote_port),
                    );
                }
            }
            tracing::info!(
                bind = %tunnel.listen_address,
                port = effective_port,
                target = %format!("{}:{}", tunnel.remote_host, tunnel.remote_port),
                "remote tunnel established"
            );
            let session_for_cancel = session.clone();
            let bind_addr = tunnel.listen_address.clone();
            let handle = tokio::spawn(async move {
                let result = std::future::pending::<()>().await;
                let _ = session_for_cancel
                    .cancel_tcpip_forward(bind_addr, effective_port)
                    .await;
                result
            });
            Ok((handle, effective_port as u16))
        }
        TunnelKind::Dynamic => {
            let listener = TcpListener::bind((tunnel.listen_address.as_str(), tunnel.listen_port))
                .await
                .with_context(|| {
                    format!(
                        "binding dynamic forward {}:{}",
                        tunnel.listen_address, tunnel.listen_port
                    )
                })?;
            tracing::info!(target = %tunnel.describe(), "dynamic SOCKS5 tunnel listening");
            let bound = listener.local_addr().map(|a| a.port()).unwrap_or(tunnel.listen_port);
            let handle = tokio::spawn(async move {
                loop {
                    let (mut local, peer) = match listener.accept().await {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!(error = %e, "dynamic tunnel accept failed");
                            break;
                        }
                    };
                    let session = session.clone();
                    tokio::spawn(async move {
                        let (host, port) = match socks5_handshake(&mut local).await {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(error = %e, "socks5 handshake failed");
                                return;
                            }
                        };
                        let channel = match session
                            .channel_open_direct_tcpip(
                                host,
                                port as u32,
                                peer.ip().to_string(),
                                peer.port() as u32,
                            )
                            .await
                        {
                            Ok(c) => c,
                            Err(e) => {
                                tracing::warn!(error = %e, "socks5 upstream open failed");
                                let _ = local
                                    .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                                    .await;
                                return;
                            }
                        };
                        if local
                            .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                            .await
                            .is_err()
                        {
                            return;
                        }
                        let mut remote = channel.into_stream();
                        let _ = tokio::io::copy_bidirectional(&mut local, &mut remote).await;
                    });
                }
            });
            Ok((handle, bound))
        }
    }
}

async fn socks5_handshake(stream: &mut tokio::net::TcpStream) -> Result<(String, u16)> {
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != 0x05 {
        return Err(anyhow!("not a SOCKS5 client"));
    }
    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    stream.read_exact(&mut methods).await?;
    stream.write_all(&[0x05, 0x00]).await?;

    let mut req = [0u8; 4];
    stream.read_exact(&mut req).await?;
    if req[0] != 0x05 || req[1] != 0x01 {
        let _ = stream.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
        return Err(anyhow!("only SOCKS5 CONNECT supported"));
    }
    let host = match req[3] {
        0x01 => {
            let mut a = [0u8; 4];
            stream.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut buf = vec![0u8; len[0] as usize];
            stream.read_exact(&mut buf).await?;
            String::from_utf8(buf).context("socks5 hostname not utf-8")?
        }
        0x04 => {
            let mut a = [0u8; 16];
            stream.read_exact(&mut a).await?;
            std::net::Ipv6Addr::from(a).to_string()
        }
        other => {
            let _ = stream.write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await;
            return Err(anyhow!("unknown SOCKS5 ATYP: {other}"));
        }
    };
    let mut port_buf = [0u8; 2];
    stream.read_exact(&mut port_buf).await?;
    let port = u16::from_be_bytes(port_buf);
    Ok((host, port))
}

fn expand_path(input: &str) -> Result<PathBuf> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("key path is empty"));
    }
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow!("cannot expand ~: HOME not set"))?;
        PathBuf::from(home).join(rest)
    } else if trimmed == "~" {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| anyhow!("cannot expand ~: HOME not set"))?;
        PathBuf::from(home)
    } else {
        PathBuf::from(trimmed)
    };
    if !expanded.exists() {
        return Err(anyhow!("key file not found: {}", expanded.display()));
    }
    Ok(expanded)
}
