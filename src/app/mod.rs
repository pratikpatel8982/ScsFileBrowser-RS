//! The graphical front-end (egui / eframe).

mod dragout;
mod list;
mod pathbar;
mod settings_ui;
mod theme;
#[cfg(all(unix, not(target_os = "macos")))]
mod xdnd;

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, FontId, Id, Key, Label, Layout, Modal, Modifiers, RichText, Sense,
    TextWrapMode, ViewportCommand,
};
use egui_extras::{Column, TableBuilder};

use self::list::{cmp_ignore_case, ListModel};
use self::pathbar::{PathBar, PathBarEvent};
use self::settings_ui::SettingsWindow;
use crate::cityhash::city_hash64;
use crate::extract::{self, extract_files, extract_folder, folder_contents_as_text, temp_dir};
use crate::fs::{has_archive_extension, FileInfo, FolderInfo, Vfs, ALLOWED_EXTENSIONS};
use crate::settings::{Program, SettingsStore};
use crate::util::{combine_path, ensure_local_path, extension, format_size, parent_path};

const APP_NAME: &str = "ScsFileBrowser";
const ROW_HEIGHT: f32 = 22.0;

// ---------------------------------------------------------------------------
// Columns and sorting
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum FolderCol {
    Name,
    SubItems,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileCol {
    Name,
    Source,
    Size,
}

fn cmp_folder(a: &FolderInfo, b: &FolderInfo, col: FolderCol) -> Ordering {
    match col {
        FolderCol::Name => cmp_ignore_case(&a.name, &b.name),
        FolderCol::SubItems => a
            .sub_entry_count
            .cmp(&b.sub_entry_count)
            .then_with(|| cmp_ignore_case(&a.name, &b.name)),
    }
}

fn cmp_file(a: &FileInfo, b: &FileInfo, col: FileCol) -> Ordering {
    match col {
        FileCol::Name => cmp_ignore_case(&a.name, &b.name),
        FileCol::Source => {
            cmp_ignore_case(&a.source, &b.source).then_with(|| cmp_ignore_case(&a.name, &b.name))
        }
        FileCol::Size => a
            .size
            .cmp(&b.size)
            .then_with(|| cmp_ignore_case(&a.name, &b.name)),
    }
}

// ---------------------------------------------------------------------------
// Messages between the UI and background threads / deferred UI actions
// ---------------------------------------------------------------------------

enum TaskEvent {
    Loaded {
        path: PathBuf,
        result: Result<(Vfs, Vec<(PathBuf, String)>), String>,
    },
    ExtractDone {
        message: String,
    },
}

enum Dialog {
    Message { title: String, text: String },
    ConfirmQuitExtraction,
    ConfirmCleanup,
}

enum Action {
    OpenFileDialog,
    OpenFolderDialog,
    /// An archive dropped onto the window.
    DropArchive(PathBuf),
    OpenRecent(String),
    OpenSettings,
    Navigate(String),
    ActivateFolder(String),
    ActivateFile(String),
    GoUp,
    CopyText(String),
    CopyFolderContents(Vec<String>),
    ExtractFolders(Vec<String>),
    ExtractFiles(Vec<String>),
    OpenFiles(Vec<String>, Option<usize>),
    /// Start an OS drag of these files (full archive paths) out of the window.
    DragOut(Vec<String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveList {
    Folders,
    Files,
}

enum RowEvent {
    Click(usize, Modifiers),
    Activate(usize),
    RightClick(usize),
    DragStart(usize),
}

/// What the context menu of a list currently applies to.
#[derive(Default)]
struct MenuTarget {
    paths: Vec<String>,
    shift: bool,
}

struct MenuCtx<'a> {
    target: &'a MenuTarget,
    extracting: bool,
    current_path: &'a str,
    programs: &'a [Program],
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct ScsApp {
    store: SettingsStore,
    vfs: Arc<Vfs>,
    current_path: String,
    folders: ListModel<FolderInfo, FolderCol>,
    files: ListModel<FileInfo, FileCol>,
    active: ActiveList,
    folder_menu: MenuTarget,
    file_menu: MenuTarget,
    path_bar: PathBar,
    path_tip: Option<(Instant, egui::Pos2)>,

    status: String,
    busy: Option<String>,
    loading: bool,
    extracting: bool,
    cancel: Arc<AtomicBool>,

    tx: Sender<TaskEvent>,
    rx: Receiver<TaskEvent>,
    dialogs: Vec<Dialog>,
    settings_win: Option<SettingsWindow>,

    pending_open: Option<PathBuf>,
    /// Files to drag out of the window, started at the end of the frame (needs the window handle).
    pending_drag: Option<Vec<String>>,
    /// The OS drag swallowed the mouse-up, so egui is told about it on the next frame.
    release_pending: bool,
    allow_close: bool,
    title: String,
}

/// Start the application. `initial` is an archive file or folder to open at launch.
pub fn run(initial: Option<PathBuf>) -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_inner_size([900.0, 700.0])
        .with_min_inner_size([560.0, 360.0]);
    match eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon.png")) {
        Ok(icon) => viewport = viewport.with_icon(Arc::new(icon)),
        Err(e) => log::warn!("Could not load the window icon: {e}"),
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(ScsApp::new(initial)))
        }),
    )
}

