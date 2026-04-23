use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use egui::{Color32, RichText, ScrollArea, Ui};
use egui_extras::{Column, TableBuilder};
use uuid::Uuid;

use crate::proto::sftp::{
    SftpCommand, SftpEntry, SftpEvent, SftpHandle, TransferDirection,
};

pub struct TransferState {
    pub label: String,
    pub direction: TransferDirection,
    pub bytes: u64,
    pub total: Option<u64>,
    pub done: bool,
    pub error: Option<String>,
}

pub struct SftpTab {
    pub id: Uuid,
    pub source_connection: Option<Uuid>,
    pub title: String,
    pub connection_label: String,
    pub handle: SftpHandle,
    pub closed: Option<String>,
    pub closed_reported: bool,
    pub tab_color: Option<Color32>,

    pub remote_cwd: String,
    pub remote_entries: Vec<SftpEntry>,
    pub remote_loading: bool,
    pub remote_selected: HashSet<String>,
    pub remote_anchor: Option<String>,

    pub local_cwd: PathBuf,
    pub local_selected: HashSet<PathBuf>,
    pub local_anchor: Option<PathBuf>,

    pub transfers: HashMap<Uuid, TransferState>,
    pub transfer_order: Vec<Uuid>,

    pub last_message: Option<String>,

    pub mkdir_buffer: String,
    pub rename_target: Option<String>,
    pub rename_target_pane: Option<Pane>,
    pub rename_buffer: String,

    pub local_path_buffer: String,
    pub remote_path_buffer: String,
    pub local_path_dirty: bool,
    pub remote_path_dirty: bool,

    pub mkdir_dialog: Option<Pane>,

