use egui::{Color32, RichText, Ui, WidgetText};
use egui_dock::{NodePath, TabStyle, TabViewer};
use uuid::Uuid;

use crate::proto::ssh::TunnelStatusKind;
use crate::ui::recordings_view::{RecordingsAction, RecordingsTab, render_recordings_tab};
use crate::ui::rdp_tab::{RdpTab, render_rdp_tab};
use crate::ui::sftp_tab::{SftpTab, render_sftp_tab};
use crate::ui::terminal_widget::{TerminalEmulator, TerminalView};
use crate::ui::vnc_tab::{VncTab, render_vnc_tab};

pub struct TerminalTab {
    pub id: Uuid,
    pub source_connection: Option<Uuid>,
    pub title: String,
    pub connection_label: String,
    pub emulator: TerminalEmulator,
    pub closed_reported: bool,
    pub tab_color: Option<Color32>,
}

pub enum EshTab {
    Terminal(TerminalTab),
    Sftp(SftpTab),
    Rdp(RdpTab),
    Vnc(VncTab),
    Recordings(RecordingsTab),
}

impl EshTab {
    pub fn id(&self) -> Uuid {
        match self {
            EshTab::Terminal(t) => t.id,
            EshTab::Sftp(t) => t.id,
            EshTab::Rdp(t) => t.id,
            EshTab::Vnc(t) => t.id,
            EshTab::Recordings(t) => t.id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            EshTab::Terminal(t) => &t.title,
            EshTab::Sftp(t) => &t.title,
            EshTab::Rdp(t) => &t.title,
            EshTab::Vnc(t) => &t.title,
            EshTab::Recordings(t) => &t.title,
        }
    }

    pub fn source_connection(&self) -> Option<Uuid> {
        match self {
            EshTab::Terminal(t) => t.source_connection,
            EshTab::Sftp(t) => t.source_connection,
            EshTab::Rdp(t) => t.source_connection,
            EshTab::Vnc(t) => t.source_connection,
            EshTab::Recordings(_) => None,
        }
    }

    pub fn tab_color(&self) -> Option<Color32> {
        match self {
            EshTab::Terminal(t) => t.tab_color,
            EshTab::Sftp(t) => t.tab_color,
            EshTab::Rdp(t) => t.tab_color,
            EshTab::Vnc(t) => t.tab_color,
            EshTab::Recordings(_) => None,
        }
    }

    pub fn set_tab_color(&mut self, color: Option<Color32>) {
        match self {
            EshTab::Terminal(t) => t.tab_color = color,
            EshTab::Sftp(t) => t.tab_color = color,
            EshTab::Rdp(t) => t.tab_color = color,
            EshTab::Vnc(t) => t.tab_color = color,
            EshTab::Recordings(_) => {}
        }
    }

    pub fn is_sftp(&self) -> bool {
        matches!(self, EshTab::Sftp(_))
    }

    pub fn is_rdp(&self) -> bool {
        matches!(self, EshTab::Rdp(_))
    }

    pub fn is_vnc(&self) -> bool {
        matches!(self, EshTab::Vnc(_))
    }
}

pub enum TabAction {
    Duplicate { source_connection: Uuid, is_sftp: bool },
    Reconnect { tab_id: Uuid, source_connection: Uuid, is_sftp: bool },
}

#[derive(Default)]
pub struct EshTabViewer {
    pub actions: Vec<TabAction>,
    pub recordings_action: Option<RecordingsAction>,
}

const TAB_COLOR_PRESETS: &[(&str, Color32)] = &[
    ("Red", Color32::from_rgb(230, 90, 90)),
    ("Orange", Color32::from_rgb(230, 150, 70)),
    ("Yellow", Color32::from_rgb(220, 200, 70)),
    ("Green", Color32::from_rgb(100, 190, 110)),
    ("Cyan", Color32::from_rgb(90, 190, 200)),
    ("Blue", Color32::from_rgb(100, 150, 230)),
    ("Purple", Color32::from_rgb(170, 120, 220)),
    ("Pink", Color32::from_rgb(230, 130, 190)),
    ("Gray", Color32::from_rgb(170, 170, 170)),
];