impl ScsApp {
    fn new(initial: Option<PathBuf>) -> Self {
        let (tx, rx) = channel();
        Self {
            store: SettingsStore::open_default(),
            vfs: Arc::new(Vfs::new()),
            current_path: String::new(),
            folders: ListModel::new(FolderCol::Name, cmp_folder, |f| f.name.as_str()),
            files: ListModel::new(FileCol::Name, cmp_file, |f| f.name.as_str()),
            active: ActiveList::Files,
            folder_menu: MenuTarget::default(),
            file_menu: MenuTarget::default(),
            path_bar: PathBar::default(),
            path_tip: None,
            status: String::new(),
            busy: None,
            loading: false,
            extracting: false,
            cancel: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            dialogs: Vec::new(),
            settings_win: None,
            pending_open: initial,
            pending_drag: None,
            release_pending: false,
            allow_close: false,
            title: APP_NAME.to_string(),
        }
    }

    // ----- small helpers ---------------------------------------------------

    fn set_status(&mut self, message: impl AsRef<str>) {
        self.status = format!(
            "[{}] {}",
            chrono::Local::now().format("%H:%M:%S"),
            message.as_ref()
        );
    }

    /// Status bar message plus an error log entry.
    fn fail(&mut self, message: String) {
        log::error!("{message}");
        self.set_status(message);
    }

    fn message(&mut self, title: &str, text: impl Into<String>) {
        self.dialogs.push(Dialog::Message {
            title: title.to_string(),
            text: text.into(),
        });
    }

    fn full_path(&self, name: &str) -> String {
        combine_path(&self.current_path, name)
            .trim_end_matches('/')
            .to_string()
    }

    /// Shows the "still loading" dialog and returns true while archives load.
    fn check_loading(&mut self) -> bool {
        if self.loading {
            self.message(
                "ScsFileBrowser",
                "Archive files are still being loaded, please wait.",
            );
        }
        self.loading
    }

    fn set_title(&mut self, ctx: &egui::Context, path: Option<&Path>) {
        self.title = match path {
            Some(p) => format!("{APP_NAME} - {}", p.display()),
            None => APP_NAME.to_string(),
        };
        ctx.send_viewport_cmd(ViewportCommand::Title(self.title.clone()));
    }

    fn reset(&mut self) {
        self.vfs = Arc::new(Vfs::new());
        self.folders.clear();
        self.files.clear();
        self.current_path.clear();
    }

    // ----- navigation ------------------------------------------------------

    fn change_path(&mut self, new_path: &str) -> bool {
        let path = ensure_local_path(new_path)
            .trim_end_matches('/')
            .to_string();
        match self.vfs.list(&path) {
            None => {
                self.fail(format!("Could not find directory '{new_path}'"));
                false
            }
            Some(listing) => {
                self.current_path = path;
                self.folders.set_items(listing.folders);
                self.files.set_items(listing.files);
                true
            }
        }
    }

    fn go_up(&mut self) {
        let parent = parent_path(&self.current_path).to_string();
        self.change_path(&parent);
    }

    // ----- loading archives ------------------------------------------------

