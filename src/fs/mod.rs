//! Archive readers and the merged virtual file system.

mod archive;
mod hashfs;
mod vfs;
mod zip;

pub use archive::{
    has_archive_extension, Archive, ArchiveKind, Codec, Directory, Entry, ALLOWED_EXTENSIONS,
};
pub use vfs::{FileInfo, FileRef, FolderInfo, Listing, LoadReport, Vfs};
