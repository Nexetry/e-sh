use alacritty_terminal::index::Side;
use egui::{
    Area, Color32, CornerRadius, EventFilter, FontFamily, FontId, Frame, Key, Modifiers, Order,
    Pos2, Rect, Sense, Stroke, TextEdit, TextStyle, Ui, Vec2,
};

use super::TerminalEmulator;

pub struct TerminalView<'a> {
    pub emulator: &'a mut TerminalEmulator,
}

impl<'a> TerminalView<'a> {
    pub fn show(mut self, ui: &mut Ui) {
        let font_id = FontId::new(13.0, FontFamily::Monospace);
        let row_height = ui.fonts_mut(|f| f.row_height(&font_id)).max(14.0);
        let cell_width = ui.fonts_mut(|f| f.glyph_width(&font_id, 'M')).max(7.0);

        let avail = ui.available_size();
        let cols = ((avail.x / cell_width).floor() as u16).max(20);
        let rows = ((avail.y / row_height).floor() as u16).max(5);
        self.emulator.resize(cols, rows);

        if self.emulator.find.open {
            self.emulator.recompute_find_matches();
        }

        let snapshot = self.emulator.snapshot();
        let (response, painter) = ui.allocate_painter(
            Vec2::new(cols as f32 * cell_width, rows as f32 * row_height),
            Sense::click_and_drag(),
        );

        // While the terminal has focus, capture Tab (and arrow keys) so they reach the
        // remote shell instead of moving egui's keyboard focus to other widgets/buttons.
        if response.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: false,
                    },
                );
            });
        }

        let origin = response.rect.min;
        let bg_default = Color32::from_rgb(0x12, 0x12, 0x14);
        let selection_bg = Color32::from_rgb(0x33, 0x55, 0x88);
        painter.rect_filled(response.rect, 0.0, bg_default);

        for (row_idx, row) in snapshot.rows.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                let x = origin.x + col_idx as f32 * cell_width;
                let y = origin.y + row_idx as f32 * row_height;
                let cell_rect =
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_width, row_height));
                let bg = if cell.selected {
                    selection_bg
                } else {
                    Color32::from_rgb(cell.bg[0], cell.bg[1], cell.bg[2])
                };
                if bg != bg_default {
                    painter.rect_filled(cell_rect, 0.0, bg);
                }
                if cell.ch != ' ' && cell.ch != '\0' {
                    let fg = Color32::from_rgb(cell.fg[0], cell.fg[1], cell.fg[2]);
                    let glyph_font = FontId::new(
                        font_id.size,
                        if cell.bold {
                            FontFamily::Monospace
                        } else {
                            font_id.family.clone()
                        },
                    );
                    painter.text(
                        Pos2::new(x, y),
                        egui::Align2::LEFT_TOP,
                        cell.ch,
                        glyph_font,
                        fg,
                    );
                }
                if cell.underline {
                    painter.line_segment(
                        [
                            Pos2::new(x, y + row_height - 1.0),
                            Pos2::new(x + cell_width, y + row_height - 1.0),
                        ],
                        Stroke::new(1.0, Color32::from_rgb(cell.fg[0], cell.fg[1], cell.fg[2])),
                    );
                }
            }
        }

        if snapshot.cursor_visible && snapshot.display_offset == 0 {
            let focused = response.has_focus();
            let cursor_on = if focused {
                let phase = ui.ctx().input(|i| i.time) % 1.0;
                ui.ctx().request_repaint();
                phase < 0.5
            } else {
                true
            };
            if cursor_on {
                let (cy, cx) = snapshot.cursor;
                let cursor_rect = Rect::from_min_size(
                    Pos2::new(
                        origin.x + cx as f32 * cell_width,
                        origin.y + cy as f32 * row_height,
                    ),
                    Vec2::new(cell_width, row_height),
                );
                painter.rect_stroke(
                    cursor_rect,
                    0.0,
                    Stroke::new(1.5, Color32::from_rgb(0xea, 0xea, 0xea)),
                    egui::StrokeKind::Inside,
                );
            }
        }

        if self.emulator.find.open && !self.emulator.find.matches.is_empty() {
            let display_offset = self.emulator.display_offset() as i32;
            let match_bg = Color32::from_rgba_unmultiplied(255, 220, 0, 60);
            let current_bg = Color32::from_rgba_unmultiplied(255, 140, 0, 160);
            let current_idx = self.emulator.find.current;
            let matches_snapshot: Vec<_> = self.emulator.find.matches.clone();
            for (i, m) in matches_snapshot.iter().enumerate() {
                let start = m.start();
                let end = m.end();
                if start.line != end.line {
                    continue;
                }
                let viewport_row = start.line.0 + display_offset;
                if viewport_row < 0 || viewport_row >= rows as i32 {
                    continue;
                }
                let y = origin.y + viewport_row as f32 * row_height;
                let x0 = origin.x + start.column.0 as f32 * cell_width;
                let x1 = origin.x + (end.column.0 + 1) as f32 * cell_width;
                let color = if Some(i) == current_idx {
                    current_bg
                } else {
                    match_bg
                };
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + row_height)),
                    0.0,
                    color,
                );
            }
        }

        if snapshot.display_offset > 0 {
            let label = format!(
                "scrollback {} / {}",
                snapshot.display_offset, snapshot.history_size
            );
            let badge_pos = Pos2::new(response.rect.max.x - 8.0, response.rect.min.y + 4.0);
            painter.text(
                badge_pos,
                egui::Align2::RIGHT_TOP,
                label,
                FontId::new(11.0, FontFamily::Monospace),
                Color32::from_rgb(0xa0, 0xa0, 0xa0),
            );
        }

        if let Some(reason) = &self.emulator.closed {
            painter.text(
                response.rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("Session closed: {reason}"),
                TextStyle::Heading.resolve(ui.style()),
                Color32::LIGHT_RED,
            );
            return;
        }

        if response.clicked() {
            response.request_focus();
        }

        // Auto-focus the terminal whenever this tab becomes visible (first
        // appearance or after switching back to it) so keystrokes reach the
        // remote shell immediately.  We track the last frame this terminal
        // was rendered: if it wasn't rendered in the immediately preceding
        // frame the tab was hidden (switched away or first appearance) and
        // we request focus.  This avoids stealing focus from other widgets
        // (like Settings) while both are simultaneously visible in a split.
        let appeared_key = egui::Id::new(("term_appeared", self.emulator as *const _ as usize));
        let frame_nr = ui.ctx().cumulative_frame_nr();
        let last_rendered_frame = ui.data(|d| d.get_temp::<u64>(appeared_key).unwrap_or(0));
        ui.data_mut(|d| d.insert_temp(appeared_key, frame_nr));
        let was_visible_last_frame = last_rendered_frame == frame_nr.saturating_sub(1);
        if !was_visible_last_frame && !response.has_focus() && !self.emulator.find.open {
            response.request_focus();
        }

        let to_cell = |pos: Pos2| -> (i32, usize, Side) {
            let local_x = (pos.x - origin.x).max(0.0);
            let local_y = (pos.y - origin.y).max(0.0);
            let col_f = local_x / cell_width;
            let col = (col_f as usize).min((cols as usize).saturating_sub(1));
            let frac = col_f - col_f.floor();
            let side = if frac < 0.5 { Side::Left } else { Side::Right };
            let row = (local_y / row_height) as usize;
            let row = row.min((rows as usize).saturating_sub(1));
            let line = row as i32 - snapshot.display_offset as i32;
            (line, col, side)
        };

        // Capture the pointer-press position so we can use it as the
        // selection anchor.  `drag_started()` fires only after the pointer
        // has moved past a threshold, so `interact_pointer_pos()` at that
        // point may already be on the *next* character.  By recording the
        // press position we guarantee the cell the user originally clicked
        // is included in the selection.
        let press_key = egui::Id::new(("term_press_pos", self.emulator as *const _ as usize));
        let primary_pressed = ui
            .ctx()
            .input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
        if primary_pressed {
            if let Some(pos) = ui.ctx().input(|i| i.pointer.interact_pos()) {
                if response.rect.contains(pos) {
                    ui.data_mut(|d| d.insert_temp(press_key, pos));
                }
            }
        }

        if response.drag_started() {
            // Use the stored press position for the anchor, falling back to
            // the current pointer position if unavailable.
            let anchor_pos: Option<Pos2> = ui.data(|d| d.get_temp(press_key));
            let pos = anchor_pos.or(response.interact_pointer_pos());
            if let Some(pos) = pos {
                let (line, col, _side) = to_cell(pos);
                // Always anchor on the Left side so the character under the
                // cursor is included when dragging forward.
                self.emulator.begin_selection(line, col, Side::Left);
            }
        } else if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (line, col, side) = to_cell(pos);
                self.emulator.update_selection(line, col, side);
            }
        }

        let single_click = response.clicked() && !response.dragged();
        if single_click {
            let dblclick_key =
                egui::Id::new(("term_last_click", self.emulator as *const _ as usize));
            let now = ui.ctx().input(|i| i.time);
            let anchor_pos: Option<Pos2> = ui.data(|d| d.get_temp(press_key));
            let pos = anchor_pos.or(response.interact_pointer_pos());
            if let Some(pos) = pos {
                let (line, col, _side) = to_cell(pos);
                let prev: Option<(f64, i32, usize)> = ui.data(|d| d.get_temp(dblclick_key));
                let is_double = prev
                    .map(|(t, pl, pc)| (now - t) < 0.4 && pl == line && pc == col)
                    .unwrap_or(false);
                ui.data_mut(|d| d.insert_temp(dblclick_key, (now, line, col)));

                if is_double {
                    self.emulator.begin_semantic_selection(line, col);
                } else {
                    self.emulator.clear_selection();
                }
            } else {
                self.emulator.clear_selection();
            }
        }

        if response.hovered() {
            let scroll_delta = ui.ctx().input(|i| i.smooth_scroll_delta.y);
            if scroll_delta.abs() > 0.5 {
                let lines = (scroll_delta / row_height).round() as i32;
                if lines != 0 {
                    self.emulator.scroll(lines);
                }
            }
        }

        let open_sc = egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
        if response.has_focus() && ui.ctx().input_mut(|i| i.consume_shortcut(&open_sc)) {
            self.emulator.open_find();
        }

        if self.emulator.find.open {
            self.render_find_bar(ui, response.rect);
        }

        if response.has_focus() {
            let (events, mods) = ui
                .ctx()
                .input(|input| (input.events.clone(), input.modifiers));
            let mut buf: Vec<u8> = Vec::new();

            let copy_combo = (mods.command && !mods.shift && !mods.alt)
                || (mods.ctrl && mods.shift && !mods.alt && !mods.command);

            for ev in &events {
                match ev {
                    egui::Event::Copy => {
                        if let Some(text) = self.emulator.selection_text() {
                            if !text.is_empty() {
                                ui.ctx().copy_text(text);
                            }
                        }
                    }
                    egui::Event::Text(text) => {
                        if mods.command && (text == "c" || text == "C") {
                            continue;
                        }
                        buf.extend_from_slice(text.as_bytes());
                    }
                    egui::Event::Paste(text) => {
                        buf.extend_from_slice(text.as_bytes());
                    }
                    egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } => {
                        if copy_combo && matches!(key, Key::C) {
                            if let Some(text) = self.emulator.selection_text() {
                                if !text.is_empty() {
                                    ui.ctx().copy_text(text);
                                }
                            }
                            continue;
                        }
                        if matches!(key, Key::PageUp) && modifiers.shift {
                            self.emulator.scroll(rows as i32);
                            continue;
                        }
                        if matches!(key, Key::PageDown) && modifiers.shift {
                            self.emulator.scroll(-(rows as i32));
                            continue;
                        }
                        if matches!(key, Key::Home) && modifiers.shift {
                            self.emulator.scroll(i32::MAX / 2);
                            continue;
                        }
                        if matches!(key, Key::End) && modifiers.shift {
                            self.emulator.scroll_to_bottom();
                            continue;
                        }
                        if let Some(seq) = key_to_bytes(*key, *modifiers) {
                            buf.extend_from_slice(&seq);
                        }
                    }
                    _ => {}
                }
            }

            if !buf.is_empty() {
                self.emulator.scroll_to_bottom();
                self.emulator.send_input(buf);
            }
        }
    }

    fn render_find_bar(&mut self, ui: &mut Ui, term_rect: Rect) {
        let anchor = Pos2::new(term_rect.max.x - 12.0, term_rect.min.y + 8.0);
        let just_opened = std::mem::replace(&mut self.emulator.find.just_opened, false);
        let mut close_requested = false;
        let mut goto_next = false;
        let mut goto_prev = false;
        let mut edit_focused = false;

        let enter = ui.ctx().input(|i| i.key_pressed(Key::Enter));
        let shift_held = ui.ctx().input(|i| i.modifiers.shift);

        let pane_salt: usize = self.emulator as *const _ as usize;
        let area_id = egui::Id::new(("terminal-find-bar", pane_salt));
        let edit_id = egui::Id::new(("terminal-find-textedit", pane_salt));

        Area::new(area_id)
            .order(Order::Foreground)
            .fixed_pos(anchor - Vec2::new(340.0, 0.0))
            .show(ui.ctx(), |ui| {
                Frame::popup(ui.style())
                    .fill(Color32::from_rgb(0x20, 0x20, 0x24))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(0x44, 0x44, 0x48)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(6.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let edit = TextEdit::singleline(&mut self.emulator.find.query)
                                .desired_width(220.0)
                                .hint_text("Find")
                                .font(TextStyle::Monospace);
                            let resp = ui.add(edit.id(edit_id));
                            edit_focused = resp.has_focus();
                            if just_opened {
                                resp.request_focus();
                                edit_focused = true;
                                if let Some(mut state) = TextEdit::load_state(ui.ctx(), edit_id) {
                                    let end = self.emulator.find.query.len();
                                    state.cursor.set_char_range(Some(
                                        egui::text::CCursorRange::two(
                                            egui::text::CCursor::new(0),
                                            egui::text::CCursor::new(end),
                                        ),
                                    ));
                                    state.store(ui.ctx(), edit_id);
                                }
                            }
                            if resp.changed() {
                                self.emulator.recompute_find_matches();
                                self.emulator.find_scroll_to_current();
                            }
                            if resp.has_focus() && enter {
                                if shift_held {
                                    goto_prev = true;
                                } else {
                                    goto_next = true;
                                }
                            }

                            let total = self.emulator.find.matches.len();
                            let idx = self.emulator.find.current.map(|i| i + 1).unwrap_or(0);
                            let counter = if total == 0 {
                                "0/0".to_string()
                            } else {
                                format!("{}/{}", idx, total)
                            };
                            ui.label(
                                egui::RichText::new(counter)
                                    .monospace()
                                    .color(Color32::from_rgb(0xa0, 0xa0, 0xa0)),
                            );

                            if ui
                                .small_button("Prev")
                                .on_hover_text("Previous match (Shift+Enter)")
                                .clicked()
                            {
                                goto_prev = true;
                            }
                            if ui
                                .small_button("Next")
                                .on_hover_text("Next match (Enter)")
                                .clicked()
                            {
                                goto_next = true;
                            }
                            if ui.small_button("x").on_hover_text("Close (Esc)").clicked() {
                                close_requested = true;
                            }
                        });
                    });
            });

        if edit_focused {
            let g_sc_next = egui::KeyboardShortcut::new(Modifiers::COMMAND, Key::G);
            let g_sc_prev =
                egui::KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::G);
            if ui.ctx().input_mut(|i| i.consume_shortcut(&g_sc_prev)) {
                goto_prev = true;
            } else if ui.ctx().input_mut(|i| i.consume_shortcut(&g_sc_next)) {
                goto_next = true;
            }
            if ui.ctx().input(|i| i.key_pressed(Key::Escape)) {
                close_requested = true;
            }
        }

        if goto_next {
            self.emulator.find_goto(true);
        } else if goto_prev {
            self.emulator.find_goto(false);
        }
        if close_requested {
            self.emulator.close_find();
        }
    }
}

