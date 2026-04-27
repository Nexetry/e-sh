use egui::{
    Align, Color32, ComboBox, Context, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea,
    Sense, TextEdit, Vec2, Window,
};
use uuid::Uuid;

use crate::core::connection::{
    AuthMethod, Connection, ConnectionStore, FreeRdpResizeMode, Protocol, RdpBackend, Tunnel,
    TunnelKind,
};
use crate::ui::password_field::MaskedBuffer;

pub struct EditConnectionDialog {
    pub open: bool,
    pub draft: Connection,
    auth_kind: AuthKind,
    password: String,
    key_path: String,
    key_passphrase: String,
    remote_commands_buf: String,
    section: Section,
    pub reveal_password: bool,
    pub reveal_passphrase: bool,
    pub reveal_requested: RevealRequest,
}

/// Which secret field the user wants to reveal.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum RevealRequest {
    #[default]
    None,
    Password,
    Passphrase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthKind {
    Agent,
    Password,
    PublicKey,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Connection,
    Authentication,
    JumpHost,
    Tunnels,
    Session,
    Recording,
    Display,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Connection => "Connection",
            Section::Authentication => "Authentication",
            Section::JumpHost => "Jump Host",
            Section::Tunnels => "Tunnels",
            Section::Session => "Session",
            Section::Recording => "Recording",
            Section::Display => "Display",
        }
    }
}

#[derive(Default)]
pub struct DialogResult {
    pub saved: bool,
    pub cancelled: bool,
    pub reveal_requested: RevealRequest,
}

impl EditConnectionDialog {
    pub fn from_connection(conn: Connection) -> Self {
        let (auth_kind, password, key_path, key_passphrase) = match &conn.auth {
            AuthMethod::Agent => (AuthKind::Agent, String::new(), String::new(), String::new()),
            AuthMethod::Password { password } => (
                AuthKind::Password,
                password.clone(),
                String::new(),
                String::new(),
            ),
            AuthMethod::PublicKey { path, passphrase } => (
                AuthKind::PublicKey,
                String::new(),
                path.clone(),
                passphrase.clone().unwrap_or_default(),
            ),
        };
        Self {
            open: true,
            remote_commands_buf: conn.remote_commands.join("\n"),
            draft: conn,
            auth_kind,
            password,
            key_path,
            key_passphrase,
            section: Section::Connection,
            reveal_password: false,
            reveal_passphrase: false,
            reveal_requested: RevealRequest::None,
        }
    }

    pub fn show(&mut self, ctx: &Context, store: &ConnectionStore) -> DialogResult {
        let mut result = DialogResult::default();
        let mut keep_open = self.open;

        let style = ctx.global_style();
        let window_frame = Frame::window(&style)
            .inner_margin(Margin::ZERO)
            .fill(style.visuals.window_fill())
            .stroke(style.visuals.window_stroke());

        Window::new("connection_settings_window")
            .title_bar(false)
            .open(&mut keep_open)
            .collapsible(false)
            .resizable(true)
            .default_size(Vec2::new(820.0, 560.0))
            .min_width(720.0)
            .min_height(460.0)
            .frame(window_frame)
            .show(ctx, |ui| {
                self.render(ui, store, &mut result);
            });

        self.open = keep_open && !result.saved && !result.cancelled;
        if !self.open && !result.saved {
            result.cancelled = true;
        }
        result.reveal_requested = self.reveal_requested;
        self.reveal_requested = RevealRequest::None;
        result
    }

