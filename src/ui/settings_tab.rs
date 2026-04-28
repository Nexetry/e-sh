use std::path::PathBuf;

use egui::{Color32, RichText, ScrollArea, Ui};
use uuid::Uuid;

use crate::config::theme::{
    self, Theme, ThemeColors, builtin_themes, export_theme, import_theme,
    save_theme,
};
use crate::recording::manifest::{ManifestStore, RecordingEntry, RecordingKind};

/// Actions the settings tab wants the app to perform.
#[derive(Default)]
pub struct SettingsAction {
    pub toast_info: Option<(String, String)>,
    pub toast_warn: Option<(String, String)>,
    pub toast_error: Option<(String, String)>,
    pub theme_changed: bool,
}

pub struct SettingsTab {
    pub id: Uuid,
    pub title: String,
    // Theme state
    pub config_dir: PathBuf,
    pub current_theme: Theme,
    pub custom_edit: ThemeColors,
    pub editing_custom: bool,
    // Recordings state
    pub recordings_dir: PathBuf,
    rec_store: ManifestStore,
    rec_error: Option<String>,
    // Section toggle
    active_section: SettingsSection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsSection {
    Theme,
    Recordings,
}

impl SettingsTab {
    pub fn new(config_dir: PathBuf, recordings_dir: PathBuf, current_theme: Theme) -> Self {
        let custom_edit = current_theme.colors.clone();
        let mut tab = Self {
            id: Uuid::new_v4(),
            title: "Settings".to_string(),
            config_dir,
            current_theme,
            custom_edit,
            editing_custom: false,
            recordings_dir,
            rec_store: ManifestStore::default(),
            rec_error: None,
            active_section: SettingsSection::Theme,
        };
        tab.reload_recordings();
        tab
    }

    pub fn reload_recordings(&mut self) {
        match ManifestStore::load(&self.recordings_dir) {
            Ok(s) => {
                self.rec_store = s;
                self.rec_error = None;
            }
            Err(e) => {
                self.rec_store = ManifestStore::default();
                self.rec_error = Some(format!("Failed to load manifest: {e}"));
            }
        }
    }
}

pub fn render_settings_tab(ui: &mut Ui, tab: &mut SettingsTab) -> SettingsAction {
    let mut action = SettingsAction::default();

    ui.horizontal(|ui| {
        ui.heading("Settings");
    });
    ui.separator();

    // Section tabs
    ui.horizontal(|ui| {
        if ui
            .selectable_label(tab.active_section == SettingsSection::Theme, "🎨 Theme")
            .clicked()
        {
            tab.active_section = SettingsSection::Theme;
        }
        if ui
            .selectable_label(
                tab.active_section == SettingsSection::Recordings,
                "⏺ Recordings",
            )
            .clicked()
        {
            tab.active_section = SettingsSection::Recordings;
        }
    });
    ui.separator();

    match tab.active_section {
        SettingsSection::Theme => render_theme_section(ui, tab, &mut action),
        SettingsSection::Recordings => render_recordings_section(ui, tab, &mut action),
    }

    action
}

// ── Theme section ──────────────────────────────────────────────────────

fn render_theme_section(ui: &mut Ui, tab: &mut SettingsTab, action: &mut SettingsAction) {
    ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("Built-in Themes").strong());
            ui.add_space(4.0);

            let builtins = builtin_themes();
            egui::Grid::new("builtin_themes_grid")
                .num_columns(4)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for theme in &builtins {
                        let selected = !tab.editing_custom && tab.current_theme.name == theme.name;
                        if render_theme_card(ui, theme, selected) {
                            tab.current_theme = theme.clone();
                            tab.editing_custom = false;
                            tab.custom_edit = theme.colors.clone();
                            apply_and_save(tab, action);
                        }
                    }
                });

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            // Custom theme section
            ui.label(RichText::new("Custom Theme").strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new("Pick your own colors. Changes apply immediately.")
                    .weak()
                    .small(),
            );
            ui.add_space(8.0);

            if !tab.editing_custom {
                if ui.button("Start from current theme").clicked() {
                    tab.custom_edit = tab.current_theme.colors.clone();
                    tab.editing_custom = true;
                }
            }

            if tab.editing_custom {
                let changed = render_color_editor(ui, &mut tab.custom_edit);
                if changed {
                    tab.current_theme = Theme {
                        name: "Custom".to_string(),
                        builtin: false,
                        colors: tab.custom_edit.clone(),
                    };
                    apply_and_save(tab, action);
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Reset to Dark").clicked() {
                        tab.current_theme = theme::dark_theme();
                        tab.custom_edit = tab.current_theme.colors.clone();
                        tab.editing_custom = false;
                        apply_and_save(tab, action);
                    }
                });
            }

            ui.add_space(16.0);
            ui.separator();
            ui.add_space(8.0);

            // Import / Export
            ui.label(RichText::new("Import / Export").strong());
            ui.add_space(4.0);
            ui.label(
                RichText::new("Share themes as .toml files with others.")
                    .weak()
                    .small(),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Import theme…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TOML", &["toml"])
                        .pick_file()
                    {
                        match import_theme(&path) {
                            Ok(imported) => {
                                tab.current_theme = imported;
                                tab.custom_edit = tab.current_theme.colors.clone();
                                tab.editing_custom = !tab.current_theme.builtin;
                                apply_and_save(tab, action);
                                action.toast_info = Some((
                                    "Theme imported".to_string(),
                                    tab.current_theme.name.clone(),
                                ));
                            }
                            Err(e) => {
                                action.toast_error =
                                    Some(("Import failed".to_string(), e.to_string()));
                            }
                        }
                    }
                }
                if ui.button("Export theme…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TOML", &["toml"])
                        .set_file_name(&format!("{}.toml", tab.current_theme.name.to_lowercase()))
                        .save_file()
                    {
                        match export_theme(&path, &tab.current_theme) {
                            Ok(()) => {
                                action.toast_info = Some((
                                    "Theme exported".to_string(),
                                    path.display().to_string(),
                                ));
                            }
                            Err(e) => {
                                action.toast_error =
                                    Some(("Export failed".to_string(), e.to_string()));
                            }
                        }
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.weak("Theme file:");
                ui.monospace(theme::theme_path(&tab.config_dir).display().to_string());
            });
        });
}

