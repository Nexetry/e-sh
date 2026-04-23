use alacritty_terminal::index::Side;
use egui::{Color32, FontFamily, FontId, Key, Modifiers, Pos2, Rect, Sense, Stroke, TextStyle, Ui, Vec2};

use super::TerminalEmulator;

pub struct TerminalView<'a> {
    pub emulator: &'a mut TerminalEmulator,
}

impl<'a> TerminalView<'a> {
    pub fn show(self, ui: &mut Ui) {
        let font_id = FontId::new(13.0, FontFamily::Monospace);
        let row_height = ui
            .fonts_mut(|f| f.row_height(&font_id))
            .max(14.0);
        let cell_width = ui.fonts_mut(|f| f.glyph_width(&font_id, 'M')).max(7.0);

        let avail = ui.available_size();
        let cols = ((avail.x / cell_width).floor() as u16).max(20);
        let rows = ((avail.y / row_height).floor() as u16).max(5);
        self.emulator.resize(cols, rows);

        let snapshot = self.emulator.snapshot();
        let (response, painter) = ui.allocate_painter(
            Vec2::new(cols as f32 * cell_width, rows as f32 * row_height),
            Sense::click_and_drag(),
        );

        let origin = response.rect.min;
        let bg_default = Color32::from_rgb(0x12, 0x12, 0x14);
        let selection_bg = Color32::from_rgb(0x33, 0x55, 0x88);
        painter.rect_filled(response.rect, 0.0, bg_default);

        for (row_idx, row) in snapshot.rows.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                let x = origin.x + col_idx as f32 * cell_width;
                let y = origin.y + row_idx as f32 * row_height;
                let cell_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(cell_width, row_height));
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
                        if cell.bold { FontFamily::Monospace } else { font_id.family.clone() },
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
            let (cy, cx) = snapshot.cursor;
            let cursor_rect = Rect::from_min_size(
                Pos2::new(origin.x + cx as f32 * cell_width, origin.y + cy as f32 * row_height),
                Vec2::new(cell_width, row_height),
            );
            painter.rect_stroke(cursor_rect, 0.0, Stroke::new(1.5, Color32::from_rgb(0xea, 0xea, 0xea)), egui::StrokeKind::Inside);
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

        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (line, col, side) = to_cell(pos);
                self.emulator.begin_selection(line, col, side);
            }
        } else if response.dragged() {
            if let Some(pos) = response.interact_pointer_pos() {
                let (line, col, side) = to_cell(pos);
                self.emulator.update_selection(line, col, side);
            }
        }

        let single_click = response.clicked() && !response.dragged();
        if single_click {
            self.emulator.clear_selection();
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

        if response.has_focus() {
            let (events, mods) = ui.ctx().input(|input| (input.events.clone(), input.modifiers));
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
                    egui::Event::Key { key, pressed: true, modifiers, .. } => {
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
        A => b'a', B => b'b', C => b'c', D => b'd', E => b'e', F => b'f',
        G => b'g', H => b'h', I => b'i', J => b'j', K => b'k', L => b'l',
        M => b'm', N => b'n', O => b'o', P => b'p', Q => b'q', R => b'r',
        S => b's', T => b't', U => b'u', V => b'v', W => b'w', X => b'x',
        Y => b'y', Z => b'z',
        OpenBracket => b'[',
        CloseBracket => b']',
        Backslash => b'\\',
        _ => return None,
    };
    Some(c & 0x1f)
}
