use egui::Ui;

pub struct StatusBar<'a> {
    pub message: &'a str,
}

impl<'a> StatusBar<'a> {
    pub fn show(self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.small(self.message);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.small("e-sh");
            });
        });
    }
}
