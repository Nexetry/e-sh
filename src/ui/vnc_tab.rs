//! VNC viewer tab — renders the remote desktop as an egui texture
//! and forwards mouse/keyboard input to the VNC session.

use egui::{Color32, ColorImage, RichText, TextureHandle, TextureOptions, Ui};
use uuid::Uuid;

use crate::proto::vnc::{VncCommand, VncEvent, VncHandle};

pub struct VncTab {
    pub id: Uuid,
    pub source_connection: Option<Uuid>,
    pub title: String,
    pub connection_label: String,
    pub handle: VncHandle,
    pub closed: Option<String>,
    pub closed_reported: bool,
    pub tab_color: Option<Color32>,

    width: u16,
    height: u16,
    connected: bool,
    framebuffer: Vec<u8>,
    texture: Option<TextureHandle>,
    dirty: bool,
    status: String,
    prev_modifiers: egui::Modifiers,
}

impl VncTab {
    pub fn new(
        id: Uuid,
        source_connection: Option<Uuid>,
        title: String,
        connection_label: String,
        handle: VncHandle,
    ) -> Self {
        Self {
            id,
            source_connection,
            title,
            connection_label,
            handle,
            closed: None,
            closed_reported: false,
            tab_color: None,
            width: 0,
            height: 0,
            connected: false,
            framebuffer: Vec::new(),
            texture: None,
            dirty: false,
            status: "Connecting...".to_string(),
            prev_modifiers: egui::Modifiers::NONE,
        }
    }

    pub fn pump(&mut self) {
        while let Ok(ev) = self.handle.events.try_recv() {
            match ev {
                VncEvent::Connected { width, height } => {
                    self.width = width;
                    self.height = height;
                    self.connected = true;
                    self.framebuffer = vec![0u8; width as usize * height as usize * 4];
                    for pixel in self.framebuffer.chunks_exact_mut(4) {
                        pixel[0] = 30;
                        pixel[1] = 30;
                        pixel[2] = 30;
                        pixel[3] = 255;
                    }
                    self.dirty = true;
                    self.status = format!("Connected {}×{}", width, height);
                }
                VncEvent::Bitmap(region) => {
                    self.blit_region(&region);
                    self.dirty = true;
                }
                VncEvent::Closed(reason) => {
                    self.closed = Some(reason.unwrap_or_else(|| "session closed".into()));
                    self.status = "Disconnected".to_string();
                }
            }
        }
    }

    fn blit_region(&mut self, region: &crate::proto::vnc::VncBitmapRegion) {
        let fb_w = self.width as usize;
        let fb_h = self.height as usize;
        let rw = region.width as usize;
        let rh = region.height as usize;
        let rx = region.x as usize;
        let ry = region.y as usize;

        if self.framebuffer.len() != fb_w * fb_h * 4 {
            return;
        }

        for row in 0..rh {
            let dst_y = ry + row;
            if dst_y >= fb_h { break; }
            for col in 0..rw {
                let dst_x = rx + col;
                if dst_x >= fb_w { break; }
                let src_idx = (row * rw + col) * 4;
                let dst_idx = (dst_y * fb_w + dst_x) * 4;
                if src_idx + 4 > region.data.len() || dst_idx + 4 > self.framebuffer.len() {
                    break;
                }
                self.framebuffer[dst_idx..dst_idx + 4]
                    .copy_from_slice(&region.data[src_idx..src_idx + 4]);
            }
        }
    }
}

