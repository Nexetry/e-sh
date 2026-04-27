//! RDP viewer tab — renders the remote desktop as an egui texture
//! and forwards mouse/keyboard input to the RDP session.

use egui::{Color32, ColorImage, RichText, TextureHandle, TextureOptions, Ui};
use uuid::Uuid;

use crate::proto::rdp::{RdpCommand, RdpEvent, RdpHandle, RdpMouseButton};

pub struct RdpTab {
    pub id: Uuid,
    pub source_connection: Option<Uuid>,
    pub title: String,
    pub connection_label: String,
    pub handle: RdpHandle,
    pub closed: Option<String>,
    pub closed_reported: bool,
    pub tab_color: Option<Color32>,

    width: u16,
    height: u16,
    connected: bool,
    /// When true, the desktop is displayed in a separate FreeRDP window
    /// and this tab only shows status information.
    external_window: bool,
    /// RGBA framebuffer, composited from bitmap regions.
    framebuffer: Vec<u8>,
    texture: Option<TextureHandle>,
    dirty: bool,
    status: String,
    /// Track modifier key state so we can send press/release on change.
    prev_modifiers: egui::Modifiers,
}

impl RdpTab {
    pub fn new(
        id: Uuid,
        source_connection: Option<Uuid>,
        title: String,
        connection_label: String,
        handle: RdpHandle,
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
            external_window: false,
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
                RdpEvent::Connected { width, height, external_window } => {
                    self.width = width;
                    self.height = height;
                    self.connected = true;
                    self.external_window = external_window;
                    if external_window {
                        self.status = "Connected (FreeRDP — desktop in separate window)".to_string();
                    } else {
                        self.framebuffer = vec![0u8; width as usize * height as usize * 4];
                        // Fill with dark gray
                        for pixel in self.framebuffer.chunks_exact_mut(4) {
                            pixel[0] = 30;
                            pixel[1] = 30;
                            pixel[2] = 30;
                            pixel[3] = 255;
                        }
                        self.dirty = true;
                        self.status = format!("Connected {}x{}", width, height);
                    }
                }
                RdpEvent::Bitmap(region) => {
                    self.blit_region(&region);
                    self.dirty = true;
                }
                RdpEvent::Closed(reason) => {
                    self.closed = Some(reason.unwrap_or_else(|| "session closed".into()));
                    self.status = "Disconnected".to_string();
                }
            }
        }
    }

    fn blit_region(&mut self, region: &crate::proto::rdp::BitmapRegion) {
        let fb_w = self.width as usize;
        let fb_h = self.height as usize;
        let rw = region.width as usize;
        let rh = region.height as usize;
        let rx = region.left as usize;
        let ry = region.top as usize;

        if self.framebuffer.len() != fb_w * fb_h * 4 {
            return;
        }

        let src_bpp = region.bpp as usize;
        let src_bytes_per_pixel = src_bpp / 8;

        for row in 0..rh {
            let dst_y = ry + row;
            if dst_y >= fb_h {
                break;
            }
            for col in 0..rw {
                let dst_x = rx + col;
                if dst_x >= fb_w {
                    break;
                }
                let src_idx = (row * rw + col) * src_bytes_per_pixel;
                let dst_idx = (dst_y * fb_w + dst_x) * 4;

                if src_idx + src_bytes_per_pixel > region.data.len() {
                    break;
                }
                if dst_idx + 4 > self.framebuffer.len() {
                    break;
                }

                match src_bpp {
                    32 => {
                        // BGRA -> RGBA
                        self.framebuffer[dst_idx] = region.data[src_idx + 2];
                        self.framebuffer[dst_idx + 1] = region.data[src_idx + 1];
                        self.framebuffer[dst_idx + 2] = region.data[src_idx];
                        self.framebuffer[dst_idx + 3] = 255;
                    }
                    24 => {
                        // BGR -> RGBA
                        self.framebuffer[dst_idx] = region.data[src_idx + 2];
                        self.framebuffer[dst_idx + 1] = region.data[src_idx + 1];
                        self.framebuffer[dst_idx + 2] = region.data[src_idx];
                        self.framebuffer[dst_idx + 3] = 255;
                    }
                    16 => {
                        let pixel = u16::from_le_bytes([
                            region.data[src_idx],
                            region.data[src_idx + 1],
                        ]);
                        // RGB565
                        let r = ((pixel >> 11) & 0x1F) as u8;
                        let g = ((pixel >> 5) & 0x3F) as u8;
                        let b = (pixel & 0x1F) as u8;
                        self.framebuffer[dst_idx] = (r << 3) | (r >> 2);
                        self.framebuffer[dst_idx + 1] = (g << 2) | (g >> 4);
                        self.framebuffer[dst_idx + 2] = (b << 3) | (b >> 2);
                        self.framebuffer[dst_idx + 3] = 255;
                    }
                    15 => {
                        let pixel = u16::from_le_bytes([
                            region.data[src_idx],
                            region.data[src_idx + 1],
                        ]);
                        // RGB555
                        let r = ((pixel >> 10) & 0x1F) as u8;
                        let g = ((pixel >> 5) & 0x1F) as u8;
                        let b = (pixel & 0x1F) as u8;
                        self.framebuffer[dst_idx] = (r << 3) | (r >> 2);
                        self.framebuffer[dst_idx + 1] = (g << 3) | (g >> 2);
                        self.framebuffer[dst_idx + 2] = (b << 3) | (b >> 2);
                        self.framebuffer[dst_idx + 3] = 255;
                    }
                    _ => {
                        // Unknown bpp, fill magenta
                        self.framebuffer[dst_idx] = 255;
                        self.framebuffer[dst_idx + 1] = 0;
                        self.framebuffer[dst_idx + 2] = 255;
                        self.framebuffer[dst_idx + 3] = 255;
                    }
                }
            }
        }
    }
}

