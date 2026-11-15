//! The settings window: external programs and which file extensions open with them.

use std::path::Path;

use eframe::egui::{self, Align2, Id, RichText, ScrollArea, Sense, TextEdit};

use super::dialog;
use super::theme;
#[cfg(all(unix, not(target_os = "macos")))]
use crate::desktop::{self, DesktopEntry};
use crate::settings::{ExtensionSetting, Program, SettingsStore};

const HELP_TEXT: &str = "- Drag an extension to a program to assign it to the program.\n\
- Use the Rename, Delete and Unassign buttons (or right click) on an extension or program.\n\
- Changes are saved automatically.";

enum Dialog {
    Message {
        title: &'static str,
        text: String,
    },
    RenameProgram {
        old: String,
        text: String,
    },
    RenameExtension {
        old: String,
        text: String,
    },
    AddProgram {
        path: String,
        name: String,
    },
    AddExtension {
        text: String,
        program: String,
    },
    DeleteProgram(String),
    DeleteExtension(String),
    /// Linux/BSD: choose among the installed applications.
    #[cfg(all(unix, not(target_os = "macos")))]
    PickApp {
        apps: Vec<DesktopEntry>,
        filter: String,
        selected: Option<usize>,
    },
}

#[derive(Default)]
pub struct SettingsWindow {
    /// The settings changed this frame; they are written to disk straight away.
    dirty: bool,
    /// An extension whose "Unassign" button was pressed this frame.
    unassign: Option<String>,
    dialog: Option<Dialog>,
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

        // Every change is saved as soon as it is made.
        if std::mem::take(&mut self.dirty) {
            store.save();
        }

        open
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