    fn render(&mut self, ui: &mut egui::Ui, store: &ConnectionStore, result: &mut DialogResult) {
        let visuals = ui.visuals().clone();
        let panel_bg = visuals.panel_fill;
        let sidebar_bg = visuals.extreme_bg_color;

        egui::Panel::top("edit_dialog_header")
            .frame(Frame::NONE.inner_margin(Margin::ZERO))
            .show_inside(ui, |ui| {
                self.header(ui);
                ui.add(egui::Separator::default().spacing(0.0));
            });

        egui::Panel::bottom("edit_dialog_footer")
            .frame(Frame::NONE.inner_margin(Margin::ZERO))
            .show_inside(ui, |ui| {
                ui.add(egui::Separator::default().spacing(0.0));
                self.footer(ui, result);
            });

        egui::Panel::left("edit_dialog_sidebar")
            .frame(
                Frame::NONE
                    .fill(sidebar_bg)
                    .inner_margin(Margin::symmetric(8, 12)),
            )
            .resizable(false)
            .exact_size(220.0)
            .show_inside(ui, |ui| {
                self.sidebar(ui);
            });

        egui::CentralPanel::default()
            .frame(
                Frame::NONE
                    .fill(panel_bg)
                    .inner_margin(Margin::symmetric(24, 20)),
            )
            .show_inside(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt("edit_dialog_detail_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.detail(ui, store);
                    });
            });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        Frame::NONE
            .inner_margin(Margin::symmetric(20, 14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Terminal Connection Settings")
                            .size(18.0)
                            .strong(),
                    );
                });
            });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ScrollArea::vertical()
                .id_salt("edit_dialog_sidebar_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.style_mut().spacing.item_spacing.y = 2.0;

                    self.sidebar_group(ui, "Terminal");
                    self.sidebar_item(ui, Section::Connection);
                    self.sidebar_item(ui, Section::Authentication);

                    if matches!(self.draft.protocol, Protocol::Rdp) {
                        ui.add_space(12.0);
                        self.sidebar_group(ui, "Advanced");
                        self.sidebar_item(ui, Section::Display);
                    } else if matches!(self.draft.protocol, Protocol::Vnc) {
                        // VNC has no extra sidebar sections beyond Connection + Auth
                    } else {
                        ui.add_space(12.0);
                        self.sidebar_group(ui, "Common");
                        self.sidebar_item(ui, Section::JumpHost);

                        ui.add_space(12.0);
                        self.sidebar_group(ui, "Advanced");
                        self.sidebar_item(ui, Section::Tunnels);
                        if matches!(self.draft.protocol, Protocol::Ssh) {
                            self.sidebar_item(ui, Section::Session);
                        }
                        if matches!(self.draft.protocol, Protocol::Ssh | Protocol::Sftp) {
                            self.sidebar_item(ui, Section::Recording);
                        }
                    }
                });
        });
    }

    fn sidebar_group(&self, ui: &mut egui::Ui, label: &str) {
        ui.add_space(2.0);
        ui.label(RichText::new(label).small().weak());
        ui.add_space(2.0);
    }

    fn sidebar_item(&mut self, ui: &mut egui::Ui, section: Section) {
        let selected = self.section == section;
        let sel_bg = ui.visuals().selection.bg_fill;
        let sel_fg = ui.visuals().selection.stroke.color;
        let text_fg = ui.visuals().text_color();
        let hover_bg = ui.visuals().widgets.hovered.bg_fill;
        let bg = if selected { sel_bg } else { Color32::TRANSPARENT };
        let fg = if selected { sel_fg } else { text_fg };

        let desired = Vec2::new(ui.available_width(), 28.0);
        let (rect, response) = ui.allocate_exact_size(desired, Sense::click());
        let hovered = response.hovered();
        let actual_bg = if hovered && !selected { hover_bg } else { bg };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), actual_bg);

        let label_galley = ui.painter().layout_no_wrap(
            section.label().to_string(),
            egui::FontId::proportional(14.0),
            fg,
        );
        let label_size = label_galley.size();
        let label_pos = egui::pos2(rect.left() + 12.0, rect.center().y - label_size.y / 2.0);
        ui.painter().galley(label_pos, label_galley, fg);

        if response.clicked() {
            self.section = section;
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, store: &ConnectionStore) {
        if matches!(self.section, Section::Recording)
            && !matches!(self.draft.protocol, Protocol::Ssh | Protocol::Sftp)
        {
            self.section = Section::Connection;
        }
        if matches!(self.section, Section::Session)
            && !matches!(self.draft.protocol, Protocol::Ssh)
        {
            self.section = Section::Connection;
        }
        if matches!(self.section, Section::Display)
            && !matches!(self.draft.protocol, Protocol::Rdp)
        {
            self.section = Section::Connection;
        }
        if matches!(self.draft.protocol, Protocol::Rdp | Protocol::Vnc)
            && matches!(
                self.section,
                Section::JumpHost | Section::Tunnels | Section::Session | Section::Recording
            )
        {
            self.section = Section::Connection;
        }
        match self.section {
            Section::Connection => self.connection_pane(ui),
            Section::Authentication => self.auth_pane(ui),
            Section::JumpHost => self.jump_host_pane(ui, store),
            Section::Tunnels => self.tunnels_pane(ui),
            Section::Session => self.session_pane(ui),
            Section::Recording => self.recording_pane(ui),
            Section::Display => self.display_pane(ui),
        }
    }

    fn recording_pane(&mut self, ui: &mut egui::Ui) {
        self.pane_header(
            ui,
            "Session Recording",
            "Capture this session to disk for later review.",
        );
        ui.checkbox(
            &mut self.draft.record_sessions,
            "Record sessions for this connection",
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(match self.draft.protocol {
                Protocol::Ssh => "SSH: server output is saved as asciicast v2 (gzipped).",
                Protocol::Sftp => "SFTP: operations are saved as JSON Lines (gzipped).",
                Protocol::Rdp => "Recording is not available for RDP connections.",
                Protocol::Vnc => "Recording is not available for VNC connections.",
            })
            .weak(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Recordings are plaintext on disk. Do not enable for sessions that may contain secrets you don't want persisted.",
            )
            .small()
            .weak(),
        );
    }

    fn session_pane(&mut self, ui: &mut egui::Ui) {
        self.pane_header(
            ui,
            "Session",
            "Keepalive, X11 forwarding, in-session commands, and local pre/post scripts.",
        );

        ui.label(RichText::new("Keepalive").strong());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label("Interval (seconds, 0 = disabled):");
            ui.add(egui::DragValue::new(&mut self.draft.keepalive_secs).range(0..=3600));
        });
        ui.label(
            RichText::new(
                "Sends SSH keepalive messages at this interval. Helps keep NAT/firewall sessions alive.",
            )
            .small()
            .weak(),
        );

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(RichText::new("X11 Forwarding").strong());
        ui.add_space(2.0);
        ui.checkbox(
            &mut self.draft.x11_forwarding,
            "Enable X11 forwarding",
        );
        ui.label(
            RichText::new(
                "Forwards X11 traffic to your local X server. Requires XQuartz on macOS, an X server on Linux, or VcXsrv/Xming on Windows.",
            )
            .small()
            .weak(),
        );

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(RichText::new("Custom Commands").strong());
        ui.add_space(2.0);
        ui.label(
            RichText::new("Commands to run inside the shell after the session starts. One per line.")
                .small()
                .weak(),
        );
        ui.add_space(4.0);
        ui.add(
            TextEdit::multiline(&mut self.remote_commands_buf)
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text("e.g. cd /var/log\nsudo -i"),
        );

        ui.add_space(14.0);
        ui.separator();
        ui.add_space(10.0);

        ui.label(RichText::new("Local Scripts").strong());
        ui.add_space(2.0);
        ui.label(
            RichText::new(
                "Run on the local machine. Interpreted by `sh -c` on Unix / `cmd /c` on Windows. Environment: ESH_HOST, ESH_PORT, ESH_USER, ESH_NAME.",
            )
            .small()
            .weak(),
        );
        ui.add_space(6.0);

        script_row(
            ui,
            "Before connect (blocking)",
            "Runs before the SSH handshake. Non-zero exit aborts the connection.",
            &mut self.draft.before_script,
            "before_script",
        );
        ui.add_space(8.0);
        script_row(
            ui,
            "After connected (async)",
            "Runs after the shell is ready. Non-blocking.",
            &mut self.draft.after_connect_script,
            "after_connect_script",
        );
        ui.add_space(8.0);
        script_row(
            ui,
            "After closed (async)",
            "Runs after the session ends.",
            &mut self.draft.after_close_script,
            "after_close_script",
        );
    }

    fn pane_header(&self, ui: &mut egui::Ui, title: &str, hint: &str) {
        ui.label(RichText::new(title).size(16.0).strong());
        ui.add_space(2.0);
        ui.label(RichText::new(hint).weak());
        ui.add_space(16.0);
    }
}

