use std::path::{Path, PathBuf};

use egui::{RichText, ScrollArea, Ui};
use uuid::Uuid;

use crate::recording::manifest::{ManifestStore, RecordingEntry, RecordingKind};

pub struct RecordingsTab {
    pub id: Uuid,
    pub title: String,
    pub recordings_dir: PathBuf,
    store: ManifestStore,
    last_error: Option<String>,
    last_action: Option<String>,
}

impl RecordingsTab {
    pub fn new(recordings_dir: PathBuf) -> Self {
        let mut tab = Self {
            id: Uuid::new_v4(),
            title: "Recordings".to_string(),
            recordings_dir,
            store: ManifestStore::default(),
            last_error: None,
            last_action: None,
        };
        tab.reload();
        tab
    }

    pub fn reload(&mut self) {
        match ManifestStore::load(&self.recordings_dir) {
            Ok(s) => {
                self.store = s;
                self.last_error = None;
            }
            Err(e) => {
                self.store = ManifestStore::default();
                self.last_error = Some(format!("Failed to load manifest: {e}"));
            }
        }
    }
}

#[derive(Default)]
pub struct RecordingsAction {
    pub toast_info: Option<(String, String)>,
    pub toast_warn: Option<(String, String)>,
    pub toast_error: Option<(String, String)>,
}

pub fn render_recordings_tab(ui: &mut Ui, tab: &mut RecordingsTab) -> RecordingsAction {
    let mut action = RecordingsAction::default();

    ui.horizontal(|ui| {
        ui.heading("Recordings");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Refresh").clicked() {
                tab.reload();
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
                        action.toast_error =
                            Some(("Failed to open folder".to_string(), e));
                    }
                }
            }
            if ui.button("Clean up missing").clicked() {
                let removed = clean_missing(&mut tab.store, &tab.recordings_dir);
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

    if let Some(err) = tab.last_error.clone() {
        ui.colored_label(egui::Color32::from_rgb(210, 90, 90), err);
    }
    if let Some(msg) = tab.last_action.take() {
        ui.weak(msg);
    }

    ui.separator();

    if tab.store.entries.is_empty() {
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
        return action;
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
        let mut v = tab.store.entries.clone();
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
        match tab.store.delete(id, &tab.recordings_dir) {
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

    action
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

fn status_label(entry: &RecordingEntry, exists: bool) -> (&'static str, egui::Color32) {
    if !exists {
        return ("File missing", egui::Color32::from_rgb(210, 90, 90));
    }
    if entry.partial {
        return ("Partial", egui::Color32::from_rgb(220, 180, 60));
    }
    if entry.ended_at.is_none() {
        return ("Incomplete", egui::Color32::from_rgb(220, 180, 60));
    }
    match entry.kind {
        RecordingKind::Ssh | RecordingKind::Sftp => {
            ("Complete", egui::Color32::from_rgb(80, 180, 100))
        }
    }
}

fn clean_missing(store: &mut ManifestStore, recordings_dir: &Path) -> Result<usize, String> {
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

fn reveal_path(path: &Path) -> Result<(), String> {
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
