use eframe::{App, CreationContext, Frame};
use egui::{CentralPanel, Panel, Ui};
use egui_dock::{DockArea, DockState, Style};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::runtime::Handle;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use uuid::Uuid;

use e_sh::config::host_keys::{HostKeyPrompt, HostKeyStore};
use e_sh::config::store::{ConfigPaths, forget_secrets, load_connections, save_connections};
use e_sh::core::connection::{Connection, ConnectionStore, Protocol};
use e_sh::proto::sftp::spawn_sftp_session;
use e_sh::proto::ssh::{HostKeyContext, spawn_session};
use e_sh::ui::connection_tree::ConnectionTree;
use e_sh::ui::dock::{EshTab, EshTabViewer, TerminalTab};
use e_sh::ui::edit_dialog::EditConnectionDialog;
use e_sh::ui::host_key_prompt::{HostKeyPromptResult, HostKeyPromptUi};
use e_sh::ui::sftp_tab::SftpTab;
use e_sh::ui::status_bar::StatusBar;
use e_sh::ui::terminal_widget::TerminalEmulator;
use e_sh::ui::toast::Toaster;

pub struct EshApp {
    rt: Handle,
    paths: Arc<ConfigPaths>,
    store: ConnectionStore,
    host_keys: Arc<Mutex<HostKeyStore>>,
    host_key_prompt_tx: UnboundedSender<HostKeyPrompt>,
    host_key_prompt_rx: UnboundedReceiver<HostKeyPrompt>,
    pending_host_key_prompts: VecDeque<HostKeyPrompt>,
    dock: DockState<EshTab>,
    viewer: EshTabViewer,
    status: String,
    editor: Option<EditConnectionDialog>,
    toaster: Toaster,
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

        Self {
            rt,
            paths,
            store,
            host_keys: Arc::new(Mutex::new(host_keys)),
            host_key_prompt_tx,
            host_key_prompt_rx,
            pending_host_key_prompts: VecDeque::new(),
            dock: DockState::new(Vec::new()),
            viewer: EshTabViewer,
            status: "Ready".to_string(),
            editor: None,
            toaster: Toaster::default(),
        }
    }

    fn open_connection(&mut self, id: Uuid) {
        let Some(conn) = self.store.find(id).cloned() else {
            return;
        };
        if matches!(conn.protocol, Protocol::Sftp) {
            self.open_sftp_tab(id);
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
        let handle = spawn_session(&self.rt, chain, host_ctx);
        let emulator = TerminalEmulator::new(handle, 80, 24);
        let title = format!("{} ({})", conn.name, conn.protocol.label());
        let connection_label = format!("{}@{}:{}", conn.username, conn.host, conn.port);
        let tab = EshTab::Terminal(TerminalTab {
            id: Uuid::new_v4(),
            title,
            connection_label,
            emulator,
            closed_reported: false,
        });
        self.push_tab(tab);
        self.status = format!("Opened {}@{}", conn.username, conn.host);
        self.toaster
            .info("Connecting", format!("{}@{}:{}", conn.username, conn.host, conn.port));
    }

    fn open_sftp_tab(&mut self, id: Uuid) {
        let Some(conn) = self.store.find(id).cloned() else {
            return;
        };
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
        let handle = spawn_sftp_session(&self.rt, chain, host_ctx);
        let title = format!("{} (SFTP)", conn.name);
        let connection_label = format!("{}@{}:{}", conn.username, conn.host, conn.port);
        let tab = EshTab::Sftp(SftpTab::new(
            Uuid::new_v4(),
            title,
            connection_label.clone(),
            handle,
        ));
        self.push_tab(tab);
        self.status = format!("Opened SFTP {}", connection_label);
        self.toaster.info("SFTP", connection_label);
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
        if let Err(e) = save_connections(&self.paths, &self.store) {
            self.status = format!("Save failed: {e}");
            self.toaster.error("Save failed", e.to_string());
        }
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
            }
        }
    }
}

impl App for EshApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut Frame) {
        Panel::bottom("status").show_inside(ui, |ui| {
            StatusBar { message: &self.status }.show(ui);
        });

        Panel::left("connections")
            .resizable(true)
            .default_size(240.0)
            .show_inside(ui, |ui| {
                let action = ConnectionTree { store: &self.store }.show(ui);
                if action.new_connection {
                    self.start_new_connection();
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
                if let Some(id) = action.delete {
                    if let Some(removed) = self.store.remove(id) {
                        forget_secrets(&removed);
                        self.persist();
                        self.status = "Deleted connection".to_string();
                        self.toaster.warn("Deleted", removed.name);
                    }
                }
            });

        let ctx = ui.ctx().clone();

        self.poll_session_errors();

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
                                self.toaster
                                    .warn("Host key rejected", host_id);
                            }
                            e_sh::config::host_keys::HostKeyDecision::AcceptOnce => {
                                self.toaster
                                    .info("Host key accepted (once)", host_id);
                            }
                            e_sh::config::host_keys::HostKeyDecision::AcceptAndSave => {
                                self.toaster
                                    .success("Host key saved", host_id);
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

        self.toaster.show(&ctx);
    }
}

fn whoami_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}
