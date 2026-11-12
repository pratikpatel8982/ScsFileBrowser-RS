// Don't open a console window alongside the GUI on Windows release builds.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    scs_file_browser::logging::init();
    log::info!("ScsFileBrowser {} starting", env!("CARGO_PKG_VERSION"));

    // An optional archive file or folder to open at startup.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    let result = scs_file_browser::app::run(initial);

    if let Err(e) = &result {
        log::error!("The application exited with an error: {e}");
    }
    result
}