pub fn render_rdp_tab(ui: &mut Ui, tab: &mut RdpTab) {
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
            ui.label(RichText::new("Connecting to RDP server...").italics());
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

    // FreeRDP external window mode — no embedded framebuffer
    if tab.external_window {
        ui.centered_and_justified(|ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new("🖥  Desktop is displayed in the FreeRDP window")
                        .size(16.0),
                );
                ui.add_space(8.0);
                ui.label(
                    RichText::new(
                        "This server requires the Graphics Pipeline (e.g. GNOME Remote Desktop).\n\
                         The session is running in an external FreeRDP window.\n\
                         Close that window to end the session."
                    )
                    .weak()
                    .italics(),
                );
            });
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
                    Some(ui.ctx().load_texture("rdp_fb", image, TextureOptions::NEAREST));
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

        // Forward mouse input
        //
        // egui's `clicked()` doesn't reliably fire with `click_and_drag` sense
        // because the press is consumed as a drag start. Instead we track
        // pointer button state from raw input events and send discrete
        // press / release messages to the RDP session.

        // Hover position — always send mouse moves when the pointer is over the image
        let hover_pos = response.hover_pos();
        if let Some(pos) = hover_pos {
            if img_rect.contains(pos) {
                let rel_x = ((pos.x - img_rect.min.x) / scale).clamp(0.0, fb_w - 1.0) as u16;
                let rel_y = ((pos.y - img_rect.min.y) / scale).clamp(0.0, fb_h - 1.0) as u16;
                let _ = tab.handle.commands.send(RdpCommand::MouseMove { x: rel_x, y: rel_y });
            }
        }

        // Button press / release via raw pointer events
        let pointer_events = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => Some((*pos, *button, *pressed)),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });

        for (pos, button, pressed) in pointer_events {
            if !img_rect.contains(pos) {
                continue;
            }
            let rel_x = ((pos.x - img_rect.min.x) / scale).clamp(0.0, fb_w - 1.0) as u16;
            let rel_y = ((pos.y - img_rect.min.y) / scale).clamp(0.0, fb_h - 1.0) as u16;
            let rdp_btn = match button {
                egui::PointerButton::Primary => RdpMouseButton::Left,
                egui::PointerButton::Secondary => RdpMouseButton::Right,
                egui::PointerButton::Middle => RdpMouseButton::Middle,
                _ => RdpMouseButton::None,
            };
            let _ = tab.handle.commands.send(RdpCommand::Mouse {
                x: rel_x,
                y: rel_y,
                button: rdp_btn,
                down: pressed,
            });
        }

        // Scroll wheel
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta);
        if scroll_delta.y != 0.0 {
            if let Some(pos) = response.hover_pos() {
                if img_rect.contains(pos) {
                    let rel_x = ((pos.x - img_rect.min.x) / scale).clamp(0.0, fb_w - 1.0) as u16;
                    let rel_y = ((pos.y - img_rect.min.y) / scale).clamp(0.0, fb_h - 1.0) as u16;
                    // RDP expects 120 units per notch; egui gives pixels
                    let delta = (scroll_delta.y * 1.0).clamp(-32000.0, 32000.0) as i16;
                    let _ = tab.handle.commands.send(RdpCommand::Scroll {
                        x: rel_x,
                        y: rel_y,
                        delta,
                    });
                }
            }
        }

        // Forward keyboard input — regular keys and modifier state changes
        let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));

        // Detect modifier key transitions and send press/release
        send_modifier_changes(tab, &modifiers);

        for event in &events {
            if let egui::Event::Key { key, pressed, .. } = event {
                if let Some(scancode) = egui_key_to_scancode(*key) {
                    let _ = tab.handle.commands.send(RdpCommand::Key {
                        code: scancode,
                        pressed: *pressed,
                    });
                }
            }
        }
    } else {
        ui.centered_and_justified(|ui| {
            ui.label("Waiting for first frame...");
        });
    }
}