pub fn render_vnc_tab(ui: &mut Ui, tab: &mut VncTab) {
    tab.pump();

    // Status bar
    ui.horizontal(|ui| {
        ui.label(RichText::new(&tab.connection_label).monospace().small());
        ui.separator();
        ui.label(RichText::new(&tab.status).small());
    });
    ui.separator();

    if !tab.connected && tab.closed.is_none() {
        ui.centered_and_justified(|ui| {
            ui.label(RichText::new("Connecting to VNC server...").italics());
        });
        return;
    }

    if let Some(reason) = &tab.closed {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(format!("Disconnected: {reason}"))
                    .color(Color32::from_rgb(220, 110, 110)),
            );
        });
        return;
    }

    // Update texture if framebuffer changed
    if tab.dirty && !tab.framebuffer.is_empty() {
        let image = ColorImage::from_rgba_unmultiplied(
            [tab.width as usize, tab.height as usize],
            &tab.framebuffer,
        );
        match &mut tab.texture {
            Some(tex) => tex.set(image, TextureOptions::NEAREST),
            None => {
                tab.texture =
                    Some(ui.ctx().load_texture("vnc_fb", image, TextureOptions::NEAREST));
            }
        }
        tab.dirty = false;
    }

    // Render the framebuffer
    if let Some(tex) = &tab.texture {
        let available = ui.available_size();
        let fb_w = tab.width as f32;
        let fb_h = tab.height as f32;

        let scale = (available.x / fb_w).min(available.y / fb_h).min(1.0);
        let display_w = fb_w * scale;
        let display_h = fb_h * scale;

        let (response, painter) = ui.allocate_painter(
            egui::vec2(available.x, available.y),
            egui::Sense::click_and_drag(),
        );

        let offset_x = (available.x - display_w) / 2.0;
        let offset_y = (available.y - display_h) / 2.0;
        let img_rect = egui::Rect::from_min_size(
            response.rect.min + egui::vec2(offset_x, offset_y),
            egui::vec2(display_w, display_h),
        );

        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );

        // --- Forward mouse input ---
        // Track current button state for the RFB button mask
        let hover_pos = response.hover_pos();

        // Button press/release via raw pointer events
        let pointer_events = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        Some((*pos, *button, *pressed))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        });

        // Build button mask from current pointer state
        let button_mask = ui.input(|i| {
            let mut mask = 0u8;
            if i.pointer.button_down(egui::PointerButton::Primary) { mask |= 1; }
            if i.pointer.button_down(egui::PointerButton::Middle) { mask |= 2; }
            if i.pointer.button_down(egui::PointerButton::Secondary) { mask |= 4; }
            mask
        });

        // Send pointer events for button presses/releases
        for (pos, button, pressed) in pointer_events {
            if !img_rect.contains(pos) { continue; }
            let rel_x = ((pos.x - img_rect.min.x) / scale).clamp(0.0, fb_w - 1.0) as u16;
            let rel_y = ((pos.y - img_rect.min.y) / scale).clamp(0.0, fb_h - 1.0) as u16;
            let btn_bit = match button {
                egui::PointerButton::Primary => 1u8,
                egui::PointerButton::Middle => 2,
                egui::PointerButton::Secondary => 4,
                _ => 0,
            };
            let mask = if pressed {
                button_mask | btn_bit
            } else {
                button_mask & !btn_bit
            };
            let _ = tab.handle.commands.send(VncCommand::Pointer {
                x: rel_x, y: rel_y, button_mask: mask,
            });
        }

        // Send mouse move on hover
        if let Some(pos) = hover_pos {
            if img_rect.contains(pos) {
                let rel_x = ((pos.x - img_rect.min.x) / scale).clamp(0.0, fb_w - 1.0) as u16;
                let rel_y = ((pos.y - img_rect.min.y) / scale).clamp(0.0, fb_h - 1.0) as u16;
                let _ = tab.handle.commands.send(VncCommand::Pointer {
                    x: rel_x, y: rel_y, button_mask,
                });
            }
        }

        // Scroll wheel — VNC uses buttons 4 (up) and 5 (down)
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta);
        if scroll_delta.y != 0.0 {
            if let Some(pos) = response.hover_pos() {
                if img_rect.contains(pos) {
                    let rel_x = ((pos.x - img_rect.min.x) / scale).clamp(0.0, fb_w - 1.0) as u16;
                    let rel_y = ((pos.y - img_rect.min.y) / scale).clamp(0.0, fb_h - 1.0) as u16;
                    let scroll_btn = if scroll_delta.y > 0.0 { 8u8 } else { 16u8 }; // button 4 / 5
                    // Press
                    let _ = tab.handle.commands.send(VncCommand::Pointer {
                        x: rel_x, y: rel_y, button_mask: button_mask | scroll_btn,
                    });
                    // Release
                    let _ = tab.handle.commands.send(VncCommand::Pointer {
                        x: rel_x, y: rel_y, button_mask,
                    });
                }
            }
        }

        // --- Forward keyboard input ---
        let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));

        send_modifier_changes(tab, &modifiers);

        for event in &events {
            match event {
                egui::Event::Text(text) => {
                    // Text events give us the actual typed character (e.g. "!" from Shift+1).
                    // Send each char as a keysym press+release.
                    for ch in text.chars() {
                        let keysym = char_to_x11_keysym(ch);
                        let _ = tab.handle.commands.send(VncCommand::Key {
                            keysym,
                            pressed: true,
                        });
                        let _ = tab.handle.commands.send(VncCommand::Key {
                            keysym,
                            pressed: false,
                        });
                    }
                }
                egui::Event::Key { key, pressed, .. } => {
                    // Non-printable / special keys (arrows, F-keys, Enter, etc.)
                    // Skip keys that produce Text events to avoid double-sending.
                    if !is_printable_key(*key) {
                        if let Some(keysym) = egui_key_to_x11_keysym(*key) {
                            let _ = tab.handle.commands.send(VncCommand::Key {
                                keysym,
                                pressed: *pressed,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    } else {
        ui.centered_and_justified(|ui| {
            ui.label("Waiting for first frame...");
        });
    }
}

/// Send press/release events for modifier keys that changed since last frame.
fn send_modifier_changes(tab: &mut VncTab, current: &egui::Modifiers) {
    let prev = &tab.prev_modifiers;

    // X11 keysyms for modifier keys
    let mappings: &[(bool, bool, u32)] = &[
        (prev.ctrl, current.ctrl, 0xFFE3),     // Control_L
        (prev.shift, current.shift, 0xFFE1),   // Shift_L
        (prev.alt, current.alt, 0xFFE9),       // Alt_L
        (prev.mac_cmd || prev.command, current.mac_cmd || current.command, 0xFFEB), // Super_L
    ];

    for &(was_down, is_down, keysym) in mappings {
        if !was_down && is_down {
            let _ = tab.handle.commands.send(VncCommand::Key { keysym, pressed: true });
        } else if was_down && !is_down {
            let _ = tab.handle.commands.send(VncCommand::Key { keysym, pressed: false });
        }
    }

    tab.prev_modifiers = *current;
}

/// Convert a Unicode character to an X11 keysym.
/// Latin-1 range (U+0020..=U+00FF) maps 1:1; above that uses the Unicode keysym
/// convention (0x0100_0000 | codepoint).
fn char_to_x11_keysym(ch: char) -> u32 {
    let cp = ch as u32;
    match cp {
        0x0020..=0x00FF => cp,
        _ => 0x0100_0000 | cp,
    }
}

/// Returns true for keys that also produce `Event::Text`, so we can avoid
/// sending them twice (once from Key, once from Text).
fn is_printable_key(key: egui::Key) -> bool {
    use egui::Key;
    matches!(
        key,
        Key::A | Key::B | Key::C | Key::D | Key::E | Key::F | Key::G | Key::H
        | Key::I | Key::J | Key::K | Key::L | Key::M | Key::N | Key::O | Key::P
        | Key::Q | Key::R | Key::S | Key::T | Key::U | Key::V | Key::W | Key::X
        | Key::Y | Key::Z
        | Key::Num0 | Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4
        | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9
        | Key::Space | Key::Minus | Key::Plus | Key::OpenBracket | Key::CloseBracket
        | Key::Backslash | Key::Semicolon | Key::Quote | Key::Comma | Key::Period
        | Key::Slash | Key::Backtick
    )
}

/// Map egui keys to X11 keysyms for VNC.
fn egui_key_to_x11_keysym(key: egui::Key) -> Option<u32> {
    use egui::Key;
    Some(match key {
        Key::Escape => 0xFF1B, Key::Backspace => 0xFF08, Key::Tab => 0xFF09,
        Key::Enter => 0xFF0D, Key::Space => 0x0020, Key::Delete => 0xFFFF,
        Key::ArrowUp => 0xFF52, Key::ArrowDown => 0xFF54,
        Key::ArrowLeft => 0xFF51, Key::ArrowRight => 0xFF53,
        Key::Home => 0xFF50, Key::End => 0xFF57,
        Key::PageUp => 0xFF55, Key::PageDown => 0xFF56, Key::Insert => 0xFF63,
        Key::A => 0x0061, Key::B => 0x0062, Key::C => 0x0063, Key::D => 0x0064,
        Key::E => 0x0065, Key::F => 0x0066, Key::G => 0x0067, Key::H => 0x0068,
        Key::I => 0x0069, Key::J => 0x006A, Key::K => 0x006B, Key::L => 0x006C,
        Key::M => 0x006D, Key::N => 0x006E, Key::O => 0x006F, Key::P => 0x0070,
        Key::Q => 0x0071, Key::R => 0x0072, Key::S => 0x0073, Key::T => 0x0074,
        Key::U => 0x0075, Key::V => 0x0076, Key::W => 0x0077, Key::X => 0x0078,
        Key::Y => 0x0079, Key::Z => 0x007A,
        Key::Num0 => 0x0030, Key::Num1 => 0x0031, Key::Num2 => 0x0032,
        Key::Num3 => 0x0033, Key::Num4 => 0x0034, Key::Num5 => 0x0035,
        Key::Num6 => 0x0036, Key::Num7 => 0x0037, Key::Num8 => 0x0038, Key::Num9 => 0x0039,
        Key::F1 => 0xFFBE, Key::F2 => 0xFFBF, Key::F3 => 0xFFC0, Key::F4 => 0xFFC1,
        Key::F5 => 0xFFC2, Key::F6 => 0xFFC3, Key::F7 => 0xFFC4, Key::F8 => 0xFFC5,
        Key::F9 => 0xFFC6, Key::F10 => 0xFFC7, Key::F11 => 0xFFC8, Key::F12 => 0xFFC9,
        Key::Minus => 0x002D,
        Key::Plus => 0x003D,
        Key::OpenBracket => 0x005B,
        Key::CloseBracket => 0x005D,
        Key::Backslash => 0x005C,
        Key::Semicolon => 0x003B,
        Key::Quote => 0x0027,
        Key::Comma => 0x002C,
        Key::Period => 0x002E,
        Key::Slash => 0x002F,
        Key::Backtick => 0x0060,
        _ => return None,
    })
}
