use egui::{
    Align, Area, Color32, Context, CornerRadius, FontId, Frame, Id, Key, Layout, Order, RichText,
    ScrollArea, Sense, Stroke, TextEdit, Ui, Vec2,
};
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    NewConnection,
    OpenConnection { id: Uuid },
    OpenSftp { id: Uuid },
    EditConnection { id: Uuid },
    SwitchTab { id: Uuid },
    CloseActiveTab,
    ToggleSidebar,
    LockSecrets,
    Quit,
}

#[derive(Debug, Clone)]
pub struct CommandItem {
    pub command: Command,
    pub label: String,
    pub detail: String,
    pub hint: String,
}

#[derive(Default)]
pub struct CommandPalette {
    pub open: bool,
    query: String,
    selected: usize,
    just_opened: bool,
}

pub enum PaletteResult {
    None,
    Execute(Command),
}

impl CommandPalette {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.selected = 0;
            self.just_opened = true;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn show(&mut self, ctx: &Context, items: &[CommandItem]) -> PaletteResult {
        if !self.open {
            return PaletteResult::None;
        }

        let filtered = self.filter(items);

        if self.selected >= filtered.len() {
            self.selected = filtered.len().saturating_sub(1);
        }

        #[allow(deprecated)]
        let screen = ctx.input(|i| i.screen_rect());
        let palette_width = (screen.width() * 0.55).clamp(420.0, 720.0);
        let palette_pos = egui::pos2(
            screen.center().x - palette_width * 0.5,
            screen.top() + (screen.height() * 0.18).max(60.0),
        );

        let mut result = PaletteResult::None;
        let mut close_requested = false;

        Area::new(Id::new("command_palette_scrim"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let painter = ui.painter();
                painter.rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(160));
                let rsp = ui.allocate_rect(screen, Sense::click());
                if rsp.clicked() {
                    close_requested = true;
                }
            });

        Area::new(Id::new("command_palette"))
            .order(Order::Foreground)
            .fixed_pos(palette_pos)
            .show(ctx, |ui| {
                let visuals = ui.visuals();
                let bg = visuals.window_fill;
                let stroke = visuals.window_stroke;
                Frame::group(ui.style())
                    .fill(bg)
                    .stroke(stroke)
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(10.0)
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 8],
                        blur: 24,
                        spread: 0,
                        color: Color32::from_black_alpha(96),
                    })
                    .show(ui, |ui| {
                        ui.set_width(palette_width);

                        let edit = TextEdit::singleline(&mut self.query)
                            .hint_text("Type a command or connection…")
                            .font(FontId::proportional(16.0))
                            .desired_width(palette_width);
                        let resp = ui.add(edit);
                        if self.just_opened {
                            resp.request_focus();
                            self.just_opened = false;
                        } else if !resp.has_focus() {
                            resp.request_focus();
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);

                        let clicked = render_list(ui, &filtered, &mut self.selected);
                        if let Some(idx) = clicked
                            && let Some(item) = filtered.get(idx)
                        {
                            result = PaletteResult::Execute(item.command.clone());
                        }
                    });
            });

        let input = ctx.input(|i| i.clone());

        if input.key_pressed(Key::Escape) {
            close_requested = true;
        }

        if !filtered.is_empty() {
            if input.key_pressed(Key::ArrowDown) {
                self.selected = (self.selected + 1).min(filtered.len() - 1);
            }
            if input.key_pressed(Key::ArrowUp) {
                self.selected = self.selected.saturating_sub(1);
            }
            if input.key_pressed(Key::Enter)
                && let Some(item) = filtered.get(self.selected)
            {
                result = PaletteResult::Execute(item.command.clone());
            }
        }

        if matches!(result, PaletteResult::Execute(_)) || close_requested {
            self.open = false;
        }

        ctx.request_repaint();
        result
    }

    fn filter<'a>(&self, items: &'a [CommandItem]) -> Vec<&'a CommandItem> {
        let q = self.query.trim();
        if q.is_empty() {
            return items.iter().collect();
        }
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(u32, &'a CommandItem)> = Vec::with_capacity(items.len());
        let mut haystack_buf = Vec::new();
        for item in items {
            let combined = format!("{} {} {}", item.label, item.detail, item.hint);
            haystack_buf.clear();
            let hay = Utf32Str::new(&combined, &mut haystack_buf);
            if let Some(score) = pattern.score(hay, &mut matcher) {
                scored.push((score, item));
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, it)| it).collect()
    }
}

fn render_list(ui: &mut Ui, items: &[&CommandItem], selected: &mut usize) -> Option<usize> {
    let mut clicked: Option<usize> = None;
    let row_height = 44.0;
    let max_visible = 9usize;
    let max_height = row_height * max_visible as f32;

    ScrollArea::vertical()
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if items.is_empty() {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("No matches").weak());
                });
                ui.add_space(12.0);
                return;
            }
            for (idx, item) in items.iter().enumerate() {
                let is_selected = idx == *selected;
                let resp = render_row(ui, item, is_selected);
                if resp.clicked() {
                    clicked = Some(idx);
                    *selected = idx;
                }
                if resp.hovered() {
                    *selected = idx;
                }
                if is_selected {
                    resp.scroll_to_me(Some(Align::Center));
                }
            }
        });

    clicked
}

fn render_row(ui: &mut Ui, item: &CommandItem, selected: bool) -> egui::Response {
    let visuals = ui.visuals();
    let bg = if selected {
        visuals.selection.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    let text_color = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };

    let frame = Frame::NONE
        .fill(bg)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(10, 8));

    let inner = frame.show(ui, |ui| {
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(ui.available_width());
            ui.vertical(|ui| {
                ui.label(RichText::new(&item.label).color(text_color).size(14.0).strong());
                if !item.detail.is_empty() {
                    ui.label(
                        RichText::new(&item.detail)
                            .color(text_color.linear_multiply(0.7))
                            .size(11.0),
                    );
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !item.hint.is_empty() {
                    let hint_color = text_color.linear_multiply(0.55);
                    ui.add_space(4.0);
                    let resp = ui.allocate_exact_size(Vec2::new(0.0, 0.0), Sense::hover());
                    let _ = resp;
                    Frame::NONE
                        .fill(Color32::TRANSPARENT)
                        .stroke(Stroke::new(1.0, hint_color))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(egui::Margin::symmetric(6, 2))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(&item.hint)
                                    .color(hint_color)
                                    .size(11.0)
                                    .monospace(),
                            );
                        });
                }
            });
        });
    });
    ui.interact(inner.response.rect, inner.response.id, Sense::click())
}
