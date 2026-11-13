// Don't open a console window alongside the GUI on Windows release builds.
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;

/// winit only delivers dropped files on its X11 backend, never on Wayland. When both
/// are available, run through X11 (XWayland) so drag & drop works. Set
/// `SCS_FORCE_WAYLAND=1` to keep native Wayland.
#[cfg(target_os = "linux")]
fn prefer_x11_for_drag_and_drop() -> bool {
    use std::env::{remove_var, var_os};
    if var_os("WAYLAND_DISPLAY").is_some()
        && var_os("DISPLAY").is_some()
        && var_os("SCS_FORCE_WAYLAND").is_none()
    {
        remove_var("WAYLAND_DISPLAY");
        remove_var("WAYLAND_SOCKET");
        return true;
    }
    false
}

fn main() -> eframe::Result<()> {
    // Must run before any other thread starts (it edits the environment).
    #[cfg(target_os = "linux")]
    let forced_x11 = prefer_x11_for_drag_and_drop();

    scs_file_browser::logging::init();
    #[cfg(target_os = "linux")]
    if forced_x11 {
        log::info!("Wayland session detected: using X11 (XWayland) so file drag & drop works");
    }
    log::info!("ScsFileBrowser {} starting", env!("CARGO_PKG_VERSION"));

    // An optional archive file or folder to open at startup.
    let initial = std::env::args_os().nth(1).map(PathBuf::from);
    let result = scs_file_browser::app::run(initial);

    if let Err(e) = &result {
        log::error!("The application exited with an error: {e}");
    }
    result
}
