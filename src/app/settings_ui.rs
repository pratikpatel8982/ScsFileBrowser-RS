//! The settings window: external programs and which file extensions open with them.

use std::path::Path;

use eframe::egui::{self, Align2, Id, Modal, RichText, ScrollArea, Sense, TextEdit};

use super::theme;
use crate::settings::{ExtensionSetting, Program, SettingsStore};

const HELP_TEXT: &str = "- Drag an extension to a program to assign it to the program.\n\
- Right click on an extension or program for options to rename or delete it.";

enum Dialog {
    Message { title: &'static str, text: String },
    RenameProgram { old: String, text: String },
    RenameExtension { old: String, text: String },
    AddProgram { path: String, name: String },
    AddExtension { text: String, program: String },
    DeleteProgram(String),
    DeleteExtension(String),
    Unsaved,
}

#[derive(Default)]
pub struct SettingsWindow {
    /// Unsaved changes exist.
    pending: bool,
    dialog: Option<Dialog>,
    /// Set once the user has resolved the "unsaved changes" prompt and the window should go away.
    closing: bool,
}

impl SettingsWindow {
    /// Draw the window. Returns `false` once it has been closed.
    pub fn show(&mut self, ctx: &egui::Context, store: &mut SettingsStore) -> bool {
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_size([420.0, 420.0])
            .min_width(320.0)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| self.body(ui, store));

        self.show_dialog(ctx, store);

        if self.closing {
            return false;
        }
        if !open {
            if self.pending {
                self.dialog = Some(Dialog::Unsaved);
                return true;
            }
            return false;
        }
        true
    }

    fn body(&mut self, ui: &mut egui::Ui, store: &mut SettingsStore) {
        ui.horizontal(|ui| {
            if ui.button("Add Program").clicked() {
                self.add_program_flow();
            }
            if ui.button("Add Extension").clicked() {
                if store.settings.programs.is_empty() {
                    self.dialog = Some(Dialog::Message {
                        title: "Settings",
                        text: "At least one program is required to add an extension".into(),
                    });
                } else {
                    self.dialog = Some(Dialog::AddExtension {
                        text: String::new(),
                        program: store.settings.programs[0].name.clone(),
                    });
                }
            }
            if ui.button("Save").clicked() {
                self.pending = false;
                store.save();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let help = ui.label(RichText::new("(?)").color(theme::GOLD));
                help.on_hover_text(HELP_TEXT);
            });
        });
        ui.separator();