fn form_row(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.add_sized(
            Vec2::new(140.0, 24.0),
            egui::Label::new(RichText::new(format!("{label}:")).strong()),
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            add_contents(ui);
        });
    });
    ui.add_space(8.0);
}

fn script_row(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    slot: &mut Option<String>,
    salt: &str,
) {
    ui.label(RichText::new(title).strong());
    ui.label(RichText::new(hint).small().weak());
    let mut text = slot.clone().unwrap_or_default();
    let resp = ui.add(
        TextEdit::multiline(&mut text)
            .id_salt(salt)
            .desired_rows(2)
            .desired_width(f32::INFINITY)
            .hint_text("Shell command(s)"),
    );
    if resp.changed() {
        *slot = if text.trim().is_empty() {
            None
        } else {
            Some(text)
        };
    }
}

impl EditConnectionDialog {

    fn connection_pane(&mut self, ui: &mut egui::Ui) {
        self.pane_header(
            ui,
            "Connection",
            "Basic identification and network endpoint for this session.",
        );

        form_row(ui, "Display Name", |ui| {
            ui.add(TextEdit::singleline(&mut self.draft.name).desired_width(360.0));
        });

        form_row(ui, "Group", |ui| {
            let mut group = self.draft.group.clone().unwrap_or_default();
            if ui
                .add(
                    TextEdit::singleline(&mut group)
                        .desired_width(360.0)
                        .hint_text("Optional"),
                )
                .changed()
            {
                self.draft.group = if group.trim().is_empty() {
                    None
                } else {
                    Some(group)
                };
            }
        });

        form_row(ui, "Connection Type", |ui| {
            ui.horizontal(|ui| {
                ComboBox::from_id_salt("proto")
                    .selected_text(self.draft.protocol.label())
                    .width(180.0)
                    .show_ui(ui, |ui| {
                        for p in [Protocol::Ssh, Protocol::Sftp, Protocol::Rdp, Protocol::Vnc] {
                            if ui
                                .selectable_value(&mut self.draft.protocol, p, p.label())
                                .changed()
                            {
                                self.draft.port = p.default_port();
                            }
                        }
                    });
                ui.add_space(16.0);
                ui.label(RichText::new("Port:").strong());
                ui.add(egui::DragValue::new(&mut self.draft.port).range(1..=65535));
            });
        });

        form_row(ui, "Computer Name", |ui| {
            ui.add(
                TextEdit::singleline(&mut self.draft.host)
                    .desired_width(360.0)
                    .hint_text("hostname or IP"),
            );
        });

        form_row(ui, "Username", |ui| {
            ui.add(TextEdit::singleline(&mut self.draft.username).desired_width(360.0));
        });
    }

