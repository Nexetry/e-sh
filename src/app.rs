use age::secrecy::SecretString;
use eframe::{App, CreationContext, Frame};
use egui::{CentralPanel, Key, KeyboardShortcut, Modifiers, Panel, Ui};
use egui_dock::{DockArea, DockState, Style};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

use e_sh::config::host_keys::{HostKeyPrompt, HostKeyStore};
use e_sh::config::secrets::SecretStore;
use e_sh::config::store::{
    ConfigPaths, forget_secrets, hydrate_after_unlock, load_connections, save_connections,
};
use e_sh::core::connection::{AuthMethod, Connection, ConnectionStore, Protocol};
use e_sh::proto::rdp::spawn_rdp_session;
use e_sh::proto::sftp::spawn_sftp_session;
use e_sh::proto::ssh::{HostKeyContext, spawn_session};
use e_sh::proto::vnc::spawn_vnc_session;
use e_sh::recording::{self, Kind as RecordingKind, StartParams};
use e_sh::ui::command_palette::{Command, CommandItem, CommandPalette, PaletteResult};
use e_sh::ui::connection_tree::{ConnectionTree, ReorderRequest};
use e_sh::ui::dock::{EshTab, EshTabViewer, TabAction, TerminalTab};
use e_sh::ui::edit_dialog::EditConnectionDialog;
use e_sh::ui::edit_dialog::RevealRequest;
use e_sh::ui::host_key_prompt::{HostKeyPromptResult, HostKeyPromptUi};
use e_sh::ui::master_password_prompt::{
    MasterPasswordMode, MasterPasswordPromptUi, MasterPasswordResult,
};
use e_sh::ui::recordings_view::RecordingsTab;
use e_sh::ui::rdp_tab::RdpTab;
use e_sh::ui::sftp_tab::SftpTab;
use e_sh::ui::status_bar::StatusBar;
use e_sh::ui::terminal_widget::TerminalEmulator;
use e_sh::ui::toast::Toaster;
use e_sh::ui::vnc_tab::VncTab;

enum PendingAction {
    Open(Uuid),
    OpenSftp(Uuid),
    SaveAndClose,
    Forget(Connection),
}

pub struct EshApp {
    rt: Handle,
    paths: Arc<ConfigPaths>,
    store: ConnectionStore,
    secrets: Option<SecretStore>,
    master_prompt: Option<MasterPasswordPromptUi>,
    pending: VecDeque<PendingAction>,
    host_keys: Arc<Mutex<HostKeyStore>>,
    host_key_prompt_tx: UnboundedSender<HostKeyPrompt>,
    host_key_prompt_rx: UnboundedReceiver<HostKeyPrompt>,
    pending_host_key_prompts: VecDeque<HostKeyPrompt>,
    dock: DockState<EshTab>,
    viewer: EshTabViewer,
    status: String,
    editor: Option<EditConnectionDialog>,
    toaster: Toaster,
    palette: CommandPalette,
    sidebar_visible: bool,
    quit_requested: bool,
    /// Timestamp of last successful master password verification for reveal.
    last_master_verify: Option<std::time::Instant>,
    /// Re-auth prompt shown when user clicks reveal.
    reauth_prompt: Option<MasterPasswordPromptUi>,
    /// Which field triggered the re-auth.
    reauth_target: RevealRequest,
    /// Pending delete confirmation for a connection.
    pending_delete_confirm: Option<Uuid>,
}

