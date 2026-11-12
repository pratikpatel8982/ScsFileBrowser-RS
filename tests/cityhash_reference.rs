//! Cross-check of our CityHash64 against an independent C implementation.
//!
//! `tests/data/cityhash_vectors.txt` holds `<hex input> <hex hash>` lines
//! (`-` for the empty input), produced by compiling `city.c` from
//! <https://github.com/nusov/cityhash-c> (the C port the original C# app was
//! derived from) and hashing every input with its `CityHash64`.
//!
//! Note: this is the CityHash 1.0.3 algorithm. Neither v1.0.2 (different
//! long-input loop) nor v1.1 (different short-input mixing) gives the same
//! values, so those can't be used as references.

use scs_file_browser::cityhash::city_hash64_bytes;

fn unhex(s: &str) -> Vec<u8> {
    if s == "-" {
        return Vec::new();
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn matches_reference_implementation() {
    let text = include_str!("data/cityhash_vectors.txt");
    let mut n = 0;
    for (i, line) in text.lines().enumerate() {
        let (input, expected) = line.split_once(' ').unwrap();
        let expected = u64::from_str_radix(expected, 16).unwrap();
        let input = unhex(input);
        assert_eq!(
            city_hash64_bytes(&input),
            expected,
            "line {} (input length {})",
            i + 1,
            input.len()
        );
        n += 1;
    }
    assert!(n > 500, "expected the full vector set, got {n}");
}