fn key_to_bytes(key: Key, mods: Modifiers) -> Option<Vec<u8>> {
    use Key::*;
    if mods.ctrl && !mods.shift && !mods.alt {
        if let Some(b) = ctrl_byte(key) {
            return Some(vec![b]);
        }
    }
    let bytes: &[u8] = match key {
        Enter => b"\r",
        Tab => b"\t",
        Backspace => b"\x7f",
        Escape => b"\x1b",
        ArrowUp => b"\x1b[A",
        ArrowDown => b"\x1b[B",
        ArrowRight => b"\x1b[C",
        ArrowLeft => b"\x1b[D",
        Home => b"\x1b[H",
        End => b"\x1b[F",
        PageUp => b"\x1b[5~",
        PageDown => b"\x1b[6~",
        Delete => b"\x1b[3~",
        Insert => b"\x1b[2~",
        F1 => b"\x1bOP",
        F2 => b"\x1bOQ",
        F3 => b"\x1bOR",
        F4 => b"\x1bOS",
        F5 => b"\x1b[15~",
        F6 => b"\x1b[17~",
        F7 => b"\x1b[18~",
        F8 => b"\x1b[19~",
        F9 => b"\x1b[20~",
        F10 => b"\x1b[21~",
        F11 => b"\x1b[23~",
        F12 => b"\x1b[24~",
        _ => return None,
    };
    Some(bytes.to_vec())
}

fn ctrl_byte(key: Key) -> Option<u8> {
    use Key::*;
    let c = match key {
        A => b'a',
        B => b'b',
        C => b'c',
        D => b'd',
        E => b'e',
        F => b'f',
        G => b'g',
        H => b'h',
        I => b'i',
        J => b'j',
        K => b'k',
        L => b'l',
        M => b'm',
        N => b'n',
        O => b'o',
        P => b'p',
        Q => b'q',
        R => b'r',
        S => b's',
        T => b't',
        U => b'u',
        V => b'v',
        W => b'w',
        X => b'x',
        Y => b'y',
        Z => b'z',
        OpenBracket => b'[',
        CloseBracket => b']',
        Backslash => b'\\',
        _ => return None,
    };
    Some(c & 0x1f)
}
