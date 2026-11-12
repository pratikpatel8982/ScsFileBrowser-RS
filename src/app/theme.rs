//! The dark "Rosé Pine Moon" palette of the original app, applied to egui.

use eframe::egui::{self, Color32, Stroke};

pub const SUBTLE: Color32 = Color32::from_rgb(144, 140, 170);
pub const TEXT: Color32 = Color32::from_rgb(224, 222, 244);

pub const BASE: Color32 = Color32::from_rgb(25, 23, 36);
pub const SURFACE: Color32 = Color32::from_rgb(31, 29, 46);
pub const OVERLAY: Color32 = Color32::from_rgb(38, 35, 58);

pub const LOVE: Color32 = Color32::from_rgb(244, 99, 34);
pub const GOLD: Color32 = Color32::from_rgb(246, 193, 119);
pub const FOAM: Color32 = Color32::from_rgb(156, 207, 216);

pub const PINE: Color32 = Color32::from_rgb(49, 116, 143);

pub const HIGHLIGHT_LOW: Color32 = Color32::from_rgb(33, 32, 46);
pub const HIGHLIGHT_MED: Color32 = Color32::from_rgb(64, 61, 82);
pub const HIGHLIGHT_HIGH: Color32 = Color32::from_rgb(82, 79, 103);

pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();

    v.override_text_color = Some(TEXT);
    v.panel_fill = SURFACE;
    v.window_fill = BASE;
    v.window_stroke = Stroke::new(1.0, HIGHLIGHT_HIGH);
    v.extreme_bg_color = OVERLAY; // text edits and list backgrounds
    v.faint_bg_color = HIGHLIGHT_LOW;
    v.hyperlink_color = FOAM;
    v.selection.bg_fill = PINE;
    v.selection.stroke = Stroke::new(1.0, TEXT);

    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.weak_bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, HIGHLIGHT_MED);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.inactive.bg_fill = HIGHLIGHT_LOW;
    v.widgets.inactive.weak_bg_fill = HIGHLIGHT_LOW;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, HIGHLIGHT_MED);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);

    v.widgets.hovered.bg_fill = HIGHLIGHT_MED;
    v.widgets.hovered.weak_bg_fill = HIGHLIGHT_MED;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, HIGHLIGHT_HIGH);
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);

    v.widgets.active.bg_fill = PINE;
    v.widgets.active.weak_bg_fill = PINE;
    v.widgets.active.bg_stroke = Stroke::new(1.0, FOAM);
    v.widgets.active.fg_stroke = Stroke::new(2.0, TEXT);

    v.widgets.open.bg_fill = HIGHLIGHT_MED;
    v.widgets.open.weak_bg_fill = HIGHLIGHT_MED;

    ctx.set_visuals(v);

    // Table cells are plain labels: if they were text-selectable they would
    // swallow clicks and right-clicks meant for the row underneath them.
    ctx.all_styles_mut(|style| style.interaction.selectable_labels = false);
}
