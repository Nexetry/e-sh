use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
}

#[derive(Deserialize)]
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
