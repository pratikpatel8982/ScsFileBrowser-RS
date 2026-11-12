//! The "uber" file system: several archives mounted on top of each other.
//!
//! Directory listings are merged across archives and a file present in more
//! than one archive resolves to the *last* mounted one (later archives
//! override earlier ones, like the games do).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::archive::{has_archive_extension, normalize, Archive, Directory, Entry};
use crate::error::Result;
use crate::util::combine_path;

/// A file inside a particular archive.
#[derive(Clone, Debug)]
pub struct FileRef {
    archive: Arc<Archive>,
    entry: Entry,
}

impl FileRef {
    pub fn read(&self) -> Result<Vec<u8>> {
        self.archive.read_entry(&self.entry)
    }

    /// Uncompressed size in bytes.
    pub fn size(&self) -> u64 {
        self.entry.size
    }

    /// Display name of the archive this file lives in.
    pub fn archive_name(&self) -> &str {
        self.archive.name()
    }

    pub fn archive(&self) -> &Arc<Archive> {
        &self.archive
    }
}

/// A sub-folder row in a [`Listing`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderInfo {
    pub name: String,
    /// Distinct files + sub-folders inside, across all archives.
    pub sub_entry_count: usize,
}

/// A file row in a [`Listing`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub name: String,
    /// Winning archive, with `(+N)` appended when others also contain the file.
    pub source: String,
    /// Size of the winning (last mounted) copy.
    pub size: u64,
    /// Every archive containing the file, in mount order.
    pub sources: Vec<String>,
}

/// The merged contents of one directory.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub folders: Vec<FolderInfo>,
    pub files: Vec<FileInfo>,
}

/// Result of mounting every archive found in a folder.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub loaded: usize,
    pub failed: Vec<(PathBuf, String)>,
}

#[derive(Debug, Default)]
pub struct Vfs {
    archives: Vec<Arc<Archive>>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn archives(&self) -> &[Arc<Archive>] {
        &self.archives
    }

    pub fn is_empty(&self) -> bool {
        self.archives.is_empty()
    }

    pub fn add_archive(&mut self, archive: Archive) {
        self.archives.push(Arc::new(archive));
    }

    /// Mount a single archive file.
    pub fn add_source_file(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let archive = Archive::open(path)?;
        self.add_archive(archive);
        Ok(())
    }

    /// Mount every archive (`.scs`, `.zip`, `.mp`) directly inside `dir`, in
    /// case-insensitive name order. Archives that fail to open are reported but
    /// don't stop the others from loading.
    pub fn add_source_directory(&mut self, dir: impl AsRef<Path>) -> Result<LoadReport> {
        let mut paths: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && has_archive_extension(p))
            .collect();
        paths.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));

        let mut report = LoadReport::default();
        for path in paths {
            match Archive::open(&path) {
                Ok(a) => {
                    self.add_archive(a);
                    report.loaded += 1;
                }
                Err(e) => {
                    log::error!("Could not load '{}': {e}", path.display());
                    report.failed.push((path, e.to_string()));
                }
            }
        }
        Ok(report)
    }

    /// The directory at `path` in every archive that has it.
    pub fn directory_entries(&self, path: &str) -> Vec<&Directory> {
        let path = normalize(path);
        self.archives
            .iter()
            .filter_map(|a| a.directory(&path))
            .collect()
    }

    /// The file at `path` in every archive that has it, in mount order.
    pub fn file_entries(&self, path: &str) -> Vec<FileRef> {
        let path = normalize(path);
        self.archives
            .iter()
            .filter_map(|a| {
                a.file_entry(&path).map(|e| FileRef {
                    archive: Arc::clone(a),
                    entry: e.clone(),
                })
            })
            .collect()
    }

    /// The copy of the file that wins (last mounted archive).
    pub fn file(&self, path: &str) -> Option<FileRef> {
        self.file_entries(path).pop()
    }

    /// Merged listing of `path`, or `None` when no archive has such a directory.
    pub fn list(&self, path: &str) -> Option<Listing> {
        let path = normalize(path);
        let dirs = self.directory_entries(&path);
        if dirs.is_empty() {
            return None;
        }

        let mut listing = Listing::default();

        for name in distinct(dirs.iter().flat_map(|d| d.sub_dirs.iter())) {
            let sub_path = combine_path(&path, &name);
            let sub_dirs = self.directory_entries(&sub_path);
            if sub_dirs.is_empty() {
                log::error!("Could not find folder '{name}' in '{path}'");
                continue;
            }
            let count = distinct(
                sub_dirs
                    .iter()
                    .flat_map(|d| d.files.iter().chain(d.sub_dirs.iter())),
            )
            .len();
            listing.folders.push(FolderInfo {
                name,
                sub_entry_count: count,
            });
        }

        for name in distinct(dirs.iter().flat_map(|d| d.files.iter())) {
            let file_path = combine_path(&path, &name);
            let copies = self.file_entries(&file_path);
            let Some(last) = copies.last() else {
                log::error!("Could not find file '{name}' in '{path}'");
                continue;
            };
            let mut source = last.archive_name().to_string();
            if copies.len() > 1 {
                source.push_str(&format!(" (+{})", copies.len() - 1));
            }
            listing.files.push(FileInfo {
                name,
                source,
                size: last.size(),
                sources: copies
                    .iter()
                    .map(|c| c.archive_name().to_string())
                    .collect(),
            });
        }

        Some(listing)
    }
}

/// Unique items, keeping first-seen order.
fn distinct<'a>(items: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut seen = HashSet::new();
    items.filter(|s| seen.insert(s.as_str())).cloned().collect()
}
