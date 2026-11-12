//! Builders for synthetic SCS archives used by the integration tests.
//!
//! The HashFS writers follow the on-disk layout the reader implements (header,
//! entry table, directory listings stored as entries). ZIPs are produced with
//! the independent `zip` crate so the ZIP reader is checked against a real writer.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::write::ZlibEncoder;
use flate2::Compression;
use scs_file_browser::cityhash::city_hash64;

/// One file to put in a test archive.
#[derive(Clone)]
pub struct TestFile {
    pub path: String,
    pub data: Vec<u8>,
    pub compress: bool,
}

pub fn file(path: &str, data: &[u8]) -> TestFile {
    TestFile {
        path: path.into(),
        data: data.to_vec(),
        compress: false,
    }
}

pub fn zfile(path: &str, data: &[u8]) -> TestFile {
    TestFile {
        path: path.into(),
        data: data.to_vec(),
        compress: true,
    }
}

/// Highly compressible filler of the given length.
pub fn filler(len: usize) -> Vec<u8> {
    (0..len).map(|i| b"abcdefgh"[i % 8]).collect()
}

/// Deterministic pseudo-random bytes (incompressible enough to stay large).
pub fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2654435761).wrapping_add(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}

pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Directory tree: dir path -> (sub dir names, file names).
type Tree = BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>;

fn build_tree(files: &[TestFile]) -> Tree {
    let mut tree: Tree = BTreeMap::new();
    tree.entry(String::new()).or_default();
    for f in files {
        let parts: Vec<&str> = f.path.split('/').collect();
        let mut dir = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i == parts.len() - 1 {
                tree.entry(dir.clone())
                    .or_default()
                    .1
                    .insert(part.to_string());
            } else {
                tree.entry(dir.clone())
                    .or_default()
                    .0
                    .insert(part.to_string());
                dir = if dir.is_empty() {
                    part.to_string()
                } else {
                    format!("{dir}/{part}")
                };
                tree.entry(dir.clone()).or_default();
            }
        }
    }
    tree
}

fn hash_for(salt: u16, path: &str) -> u64 {
    if salt != 0 {
        city_hash64(&format!("{salt}{path}"))
    } else {
        city_hash64(path)
    }
}

// ---------------------------------------------------------------------------
// HashFS v1
// ---------------------------------------------------------------------------

