mod common;

use std::sync::atomic::AtomicBool;

use common::*;
use scs_file_browser::extract::{
    extract_file, extract_files, extract_folder, folder_contents_as_text, ExtractStats,
};
use scs_file_browser::fs::{Archive, ArchiveKind, Vfs};
use scs_file_browser::util::SII_3NK_MAGIC;

/// A small tree used by most tests.
fn sample() -> Vec<TestFile> {
    vec![
        file("version.txt", b"1.50.1.0\n"),
        zfile("def/readme.txt", &filler(400)),
        file("def/vehicle/truck/a.sii", b"SiiNunit {}"),
        zfile("def/vehicle/truck/b.sii", &filler(1000)),
        file("def/vehicle/trailer/c.sii", b"trailer"),
        file("map/europe.mbd", b"map"),
    ]
}

fn check_sample(a: &Archive) {
    let root = a.directory("").expect("root dir");
    assert_eq!(sorted(&root.sub_dirs), ["def", "map"]);
    assert_eq!(root.files, ["version.txt"]);

    let truck = a.directory("def/vehicle/truck").expect("truck dir");
    assert_eq!(sorted(&truck.files), ["a.sii", "b.sii"]);

    // Trailing / leading slashes and backslashes resolve to the same thing.
    assert!(a.directory("/def/vehicle/").is_some());
    assert!(a.directory("def\\vehicle").is_some());
    assert!(a.directory("def/nope").is_none());

    for (path, expected) in [
        ("version.txt", b"1.50.1.0\n".to_vec()),
        ("def/readme.txt", filler(400)),
        ("def/vehicle/truck/a.sii", b"SiiNunit {}".to_vec()),
        ("def/vehicle/truck/b.sii", filler(1000)),
    ] {
        let entry = a
            .file_entry(path)
            .unwrap_or_else(|| panic!("missing {path}"));
        assert_eq!(entry.size as usize, expected.len(), "{path}");
        assert_eq!(a.read_entry(entry).unwrap(), expected, "{path}");
    }
    assert!(a.file_entry("def/missing.sii").is_none());
}

fn sorted(v: &[String]) -> Vec<&str> {
    let mut v: Vec<&str> = v.iter().map(String::as_str).collect();
    v.sort();
    v
}

#[test]
fn hashfs_v1_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "base.scs", &build_hashfs_v1(&sample(), 0));
    let a = Archive::open(&path).unwrap();
    assert_eq!(a.kind(), ArchiveKind::HashFsV1);
    assert_eq!(a.name(), "base");
    check_sample(&a);
}

#[test]
fn hashfs_v1_with_salt() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "salted.scs", &build_hashfs_v1(&sample(), 1234));
    let a = Archive::open(&path).unwrap();
    check_sample(&a);
}

#[test]
fn hashfs_v2_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "base.scs", &build_hashfs_v2(&sample(), 0));
    let a = Archive::open(&path).unwrap();
    assert_eq!(a.kind(), ArchiveKind::HashFsV2);
    check_sample(&a);
}

#[test]
fn hashfs_v2_with_salt() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "salted.scs", &build_hashfs_v2(&sample(), 77));
    check_sample(&Archive::open(&path).unwrap());
}

/// The original app masked v2 sizes to 16 bits, truncating anything over 64 KiB.
#[test]
fn hashfs_v2_large_files_are_not_truncated() {
    let big = noise(300_000, 7);
    let big_compressed = filler(200_000);
    let files = vec![
        file("data/big.bin", &big),
        zfile("data/big_z.bin", &big_compressed),
    ];
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "big.scs", &build_hashfs_v2(&files, 0));
    let a = Archive::open(&path).unwrap();

    let e = a.file_entry("data/big.bin").unwrap();
    assert_eq!(e.size, 300_000);
    assert_eq!(a.read_entry(e).unwrap(), big);

    let e = a.file_entry("data/big_z.bin").unwrap();
    assert_eq!(e.size, 200_000);
    assert_eq!(a.read_entry(e).unwrap(), big_compressed);
}

#[test]
fn hashfs_v1_large_files() {
    let big = noise(300_000, 9);
    let files = vec![file("x/big.bin", &big), zfile("x/z.bin", &filler(150_000))];
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::open(write(dir.path(), "b.scs", &build_hashfs_v1(&files, 0))).unwrap();
    assert_eq!(
        a.read_entry(a.file_entry("x/big.bin").unwrap()).unwrap(),
        big
    );
    assert_eq!(
        a.read_entry(a.file_entry("x/z.bin").unwrap()).unwrap(),
        filler(150_000)
    );
}

