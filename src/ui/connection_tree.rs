use egui::{CollapsingHeader, ScrollArea, Ui};
use uuid::Uuid;

use crate::core::connection::{ConnectionStore, Protocol};

pub struct ConnectionTree<'a> {
    pub store: &'a ConnectionStore,
}

#[derive(Default)]
pub struct TreeAction {
    pub open: Option<Uuid>,
    pub open_sftp: Option<Uuid>,
    pub edit: Option<Uuid>,
    pub delete: Option<Uuid>,
    pub new_connection: bool,
}

impl<'a> ConnectionTree<'a> {
    pub fn show(self, ui: &mut Ui) -> TreeAction {
        let mut action = TreeAction::default();
        ui.horizontal(|ui| {
            ui.heading("Connections");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("+")
                    .on_hover_text("New connection")
                    .clicked()
                {
                    action.new_connection = true;
                }
            });
        });
        ui.separator();
        ScrollArea::vertical().show(ui, |ui| {
            let mut groups: std::collections::BTreeMap<String, Vec<&_>> = Default::default();
            for c in &self.store.connections {
                groups
                    .entry(c.group.clone().unwrap_or_else(|| "Default".to_string()))
                    .or_default()
                    .push(c);
            }
            if groups.is_empty() {
                ui.weak("No saved connections.");
                ui.weak("Click ＋ above to add one.");
            }
            for (group, items) in groups {
                CollapsingHeader::new(group)
                    .default_open(true)
                    .show(ui, |ui| {
                        for c in items {
                            let label = format!("{}  ·  {}", c.name, c.display_address());
                            let resp = ui
                                .selectable_label(false, label)
                                .on_hover_text(format!(
                                    "{} {}",
                                    c.protocol.label(),
                                    c.display_address()
                                ));
                            if resp.double_clicked() {
                                action.open = Some(c.id);
                            }
                            resp.context_menu(|ui| {
                                if ui.button("Open").clicked() {
                                    action.open = Some(c.id);
                                    ui.close();
                                }
                                if matches!(c.protocol, Protocol::Ssh | Protocol::Sftp) {
                                    if ui.button("Open SFTP").clicked() {
                                        action.open_sftp = Some(c.id);
                                        ui.close();
                                    }
                                }
                                if ui.button("Edit…").clicked() {
                                    action.edit = Some(c.id);
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button("Delete").clicked() {
                                    action.delete = Some(c.id);
                                    ui.close();
                                }
                            });
                        }
                    });
            }
        });
        action
    }
}