    fn start_load(&mut self, ctx: &egui::Context, path: PathBuf) {
        let is_dir = path.is_dir();
        if self.check_loading() {
            return;
        }
        if self.extracting {
            let what = if is_dir {
                "a folder"
            } else {
                "an archive file"
            };
            self.message(
                "ScsFileBrowser",
                format!("Cannot open {what} while an extraction is running"),
            );
            return;
        }

        self.reset();
        self.loading = true;
        self.busy = Some("Loading archive files".into());
        log::debug!("Opening '{}'", path.display());

        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut vfs = Vfs::new();
            let loaded = if is_dir {
                vfs.add_source_directory(&path).map(|r| r.failed)
            } else {
                vfs.add_source_file(&path).map(|_| Vec::new())
            };
            let result = loaded
                .map(|failed| (vfs, failed))
                .map_err(|e| e.to_string());
            let _ = tx.send(TaskEvent::Loaded { path, result });
            ctx.request_repaint();
        });
    }

    fn finish_load(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        result: Result<(Vfs, Vec<(PathBuf, String)>), String>,
    ) {
        self.loading = false;
        self.busy = None;
        match result {
            Err(e) => {
                log::error!("Could not open '{}': {e}", path.display());
                self.set_status(format!("Error opening '{}'", path.display()));
                self.set_title(ctx, None);
                self.message(
                    "Could not open",
                    format!("'{}' could not be opened.\n\n{e}", path.display()),
                );
            }
            Ok((vfs, failed)) => {
                let is_dir = path.is_dir();
                let count = vfs.archives().len();
                self.vfs = Arc::new(vfs);

                let path_str = path.to_string_lossy().into_owned();
                self.store.settings.add_recent(&path_str);
                self.store.save();
                self.set_title(ctx, Some(&path));

                if count == 0 {
                    let msg = if is_dir {
                        format!(
                            "No archive files ({}) found in '{path_str}'",
                            ALLOWED_EXTENSIONS.join(", ")
                        )
                    } else {
                        format!("Error opening '{path_str}'")
                    };
                    self.fail(msg);
                    return;
                }

                log::info!("Mounted {count} archive(s) from '{path_str}'");
                if self.change_path("") {
                    let kind = if is_dir { "Folder" } else { "File" };
                    let mut msg = format!("Opened {kind}: {path_str}");
                    if !failed.is_empty() {
                        msg.push_str(&format!(
                            " ({} archive(s) could not be loaded, see the log)",
                            failed.len()
                        ));
                    }
                    self.set_status(msg);
                }
            }
        }
    }

    fn open_file_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Open an archive file")
            .add_filter(
                "SCS archives",
                &ALLOWED_EXTENSIONS
                    .iter()
                    .map(|e| e.trim_start_matches('.'))
                    .collect::<Vec<_>>(),
            )
            .add_filter("All files", &["*"]);
        let last = &self.store.settings.last_selected_file;
        if let Some(dir) = Path::new(last).parent().filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(dir);
        }
        let Some(file) = dialog.pick_file() else {
            return;
        };
        self.store.settings.last_selected_file = file.to_string_lossy().into_owned();
        self.store.save();
        self.start_load(ctx, file);
    }

    fn open_folder_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new().set_title("Open a folder containing archives");
        let last = Path::new(&self.store.settings.last_selected_folder);
        if last.is_dir() {
            dialog = dialog.set_directory(last);
        }
        let Some(folder) = dialog.pick_folder() else {
            return;
        };
        self.store.settings.last_selected_folder = folder.to_string_lossy().into_owned();
        self.store.save();
        self.start_load(ctx, folder);
    }

    fn open_recent(&mut self, ctx: &egui::Context, path: &str) {
        let p = PathBuf::from(path);
        if !p.exists() {
            self.fail(format!("Could not open the recent path '{path}'"));
            return;
        }
        log::debug!("Opening recent path '{path}'");
        self.start_load(ctx, p);
    }

    /// Open a path from the command line or a drag & drop.
    fn open_dropped(&mut self, ctx: &egui::Context, path: PathBuf) {
        if path.is_dir() || has_archive_extension(&path) {
            self.start_load(ctx, path);
        } else {
            let err = format!(
                "File '{}' did not have an allowed file extension ({})",
                path.display(),
                ALLOWED_EXTENSIONS.join(", ")
            );
            log::error!("{err}");
            self.message("Invalid file type", err);
        }
    }

    /// A file dropped onto the window. Like the original, only archive files with an
    /// allowed extension are accepted (folders are opened via File > Open Folder).
    fn open_drop(&mut self, ctx: &egui::Context, path: PathBuf) {
        if path.is_file() && has_archive_extension(&path) {
            log::debug!("Dropped archive '{}'", path.display());
            self.start_load(ctx, path);
        } else {
            let err = format!(
                "File '{}' did not have an allowed file extension ({})",
                path.display(),
                ALLOWED_EXTENSIONS.join(", ")
            );
            log::error!("{err}");
            self.message("Invalid file type", err);
        }
    }

    /// Extract the dragged files to a temp folder and hand them to the OS drag loop.
    /// Blocks until the user drops or cancels, like the original's `DoDragDrop`.
    fn drag_out(&mut self, ctx: &egui::Context, frame: &eframe::Frame, paths: Vec<String>) {
        let dir = temp_dir().join("drag");
        let (files, failed) = extract::extract_for_drag(&self.vfs, &paths, &dir);
        if files.is_empty() {
            self.fail("Could not extract the file(s) to drag out".to_string());
            return;
        }
        log::debug!(
            "Drag dropping {} file(s): {}",
            files.len(),
            files
                .iter()
                .map(|f| f.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        if failed > 0 {
            self.set_status(format!(
                "{failed} file(s) could not be extracted for dragging"
            ));
        }
        match dragout::start(frame, files) {
            Ok(()) => {
                if dragout::BLOCKING {
                    self.release_pending = true;
                    ctx.request_repaint();
                }
            }
            Err(e) => self.fail(format!("Could not drag the file(s): {e}")),
        }
    }

    // ----- extraction ------------------------------------------------------

    fn export_dir_dialog(&self) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title("Select the folder to extract to");
        let last = Path::new(&self.store.settings.last_export_path);
        if last.is_dir() {
            dialog = dialog.set_directory(last);
        }
        dialog.pick_folder()
    }

    fn begin_extraction(&mut self, label: &str, status: String) {
        log::debug!("{status}");
        self.set_status(status);
        self.extracting = true;
        self.cancel.store(false, AtomicOrdering::Relaxed);
        self.busy = Some(label.to_string());
    }

    fn extract_folders_prompt(&mut self, ctx: &egui::Context, paths: Vec<String>) {
        if self.extracting {
            self.message(
                "ScsFileBrowser",
                "Cannot have multiple extractions running at once.",
            );
            return;
        }
        let Some(out) = self.export_dir_dialog() else {
            return;
        };
        self.store.settings.last_export_path = out.to_string_lossy().into_owned();
        self.store.save();

        self.begin_extraction(
            "Extracting folder",
            format!("Extracting folder(s) {}", paths.join(", ")),
        );
        let (vfs, cancel, tx, ctx) = (
            Arc::clone(&self.vfs),
            Arc::clone(&self.cancel),
            self.tx.clone(),
            ctx.clone(),
        );
        std::thread::spawn(move || {
            for path in &paths {
                let stats = extract_folder(&vfs, path, &out, &cancel);
                log::info!(
                    "Finished extracting folder '{path}' ({} file(s), {} failed)",
                    stats.extracted,
                    stats.failed
                );
            }
            let message = format!("Finished extracting folder(s) {}", paths.join(", "));
            log::info!("{message}");
            let _ = tx.send(TaskEvent::ExtractDone { message });
            ctx.request_repaint();
        });
    }

    fn extract_files_prompt(&mut self, ctx: &egui::Context, paths: Vec<String>) {
        if self.extracting {
            self.message(
                "ScsFileBrowser",
                "Cannot have multiple extractions running at once.",
            );
            return;
        }
        let Some(out) = self.export_dir_dialog() else {
            return;
        };
        self.store.settings.last_export_path = out.to_string_lossy().into_owned();
        self.store.save();

        self.begin_extraction("Extracting files", "Extracting files...".to_string());
        log::debug!("Extracting files ({})", paths.join(", "));
        let (vfs, cancel, tx, ctx) = (
            Arc::clone(&self.vfs),
            Arc::clone(&self.cancel),
            self.tx.clone(),
            ctx.clone(),
        );
        std::thread::spawn(move || {
            let stats = extract_files(&vfs, &paths, &out, &cancel);
            let message = format!("Finished extracting {} file(s).", stats.extracted);
            log::info!("{message}");
            let _ = tx.send(TaskEvent::ExtractDone { message });
            ctx.request_repaint();
        });
    }

    // ----- opening files with external programs ----------------------------

    fn open_files(&mut self, paths: &[String], program: Option<usize>) {
        if self.extracting {
            self.message(
                "ScsFileBrowser",
                "Cannot open file(s) when an extraction is running.",
            );
            return;
        }
        for path in paths {
            match program {
                Some(i) => {
                    if let Some(p) = self.store.settings.programs.get(i).cloned() {
                        self.open_file_with(path, &p);
                    }
                }
                None => self.open_file_default(path),
            }
        }
    }

    fn open_file_default(&mut self, path: &str) {
        let ext = extension(path).to_string();
        let setting = self
            .store
            .settings
            .extensions
            .iter()
            .find(|e| e.name == ext)
            .cloned();
        let Some(setting) = setting.filter(|e| !e.default_program.trim().is_empty()) else {
            self.set_status(format!(
                "Could not find default program for extension '{ext}', head over to the settings if you want to add it"
            ));
            log::error!("Could not find default program for extension '{ext}'");
            return;
        };
        let program = self
            .store
            .settings
            .programs
            .iter()
            .find(|p| p.name == setting.default_program)
            .cloned();
        match program {
            Some(p) => self.open_file_with(path, &p),
            None => self.fail(format!(
                "Could not find program '{}'",
                setting.default_program
            )),
        }
    }

    fn open_file_with(&mut self, path: &str, program: &Program) {
        let Some(file) = self.vfs.file(path) else {
            self.fail(format!("Could not find file '{path}'"));
            return;
        };
        if !Path::new(&program.path).exists() {
            self.fail(format!(
                "The executable ('{}') for program '{}' does not exist.",
                program.path, program.name
            ));
            return;
        }
        let out = match extract::extract_file(&file, &temp_dir(), path, None) {
            Ok(p) => p,
            Err(e) => {
                self.fail(format!("Could not extract the file '{path}': {e}"));
                return;
            }
        };
        match spawn_program(&program.path, &out) {
            Ok(()) => {
                self.set_status(format!("File '{path}' opened with '{}'", program.name));
                log::info!(
                    "File '{path}' opened with '{}' ('{}')",
                    program.name,
                    program.path
                );
            }
            Err(e) => self.fail(format!("Could not start '{}': {e}", program.name)),
        }
    }

    // ----- closing ---------------------------------------------------------

    fn request_close(&mut self, ctx: &egui::Context) {
        if self.extracting {
            if !self
                .dialogs
                .iter()
                .any(|d| matches!(d, Dialog::ConfirmQuitExtraction))
            {
                self.dialogs.push(Dialog::ConfirmQuitExtraction);
            }
        } else {
            self.close_after_extraction_check(ctx);
        }
    }

    fn close_after_extraction_check(&mut self, ctx: &egui::Context) {
        if temp_dir_has_files() {
            if !self
                .dialogs
                .iter()
                .any(|d| matches!(d, Dialog::ConfirmCleanup))
            {
                self.dialogs.push(Dialog::ConfirmCleanup);
            }
        } else {
            self.quit(ctx);
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        self.allow_close = true;
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }

    // ----- action dispatch --------------------------------------------------

    fn handle(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::OpenFileDialog => self.open_file_dialog(ctx),
            Action::OpenFolderDialog => self.open_folder_dialog(ctx),
            Action::DropArchive(p) => self.open_drop(ctx, p),
            Action::OpenRecent(p) => self.open_recent(ctx, &p),
            Action::OpenSettings => {
                if self.settings_win.is_none() {
                    self.settings_win = Some(SettingsWindow::default());
                }
            }
            Action::Navigate(path) => {
                if self.check_loading() {
                    return;
                }
                if !self.change_path(&path) {
                    log::debug!("The path '{path}' was not found in the current archive(s)");
                    self.path_tip = Some((Instant::now(), egui::Pos2::ZERO));
                }
            }
            Action::GoUp => {
                if !self.vfs.is_empty() {
                    self.go_up();
                }
            }
            Action::ActivateFolder(name) => {
                if self.check_loading() {
                    return;
                }
                let target = self.full_path(&name);
                self.change_path(&target);
            }
            Action::ActivateFile(name) => {
                if self.check_loading() {
                    return;
                }
                let path = self.full_path(&name);
                self.open_files(&[path], None);
            }
            Action::CopyText(text) => ctx.copy_text(text),
            Action::CopyFolderContents(paths) => {
                ctx.copy_text(folder_contents_as_text(&self.vfs, &paths));
                self.set_status("Copied folder contents to clipboard");
            }
            Action::ExtractFolders(paths) => {
                if !self.check_loading() {
                    self.extract_folders_prompt(ctx, paths);
                }
            }
            Action::ExtractFiles(paths) => {
                if !self.check_loading() {
                    self.extract_files_prompt(ctx, paths);
                }
            }
            Action::OpenFiles(paths, program) => {
                if !self.check_loading() {
                    self.open_files(&paths, program);
                }
            }
            Action::DragOut(paths) => {
                if self.check_loading() {
                    return;
                }
                if self.extracting {
                    self.message(
                        "ScsFileBrowser",
                        "Cannot extract file(s) when an extraction is running.",
                    );
                    return;
                }
                self.pending_drag = Some(paths);
            }
        }
    }

    fn poll_tasks(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                TaskEvent::Loaded { path, result } => self.finish_load(ctx, path, result),
                TaskEvent::ExtractDone { message } => {
                    self.extracting = false;
                    self.busy = None;
                    self.set_status(message);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// eframe integration
// ---------------------------------------------------------------------------

impl eframe::App for ScsApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_tasks(ctx);

        if let Some(path) = self.pending_open.take() {
            self.open_dropped(ctx, path);
        }

        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.request_close(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut actions: Vec<Action> = Vec::new();

        // Files dropped onto the window.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if let Some(first) = dropped.into_iter().next() {
            actions.push(Action::DropArchive(first));
        }

        // ----- top: menu -----
        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open File").clicked() {
                        actions.push(Action::OpenFileDialog);
                        ui.close();
                    }
                    if ui.button("Open Folder").clicked() {
                        actions.push(Action::OpenFolderDialog);
                        ui.close();
                    }
                    let recent = self.store.settings.recent_paths.clone();
                    ui.add_enabled_ui(!recent.is_empty(), |ui| {
                        ui.menu_button("Open Recent", |ui| {
                            for p in &recent {
                                if ui.button(p).clicked() {
                                    actions.push(Action::OpenRecent(p.clone()));
                                    ui.close();
                                }
                            }
                        });
                    });
                });
                if ui.button("Settings").clicked() {
                    actions.push(Action::OpenSettings);
                }
            });
        });

        // ----- top: path bar -----
        let path_rect = egui::Panel::top("pathbar")
            .frame(
                egui::Frame::new()
                    .fill(theme::BASE)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| {
                if let Some(ev) = self.path_bar.ui(ui, &self.current_path) {
                    match ev {
                        PathBarEvent::Navigate(p) => actions.push(Action::Navigate(p)),
                        PathBarEvent::CopyPath => {
                            actions.push(Action::CopyText(format!("/{}", self.current_path)));
                        }
                    }
                }
            })
            .response
            .rect;

        // ----- bottom: status bar -----
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme::BASE)
                    .inner_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if let Some(label) = &self.busy {
                        ui.add(egui::Spinner::new().size(16.0).color(theme::GOLD));
                        ui.label(RichText::new(label).color(theme::GOLD));
                        ui.separator();
                    }
                    ui.add(Label::new(&self.status).wrap_mode(TextWrapMode::Truncate));
                });
            });

        // ----- left: folders, centre: files -----
        egui::Panel::left("folders")
            .resizable(true)
            .default_size(340.0)
            .min_size(180.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| self.folder_table(ui, &mut actions));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::same(4)),
            )
            .show(ui, |ui| self.file_table(ui, &mut actions));

        // ----- keyboard -----
        let typing = ctx.egui_wants_keyboard_input();
        if !typing && self.dialogs.is_empty() && self.settings_win.is_none() {
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Backspace)) {
                actions.push(Action::GoUp);
            }
            if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
                match self.active {
                    ActiveList::Folders if self.folders.sel.len() == 1 => {
                        if let Some(n) = self.folders.selected_names().pop() {
                            actions.push(Action::ActivateFolder(n));
                        }
                    }
                    ActiveList::Files if self.files.sel.len() == 1 => {
                        if let Some(n) = self.files.selected_names().pop() {
                            actions.push(Action::ActivateFile(n));
                        }
                    }
                    _ => {}
                }
            }
        }

        // ----- overlays -----
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::unique("drop_overlay"),
            ));
            let screen = ctx.content_rect();
            painter.rect_filled(screen, 0.0, Color32::from_black_alpha(170));
            painter.text(
                screen.center(),
                egui::Align2::CENTER_CENTER,
                "Drop an archive (.scs, .zip, .mp) to open it",
                FontId::proportional(22.0),
                theme::TEXT,
            );
        }

        if let Some((since, _)) = self.path_tip {
            if since.elapsed() < Duration::from_secs(2) {
                egui::Area::new(Id::unique("path_tip"))
                    .order(egui::Order::Tooltip)
                    .fixed_pos(path_rect.left_bottom() + egui::vec2(20.0, 6.0))
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label("The selected path was not found in the archive(s).");
                        });
                    });
                ctx.request_repaint_after(Duration::from_millis(200));
            } else {
                self.path_tip = None;
            }
        }

        if let Some(win) = &mut self.settings_win {
            if !win.show(&ctx, &mut self.store) {
                self.settings_win = None;
            }
        }

        self.show_dialogs(&ctx);

        for action in actions {
            self.handle(&ctx, action);
        }

        if let Some(paths) = self.pending_drag.take() {
            self.drag_out(&ctx, frame, paths);
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The OS drag loop consumed the mouse-up that ended the drag; without it egui
        // would keep treating the button as held (no hover, stuck drag state).
        if std::mem::take(&mut self.release_pending) {
            if let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) {
                raw_input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

/// A clickable column header; returns whether it was clicked.
fn header_cell(ui: &mut egui::Ui, title: &str, sorted: Option<bool>) -> bool {
    let text = match sorted {
        Some(true) => format!("{title}  ^"),
        Some(false) => format!("{title}  v"),
        None => title.to_string(),
    };
    ui.add(Label::new(RichText::new(text).strong()).sense(Sense::click()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn apply_row_events<T, C: Copy + PartialEq>(model: &mut ListModel<T, C>, events: &[RowEvent]) {
    for ev in events {
        let names: Vec<String> = model
            .display_names()
            .into_iter()
            .map(str::to_string)
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        match ev {
            RowEvent::Click(i, mods) => model.sel.click(*i, &refs, *mods),
            RowEvent::RightClick(i) => model.sel.right_click(*i, &refs),
            RowEvent::Activate(_) | RowEvent::DragStart(_) => {}
        }
    }
}

impl ScsApp {
    fn folder_table(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let mods = ui.input(|i| i.modifiers);
        let shift = mods.shift;
        let active = self.active == ActiveList::Folders;
        let bg = ui.interact(
            ui.max_rect(),
            ui.scope_id().with("folders_bg"),
            Sense::click(),
        );

        let mut events: Vec<RowEvent> = Vec::new();
        let mut header_click: Option<FolderCol> = None;
        {
            let menu = MenuCtx {
                target: &self.folder_menu,
                extracting: self.extracting,
                current_path: &self.current_path,
                programs: &self.store.settings.programs,
            };
            let model = &self.folders;
            ui.scope(|ui| {
                ui.visuals_mut().selection.bg_fill = if active {
                    theme::PINE
                } else {
                    theme::HIGHLIGHT_MED
                };
                let sort = model.sort;
                TableBuilder::new(ui)
                    .id_salt("folder_table")
                    .sense(Sense::click())
                    .resizable(true)
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.0)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::initial(220.0).at_least(80.0).clip(true))
                    .column(Column::remainder().at_least(60.0))
                    .header(26.0, |mut header| {
                        header.col(|ui| {
                            let s = (sort.column == FolderCol::Name).then_some(sort.ascending);
                            if header_cell(ui, "Folder Name", s) {
                                header_click = Some(FolderCol::Name);
                            }
                        });
                        header.col(|ui| {
                            let s = (sort.column == FolderCol::SubItems).then_some(sort.ascending);
                            if header_cell(ui, "Sub Items", s) {
                                header_click = Some(FolderCol::SubItems);
                            }
                        });
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, model.order.len(), |mut row| {
                            let i = row.index();
                            let item = &model.items[model.order[i]];
                            row.set_selected(model.sel.contains(&item.name));
                            row.col(|ui| {
                                ui.add(Label::new(&item.name).truncate());
                            });
                            row.col(|ui| {
                                ui.add(Label::new(item.sub_entry_count.to_string()).truncate());
                            });
                            let resp = row.response();
                            if resp.clicked() {
                                events.push(RowEvent::Click(i, mods));
                            }
                            if resp.double_clicked() {
                                events.push(RowEvent::Activate(i));
                            }
                            if resp.secondary_clicked() {
                                events.push(RowEvent::RightClick(i));
                            }
                            resp.context_menu(|ui| folder_menu(ui, &menu, actions));
                        });
                    });
            });
            bg.context_menu(|ui| folder_menu(ui, &menu, actions));
        }

        if let Some(col) = header_click {
            self.folders.toggle_sort(col);
        }
        if !events.is_empty() {
            self.active = ActiveList::Folders;
        }
        for ev in &events {
            if let RowEvent::Activate(i) = ev {
                if let Some(&idx) = self.folders.order.get(*i) {
                    actions.push(Action::ActivateFolder(self.folders.items[idx].name.clone()));
                }
            }
        }
        apply_row_events(&mut self.folders, &events);

        if bg.clicked() {
            self.folders.sel.clear();
            self.active = ActiveList::Folders;
        }
        let right_clicked_row = events.iter().any(|e| matches!(e, RowEvent::RightClick(..)));
        if bg.secondary_clicked() && !right_clicked_row {
            self.folders.sel.clear();
        }
        if right_clicked_row || bg.secondary_clicked() {
            let selected = self.folders.selected_names();
            let paths = if selected.is_empty() {
                vec![self.current_path.clone()]
            } else {
                selected.iter().map(|n| self.full_path(n)).collect()
            };
            self.folder_menu = MenuTarget { paths, shift };
            self.active = ActiveList::Folders;
        }
    }

    fn file_table(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let mods = ui.input(|i| i.modifiers);
        let shift = mods.shift;
        let active = self.active == ActiveList::Files;
        let bg = ui.interact(
            ui.max_rect(),
            ui.scope_id().with("files_bg"),
            Sense::click(),
        );

        let mut events: Vec<RowEvent> = Vec::new();
        let mut header_click: Option<FileCol> = None;
        {
            let menu = MenuCtx {
                target: &self.file_menu,
                extracting: self.extracting,
                current_path: &self.current_path,
                programs: &self.store.settings.programs,
            };
            let model = &self.files;
            ui.scope(|ui| {
                ui.visuals_mut().selection.bg_fill = if active {
                    theme::PINE
                } else {
                    theme::HIGHLIGHT_MED
                };
                let sort = model.sort;
                TableBuilder::new(ui)
                    .id_salt("file_table")
                    .sense(if dragout::SUPPORTED {
                        Sense::click_and_drag()
                    } else {
                        Sense::click()
                    })
                    .resizable(true)
                    .auto_shrink([false, false])
                    .min_scrolled_height(0.0)
                    .cell_layout(Layout::left_to_right(Align::Center))
                    .column(Column::initial(280.0).at_least(80.0).clip(true))
                    .column(Column::initial(160.0).at_least(60.0).clip(true))
                    .column(Column::remainder().at_least(60.0))
                    .header(26.0, |mut header| {
                        for (col, title) in [
                            (FileCol::Name, "File Name"),
                            (FileCol::Source, "Source"),
                            (FileCol::Size, "Size"),
                        ] {
                            header.col(|ui| {
                                let s = (sort.column == col).then_some(sort.ascending);
                                if header_cell(ui, title, s) {
                                    header_click = Some(col);
                                }
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ROW_HEIGHT, model.order.len(), |mut row| {
                            let i = row.index();
                            let item = &model.items[model.order[i]];
                            row.set_selected(model.sel.contains(&item.name));
                            row.col(|ui| {
                                ui.add(Label::new(&item.name).truncate());
                            });
                            row.col(|ui| {
                                let r = ui.add(Label::new(&item.source).truncate());
                                if item.sources.len() > 1 {
                                    r.on_hover_text(item.sources.join("\n"));
                                }
                            });
                            row.col(|ui| {
                                ui.add(Label::new(format_size(item.size)).truncate());
                            });
                            let resp = row.response();
                            if resp.clicked() {
                                events.push(RowEvent::Click(i, mods));
                            }
                            if resp.double_clicked() {
                                events.push(RowEvent::Activate(i));
                            }
                            if resp.secondary_clicked() {
                                events.push(RowEvent::RightClick(i));
                            }
                            if dragout::SUPPORTED
                                && resp.drag_started_by(egui::PointerButton::Primary)
                            {
                                events.push(RowEvent::DragStart(i));
                            }
                            resp.context_menu(|ui| file_menu(ui, &menu, actions));
                        });
                    });
            });
        }

        if let Some(col) = header_click {
            self.files.toggle_sort(col);
        }
        if !events.is_empty() {
            self.active = ActiveList::Files;
        }
        for ev in &events {
            if let RowEvent::Activate(i) = ev {
                if let Some(&idx) = self.files.order.get(*i) {
                    actions.push(Action::ActivateFile(self.files.items[idx].name.clone()));
                }
            }
        }
        apply_row_events(&mut self.files, &events);
        if let Some(RowEvent::DragStart(i)) =
            events.iter().find(|e| matches!(e, RowEvent::DragStart(..)))
        {
            // Dragging an unselected row drags just that row; dragging part of a
            // selection drags the whole selection.
            let names: Vec<String> = self
                .files
                .display_names()
                .into_iter()
                .map(str::to_string)
                .collect();
            let refs: Vec<&str> = names.iter().map(String::as_str).collect();
            self.files.sel.right_click(*i, &refs);
            let paths = self
                .files
                .selected_names()
                .iter()
                .map(|n| self.full_path(n))
                .collect();
            actions.push(Action::DragOut(paths));
        }

        if bg.clicked() {
            self.files.sel.clear();
            self.active = ActiveList::Files;
        }
        if events.iter().any(|e| matches!(e, RowEvent::RightClick(..))) {
            let paths = self
                .files
                .selected_names()
                .iter()
                .map(|n| self.full_path(n))
                .collect();
            self.file_menu = MenuTarget { paths, shift };
        }
    }
}

// ---------------------------------------------------------------------------
// Context menus (same items as the original app; Shift reveals the extras)
// ---------------------------------------------------------------------------

fn plural<'a>(multiple: bool, one: &'a str, many: &'a str) -> &'a str {
    if multiple {
        many
    } else {
        one
    }
}

fn folder_menu(ui: &mut egui::Ui, m: &MenuCtx, actions: &mut Vec<Action>) {
    ui.style_mut().wrap_mode = Some(TextWrapMode::Extend); // menus size to their widest item
    let paths = &m.target.paths;
    if paths.is_empty() {
        ui.label("Nothing selected");
        return;
    }
    let multi = paths.len() > 1;

    if ui
        .button(plural(multi, "Copy Path", "Copy Paths"))
        .clicked()
    {
        actions.push(Action::CopyText(paths.join("\n")));
        ui.close();
    }
    if m.target.shift {
        if !m.extracting
            && ui
                .button("Copy sub folder/file names")
                .on_hover_text(format!(
                    "Copies the sub folder and file names for the {} to the clipboard",
                    plural(multi, "directory", "directories")
                ))
                .clicked()
        {
            actions.push(Action::CopyFolderContents(paths.clone()));
            ui.close();
        }
        if ui
            .button("Copy CityHash Value")
            .on_hover_text(format!(
                "Copies the cityhash value for the {} to the clipboard",
                plural(multi, "directory", "directories")
            ))
            .clicked()
        {
            actions.push(Action::CopyText(hash_lines(paths)));
            ui.close();
        }
    }
    if !m.extracting
        && ui
            .button(plural(multi, "Extract Folder", "Extract Folders"))
            .on_hover_text(format!(
                "Extracts the {} with the current path '{}' prefixed",
                plural(multi, "directory", "directories"),
                m.current_path
            ))
            .clicked()
    {
        actions.push(Action::ExtractFolders(paths.clone()));
        ui.close();
    }
}

fn file_menu(ui: &mut egui::Ui, m: &MenuCtx, actions: &mut Vec<Action>) {
    ui.style_mut().wrap_mode = Some(TextWrapMode::Extend); // menus size to their widest item
    let paths = &m.target.paths;
    if paths.is_empty() {
        ui.label("Nothing selected");
        return;
    }
    let multi = paths.len() > 1;

    if !m.extracting {
        if ui.button("Open").clicked() {
            actions.push(Action::OpenFiles(paths.clone(), None));
            ui.close();
        }
        if !m.programs.is_empty() {
            ui.menu_button("Open With", |ui| {
                for (i, program) in m.programs.iter().enumerate() {
                    if ui.button(&program.name).clicked() {
                        actions.push(Action::OpenFiles(paths.clone(), Some(i)));
                        ui.close();
                    }
                }
            });
        }
    }

    if ui
        .button(plural(multi, "Copy Path", "Copy Paths"))
        .clicked()
    {
        let text = paths
            .iter()
            .map(|p| format!("/{p}"))
            .collect::<Vec<_>>()
            .join("\n");
        actions.push(Action::CopyText(text));
        ui.close();
    }
    if m.target.shift
        && ui
            .button("Copy CityHash Value")
            .on_hover_text(format!(
                "Copies the cityhash value for the {} to the clipboard",
                plural(multi, "file", "files")
            ))
            .clicked()
    {
        actions.push(Action::CopyText(hash_lines(paths)));
        ui.close();
    }
    if !m.extracting
        && ui
            .button(plural(multi, "Extract File", "Extract Files"))
            .on_hover_text(format!(
                "Extracts the {} with the current path '{}' prefixed",
                plural(multi, "file", "files"),
                m.current_path
            ))
            .clicked()
    {
        actions.push(Action::ExtractFiles(paths.clone()));
        ui.close();
    }
}

fn hash_lines(paths: &[String]) -> String {
    paths
        .iter()
        .map(|p| format!("{:X}", city_hash64(p)))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Dialogs
// ---------------------------------------------------------------------------

impl ScsApp {
    fn show_dialogs(&mut self, ctx: &egui::Context) {
        if self.dialogs.is_empty() {
            return;
        }
        enum Outcome {
            None,
            Ok,
            Yes,
            No,
        }
        let mut outcome = Outcome::None;
        {
            let dialog = &self.dialogs[0];
            let r = Modal::new(Id::unique("app_dialog")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                match dialog {
                    Dialog::Message { title, text } => {
                        ui.heading(title);
                        ui.add_space(4.0);
                        ui.label(text);
                        ui.add_space(8.0);
                        if ui.button("OK").clicked() {
                            outcome = Outcome::Ok;
                        }
                    }
                    Dialog::ConfirmQuitExtraction => {
                        ui.heading("Stop extraction and close?");
                        ui.add_space(4.0);
                        ui.label("An extraction is still running, are you sure you want to quit?");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Yes").clicked() {
                                outcome = Outcome::Yes;
                            }
                            if ui.button("No").clicked() {
                                outcome = Outcome::No;
                            }
                        });
                    }
                    Dialog::ConfirmCleanup => {
                        ui.heading("Cleanup temporary files?");
                        ui.add_space(4.0);
                        ui.label("Do you want to remove all the temporarily extracted files?");
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("Yes").clicked() {
                                outcome = Outcome::Yes;
                            }
                            if ui.button("No").clicked() {
                                outcome = Outcome::No;
                            }
                        });
                    }
                }
            });
            if matches!(outcome, Outcome::None) && r.should_close() {
                outcome = match dialog {
                    Dialog::Message { .. } => Outcome::Ok,
                    _ => Outcome::No,
                };
            }
        }

        if matches!(outcome, Outcome::None) {
            return;
        }
        let dialog = self.dialogs.remove(0);
        match (dialog, outcome) {
            (Dialog::ConfirmQuitExtraction, Outcome::Yes) => {
                self.cancel.store(true, AtomicOrdering::Relaxed);
                self.close_after_extraction_check(ctx);
            }
            (Dialog::ConfirmCleanup, Outcome::Yes) => {
                cleanup_temp_dir();
                self.quit(ctx);
            }
            (Dialog::ConfirmCleanup, Outcome::No) => self.quit(ctx),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// OS helpers
// ---------------------------------------------------------------------------

fn temp_dir_has_files() -> bool {
    std::fs::read_dir(temp_dir())
        .map(|mut d| d.next().is_some())
        .unwrap_or(false)
}

fn cleanup_temp_dir() {
    let Ok(entries) = std::fs::read_dir(temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let res = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(e) = res {
            log::warn!("Could not remove '{}': {e}", path.display());
        }
    }
}

/// Start `program` with `file` as its argument without waiting for it.
fn spawn_program(program: &str, file: &Path) -> std::io::Result<()> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    let mut cmd = if program.ends_with(".app") {
        // Application bundles are directories; launch them through `open`.
        let mut c = Command::new("open");
        c.arg("-a").arg(program).arg(file);
        c
    } else {
        let mut c = Command::new(program);
        c.arg(file);
        c
    };
    #[cfg(not(target_os = "macos"))]
    let mut cmd = {
        let mut c = Command::new(program);
        c.arg(file);
        c
    };

    let mut child = cmd.spawn()?;
    // Reap the child when it exits so it doesn't linger as a zombie on Unix.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