impl EshApp {
    pub fn new(_cc: &CreationContext<'_>, rt: Handle) -> Self {
        let paths = Arc::new(ConfigPaths::discover().expect("config paths"));
        let mut store = load_connections(&paths).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed loading connections, starting empty");
            ConnectionStore::default()
        });

        if store.connections.is_empty() {
            store.add(Connection::new_ssh("Localhost", "127.0.0.1", whoami_user()));
        }

        let host_keys = HostKeyStore::load(&paths).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed loading host keys, starting empty");
            HostKeyStore::default()
        });

        let (host_key_prompt_tx, host_key_prompt_rx) = unbounded_channel::<HostKeyPrompt>();

        let master_prompt = if SecretStore::file_exists(&paths.config_dir) {
            Some(MasterPasswordPromptUi::new(MasterPasswordMode::Unlock))
        } else if store.connections.iter().any(connection_needs_secret) {
            Some(MasterPasswordPromptUi::new(MasterPasswordMode::Create))
        } else {
            None
        };

        Self {
            rt,
            paths,
            store,
            secrets: None,
            master_prompt,
            pending: VecDeque::new(),
            host_keys: Arc::new(Mutex::new(host_keys)),
            host_key_prompt_tx,
            host_key_prompt_rx,
            pending_host_key_prompts: VecDeque::new(),
            dock: DockState::new(Vec::new()),
            viewer: EshTabViewer::default(),
            status: "Ready".to_string(),
            editor: None,
            toaster: Toaster::default(),
            palette: CommandPalette::default(),
            sidebar_visible: true,
            quit_requested: false,
            last_master_verify: None,
            reauth_prompt: None,
            reauth_target: RevealRequest::None,
            pending_delete_confirm: None,
        }
    }

    fn ensure_secrets_unlocked(&mut self, mode: MasterPasswordMode) -> bool {
        if self.secrets.is_some() {
            return true;
        }
        if self.master_prompt.is_none() {
            self.master_prompt = Some(MasterPasswordPromptUi::new(mode));
        }
        false
    }

    fn try_unlock_or_create(&mut self, password: String) -> Result<(), String> {
        let secret = SecretString::from(password);
        let mode = self
            .master_prompt
            .as_ref()
            .map(|p| p.mode)
            .unwrap_or(MasterPasswordMode::Create);
        let store = match mode {
            MasterPasswordMode::Unlock => SecretStore::open(&self.paths.config_dir, secret)
                .map_err(|e| e.to_string())?,
            MasterPasswordMode::Create => SecretStore::create(&self.paths.config_dir, secret)
                .map_err(|e| e.to_string())?,
        };
        self.secrets = Some(store);
        if let Some(secrets) = self.secrets.as_mut()
            && let Err(err) = hydrate_after_unlock(&self.paths, &mut self.store, secrets)
        {
            tracing::warn!(?err, "secret hydration after unlock failed");
        }
        self.master_prompt = None;
        self.toaster.success(
            match mode {
                MasterPasswordMode::Unlock => "Secrets unlocked",
                MasterPasswordMode::Create => "Master password set",
            },
            "",
        );
        self.run_pending();
        Ok(())
    }

    fn run_pending(&mut self) {
        while let Some(action) = self.pending.pop_front() {
            match action {
                PendingAction::Open(id) => self.open_connection(id),
                PendingAction::OpenSftp(id) => self.open_sftp_tab(id),
                PendingAction::SaveAndClose => self.persist(),
                PendingAction::Forget(conn) => {
                    if let Some(secrets) = self.secrets.as_mut() {
                        forget_secrets(&conn, secrets);
                    }
                }
            }
        }
    }

    fn open_connection(&mut self, id: Uuid) {
        let Some(conn) = self.store.find(id).cloned() else {
            return;
        };
        if connection_needs_secret(&conn) && self.secrets.is_none() {
            self.pending.push_back(PendingAction::Open(id));
            self.ensure_secrets_unlocked(if SecretStore::file_exists(&self.paths.config_dir) {
                MasterPasswordMode::Unlock
            } else {
                MasterPasswordMode::Create
            });
            return;
        }
        if matches!(conn.protocol, Protocol::Sftp) {
            self.open_sftp_tab(id);
            return;
        }
        if matches!(conn.protocol, Protocol::Rdp) {
            self.launch_rdp(id);
            return;
        }
        if matches!(conn.protocol, Protocol::Vnc) {
            self.launch_vnc(id);
            return;
        }
        let chain = match self.store.resolve_jump_chain(id) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Jump host error: {e}");
                self.toaster.error("Jump host error", e.to_string());
                return;
            }
        };
        let host_ctx = HostKeyContext {
            store: self.host_keys.clone(),
            paths: self.paths.clone(),
            prompts: self.host_key_prompt_tx.clone(),
        };
        let recorder = if conn.record_sessions {
            match recording::start_recording(StartParams {
                conn: &conn,
                recordings_dir: &self.paths.recordings_dir,
                kind: RecordingKind::Ssh,
                width: 80,
                height: 24,
                term: "xterm-256color",
            }) {
                Ok(rec) => {
                    self.toaster.warn(
                        "Recording started",
                        "Server output is being saved to disk in plaintext.",
                    );
                    Some(rec)
                }
                Err(e) => {
                    tracing::warn!(?e, "failed to start SSH recording; continuing without");
                    self.toaster.error("Recording failed to start", format!("{e:#}"));
                    None
                }
            }
        } else {
            None
        };
        let handle = spawn_session(&self.rt, chain, host_ctx, recorder);
        let emulator = TerminalEmulator::new(handle, 80, 24);
        let title = format!("{} ({})", conn.name, conn.protocol.label());
        let connection_label = format!("{}@{}:{}", conn.username, conn.host, conn.port);
        let tab = EshTab::Terminal(TerminalTab {
            id: Uuid::new_v4(),
            source_connection: Some(id),
            title,
            connection_label,
            emulator,
            closed_reported: false,
            tab_color: None,
        });
        self.push_tab(tab);
        self.status = format!("Opened {}@{}", conn.username, conn.host);
        self.toaster.info(
            "Connecting",
            format!("{}@{}:{}", conn.username, conn.host, conn.port),
        );
    }

    fn open_sftp_tab(&mut self, id: Uuid) {
        let Some(conn) = self.store.find(id).cloned() else {
            return;
        };
        if connection_needs_secret(&conn) && self.secrets.is_none() {
            self.pending.push_back(PendingAction::OpenSftp(id));
            self.ensure_secrets_unlocked(if SecretStore::file_exists(&self.paths.config_dir) {
                MasterPasswordMode::Unlock
            } else {
                MasterPasswordMode::Create
            });
            return;
        }
        let chain = match self.store.resolve_jump_chain(id) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Jump host error: {e}");
                self.toaster.error("Jump host error", e.to_string());
                return;
            }
        };
        let host_ctx = HostKeyContext {
            store: self.host_keys.clone(),
            paths: self.paths.clone(),
            prompts: self.host_key_prompt_tx.clone(),
        };
        let recorder = if conn.record_sessions {
            match recording::start_recording(StartParams {
                conn: &conn,
                recordings_dir: &self.paths.recordings_dir,
                kind: RecordingKind::Sftp,
                width: 0,
                height: 0,
                term: "",
            }) {
                Ok(rec) => {
                    self.toaster.warn(
                        "Recording started",
                        "SFTP operations are being audit-logged to disk in plaintext.",
                    );
                    Some(rec)
                }
                Err(e) => {
                    tracing::warn!(?e, "failed to start SFTP recording; continuing without");
                    self.toaster.error("Recording failed to start", format!("{e:#}"));
                    None
                }
            }
        } else {
            None
        };
        let handle = spawn_sftp_session(&self.rt, chain, host_ctx, recorder);
        let title = format!("{} (SFTP)", conn.name);
        let connection_label = format!("{}@{}:{}", conn.username, conn.host, conn.port);
        let tab = EshTab::Sftp(SftpTab::new(
            Uuid::new_v4(),
            Some(id),
            title,
            connection_label.clone(),
            handle,
        ));
        self.push_tab(tab);
        self.status = format!("Opened SFTP {}", connection_label);
        self.toaster.info("SFTP", connection_label);
    }

    fn launch_rdp(&mut self, id: Uuid) {
        let Some(conn) = self.store.find(id).cloned() else {
            return;
        };
        if connection_needs_secret(&conn) && self.secrets.is_none() {
            self.pending.push_back(PendingAction::Open(id));
            self.ensure_secrets_unlocked(if SecretStore::file_exists(&self.paths.config_dir) {
                MasterPasswordMode::Unlock
            } else {
                MasterPasswordMode::Create
            });
            return;
        }
        let label = format!("{}@{}:{}", conn.username, conn.host, conn.port);
        let title = format!("{} (RDP)", conn.name);
        let handle = spawn_rdp_session(&self.rt, conn);
        let tab = EshTab::Rdp(RdpTab::new(
            Uuid::new_v4(),
            Some(id),
            title,
            label.clone(),
            handle,
        ));
        self.push_tab(tab);
        self.status = format!("Opened RDP {label}");
        self.toaster.info("RDP", format!("Connecting to {label}"));
    }

    fn launch_vnc(&mut self, id: Uuid) {
        let Some(conn) = self.store.find(id).cloned() else {
            return;
        };
        if connection_needs_secret(&conn) && self.secrets.is_none() {
            self.pending.push_back(PendingAction::Open(id));
            self.ensure_secrets_unlocked(if SecretStore::file_exists(&self.paths.config_dir) {
                MasterPasswordMode::Unlock
            } else {
                MasterPasswordMode::Create
            });
            return;
        }
        let label = format!("{}@{}:{}", conn.username, conn.host, conn.port);
        let title = format!("{} (VNC)", conn.name);
        let handle = spawn_vnc_session(&self.rt, conn);
        let tab = EshTab::Vnc(VncTab::new(
            Uuid::new_v4(),
            Some(id),
            title,
            label.clone(),
            handle,
        ));
        self.push_tab(tab);
        self.status = format!("Opened VNC {label}");
        self.toaster.info("VNC", format!("Connecting to {label}"));
    }

    fn push_tab(&mut self, tab: EshTab) {
        if self.dock.main_surface_mut().is_empty() {
            self.dock = DockState::new(vec![tab]);
        } else {
            self.dock.push_to_focused_leaf(tab);
        }
    }

    fn start_new_connection(&mut self) {
        let new_conn = Connection::new_ssh(
            format!("Connection {}", self.store.connections.len() + 1),
            "127.0.0.1",
            whoami_user(),
        );
        self.store.add(new_conn.clone());
        self.persist();
        self.editor = Some(EditConnectionDialog::from_connection(new_conn));
    }

    fn drain_host_key_prompts(&mut self) {
        while let Ok(p) = self.host_key_prompt_rx.try_recv() {
            self.pending_host_key_prompts.push_back(p);
        }
    }

    fn persist(&mut self) {
        let needs_secret = self.store.connections.iter().any(connection_needs_secret);
        if needs_secret && self.secrets.is_none() {
            self.pending.push_back(PendingAction::SaveAndClose);
            self.ensure_secrets_unlocked(if SecretStore::file_exists(&self.paths.config_dir) {
                MasterPasswordMode::Unlock
            } else {
                MasterPasswordMode::Create
            });
            return;
        }
        if let Some(secrets) = self.secrets.as_mut() {
            if let Err(e) = save_connections(&self.paths, &self.store, secrets) {
                self.status = format!("Save failed: {e}");
                self.toaster.error("Save failed", e.to_string());
            }
        } else {
            if let Err(e) = save_connections_no_secrets(&self.paths, &self.store) {
                self.status = format!("Save failed: {e}");
                self.toaster.error("Save failed", e.to_string());
            }
        }
    }

    fn build_palette_items(&self) -> Vec<CommandItem> {
        let mut items: Vec<CommandItem> = Vec::new();

        items.push(CommandItem {
            command: Command::NewConnection,
            label: "New connection".to_string(),
            detail: "Create a new SSH/SFTP entry".to_string(),
            hint: "+".to_string(),
        });
        items.push(CommandItem {
            command: Command::ToggleSidebar,
            label: if self.sidebar_visible {
                "Hide sidebar".to_string()
            } else {
                "Show sidebar".to_string()
            },
            detail: "Toggle the connection tree".to_string(),
            hint: "\u{2318}B".to_string(),
        });
        items.push(CommandItem {
            command: Command::CloseActiveTab,
            label: "Close active tab".to_string(),
            detail: "Close the currently focused session".to_string(),
            hint: "\u{2318}W".to_string(),
        });
        items.push(CommandItem {
            command: Command::LockSecrets,
            label: "Lock secrets".to_string(),
            detail: "Clear unlocked master-password session".to_string(),
            hint: "".to_string(),
        });
        items.push(CommandItem {
            command: Command::OpenRecordings,
            label: "Open recordings".to_string(),
            detail: "Browse captured SSH / SFTP sessions".to_string(),
            hint: "".to_string(),
        });
        if self.has_active_terminal() {
            items.push(CommandItem {
                command: Command::FindInTerminal,
                label: "Find in terminal".to_string(),
                detail: "Search scrollback of the active terminal".to_string(),
                hint: "\u{2318}F".to_string(),
            });
        }
        items.push(CommandItem {
            command: Command::Quit,
            label: "Quit e-sh".to_string(),
            detail: "Close the application".to_string(),
            hint: "\u{2318}Q".to_string(),
        });

        for (_, tab) in self.dock.iter_all_tabs() {
            items.push(CommandItem {
                command: Command::SwitchTab { id: tab.id() },
                label: format!("Switch to: {}", tab.title()),
                detail: "Open tab".to_string(),
                hint: "tab".to_string(),
            });
        }

        for conn in &self.store.connections {
            let addr = format!("{}@{}:{}", conn.username, conn.host, conn.port);
            let group = conn
                .group
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or("Ungrouped")
                .to_string();

            match conn.protocol {
                Protocol::Sftp => {
                    items.push(CommandItem {
                        command: Command::OpenSftp { id: conn.id },
                        label: format!("SFTP: {}", conn.name),
                        detail: format!("{addr}  -  {group}"),
                        hint: "SFTP".to_string(),
                    });
                }
                Protocol::Rdp => {
                    items.push(CommandItem {
                        command: Command::OpenConnection { id: conn.id },
                        label: format!("RDP: {}", conn.name),
                        detail: format!("{addr}  -  {group}"),
                        hint: "RDP".to_string(),
                    });
                }
                Protocol::Vnc => {
                    items.push(CommandItem {
                        command: Command::OpenConnection { id: conn.id },
                        label: format!("VNC: {}", conn.name),
                        detail: format!("{addr}  -  {group}"),
                        hint: "VNC".to_string(),
                    });
                }
                _ => {
                    items.push(CommandItem {
                        command: Command::OpenConnection { id: conn.id },
                        label: format!("Open: {}", conn.name),
                        detail: format!("{addr}  -  {group}"),
                        hint: conn.protocol.label().to_string(),
                    });
                    items.push(CommandItem {
                        command: Command::OpenSftp { id: conn.id },
                        label: format!("SFTP: {}", conn.name),
                        detail: format!("{addr}  -  {group}"),
                        hint: "SFTP".to_string(),
                    });
                }
            }
            items.push(CommandItem {
                command: Command::EditConnection { id: conn.id },
                label: format!("Edit: {}", conn.name),
                detail: format!("{addr}  -  {group}"),
                hint: "edit".to_string(),
            });
        }

        items
    }

    fn dispatch_command(&mut self, cmd: Command) {
        match cmd {
            Command::NewConnection => self.start_new_connection(),
            Command::OpenConnection { id } => self.open_connection(id),
            Command::OpenSftp { id } => self.open_sftp_tab(id),
            Command::EditConnection { id } => {
                if let Some(conn) = self.store.find(id).cloned() {
                    self.editor = Some(EditConnectionDialog::from_connection(conn));
                }
            }
            Command::SwitchTab { id } => self.focus_tab_by_id(id),
            Command::CloseActiveTab => self.close_active_tab(),
            Command::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                self.status = if self.sidebar_visible {
                    "Sidebar shown".into()
                } else {
                    "Sidebar hidden".into()
                };
            }
            Command::LockSecrets => {
                self.secrets = None;
                self.status = "Secrets locked".into();
                self.toaster
                    .info("Locked", "Master-password session cleared");
            }
            Command::FindInTerminal => {
                if let Some(tab) = self.active_terminal_tab_mut() {
                    tab.emulator.open_find();
                }
            }
            Command::OpenRecordings => self.open_recordings_tab(),
            Command::Quit => {
                self.quit_requested = true;
            }
        }
    }

    fn focus_tab_by_id(&mut self, id: Uuid) {
        let target: Option<egui_dock::TabPath> = self
            .dock
            .iter_all_tabs()
            .find_map(|(path, tab)| if tab.id() == id { Some(path) } else { None });
        if let Some(path) = target {
            let _ = self.dock.set_active_tab(path);
            self.dock.set_focused_node_and_surface(path.node_path());
        }
    }

    fn open_recordings_tab(&mut self) {
        let existing: Option<Uuid> = self.dock.iter_all_tabs().find_map(|(_, tab)| match tab {
            EshTab::Recordings(t) => Some(t.id),
            _ => None,
        });
        if let Some(id) = existing {
            self.focus_tab_by_id(id);
            for (_, tab) in self.dock.iter_all_tabs_mut() {
                if let EshTab::Recordings(t) = tab {
                    t.reload();
                }
            }
            return;
        }
        let tab = RecordingsTab::new(self.paths.recordings_dir.clone());
        self.dock.push_to_focused_leaf(EshTab::Recordings(tab));
        self.status = "Opened recordings".to_string();
    }

    fn close_active_tab(&mut self) {
        let Some(node_path) = self.dock.focused_leaf() else {
            return;
        };
        let Ok(leaf) = self.dock.leaf(node_path) else {
            return;
        };
        if leaf.tabs.is_empty() {
            return;
        }
        let active_idx = leaf.active.0.min(leaf.tabs.len() - 1);
        let tab_path = egui_dock::TabPath::new(
            node_path.surface,
            node_path.node,
            egui_dock::TabIndex(active_idx),
        );
        let _ = self.dock.remove_tab(tab_path);
    }

    fn handle_tab_action(&mut self, action: TabAction) {
        match action {
            TabAction::Duplicate { source_connection, is_sftp } => {
                if is_sftp {
                    self.open_sftp_tab(source_connection);
                } else {
                    self.open_connection(source_connection);
                }
            }
            TabAction::Reconnect { tab_id, source_connection, is_sftp } => {
                let target: Option<egui_dock::TabPath> = self
                    .dock
                    .iter_all_tabs()
                    .find_map(|(path, tab)| if tab.id() == tab_id { Some(path) } else { None });
                if let Some(path) = target {
                    let _ = self.dock.remove_tab(path);
                }
                if is_sftp {
                    self.open_sftp_tab(source_connection);
                } else {
                    self.open_connection(source_connection);
                }
            }
        }
    }

    fn active_terminal_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        let node_path = self.dock.focused_leaf()?;
        let leaf = self.dock.leaf_mut(node_path).ok()?;
        if leaf.tabs.is_empty() {
            return None;
        }
        let active_idx = leaf.active.0.min(leaf.tabs.len() - 1);
        match leaf.tabs.get_mut(active_idx)? {
            EshTab::Terminal(tab) => Some(tab),
            _ => None,
        }
    }

    fn has_active_terminal(&self) -> bool {
        let Some(node_path) = self.dock.focused_leaf() else {
            return false;
        };
        let Ok(leaf) = self.dock.leaf(node_path) else {
            return false;
        };
        if leaf.tabs.is_empty() {
            return false;
        }
        let active_idx = leaf.active.0.min(leaf.tabs.len() - 1);
        matches!(leaf.tabs.get(active_idx), Some(EshTab::Terminal(_)))
    }

    fn poll_session_errors(&mut self) {
        for (_, tab) in self.dock.iter_all_tabs_mut() {
            match tab {
                EshTab::Terminal(tab) => {
                    if tab.closed_reported {
                        continue;
                    }
                    if let Some(reason) = tab.emulator.closed.clone() {
                        tab.closed_reported = true;
                        let label = tab.connection_label.clone();
                        let lower = reason.to_lowercase();
                        if lower == "session closed"
                            || lower.is_empty()
                            || lower == "client closing"
                        {
                            self.toaster.info("Disconnected", label);
                        } else {
                            self.toaster.error(format!("{label} failed"), reason);
                        }
                    }
                }
                EshTab::Sftp(tab) => {
                    if tab.closed_reported {
                        continue;
                    }
                    if let Some(reason) = tab.closed.clone() {
                        tab.closed_reported = true;
                        let label = tab.connection_label.clone();
                        let lower = reason.to_lowercase();
                        if lower == "session closed"
                            || lower.is_empty()
                            || lower == "client closing"
                        {
                            self.toaster.info("SFTP disconnected", label);
                        } else {
                            self.toaster.error(format!("{label} SFTP failed"), reason);
                        }
                    }
                }
                EshTab::Rdp(tab) => {
                    if tab.closed_reported {
                        continue;
                    }
                    if let Some(reason) = tab.closed.clone() {
                        tab.closed_reported = true;
                        let label = tab.connection_label.clone();
                        let lower = reason.to_lowercase();
                        if lower == "session closed"
                            || lower.is_empty()
                            || lower == "client closing"
                        {
                            self.toaster.info("RDP disconnected", label);
                        } else {
                            self.toaster.error(format!("{label} RDP failed"), reason);
                        }
                    }
                }
                EshTab::Vnc(tab) => {
                    if tab.closed_reported {
                        continue;
                    }
                    if let Some(reason) = tab.closed.clone() {
                        tab.closed_reported = true;
                        let label = tab.connection_label.clone();
                        let lower = reason.to_lowercase();
                        if lower == "session closed"
                            || lower.is_empty()
                            || lower == "client closing"
                        {
                            self.toaster.info("VNC disconnected", label);
                        } else {
                            self.toaster.error(format!("{label} VNC failed"), reason);
                        }
                    }
                }
                EshTab::Recordings(_) => {}
            }
        }
    }
}