    pub local_filter: String,
    pub remote_filter: String,
    pub local_sort: (SortKey, SortDir),
    pub remote_sort: (SortKey, SortDir),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SftpTab {
    pub fn new(
        id: Uuid,
        source_connection: Option<Uuid>,
        title: String,
        connection_label: String,
        handle: SftpHandle,
    ) -> Self {
        let local_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let local_path_buffer = local_cwd.display().to_string();
        Self {
            id,
            source_connection,
            title,
            connection_label,
            handle,
            closed: None,
            closed_reported: false,
            tab_color: None,
            remote_cwd: String::from("/"),
            remote_entries: Vec::new(),
            remote_loading: true,
            remote_selected: HashSet::new(),
            remote_anchor: None,
            local_cwd,
            local_selected: HashSet::new(),
            local_anchor: None,
            transfers: HashMap::new(),
            transfer_order: Vec::new(),
            last_message: None,
            mkdir_buffer: String::new(),
            rename_target: None,
            rename_target_pane: None,
            rename_buffer: String::new(),
            local_path_buffer,
            remote_path_buffer: String::from("/"),
            local_path_dirty: false,
            remote_path_dirty: false,
            mkdir_dialog: None,
            local_filter: String::new(),
            remote_filter: String::new(),
            local_sort: (SortKey::Name, SortDir::Asc),
            remote_sort: (SortKey::Name, SortDir::Asc),
        }
    }

    pub fn pump(&mut self) {
        while let Ok(ev) = self.handle.events.try_recv() {
            match ev {
                SftpEvent::Connected { home } => {
                    self.remote_cwd = home;
                    self.remote_loading = true;
                }
                SftpEvent::DirListing { path, entries } => {
                    self.remote_cwd = path;
                    self.remote_entries = entries;
                    self.remote_loading = false;
                    if !self.remote_path_dirty {
                        self.remote_path_buffer = self.remote_cwd.clone();
                    }
                }
                SftpEvent::OperationOk { message } => {
                    self.last_message = Some(message);
                    let _ = self
                        .handle
                        .commands
                        .send(SftpCommand::ListDir { path: self.remote_cwd.clone() });
                }
                SftpEvent::OperationError { message } => {
                    self.last_message = Some(format!("error: {message}"));
                }
                SftpEvent::TransferStarted { id, direction, label, total } => {
                    self.transfers.insert(
                        id,
                        TransferState {
                            label,
                            direction,
                            bytes: 0,
                            total,
                            done: false,
                            error: None,
                        },
                    );
                    self.transfer_order.push(id);
                }
                SftpEvent::TransferProgress { id, bytes, total } => {
                    if let Some(t) = self.transfers.get_mut(&id) {
                        t.bytes = bytes;
                        if total.is_some() {
                            t.total = total;
                        }
                    }
                }
                SftpEvent::TransferDone { id } => {
                    if let Some(t) = self.transfers.get_mut(&id) {
                        t.done = true;
                    }
                    let _ = self
                        .handle
                        .commands
                        .send(SftpCommand::ListDir { path: self.remote_cwd.clone() });
                }
                SftpEvent::TransferFailed { id, error } => {
                    if let Some(t) = self.transfers.get_mut(&id) {
                        t.done = true;
                        t.error = Some(error);
                    }
                }
                SftpEvent::Closed(reason) => {
                    self.closed = Some(reason.unwrap_or_else(|| "session closed".into()));
                }
            }
        }
    }
}

pub fn render_sftp_tab(ui: &mut Ui, tab: &mut SftpTab) {
    tab.pump();

    handle_dropped_files(ui, tab);

    egui::Panel::bottom(egui::Id::new(("sftp_transfers", tab.id)))
        .resizable(true)
        .default_size(140.0)
        .show_inside(ui, |ui| render_transfer_strip(ui, tab));

    egui::Panel::top(egui::Id::new(("sftp_header", tab.id)))
        .show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&tab.connection_label).monospace().small());
                if let Some(msg) = &tab.last_message {
                    ui.separator();
                    ui.label(RichText::new(msg).small().color(if msg.starts_with("error:") {
                        Color32::from_rgb(220, 110, 110)
                    } else {
                        Color32::GRAY
                    }));
                }
            });
            ui.separator();
        });

    let total_w = ui.available_width();
    let pane_w = (total_w - 8.0).max(200.0) / 2.0;
    egui::Panel::left(egui::Id::new(("sftp_local", tab.id)))
        .resizable(true)
        .default_size(pane_w)
        .show_inside(ui, |ui| {
            ui.push_id(("sftp_local_pane", tab.id), |ui| render_local_pane(ui, tab));
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show_inside(ui, |ui| {
            ui.push_id(("sftp_remote_pane", tab.id), |ui| render_remote_pane(ui, tab));
        });
}

fn render_local_pane(ui: &mut Ui, tab: &mut SftpTab) {
    ui.label(RichText::new("Local").strong());
    render_local_breadcrumb(ui, tab);
    render_local_path_field(ui, tab);
    render_filter_row(ui, &mut tab.local_filter, ("local_filter", tab.id));
    ui.separator();

    let entries = sorted_filtered_local(&list_local(&tab.local_cwd), &tab.local_filter, tab.local_sort);
    let scroll_resp = ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut clicked_header: Option<SortKey> = None;
            ui.push_id(("sftp_local_table", tab.id), |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(260.0).at_least(80.0).clip(true).resizable(true))
                    .column(Column::initial(80.0).at_least(40.0).resizable(true))
                    .column(Column::initial(160.0).at_least(80.0).resizable(true).clip(true))
                    .header(20.0, |mut h| {
                        h.col(|ui| { if sort_header(ui, "Name", SortKey::Name, tab.local_sort).clicked() { clicked_header = Some(SortKey::Name); } });
                        h.col(|ui| { if sort_header(ui, "Size", SortKey::Size, tab.local_sort).clicked() { clicked_header = Some(SortKey::Size); } });
                        h.col(|ui| { if sort_header(ui, "Modified", SortKey::Modified, tab.local_sort).clicked() { clicked_header = Some(SortKey::Modified); } });
                    })
                    .body(|mut body| {
                        for (idx, entry) in entries.iter().enumerate() {
                            body.row(20.0, |mut row| {
                                row.col(|ui| {
                                    let icon = if entry.is_dir { "\u{1F4C1}" } else { "\u{1F4C4}" };
                                    let label = format!("{icon}  {}", entry.name);
                                    let selected = tab.local_selected.contains(&entry.path);
                                    let resp = ui.selectable_label(selected, label);
                                    if resp.clicked() {
                                        let mods = ui.input(|i| i.modifiers);
                                        update_local_selection(tab, &entries, idx, mods);
                                    }
                                    if resp.secondary_clicked() && !tab.local_selected.contains(&entry.path) {
                                        tab.local_selected.clear();
                                        tab.local_selected.insert(entry.path.clone());
                                        tab.local_anchor = Some(entry.path.clone());
                                    }
                                    if resp.double_clicked() && entry.is_dir {
                                        tab.local_cwd = entry.path.clone();
                                        tab.local_selected.clear();
                                        tab.local_anchor = None;
                                        tab.local_path_dirty = false;
                                        tab.local_path_buffer = tab.local_cwd.display().to_string();
                                    }
                                    local_entry_context_menu(&resp, tab, entry);
                                });
                                row.col(|ui| { ui.label(RichText::new(format_size(entry.size)).small().monospace()); });
                                row.col(|ui| { ui.label(RichText::new(format_mtime(entry.modified)).small().monospace()); });
                            });
                        }
                    });
            });
            if let Some(k) = clicked_header { toggle_sort(&mut tab.local_sort, k); }
            ui.allocate_response(ui.available_size(), egui::Sense::click())
        });
    pane_empty_context_menu(&scroll_resp.inner, tab, Pane::Local);
    render_mkdir_dialog(ui, tab, Pane::Local);
    render_rename_dialog(ui, tab, Pane::Local);
}

