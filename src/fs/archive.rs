//! Format-independent archive model shared by HashFS and ZIP readers.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flate2::read::{DeflateDecoder, ZlibDecoder};

use crate::cityhash::city_hash64;
use crate::error::{Error, Result};
use crate::util::ensure_local_path;

/// File extensions (with leading dot, lower case) that can be opened as archives.
pub const ALLOWED_EXTENSIONS: &[&str] = &[".scs", ".zip", ".mp"];

/// Whether `path` has an extension that [`ALLOWED_EXTENSIONS`] allows.
pub fn has_archive_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let dotted = format!(".{}", e.to_ascii_lowercase());
            ALLOWED_EXTENSIONS.contains(&dotted.as_str())
        })
        .unwrap_or(false)
}

/// Which on-disk format an archive uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchiveKind {
    HashFsV1,
    HashFsV2,
    Zip,
}

/// How an entry's bytes are encoded on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// Stored as-is.
    Stored,
    /// zlib stream (2 byte header + deflate + adler32). HashFS v1.
    Zlib,
    /// zlib stream whose 2 byte header is skipped and the rest inflated raw. HashFS v2.
    ZlibSkipHeader,
    /// Raw deflate. ZIP.
    Deflate,
    /// A compression method we cannot decode (carries the method id).
    Unsupported(u16),
}

/// Location and encoding of one file inside an archive.
#[derive(Clone, Debug)]
pub struct Entry {
    pub offset: u64,
    /// Uncompressed size.
    pub size: u64,
    pub compressed_size: u64,
    pub codec: Codec,
}

/// A directory listing: names of the sub-directories and files it contains.
#[derive(Clone, Debug, Default)]
pub struct Directory {
    pub sub_dirs: Vec<String>,
    pub files: Vec<String>,
}

/// Random-access reader over the archive file with bounds checking.
pub(crate) struct Reader {
    file: File,
    len: u64,
}

impl Reader {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }

    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    pub(crate) fn read_at(&mut self, offset: u64, len: u64) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .ok_or_else(|| Error::invalid("offset overflow"))?;
        if end > self.len {
            return Err(Error::invalid(format!(
                "read of {len} bytes at offset {offset} runs past the end of the archive ({} bytes)",
                self.len
            )));
        }
        self.file.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }
}

/// Inflate `raw` according to `codec`, expecting exactly `expected` bytes.
pub(crate) fn inflate(raw: &[u8], codec: Codec, expected: u64) -> Result<Vec<u8>> {
    const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
    if raw.starts_with(&ZSTD_MAGIC) {
        return Err(Error::Unsupported("zstd compressed entries".into()));
    }
    let mut out = Vec::with_capacity(expected.min(256 * 1024 * 1024) as usize);
    let res = match codec {
        Codec::Stored => return Ok(raw.to_vec()),
        Codec::Zlib => ZlibDecoder::new(raw).take(expected).read_to_end(&mut out),
        Codec::ZlibSkipHeader => {
            if raw.len() < 2 {
                return Err(Error::Decompress("compressed data too short".into()));
            }
            DeflateDecoder::new(&raw[2..])
                .take(expected)
                .read_to_end(&mut out)
        }
        Codec::Deflate => DeflateDecoder::new(raw)
            .take(expected)
            .read_to_end(&mut out),
        Codec::Unsupported(m) => {
            return Err(Error::Unsupported(format!("compression method {m}")));
        }
    };
    res.map_err(|e| Error::Decompress(e.to_string()))?;
    if out.len() as u64 != expected {
        return Err(Error::Decompress(format!(
            "expected {expected} bytes but got {}",
            out.len()
        )));
    }
    Ok(out)
}

/// Read and decode an entry from `reader`.
pub(crate) fn read_entry(reader: &mut Reader, entry: &Entry) -> Result<Vec<u8>> {
    let raw = reader.read_at(entry.offset, entry.compressed_size)?;
    if entry.codec == Codec::Stored {
        return Ok(raw);
    }
    inflate(&raw, entry.codec, entry.size)
}

/// An opened archive: a lookup table of files and directories by path hash.
pub struct Archive {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) kind: ArchiveKind,
    pub(crate) salt: u16,
    pub(crate) reader: Mutex<Reader>,
    pub(crate) files: HashMap<u64, Entry>,
    pub(crate) dirs: HashMap<u64, Directory>,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Archive")
            .field("path", &self.path)
            .field("kind", &self.kind)
            .field("files", &self.files.len())
            .field("dirs", &self.dirs.len())
            .finish()
    }
}

impl Archive {
    /// Open an archive, detecting its format from its contents.
    pub fn open(path: impl AsRef<Path>) -> Result<Archive> {
        let path = path.as_ref();
        let mut reader = Reader::open(path)?;
        let head = reader.read_at(0, reader.len().min(8))?;
        if head.starts_with(b"SCS#") {
            let version = u16::from_le_bytes([head[4], head[5]]);
            match version {
                1 => super::hashfs::open_v1(path, reader),
                2 => super::hashfs::open_v2(path, reader),
                v => Err(Error::Unsupported(format!("HashFS version {v}"))),
            }
        } else if head.starts_with(b"PK") {
            super::zip::open(path, reader)
        } else {
            Err(Error::invalid("not an SCS (HashFS) or ZIP archive"))
        }
    }

    /// Display name of the archive: the file name without its extension.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> ArchiveKind {
        self.kind
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Hash under which `path` is stored in this archive.
    pub fn path_hash(&self, path: &str) -> u64 {
        let local = normalize(path);
        if self.salt != 0 {
            city_hash64(&format!("{}{}", self.salt, local))
        } else {
            city_hash64(&local)
        }
    }

    pub fn directory(&self, path: &str) -> Option<&Directory> {
        self.dirs.get(&self.path_hash(path))
    }

    pub fn file_entry(&self, path: &str) -> Option<&Entry> {
        self.files.get(&self.path_hash(path))
    }

    /// Read and decode the contents of `entry`.
    pub fn read_entry(&self, entry: &Entry) -> Result<Vec<u8>> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| Error::invalid("archive reader poisoned"))?;
        read_entry(&mut reader, entry)
    }
}

/// Normalise a lookup path: local form without a trailing slash.
pub(crate) fn normalize(path: &str) -> String {
    let local = ensure_local_path(path);
    local.trim_end_matches('/').to_string()
}

pub(crate) fn archive_display_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}
