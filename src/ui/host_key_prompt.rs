use egui::{Color32, Context, RichText, Window};

use crate::config::host_keys::{HostKeyDecision, HostKeyPrompt, HostKeyPromptKind};

pub struct HostKeyPromptUi;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyPromptResult {
    Pending,
    Decided(HostKeyDecision),
}

impl HostKeyPromptUi {
    pub fn show(ctx: &Context, prompt: &HostKeyPrompt) -> HostKeyPromptResult {
        let mut result = HostKeyPromptResult::Pending;

        let title = match &prompt.kind {
            HostKeyPromptKind::NewHost => "New SSH host key",
            HostKeyPromptKind::Mismatch { .. } => "WARNING: host key changed",
        };

        Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(520.0);

                match &prompt.kind {
                    HostKeyPromptKind::NewHost => {
                        ui.label(format!(
                            "The authenticity of host '{}:{}' can't be established.",
                            prompt.host, prompt.port
                        ));
                        ui.add_space(6.0);
                        ui.label(format!("{} key fingerprint:", prompt.algorithm));
                        ui.code(&prompt.fingerprint);
                        ui.add_space(8.0);
                        ui.label("Are you sure you want to continue connecting?");
                    }
                    HostKeyPromptKind::Mismatch { expected } => {
                        ui.colored_label(
                            Color32::from_rgb(220, 80, 80),
                            RichText::new(
                                "REMOTE HOST IDENTIFICATION HAS CHANGED!\nIt is possible someone is doing something nasty (man-in-the-middle attack)."
                            )
                            .strong(),
                        );
                        ui.add_space(8.0);
                        ui.label(format!("Host: {}:{}", prompt.host, prompt.port));
                        ui.add_space(6.0);
                        ui.label("Expected key:");
                        ui.code(format!("{} {}", expected.algorithm, expected.fingerprint));
                        ui.add_space(4.0);
                        ui.label("Offered key:");
                        ui.code(format!("{} {}", prompt.algorithm, prompt.fingerprint));
                    }
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Reject").clicked() {
                        result = HostKeyPromptResult::Decided(HostKeyDecision::Reject);
                    }
                    if ui.button("Accept once").clicked() {
                        result = HostKeyPromptResult::Decided(HostKeyDecision::AcceptOnce);
                    }
                    let save_label = match prompt.kind {
                        HostKeyPromptKind::NewHost => "Accept and save",
                        HostKeyPromptKind::Mismatch { .. } => "Accept and overwrite saved key",
                    };
                    if ui.button(save_label).clicked() {
                        result = HostKeyPromptResult::Decided(HostKeyDecision::AcceptAndSave);
                    }
                });
            });

        result
    }
}