fn render_local_breadcrumb(ui: &mut Ui, tab: &mut SftpTab) {
    let path = tab.local_cwd.clone();
    let mut nav: Option<PathBuf> = None;
    ui.horizontal_wrapped(|ui| {
        let comps: Vec<_> = path.components().collect();
        let mut acc = PathBuf::new();
        for (i, c) in comps.iter().enumerate() {
            use std::path::Component;
            let label = match c {
                Component::RootDir => "/".to_string(),
                Component::Prefix(p) => p.as_os_str().to_string_lossy().into_owned(),
                Component::Normal(s) => s.to_string_lossy().into_owned(),
                Component::CurDir => ".".to_string(),
                Component::ParentDir => "..".to_string(),
            };
            acc.push(c.as_os_str());
            let is_last = i + 1 == comps.len();
            let resp = ui.add(egui::Link::new(
                RichText::new(&label).small().monospace(),
            ));
            if resp.clicked() && !is_last {
                nav = Some(acc.clone());
            }
            if !is_last && !matches!(c, Component::RootDir | Component::Prefix(_)) {
                ui.label(RichText::new("/").small().monospace().color(Color32::GRAY));
            }
        }
    });
    if let Some(p) = nav {
        tab.local_cwd = p;
        tab.local_selected.clear();
        tab.local_anchor = None;
        tab.local_path_dirty = false;
        tab.local_path_buffer = tab.local_cwd.display().to_string();
    }
}

fn render_local_path_field(ui: &mut Ui, tab: &mut SftpTab) {
    if !tab.local_path_dirty {
        let want = tab.local_cwd.display().to_string();
        if tab.local_path_buffer != want {
            tab.local_path_buffer = want;
        }
    }
    let resp = ui.add(
        egui::TextEdit::singleline(&mut tab.local_path_buffer)
            .desired_width(f32::INFINITY)
            .hint_text("local path"),
    );
    if resp.changed() {
        tab.local_path_dirty = true;
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let p = PathBuf::from(tab.local_path_buffer.trim());
        if p.is_dir() {
            tab.local_cwd = p;
            tab.local_selected.clear();
            tab.local_anchor = None;
        } else {
            tab.last_message = Some(format!("error: not a directory: {}", tab.local_path_buffer));
        }
        tab.local_path_dirty = false;
        tab.local_path_buffer = tab.local_cwd.display().to_string();
    }
}

fn local_entry_context_menu(resp: &egui::Response, tab: &mut SftpTab, entry: &LocalEntry) {
    resp.context_menu(|ui| {
        let selection: Vec<PathBuf> = if tab.local_selected.contains(&entry.path) {
            tab.local_selected.iter().cloned().collect()
        } else {
            vec![entry.path.clone()]
        };
        let n = selection.len();

        if n == 1 && entry.is_dir {
            if ui.button("Open").clicked() {
                tab.local_cwd = entry.path.clone();
                tab.local_selected.clear();
                tab.local_anchor = None;
                tab.local_path_dirty = false;
                tab.local_path_buffer = tab.local_cwd.display().to_string();
                ui.close();
            }
        }
        let upload_label = if n > 1 {
            format!("Upload {n} items >> {}", tab.remote_cwd)
        } else if entry.is_dir {
            format!("Upload folder >> {}", tab.remote_cwd)
        } else {
            format!("Upload >> {}", tab.remote_cwd)
        };
        if ui.button(upload_label).clicked() {
            for p in &selection {
                let name = p.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let remote = join_remote(&tab.remote_cwd, &name);
                let id = Uuid::new_v4();
                let _ = tab.handle.commands.send(SftpCommand::Upload {
                    id,
                    local: p.clone(),
                    remote,
                });
            }
            ui.close();
        }
        ui.separator();
        ui.add_enabled_ui(n == 1, |ui| {
            if ui.button("Rename").clicked() {
                let name = entry.path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                tab.rename_buffer = name.clone();
                tab.rename_target = Some(name);
                tab.rename_target_pane = Some(Pane::Local);
                ui.close();
            }
        });
        let del_label = if n > 1 { format!("Delete {n} items") } else { "Delete".to_string() };
        if ui.button(del_label).clicked() {
            let mut ok = 0usize;
            let mut errs: Vec<String> = Vec::new();
            for p in &selection {
                let is_dir = p.is_dir();
                let res = if is_dir {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_file(p)
                };
                match res {
                    Ok(_) => ok += 1,
                    Err(e) => errs.push(format!("{}: {e}", p.display())),
                }
            }
            if errs.is_empty() {
                tab.last_message = Some(format!("rm {ok} items"));
            } else {
                tab.last_message = Some(format!("rm {ok} ok, {} errors: {}", errs.len(), errs.join("; ")));
            }
            tab.local_selected.clear();
            tab.local_anchor = None;
            ui.close();
        }
    });
}