fn apply_and_save(tab: &mut SettingsTab, action: &mut SettingsAction) {
    action.theme_changed = true;
    if let Err(e) = save_theme(&tab.config_dir, &tab.current_theme) {
        action.toast_error = Some(("Theme save failed".to_string(), e.to_string()));
    }
}

fn render_theme_card(ui: &mut Ui, theme: &Theme, selected: bool) -> bool {
    let c = &theme.colors;
    let bg = Color32::from_rgb(c.bg_primary[0], c.bg_primary[1], c.bg_primary[2]);
    let fg = Color32::from_rgb(c.text_primary[0], c.text_primary[1], c.text_primary[2]);
    let accent = Color32::from_rgb(c.accent[0], c.accent[1], c.accent[2]);
    let border_c = if selected {
        accent
    } else {
        Color32::from_rgb(c.border[0], c.border[1], c.border[2])
    };

    let width = 120.0;
    let height = 72.0;

    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, bg);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(if selected { 2.0 } else { 1.0 }, border_c),
        egui::StrokeKind::Outside,
    );

    // Mini preview bars
    let bar_y = rect.min.y + 8.0;
    let bar_h = 4.0;
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, bar_y),
            egui::vec2(width * 0.6, bar_h),
        ),
        2.0,
        fg,
    );
    painter.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 8.0, bar_y + 8.0),
            egui::vec2(width * 0.4, bar_h),
        ),
        2.0,
        accent,
    );

    // Theme name
    let text_pos = egui::pos2(rect.min.x + 8.0, rect.max.y - 20.0);
    painter.text(
        text_pos,
        egui::Align2::LEFT_TOP,
        &theme.name,
        egui::FontId::proportional(11.0),
        fg,
    );

    if selected {
        let check_pos = egui::pos2(rect.max.x - 14.0, rect.min.y + 10.0);
        painter.text(
            check_pos,
            egui::Align2::CENTER_CENTER,
            "✓",
            egui::FontId::proportional(12.0),
            accent,
        );
    }

    response.clicked()
}

const COLOR_FIELDS: &[(&str, &str)] = &[
    ("bg_primary", "Background"),
    ("bg_secondary", "Panel background"),
    ("bg_tertiary", "Hover background"),
    ("text_primary", "Text"),
    ("text_secondary", "Secondary text"),
    ("accent", "Accent"),
    ("accent_hover", "Accent hover"),
    ("border", "Border"),
    ("success", "Success"),
    ("warning", "Warning"),
    ("error", "Error"),
    ("selection_bg", "Selection background"),
    ("selection_text", "Selection text"),
    ("tab_bar_bg", "Tab bar"),
    ("sidebar_bg", "Sidebar"),
    ("status_bar_bg", "Status bar"),
];

fn render_color_editor(ui: &mut Ui, colors: &mut ThemeColors) -> bool {
    let mut changed = false;

    egui::Grid::new("color_editor_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for (field, label) in COLOR_FIELDS {
                ui.label(*label);
                let rgb = get_color_mut(colors, field);
                let mut c = [rgb[0], rgb[1], rgb[2]];
                if ui.color_edit_button_srgb(&mut c).changed() {
                    rgb[0] = c[0];
                    rgb[1] = c[1];
                    rgb[2] = c[2];
                    changed = true;
                }
                ui.end_row();
            }
        });

    changed
}

