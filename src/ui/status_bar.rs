use egui::Ui;

pub struct StatusBar<'a> {
    pub message: &'a str,
}

pub struct StatusBarResponse {
    pub version_clicked: bool,
}

impl<'a> StatusBar<'a> {
    pub fn show(self, ui: &mut Ui) -> StatusBarResponse {
        let mut version_clicked = false;
        ui.horizontal(|ui| {
            ui.small(self.message);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = format!("e-sh v{}", env!("CARGO_PKG_VERSION"));
                let resp = ui.small_button(label);
                if resp.clicked() {
                    version_clicked = true;
                }
                resp.on_hover_text("Check for updates");
            });
        });
        StatusBarResponse { version_clicked }
    }
}
