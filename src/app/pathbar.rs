//! Breadcrumb path bar that turns into a text box when clicked.

use eframe::egui::{
    self, Align, Button, CursorIcon, FontId, Key, Layout, RichText, Sense, TextEdit, UiBuilder,
};

use super::theme;
use crate::util::ensure_local_path;

pub enum PathBarEvent {
    /// Navigate to this archive path (`""` is the root).
    Navigate(String),
    /// Copy the current path (with a leading `/`) to the clipboard.
    CopyPath,
}

#[derive(Default)]
pub struct PathBar {
    edit: bool,
    text: String,
    focus_next: bool,
}

impl PathBar {
    fn start_edit(&mut self, current: &str) {
        self.edit = true;
        self.text = current.to_string();
        self.focus_next = true;
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, current: &str) -> Option<PathBarEvent> {
        let mut event = None;
        let height = 28.0;
        let (rect, bg) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());

        if bg.hovered() && !self.edit {
            ui.painter().rect_filled(rect, 3.0, theme::OVERLAY);
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }
        if bg.clicked() && !self.edit {
            self.start_edit(current);
        }

        let mut want_edit = false;
        bg.context_menu(|ui| {
            if ui.button("Copy Path").clicked() {
                event = Some(PathBarEvent::CopyPath);
                ui.close();
            }
            if ui.button("Edit Path").clicked() {
                want_edit = true;
                ui.close();
            }
        });
        if want_edit {
            self.start_edit(current);
        }

        let builder = UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center));
        ui.scope_builder(builder, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new("Current Path:").size(16.0));
            ui.add_space(8.0);

            if self.edit {
                let edit = TextEdit::singleline(&mut self.text)
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(16.0));
                let resp = ui.add(edit);
                if self.focus_next {
                    resp.request_focus();
                    self.focus_next = false;
                }
                let (enter, esc) =
                    ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
                if resp.lost_focus() {
                    if enter {
                        event = Some(PathBarEvent::Navigate(ensure_local_path(&self.text)));
                    }
                    self.edit = false;
                } else if esc {
                    self.edit = false;
                }
            } else {
                let parts: Vec<&str> = current.split('/').filter(|p| !p.is_empty()).collect();
                if crumb(ui, "/") {
                    event = Some(PathBarEvent::Navigate(String::new()));
                }
                for (i, part) in parts.iter().enumerate() {
                    ui.label(RichText::new(">").color(theme::SUBTLE));
                    if crumb(ui, part) {
                        event = Some(PathBarEvent::Navigate(parts[..=i].join("/")));
                    }
                }
            }
        });

        event
    }
}

fn crumb(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add(Button::new(RichText::new(text).size(15.0)).frame(false))
        .on_hover_cursor(CursorIcon::PointingHand)
        .clicked()
}
