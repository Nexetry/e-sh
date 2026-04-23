use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, FontId, Frame, Id, Order, RichText, Stroke, Vec2};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Warn,
    Error,
    Success,
}

pub struct Toast {
    pub kind: ToastKind,
    pub title: String,
    pub body: String,
    pub created: Instant,
    pub ttl: Duration,
}

#[derive(Default)]
pub struct Toaster {
    toasts: Vec<Toast>,
}

impl Toaster {
    pub fn push(&mut self, kind: ToastKind, title: impl Into<String>, body: impl Into<String>) {
        let ttl = match kind {
            ToastKind::Error => Duration::from_secs(8),
            ToastKind::Warn => Duration::from_secs(6),
            ToastKind::Success => Duration::from_secs(3),
            ToastKind::Info => Duration::from_secs(4),
        };
        self.toasts.push(Toast {
            kind,
            title: title.into(),
            body: body.into(),
            created: Instant::now(),
            ttl,
        });
    }

    pub fn info(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Info, title, body);
    }
    pub fn success(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Success, title, body);
    }
    pub fn warn(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Warn, title, body);
    }
    pub fn error(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.push(ToastKind::Error, title, body);
    }

    pub fn show(&mut self, ctx: &Context) {
        let now = Instant::now();
        self.toasts
            .retain(|t| now.duration_since(t.created) < t.ttl);

        if self.toasts.is_empty() {
            return;
        }

        let margin = 12.0;
        let toast_width = 340.0;
        let mut y_offset = margin;
        let mut dismiss: Option<usize> = None;

        for (idx, toast) in self.toasts.iter().enumerate() {
            let area_id = Id::new(("toast", idx, toast.created));
            let response = egui::Area::new(area_id)
                .order(Order::Foreground)
                .anchor(
                    Align2::RIGHT_TOP,
                    Vec2::new(-margin, y_offset),
                )
                .interactable(true)
                .show(ctx, |ui| {
                    let (accent, label, fg) = match toast.kind {
                        ToastKind::Info => (
                            Color32::from_rgb(0x3b, 0x8e, 0xea),
                            "INFO",
                            Color32::from_rgb(0xea, 0xea, 0xea),
                        ),
                        ToastKind::Success => (
                            Color32::from_rgb(0x23, 0xd1, 0x8b),
                            "OK",
                            Color32::from_rgb(0xea, 0xea, 0xea),
                        ),
                        ToastKind::Warn => (
                            Color32::from_rgb(0xe5, 0xa5, 0x10),
                            "WARN",
                            Color32::from_rgb(0xea, 0xea, 0xea),
                        ),
                        ToastKind::Error => (
                            Color32::from_rgb(0xf1, 0x4c, 0x4c),
                            "ERROR",
                            Color32::from_rgb(0xff, 0xff, 0xff),
                        ),
                    };
                    let frame = Frame::default()
                        .fill(Color32::from_rgb(0x1c, 0x1c, 0x20))
                        .stroke(Stroke::new(1.0, accent))
                        .inner_margin(egui::Margin::same(10))
                        .corner_radius(egui::CornerRadius::same(4));
                    frame
                        .show(ui, |ui| {
                            ui.set_width(toast_width);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(label)
                                        .color(accent)
                                        .strong()
                                        .font(FontId::monospace(11.0)),
                                );
                                ui.label(
                                    RichText::new(&toast.title).color(fg).strong(),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("x").on_hover_text("Dismiss").clicked() {
                                            dismiss = Some(idx);
                                        }
                                    },
                                );
                            });
                            if !toast.body.is_empty() {
                                ui.add_space(4.0);
                                ui.label(RichText::new(&toast.body).color(fg).small());
                            }
                        })
                        .response
                });
            y_offset += response.response.rect.height() + 8.0;
        }

        if let Some(idx) = dismiss {
            self.toasts.remove(idx);
        }

        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
