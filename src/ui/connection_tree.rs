use egui::{
    CollapsingHeader, FontId, Frame, Id, LayerId, Order, PointerButton, ScrollArea, Sense, Stroke,
    Ui, UiBuilder,
};
use uuid::Uuid;

use crate::core::connection::{ConnectionStore, Protocol};

const DRAG_THRESHOLD: f32 = 4.0;

pub struct ConnectionTree<'a> {
    pub store: &'a ConnectionStore,
}

#[derive(Debug, Clone)]
pub struct ReorderRequest {
    pub dragged: Uuid,
    /// `Some(id)` = insert before this connection. `None` = append to the end
    /// of `target_group` (used for end-of-group drops, including empty groups).
    pub target: Option<Uuid>,
    pub target_group: String,
}

#[derive(Default)]
pub struct TreeAction {
    pub open: Option<Uuid>,
    pub open_sftp: Option<Uuid>,
    pub edit: Option<Uuid>,
    pub duplicate: Option<Uuid>,
    pub delete: Option<Uuid>,
    pub new_connection: bool,
    pub open_settings: bool,
    pub reorder: Option<ReorderRequest>,
}

impl<'a> ConnectionTree<'a> {
    pub fn show(self, ui: &mut Ui) -> TreeAction {
        let mut action = TreeAction::default();
        ui.horizontal(|ui| {
            ui.heading("Connections");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("+")
                    .on_hover_text("New connection")
                    .clicked()
                {
                    action.new_connection = true;
                }
            });
        });
        ui.separator();
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if ui
                .selectable_label(false, "⚙ Settings")
                .on_hover_text("Theme, recordings, and preferences")
                .clicked()
            {
                action.open_settings = true;
            }
            ui.separator();
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    let mut group_order: Vec<String> = Vec::new();
                    let mut groups: std::collections::HashMap<String, Vec<&_>> =
                        std::collections::HashMap::new();
                    for c in &self.store.connections {
                        let key = c.group.clone().unwrap_or_else(|| "Default".to_string());
                        if !groups.contains_key(&key) {
                            group_order.push(key.clone());
                        }
                        groups.entry(key).or_default().push(c);
                    }
                    if group_order.is_empty() {
                        ui.weak("No saved connections.");
                        ui.weak("Click ＋ above to add one.");
                    }
                    for group in group_order {
                        let items = groups.remove(&group).unwrap_or_default();
                        let group_clone = group.clone();
                        CollapsingHeader::new(&group)
                            .default_open(true)
                            .show(ui, |ui| {
                                draw_group(ui, &group_clone, &items, &mut action);
                            });
                    }
                });
            });
        });
        action
    }
}

fn draw_group(
    ui: &mut Ui,
    group: &str,
    items: &[&crate::core::connection::Connection],
    action: &mut TreeAction,
) {
    let dragged_payload: Option<Uuid> = ui.ctx().memory(|m| m.data.get_temp(Id::new("conn-dragging")));

    for c in items {
        let drop_response = drop_zone_thin(ui);
        if let Some(dragged) = dragged_payload {
            if dragged != c.id
                && drop_response.contains_pointer
                && ui.input(|i| i.pointer.any_released())
            {
                action.reorder = Some(ReorderRequest {
                    dragged,
                    target: Some(c.id),
                    target_group: group.to_string(),
                });
            }
        }

        draw_row(ui, c, action, dragged_payload);
    }

    let tail = drop_zone_thin(ui);
    if let Some(dragged) = dragged_payload {
        if tail.contains_pointer && ui.input(|i| i.pointer.any_released()) {
            let already_tail = items.last().map(|c| c.id) == Some(dragged);
            if !already_tail {
                action.reorder = Some(ReorderRequest {
                    dragged,
                    target: None,
                    target_group: group.to_string(),
                });
            }
        }
    }

    if dragged_payload.is_some() && ui.input(|i| i.pointer.any_released()) {
        ui.ctx().memory_mut(|m| {
            m.data.remove::<Uuid>(Id::new("conn-dragging"));
            m.data.remove::<egui::Pos2>(Id::new("conn-drag-press"));
        });
    }
}

struct DropZoneResponse {
    contains_pointer: bool,
}

fn drop_zone_thin(ui: &mut Ui) -> DropZoneResponse {
    let height = 4.0;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    let dragging_anything = ui.ctx().dragged_id().is_some();
    if response.contains_pointer() && dragging_anything {
        let painter = ui.painter_at(rect);
        let y = rect.center().y;
        let accent = ui.visuals().selection.bg_fill;
        painter.line_segment(
            [
                egui::pos2(rect.left() + 4.0, y),
                egui::pos2(rect.right() - 4.0, y),
            ],
            Stroke::new(2.0, accent),
        );
    }
    DropZoneResponse {
        contains_pointer: response.contains_pointer(),
    }
}