            // Highlight the card an extension is being dragged over.
            let rect_id = Id::unique(("program_rect", &program.name));
            let hovered = egui::DragAndDrop::has_payload_of_type::<String>(ui.ctx())
                && ui
                    .ctx()
                    .data(|d| d.get_temp::<egui::Rect>(rect_id))
                    .zip(ui.ctx().pointer_hover_pos())
                    .is_some_and(|(r, p)| r.contains(p));
            let frame = card_frame(hovered);
            let (resp, dropped) = ui.dnd_drop_zone::<String, _>(frame, |ui| {
                ui.set_min_width(ui.available_width());
                let id = ui.make_persistent_id(("program", &program.name));
                let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                    ui.ctx(),
                    id,
                    true,
                );
                state
                    .show_header(ui, |ui| {
                        let r = ui
                            .label(RichText::new(&program.name).strong().size(15.0))
                            .on_hover_text(&program.path);
                        r.context_menu(|ui| {
                            self.item_menu(ui, ItemKind::Program(program.name.clone()))
                        });
                        let count = exts.len();
                        ui.label(
                            RichText::new(match count {
                                0 => "no extensions".to_string(),
                                1 => "1 extension".to_string(),
                                n => format!("{n} extensions"),
                            })
                            .small()
                            .color(theme::SUBTLE),
                        );
                        self.item_buttons(ui, ItemKind::Program(program.name.clone()), false);
                    })
                    .body(|ui| {
                        if exts.is_empty() {
                            ui.label(
                                RichText::new(
                                    "Drag an extension here to open it with this program",
                                )
                                .small()
                                .italics()
                                .color(theme::SUBTLE),
                            );
                        }
                        for ext in &exts {
                            self.extension_row(ui, &ext.name, true);
                        }
                    });
            });
            ui.ctx()
                .data_mut(|d| d.insert_temp(rect_id, resp.response.rect));
            if let Some(ext) = dropped {
                reassign = Some(((*ext).clone(), program.name.clone()));
            }
            ui.add_space(4.0);
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
            card_frame(false).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(
                    RichText::new("Unassigned extensions")
                        .strong()
                        .size(15.0)
                        .color(theme::GOLD),
                );
                ui.label(
                    RichText::new("These open with nothing yet. Drag one onto a program.")
                        .small()
                        .color(theme::SUBTLE),
                );
                ui.add_space(2.0);
                for name in &orphans {
                    self.extension_row(ui, name, false);
                }
            });
        }

        if let Some(ext) = self.unassign.take() {
            if let Some(e) = store.settings.extensions.iter_mut().find(|e| e.name == ext) {
                log::debug!(
                    "Removing the default program '{}' from extension '{}'",
                    e.default_program,
                    e.name
                );
                e.default_program.clear();
                self.dirty = true;
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
                    self.dirty = true;
                }
            }
        }
    }

    fn extension_row(&mut self, ui: &mut egui::Ui, name: &str, assigned: bool) {
        ui.horizontal(|ui| {
            let id = Id::unique(("ext", name));
            let inner = ui.dnd_drag_source(id, name.to_string(), |ui| {
                ui.set_min_width(90.0);
                let r = ui.add(
                    egui::Label::new(RichText::new(name).monospace().color(theme::TEXT))
                        .sense(Sense::click()),
                );
                r.context_menu(|ui| self.item_menu(ui, ItemKind::Extension(name.to_string())));
            });
            inner
                .response
                .on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text("Drag onto a program to assign it");
            self.item_buttons(ui, ItemKind::Extension(name.to_string()), assigned);
        });
    }

    /// Visible Rename / Delete (and, for assigned extensions, Unassign) buttons, right-aligned in
    /// the current row. The same actions are in the right-click menu.
    fn item_buttons(&mut self, ui: &mut egui::Ui, kind: ItemKind, unassignable: bool) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let what = match kind {
                ItemKind::Program(_) => "program",
                ItemKind::Extension(_) => "extension",
            };
            if text_button(ui, "Delete", theme::LOVE)
                .on_hover_text(format!("Delete this {what}"))
                .clicked()
            {
                self.delete_dialog(kind.clone());
            }
            if text_button(ui, "Rename", theme::FOAM)
                .on_hover_text(format!("Rename this {what}"))
                .clicked()
            {
                self.rename_dialog(&kind);
            }
            if let (true, ItemKind::Extension(name)) = (unassignable, &kind) {
                if text_button(ui, "Unassign", theme::GOLD)
                    .on_hover_text(
                        "Stop opening this extension with the program (keeps the extension)",
                    )
                    .clicked()
                {
                    self.unassign = Some(name.clone());
                }
            }
        });
    }

    fn rename_dialog(&mut self, kind: &ItemKind) {
        self.dialog = Some(match kind {
            ItemKind::Program(n) => Dialog::RenameProgram {
                old: n.clone(),
                text: n.clone(),
            },
            ItemKind::Extension(n) => Dialog::RenameExtension {
                old: n.clone(),
                text: n.clone(),
            },
        });
    }

    fn delete_dialog(&mut self, kind: ItemKind) {
        self.dialog = Some(match kind {
            ItemKind::Program(n) => Dialog::DeleteProgram(n),
            ItemKind::Extension(n) => Dialog::DeleteExtension(n),
        });
    }

    fn item_menu(&mut self, ui: &mut egui::Ui, kind: ItemKind) {
        if ui.button("Rename").clicked() {
            self.rename_dialog(&kind);
            ui.close();
        }
        if ui
            .button(RichText::new("Delete").color(theme::LOVE))
            .clicked()
        {
            self.delete_dialog(kind);
            ui.close();
        }
    }

    /// "Add Program": on Linux/BSD pick from the installed applications (there is no
    /// "program file" to browse for there), elsewhere browse for the executable.
    fn add_program_flow(&mut self) {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.dialog = Some(Dialog::PickApp {
                apps: desktop::installed_apps(),
                filter: String::new(),
                selected: None,
            });
        }
        #[cfg(not(all(unix, not(target_os = "macos"))))]
        self.browse_for_program();
    }

    fn browse_for_program(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Select a program")
            .pick_file()
        else {
            return;
        };
        let path = path.to_string_lossy().into_owned();
        let name = default_program_name(&path);
        self.dialog = Some(Dialog::AddProgram { path, name });
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
                let mut ok = false;
                let closed = dialog::show(ctx, id, title, 380.0, |ui| {
                    ui.label(&text);
                    ui.add_space(8.0);
                    ok = ui.button("OK").clicked();
                });
                if !(ok || closed) {
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
                        self.dirty = true;
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
                        self.dirty = true;
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
                        default_program_name(&path)
                    } else {
                        name.trim().to_string()
                    };
                    if !settings.programs.iter().any(|p| p.path == path) {
                        log::debug!("Adding new program '{name}' with path '{path}'");
                        settings.programs.push(Program { name, path });
                        self.dirty = true;
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
                let closed = dialog::show(ctx, id, "Add a file extension", 340.0, |ui| {
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
                cancel |= closed;
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
                            self.dirty = true;
                        }
                    }
                } else if !cancel {
                    keep = Some(Dialog::AddExtension { text, program });
                }
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Dialog::PickApp {
                apps,
                mut filter,
                mut selected,
            } => {
                enum Outcome {
                    Pick(usize),
                    Browse,
                    Cancel,
                }
                let mut outcome = None;
                let closed = dialog::show(ctx, id, "Add a program", 400.0, |ui| {
                    ui.label("Choose an installed application");
                    let search = ui.add(
                        TextEdit::singleline(&mut filter)
                            .hint_text("Search")
                            .desired_width(f32::INFINITY),
                    );
                    if !search.has_focus() && filter.is_empty() {
                        search.request_focus();
                    }
                    let needle = filter.trim().to_lowercase();
                    let shown: Vec<usize> = (0..apps.len())
                        .filter(|&i| {
                            needle.is_empty()
                                || apps[i].name.to_lowercase().contains(&needle)
                                || apps[i].comment.to_lowercase().contains(&needle)
                        })
                        .collect();
                    if selected.is_some_and(|s| !shown.contains(&s)) {
                        selected = None;
                    }
                    ui.add_space(4.0);
                    ScrollArea::vertical()
                        .max_height(280.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.with_layout(
                                egui::Layout::top_down_justified(egui::Align::LEFT),
                                |ui| {
                                    if shown.is_empty() {
                                        ui.label(
                                            RichText::new("No applications found")
                                                .color(theme::SUBTLE),
                                        );
                                    }
                                    for &i in &shown {
                                        let app = &apps[i];
                                        let r = ui
                                            .selectable_label(
                                                selected == Some(i),
                                                app.name.as_str(),
                                            )
                                            .on_hover_text(if app.comment.is_empty() {
                                                app.path.to_string_lossy().into_owned()
                                            } else {
                                                format!("{}\n{}", app.comment, app.path.display())
                                            });
                                        if r.clicked() {
                                            selected = Some(i);
                                        }
                                        if r.double_clicked() {
                                            outcome = Some(Outcome::Pick(i));
                                        }
                                    }
                                },
                            );
                        });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(selected.is_some(), egui::Button::new("Select"))
                            .clicked()
                        {
                            outcome = selected.map(Outcome::Pick);
                        }
                        if ui.button("Browse...").clicked() {
                            outcome = Some(Outcome::Browse);
                        }
                        if ui.button("Cancel").clicked() {
                            outcome = Some(Outcome::Cancel);
                        }
                    });
                });
                if closed && outcome.is_none() {
                    outcome = Some(Outcome::Cancel);
                }
                match outcome {
                    Some(Outcome::Pick(i)) => {
                        let app = &apps[i];
                        keep = Some(Dialog::AddProgram {
                            path: app.path.to_string_lossy().into_owned(),
                            name: app.name.clone(),
                        });
                    }
                    Some(Outcome::Browse) => {
                        self.browse_for_program();
                        keep = self.dialog.take();
                    }
                    Some(Outcome::Cancel) => {}
                    None => {
                        keep = Some(Dialog::PickApp {
                            apps,
                            filter,
                            selected,
                        })
                    }
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
                            self.dirty = true;
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
                        self.dirty = true;
                    }
                    Some(false) => {}
                    None => keep = Some(Dialog::DeleteExtension(name)),
                }
            }
        }
        self.dialog = keep;
    }
}

