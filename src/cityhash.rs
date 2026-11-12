//! CityHash64, as used by SCS archives to hash paths.
//!
//! Ported from the C# implementation in the original ScsFileBrowser (itself a
//! port of Google's CityHash, MIT licensed):
//!
//! ```text
//! Copyright (c) 2011 Google, Inc.
//! CityHash, by Geoff Pike and Jyrki Alakuijala
//! ```
//!
//! The hash is computed over the raw (UTF-8) bytes of the path.

// Some primes between 2^63 and 2^64 for various uses.
const K0: u64 = 0xc3a5c85c97cb3127;
const K1: u64 = 0xb492b66fbe98f273;
const K2: u64 = 0x9ae16a3b2f90404f;
const K3: u64 = 0xc949d7c7509e6557;

#[inline]
fn fetch64(s: &[u8], pos: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&s[pos..pos + 8]);
    u64::from_le_bytes(b)
}

#[inline]
fn fetch32(s: &[u8], pos: usize) -> u64 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&s[pos..pos + 4]);
    u32::from_le_bytes(b) as u64
}

/// Bitwise right rotate.
#[inline]
fn rotate(val: u64, shift: u32) -> u64 {
    val.rotate_right(shift)
}

#[inline]
fn shift_mix(val: u64) -> u64 {
    val ^ (val >> 47)
}

/// Hash 128 input bits down to 64 bits of output (Murmur-inspired).
#[inline]
fn hash128_to_64(first: u64, second: u64) -> u64 {
    const K_MUL: u64 = 0x9ddfea08eb382d69;
    let mut a = (first ^ second).wrapping_mul(K_MUL);
    a ^= a >> 47;
    let mut b = (second ^ a).wrapping_mul(K_MUL);
    b ^= b >> 47;
    b.wrapping_mul(K_MUL)
}

#[inline]
fn hash_len16(u: u64, v: u64) -> u64 {
    hash128_to_64(u, v)
}

fn hash_len_0_to_16(s: &[u8]) -> u64 {
    let len = s.len();
    let len64 = len as u64;
    if len > 8 {
        let a = fetch64(s, 0);
        let b = fetch64(s, len - 8);
        return hash_len16(a, rotate(b.wrapping_add(len64), len as u32)) ^ b;
    }
    if len >= 4 {
        let a = fetch32(s, 0);
        return hash_len16(len64.wrapping_add(a << 3), fetch32(s, len - 4));
    }
    if len > 0 {
        let a = s[0] as u32;
        let b = s[len >> 1] as u32;
        let c = s[len - 1] as u32;
        let y = a.wrapping_add(b << 8) as u64;
        let z = len64.wrapping_add((c as u64) << 2);
        return shift_mix((y.wrapping_mul(K2)) ^ (z.wrapping_mul(K3))).wrapping_mul(K2);
    }
    K2
}

fn hash_len_17_to_32(s: &[u8]) -> u64 {
    let len = s.len();
    let a = fetch64(s, 0).wrapping_mul(K1);
    let b = fetch64(s, 8);
    let c = fetch64(s, len - 8).wrapping_mul(K2);
    let d = fetch64(s, len - 16).wrapping_mul(K0);
    hash_len16(
        rotate(a.wrapping_sub(b), 43)
            .wrapping_add(rotate(c, 30))
            .wrapping_add(d),
        a.wrapping_add(rotate(b ^ K3, 20))
            .wrapping_sub(c)
            .wrapping_add(len as u64),
    )
}

fn hash_len_33_to_64(s: &[u8]) -> u64 {
    let len = s.len();
    let len64 = len as u64;
    let mut z = fetch64(s, 24);
    let mut a =
        fetch64(s, 0).wrapping_add(len64.wrapping_add(fetch64(s, len - 16)).wrapping_mul(K0));
    let mut b = rotate(a.wrapping_add(z), 52);
    let mut c = rotate(a, 37);
    a = a.wrapping_add(fetch64(s, 8));
    c = c.wrapping_add(rotate(a, 7));
    a = a.wrapping_add(fetch64(s, 16));
    let vf = a.wrapping_add(z);
    let vs = b.wrapping_add(rotate(a, 31)).wrapping_add(c);
    a = fetch64(s, 16).wrapping_add(fetch64(s, len - 32));
    z = fetch64(s, len - 8);
    b = rotate(a.wrapping_add(z), 52);
    c = rotate(a, 37);
    a = a.wrapping_add(fetch64(s, len - 24));
    c = c.wrapping_add(rotate(a, 7));
    a = a.wrapping_add(fetch64(s, len - 16));
    let wf = a.wrapping_add(z);
    let ws = b.wrapping_add(rotate(a, 31)).wrapping_add(c);
    let r = shift_mix(
        vf.wrapping_add(ws)
            .wrapping_mul(K2)
            .wrapping_add(wf.wrapping_add(vs).wrapping_mul(K0)),
    );
    shift_mix(r.wrapping_mul(K0).wrapping_add(vs)).wrapping_mul(K2)
}