impl App for EshApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut Frame) {
        let ctx = ui.ctx().clone();

        let palette_primary = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
        let palette_alt = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
        let toggle_sidebar = KeyboardShortcut::new(Modifiers::COMMAND, Key::B);
        let close_tab = KeyboardShortcut::new(Modifiers::COMMAND, Key::W);
        let quit_shortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Q);

        let (open_palette, toggle_sb, close_t, quit_now) = ctx.input_mut(|i| {
            (
                i.consume_shortcut(&palette_primary) || i.consume_shortcut(&palette_alt),
                i.consume_shortcut(&toggle_sidebar),
                i.consume_shortcut(&close_tab),
                i.consume_shortcut(&quit_shortcut),
            )
        });
        if open_palette {
            self.palette.toggle();
        }
        if toggle_sb {
            self.dispatch_command(Command::ToggleSidebar);
        }
        if close_t {
            self.dispatch_command(Command::CloseActiveTab);
        }
        if quit_now {
            self.dispatch_command(Command::Quit);
        }

        Panel::bottom("status").show_inside(ui, |ui| {
            StatusBar { message: &self.status }.show(ui);
        });

        if self.sidebar_visible {
            Panel::left("connections")
                .resizable(true)
                .default_size(240.0)
                .show_inside(ui, |ui| {
                    let action = ConnectionTree { store: &self.store }.show(ui);
                    if action.new_connection {
                        self.start_new_connection();
                    }
                    if action.open_recordings {
                        self.open_recordings_tab();
                    }
                    if let Some(id) = action.open {
                        self.open_connection(id);
                    }
                    if let Some(id) = action.open_sftp {
                        self.open_sftp_tab(id);
                    }
                    if let Some(id) = action.edit {
                        if let Some(conn) = self.store.find(id).cloned() {
                            self.editor = Some(EditConnectionDialog::from_connection(conn));
                        }
                    }
                    if let Some(id) = action.duplicate {
                        if let Some(src) = self.store.find(id).cloned() {
                            let mut dup = src.clone();
                            dup.id = Uuid::new_v4();
                            dup.name = format!("{} (copy)", src.name);
                            let dup_name = dup.name.clone();
                            self.store.add(dup);
                            self.persist();
                            self.status = "Duplicated connection".to_string();
                            self.toaster.success("Duplicated", dup_name);
                        }
                    }
                    if let Some(id) = action.delete {
                        self.pending_delete_confirm = Some(id);
                    }
                    if let Some(req) = action.reorder {
                        if apply_reorder(&mut self.store, &req) {
                            self.persist();
                            self.status = "Reordered connections".to_string();
                        }
                    }
                });
        }

        self.poll_session_errors();

        // Connection delete confirmation dialog
        if let Some(id) = self.pending_delete_confirm {
            let conn_name = self
                .store
                .find(id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| "this connection".to_string());
            let mut confirmed = false;
            let mut dismissed = false;
            egui::Window::new("Confirm Delete")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(&ctx, |ui| {
                    ui.label(format!(
                        "Are you sure you want to delete \"{}\"?",
                        conn_name
                    ));
                    ui.label(
                        egui::RichText::new("This action cannot be undone.")
                            .small()
                            .weak(),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            dismissed = true;
                        }
                    });
                });
            if confirmed {
                if let Some(removed) = self.store.remove(id) {
                    if let Some(secrets) = self.secrets.as_mut() {
                        forget_secrets(&removed, secrets);
                    } else {
                        self.pending
                            .push_back(PendingAction::Forget(removed.clone()));
                    }
                    self.persist();
                    self.status = "Deleted connection".to_string();
                    self.toaster.warn("Deleted", removed.name);
                }
                self.pending_delete_confirm = None;
            } else if dismissed {
                self.pending_delete_confirm = None;
            }
        }

        if let Some(prompt) = self.master_prompt.as_mut() {
            match prompt.show(&ctx) {
                MasterPasswordResult::Pending => {}
                MasterPasswordResult::Submit(pw) => {
                    if let Err(err) = self.try_unlock_or_create(pw) {
                        if let Some(p) = self.master_prompt.as_mut() {
                            p.error = Some(err);
                            p.password.clear();
                            p.confirm.clear();
                        }
                    }
                }
            }
            ctx.request_repaint();
            return;
        }

        self.drain_host_key_prompts();
        if let Some(prompt) = self.pending_host_key_prompts.front() {
            match HostKeyPromptUi::show(&ctx, prompt) {
                HostKeyPromptResult::Pending => {
                    ctx.request_repaint();
                }
                HostKeyPromptResult::Decided(decision) => {
                    if let Some(p) = self.pending_host_key_prompts.pop_front() {
                        let host_id = format!("{}:{}", p.host, p.port);
                        let _ = p.responder.send(decision);
                        self.status = format!("Host key decision for {host_id}: {decision:?}");
                        match decision {
                            e_sh::config::host_keys::HostKeyDecision::Reject => {
                                self.toaster.warn("Host key rejected", host_id);
                            }
                            e_sh::config::host_keys::HostKeyDecision::AcceptOnce => {
                                self.toaster.info("Host key accepted (once)", host_id);
                            }
                            e_sh::config::host_keys::HostKeyDecision::AcceptAndSave => {
                                self.toaster.success("Host key saved", host_id);
                            }
                        }
                    }
                }
            }
        }

        if let Some(editor) = self.editor.as_mut() {
            let result = editor.show(&ctx, &self.store);
            if result.saved {
                let draft = editor.draft.clone();
                let id = draft.id;
                let label = draft.name.clone();
                if let Some(slot) = self.store.find_mut(id) {
                    *slot = draft;
                } else {
                    self.store.add(draft);
                }
                self.persist();
                self.status = "Connection saved".to_string();
                self.toaster.success("Saved", label);
                self.editor = None;
            } else if result.cancelled {
                self.editor = None;
            }
            if result.reveal_requested != RevealRequest::None {
                let within_cache = self
                    .last_master_verify
                    .map(|t| t.elapsed() < std::time::Duration::from_secs(600))
                    .unwrap_or(false);
                if within_cache {
                    if let Some(ed) = self.editor.as_mut() {
                        match result.reveal_requested {
                            RevealRequest::Password => ed.reveal_password = true,
                            RevealRequest::Passphrase => ed.reveal_passphrase = true,
                            RevealRequest::None => {}
                        }
                    }
                } else {
                    self.reauth_target = result.reveal_requested;
                    self.reauth_prompt =
                        Some(MasterPasswordPromptUi::new(MasterPasswordMode::Unlock));
                }
            }
        }

        // Re-auth prompt for reveal password
        if let Some(prompt) = self.reauth_prompt.as_mut() {
            match prompt.show(&ctx) {
                MasterPasswordResult::Pending => {}
                MasterPasswordResult::Submit(pw) => {
                    if let Some(secrets) = &self.secrets {
                        if secrets.verify_password(&pw) {
                            self.last_master_verify = Some(std::time::Instant::now());
                            if let Some(ed) = self.editor.as_mut() {
                                match self.reauth_target {
                                    RevealRequest::Password => ed.reveal_password = true,
                                    RevealRequest::Passphrase => ed.reveal_passphrase = true,
                                    RevealRequest::None => {}
                                }
                            }
                            self.reauth_prompt = None;
                            self.reauth_target = RevealRequest::None;
                        } else {
                            if let Some(p) = self.reauth_prompt.as_mut() {
                                p.error = Some("Incorrect master password.".to_string());
                                p.password.clear();
                            }
                        }
                    } else {
                        self.reauth_prompt = None;
                        self.reauth_target = RevealRequest::None;
                        self.toaster
                            .error("Cannot verify", "Secrets are not unlocked");
                    }
                }
            }
        }

        CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(ui, |ui| {
                if self.dock.main_surface_mut().is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.heading("Open a connection from the sidebar");
                    });
                    return;
                }
                DockArea::new(&mut self.dock)
                    .style(Style::from_egui(ctx.global_style().as_ref()))
                    .show_inside(ui, &mut self.viewer);
            });

        if let Some(act) = self.viewer.recordings_action.take() {
            if let Some((title, body)) = act.toast_info {
                self.toaster.info(title, body);
            }
            if let Some((title, body)) = act.toast_warn {
                self.toaster.warn(title, body);
            }
            if let Some((title, body)) = act.toast_error {
                self.toaster.error(title, body);
            }
        }

        let tab_actions = std::mem::take(&mut self.viewer.actions);
        for action in tab_actions {
            self.handle_tab_action(action);
        }

        let palette_items = self.build_palette_items();
        match self.palette.show(&ctx, &palette_items) {
            PaletteResult::None => {}
            PaletteResult::Execute(cmd) => self.dispatch_command(cmd),
        }

        self.toaster.show(&ctx);

        if self.quit_requested {
            self.quit_requested = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            let _ = frame;
        }
    }
}

