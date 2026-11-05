//! The composite `b1` end to end (with the size cap).
//! Runs the real binary when `CIV5_ROOT` and `CIV5_DB_SNAPSHOT` are set, so run it with
//! `cargo test --release -p asset-pipeline --test b1_full -- --nocapture`.

use std::path::{Path, PathBuf};
use std::process::Command;

use asset_format::{ArtRecord, ArtTable, LogicalId, Manifest, VariantFormat};
use asset_pipeline::report::Report;
use asset_pipeline::sets::ART_TABLE_ID;

/// The whole output of `b1`, in MB.
const SIZE_CAP_MB: u64 = 150;

fn dir_size(p: &Path) -> u64 {
    std::fs::read_dir(p).unwrap().fold(0, |sum, e| {
        let e = e.unwrap();
        let m = e.metadata().unwrap();
        sum + if m.is_dir() {
            dir_size(&e.path())
        } else {
            m.len()
        }
    })
}

#[test]
fn real_b1_builds_with_a_complete_table_no_placeholders_and_bc7_only_textures() {
    let (Some(root), Some(snap)) = (
        std::env::var_os("CIV5_ROOT"),
        std::env::var_os("CIV5_DB_SNAPSHOT"),
    ) else {
        println!("skipped: CIV5_ROOT or CIV5_DB_SNAPSHOT not set");
        return;
    };
    let out = std::env::temp_dir().join(format!("civ5-b1full-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    // `b1-table` is the last step of the composite and fails the build on a missing key.
    let status = Command::new(env!("CARGO_BIN_EXE_civ5-assets"))
        .arg("--game")
        .arg(PathBuf::from(root))
        .args(["build", "--set", "b1", "--db"])
        .arg(PathBuf::from(snap))
        .arg("--out")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success(), "build --set b1 failed: {status}");

    let hash = std::fs::read_to_string(out.join("manifest")).unwrap();
    let m = Manifest::decode(&std::fs::read(out.join(hash.trim())).unwrap()).unwrap();
    let t = &m.variants(&LogicalId::new(ART_TABLE_ID))[0];
    let table = ArtTable::decode(&std::fs::read(out.join(t.hash.to_string())).unwrap()).unwrap();

    // Criterion 2: no placeholder units.
    let report = Report::load(&out);
    assert_eq!(report.sets["b1-units"].counts["placeholder units"], 0);
    assert_eq!(report.sets["b1-table"].counts["placeholder units"], 0);
    assert!(report.sets["b1-table"].entries.is_empty());

    // SIMP-022: every texture the table names has a BC7 and an ASTC variant and nothing else.
    let mut textures = 0;
    for (_, rec) in table.iter() {
        let ids: Vec<&LogicalId> = match rec {
            ArtRecord::Terrain(t) => vec![&t.texture],
            ArtRecord::Sv(s) => vec![&s.icon],
            ArtRecord::River(r) => r.textures.iter().collect(),
            _ => Vec::new(),
        };
        for id in ids {
            let formats: Vec<_> = m.variants(id).iter().map(|v| v.format).collect();
            assert_eq!(formats, [VariantFormat::Bc, VariantFormat::Astc], "{id:?}");
            textures += 1;
        }
    }
    assert!(textures > 400, "{textures}");

    // Size cap.
    let mb = dir_size(&out) / 1_000_000;
    println!("b1 output: {mb} MB");
    let _ = std::fs::remove_dir_all(&out);
    assert!(mb <= SIZE_CAP_MB, "{mb} MB > {SIZE_CAP_MB} MB");
}
