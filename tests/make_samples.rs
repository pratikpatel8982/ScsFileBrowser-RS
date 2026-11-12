//! Developer tool (ignored by default): writes a folder of sample archives for
//! manually trying out the GUI.
//!
//! ```text
//! SCS_SAMPLE_DIR=/tmp/scs-samples cargo test --test make_samples -- --ignored
//! ```

mod common;

use common::*;

#[test]
#[ignore = "writes sample archives to $SCS_SAMPLE_DIR; run explicitly"]
fn write_sample_archives() {
    let dir = std::path::PathBuf::from(
        std::env::var("SCS_SAMPLE_DIR").expect("set SCS_SAMPLE_DIR to an output folder"),
    );
    std::fs::create_dir_all(&dir).unwrap();

    let sii = |body: &str| format!("SiiNunit\n{{\n{body}\n}}\n").into_bytes();

    // A "base" HashFS v1 archive.
    let mut base = vec![
        file("version.txt", b"1.50.1.0\n"),
        file("map/europe.mbd", b"map"),
        zfile("def/readme.txt", &filler(2_000)),
        file("def/economy_data.sii", &sii("economy_data : .economy { }")),
        file("def/world/road_look.sii", &sii("road_look : .rl { }")),
    ];
    for i in 0..25 {
        base.push(file(
            &format!("def/vehicle/truck/truck_{i:02}.sii"),
            &sii(&format!("truck : .t{i} {{ }}")),
        ));
    }
    base.push(file(
        "def/vehicle/trailer/box.sii",
        &sii("trailer : .box { }"),
    ));
    base.push(zfile(
        "vehicle/truck/upgrade/paintjob/scania.streamline/accessory/a_really_long_file_name_here.sii",
        &filler(70_000),
    ));
    write(&dir, "base.scs", &build_hashfs_v1(&base, 0));

    // A DLC in HashFS v2 that overrides one file and adds more.
    let dlc = vec![
        file(
            "def/economy_data.sii",
            &sii("economy_data : .economy_dlc { }"),
        ),
        file("def/vehicle/truck/dlc_truck.sii", &sii("truck : .dlc { }")),
        file("def/dlc/readme.txt", b"hello from the dlc"),
        file("material/big.bin", &noise(300_000, 3)),
    ];
    write(&dir, "dlc_north.scs", &build_hashfs_v2(&dlc, 0));

    // A zip "mod" with no explicit directory entries.
    let m = vec![
        zfile(
            "def/economy_data.sii",
            &sii("economy_data : .economy_mod { }"),
        ),
        file("def/mods/custom.sii", &sii("custom : .c { }")),
    ];
    write(&dir, "zz_mod.zip", &build_zip(&m, &[]));
}