fn render_remote_pane(ui: &mut Ui, tab: &mut SftpTab) {
    ui.label(RichText::new("Remote").strong());
    render_remote_breadcrumb(ui, tab);
    render_remote_path_field(ui, tab);
    render_filter_row(ui, &mut tab.remote_filter, ("remote_filter", tab.id));
    ui.separator();

    if tab.remote_loading && tab.remote_entries.is_empty() {
        ui.label(RichText::new("loading...").small().italics());
        return;
    }

    let entries = sorted_filtered_remote(&tab.remote_entries, &tab.remote_filter, tab.remote_sort);
    let scroll_resp = ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut clicked_header: Option<SortKey> = None;
            ui.push_id(("sftp_remote_table", tab.id), |ui| {
                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(260.0).at_least(80.0).clip(true).resizable(true))
                    .column(Column::initial(80.0).at_least(40.0).resizable(true))
                    .column(Column::initial(160.0).at_least(80.0).resizable(true).clip(true))
                    .header(20.0, |mut h| {
                        h.col(|ui| { if sort_header(ui, "Name", SortKey::Name, tab.remote_sort).clicked() { clicked_header = Some(SortKey::Name); } });
                        h.col(|ui| { if sort_header(ui, "Size", SortKey::Size, tab.remote_sort).clicked() { clicked_header = Some(SortKey::Size); } });
                        h.col(|ui| { if sort_header(ui, "Modified", SortKey::Modified, tab.remote_sort).clicked() { clicked_header = Some(SortKey::Modified); } });
                    })
                    .body(|mut body| {
                        for (idx, entry) in entries.iter().enumerate() {
                            body.row(20.0, |mut row| {
                                row.col(|ui| {
                                    let icon = if entry.is_dir {
                                        "\u{1F4C1}"
                                    } else if entry.is_symlink {
                                        "\u{1F517}"
                                    } else {
                                        "\u{1F4C4}"
                                    };
                                    let label = format!("{icon}  {}", entry.name);
                                    let selected = tab.remote_selected.contains(&entry.name);
                                    let resp = ui.selectable_label(selected, label);
                                    if resp.clicked() {
                                        let mods = ui.input(|i| i.modifiers);
                                        update_remote_selection(tab, &entries, idx, mods);
                                    }
                                    if resp.secondary_clicked() && !tab.remote_selected.contains(&entry.name) {
                                        tab.remote_selected.clear();
                                        tab.remote_selected.insert(entry.name.clone());
                                        tab.remote_anchor = Some(entry.name.clone());
                                    }
                                    if resp.double_clicked() && entry.is_dir {
                                        let path = join_remote(&tab.remote_cwd, &entry.name);
                                        tab.remote_loading = true;
                                        tab.remote_path_dirty = false;
                                        let _ = tab.handle.commands.send(SftpCommand::ListDir { path });
                                    }
                                    remote_entry_context_menu(&resp, tab, entry);
                                });
                                row.col(|ui| { ui.label(RichText::new(format_size(entry.size)).small().monospace()); });
                                row.col(|ui| { ui.label(RichText::new(format_mtime(entry.modified)).small().monospace()); });
                            });
                        }
                    });
            });
            if let Some(k) = clicked_header { toggle_sort(&mut tab.remote_sort, k); }
            ui.allocate_response(ui.available_size(), egui::Sense::click())
        });
    pane_empty_context_menu(&scroll_resp.inner, tab, Pane::Remote);
    render_mkdir_dialog(ui, tab, Pane::Remote);
    render_rename_dialog(ui, tab, Pane::Remote);
}