#[test]
fn long_paths_over_64_bytes_resolve() {
    // Exercises the CityHash long-input branch end to end.
    let long = "vehicle/truck/upgrade/paintjob/scania.streamline/accessory/a_really_long_file_name_here.sii";
    assert!(long.len() > 64);
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::open(write(
        dir.path(),
        "l.scs",
        &build_hashfs_v1(&[file(long, b"x")], 0),
    ))
    .unwrap();
    assert_eq!(a.read_entry(a.file_entry(long).unwrap()).unwrap(), b"x");
    let a = Archive::open(write(
        dir.path(),
        "l2.scs",
        &build_hashfs_v2(&[file(long, b"y")], 0),
    ))
    .unwrap();
    assert_eq!(a.read_entry(a.file_entry(long).unwrap()).unwrap(), b"y");
}

#[test]
fn zip_roundtrip_with_explicit_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = build_zip(
        &sample(),
        &[
            "def/",
            "def/vehicle/",
            "def/vehicle/truck/",
            "def/vehicle/trailer/",
            "map/",
        ],
    );
    let a = Archive::open(write(dir.path(), "mod.zip", &bytes)).unwrap();
    assert_eq!(a.kind(), ArchiveKind::Zip);
    check_sample(&a);
}

/// Zips frequently lack entries for parent folders; they must still be browsable.
#[test]
fn zip_roundtrip_with_implicit_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let a = Archive::open(write(dir.path(), "mod.mp", &build_zip(&sample(), &[]))).unwrap();
    check_sample(&a);
}

#[test]
fn zip_empty_files_are_files_not_folders() {
    let dir = tempfile::tempdir().unwrap();
    let files = vec![file("a/empty.txt", b""), file("a/full.txt", b"hi")];
    let a = Archive::open(write(dir.path(), "z.zip", &build_zip(&files, &[]))).unwrap();
    let d = a.directory("a").unwrap();
    assert_eq!(sorted(&d.files), ["empty.txt", "full.txt"]);
    assert!(d.sub_dirs.is_empty());
    assert_eq!(
        a.read_entry(a.file_entry("a/empty.txt").unwrap()).unwrap(),
        b""
    );
}

#[test]
fn rejects_non_archives() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Archive::open(write(dir.path(), "a.scs", b"hello world, not an archive")).is_err());
    assert!(Archive::open(write(dir.path(), "b.scs", b"")).is_err());
    assert!(Archive::open(dir.path().join("missing.scs")).is_err());
    // Future / unknown HashFS version.
    let mut v9 = build_hashfs_v1(&sample(), 0);
    v9[4] = 9;
    assert!(Archive::open(write(dir.path(), "c.scs", &v9)).is_err());
}