/// Return a 16-byte hash for 48 bytes. Quick and dirty.
#[inline]
fn weak_hash_len32_with_seeds_raw(
    w: u64,
    x: u64,
    y: u64,
    z: u64,
    mut a: u64,
    mut b: u64,
) -> (u64, u64) {
    a = a.wrapping_add(w);
    b = rotate(b.wrapping_add(a).wrapping_add(z), 21);
    let c = a;
    a = a.wrapping_add(x);
    a = a.wrapping_add(y);
    b = b.wrapping_add(rotate(a, 44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

/// Return a 16-byte hash for `s[pos..pos + 32]`, `a`, and `b`. Quick and dirty.
#[inline]
fn weak_hash_len32_with_seeds(s: &[u8], pos: usize, a: u64, b: u64) -> (u64, u64) {
    weak_hash_len32_with_seeds_raw(
        fetch64(s, pos),
        fetch64(s, pos + 8),
        fetch64(s, pos + 16),
        fetch64(s, pos + 24),
        a,
        b,
    )
}

/// CityHash64 of a byte string.
pub fn city_hash64_bytes(s: &[u8]) -> u64 {
    let len = s.len();
    if len <= 16 {
        return hash_len_0_to_16(s);
    }
    if len <= 32 {
        return hash_len_17_to_32(s);
    }
    if len <= 64 {
        return hash_len_33_to_64(s);
    }

    // For strings over 64 bytes we hash the end first, and then as we loop we
    // keep 56 bytes of state: v, w, x, y, and z.
    let len64 = len as u64;
    let mut x = fetch64(s, len - 40);
    let mut y = fetch64(s, len - 16).wrapping_add(fetch64(s, len - 56));
    let mut z = hash_len16(
        fetch64(s, len - 48).wrapping_add(len64),
        fetch64(s, len - 24),
    );
    let mut v = weak_hash_len32_with_seeds(s, len - 64, len64, z);
    let mut w = weak_hash_len32_with_seeds(s, len - 32, y.wrapping_add(K1), x);
    x = x.wrapping_mul(K1).wrapping_add(fetch64(s, 0));

    let mut pos = 0usize;
    // Decrease len to the nearest multiple of 64, and operate on 64-byte chunks.
    let mut remaining = (len - 1) & !63usize;
    loop {
        x = rotate(
            x.wrapping_add(y)
                .wrapping_add(v.0)
                .wrapping_add(fetch64(s, pos + 8)),
            37,
        )
        .wrapping_mul(K1);
        y = rotate(y.wrapping_add(v.1).wrapping_add(fetch64(s, pos + 48)), 42).wrapping_mul(K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(fetch64(s, pos + 40));
        z = rotate(z.wrapping_add(w.0), 33).wrapping_mul(K1);
        v = weak_hash_len32_with_seeds(s, pos, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak_hash_len32_with_seeds(
            s,
            pos + 32,
            z.wrapping_add(w.1),
            y.wrapping_add(fetch64(s, pos + 16)),
        );
        std::mem::swap(&mut z, &mut x);

        pos += 64;
        remaining -= 64;
        if remaining == 0 {
            break;
        }
    }

    hash_len16(
        hash_len16(v.0, w.0)
            .wrapping_add(shift_mix(y).wrapping_mul(K1))
            .wrapping_add(z),
        hash_len16(v.1, w.1).wrapping_add(x),
    )
}

/// CityHash64 of a string's UTF-8 bytes. This is how SCS archives hash paths.
pub fn city_hash64(s: &str) -> u64 {
    city_hash64_bytes(s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_k2() {
        // The root directory of every unsalted archive is stored under this hash.
        assert_eq!(city_hash64(""), 0x9ae16a3b2f90404f);
    }

    #[test]
    fn all_length_branches_run() {
        // Exercise every branch (0, 1-3, 4-8, 9-16, 17-32, 33-64, 65+, and
        // multi-block) for panics and make sure distinct inputs hash differently.
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut seen = std::collections::HashSet::new();
        for len in 0..=data.len() {
            assert!(
                seen.insert(city_hash64_bytes(&data[..len])),
                "collision at len {len}"
            );
        }
    }
}
