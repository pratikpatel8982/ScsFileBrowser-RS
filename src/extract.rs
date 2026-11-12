//! Extracting files and folders out of a [`Vfs`] onto disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};
use crate::fs::{FileRef, Vfs};
use crate::util::{combine_path, decrypt_3nk, is_3nk, safe_join};

/// Folder (inside the system temp dir) where "open with" and drag-out files go.
pub fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("ScsFileBrowser")
}

/// Maximum folder nesting followed when extracting. Guards against cyclic or
/// absurdly deep (malicious) directory listings.
const MAX_DEPTH: usize = 128;

/// Write one file to `out_dir`.
///
/// The file is stored under `custom_name` when given (it may contain `/` to
/// create sub-folders), otherwise as `original_path` with `/` replaced by `+`.
/// Encrypted (3nK) SII files are decrypted on the way out.
pub fn extract_file(
    file: &FileRef,
    out_dir: &Path,
    original_path: &str,
    custom_name: Option<&str>,
) -> Result<PathBuf> {
    let relative = match custom_name {
        Some(name) => name.to_string(),
        None => original_path.replace('/', "+"),
    };
    let out_path = safe_join(out_dir, &relative)?;

    let bytes = file.read()?;
    let bytes = if is_3nk(&bytes) {
        decrypt_3nk(&bytes)
    } else {
        bytes
    };

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, bytes)?;
    Ok(out_path)
}

/// Outcome of extracting a folder tree.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExtractStats {
    pub extracted: usize,
    pub failed: usize,
}

impl ExtractStats {
    fn merge(&mut self, other: ExtractStats) {
        self.extracted += other.extracted;
        self.failed += other.failed;
    }
}

/// Recursively extract the folder `path`, preserving its structure under
/// `out_dir`. Checks `cancel` between files so a long extraction can be aborted.
pub fn extract_folder(vfs: &Vfs, path: &str, out_dir: &Path, cancel: &AtomicBool) -> ExtractStats {
    extract_folder_inner(vfs, path, out_dir, cancel, 0)
}

fn extract_folder_inner(
    vfs: &Vfs,
    path: &str,
    out_dir: &Path,
    cancel: &AtomicBool,
    depth: usize,
) -> ExtractStats {
    let mut stats = ExtractStats::default();
    if depth > MAX_DEPTH {
        log::error!("Folder '{path}' is nested too deeply, skipping");
        stats.failed += 1;
        return stats;
    }
    let dirs = vfs.directory_entries(path);
    if dirs.is_empty() {
        log::error!("Could not find folder '{path}'");
        stats.failed += 1;
        return stats;
    }

    let mut seen = std::collections::HashSet::new();
    for sub in dirs.iter().flat_map(|d| d.sub_dirs.iter()) {
        if !seen.insert(sub.as_str()) {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            return stats;
        }
        stats.merge(extract_folder_inner(
            vfs,
            &combine_path(path, sub),
            out_dir,
            cancel,
            depth + 1,
        ));
    }

    seen.clear();
    for name in dirs.iter().flat_map(|d| d.files.iter()) {
        if !seen.insert(name.as_str()) {
            continue;
        }
        if cancel.load(Ordering::Relaxed) {
            return stats;
        }
        let file_path = combine_path(path, name);
        let Some(file) = vfs.file(&file_path) else {
            log::error!("Could not find file '{file_path}'");
            stats.failed += 1;
            continue;
        };
        match extract_file(&file, out_dir, &file_path, Some(&file_path)) {
            Ok(_) => stats.extracted += 1,
            Err(e) => {
                log::error!("Could not extract the file '{file_path}': {e}");
                stats.failed += 1;
            }
        }
    }
    stats
}

/// Extract the given files (flat list of full paths) into `out_dir`, keeping
/// their archive paths as sub-folders.
pub fn extract_files(
    vfs: &Vfs,
    paths: &[String],
    out_dir: &Path,
    cancel: &AtomicBool,
) -> ExtractStats {
    let mut stats = ExtractStats::default();
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(file) = vfs.file(path) else {
            log::error!("Could not find file '{path}'");
            stats.failed += 1;
            continue;
        };
        match extract_file(&file, out_dir, path, Some(path)) {
            Ok(_) => stats.extracted += 1,
            Err(e) => {
                log::error!("Could not extract the file '{path}': {e}");
                stats.failed += 1;
            }
        }
    }
    stats
}

/// Text for "Copy sub folder/file names": a header per folder followed by its
/// sorted sub-folders and files as absolute archive paths. When the mounted
/// archives look like ETS2/ATS, the game and version are included in the header.
pub fn folder_contents_as_text(vfs: &Vfs, paths: &[String]) -> String {
    let game_info = game_info(vfs);
    let mut out = String::new();
    let mut first = true;
    for path in paths {
        let dirs = vfs.directory_entries(path);
        if dirs.is_empty() {
            continue;
        }
        if !first {
            out.push('\n');
        }
        first = false;

        out.push_str(&format!("=== Contents of '{path}'"));
        if let Some(info) = &game_info {
            out.push_str(&format!(" for {info}"));
        }
        out.push_str(" ===\n");

        let mut sub_dirs: Vec<&String> = dirs.iter().flat_map(|d| d.sub_dirs.iter()).collect();
        sub_dirs.sort();
        sub_dirs.dedup();
        for d in sub_dirs {
            out.push_str(&format!("/{}\n", combine_path(path, d)));
        }

        let mut files: Vec<&String> = dirs.iter().flat_map(|d| d.files.iter()).collect();
        files.sort();
        files.dedup();
        for f in files {
            out.push_str(&format!("/{}\n", combine_path(path, f)));
        }
    }
    out
}

fn game_info(vfs: &Vfs) -> Option<String> {
    let version_file = vfs.file("version.txt")?;
    let is_usa = vfs.file("map/usa.mbd").is_some();
    let is_eu = vfs.file("map/europe.mbd").is_some();
    if !is_usa && !is_eu {
        return None;
    }
    let version = version_file.read().ok()?;
    let version = String::from_utf8_lossy(&version);
    let game = if is_usa {
        "American Truck Simulator"
    } else {
        "Euro Truck Simulator 2"
    };
    Some(format!("{game} (v{})", version.trim_end_matches('\n')))
}

/// Convenience used by the GUI: error text for a missing file.
pub fn not_found(path: &str) -> Error {
    Error::NotFound(path.to_string())
}
