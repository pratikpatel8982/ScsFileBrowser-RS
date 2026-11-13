//! Dragging files *out* of the window into the file manager / another app.
//!
//! The original app starts an OS file drag from the file list (`ItemDrag`). Here
//! the selected files are extracted to a temp folder first and then handed to the
//! OS drag machinery:
//!
//! * Windows / macOS: the `drag` crate (blocks until the drop finishes).
//! * Linux / BSD: our own X11 XDND source in [`super::xdnd`] (runs on a thread).

/// Whether this platform can start a drag out of the window.
pub const SUPPORTED: bool = cfg!(unix) || cfg!(windows);

/// Whether starting the drag blocks in the OS drag loop. That loop swallows the
/// mouse-up, so the app has to tell egui about it afterwards.
pub const BLOCKING: bool = cfg!(any(windows, target_os = "macos"));

#[cfg(any(windows, target_os = "macos"))]
pub fn start(frame: &eframe::Frame, files: Vec<std::path::PathBuf>) -> Result<(), String> {
    // A small preview image shown next to the cursor while dragging.
    const PREVIEW: &[u8] = include_bytes!("../../assets/drag_icon.png");

    let count = files.len();
    drag::start_drag(
        frame,
        drag::DragItem::Files(files),
        drag::Image::Raw(PREVIEW.to_vec()),
        move |result, _cursor| log::debug!("Drag of {count} file(s) finished: {result:?}"),
        drag::Options::default(),
    )
    .map_err(|e| e.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn start(frame: &eframe::Frame, files: Vec<std::path::PathBuf>) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let own_window = match frame.window_handle().map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Xlib(h)) => h.window as u32,
        Ok(RawWindowHandle::Xcb(h)) => h.window.get(),
        _ => {
            return Err(
                "dragging files out needs an X11 window (this one isn't running on X11)".into(),
            )
        }
    };
    super::xdnd::start(own_window, files)
}
