//! Set `b2-water` (R2.6) on the real install; skipped without `CIV5_ROOT`. The synthetic install
//! tests are in `src/sets/b2_water.rs`.

use std::path::PathBuf;

use asset_format::{
    COAST_NONE, GROUP_TABLE_ID, GroupTable, LogicalId, TextureBlob, TextureFormat,
    WORLD_SETTINGS_ID, WorldSettings, f16_bits_to_f32,
};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::report::stubs::StubAllow;
use asset_pipeline::sets::b2_water::{self, LEAN_FILES};
use asset_pipeline::sets::{b2_light, b2_terrain};
use asset_pipeline::store::{BlobStore, ManifestBuilder};
use asset_pipeline::textures::{HALF_ERROR, RawFormat, lean_textures, parse_lean, read_dds};
use asset_pipeline::world::settings;

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
        let dir = std::env::temp_dir().join(format!("civ5-b2-water-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read(game: &GameSource, name: &str) -> Vec<u8> {
    game.read_by_name(name)
        .unwrap()
        .unwrap_or_else(|| panic!("{name} is missing"))
}

/// AB-8 done: the six `.lean` files become `Rgba16F` + `R16F` with every mip; the numbers of the
/// headers are the ones measured in the install.
#[test]
fn six_lean_files_become_half_float_textures() {
    let Some(game) = real_source() else { return };
    // file, scale, width, height, mips, bytes per channel
    let want: [(&str, f32, u32, u32, u32, u32); 6] = [
        ("waterbumps16.lean", 0.33, 512, 512, 10, 2),
        ("riversmooth16.lean", 0.7, 256, 64, 9, 2),
        ("riverrough16.lean", 0.7, 256, 64, 9, 2),
        ("riverrough_new16.lean", 2.5, 256, 64, 9, 2),
        ("ice_bump.lean", 0.18431, 512, 512, 10, 1),
        ("ice_bump_sides.lean", 0.18431, 512, 512, 10, 1),
    ];
    assert_eq!(LEAN_FILES.len(), want.len());
    for (file, scale, w, h, mips, n) in want {
        let bytes = read(&game, file);
        let t = lean_textures(&bytes).unwrap_or_else(|e| panic!("{file}: {e}"));
        let hd = t.header;
        assert!(
            (hd.scale - scale).abs() < 1e-5,
            "{file}: scale {}",
            hd.scale
        );
        assert_eq!(
            (hd.width, hd.height, hd.mips, hd.bytes_per_channel),
            (w, h, mips, n),
            "{file}"
        );
        assert_eq!(hd.flag, 1, "{file}");
        assert_eq!(t.slopes.format, TextureFormat::Rgba16F);
        assert_eq!(t.cross.format, TextureFormat::R16F);
        assert_eq!(t.slopes.mips.len(), mips as usize);
        assert_eq!(t.cross.mips.len(), mips as usize);
        // The chains are the whole of the file: nothing is left over, nothing is made up.
        let raw = parse_lean(&bytes).unwrap();
        let in_chains: usize = raw.slopes.iter().chain(&raw.cross).map(Vec::len).sum();
        assert_eq!(in_chains + 36, bytes.len(), "{file}");
        // Every value is within the half float error of the file's.
        let max = if n == 1 { 255.0 } else { 65535.0 };
        let mut worst = 0f32;
        for (src, dst) in raw.slopes.iter().zip(&t.slopes.mips) {
            for (v, d) in src.chunks_exact(n as usize).zip(dst.chunks_exact(2)) {
                let u = if n == 1 {
                    f32::from(v[0])
                } else {
                    f32::from(u16::from_le_bytes([v[0], v[1]]))
                };
                let got = f16_bits_to_f32(u16::from_le_bytes([d[0], d[1]]));
                worst = worst.max((got - u / max).abs());
            }
        }
        println!(
            "{file}: {w}x{h}, {mips} mips, {n} B/channel; Rgba16F {} B + R16F {} B; worst error {worst:.2e}",
            t.slopes.encode().len(),
            t.cross.encode().len()
        );
        assert!(worst <= HALF_ERROR + 1e-7, "{file}: {worst}");
    }
}

/// The install holds the two chains of `riverrough_new16.lean` as DDS files (`..._1.dds`,
/// `..._2.dds`): the decoder reads the same bytes, which is the proof that the file is raw.
#[test]
fn riverrough_new16_matches_its_dds_twins() {
    let Some(game) = real_source() else { return };
    let raw = parse_lean(&read(&game, "riverrough_new16.lean")).unwrap();
    for (name, chain, format) in [
        ("riverrough_new16_1.dds", &raw.slopes, RawFormat::Rgba16),
        ("riverrough_new16_2.dds", &raw.cross, RawFormat::R16),
    ] {
        let d = read_dds(&read(&game, name)).unwrap();
        assert_eq!(d.format, format, "{name}");
        assert_eq!(d.mips.len(), chain.len(), "{name}");
        for (l, (a, b)) in d.mips.iter().zip(chain).enumerate() {
            assert!(a == b, "{name}: level {l} differs from the .lean chain");
        }
    }
}

/// AB-8 done: `coaststyles.xml` gives 13 styles and 9 profiles of 100 points, nothing unknown.
#[test]
fn coast_wave_and_ice_settings_of_the_install() {
    let Some(game) = real_source() else { return };
    let r = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(n)?) };
    let (coast, waves, ice) = settings::build_water_settings(&r).unwrap();
    assert_eq!(coast.styles.len(), 13);
    assert_eq!(coast.profiles.len(), 9);
    for p in &coast.profiles {
        assert_eq!(p.points.len(), 100, "{}", p.file);
    }
    assert_eq!(coast.spline.len(), 79);
    assert_eq!(coast.profiles[0].file, "CoastCharacterSpline_01_Water.txt");
    let names: Vec<&str> = coast.styles.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names[0], "Default");
    assert_eq!(names[12], "DefaultShallows");
    assert!(names.contains(&"Mountain"));
    for s in &coast.styles {
        assert!(s.extra.is_empty(), "{}: {:?}", s.name, s.extra);
        for l in [&s.high_beach, &s.low_beach, &s.waterline] {
            assert!(l.character == COAST_NONE || usize::from(l.character) < 9);
        }
    }
    let shallows = &coast.styles[12];
    assert_eq!(shallows.waterline.character, COAST_NONE);
    assert_eq!(coast.styles[11].high_beach.character, 6, "Mountain");
    assert!(waves.extra.is_empty() && ice.full.extra.is_empty() && ice.low.extra.is_empty());
    assert_eq!(
        (
            waves.crest_pages_u,
            waves.crest_pages_v,
            waves.crest_pages_in_use
        ),
        (8, 2, 16)
    );
    assert_eq!(waves.cycle_time, 9.0);
    // The two ice files differ in the sample counts and the lattice, nowhere else we know of.
    assert_eq!((ice.full.far_samples, ice.low.far_samples), (48.0, 16.0));
    assert_eq!(
        (
            ice.full.lattice_verts_per_hex,
            ice.low.lattice_verts_per_hex
        ),
        (12.0, 8.0)
    );
    assert_eq!(ice.full.side_texture_repeat_u, 0.005, "the padded value");
}