#[derive(Clone)]
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
    let closed = dialog::show(ctx, id, title, 340.0, |ui| {
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
    cancel |= closed;
    (ok && !cancel, cancel)
}

/// Yes/No confirmation. `Some(answer)` once decided, `None` while still open.
fn confirm(ctx: &egui::Context, id: Id, title: &str, text: &str) -> Option<bool> {
    let mut answer = None;
    let closed = dialog::show(ctx, id, title, 380.0, |ui| {
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
    if answer.is_none() && closed {
        answer = Some(false);
    }
    answer
}

/// The name offered for a newly added program: the application's own name for a `.desktop`
/// entry, otherwise the executable's file name without extension.
fn default_program_name(path: &str) -> String {
    if crate::desktop::is_desktop_file(path) {
        if let Ok(entry) = crate::desktop::DesktopEntry::load(Path::new(path)) {
            return entry.name;
        }
    }
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The rounded panel each program (and the unassigned list) is drawn in.
fn card_frame(highlight: bool) -> egui::Frame {
    egui::Frame::new()
        .fill(if highlight {
            theme::HIGHLIGHT_LOW
        } else {
            theme::SURFACE
        })
        .stroke(egui::Stroke::new(
            if highlight { 1.5 } else { 1.0 },
            if highlight {
                theme::PINE
            } else {
                theme::HIGHLIGHT_MED
            },
        ))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(8, 6))
}

/// A frameless coloured text button that tints its background on hover.
fn text_button(ui: &mut egui::Ui, text: &str, color: egui::Color32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).size(12.5).color(color))
            .frame_when_inactive(false)
            .min_size(egui::vec2(0.0, 20.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}