    fn display_pane(&mut self, ui: &mut egui::Ui) {
        self.pane_header(
            ui,
            "Display",
            "RDP backend, resolution and scaling settings.",
        );

        form_row(ui, "RDP Backend", |ui| {
            ComboBox::from_id_salt("rdp_backend")
                .selected_text(self.draft.rdp_backend.label())
                .width(220.0)
                .show_ui(ui, |ui| {
                    for b in RdpBackend::ALL {
                        ui.selectable_value(&mut self.draft.rdp_backend, b, b.label());
                    }
                });
        });

        // Contextual description for the selected backend
        ui.add_space(4.0);
        match self.draft.rdp_backend {
            RdpBackend::Auto => {
                ui.label(
                    RichText::new(
                        "Connects with the built-in client first. If the server requires \
                         the Graphics Pipeline (e.g. GNOME Remote Desktop), automatically \
                         retries with FreeRDP. Requires FreeRDP to be installed for the \
                         fallback to work."
                    ).weak().italics(),
                );
            }
            RdpBackend::Ironrdp => {
                ui.label(
                    RichText::new(
                        "Uses the built-in IronRDP client. Works with Windows RDP and xrdp. \
                         Does not support servers that require the Graphics Pipeline \
                         (e.g. GNOME Remote Desktop)."
                    ).weak().italics(),
                );
            }
            RdpBackend::Freerdp => {
                ui.label(
                    RichText::new(
                        "Uses an external FreeRDP client. The remote desktop is displayed \
                         in a separate window. Supports all RDP servers including GNOME \
                         Remote Desktop."
                    ).weak().italics(),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("FreeRDP must be installed:").strong());
                ui.add_space(4.0);

                egui::Grid::new("freerdp_install_grid")
                    .num_columns(2)
                    .spacing([12.0, 4.0])
                    .show(ui, |ui| {
                        ui.label(RichText::new("macOS").strong().small());
                        ui.label(RichText::new("brew install freerdp").monospace().small());
                        ui.end_row();

                        ui.label("");
                        ui.label(
                            RichText::new("Uses sdl-freerdp (native, no XQuartz needed)")
                                .weak().small(),
                        );
                        ui.end_row();

                        ui.label(RichText::new("Linux").strong().small());
                        ui.label(
                            RichText::new("apt install freerdp3-x11  or  dnf install freerdp")
                                .monospace().small(),
                        );
                        ui.end_row();

                        ui.label("");
                        ui.label(
                            RichText::new("Uses xfreerdp3 / xfreerdp (requires X11 or Wayland)")
                                .weak().small(),
                        );
                        ui.end_row();

                        ui.label(RichText::new("Windows").strong().small());
                        ui.label(
                            RichText::new("winget install --id FreeRDP.FreeRDP")
                                .monospace().small(),
                        );
                        ui.end_row();

                        ui.label("");
                        ui.label(
                            RichText::new("Or download from github.com/FreeRDP/FreeRDP/releases")
                                .weak().small(),
                        );
                        ui.end_row();
                    });
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        form_row(ui, "Resolution", |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut self.draft.rdp_width)
                        .range(640..=7680)
                        .prefix("W: "),
                );
                ui.label("×");
                ui.add(
                    egui::DragValue::new(&mut self.draft.rdp_height)
                        .range(480..=4320)
                        .prefix("H: "),
                );
            });
        });

        // FreeRDP resize mode — only relevant when FreeRDP may be used
        if self.draft.rdp_backend != RdpBackend::Ironrdp {
            form_row(ui, "Resize Mode", |ui| {
                ui.horizontal(|ui| {
                    ComboBox::from_id_salt("freerdp_resize")
                        .selected_text(self.draft.freerdp_resize_mode.label())
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for m in FreeRdpResizeMode::ALL {
                                ui.selectable_value(
                                    &mut self.draft.freerdp_resize_mode,
                                    m,
                                    m.label(),
                                );
                            }
                        });
                    ui.weak("ℹ").on_hover_text(
                        "Dynamic resolution: session resizes when you resize the window.\n\
                         Smart sizing: client-side scaling to fit the window.\n\
                         Static: fixed resolution using the width × height above.",
                    );
                });
            });
        }
    }

    fn auth_pane(&mut self, ui: &mut egui::Ui) {
        self.pane_header(
            ui,
            "Authentication",
            "How e-sh proves identity to the remote host.",
        );

        form_row(ui, "Method", |ui| {
            ComboBox::from_id_salt("auth")
                .selected_text(match self.auth_kind {
                    AuthKind::Agent => "SSH agent",
                    AuthKind::Password => "Password",
                    AuthKind::PublicKey => "Public key",
                })
                .width(220.0)
                .show_ui(ui, |ui| {
                    if !matches!(self.draft.protocol, Protocol::Rdp) {
                        ui.selectable_value(&mut self.auth_kind, AuthKind::Agent, "SSH agent");
                    }
                    ui.selectable_value(&mut self.auth_kind, AuthKind::Password, "Password");
                    if !matches!(self.draft.protocol, Protocol::Rdp) {
                        ui.selectable_value(&mut self.auth_kind, AuthKind::PublicKey, "Public key");
                    }
                });
        });

        match self.auth_kind {
            AuthKind::Agent => {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Uses your running SSH agent (SSH_AUTH_SOCK). Each loaded identity \
                         will be tried in order until one is accepted. Run `ssh-add` to load keys.",
                    )
                    .italics()
                    .weak(),
                );
            }
            AuthKind::Password => {
                form_row(ui, "Password", |ui| {
                    ui.horizontal(|ui| {
                        if self.reveal_password {
                            ui.add(
                                TextEdit::singleline(&mut self.password)
                                    .desired_width(310.0),
                            );
                            if ui.button("\u{1F512}").on_hover_text("Hide password").clicked() {
                                self.reveal_password = false;
                            }
                        } else {
                            let mut buf = MaskedBuffer::new(&mut self.password);
                            ui.add(
                                TextEdit::singleline(&mut buf)
                                    .desired_width(310.0),
                            );
                            if !self.password.is_empty()
                                && ui.button("\u{1F441}").on_hover_text("Reveal password").clicked()
                            {
                                self.reveal_requested = RevealRequest::Password;
                            }
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Encrypted with your master password and stored in secrets.enc.toml.",
                    )
                    .small()
                    .weak(),
                );
            }
            AuthKind::PublicKey => {
                form_row(ui, "Key Path", |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            TextEdit::singleline(&mut self.key_path)
                                .desired_width(310.0)
                                .hint_text("~/.ssh/id_ed25519"),
                        );
                        if ui.button("...").on_hover_text("Browse").clicked() {
                            let mut dialog =
                                rfd::FileDialog::new().set_title("Select private key");
                            if let Some(home) = std::env::var_os("HOME") {
                                let ssh_dir = std::path::PathBuf::from(home).join(".ssh");
                                if ssh_dir.exists() {
                                    dialog = dialog.set_directory(ssh_dir);
                                }
                            }
                            if let Some(picked) = dialog.pick_file() {
                                self.key_path = picked.to_string_lossy().into_owned();
                            }
                        }
                    });
                });
                form_row(ui, "Passphrase", |ui| {
                    ui.horizontal(|ui| {
                        if self.reveal_passphrase {
                            ui.add(
                                TextEdit::singleline(&mut self.key_passphrase)
                                    .desired_width(310.0)
                                    .hint_text("Leave empty if key is unencrypted"),
                            );
                            if ui.button("\u{1F512}").on_hover_text("Hide passphrase").clicked() {
                                self.reveal_passphrase = false;
                            }
                        } else {
                            let mut buf = MaskedBuffer::new(&mut self.key_passphrase);
                            ui.add(
                                TextEdit::singleline(&mut buf)
                                    .desired_width(310.0)
                                    .hint_text("Leave empty if key is unencrypted"),
                            );
                            if !self.key_passphrase.is_empty()
                                && ui.button("\u{1F441}").on_hover_text("Reveal passphrase").clicked()
                            {
                                self.reveal_requested = RevealRequest::Passphrase;
                            }
                        }
                    });
                });
            }
        }
    }

    fn jump_host_pane(&mut self, ui: &mut egui::Ui, store: &ConnectionStore) {
        self.pane_header(
            ui,
            "Jump Host",
            "Tunnel through one or more saved SSH hosts (chained ProxyJump). Hops connect in order, top to bottom.",
        );

        let candidates: Vec<&Connection> = store
            .connections
            .iter()
            .filter(|c| c.id != self.draft.id && c.protocol == Protocol::Ssh)
            .collect();

        let label_for = |id: Uuid| -> String {
            store
                .find(id)
                .map(|c| format!("{} ({}@{}:{})", c.name, c.username, c.host, c.port))
                .unwrap_or_else(|| format!("(missing: {id})"))
        };

        ui.label(RichText::new("Hop chain").strong());
        ui.add_space(4.0);

        if self.draft.jump_chain.is_empty() {
            ui.label(
                RichText::new("(none - connect directly)")
                    .small()
                    .weak(),
            );
        } else {
            let mut remove_idx: Option<usize> = None;
            let mut move_up: Option<usize> = None;
            let mut move_down: Option<usize> = None;
            let len = self.draft.jump_chain.len();

            for (idx, hop_id) in self.draft.jump_chain.clone().iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}.", idx + 1)).monospace());
                    ui.add_space(4.0);
                    ComboBox::from_id_salt(("jump_hop", idx))
                        .selected_text(label_for(*hop_id))
                        .width(320.0)
                        .show_ui(ui, |ui| {
                            for c in &candidates {
                                let label =
                                    format!("{} ({}@{}:{})", c.name, c.username, c.host, c.port);
                                ui.selectable_value(
                                    &mut self.draft.jump_chain[idx],
                                    c.id,
                                    label,
                                );
                            }
                        });
                    ui.add_space(8.0);
                    if ui
                        .add_enabled(idx > 0, egui::Button::new("Up"))
                        .clicked()
                    {
                        move_up = Some(idx);
                    }
                    if ui
                        .add_enabled(idx + 1 < len, egui::Button::new("Down"))
                        .clicked()
                    {
                        move_down = Some(idx);
                    }
                    if ui.button("Remove").clicked() {
                        remove_idx = Some(idx);
                    }
                });
            }

            if let Some(i) = move_up {
                self.draft.jump_chain.swap(i - 1, i);
            }
            if let Some(i) = move_down {
                self.draft.jump_chain.swap(i, i + 1);
            }
            if let Some(i) = remove_idx {
                self.draft.jump_chain.remove(i);
            }
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let can_add = !candidates.is_empty();
            if ui
                .add_enabled(can_add, egui::Button::new("+ Add hop"))
                .clicked()
            {
                if let Some(first) = candidates.first() {
                    self.draft.jump_chain.push(first.id);
                }
            }
            if !self.draft.jump_chain.is_empty() && ui.button("Clear").clicked() {
                self.draft.jump_chain.clear();
            }
        });

        if candidates.is_empty() {
            ui.add_space(6.0);
            ui.label(
                RichText::new("No other SSH connections available. Add one to use it as a jump host.")
                    .small()
                    .weak(),
            );
        }

        if !self.draft.jump_chain.is_empty() {
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new("Resolved path").strong());
            ui.add_space(4.0);
            let mut path: Vec<String> = self
                .draft
                .jump_chain
                .iter()
                .map(|id| {
                    store
                        .find(*id)
                        .map(|c| format!("{}@{}:{}", c.username, c.host, c.port))
                        .unwrap_or_else(|| "(missing)".to_string())
                })
                .collect();
            path.push(format!(
                "{}@{}:{}",
                self.draft.username, self.draft.host, self.draft.port
            ));
            ui.label(RichText::new(path.join("  ->  ")).monospace());
        }
    }

    fn tunnels_pane(&mut self, ui: &mut egui::Ui) {
        self.pane_header(
            ui,
            "Tunnels",
            "Local (-L), remote (-R), and dynamic (-D, SOCKS5) port forwards.",
        );

        ui.horizontal(|ui| {
            if ui.button("+ Local (-L)").clicked() {
                self.draft.tunnels.push(Tunnel {
                    id: Uuid::new_v4(),
                    kind: TunnelKind::Local,
                    listen_address: "127.0.0.1".into(),
                    listen_port: 8080,
                    remote_host: "127.0.0.1".into(),
                    remote_port: 80,
                    enabled: true,
                });
            }
            if ui.button("+ Remote (-R)").clicked() {
                self.draft.tunnels.push(Tunnel {
                    id: Uuid::new_v4(),
                    kind: TunnelKind::Remote,
                    listen_address: "127.0.0.1".into(),
                    listen_port: 8080,
                    remote_host: "127.0.0.1".into(),
                    remote_port: 80,
                    enabled: true,
                });
            }
            if ui.button("+ Dynamic (-D)").clicked() {
                self.draft.tunnels.push(Tunnel {
                    id: Uuid::new_v4(),
                    kind: TunnelKind::Dynamic,
                    listen_address: "127.0.0.1".into(),
                    listen_port: 1080,
                    remote_host: String::new(),
                    remote_port: 0,
                    enabled: true,
                });
            }
        });

        ui.add_space(12.0);

        if self.draft.tunnels.is_empty() {
            ui.label(RichText::new("No tunnels configured.").italics().weak());
            return;
        }

        let mut to_delete: Option<usize> = None;
        for (idx, tunnel) in self.draft.tunnels.iter_mut().enumerate() {
            Frame::group(ui.style())
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut tunnel.enabled, "");
                        ui.label(RichText::new(tunnel.kind.label()).strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Remove").clicked() {
                                to_delete = Some(idx);
                            }
                        });
                    });
                    ui.add_space(6.0);
                    egui::Grid::new(("tunnel_grid", tunnel.id))
                        .num_columns(2)
                        .spacing([12.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Listen address");
                            ui.add(
                                TextEdit::singleline(&mut tunnel.listen_address)
                                    .desired_width(220.0),
                            );
                            ui.end_row();
                            ui.label("Listen port");
                            ui.add(egui::DragValue::new(&mut tunnel.listen_port).range(1..=65535));
                            ui.end_row();
                            if tunnel.kind != TunnelKind::Dynamic {
                                ui.label("Remote host");
                                ui.add(
                                    TextEdit::singleline(&mut tunnel.remote_host)
                                        .desired_width(220.0),
                                );
                                ui.end_row();
                                ui.label("Remote port");
                                ui.add(
                                    egui::DragValue::new(&mut tunnel.remote_port).range(1..=65535),
                                );
                                ui.end_row();
                            }
                        });
                });
            ui.add_space(8.0);
        }
        if let Some(idx) = to_delete {
            self.draft.tunnels.remove(idx);
        }
    }

    fn footer(&mut self, ui: &mut egui::Ui, result: &mut DialogResult) {
        Frame::NONE
            .inner_margin(Margin::symmetric(20, 12))
            .show(ui, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let apply =
                        ui.add_sized(Vec2::new(140.0, 32.0), egui::Button::new("Apply & Close"));
                    if apply.clicked() {
                        self.commit_auth();
                        result.saved = true;
                    }
                    ui.add_space(8.0);
                    let discard = ui.add_sized(
                        Vec2::new(140.0, 32.0),
                        egui::Button::new("Discard changes"),
                    );
                    if discard.clicked() {
                        result.cancelled = true;
                    }
                });
            });
    }

    fn commit_auth(&mut self) {
        self.draft.auth = match self.auth_kind {
            AuthKind::Agent => AuthMethod::Agent,
            AuthKind::Password => AuthMethod::Password {
                password: self.password.clone(),
            },
            AuthKind::PublicKey => AuthMethod::PublicKey {
                path: self.key_path.clone(),
                passphrase: if self.key_passphrase.is_empty() {
                    None
                } else {
                    Some(self.key_passphrase.clone())
                },
            },
        };
        self.draft.remote_commands = self
            .remote_commands_buf
            .lines()
            .map(|s| s.trim_end().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
}
