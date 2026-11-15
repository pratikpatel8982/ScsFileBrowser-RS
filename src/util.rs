//! Small helpers: path handling, size formatting and SII decryption (3nK and `ScsC`).

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Normalise a path to the form archives use: forward slashes, no leading `/`.
pub fn ensure_local_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    p.trim_start_matches('/').to_string()
}

/// Join two archive paths (`Path.Combine` semantics of the original app).
pub fn combine_path(first: &str, second: &str) -> String {
    let second = ensure_local_path(second);
    if second.is_empty() {
        return ensure_local_path(first);
    }
    if first.is_empty() {
        return second;
    }
    let joined = if first.ends_with('/') || first.ends_with('\\') {
        format!("{first}{second}")
    } else {
        format!("{first}/{second}")
    };
    ensure_local_path(&joined)
}

/// Parent of an archive path; the parent of a top-level item is `""` (the root).
pub fn parent_path(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// The last component of an archive path.
pub fn file_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Extension including the leading dot (`"a/b.sii"` -> `".sii"`), or `""`.
pub fn extension(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(i) if i > 0 || name.len() > 1 => &name[i..],
        _ => "",
    }
}

/// Human readable size: bytes below 1 KiB, otherwise rounded KiB with
/// thousands separators (`1,234 KB`), matching the original app.
pub fn format_size(size: u64) -> String {
    if size >= 1024 {
        let kb = (size as f32 / 1024.0).round_ties_even() as u64;
        format!("{} KB", group_thousands(kb))
    } else {
        format!("{size} B")
    }
}

fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Join an archive-relative path onto `base`, refusing anything that could
/// escape `base` (`..`, absolute paths, drive prefixes, backslashes, NUL).
///
/// Archive contents are untrusted, so extraction must never write outside the
/// chosen output folder.
pub fn safe_join(base: &Path, relative: &str) -> Result<PathBuf> {
    let unsafe_err = || Error::UnsafePath(relative.to_string());
    if relative.contains('\0') || relative.contains('\\') || relative.contains(':') {
        return Err(unsafe_err());
    }
    let mut out = base.to_path_buf();
    let mut any = false;
    for part in relative.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(unsafe_err());
        }
        // Belt and braces: make sure the component is a plain name.
        let mut comps = Path::new(part).components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(_)), None) => {}
            _ => return Err(unsafe_err()),
        }
        out.push(part);
        any = true;
    }
    if !any {
        return Err(unsafe_err());
    }
    Ok(out)
}

/// Magic of an encrypted SII file: `"3nK\x01"` read as a little-endian u32.
pub const SII_3NK_MAGIC: u32 = 0x014B_6E33;

/// Whether `data` starts with the 3nK magic.
pub fn is_3nk(data: &[u8]) -> bool {
    data.len() >= 4 && u32::from_le_bytes([data[0], data[1], data[2], data[3]]) == SII_3NK_MAGIC
}

/// Decrypt a 3nK-encoded buffer (the simple XOR scheme used for binary
/// `.sii` files). Data without the 3nK magic is returned unchanged.
pub fn decrypt_3nk(src: &[u8]) -> Vec<u8> {
    // Header is 6 bytes: magic (4), a flag byte and the initial key.
    if src.len() < 6 || &src[0..3] != b"3nK" {
        return src.to_vec();
    }
    let mut key = src[5];
    let mut out = Vec::with_capacity(src.len() - 6);
    for &b in &src[6..] {
        let k = key as u32;
        let mixed = (((k << 2) ^ k ^ 0xff) << 3) ^ k ^ b as u32;
        out.push(mixed as u8);
        key = key.wrapping_add(1);
    }
    out
}

/// Magic of an AES-encrypted SII file (`ScsC`).
const SCSC_MAGIC: &[u8; 4] = b"ScsC";

/// `ScsC` header: magic (4), HMAC (32), IV (16) and the data size (4).
const SCSC_HEADER_LEN: usize = 56;

/// The AES-256 key all encrypted SII files use.
const SII_KEY: [u8; 32] = [
    0x2a, 0x5f, 0xcb, 0x17, 0x91, 0xd2, 0x2f, 0xb6, 0x02, 0x45, 0xb3, 0xd8, 0x36, 0x9e, 0xd0, 0xb2,
    0xc2, 0x73, 0x71, 0x56, 0x3f, 0xbf, 0x1f, 0x3c, 0x9e, 0xdf, 0x6b, 0x11, 0x82, 0x5a, 0x5d, 0x0a,
];

/// Whether `data` starts with the `ScsC` magic of an encrypted SII file.
pub fn is_scsc(data: &[u8]) -> bool {
    data.starts_with(SCSC_MAGIC)
}