impl TabViewer for EshTabViewer {
    type Tab = EshTab;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        let text = tab.title().to_string();
        match tab.tab_color() {
            Some(c) => WidgetText::from(RichText::new(text).color(c).strong()),
            None => WidgetText::from(text),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        match tab {
            EshTab::Terminal(t) => {
                let new_data = t.emulator.pump();
                if new_data {
                    t.emulator.refresh_find_matches();
                }
                render_tunnel_strip(ui, t);
                TerminalView { emulator: &mut t.emulator }.show(ui);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
            EshTab::Sftp(t) => {
                render_sftp_tab(ui, t);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            }
            EshTab::Rdp(t) => {
                render_rdp_tab(ui, t);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
            EshTab::Vnc(t) => {
                render_vnc_tab(ui, t);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
            EshTab::Recordings(t) => {
                let act = render_recordings_tab(ui, t);
                let has = act.toast_info.is_some()
                    || act.toast_warn.is_some()
                    || act.toast_error.is_some();
                if has {
                    self.recordings_action = Some(act);
                }
            }
        }
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(tab.id())
    }

    fn clear_background(&self, _tab: &Self::Tab) -> bool {
        true
    }

    fn context_menu(&mut self, ui: &mut Ui, tab: &mut Self::Tab, _path: NodePath) {
        if matches!(tab, EshTab::Recordings(_)) {
            return;
        }

        let tab_id = tab.id();
        let source = tab.source_connection();
        let is_sftp = tab.is_sftp();
        let _is_rdp = tab.is_rdp();
        let _is_vnc = tab.is_vnc();
        let current_color = tab.tab_color();

        let duplicate_enabled = source.is_some();
        let reconnect_enabled = source.is_some();

        if ui
            .add_enabled(duplicate_enabled, egui::Button::new("Duplicate"))
            .clicked()
        {
            if let Some(src) = source {
                self.actions.push(TabAction::Duplicate { source_connection: src, is_sftp });
            }
            ui.close();
        }

        if ui
            .add_enabled(reconnect_enabled, egui::Button::new("Reconnect"))
            .clicked()
        {
            if let Some(src) = source {
                self.actions.push(TabAction::Reconnect {
                    tab_id,
                    source_connection: src,
                    is_sftp,
                });
            }
            ui.close();
        }

        ui.menu_button("Tab Color", |ui| {
            for (name, color) in TAB_COLOR_PRESETS {
                let selected = current_color == Some(*color);
                let swatch_text =
                    RichText::new(format!("{}  {}", if selected { "*" } else { " " }, name))
                        .color(*color)
                        .strong();
                if ui.button(swatch_text).clicked() {
                    tab.set_tab_color(Some(*color));
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .add_enabled(current_color.is_some(), egui::Button::new("Reset"))
                .clicked()
            {
                tab.set_tab_color(None);
                ui.close();
            }
        });

        ui.separator();
    }

    fn tab_style_override(
        &self,
        tab: &Self::Tab,
        global_style: &TabStyle,
    ) -> Option<TabStyle> {
        let color = tab.tab_color()?;
        let mut style = global_style.clone();
        style.active.text_color = color;
        style.inactive.text_color = color;
        style.focused.text_color = color;
        style.hovered.text_color = color;
        style.active_with_kb_focus.text_color = color;
        style.inactive_with_kb_focus.text_color = color;
        style.focused_with_kb_focus.text_color = color;
        Some(style)
    }
}

fn render_tunnel_strip(ui: &mut Ui, tab: &TerminalTab) {
    let snapshot: Vec<_> = {
        let map = tab.emulator.tunnels().lock();
        let mut v: Vec<_> = map.values().cloned().collect();
        v.sort_by_key(|t| (t.listen_address.clone(), t.listen_port));
        v
    };
    if snapshot.is_empty() {
        return;
    }
    let counts = snapshot.iter().fold((0usize, 0, 0, 0), |mut acc, t| {
        match t.status {
            TunnelStatusKind::Listening { .. } => acc.0 += 1,
            TunnelStatusKind::Pending => acc.1 += 1,
            TunnelStatusKind::Failed { .. } => acc.2 += 1,
            TunnelStatusKind::Disabled => acc.3 += 1,
        }
        acc
    });
    let header = {
        let mut parts = Vec::new();
        if counts.0 > 0 {
            parts.push(format!("{} listening", counts.0));
        }
        if counts.1 > 0 {
            parts.push(format!("{} pending", counts.1));
        }
        if counts.2 > 0 {
            parts.push(format!("{} failed", counts.2));
        }
        if counts.3 > 0 {
            parts.push(format!("{} disabled", counts.3));
        }
        format!("Tunnels ({}) - {}", snapshot.len(), parts.join(", "))
    };
    egui::CollapsingHeader::new(RichText::new(header).strong().small())
        .id_salt(("tunnel_strip", tab.id))
        .default_open(false)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for t in &snapshot {
                    let (color, label) = match &t.status {
                        TunnelStatusKind::Pending => (Color32::from_rgb(220, 180, 60), "pending"),
                        TunnelStatusKind::Listening { .. } => {
                            (Color32::from_rgb(80, 180, 100), "listening")
                        }
                        TunnelStatusKind::Failed { .. } => {
                            (Color32::from_rgb(210, 90, 90), "failed")
                        }
                        TunnelStatusKind::Disabled => (Color32::DARK_GRAY, "disabled"),
                    };
                    let bound_suffix = match &t.status {
                        TunnelStatusKind::Listening { bound_port }
                            if *bound_port != t.listen_port =>
                        {
                            format!(" (bound :{bound_port})")
                        }
                        _ => String::new(),
                    };
                    let chip_text = format!("{} - {}{}", t.describe(), label, bound_suffix);
                    let chip = egui::Label::new(
                        RichText::new(chip_text).color(color).monospace().small(),
                    )
                    .sense(egui::Sense::hover());
                    let resp = ui.add(chip);
                    if let TunnelStatusKind::Failed { error } = &t.status {
                        resp.on_hover_text(error);
                    }
                    ui.separator();
                }
            });
        });
}