fn draw_row(
    ui: &mut Ui,
    c: &crate::core::connection::Connection,
    action: &mut TreeAction,
    dragged_payload: Option<Uuid>,
) {
    let is_self_dragging = dragged_payload == Some(c.id);

    let frame = Frame::new()
        .inner_margin(egui::Margin {
            left: 4,
            right: 4,
            top: 2,
            bottom: 2,
        })
        .corner_radius(4.0);

    if is_self_dragging {
        let layer_id = LayerId::new(Order::Tooltip, Id::new(("conn-drag-layer", c.id)));
        let inner = ui.scope_builder(UiBuilder::new().layer_id(layer_id), |ui| {
            paint_row_body(ui, c, frame, true);
        });
        if let Some(pointer_pos) = ui.ctx().pointer_interact_pos() {
            let delta = pointer_pos - inner.response.rect.center();
            ui.ctx()
                .transform_layer_shapes(layer_id, egui::emath::TSTransform::from_translation(delta));
        }
        let _placeholder = paint_row_body(ui, c, frame, false);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        return;
    }

    let row_rect = paint_row_body(ui, c, frame, false);

    let interact = ui.interact(
        row_rect,
        Id::new(("conn-row-i", c.id)),
        Sense::click_and_drag(),
    );

    if interact.double_clicked() {
        action.open = Some(c.id);
    }

    let primary_down = ui.input(|i| i.pointer.button_down(PointerButton::Primary));
    let press_key = Id::new("conn-drag-press");
    let dragging_key = Id::new("conn-dragging");

    if interact.drag_started_by(PointerButton::Primary) {
        if let Some(pos) = ui.input(|i| i.pointer.press_origin()) {
            ui.ctx().memory_mut(|m| m.data.insert_temp(press_key, pos));
        }
    }

    if dragged_payload.is_none() && primary_down {
        let press_origin: Option<egui::Pos2> = ui.ctx().memory(|m| m.data.get_temp(press_key));
        if let (Some(origin), Some(current)) = (press_origin, ui.input(|i| i.pointer.interact_pos()))
            && interact.is_pointer_button_down_on()
            && (current - origin).length() >= DRAG_THRESHOLD
        {
            ui.ctx()
                .memory_mut(|m| m.data.insert_temp(dragging_key, c.id));
        }
    }

    if interact.hovered() && dragged_payload.is_none() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }

    interact
        .on_hover_text(format!("{} {}", c.protocol.label(), c.display_address()))
        .context_menu(|ui| {
            if ui.button("Open").clicked() {
                action.open = Some(c.id);
                ui.close();
            }
            if matches!(c.protocol, Protocol::Ssh | Protocol::Sftp)
                && ui.button("Open SFTP").clicked()
            {
                action.open_sftp = Some(c.id);
                ui.close();
            }
            if ui.button("Edit").clicked() {
                action.edit = Some(c.id);
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                action.duplicate = Some(c.id);
                ui.close();
            }
            ui.separator();
            if ui.button("Delete").clicked() {
                action.delete = Some(c.id);
                ui.close();
            }
        });
}

fn paint_row_body(
    ui: &mut Ui,
    c: &crate::core::connection::Connection,
    frame: Frame,
    floating: bool,
) -> egui::Rect {
    let mut prepared = frame.begin(ui);
    {
        let content_ui = &mut prepared.content_ui;
        content_ui.set_min_width(content_ui.available_width());

        let row_width = content_ui.available_width();

        content_ui.vertical(|ui| {
            ui.set_max_width(row_width);

            let mut name_text = egui::RichText::new(&c.name);
            if floating {
                name_text = name_text.strong();
            }
            ui.add(egui::Label::new(name_text).truncate().selectable(false));

            let subtitle_color = ui.visuals().weak_text_color();
            let subtitle_text = format!("{}  ·  {}", c.protocol.label(), c.display_address());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(subtitle_text)
                        .color(subtitle_color)
                        .font(FontId::proportional(10.5)),
                )
                .truncate()
                .selectable(false),
            );
        });
    }

    let content_rect = prepared.content_ui.min_rect();
    let response = ui.allocate_rect(content_rect, Sense::hover());

    if floating {
        let bg = ui.visuals().widgets.active.weak_bg_fill;
        ui.painter().rect_filled(response.rect, 4.0, bg);
    } else if response.hovered() {
        let bg = ui.visuals().widgets.hovered.weak_bg_fill;
        ui.painter().rect_filled(response.rect, 4.0, bg);
    }
    prepared.paint(ui);

    response.rect
}