fn connection_needs_secret(conn: &Connection) -> bool {
    match &conn.auth {
        AuthMethod::Password { password } => !password.is_empty(),
        AuthMethod::PublicKey { passphrase, .. } => {
            passphrase.as_ref().map(|s| !s.is_empty()).unwrap_or(false)
        }
        AuthMethod::Agent => false,
    }
}

fn save_connections_no_secrets(paths: &ConfigPaths, store: &ConnectionStore) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::fs;
    fs::create_dir_all(&paths.config_dir)
        .with_context(|| format!("creating {}", paths.config_dir.display()))?;
    let mut sanitized = store.clone();
    for conn in &mut sanitized.connections {
        match &mut conn.auth {
            AuthMethod::Password { password } => password.clear(),
            AuthMethod::PublicKey { passphrase, .. } => *passphrase = None,
            AuthMethod::Agent => {}
        }
    }
    let text = toml::to_string_pretty(&sanitized).context("serializing connections")?;
    fs::write(&paths.connections_file, text)
        .with_context(|| format!("writing {}", paths.connections_file.display()))?;
    Ok(())
}

fn whoami_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

fn apply_reorder(store: &mut ConnectionStore, req: &ReorderRequest) -> bool {
    let from = match store.connections.iter().position(|c| c.id == req.dragged) {
        Some(i) => i,
        None => return false,
    };
    let mut moved = store.connections.remove(from);

    let new_group = if req.target_group == "Default" {
        None
    } else {
        Some(req.target_group.clone())
    };
    let group_changed = moved.group != new_group;
    moved.group = new_group;

    let insert_at = match req.target {
        Some(target_id) => store
            .connections
            .iter()
            .position(|c| c.id == target_id)
            .unwrap_or(store.connections.len()),
        None => {
            let group_key = req.target_group.as_str();
            let last_in_group = store
                .connections
                .iter()
                .rposition(|c| c.group.as_deref().unwrap_or("Default") == group_key);
            match last_in_group {
                Some(i) => i + 1,
                None => store.connections.len(),
            }
        }
    };

    if !group_changed && insert_at == from {
        store.connections.insert(from, moved);
        return false;
    }

    store.connections.insert(insert_at, moved);
    true
}
