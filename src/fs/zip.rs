//! Minimal ZIP reader (central directory only) for `.zip` / `.mp` mod archives.
//!
//! Paths are hashed with CityHash64 like HashFS so both formats share the same
//! lookup code. Directories that only exist implicitly (a file `a/b/c.txt`
//! without `a/` or `a/b/` entries) are created on demand.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::archive::{archive_display_name, Archive, ArchiveKind, Codec, Directory, Entry, Reader};
use crate::cityhash::city_hash64;
use crate::error::{Error, Result};
use crate::util::{ensure_local_path, parent_path};

const EOCD_SIG: u32 = 0x0605_4b50;
const CD_SIG: u32 = 0x0201_4b50;
const LOCAL_SIG: u32 = 0x0403_4b50;
const EOCD_MIN: usize = 22;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn open(path: &Path, mut reader: Reader) -> Result<Archive> {
    let len = reader.len();
    // The end-of-central-directory record sits within the last 64 KiB + 22 bytes
    // (the maximum comment length follows it).
    let tail_len = len.min(0xFFFF + EOCD_MIN as u64);
    let tail = reader.read_at(len - tail_len, tail_len)?;
    let eocd_pos = (0..=tail.len().saturating_sub(EOCD_MIN))
        .rev()
        .find(|&i| u32_at(&tail, i) == EOCD_SIG)
        .ok_or_else(|| Error::invalid("ZIP end-of-central-directory record not found"))?;
    let eocd = &tail[eocd_pos..];

    let entry_count = u16_at(eocd, 10) as usize;
    let cd_size = u32_at(eocd, 12) as u64;
    let cd_offset = u32_at(eocd, 16) as u64;
    if entry_count == 0xFFFF || cd_offset == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF {
        return Err(Error::Unsupported("ZIP64 archives".into()));
    }

    let cd = reader.read_at(cd_offset, cd_size)?;

    let mut builder = TreeBuilder::default();
    let mut files = HashMap::new();
    let mut pos = 0usize;
    for _ in 0..entry_count {
        if pos + 46 > cd.len() || u32_at(&cd, pos) != CD_SIG {
            return Err(Error::invalid("corrupt ZIP central directory"));
        }
        let flags = u16_at(&cd, pos + 8);
        let method = u16_at(&cd, pos + 10);
        let compressed_size = u32_at(&cd, pos + 20) as u64;
        let size = u32_at(&cd, pos + 24) as u64;
        let name_len = u16_at(&cd, pos + 28) as usize;
        let extra_len = u16_at(&cd, pos + 30) as usize;
        let comment_len = u16_at(&cd, pos + 32) as usize;
        let local_offset = u32_at(&cd, pos + 42) as u64;
        let name_bytes = cd
            .get(pos + 46..pos + 46 + name_len)
            .ok_or_else(|| Error::invalid("ZIP entry name runs past the central directory"))?;
        pos += 46 + name_len + extra_len + comment_len;

        let raw_name = String::from_utf8_lossy(name_bytes).replace('\\', "/");
        let is_dir = raw_name.ends_with('/');
        let name = ensure_local_path(&raw_name)
            .trim_end_matches('/')
            .to_string();
        if name.is_empty() {
            continue;
        }

        if is_dir {
            builder.ensure_dir(&name);
            continue;
        }

        if flags & 1 != 0 {
            log::warn!("{}: skipping encrypted entry '{name}'", path.display());
            continue;
        }

        // The extra field length in the local header may differ from the
        // central directory's, so read the local header to find the data.
        let local = reader.read_at(local_offset, 30)?;
        if u32_at(&local, 0) != LOCAL_SIG {
            log::warn!(
                "{}: bad local header for '{name}', skipping",
                path.display()
            );
            continue;
        }
        let data_offset = local_offset + 30 + u16_at(&local, 26) as u64 + u16_at(&local, 28) as u64;

        let codec = match method {
            0 => Codec::Stored,
            8 => Codec::Deflate,
            m => Codec::Unsupported(m),
        };
        let hash = city_hash64(&name);
        let entry = Entry {
            offset: data_offset,
            size,
            compressed_size,
            codec,
        };
        if files.insert(hash, entry).is_none() {
            builder.add_file(&name);
        }
    }

    Ok(Archive {
        path: path.to_path_buf(),
        name: archive_display_name(path),
        kind: ArchiveKind::Zip,
        salt: 0,
        reader: Mutex::new(reader),
        files,
        dirs: builder.dirs,
    })
}

#[derive(Default)]
struct TreeBuilder {
    dirs: HashMap<u64, Directory>,
}

impl TreeBuilder {
    /// Make sure `path` and all of its ancestors exist, linking each new
    /// directory into its parent.
    fn ensure_dir(&mut self, path: &str) {
        let hash = city_hash64(path);
        if self.dirs.contains_key(&hash) {
            return;
        }
        self.dirs.insert(hash, Directory::default());
        if path.is_empty() {
            return;
        }
        let parent = parent_path(path);
        self.ensure_dir(parent);
        let child = &path[parent.len()..].trim_start_matches('/');
        if let Some(p) = self.dirs.get_mut(&city_hash64(parent)) {
            p.sub_dirs.push((*child).to_string());
        }
    }

    fn add_file(&mut self, path: &str) {
        let parent = parent_path(path);
        self.ensure_dir(parent);
        let name = &path[parent.len()..].trim_start_matches('/');
        if let Some(p) = self.dirs.get_mut(&city_hash64(parent)) {
            p.files.push((*name).to_string());
        }
    }
}