fn render_remote_breadcrumb(ui: &mut Ui, tab: &mut SftpTab) {
    let path = tab.remote_cwd.clone();
    let mut nav: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        let resp_root = ui.add(egui::Link::new(
            RichText::new("/").small().monospace(),
        ));
        if resp_root.clicked() {
            nav = Some("/".to_string());
        }
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut acc = String::new();
        for (i, part) in parts.iter().enumerate() {
            acc.push('/');
            acc.push_str(part);
            let is_last = i + 1 == parts.len();
            let resp = ui.add(egui::Link::new(
                RichText::new(*part).small().monospace(),
            ));
            if resp.clicked() && !is_last {
                nav = Some(acc.clone());
            }
            if !is_last {
                ui.label(RichText::new("/").small().monospace().color(Color32::GRAY));
            }
        }
    });
    if let Some(p) = nav {
        tab.remote_loading = true;
        tab.remote_path_dirty = false;
        let _ = tab.handle.commands.send(SftpCommand::ListDir { path: p });
    }
}

fn render_remote_path_field(ui: &mut Ui, tab: &mut SftpTab) {
    if !tab.remote_path_dirty && tab.remote_path_buffer != tab.remote_cwd {
        tab.remote_path_buffer = tab.remote_cwd.clone();
    }
    let resp = ui.add(
        egui::TextEdit::singleline(&mut tab.remote_path_buffer)
            .desired_width(f32::INFINITY)
            .hint_text("remote path"),
    );
    if resp.changed() {
        tab.remote_path_dirty = true;
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        let p = tab.remote_path_buffer.trim().to_string();
        if !p.is_empty() {
            tab.remote_loading = true;
            let _ = tab.handle.commands.send(SftpCommand::Realpath { path: p });
        }
        tab.remote_path_dirty = false;
    }
}

fn remote_entry_context_menu(resp: &egui::Response, tab: &mut SftpTab, entry: &SftpEntry) {
    resp.context_menu(|ui| {
        let selection: Vec<String> = if tab.remote_selected.contains(&entry.name) {
            tab.remote_selected.iter().cloned().collect()
        } else {
            vec![entry.name.clone()]
        };
        let n = selection.len();
        let entries_snapshot = tab.remote_entries.clone();

        if n == 1 && entry.is_dir {
            if ui.button("Open").clicked() {
                let path = join_remote(&tab.remote_cwd, &entry.name);
                tab.remote_loading = true;
                tab.remote_path_dirty = false;
                let _ = tab.handle.commands.send(SftpCommand::ListDir { path });
                ui.close();
            }
        }
        let dl_label = if n > 1 {
            format!("Download {n} items << {}", tab.local_cwd.display())
        } else if entry.is_dir {
            format!("Download folder << {}", tab.local_cwd.display())
        } else {
            format!("Download << {}", tab.local_cwd.display())
        };
        if ui.button(dl_label).clicked() {
            for name in &selection {
                let remote = join_remote(&tab.remote_cwd, name);
                let local = tab.local_cwd.join(name);
                let id = Uuid::new_v4();
                let _ = tab.handle.commands.send(SftpCommand::Download {
                    id,
                    remote,
                    local,
                });
            }
            ui.close();
        }
        ui.separator();
        ui.add_enabled_ui(n == 1, |ui| {
            if ui.button("Rename").clicked() {
                tab.rename_buffer = entry.name.clone();
                tab.rename_target = Some(entry.name.clone());
                tab.rename_target_pane = Some(Pane::Remote);
                ui.close();
            }
        });
        let del_label = if n > 1 { format!("Delete {n} items") } else { "Delete".to_string() };
        if ui.button(del_label).clicked() {
            for name in &selection {
                let path = join_remote(&tab.remote_cwd, name);
                let is_dir = entries_snapshot
                    .iter()
                    .find(|e| e.name == *name)
                    .map(|e| e.is_dir)
                    .unwrap_or(false);
                let cmd = if is_dir {
                    SftpCommand::Rmdir { path }
                } else {
                    SftpCommand::Remove { path }
                };
                let _ = tab.handle.commands.send(cmd);
            }
            tab.remote_selected.clear();
            tab.remote_anchor = None;
            ui.close();
        }
    });
}

