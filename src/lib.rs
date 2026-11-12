//! Core library of the Rust port of ScsFileBrowser: reading SCS (HashFS v1/v2)
//! and ZIP archives, merging them into one virtual file system, and extracting
//! files. The GUI lives in the `scs-file-browser` binary (feature `gui`).

pub mod cityhash;
pub mod error;
pub mod extract;
pub mod fs;
pub mod logging;
pub mod settings;
pub mod util;

#[cfg(feature = "gui")]
pub mod app;

pub use error::{Error, Result};
