//! Readers for the SCS `HashFS` container (versions 1 and 2).
//!
//! Files and directories are addressed by the CityHash64 of their path. Every
//! directory has an entry whose contents list the names of its children, which
//! is how the tree is reconstructed for browsing.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use flate2::read::ZlibDecoder;

use super::archive::{
    archive_display_name, read_entry, Archive, ArchiveKind, Codec, Directory, Entry, Reader,
};
use crate::error::{Error, Result};

/// Upper bound for a decompressed table, to bound memory on corrupt input.
const MAX_TABLE_BYTES: u64 = 1 << 30;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

fn check_city(header: &[u8]) -> Result<()> {
    if &header[8..12] != b"CITY" {
        return Err(Error::Unsupported(format!(
            "hash method '{}' (only CITY is supported)",
            String::from_utf8_lossy(&header[8..12])
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Version 1
// ---------------------------------------------------------------------------

const V1_ENTRY_SIZE: u64 = 32;

pub(crate) fn open_v1(path: &Path, mut reader: Reader) -> Result<Archive> {
    let header = reader.read_at(0, 0x14)?;
    check_city(&header)?;
    let salt = u16_at(&header, 6);
    let entry_count = u32_at(&header, 12) as u64;
    let table_start = u32_at(&header, 16) as u64;

    let table = reader.read_at(table_start, entry_count * V1_ENTRY_SIZE)?;

    let mut files = HashMap::new();
    let mut dir_entries: Vec<(u64, Entry)> = Vec::new();
    for chunk in table.chunks_exact(V1_ENTRY_SIZE as usize) {
        let hash = u64_at(chunk, 0);
        let offset = u64_at(chunk, 8);
        let flags = u32_at(chunk, 16);
        // chunk[20..24] is a CRC32 of the data, which we don't verify.
        let size = u32_at(chunk, 24) as u64;
        let compressed_size = u32_at(chunk, 28) as u64;
        let is_dir = flags & 1 != 0;
        let is_compressed = flags & 2 != 0;
        let entry = Entry {
            offset,
            size,
            compressed_size,
            codec: if is_compressed {
                Codec::Zlib
            } else {
                Codec::Stored
            },
        };
        if is_dir {
            dir_entries.push((hash, entry));
        } else {
            files.insert(hash, entry);
        }
    }

    let mut dirs = HashMap::new();
    for (hash, entry) in dir_entries {
        match read_entry(&mut reader, &entry).and_then(|data| parse_v1_directory(&data)) {
            Ok(dir) => {
                merge_dir(&mut dirs, hash, dir);
            }
            Err(e) => log::warn!(
                "{}: skipping unreadable directory {hash:016X}: {e}",
                path.display()
            ),
        }
    }

    Ok(Archive {
        path: path.to_path_buf(),
        name: archive_display_name(path),
        kind: ArchiveKind::HashFsV1,
        salt,
        reader: Mutex::new(reader),
        files,
        dirs,
    })
}

/// V1 directory listings are newline separated; directories are prefixed `*`.
fn parse_v1_directory(data: &[u8]) -> Result<Directory> {
    let text = String::from_utf8_lossy(data);
    let mut dir = Directory::default();
    for line in text.split(['\r', '\n']).filter(|l| !l.is_empty()) {
        match line.strip_prefix('*') {
            Some(name) => dir.sub_dirs.push(name.to_string()),
            None => dir.files.push(line.to_string()),
        }
    }
    Ok(dir)
}

// ---------------------------------------------------------------------------
// Version 2
// ---------------------------------------------------------------------------

const V2_ENTRY_SIZE: usize = 16;
/// Metadata sizes use the low 28 bits; the top 4 bits carry flags.
const V2_SIZE_MASK: u32 = 0x0FFF_FFFF;

fn inflate_table(raw: &[u8], what: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ZlibDecoder::new(raw)
        .take(MAX_TABLE_BYTES)
        .read_to_end(&mut out)
        .map_err(|e| Error::Decompress(format!("{what}: {e}")))?;
    Ok(out)
}

pub(crate) fn open_v2(path: &Path, mut reader: Reader) -> Result<Archive> {
    let header = reader.read_at(0, 0x2C)?;
    check_city(&header)?;
    let salt = u16_at(&header, 6);
    let entry_count = u32_at(&header, 12) as usize;
    let entry_table_size = u32_at(&header, 0x10) as u64;
    let metadata_table_size = u32_at(&header, 0x18) as u64;
    let entry_table_offset = u64_at(&header, 0x1C);
    let metadata_table_offset = u64_at(&header, 0x24);

    let entries_raw = inflate_table(
        &reader.read_at(entry_table_offset, entry_table_size)?,
        "entry table",
    )?;
    if (entries_raw.len() as u64) < entry_count as u64 * V2_ENTRY_SIZE as u64 {
        return Err(Error::invalid(format!(
            "entry table holds {} bytes but {entry_count} entries were declared",
            entries_raw.len()
        )));
    }
    let metadata_raw = inflate_table(
        &reader.read_at(metadata_table_offset, metadata_table_size)?,
        "metadata table",
    )?;
    let meta: Vec<u32> = metadata_raw
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let mut files = HashMap::new();
    let mut dir_entries: Vec<(u64, Entry)> = Vec::new();
    for i in 0..entry_count {
        let raw = &entries_raw[i * V2_ENTRY_SIZE..(i + 1) * V2_ENTRY_SIZE];
        let hash = u64_at(raw, 0);
        let meta_index = u32_at(raw, 8) as usize;
        let flags = u16_at(raw, 14);
        let is_dir = flags & 1 != 0;

        // A plain-data metadata chunk is five 32-bit words:
        //   [0] header, [1] compressed size, [2] size, [3] unknown, [4] offset / 16
        let Some(chunk) = meta_index
            .checked_add(5)
            .and_then(|end| meta.get(meta_index..end))
        else {
            log::warn!(
                "{}: entry {hash:016X} points outside the metadata table (index {meta_index})",
                path.display()
            );
            continue;
        };
        let compressed_size = (chunk[1] & V2_SIZE_MASK) as u64;
        let size = (chunk[2] & V2_SIZE_MASK) as u64;
        let entry = Entry {
            offset: chunk[4] as u64 * 0x10,
            size,
            compressed_size,
            codec: if size != compressed_size {
                Codec::ZlibSkipHeader
            } else {
                Codec::Stored
            },
        };
        if is_dir {
            dir_entries.push((hash, entry));
        } else {
            files.insert(hash, entry);
        }
    }

    let mut dirs = HashMap::new();
    for (hash, entry) in dir_entries {
        match read_entry(&mut reader, &entry).and_then(|data| parse_v2_directory(&data)) {
            Ok(dir) => merge_dir(&mut dirs, hash, dir),
            Err(e) => log::warn!(
                "{}: skipping unreadable directory {hash:016X}: {e}",
                path.display()
            ),
        }
    }

    Ok(Archive {
        path: path.to_path_buf(),
        name: archive_display_name(path),
        kind: ArchiveKind::HashFsV2,
        salt,
        reader: Mutex::new(reader),
        files,
        dirs,
    })
}

/// V2 directory listing: `u32 count`, `count` length bytes, then the names
/// back to back. Directory names are prefixed with `/`.
fn parse_v2_directory(data: &[u8]) -> Result<Directory> {
    if data.len() < 4 {
        return Err(Error::invalid("directory listing too short"));
    }
    let count = u32_at(data, 0) as usize;
    let names_start = 4usize
        .checked_add(count)
        .filter(|&e| e <= data.len())
        .ok_or_else(|| Error::invalid("directory listing truncated"))?;
    let lengths = &data[4..names_start];
    let mut pos = names_start;
    let mut dir = Directory::default();
    for &len in lengths {
        let end = pos + len as usize;
        let bytes = data
            .get(pos..end)
            .ok_or_else(|| Error::invalid("directory name runs past end of listing"))?;
        pos = end;
        let name = String::from_utf8_lossy(bytes);
        match name.strip_prefix('/') {
            Some(sub) => dir.sub_dirs.push(sub.to_string()),
            None => dir.files.push(name.into_owned()),
        }
    }
    Ok(dir)
}

fn merge_dir(dirs: &mut HashMap<u64, Directory>, hash: u64, dir: Directory) {
    let slot = dirs.entry(hash).or_default();
    for d in dir.sub_dirs {
        if !slot.sub_dirs.contains(&d) {
            slot.sub_dirs.push(d);
        }
    }
    slot.files.extend(dir.files);
}