fn pane_empty_context_menu(resp: &egui::Response, tab: &mut SftpTab, pane: Pane) {
    resp.context_menu(|ui| {
        if ui.button("New folder...").clicked() {
            tab.mkdir_buffer.clear();
            tab.mkdir_dialog = Some(pane);
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            match pane {
                Pane::Local => {
                    tab.local_selected.clear();
                    tab.local_anchor = None;
                }
                Pane::Remote => {
                    tab.remote_loading = true;
                    let _ = tab
                        .handle
                        .commands
                        .send(SftpCommand::ListDir { path: tab.remote_cwd.clone() });
                }
            }
            ui.close();
        }
        if ui.button("Up").clicked() {
            match pane {
                Pane::Local => {
                    if let Some(parent) = tab.local_cwd.parent() {
                        tab.local_cwd = parent.to_path_buf();
                        tab.local_selected.clear();
                        tab.local_anchor = None;
                        tab.local_path_dirty = false;
                        tab.local_path_buffer = tab.local_cwd.display().to_string();
                    }
                }
                Pane::Remote => {
                    let parent = parent_remote(&tab.remote_cwd);
                    tab.remote_loading = true;
                    tab.remote_path_dirty = false;
                    let _ = tab.handle.commands.send(SftpCommand::ListDir { path: parent });
                }
            }
            ui.close();
        }
    });
}

fn render_mkdir_dialog(ui: &mut Ui, tab: &mut SftpTab, pane: Pane) {
    if tab.mkdir_dialog != Some(pane) {
        return;
    }
    let mut close = false;
    let mut do_create = false;
    egui::Window::new(match pane {
        Pane::Local => "New local folder",
        Pane::Remote => "New remote folder",
    })
    .id(egui::Id::new(("sftp_mkdir", tab.id, pane)))
    .collapsible(false)
    .resizable(false)
    .show(ui.ctx(), |ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut tab.mkdir_buffer)
                .hint_text("folder name")
                .desired_width(240.0),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            do_create = true;
        }
        ui.horizontal(|ui| {
            if ui.button("Create").clicked() {
                do_create = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if do_create {
        let name = tab.mkdir_buffer.trim().to_string();
        if !name.is_empty() {
            match pane {
                Pane::Local => {
                    let p = tab.local_cwd.join(&name);
                    match std::fs::create_dir(&p) {
                        Ok(_) => tab.last_message = Some(format!("mkdir {}", p.display())),
                        Err(e) => {
                            tab.last_message = Some(format!("error: mkdir {}: {e}", p.display()))
                        }
                    }
                }
                Pane::Remote => {
                    let path = join_remote(&tab.remote_cwd, &name);
                    let _ = tab.handle.commands.send(SftpCommand::Mkdir { path });
                }
            }
        }
        tab.mkdir_buffer.clear();
        close = true;
    }
    if close {
        tab.mkdir_dialog = None;
    }
}

fn render_rename_dialog(ui: &mut Ui, tab: &mut SftpTab, pane: Pane) {
    if tab.rename_target_pane != Some(pane) || tab.rename_target.is_none() {
        return;
    }
    let target = tab.rename_target.clone().unwrap();
    let mut close = false;
    let mut do_apply = false;
    egui::Window::new(format!("Rename {target}"))
        .id(egui::Id::new(("sftp_rename", tab.id, pane)))
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            let resp = ui.add(
                egui::TextEdit::singleline(&mut tab.rename_buffer)
                    .hint_text("new name")
                    .desired_width(240.0),
            );
            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                do_apply = true;
            }
            ui.horizontal(|ui| {
                if ui.button("Rename").clicked() {
                    do_apply = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if do_apply {
        let new_name = tab.rename_buffer.trim().to_string();
        if !new_name.is_empty() && new_name != target {
            match pane {
                Pane::Local => {
                    let from = tab.local_cwd.join(&target);
                    let to = tab.local_cwd.join(&new_name);
                    match std::fs::rename(&from, &to) {
                        Ok(_) => {
                            tab.last_message =
                                Some(format!("mv {} -> {}", from.display(), to.display()));
                            tab.local_selected.clear();
                            tab.local_selected.insert(to.clone());
                            tab.local_anchor = Some(to);
                        }
                        Err(e) => {
                            tab.last_message =
                                Some(format!("error: mv {}: {e}", from.display()))
                        }
                    }
                }
                Pane::Remote => {
                    let from = join_remote(&tab.remote_cwd, &target);
                    let to = join_remote(&tab.remote_cwd, &new_name);
                    let _ = tab.handle.commands.send(SftpCommand::Rename { from, to });
                }
            }
        }
        close = true;
    }
    if close {
        tab.rename_target = None;
        tab.rename_target_pane = None;
        tab.rename_buffer.clear();
    }
}

struct LocalEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
}

fn list_local(path: &PathBuf) -> Vec<LocalEntry> {
    let Ok(rd) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut out: Vec<LocalEntry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some(LocalEntry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir: meta.is_dir(),
                size: meta.len(),
                modified: meta.modified().ok(),
            })
        })
        .collect();
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
    out
}