fn get_color_mut<'a>(colors: &'a mut ThemeColors, field: &str) -> &'a mut [u8; 3] {
    match field {
        "bg_primary" => &mut colors.bg_primary,
        "bg_secondary" => &mut colors.bg_secondary,
        "bg_tertiary" => &mut colors.bg_tertiary,
        "text_primary" => &mut colors.text_primary,
        "text_secondary" => &mut colors.text_secondary,
        "accent" => &mut colors.accent,
        "accent_hover" => &mut colors.accent_hover,
        "border" => &mut colors.border,
        "success" => &mut colors.success,
        "warning" => &mut colors.warning,
        "error" => &mut colors.error,
        "selection_bg" => &mut colors.selection_bg,
        "selection_text" => &mut colors.selection_text,
        "tab_bar_bg" => &mut colors.tab_bar_bg,
        "sidebar_bg" => &mut colors.sidebar_bg,
        "status_bar_bg" => &mut colors.status_bar_bg,
        _ => &mut colors.bg_primary,
    }
}

// ── Recordings section ─────────────────────────────────────────────────

fn render_recordings_section(ui: &mut Ui, tab: &mut SettingsTab, action: &mut SettingsAction) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Session Recordings").strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                tab.reload_recordings();
            }
            if ui.button("Open folder").clicked() {
                match reveal_path(&tab.recordings_dir) {
                    Ok(()) => {
                        action.toast_info = Some((
                            "Opened folder".to_string(),
                            tab.recordings_dir.display().to_string(),
                        ));
                    }
                    Err(e) => {
                        action.toast_error = Some(("Failed to open folder".to_string(), e));
                    }
                }
            }
            if ui.button("Clean up missing").clicked() {
                let removed = clean_missing(&mut tab.rec_store, &tab.recordings_dir);
                match removed {
                    Ok(n) if n == 0 => {
                        action.toast_info =
                            Some(("Clean up".to_string(), "No missing files".to_string()));
                    }
                    Ok(n) => {
                        action.toast_warn = Some((
                            "Cleaned".to_string(),
                            format!("Removed {n} manifest row(s) with missing files"),
                        ));
                    }
                    Err(e) => {
                        action.toast_error = Some(("Clean up failed".to_string(), e));
                    }
                }
            }
        });
    });

    ui.horizontal_wrapped(|ui| {
        ui.weak("Directory:");
        ui.monospace(tab.recordings_dir.display().to_string());
    });

    if let Some(err) = tab.rec_error.clone() {
        ui.colored_label(Color32::from_rgb(210, 90, 90), err);
    }

    ui.separator();

    if tab.rec_store.entries.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No recordings yet.").weak());
            ui.label(
                RichText::new(
                    "Enable \"Record sessions\" in a connection's edit dialog to capture future sessions.",
                )
                .weak()
                .small(),
            );
        });
        return;
    }

    let row_height = 22.0;
    let avail_w = ui.available_width();
    let name_w = (avail_w * 0.22).clamp(140.0, 260.0);
    let started_w = 170.0;
    let kind_w = 60.0;
    let dur_w = 90.0;
    let size_w = 90.0;
    let status_w = 110.0;

    ui.horizontal(|ui| {
        ui.add_sized([name_w, row_height], egui::Label::new(RichText::new("Connection").strong().small()));
        ui.add_sized([started_w, row_height], egui::Label::new(RichText::new("Started").strong().small()));
        ui.add_sized([kind_w, row_height], egui::Label::new(RichText::new("Kind").strong().small()));
        ui.add_sized([dur_w, row_height], egui::Label::new(RichText::new("Duration").strong().small()));
        ui.add_sized([size_w, row_height], egui::Label::new(RichText::new("Size").strong().small()));
        ui.add_sized([status_w, row_height], egui::Label::new(RichText::new("Status").strong().small()));
        ui.label(RichText::new("Actions").strong().small());
    });
    ui.separator();

    let entries: Vec<RecordingEntry> = {
        let mut v = tab.rec_store.entries.clone();
        v.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        v
    };

    let mut delete_id: Option<Uuid> = None;

    ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for entry in &entries {
                let file_path = tab.recordings_dir.join(&entry.file);
                let exists = file_path.exists();
                let size_bytes = if exists {
                    std::fs::metadata(&file_path).ok().map(|m| m.len()).unwrap_or(entry.bytes_captured)
                } else {
                    entry.bytes_captured
                };

                ui.horizontal(|ui| {
                    ui.add_sized(
                        [name_w, row_height],
                        egui::Label::new(
                            RichText::new(&entry.connection_name).monospace().small(),
                        )
                        .truncate(),
                    );
                    ui.add_sized(
                        [started_w, row_height],
                        egui::Label::new(RichText::new(&entry.started_at).small()),
                    );
                    ui.add_sized(
                        [kind_w, row_height],
                        egui::Label::new(RichText::new(entry.kind.label()).small()),
                    );
                    ui.add_sized(
                        [dur_w, row_height],
                        egui::Label::new(RichText::new(format_duration(entry)).small()),
                    );
                    ui.add_sized(
                        [size_w, row_height],
                        egui::Label::new(RichText::new(format_size(size_bytes)).small()),
                    );
                    let (status_text, status_color) = status_label(entry, exists);
                    ui.add_sized(
                        [status_w, row_height],
                        egui::Label::new(
                            RichText::new(status_text).color(status_color).small(),
                        ),
                    );

                    let reveal = ui.small_button("Reveal").on_hover_text(
                        if exists {
                            format!("Reveal {}", file_path.display())
                        } else {
                            "File missing".to_string()
                        },
                    );
                    if reveal.clicked() {
                        if exists {
                            match reveal_path(&file_path) {
                                Ok(()) => {}
                                Err(e) => {
                                    action.toast_error =
                                        Some(("Reveal failed".to_string(), e));
                                }
                            }
                        } else {
                            action.toast_warn = Some((
                                "File missing".to_string(),
                                file_path.display().to_string(),
                            ));
                        }
                    }

                    if ui.small_button("Copy path").clicked() {
                        ui.ctx().copy_text(file_path.display().to_string());
                        action.toast_info = Some((
                            "Copied path".to_string(),
                            file_path.display().to_string(),
                        ));
                    }

                    let del = ui.small_button("Delete").on_hover_text(
                        "Remove manifest row and file from disk",
                    );
                    if del.clicked() {
                        delete_id = Some(entry.id);
                    }
                });
                ui.separator();
            }
        });

    if let Some(id) = delete_id {
        match tab.rec_store.delete(id, &tab.recordings_dir) {
            Ok(true) => {
                action.toast_warn =
                    Some(("Deleted recording".to_string(), String::new()));
            }
            Ok(false) => {
                action.toast_warn = Some((
                    "Not found".to_string(),
                    "Recording already removed".to_string(),
                ));
            }
            Err(e) => {
                action.toast_error = Some(("Delete failed".to_string(), e.to_string()));
            }
        }
    }
}