        if store.settings.programs.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(
                        "Once you add programs and extensions you will be able to see and configure them here.",
                    )
                    .color(theme::FOAM),
                );
            });
            return;
        }

        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.tree(ui, store);
            });
    }

    fn tree(&mut self, ui: &mut egui::Ui, store: &mut SettingsStore) {
        let mut programs: Vec<Program> = store.settings.programs.clone();
        programs.sort_by(|a, b| super::list::cmp_ignore_case(&a.name, &b.name));

        let mut reassign: Option<(String, String)> = None; // (extension, program)

        for program in &programs {
            let mut exts: Vec<&ExtensionSetting> = store
                .settings
                .extensions
                .iter()
                .filter(|e| e.default_program == program.name)
                .collect();
            exts.sort_by(|a, b| super::list::cmp_ignore_case(&a.name, &b.name));

            let frame = egui::Frame::group(ui.style()).inner_margin(4.0);
            let (_, dropped) = ui.dnd_drop_zone::<String, _>(frame, |ui| {
                ui.set_min_width(ui.available_width());
                let header = egui::CollapsingHeader::new(RichText::new(&program.name).strong())
                    .id_salt(("program", &program.name))
                    .default_open(true)
                    .show(ui, |ui| {
                        for ext in &exts {
                            self.extension_row(ui, &ext.name);
                        }
                    });
                header
                    .header_response
                    .clone()
                    .on_hover_text(&program.path)
                    .context_menu(|ui| self.item_menu(ui, ItemKind::Program(program.name.clone())));
            });
            if let Some(ext) = dropped {
                reassign = Some(((*ext).clone(), program.name.clone()));
            }
            ui.add_space(2.0);
        }

        // Extensions pointing at a program that no longer exists would otherwise be invisible.
        let orphans: Vec<String> = store
            .settings
            .extensions
            .iter()
            .filter(|e| !programs.iter().any(|p| p.name == e.default_program))
            .map(|e| e.name.clone())
            .collect();
        if !orphans.is_empty() {
            ui.add_space(6.0);
            ui.label(RichText::new("No program assigned").color(theme::SUBTLE));
            for name in &orphans {
                self.extension_row(ui, name);
            }
        }

        if let Some((ext, program)) = reassign {
            if let Some(e) = store.settings.extensions.iter_mut().find(|e| e.name == ext) {
                if e.default_program != program {
                    log::debug!(
                        "Setting default program for extension '{}' from '{}' to '{}'",
                        e.name,
                        e.default_program,
                        program
                    );
                    e.default_program = program;
                    self.pending = true;
                }
            }
        }
    }

    fn extension_row(&mut self, ui: &mut egui::Ui, name: &str) {
        let id = Id::unique(("ext", name));
        let inner = ui.dnd_drag_source(id, name.to_string(), |ui| {
            let r = ui.add(egui::Label::new(name).sense(Sense::click()));
            r.context_menu(|ui| self.item_menu(ui, ItemKind::Extension(name.to_string())));
        });
        inner
            .response
            .on_hover_text("Drag onto a program to assign it");
    }

    fn item_menu(&mut self, ui: &mut egui::Ui, kind: ItemKind) {
        if ui.button("Rename").clicked() {
            self.dialog = Some(match &kind {
                ItemKind::Program(n) => Dialog::RenameProgram {
                    old: n.clone(),
                    text: n.clone(),
                },
                ItemKind::Extension(n) => Dialog::RenameExtension {
                    old: n.clone(),
                    text: n.clone(),
                },
            });
            ui.close();
        }
        if ui
            .button(RichText::new("Delete").color(theme::LOVE))
            .clicked()
        {
            self.dialog = Some(match kind {
                ItemKind::Program(n) => Dialog::DeleteProgram(n),
                ItemKind::Extension(n) => Dialog::DeleteExtension(n),
            });
            ui.close();
        }
    }

    fn add_program_flow(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Select a program")
            .pick_file()
        else {
            return;
        };
        let name = Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.dialog = Some(Dialog::AddProgram {
            path: path.to_string_lossy().into_owned(),
            name,
        });
    }

    fn show_dialog(&mut self, ctx: &egui::Context, store: &mut SettingsStore) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let settings = &mut store.settings;
        let mut keep: Option<Dialog> = None;
        let id = Id::unique("settings_dialog");

        match dialog {
            Dialog::Message { title, text } => {
                let r = Modal::new(id).show(ctx, |ui| {
                    ui.heading(title);
                    ui.label(&text);
                    ui.add_space(8.0);
                    ui.button("OK").clicked()
                });
                if !(r.inner || r.should_close()) {
                    keep = Some(Dialog::Message { title, text });
                }
            }
            Dialog::RenameProgram { old, mut text } => {
                let (ok, cancel) = text_prompt(
                    ctx,
                    id,
                    &format!("Renaming '{old}'"),
                    "Enter a new program name",
                    &mut text,
                );
                if ok {
                    let new = text.trim().to_string();
                    if !new.is_empty() && new != old {
                        log::debug!("Renaming program from '{old}' to '{new}'");
                        for e in settings
                            .extensions
                            .iter_mut()
                            .filter(|e| e.default_program == old)
                        {
                            e.default_program = new.clone();
                        }
                        if let Some(p) = settings.programs.iter_mut().find(|p| p.name == old) {
                            p.name = new;
                        }
                        self.pending = true;
                    }
                } else if !cancel {
                    keep = Some(Dialog::RenameProgram { old, text });
                }
            }
            Dialog::RenameExtension { old, mut text } => {
                let (ok, cancel) = text_prompt(
                    ctx,
                    id,
                    &format!("Renaming '{old}'"),
                    "Enter a new extension name",
                    &mut text,
                );
                if ok {
                    let new = text.trim().to_string();
                    if !new.is_empty() && new != old {
                        log::debug!("Renaming extension from '{old}' to '{new}'");
                        if let Some(e) = settings.extensions.iter_mut().find(|e| e.name == old) {
                            e.name = new;
                        }
                        self.pending = true;
                    }
                } else if !cancel {
                    keep = Some(Dialog::RenameExtension { old, text });
                }
            }
            Dialog::AddProgram { path, mut name } => {
                let (ok, cancel) = text_prompt(
                    ctx,
                    id,
                    "Add a program",
                    "Enter a name for the program",
                    &mut name,
                );
                if ok {
                    let name = if name.trim().is_empty() {
                        Path::new(&path)
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    } else {
                        name.trim().to_string()
                    };
                    if !settings.programs.iter().any(|p| p.path == path) {
                        log::debug!("Adding new program '{name}' with path '{path}'");
                        settings.programs.push(Program { name, path });
                        store.save();
                    }
                } else if !cancel {
                    keep = Some(Dialog::AddProgram { path, name });
                }
            }
            Dialog::AddExtension {
                mut text,
                mut program,
            } => {
                let names: Vec<String> = settings.programs.iter().map(|p| p.name.clone()).collect();
                let mut ok = false;
                let mut cancel = false;
                let r = Modal::new(id).show(ctx, |ui| {
                    ui.heading("Add a file extension");
                    ui.label("Enter a file extension (e.g. .sii)");
                    let resp = ui.add(TextEdit::singleline(&mut text).desired_width(260.0));
                    if !resp.has_focus() && text.is_empty() {
                        resp.request_focus();
                    }
                    ui.add_space(6.0);
                    ui.label("Select a default program for the extension");
                    egui::ComboBox::from_id_salt("ext_program")
                        .selected_text(&program)
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for n in &names {
                                ui.selectable_value(&mut program, n.clone(), n);
                            }
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ok = ui.button("Continue").clicked()
                            || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        cancel = ui.button("Cancel").clicked();
                    });
                });
                cancel |= r.should_close();
                if ok {
                    let mut ext = text.trim().to_string();
                    if !ext.is_empty() && !program.is_empty() {
                        if !ext.starts_with('.') {
                            ext = format!(".{ext}");
                        }
                        if !settings.extensions.iter().any(|e| e.name == ext) {
                            log::debug!(
                                "Adding new extension '{ext}' with default program '{program}'"
                            );
                            settings.extensions.push(ExtensionSetting {
                                name: ext,
                                default_program: program,
                            });
                            store.save();
                        }
                    }
                } else if !cancel {
                    keep = Some(Dialog::AddExtension { text, program });
                }
            }
            Dialog::DeleteProgram(name) => {
                if settings
                    .extensions
                    .iter()
                    .any(|e| e.default_program == name)
                {
                    keep = Some(Dialog::Message {
                        title: "Could not delete program.",
                        text: "Cannot remove a program that has extension(s) assigned to it."
                            .into(),
                    });
                } else {
                    match confirm(
                        ctx,
                        id,
                        "Delete program?",
                        &format!("Are you sure you want to delete the program '{name}'?"),
                    ) {
                        Some(true) => {
                            log::debug!("Deleting program '{name}'");
                            settings.programs.retain(|p| p.name != name);
                            self.pending = true;
                        }
                        Some(false) => {}
                        None => keep = Some(Dialog::DeleteProgram(name)),
                    }
                }
            }
            Dialog::DeleteExtension(name) => {
                match confirm(
                    ctx,
                    id,
                    "Delete extension?",
                    &format!("Are you sure you want to delete the extension '{name}'?"),
                ) {
                    Some(true) => {
                        log::debug!("Deleting extension '{name}'");
                        settings.extensions.retain(|e| e.name != name);
                        self.pending = true;
                    }
                    Some(false) => {}
                    None => keep = Some(Dialog::DeleteExtension(name)),
                }
            }
            Dialog::Unsaved => {
                // Yes = save and close, No = discard and close, Cancel = keep editing.
                let mut choice = None;
                let r = Modal::new(id).show(ctx, |ui| {
                    ui.heading("Save changes?");
                    ui.label("There are unsaved changes, do you want to save them?");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Yes").clicked() {
                            choice = Some(Some(true));
                        }
                        if ui.button("No").clicked() {
                            choice = Some(Some(false));
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(None);
                        }
                    });
                });
                if r.should_close() && choice.is_none() {
                    choice = Some(None);
                }
                match choice {
                    Some(Some(true)) => {
                        store.save();
                        self.pending = false;
                        self.closing = true;
                    }
                    Some(Some(false)) => {
                        store.load();
                        self.pending = false;
                        self.closing = true;
                    }
                    Some(None) => {}
                    None => keep = Some(Dialog::Unsaved),
                }
            }
        }
        self.dialog = keep;
    }
}