/// Send press/release events for modifier keys that changed since last frame.
fn send_modifier_changes(tab: &mut RdpTab, current: &egui::Modifiers) {
    let prev = &tab.prev_modifiers;

    // (modifier_field, scancode)
    // Left Ctrl = 0x1D, Left Shift = 0x2A, Left Alt = 0x38, Left Super = 0xE05B
    let mappings: &[(bool, bool, u16)] = &[
        (prev.ctrl, current.ctrl, 0x1D),    // Ctrl
        (prev.shift, current.shift, 0x2A),  // Shift
        (prev.alt, current.alt, 0x38),      // Alt / Option
        (prev.mac_cmd || prev.command, current.mac_cmd || current.command, 0x5B), // Super / Cmd (extended)
    ];

    for &(was_down, is_down, scancode) in mappings {
        if !was_down && is_down {
            let _ = tab.handle.commands.send(RdpCommand::Key {
                code: scancode,
                pressed: true,
            });
        } else if was_down && !is_down {
            let _ = tab.handle.commands.send(RdpCommand::Key {
                code: scancode,
                pressed: false,
            });
        }
    }

    tab.prev_modifiers = *current;
}

fn egui_key_to_scancode(key: egui::Key) -> Option<u16> {
    use egui::Key;
    Some(match key {
        Key::Escape => 0x01, Key::Backspace => 0x0E, Key::Tab => 0x0F,
        Key::Enter => 0x1C, Key::Space => 0x39, Key::Delete => 0x53,
        Key::ArrowUp => 0x48, Key::ArrowDown => 0x50,
        Key::ArrowLeft => 0x4B, Key::ArrowRight => 0x4D,
        Key::Home => 0x47, Key::End => 0x4F,
        Key::PageUp => 0x49, Key::PageDown => 0x51, Key::Insert => 0x52,
        Key::A => 0x1E, Key::B => 0x30, Key::C => 0x2E, Key::D => 0x20,
        Key::E => 0x12, Key::F => 0x21, Key::G => 0x22, Key::H => 0x23,
        Key::I => 0x17, Key::J => 0x24, Key::K => 0x25, Key::L => 0x26,
        Key::M => 0x32, Key::N => 0x31, Key::O => 0x18, Key::P => 0x19,
        Key::Q => 0x10, Key::R => 0x13, Key::S => 0x1F, Key::T => 0x14,
        Key::U => 0x16, Key::V => 0x2F, Key::W => 0x11, Key::X => 0x2D,
        Key::Y => 0x15, Key::Z => 0x2C,
        Key::Num0 => 0x0B, Key::Num1 => 0x02, Key::Num2 => 0x03,
        Key::Num3 => 0x04, Key::Num4 => 0x05, Key::Num5 => 0x06,
        Key::Num6 => 0x07, Key::Num7 => 0x08, Key::Num8 => 0x09, Key::Num9 => 0x0A,
        Key::F1 => 0x3B, Key::F2 => 0x3C, Key::F3 => 0x3D, Key::F4 => 0x3E,
        Key::F5 => 0x3F, Key::F6 => 0x40, Key::F7 => 0x41, Key::F8 => 0x42,
        Key::F9 => 0x43, Key::F10 => 0x44, Key::F11 => 0x57, Key::F12 => 0x58,
        // Punctuation / symbols
        Key::Minus => 0x0C,
        Key::Plus => 0x0D,  // =/+ key
        Key::OpenBracket => 0x1A,
        Key::CloseBracket => 0x1B,
        Key::Backslash => 0x2B,
        Key::Semicolon => 0x27,
        Key::Quote => 0x28,  // '/\" key (apostrophe)
        Key::Comma => 0x33,
        Key::Period => 0x34,
        Key::Slash => 0x35,
        Key::Backtick => 0x29, // `/~ key
        _ => return None,
    })
}