/// Decrypt an `ScsC` file: AES-256-CBC with a fixed key, then a zlib stream.
pub fn decrypt_scsc(src: &[u8]) -> Result<Vec<u8>> {
    use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    if !is_scsc(src) || src.len() < SCSC_HEADER_LEN {
        return Err(Error::invalid("not an encrypted SII file"));
    }
    let iv = &src[36..52];
    let data_size = u32::from_le_bytes([src[52], src[53], src[54], src[55]]) as usize;
    let mut buf = src[SCSC_HEADER_LEN..].to_vec();
    if buf.len() % 16 != 0 {
        return Err(Error::invalid(
            "encrypted SII data is not a whole number of blocks",
        ));
    }
    cbc::Decryptor::<aes::Aes256>::new(&SII_KEY.into(), iv.into())
        .decrypt_padded_mut::<NoPadding>(&mut buf)
        .map_err(|_| Error::invalid("could not decrypt the SII file"))?;

    // Any padding after the zlib stream is ignored by the decoder.
    let mut out = Vec::with_capacity(data_size.min(256 * 1024 * 1024));
    ZlibDecoder::new(buf.as_slice())
        .read_to_end(&mut out)
        .map_err(|e| Error::Decompress(format!("encrypted SII: {e}")))?;
    Ok(out)
}

/// Turn an encrypted SII (`ScsC`, 3nK, or both layered) into its plain form. Anything else is
/// returned unchanged.
pub fn decode_sii(data: &[u8]) -> Result<Vec<u8>> {
    let mut current = data.to_vec();
    // A few layers at most; the limit only guards against a pathological file.
    for _ in 0..4 {
        if is_scsc(&current) {
            current = decrypt_scsc(&current)?;
        } else if is_3nk(&current) {
            current = decrypt_3nk(&current);
        } else {
            break;
        }
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(296), "296 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(5 * 1024 + 100), "5 KB");
        assert_eq!(format_size(1_234 * 1024), "1,234 KB");
        assert_eq!(format_size(12_345_678), "12,056 KB");
        // Ties round to even like .NET's Math.Round.
        assert_eq!(format_size(1536), "2 KB");
        assert_eq!(format_size(2560), "2 KB");
    }

    #[test]
    fn combine() {
        assert_eq!(combine_path("", "a"), "a");
        assert_eq!(combine_path("a", "b"), "a/b");
        assert_eq!(combine_path("a/", "b"), "a/b");
        assert_eq!(combine_path("a", "/b"), "a/b");
        assert_eq!(combine_path("a", ""), "a");
        assert_eq!(combine_path("a\\b", "c"), "a/b/c");
    }

    #[test]
    fn parents_and_names() {
        assert_eq!(parent_path("a/b/c"), "a/b");
        assert_eq!(parent_path("a"), "");
        assert_eq!(parent_path(""), "");
        assert_eq!(file_name("a/b/c.sii"), "c.sii");
        assert_eq!(extension("a/b/c.sii"), ".sii");
        assert_eq!(extension("a/b/noext"), "");
        assert_eq!(extension("a.b/noext"), "");
    }

    #[test]
    fn safe_join_rejects_traversal() {
        let base = Path::new("/out");
        assert_eq!(
            safe_join(base, "a/b.txt").unwrap(),
            Path::new("/out/a/b.txt")
        );
        assert!(safe_join(base, "../x").is_err());
        assert!(safe_join(base, "a/../../x").is_err());
        assert!(safe_join(base, "a\\..\\x").is_err());
        assert!(safe_join(base, "C:/x").is_err());
        assert!(safe_join(base, "").is_err());
        // Leading slash is just treated as relative.
        assert_eq!(safe_join(base, "/a").unwrap(), Path::new("/out/a"));
    }

    #[test]
    fn decrypt_roundtrip() {
        // Encrypt with the inverse of the cipher (it is a pure XOR with a
        // key stream, so encrypting == decrypting).
        let plain = b"SiiNunit\n{\n}\n";
        let mut enc = vec![0x33, 0x6E, 0x4B, 0x01, 0x00, 250];
        let mut key = 250u8;
        for &b in plain {
            let k = key as u32;
            enc.push(((((k << 2) ^ k ^ 0xff) << 3) ^ k ^ b as u32) as u8);
            key = key.wrapping_add(1); // wraps past 255 -> exercises wraparound
        }
        assert!(is_3nk(&enc));
        assert_eq!(decrypt_3nk(&enc), plain);
        assert_eq!(decrypt_3nk(b"plain text"), b"plain text");
        assert_eq!(decrypt_3nk(b"3nK"), b"3nK"); // too short: left alone
    }
}
