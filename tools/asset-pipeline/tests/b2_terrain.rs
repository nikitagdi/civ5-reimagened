//! Set `b2-terrain` (R2.4) on the real install; skipped without `CIV5_ROOT`. The synthetic install
//! tests are in `src/sets/b2_terrain.rs`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use asset_format::{
    Codec, GROUP_TABLE_ID, GroupTable, LogicalId, PieceStyle, TerrainPiece, TerrainPieceSet,
    WORLD_SETTINGS_ID, WorldSettings,
};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::report::stubs::StubAllow;
use asset_pipeline::sets::{b2_light, b2_terrain};
use asset_pipeline::store::{BlobStore, ManifestBuilder};
use asset_pipeline::textures::{RawFormat, read_dds};
use asset_pipeline::world::pieces::build_piece;
use rayon::prelude::*;

fn real_source() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(&root).expect("scan failed"))
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("civ5-b2-terrain-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// AB acceptance criterion 5: every piece of the install packs and unpacks without a loss. The
/// source here is read independently of the packer, straight from the DDS files.
#[test]
fn every_real_piece_round_trips_without_loss() {
    let Some(game) = real_source() else { return };
    let found = b2_terrain::find_pieces(&game).unwrap();
    println!("{} pieces", found.len());
    assert!(found.len() >= 300, "{} pieces", found.len());

    let read = |name: &str| {
        game.read_by_name(name)
            .unwrap()
            .unwrap_or_else(|| panic!("{name} is missing"))
    };
    let started = std::time::Instant::now();
    let built: Vec<(PieceStyle, TerrainPiece)> = found
        .par_iter()
        .map(|f| {
            let files = f.csv.files.clone().map(|n| read(&n));
            let piece = build_piece(&f.csv, [&files[0], &files[1], &files[2], &files[3]])
                .unwrap_or_else(|e| panic!("{e}"));
            (f.style, piece)
        })
        .collect();
    let mut by_style: BTreeMap<PieceStyle, Vec<TerrainPiece>> = BTreeMap::new();
    let (mut flat_heights, mut flat_alpha, mut diff_texels) = (0, 0, 0usize);
    let mut sides: BTreeMap<u16, usize> = BTreeMap::new();
    for (style, piece) in built {
        flat_heights += usize::from(piece.height_g.codec == Codec::Const);
        flat_alpha += usize::from(piece.height_a.codec == Codec::Const);
        diff_texels += piece.height_diff.len();
        *sides.entry(piece.side).or_default() += 1;
        by_style.entry(style).or_default().push(piece);
    }
    println!(
        "flat height G: {flat_heights}, constant A: {flat_alpha}, texels with R or B != G: {diff_texels}, sides {sides:?}"
    );
    assert!(sides.keys().all(|s| matches!(s, 128 | 512)), "{sides:?}");
    println!("packed in {:.1}s", started.elapsed().as_secs_f64());

    // The csv of each piece, to find its files again.
    let csv_of: BTreeMap<&str, &b2_terrain::FoundPiece> =
        found.iter().map(|f| (f.csv.name.as_str(), f)).collect();
    let mut total = 0usize;
    for (style, mut pieces) in by_style {
        pieces.sort_by(|a, b| a.name.cmp(&b.name));
        let set = TerrainPieceSet { style, pieces };
        let bytes = set.encode();
        total += bytes.len();
        println!(
            "{style}: {} pieces, {:.2} MB",
            set.pieces.len(),
            bytes.len() as f64 / 1e6
        );
        let back = TerrainPieceSet::decode(&bytes).unwrap();
        assert_eq!(back, set);
        (0..back.pieces.len()).into_par_iter().for_each(|i| {
            let p = &back.pieces[i];
            let files = csv_of[p.name.as_str()].csv.files.clone().map(|n| read(&n));
            let d = back.piece(i).unwrap_or_else(|e| panic!("{}: {e}", p.name));
            let h = read_dds(&files[0]).unwrap();
            assert_eq!(h.format, RawFormat::Rgba16);
            let want: Vec<[u16; 4]> = h.mips[0]
                .chunks_exact(8)
                .map(|c| [0, 2, 4, 6].map(|o| u16::from_le_bytes([c[o], c[o + 1]])))
                .collect();
            assert!(d.height_rgba() == want, "{}: height", p.name);
            // Terrain id: the code of every texel is the file's bytes B, G, R, A.
            let t1 = read_dds(&files[1]).unwrap();
            let codes: Vec<u32> = t1.mips[0]
                .chunks_exact(4)
                .map(|px| u32::from_le_bytes([px[2], px[1], px[0], px[3]]))
                .collect();
            let got: Vec<u32> = (0..codes.len()).map(|t| d.id_code(t)).collect();
            assert!(got == codes, "{}: terrain ids", p.name);
            // Both blend maps, plane by plane.
            for (planes, file) in [(&d.h_blend, &files[2]), (&d.t_blend, &files[3])] {
                let b = read_dds(file).unwrap();
                let n = b.mips[0].len() / 4;
                for (c, plane) in planes.iter().enumerate() {
                    let want: Vec<u8> = (0..n).map(|i| b.mips[0][i * 4 + c]).collect();
                    assert!(*plane == want, "{}: blend plane {c}", p.name);
                }
            }
        });
    }
    println!("pieces total {:.2} MB", total as f64 / 1e6);
    // AB acceptance criterion 5: the style blobs are about 20 MB (at most 25 MB).
    assert!(total <= 25_000_000, "{total} B");
}

/// The whole of `b2-light` and `b2-terrain` in one directory closes (`verify`), `boot` stays in its
/// budget, the style groups exist.
#[test]
fn light_and_terrain_together_close() {
    let Some(game) = real_source() else { return };
    let out = TempDir::new("both");
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(n)?) };
    // Terrain first: the order of the sets must not matter.
    let t = b2_terrain::build_terrain(&game, &out.0).unwrap();
    b2_light::build_light(&read, &out.0).unwrap();

    let report = asset_pipeline::report::verify::verify(&out.0, &StubAllow::default()).unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);

    let store = BlobStore::open(&out.0).unwrap();
    let manifest = ManifestBuilder::open(&store).unwrap();
    let m = manifest.manifest();
    let blob = |id: &str| {
        std::fs::read(store.blob_path(&m.variants(&LogicalId::new(id))[0].hash)).unwrap()
    };
    let table = GroupTable::decode(&blob(GROUP_TABLE_ID)).unwrap();
    for id in [
        "boot",
        "minimap",
        "decals",
        "style:generic",
        "style:africa",
        "style:america",
        "style:asia",
        "style:europe",
    ] {
        assert!(table.get(id).is_some(), "{id}");
    }
    assert!(table.get("boot").unwrap().bytes <= 30_000_000);
    for s in ["africa", "america", "asia", "europe"] {
        let g = table.get(&format!("style:{s}")).unwrap();
        assert!(
            g.members.contains(&LogicalId::new(format!("pc:{s}"))),
            "{s}"
        );
        assert_eq!(
            g.requires,
            ["boot", "style:shared"],
            "{s}: the boot group arrived after the style groups"
        );
    }
    // The textures several continents use are in the shared group once, not in the styles.
    let shared = table.get("style:shared").unwrap();
    assert_eq!(shared.trigger, asset_format::Trigger::Always);
    assert_eq!(shared.requires, ["boot"]);
    assert!(!shared.members.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for s in ["generic", "africa", "america", "asia", "europe"] {
        let g = table.get(&format!("style:{s}")).unwrap();
        assert_eq!(g.requires, ["boot", "style:shared"], "{s}");
        for member in g.members.iter().filter(|m| m.as_str().starts_with("tx:")) {
            assert!(
                !shared.members.contains(member),
                "{} is in style:{s} and in the shared group",
                member.as_str()
            );
            assert!(
                seen.insert(member.clone()),
                "{} is in two style groups",
                member.as_str()
            );
        }
    }
    // Both sets' sections are in the one settings blob.
    let w = WorldSettings::decode(&blob(WORLD_SETTINGS_ID)).unwrap();
    assert_eq!(w.terrain_defs.len(), 41);
    assert_eq!(w.highlights.len(), 34);
    assert!(w.extras().is_empty(), "{:?}", w.extras());
    assert!(w.decals.len() >= 10);
    assert!(t.ids.codes >= 40, "{} codes", t.ids.codes);
    println!(
        "{} terrain textures, {} @low, {} decal textures",
        t.textures, t.low_textures, t.decal_textures
    );
}

/// The two copies of `terraindef.csv` are both in the index; the default pick is the loose one.
#[test]
fn terraindef_copies_are_both_kept() {
    let Some(game) = real_source() else { return };
    let copies = game.copies("terraindef.csv");
    assert_eq!(copies.len(), 2, "{copies:?}");
    let taken = asset_pipeline::world::settings::terrain_def_entry(
        &game,
        asset_pipeline::world::settings::TERRAIN_DEF_COPY,
    )
    .unwrap();
    assert!(std::ptr::eq(taken, copies[0]));
    println!(
        "{}",
        asset_pipeline::world::settings::terrain_def_note(&game).unwrap()
    );
}