enum ItemKind {
    Program(String),
    Extension(String),
}

/// A small modal with one text box. Returns `(ok, cancelled)`.
fn text_prompt(
    ctx: &egui::Context,
    id: Id,
    title: &str,
    label: &str,
    text: &mut String,
) -> (bool, bool) {
    let mut ok = false;
    let mut cancel = false;
    let r = Modal::new(id).show(ctx, |ui| {
        ui.heading(title);
        ui.label(label);
        let resp = ui.add(TextEdit::singleline(text).desired_width(280.0));
        if !resp.has_focus() && !resp.lost_focus() {
            resp.request_focus();
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ok = ui.button("Continue").clicked()
                || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            cancel = ui.button("Cancel").clicked();
        });
    });
    cancel |= r.should_close();
    (ok && !cancel, cancel)
}

/// Yes/No confirmation. `Some(answer)` once decided, `None` while still open.
fn confirm(ctx: &egui::Context, id: Id, title: &str, text: &str) -> Option<bool> {
    let mut answer = None;
    let r = Modal::new(id).show(ctx, |ui| {
        ui.heading(title);
        ui.label(text);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Yes").clicked() {
                answer = Some(true);
            }
            if ui.button("No").clicked() {
                answer = Some(false);
            }
        });
    });
    if answer.is_none() && r.should_close() {
        answer = Some(false);
    }
    answer
}