pub fn build_hashfs_v1(files: &[TestFile], salt: u16) -> Vec<u8> {
    let mut out = vec![0u8; 0x20]; // header, filled in at the end
    let mut entries: Vec<(u64, u64, u32, u32, u32)> = Vec::new(); // hash, offset, flags, size, csize

    for f in files {
        let offset = out.len() as u64;
        let (bytes, flags) = if f.compress {
            (zlib(&f.data), 2)
        } else {
            (f.data.clone(), 0)
        };
        out.extend_from_slice(&bytes);
        entries.push((
            hash_for(salt, &f.path),
            offset,
            flags,
            f.data.len() as u32,
            bytes.len() as u32,
        ));
    }

    for (i, (dir, (subs, fs))) in build_tree(files).iter().enumerate() {
        let mut text = String::new();
        for s in subs {
            text.push_str(&format!("*{s}\n"));
        }
        for f in fs {
            text.push_str(&format!("{f}\n"));
        }
        let offset = out.len() as u64;
        // Alternate compressed / stored directory listings to cover both.
        let (bytes, flags) = if i % 2 == 0 {
            (zlib(text.as_bytes()), 1 | 2)
        } else {
            (text.clone().into_bytes(), 1)
        };
        out.extend_from_slice(&bytes);
        entries.push((
            hash_for(salt, dir),
            offset,
            flags,
            text.len() as u32,
            bytes.len() as u32,
        ));
    }

    let table_start = out.len() as u32;
    for (hash, offset, flags, size, csize) in &entries {
        out.extend_from_slice(&hash.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // crc
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&csize.to_le_bytes());
    }

    out[0..4].copy_from_slice(b"SCS#");
    out[4..6].copy_from_slice(&1u16.to_le_bytes());
    out[6..8].copy_from_slice(&salt.to_le_bytes());
    out[8..12].copy_from_slice(b"CITY");
    out[12..16].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    out[16..20].copy_from_slice(&table_start.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// HashFS v2
// ---------------------------------------------------------------------------

fn pad16(out: &mut Vec<u8>) {
    while out.len() % 16 != 0 {
        out.push(0);
    }
}

pub fn build_hashfs_v2(files: &[TestFile], salt: u16) -> Vec<u8> {
    let mut out = vec![0u8; 0x30];

    // (hash, is_dir, offset, size, csize)
    let mut entries: Vec<(u64, bool, u64, u32, u32)> = Vec::new();

    for f in files {
        pad16(&mut out);
        let offset = out.len() as u64;
        let bytes = if f.compress {
            let z = zlib(&f.data);
            assert_ne!(
                z.len(),
                f.data.len(),
                "test data must change size when compressed"
            );
            z
        } else {
            f.data.clone()
        };
        out.extend_from_slice(&bytes);
        entries.push((
            hash_for(salt, &f.path),
            false,
            offset,
            f.data.len() as u32,
            bytes.len() as u32,
        ));
    }

    for (dir, (subs, fs)) in build_tree(files).iter() {
        // u32 count, `count` length bytes, then the names; dirs are '/'-prefixed.
        let mut names: Vec<String> = subs.iter().map(|s| format!("/{s}")).collect();
        names.extend(fs.iter().cloned());
        let mut listing = (names.len() as u32).to_le_bytes().to_vec();
        listing.extend(names.iter().map(|n| n.len() as u8));
        for n in &names {
            listing.extend_from_slice(n.as_bytes());
        }
        let z = zlib(&listing);
        pad16(&mut out);
        let offset = out.len() as u64;
        out.extend_from_slice(&z);
        entries.push((
            hash_for(salt, dir),
            true,
            offset,
            listing.len() as u32,
            z.len() as u32,
        ));
    }

    // Metadata table: five 32-bit words per entry (header, csize, size, unknown, offset/16).
    // The top 4 bits of the size words carry flags the reader must mask off.
    let mut meta = Vec::new();
    let mut entry_table = Vec::new();
    for (i, (hash, is_dir, offset, size, csize)) in entries.iter().enumerate() {
        let words = [
            (i as u32) | (1 << 24),
            *csize | 0x1000_0000,
            *size | 0x2000_0000,
            0,
            (*offset / 16) as u32,
        ];
        for w in words {
            meta.extend_from_slice(&w.to_le_bytes());
        }
        entry_table.extend_from_slice(&hash.to_le_bytes());
        entry_table.extend_from_slice(&((i * 5) as u32).to_le_bytes());
        entry_table.extend_from_slice(&1u16.to_le_bytes());
        entry_table.extend_from_slice(&(*is_dir as u16).to_le_bytes());
    }

    let entry_z = zlib(&entry_table);
    let meta_z = zlib(&meta);
    pad16(&mut out);
    let entry_off = out.len() as u64;
    out.extend_from_slice(&entry_z);
    pad16(&mut out);
    let meta_off = out.len() as u64;
    out.extend_from_slice(&meta_z);

    out[0..4].copy_from_slice(b"SCS#");
    out[4..6].copy_from_slice(&2u16.to_le_bytes());
    out[6..8].copy_from_slice(&salt.to_le_bytes());
    out[8..12].copy_from_slice(b"CITY");
    out[12..16].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    out[0x10..0x14].copy_from_slice(&(entry_z.len() as u32).to_le_bytes());
    out[0x14..0x18].copy_from_slice(&((entries.len() * 5) as u32).to_le_bytes());
    out[0x18..0x1C].copy_from_slice(&(meta_z.len() as u32).to_le_bytes());
    out[0x1C..0x24].copy_from_slice(&entry_off.to_le_bytes());
    out[0x24..0x2C].copy_from_slice(&meta_off.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// ZIP (via the `zip` crate)
// ---------------------------------------------------------------------------

/// Build a zip. `dir_entries` are written as explicit `name/` entries.
pub fn build_zip(files: &[TestFile], dir_entries: &[&str]) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for d in dir_entries {
        w.add_directory(*d, SimpleFileOptions::default()).unwrap();
    }
    for f in files {
        let method = if f.compress {
            CompressionMethod::Deflated
        } else {
            CompressionMethod::Stored
        };
        w.start_file(
            &f.path,
            SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        w.write_all(&f.data).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// Write `bytes` to `dir/name` and return the path.
pub fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}