fn render_transfer_strip(ui: &mut Ui, tab: &mut SftpTab) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Transfers").strong().small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Clear finished").clicked() {
                let keep: Vec<Uuid> = tab
                    .transfer_order
                    .iter()
                    .copied()
                    .filter(|id| {
                        tab.transfers
                            .get(id)
                            .map(|t| !t.done)
                            .unwrap_or(false)
                    })
                    .collect();
                tab.transfers.retain(|id, t| !t.done || keep.contains(id));
                tab.transfer_order = keep;
            }
        });
    });
    ui.separator();
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if tab.transfer_order.is_empty() {
            ui.label(RichText::new("no transfers").small().italics());
            return;
        }
        for id in tab.transfer_order.clone() {
            let Some(t) = tab.transfers.get(&id) else { continue };
            let dir = match t.direction {
                TransferDirection::Upload => "UP  ",
                TransferDirection::Download => "DOWN",
            };
            let progress = match t.total {
                Some(total) if total > 0 => (t.bytes as f32 / total as f32).clamp(0.0, 1.0),
                _ if t.done => 1.0,
                _ => 0.0,
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(dir).monospace().small());
                ui.label(RichText::new(&t.label).small());
                if !t.done && t.error.is_none() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Cancel").clicked() {
                            let _ = tab
                                .handle
                                .commands
                                .send(SftpCommand::CancelTransfer { id });
                        }
                    });
                }
            });
            let bar = egui::ProgressBar::new(progress).show_percentage().desired_height(12.0);
            ui.add(bar);
            let bytes_str = format!(
                "{} / {}",
                format_size(t.bytes),
                t.total.map(format_size).unwrap_or_else(|| "?".into())
            );
            let status = if let Some(err) = &t.error {
                format!("failed: {err}")
            } else if t.done {
                "done".into()
            } else {
                "in progress".into()
            };
            ui.label(
                RichText::new(format!("{bytes_str} - {status}"))
                    .small()
                    .color(if t.error.is_some() {
                        Color32::from_rgb(220, 110, 110)
                    } else {
                        Color32::GRAY
                    }),
            );
            ui.separator();
        }
    });
}

fn handle_dropped_files(ui: &mut Ui, tab: &mut SftpTab) {
    let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
    if dropped.is_empty() {
        return;
    }
    for f in dropped {
        let Some(path) = f.path else { continue };
        if !path.exists() {
            continue;
        }
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let remote = join_remote(&tab.remote_cwd, &name);
        let id = Uuid::new_v4();
        let _ = tab.handle.commands.send(SftpCommand::Upload {
            id,
            local: path,
            remote,
        });
    }
}

fn render_filter_row(ui: &mut Ui, buf: &mut String, id_src: impl std::hash::Hash) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("\u{1F50D}").small());
        let resp = ui.add(
            egui::TextEdit::singleline(buf)
                .id(egui::Id::new(id_src))
                .desired_width(f32::INFINITY)
                .hint_text("filter"),
        );
        let _ = resp;
        if !buf.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("x").clicked() {
                    buf.clear();
                }
            });
        }
    });
}

fn sort_header(ui: &mut Ui, label: &str, key: SortKey, current: (SortKey, SortDir)) -> egui::Response {
    let arrow = if current.0 == key {
        match current.1 {
            SortDir::Asc => " \u{25B2}",
            SortDir::Desc => " \u{25BC}",
        }
    } else {
        ""
    };
    let text = RichText::new(format!("{label}{arrow}")).strong().small();
    ui.add(egui::Label::new(text).sense(egui::Sense::click()))
}

fn toggle_sort(state: &mut (SortKey, SortDir), key: SortKey) {
    if state.0 == key {
        state.1 = match state.1 {
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Asc,
        };
    } else {
        *state = (key, SortDir::Asc);
    }
}

