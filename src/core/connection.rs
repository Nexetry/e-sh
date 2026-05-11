use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RdpBackend {
    /// Try IronRDP first, fall back to FreeRDP if the server requires GFX.
    Auto,
    /// Always use the built-in IronRDP backend (Windows / xrdp).
    Ironrdp,
    /// Always use FreeRDP (required for gnome-remote-desktop).
    Freerdp,
}

impl Default for RdpBackend {
    fn default() -> Self {
        RdpBackend::Auto
    }
}

impl RdpBackend {
    pub fn label(self) -> &'static str {
        match self {
            RdpBackend::Auto => "Auto",
            RdpBackend::Ironrdp => "Built-in (IronRDP)",
            RdpBackend::Freerdp => "FreeRDP (external)",
        }
    }

    pub const ALL: [RdpBackend; 3] = [RdpBackend::Auto, RdpBackend::Ironrdp, RdpBackend::Freerdp];
}

/// How the FreeRDP external window handles resolution / scaling.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FreeRdpResizeMode {
    /// Negotiate dynamic resolution — the session resizes when the window is
    /// resized (requires server support).
    DynamicResolution,
    /// Scale the remote desktop to fit the window (client-side scaling).
    SmartSizing,
    /// Fixed resolution — use the width/height from the connection config.
    Static,
}

impl Default for FreeRdpResizeMode {
    fn default() -> Self {
        FreeRdpResizeMode::DynamicResolution
    }
}

impl FreeRdpResizeMode {
    pub fn label(self) -> &'static str {
        match self {
            FreeRdpResizeMode::DynamicResolution => "Dynamic resolution",
            FreeRdpResizeMode::SmartSizing => "Smart sizing (client scale)",
            FreeRdpResizeMode::Static => "Static resolution",
        }
    }

    pub const ALL: [FreeRdpResizeMode; 3] = [
        FreeRdpResizeMode::DynamicResolution,
        FreeRdpResizeMode::SmartSizing,
        FreeRdpResizeMode::Static,
    ];
}

/// RDP security mode preference.
///
/// - `negotiate` (default): allow TLS and NLA (CredSSP) and let the server pick.
/// - `nla`: require NLA (CredSSP).
/// - `tls`: require TLS (SSL) without NLA.
/// - `rdp`: force legacy "Standard RDP security" (no TLS/NLA). Use only when
///   the server is configured to select `STANDARD_RDP_SECURITY`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RdpSecurityMode {
    Negotiate,
    Nla,
    Tls,
    Rdp,
}

impl Default for RdpSecurityMode {
    fn default() -> Self {
        RdpSecurityMode::Negotiate
    }
}

impl RdpSecurityMode {
    pub fn label(self) -> &'static str {
        match self {
            RdpSecurityMode::Negotiate => "Negotiate (recommended)",
            RdpSecurityMode::Nla => "NLA (CredSSP)",
            RdpSecurityMode::Tls => "TLS (SSL)",
            RdpSecurityMode::Rdp => "Standard RDP security (legacy)",
        }
    }

    pub const ALL: [RdpSecurityMode; 4] = [
        RdpSecurityMode::Negotiate,
        RdpSecurityMode::Nla,
        RdpSecurityMode::Tls,
        RdpSecurityMode::Rdp,
    ];
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ssh,
    Sftp,
    Rdp,
    Vnc,
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ssh => "SSH",
            Protocol::Sftp => "SFTP",
            Protocol::Rdp => "RDP",
            Protocol::Vnc => "VNC",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ssh | Protocol::Sftp => 22,
            Protocol::Rdp => 3389,
            Protocol::Vnc => 5900,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthMethod {
    Password { password: String },
    PublicKey { path: String, passphrase: Option<String> },
    Agent,
}

