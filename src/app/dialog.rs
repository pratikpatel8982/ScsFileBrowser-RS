//! The one look every dialog shares: a modal panel with a title bar of its own (title centred,
//! close button on the right), so no dialog is bar-less while others have one.

use eframe::egui::{
    self, pos2, vec2, Align2, CornerRadius, FontId, Id, Margin, Modal, Rect, Sense, Stroke, Ui,
};

use super::theme;

const BAR_HEIGHT: f32 = 26.0;
const RADIUS: u8 = 8;

/// Show a modal dialog `width` points wide. Returns `true` when it was dismissed (close button,
/// Escape, or a click outside it); `add_contents` draws the body.
pub fn show(
    ctx: &egui::Context,
    id: Id,
    title: &str,
    width: f32,
    add_contents: impl FnOnce(&mut Ui),
) -> bool {
    let frame = egui::Frame::new()
        .fill(theme::BASE)
        .stroke(Stroke::new(1.0, theme::HIGHLIGHT_HIGH))
        .corner_radius(RADIUS)
        .shadow(ctx.global_style().visuals.popup_shadow);

    let mut close_clicked = false;
    let r = Modal::new(id).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 6.0;

        // Title bar.
        let (bar, _) = ui.allocate_exact_size(vec2(width, BAR_HEIGHT), Sense::hover());
        ui.painter().rect_filled(
            bar,
            CornerRadius {
                nw: RADIUS,
                ne: RADIUS,
                sw: 0,
                se: 0,
            },
            theme::HIGHLIGHT_MED,
        );
        ui.painter().text(
            bar.center(),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(14.5),
            theme::TEXT,
        );
        let close =
            Rect::from_center_size(pos2(bar.right() - 16.0, bar.center().y), vec2(20.0, 20.0));
        let resp = ui
            .interact(close, id.with("close"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let color = if resp.hovered() {
            theme::LOVE
        } else {
            theme::TEXT
        };
        let c = close.center();
        let stroke = Stroke::new(1.5, color);
        ui.painter()
            .line_segment([c + vec2(-4.5, -4.5), c + vec2(4.5, 4.5)], stroke);
        ui.painter()
            .line_segment([c + vec2(-4.5, 4.5), c + vec2(4.5, -4.5)], stroke);
        close_clicked = resp.clicked();

        // Body.
        egui::Frame::new()
            .inner_margin(Margin::same(12))
            .show(ui, |ui| add_contents(ui));
    });
    close_clicked || r.should_close()
}