/// The three B2 sets in one directory, water first: it closes, the water group is in budget and
/// the settings hold every section of the three sets.
#[test]
fn light_terrain_and_water_together_close() {
    let Some(game) = real_source() else { return };
    let out = TempDir::new("all");
    let r = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(n)?) };
    let w = b2_water::build_water(&game, &out.0).unwrap();
    for n in &w.lean_notes {
        println!("{n}");
    }
    println!(
        "water group {:.2} MB, textures {:?}",
        w.group_bytes as f64 / 1e6,
        w.texture_bytes
    );
    // `water` is about 20 MB.
    assert!(w.group_bytes <= 20_000_000, "{} B", w.group_bytes);
    b2_light::build_light(&r, &out.0).unwrap();
    b2_terrain::build_terrain(&game, &out.0).unwrap();

    let report = asset_pipeline::report::verify::verify(&out.0, &StubAllow::default()).unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);

    let store = BlobStore::open(&out.0).unwrap();
    let manifest = ManifestBuilder::open(&store).unwrap();
    let m = manifest.manifest();
    let blob = |id: &str| {
        std::fs::read(store.blob_path(&m.variants(&LogicalId::new(id))[0].hash)).unwrap()
    };
    let ws = WorldSettings::decode(&blob(WORLD_SETTINGS_ID)).unwrap();
    assert_eq!(ws.coast.styles.len(), 13);
    assert_eq!(ws.lean.len(), 6);
    assert_eq!(ws.terrain_defs.len(), 41);
    assert_eq!(ws.highlights.len(), 34);
    assert!(ws.extras().is_empty(), "{:?}", ws.extras());

    let table = GroupTable::decode(&blob(GROUP_TABLE_ID)).unwrap();
    let g = table.get("water").unwrap();
    assert_eq!(g.requires, ["boot"]);
    // 19 textures and 12 lean halves.
    assert_eq!(g.members.len(), 31);
    // A lean texture reads back with its own header.
    let bumps = TextureBlob::decode(&blob("tx:waterbumps16.lean")).unwrap();
    assert_eq!(
        (bumps.format, bumps.width, bumps.mips.len()),
        (TextureFormat::Rgba16F, 512, 10)
    );
}