impl Default for AuthMethod {
    fn default() -> Self {
        AuthMethod::Agent
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelKind {
    Local,
    Remote,
    Dynamic,
}

impl TunnelKind {
    pub fn label(self) -> &'static str {
        match self {
            TunnelKind::Local => "Local (-L)",
            TunnelKind::Remote => "Remote (-R)",
            TunnelKind::Dynamic => "Dynamic (-D, SOCKS5)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tunnel {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub kind: TunnelKind,
    #[serde(default = "default_listen_address")]
    pub listen_address: String,
    pub listen_port: u16,
    #[serde(default)]
    pub remote_host: String,
    #[serde(default)]
    pub remote_port: u16,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_listen_address() -> String {
    "127.0.0.1".to_string()
}

fn default_enabled() -> bool {
    true
}

impl Tunnel {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ConnectionRaw")]
pub struct Connection {
    pub id: Uuid,
    pub name: String,
    pub group: Option<String>,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(default)]
    pub auth: AuthMethod,
    #[serde(default)]
    pub tunnels: Vec<Tunnel>,
    #[serde(default)]
    pub jump_chain: Vec<Uuid>,
    #[serde(default)]
    pub record_sessions: bool,
    #[serde(default)]
    pub x11_forwarding: bool,
    #[serde(default)]
    pub keepalive_secs: u32,
    #[serde(default)]
    pub remote_commands: Vec<String>,
    #[serde(default)]
    pub before_script: Option<String>,
    #[serde(default)]
    pub after_connect_script: Option<String>,
    #[serde(default)]
    pub after_close_script: Option<String>,
    #[serde(default)]
    pub rdp_backend: RdpBackend,
    #[serde(default)]
    pub freerdp_resize_mode: FreeRdpResizeMode,
    #[serde(default)]
    pub rdp_security_mode: RdpSecurityMode,
    #[serde(default = "default_rdp_width")]
    pub rdp_width: u16,
    #[serde(default = "default_rdp_height")]
    pub rdp_height: u16,
}

fn default_rdp_width() -> u16 { 1920 }
fn default_rdp_height() -> u16 { 1080 }#[derive(Deserialize)]
struct ConnectionRaw {
    id: Uuid,
    name: String,
    #[serde(default)]
    group: Option<String>,
    protocol: Protocol,
    host: String,
    port: u16,
    username: String,
    #[serde(default)]
    auth: AuthMethod,
    #[serde(default)]
    tunnels: Vec<Tunnel>,
    #[serde(default)]
    jump_chain: Vec<Uuid>,
    #[serde(default)]
    jump_via: Option<Uuid>,
    #[serde(default)]
    record_sessions: bool,
    #[serde(default)]
    x11_forwarding: bool,
    #[serde(default)]
    keepalive_secs: u32,
    #[serde(default)]
    remote_commands: Vec<String>,
    #[serde(default)]
    before_script: Option<String>,
    #[serde(default)]
    after_connect_script: Option<String>,
    #[serde(default)]
    after_close_script: Option<String>,
    #[serde(default)]
    rdp_backend: RdpBackend,
    #[serde(default)]
    freerdp_resize_mode: FreeRdpResizeMode,
    #[serde(default)]
    rdp_security_mode: RdpSecurityMode,
    #[serde(default = "default_rdp_width")]
    rdp_width: u16,
    #[serde(default = "default_rdp_height")]
    rdp_height: u16,
}

impl From<ConnectionRaw> for Connection {
    fn from(raw: ConnectionRaw) -> Self {
        let mut jump_chain = raw.jump_chain;
        if jump_chain.is_empty() {
            if let Some(legacy) = raw.jump_via {
                jump_chain.push(legacy);
            }
        }
        Self {
            id: raw.id,
            name: raw.name,
            group: raw.group,
            protocol: raw.protocol,
            host: raw.host,
            port: raw.port,
            username: raw.username,
            auth: raw.auth,
            tunnels: raw.tunnels,
            jump_chain,
            record_sessions: raw.record_sessions,
            x11_forwarding: raw.x11_forwarding,
            keepalive_secs: raw.keepalive_secs,
            remote_commands: raw.remote_commands,
            before_script: raw.before_script,
            after_connect_script: raw.after_connect_script,
            after_close_script: raw.after_close_script,
            rdp_backend: raw.rdp_backend,
            freerdp_resize_mode: raw.freerdp_resize_mode,
            rdp_security_mode: raw.rdp_security_mode,
            rdp_width: raw.rdp_width,
            rdp_height: raw.rdp_height,
        }
    }
}

impl Connection {
    pub fn new_ssh(name: impl Into<String>, host: impl Into<String>, username: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            group: None,
            protocol: Protocol::Ssh,
            host: host.into(),
            port: 22,
            username: username.into(),
            auth: AuthMethod::default(),
            tunnels: Vec::new(),
            jump_chain: Vec::new(),
            record_sessions: false,
            x11_forwarding: false,
            keepalive_secs: 0,
            remote_commands: Vec::new(),
            before_script: None,
            after_connect_script: None,
            after_close_script: None,
            rdp_backend: RdpBackend::default(),
            freerdp_resize_mode: FreeRdpResizeMode::default(),
            rdp_security_mode: RdpSecurityMode::default(),
            rdp_width: default_rdp_width(),
            rdp_height: default_rdp_height(),
        }
    }

    pub fn display_address(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ConnectionStore {
    #[serde(default)]
    pub connections: Vec<Connection>,
}

impl ConnectionStore {
    pub fn add(&mut self, conn: Connection) {
        self.connections.push(conn);
    }

    pub fn find(&self, id: Uuid) -> Option<&Connection> {
        self.connections.iter().find(|c| c.id == id)
    }

    pub fn find_mut(&mut self, id: Uuid) -> Option<&mut Connection> {
        self.connections.iter_mut().find(|c| c.id == id)
    }

    pub fn remove(&mut self, id: Uuid) -> Option<Connection> {
        let idx = self.connections.iter().position(|c| c.id == id)?;
        Some(self.connections.remove(idx))
    }

    pub fn resolve_jump_chain(&self, target: Uuid) -> Result<Vec<Connection>, JumpChainError> {
        let target_conn = self
            .find(target)
            .ok_or(JumpChainError::Missing(target))?
            .clone();

        if target_conn.jump_chain.len() > 8 {
            return Err(JumpChainError::TooDeep);
        }

        let mut seen = std::collections::HashSet::new();
        seen.insert(target);

        let mut chain = Vec::with_capacity(target_conn.jump_chain.len() + 1);
        for hop_id in &target_conn.jump_chain {
            if !seen.insert(*hop_id) {
                return Err(JumpChainError::Cycle);
            }
            let hop = self
                .find(*hop_id)
                .ok_or(JumpChainError::Missing(*hop_id))?
                .clone();
            chain.push(hop);
        }
        chain.push(target_conn);
        Ok(chain)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JumpChainError {
    #[error("jump host {0} not found in store")]
    Missing(Uuid),
    #[error("jump host chain forms a cycle")]
    Cycle,
    #[error("jump host chain too deep (max 8)")]
    TooDeep,
}
