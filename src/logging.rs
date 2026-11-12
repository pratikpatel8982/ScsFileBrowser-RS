//! File + stderr logger built on the `log` facade.

use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};

use crate::settings::app_data_dir;

struct FileLogger {
    file: Mutex<Option<File>>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        // Our own crate logs down to DEBUG; dependencies (winit, egui, wgpu...) are
        // very chatty at DEBUG/INFO, so only let their warnings and errors through.
        let ours = metadata.target().starts_with("scs_file_browser")
            || metadata.target().starts_with("scs-file-browser");
        let limit = if ours {
            LevelFilter::Debug
        } else {
            LevelFilter::Warn
        };
        metadata.level() <= limit
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{}|{:>5}] [{}] {}\n",
            chrono::Local::now().format("%H:%M:%S"),
            record.level(),
            record.target(),
            record.args()
        );
        eprint!("{line}");
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// Install the logger. Writes `log.log` in the app data folder, falling back to
/// `log-2.log`, `log-3.log`, ... when another running instance holds the file.
pub fn init() {
    let dir = app_data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = (1..=10).find_map(|i| {
        let name = if i == 1 {
            "log.log".to_string()
        } else {
            format!("log-{i}.log")
        };
        File::create(dir.join(name)).ok()
    });
    // The logger lives for the whole process, so leaking it is the simplest
    // way to get the `&'static dyn Log` that `set_logger` wants.
    let logger: &'static FileLogger = Box::leak(Box::new(FileLogger {
        file: Mutex::new(file),
    }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(LevelFilter::Debug);
    }
}