fn format_duration(entry: &RecordingEntry) -> String {
    match entry.duration_ms {
        Some(ms) => {
            let s = ms / 1000;
            let h = s / 3600;
            let m = (s % 3600) / 60;
            let sec = s % 60;
            if h > 0 {
                format!("{h}h {m}m {sec}s")
            } else if m > 0 {
                format!("{m}m {sec}s")
            } else {
                format!("{sec}s")
            }
        }
        None => "—".to_string(),
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut idx = 0;
    while v >= 1024.0 && idx + 1 < UNITS.len() {
        v /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{bytes} B")
    } else {
        format!("{:.1} {}", v, UNITS[idx])
    }
}

fn status_label(entry: &RecordingEntry, exists: bool) -> (&'static str, Color32) {
    if !exists {
        return ("File missing", Color32::from_rgb(210, 90, 90));
    }
    if entry.partial {
        return ("Partial", Color32::from_rgb(220, 180, 60));
    }
    if entry.ended_at.is_none() {
        return ("Incomplete", Color32::from_rgb(220, 180, 60));
    }
    match entry.kind {
        RecordingKind::Ssh | RecordingKind::Sftp => {
            ("Complete", Color32::from_rgb(80, 180, 100))
        }
    }
}

fn clean_missing(
    store: &mut ManifestStore,
    recordings_dir: &std::path::Path,
) -> Result<usize, String> {
    let ids: Vec<Uuid> = store
        .entries
        .iter()
        .filter(|e| !recordings_dir.join(&e.file).exists())
        .map(|e| e.id)
        .collect();
    let mut removed = 0usize;
    for id in ids {
        match store.delete(id, recordings_dir) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(removed)
}

fn reveal_path(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let arg = path.as_os_str();
        let status = std::process::Command::new("open")
            .arg("-R")
            .arg(arg)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            let _ = std::process::Command::new("open")
                .arg(arg)
                .status()
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        if path.is_dir() {
            let status = std::process::Command::new("explorer")
                .arg(path)
                .status()
                .map_err(|e| e.to_string())?;
            if status.success() {
                return Ok(());
            }
        } else {
            let arg = format!("/select,{}", path.display());
            let status = std::process::Command::new("explorer")
                .arg(arg)
                .status()
                .map_err(|e| e.to_string())?;
            if status.success() {
                return Ok(());
            }
        }
        return Err("explorer failed".to_string());
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let target = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let status = std::process::Command::new("xdg-open")
            .arg(target)
            .status()
            .map_err(|e| e.to_string())?;
        if !status.success() {
            return Err("xdg-open failed".to_string());
        }
        Ok(())
    }
}
