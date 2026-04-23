use egui::{Color32, RichText, Ui, WidgetText};
use egui_dock::TabViewer;
use uuid::Uuid;

use crate::proto::ssh::TunnelStatusKind;
use crate::ui::terminal_widget::{TerminalEmulator, TerminalView};

pub struct TerminalTab {
    pub id: Uuid,
    pub title: String,
    pub connection_label: String,
    pub emulator: TerminalEmulator,
    pub closed_reported: bool,
}

pub struct EshTabViewer;

impl TabViewer for EshTabViewer {
    type Tab = TerminalTab;

    fn title(&mut self, tab: &mut Self::Tab) -> WidgetText {
        WidgetText::from(&tab.title)
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Self::Tab) {
        tab.emulator.pump();
        render_tunnel_strip(ui, tab);
        TerminalView { emulator: &mut tab.emulator }.show(ui);
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(tab.id)
    }

    fn clear_background(&self, _tab: &Self::Tab) -> bool {
        true
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