/// Corrupt or truncated archives must produce errors, never panics.
#[test]
fn corrupt_archives_never_panic() {
    let dir = tempfile::tempdir().unwrap();
    let archives = [
        ("v1", build_hashfs_v1(&sample(), 0)),
        ("v2", build_hashfs_v2(&sample(), 0)),
        ("zip", build_zip(&sample(), &["def/"])),
    ];
    for (name, bytes) in &archives {
        // Truncate at many points.
        for cut in (0..bytes.len()).step_by(7) {
            let p = write(dir.path(), &format!("{name}_cut.scs"), &bytes[..cut]);
            let _ = Archive::open(&p);
        }
        // Flip bytes across the file (header, tables and data), then also use the result.
        for i in (0..bytes.len()).step_by(3) {
            let mut b = bytes.clone();
            b[i] ^= 0xFF;
            let p = write(dir.path(), &format!("{name}_flip.scs"), &b);
            if let Ok(a) = Archive::open(&p) {
                for path in [
                    "",
                    "def",
                    "def/vehicle/truck",
                    "version.txt",
                    "def/readme.txt",
                ] {
                    if let Some(d) = a.directory(path) {
                        let _ = d.files.len();
                    }
                    if let Some(e) = a.file_entry(path) {
                        let _ = a.read_entry(e);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Virtual file system
// ---------------------------------------------------------------------------

fn vfs_with_overrides() -> (tempfile::TempDir, Vfs) {
    let dir = tempfile::tempdir().unwrap();
    // base is a HashFS v1, dlc a v2, patch a zip -- mixed formats mounted together.
    write(
        dir.path(),
        "a_base.scs",
        &build_hashfs_v1(
            &[
                file("def/shared.sii", b"base"),
                file("def/only_base.sii", b"ob"),
                file("def/sub/x.sii", b"x"),
            ],
            0,
        ),
    );
    write(
        dir.path(),
        "b_dlc.scs",
        &build_hashfs_v2(
            &[file("def/shared.sii", b"dlc!"), file("def/sub/y.sii", b"y")],
            0,
        ),
    );
    write(
        dir.path(),
        "C_patch.zip",
        &build_zip(&[zfile("def/shared.sii", &filler(300))], &[]),
    );
    // Not an archive despite the extension: reported, doesn't stop the rest.
    write(dir.path(), "d_broken.scs", b"garbage");
    // Wrong extension: ignored entirely.
    write(dir.path(), "notes.txt", b"ignore me");

    let mut vfs = Vfs::new();
    let report = vfs.add_source_directory(dir.path()).unwrap();
    assert_eq!(report.loaded, 3);
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].0.ends_with("d_broken.scs"));
    (dir, vfs)
}

#[test]
fn vfs_mounts_in_name_order_and_last_wins() {
    let (_dir, vfs) = vfs_with_overrides();
    let names: Vec<&str> = vfs.archives().iter().map(|a| a.name()).collect();
    assert_eq!(names, ["a_base", "b_dlc", "C_patch"]); // case-insensitive order

    let copies = vfs.file_entries("def/shared.sii");
    assert_eq!(copies.len(), 3);
    let winner = vfs.file("def/shared.sii").unwrap();
    assert_eq!(winner.archive_name(), "C_patch");
    assert_eq!(winner.read().unwrap(), filler(300));
}

#[test]
fn vfs_listing_merges_archives() {
    let (_dir, vfs) = vfs_with_overrides();
    let list = vfs.list("def").unwrap();

    let sub = list.folders.iter().find(|f| f.name == "sub").unwrap();
    assert_eq!(sub.sub_entry_count, 2); // x.sii (base) + y.sii (dlc)

    let shared = list.files.iter().find(|f| f.name == "shared.sii").unwrap();
    assert_eq!(shared.source, "C_patch (+2)");
    assert_eq!(shared.size, 300);
    assert_eq!(shared.sources, ["a_base", "b_dlc", "C_patch"]);

    let only = list
        .files
        .iter()
        .find(|f| f.name == "only_base.sii")
        .unwrap();
    assert_eq!(only.source, "a_base"); // no (+N) suffix for a single source
    assert_eq!(list.files.len(), 2);

    assert!(vfs.list("def/does/not/exist").is_none());
    assert!(vfs.list("").is_some());
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

fn vfs_of(files: &[TestFile]) -> (tempfile::TempDir, Vfs) {
    let dir = tempfile::tempdir().unwrap();
    let p = write(dir.path(), "t.scs", &build_hashfs_v2(files, 0));
    let mut vfs = Vfs::new();
    vfs.add_source_file(p).unwrap();
    (dir, vfs)
}

#[test]
fn extract_single_file_naming() {
    let (_d, vfs) = vfs_of(&sample());
    let out = tempfile::tempdir().unwrap();
    let f = vfs.file("def/vehicle/truck/a.sii").unwrap();

    // Default: path flattened with '+'.
    let p = extract_file(&f, out.path(), "def/vehicle/truck/a.sii", None).unwrap();
    assert_eq!(p.file_name().unwrap(), "def+vehicle+truck+a.sii");
    assert_eq!(std::fs::read(&p).unwrap(), b"SiiNunit {}");

    // Custom name keeps the folder structure.
    let p = extract_file(&f, out.path(), "x", Some("def/vehicle/truck/a.sii")).unwrap();
    assert_eq!(p, out.path().join("def/vehicle/truck/a.sii"));
    assert!(p.is_file());
}

#[test]
fn extract_folder_preserves_structure() {
    let (_d, vfs) = vfs_of(&sample());
    let out = tempfile::tempdir().unwrap();
    let stats = extract_folder(&vfs, "def", out.path(), &AtomicBool::new(false));
    assert_eq!(
        stats,
        ExtractStats {
            extracted: 4,
            failed: 0
        }
    );
    assert_eq!(
        std::fs::read(out.path().join("def/readme.txt")).unwrap(),
        filler(400)
    );
    assert_eq!(
        std::fs::read(out.path().join("def/vehicle/truck/b.sii")).unwrap(),
        filler(1000)
    );
    assert!(out.path().join("def/vehicle/trailer/c.sii").is_file());
    assert!(!out.path().join("version.txt").exists());

    // Whole archive from the root.
    let out2 = tempfile::tempdir().unwrap();
    let stats = extract_folder(&vfs, "", out2.path(), &AtomicBool::new(false));
    assert_eq!(stats.extracted, 6);
    assert!(out2.path().join("map/europe.mbd").is_file());
}

#[test]
fn extract_files_list_and_missing() {
    let (_d, vfs) = vfs_of(&sample());
    let out = tempfile::tempdir().unwrap();
    let stats = extract_files(
        &vfs,
        &[
            "version.txt".into(),
            "def/readme.txt".into(),
            "nope.txt".into(),
        ],
        out.path(),
        &AtomicBool::new(false),
    );
    assert_eq!(
        stats,
        ExtractStats {
            extracted: 2,
            failed: 1
        }
    );
    assert!(out.path().join("version.txt").is_file());
}

#[test]
fn extract_can_be_cancelled() {
    let (_d, vfs) = vfs_of(&sample());
    let out = tempfile::tempdir().unwrap();
    let stats = extract_folder(&vfs, "", out.path(), &AtomicBool::new(true));
    assert_eq!(stats.extracted, 0);
}

#[test]
fn extract_empty_files() {
    let (_d, vfs) = vfs_of(&[file("a/empty.txt", b"")]);
    let out = tempfile::tempdir().unwrap();
    let stats = extract_folder(&vfs, "a", out.path(), &AtomicBool::new(false));
    assert_eq!(stats.extracted, 1);
    assert_eq!(std::fs::read(out.path().join("a/empty.txt")).unwrap(), b"");
}

#[test]
fn extract_decrypts_3nk_sii() {
    // Encrypt "SiiNunit" with the 3nK scheme (the cipher is its own inverse).
    let plain = b"SiiNunit\n{\n}\n";
    let mut enc = SII_3NK_MAGIC.to_le_bytes().to_vec();
    enc.push(0); // flags
    enc.push(0x42); // key
    let mut key = 0x42u8;
    for &b in plain {
        let k = key as u32;
        enc.push(((((k << 2) ^ k ^ 0xff) << 3) ^ k ^ b as u32) as u8);
        key = key.wrapping_add(1);
    }
    let (_d, vfs) = vfs_of(&[
        file("def/enc.sii", &enc),
        file("def/plain.sii", b"SiiNunit"),
    ]);
    let out = tempfile::tempdir().unwrap();
    extract_folder(&vfs, "def", out.path(), &AtomicBool::new(false));
    assert_eq!(
        std::fs::read(out.path().join("def/enc.sii")).unwrap(),
        plain
    );
    assert_eq!(
        std::fs::read(out.path().join("def/plain.sii")).unwrap(),
        b"SiiNunit"
    );
}

/// A malicious archive must not be able to write outside the output folder.
#[test]
fn extraction_blocks_path_traversal() {
    let (_d, vfs) = vfs_of(&[
        file("../evil.txt", b"pwned"),
        file("a/../../evil2.txt", b"pwned"),
        file("ok/fine.txt", b"fine"),
    ]);
    let parent = tempfile::tempdir().unwrap();
    let out = parent.path().join("out");
    std::fs::create_dir(&out).unwrap();

    let stats = extract_folder(&vfs, "", &out, &AtomicBool::new(false));
    assert!(
        stats.failed >= 1,
        "traversal entries must be rejected: {stats:?}"
    );
    assert!(!parent.path().join("evil.txt").exists());
    assert!(!parent.path().join("evil2.txt").exists());
    assert!(!out.join("evil.txt").exists());
    assert_eq!(std::fs::read(out.join("ok/fine.txt")).unwrap(), b"fine");

    // The single-file API refuses too, regardless of how the name is supplied.
    let f = vfs.file("ok/fine.txt").unwrap();
    assert!(extract_file(&f, &out, "x", Some("../escape.txt")).is_err());
    assert!(extract_file(&f, &out, "x", Some("/abs/../../escape.txt")).is_err());
    assert!(!parent.path().join("escape.txt").exists());
}

#[test]
fn folder_text_with_game_info() {
    let (_d, vfs) = vfs_of(&sample());
    let text = folder_contents_as_text(&vfs, &["def".into(), "def/vehicle/truck".into()]);
    let expected = "\
=== Contents of 'def' for Euro Truck Simulator 2 (v1.50.1.0) ===
/def/vehicle
/def/readme.txt

=== Contents of 'def/vehicle/truck' for Euro Truck Simulator 2 (v1.50.1.0) ===
/def/vehicle/truck/a.sii
/def/vehicle/truck/b.sii
";
    assert_eq!(text, expected);

    // Without version.txt + a map file there is no game info.
    let (_d, vfs) = vfs_of(&[file("def/a.sii", b"")]);
    let text = folder_contents_as_text(&vfs, &["def".into(), "missing".into()]);
    assert_eq!(text, "=== Contents of 'def' ===\n/def/a.sii\n");
}