fn sorted_filtered_local(
    src: &[LocalEntry],
    filter: &str,
    sort: (SortKey, SortDir),
) -> Vec<LocalEntry> {
    let needle = filter.to_lowercase();
    let mut out: Vec<LocalEntry> = src
        .iter()
        .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
        .map(|e| LocalEntry {
            name: e.name.clone(),
            path: e.path.clone(),
            is_dir: e.is_dir,
            size: e.size,
            modified: e.modified,
        })
        .collect();
    out.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        let ord = match sort.0 {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
        };
        match sort.1 {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
    out
}

fn sorted_filtered_remote(
    src: &[SftpEntry],
    filter: &str,
    sort: (SortKey, SortDir),
) -> Vec<SftpEntry> {
    let needle = filter.to_lowercase();
    let mut out: Vec<SftpEntry> = src
        .iter()
        .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    out.sort_by(|a, b| {
        match (a.is_dir, b.is_dir) {
            (true, false) => return std::cmp::Ordering::Less,
            (false, true) => return std::cmp::Ordering::Greater,
            _ => {}
        }
        let ord = match sort.0 {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
        };
        match sort.1 {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
    out
}

fn join_remote(cwd: &str, name: &str) -> String {
    if cwd.ends_with('/') {
        format!("{cwd}{name}")
    } else {
        format!("{cwd}/{name}")
    }
}

fn update_local_selection(
    tab: &mut SftpTab,
    entries: &[LocalEntry],
    clicked_idx: usize,
    mods: egui::Modifiers,
) {
    let clicked = &entries[clicked_idx];
    if mods.shift_only() && tab.local_anchor.is_some() {
        let anchor = tab.local_anchor.clone().unwrap();
        if let Some(a_idx) = entries.iter().position(|e| e.path == anchor) {
            let (lo, hi) = if a_idx <= clicked_idx {
                (a_idx, clicked_idx)
            } else {
                (clicked_idx, a_idx)
            };
            tab.local_selected.clear();
            for e in &entries[lo..=hi] {
                tab.local_selected.insert(e.path.clone());
            }
            return;
        }
    }
    if mods.command || mods.ctrl {
        if !tab.local_selected.insert(clicked.path.clone()) {
            tab.local_selected.remove(&clicked.path);
        }
        tab.local_anchor = Some(clicked.path.clone());
        return;
    }
    tab.local_selected.clear();
    tab.local_selected.insert(clicked.path.clone());
    tab.local_anchor = Some(clicked.path.clone());
}

fn update_remote_selection(
    tab: &mut SftpTab,
    entries: &[SftpEntry],
    clicked_idx: usize,
    mods: egui::Modifiers,
) {
    let clicked = &entries[clicked_idx];
    if mods.shift_only() && tab.remote_anchor.is_some() {
        let anchor = tab.remote_anchor.clone().unwrap();
        if let Some(a_idx) = entries.iter().position(|e| e.name == anchor) {
            let (lo, hi) = if a_idx <= clicked_idx {
                (a_idx, clicked_idx)
            } else {
                (clicked_idx, a_idx)
            };
            tab.remote_selected.clear();
            for e in &entries[lo..=hi] {
                tab.remote_selected.insert(e.name.clone());
            }
            return;
        }
    }
    if mods.command || mods.ctrl {
        if !tab.remote_selected.insert(clicked.name.clone()) {
            tab.remote_selected.remove(&clicked.name);
        }
        tab.remote_anchor = Some(clicked.name.clone());
        return;
    }
    tab.remote_selected.clear();
    tab.remote_selected.insert(clicked.name.clone());
    tab.remote_anchor = Some(clicked.name.clone());
}

fn parent_remote(path: &str) -> String {
    if path == "/" {
        return "/".into();
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rsplit_once('/') {
        Some(("", _)) => "/".into(),
        Some((parent, _)) => parent.into(),
        None => "/".into(),
    }
}

fn format_size(n: u64) -> String {
    const UNITS: &[&str] = &["B", "K", "M", "G", "T"];
    let mut v = n as f64;
    let mut idx = 0;
    while v >= 1024.0 && idx < UNITS.len() - 1 {
        v /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[idx])
    }
}

fn format_mtime(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return "-".into() };
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => {
            let secs = d.as_secs() as i64;
            format_unix_secs(secs)
        }
        Err(_) => "-".into(),
    }
}

fn format_unix_secs(secs: i64) -> String {
    let days = secs / 86400;
    let mut z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    z = secs % 86400;
    if z < 0 {
        z += 86400;
    }
    let hh = z / 3600;
    let mm = (z % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}
