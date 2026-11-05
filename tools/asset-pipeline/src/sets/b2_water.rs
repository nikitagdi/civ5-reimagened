//! Set `b2-water`: the water, river, ice and wave
//! textures, the six `.lean` files as `Rgba16F`/`R16F`, and the coast, wave and ice settings.
//!
//! - **Textures** ([`WATER_TEXTURES`]): the bytes of the original, no re-encoding (AB-D2). The ramps
//!   and colour tables the shaders read without filtering keep level 0 only (4.2, "small data").
//!   Both the files the spec names and the `_new` files the engine's `WaterSystem::Startup` loads
//!   (`waterdepthcolor_new`, `river_diffuse_new`, `river_alpha_new`) are built.
//! - **`.lean`** ([`LEAN_FILES`], `textures/lean.rs`): decoded in the pipeline, the client does not
//!   calculate (AB-D17). A file gives two textures: `tx:<file>` (`Rgba16F`, mean slopes and
//!   moments) and `tx:<file>#cross` (`R16F`, the cross moment). The headers are listed in
//!   `C5WS.lean` and in the notes of the summary.
//! - **Settings**: `coast`, `waves`, `ice` (both versions) and `lean` of `tb:world-b2`, merged into
//!   the blob there (`world::settings`).
//! - **Group** (4.13): `water` (class Map) with every texture above.
//!
//! The decal textures of the rivers (`riverbank_*`, `floodplains_*`) are the `decals` group of
//! `b2-terrain`; the marsh textures are in the style groups through `terraindef.csv`.

use std::collections::BTreeMap;
use std::path::Path;

use asset_format::{
    ContentHash, LeanInfo, LogicalId, PrefetchGroup, Trigger, Variant, VariantFormat,
};
use rayon::prelude::*;

use super::tx_id;
use crate::game::GameSource;
use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::textures::{LeanHeader, dds_passthrough, lean_textures, level0_only, variant_format};
use crate::world::groups::{self, GroupSpec};
use crate::world::settings;

pub const SET_NAME: &str = "b2-water";

/// Id of the group of this set.
pub const GROUP: &str = "water";

/// How a texture is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    /// All the levels of the file.
    Chain,
    /// Level 0 only: ramps and colour tables.
    Level0,
}

/// The DDS files of the set, by spec row.
const WATER_TEXTURES: [(&str, Keep); 20] = [
    // W2
    ("waterbumps.dds", Keep::Chain),
    ("waterdepthcolor.dds", Keep::Level0),
    ("watersky.dds", Keep::Level0),
    // The files `WaterSystem::Startup` loads in place of the ones above and below.
    ("waterdepthcolor_new.dds", Keep::Level0),
    ("river_diffuse_new.dds", Keep::Chain),
    ("river_alpha_new.dds", Keep::Chain),
    // W4
    ("river_alpha.dds", Keep::Chain),
    ("river_smooth.dds", Keep::Chain),
    ("river_diffuse.dds", Keep::Chain),
    ("river_rough.dds", Keep::Chain),
    ("generic_riverbank_d.dds", Keep::Chain),
    ("generic_riverbank_s.dds", Keep::Chain),
    ("floodplains_depth.dds", Keep::Chain),
    // W5
    ("marsh_stamp.dds", Keep::Chain),
    // W6
    ("wave_coastline_01.dds", Keep::Chain),
    ("wave_coastline_all.dds", Keep::Chain),
    // W7
    ("ice_d.dds", Keep::Chain),
    ("ice_d_sides.dds", Keep::Chain),
    ("icedisplacement.dds", Keep::Chain),
    // `g_Cube` of `PS_Ice` (rw-a6-answers RW-22 (b) 4: surface slot 0, a cube map).
    ("ice_environment_diffuse.dds", Keep::Chain),
];

/// The `.lean` files of the install (W3, W7).
pub const LEAN_FILES: [&str; 6] = [
    "waterbumps16.lean",
    "riversmooth16.lean",
    "riverrough16.lean",
    "riverrough_new16.lean",
    "ice_bump.lean",
    "ice_bump_sides.lean",
];

/// Id of the second texture of a `.lean` file.
pub fn lean_cross_id(file: &str) -> LogicalId {
    LogicalId::new(format!("{}#cross", tx_id(file).as_str()))
}

#[derive(Debug)]
pub struct WaterSummary {
    /// DDS textures built.
    pub textures: usize,
    /// The `.lean` files, as they are in `C5WS.lean`.
    pub lean: Vec<LeanInfo>,
    /// Header and sizes of every `.lean` file, for the report.
    pub lean_notes: Vec<String>,
    /// Bytes of the textures, by variant format.
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub settings_bytes: u64,
    pub group_table_bytes: u64,
    /// Stored bytes of the `water` group.
    pub group_bytes: u64,
    /// Lines for the report: unknown attributes, the `_new` files.
    pub notes: Vec<String>,
    pub stats: StoreStats,
}

/// A stored texture: its id, variant format, blob hash and size.
type StoredTexture = (LogicalId, VariantFormat, ContentHash, u64);

type Stored = Result<Vec<StoredTexture>, ModelError>;

/// A `.lean` file: its header, the two textures and the bytes of the file.
type LeanStored = (LeanHeader, Vec<StoredTexture>, u64);

/// A DDS file as one blob.
fn dds_job(store: &BlobStore, game: &GameSource, file: &str, keep: Keep) -> Stored {
    let bytes = game
        .read_by_name(file)?
        .ok_or_else(|| ModelError::invalid(format!("{file} is missing from the install")))?;
    let mut blob = dds_passthrough(&bytes).map_err(|source| ModelError::Texture {
        name: file.to_owned(),
        source,
    })?;
    if keep == Keep::Level0 {
        blob = level0_only(blob);
    }
    let (hash, size) = store.put(&blob.encode())?;
    Ok(vec![(tx_id(file), variant_format(blob.format), hash, size)])
}

/// A `.lean` file as two blobs; the header comes back with them.
fn lean_job(store: &BlobStore, game: &GameSource, file: &str) -> Result<LeanStored, ModelError> {
    let bytes = game
        .read_by_name(file)?
        .ok_or_else(|| ModelError::invalid(format!("{file} is missing from the install")))?;
    let t = lean_textures(&bytes).map_err(|e| ModelError::invalid(format!("{file}: {e}")))?;
    let mut out = Vec::new();
    for (id, blob) in [(tx_id(file), &t.slopes), (lean_cross_id(file), &t.cross)] {
        let (hash, size) = store.put(&blob.encode())?;
        out.push((id, variant_format(blob.format), hash, size));
    }
    Ok((t.header, out, bytes.len() as u64))
}

/// Builds the set into `out` and adds it to the manifest there. A file the install lacks is an
/// error (no stand-ins, AB-D14).
pub fn build_water(game: &GameSource, out: &Path) -> Result<WaterSummary, ModelError> {
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let mut notes = Vec::new();

    // ---- settings sources ----
    let read = |name: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(name)?) };
    let water = settings::build_water_settings(&read)?;

    // ---- textures ----
    let dds: Vec<Stored> = WATER_TEXTURES
        .par_iter()
        .map(|(file, keep)| dds_job(&store, game, file, *keep))
        .collect();
    let leans: Vec<_> = LEAN_FILES
        .par_iter()
        .map(|file| lean_job(&store, game, file))
        .collect();

    let mut texture_bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    let mut members: Vec<LogicalId> = Vec::new();
    let mut add =
        |manifest: &mut ManifestBuilder,
         (id, format, hash, size): (LogicalId, VariantFormat, ContentHash, u64)| {
            *texture_bytes.entry(format).or_default() += size;
            members.push(id.clone());
            manifest.add(
                id,
                Variant {
                    format,
                    hash,
                    size,
                    prefetch: PrefetchGroup(0),
                },
            );
        };
    for r in dds {
        for v in r? {
            add(&mut manifest, v);
        }
    }
    let mut lean_info = Vec::new();
    let mut lean_notes = Vec::new();
    for (file, r) in LEAN_FILES.iter().zip(leans) {
        let (h, blobs, source_bytes) = r?;
        let stored: u64 = blobs.iter().map(|b| b.3).sum();
        lean_notes.push(format!(
            "lean {file}: {}x{}, {} mips, {} byte(s) per channel, scale {}, flag {}; {} B -> {} B (Rgba16F + R16F)",
            h.width, h.height, h.mips, h.bytes_per_channel, h.scale, h.flag, source_bytes, stored
        ));
        for v in blobs {
            add(&mut manifest, v);
        }
        lean_info.push(LeanInfo {
            file: (*file).to_owned(),
            scale: h.scale,
            width: h.width,
            height: h.height,
            mips: h.mips,
            bytes_per_channel: h.bytes_per_channel as u8,
            slopes: tx_id(file),
            cross: lean_cross_id(file),
        });
    }
    notes.push(
        "finding: the engine (WaterSystem::Startup) loads waterdepthcolor_new, river_diffuse_new and river_alpha_new; the spec names the files without `_new`; both are built"
            .to_owned(),
    );

    // ---- settings ----
    let mut world = settings::load_existing(&store, &manifest)?;
    settings::apply_water(&mut world, water, lean_info.clone());
    for e in world.extras() {
        if e.starts_with("coast[") || e.starts_with("waves") || e.starts_with("ice.") {
            notes.push(format!("unknown in the settings files: {e}"));
        }
    }
    let mut missing: Vec<String> = world
        .texture_refs()
        .into_iter()
        .filter(|(what, id)| {
            what.starts_with("lean[") && manifest.manifest().variants(id).is_empty()
        })
        .map(|(what, id)| format!("{} ({what})", id.as_str()))
        .collect();
    missing.sort();
    if !missing.is_empty() {
        return Err(ModelError::invalid(format!(
            "settings name textures that are not in the build: {}",
            missing.join(", ")
        )));
    }
    let settings_bytes = settings::store_settings(&store, &mut manifest, &world)?;

    // ---- group ----
    members.sort();
    members.dedup();
    let table_bytes = groups::publish(
        &store,
        &mut manifest,
        vec![GroupSpec::map(GROUP, Trigger::Always, members)],
    )?;
    let table = groups::load(&store, &manifest)?;
    let group_bytes = table.get(GROUP).map_or(0, |g| g.bytes);

    manifest.finish(&store)?;
    Ok(WaterSummary {
        textures: WATER_TEXTURES.len(),
        lean: lean_info,
        lean_notes,
        texture_bytes,
        settings_bytes,
        group_table_bytes: table_bytes,
        group_bytes,
        notes,
        stats: store.stats(),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use asset_format::{
        GROUP_TABLE_ID, GroupTable, TexDim, TextureBlob, TextureFormat, WORLD_SETTINGS_ID,
        WorldSettings,
    };
    use ddsfile::{D3DFormat, Dds, NewD3dParams};

    use super::*;
    use crate::report::stubs::StubAllow;
    use crate::report::verify::verify;

    struct Tree(PathBuf);

    impl Tree {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "civ5-b2-water-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn dds(format: D3DFormat, w: u32, h: u32, levels: Option<u32>, fill: u8) -> Vec<u8> {
        let mut d = Dds::new_d3d(NewD3dParams {
            height: h,
            width: w,
            depth: None,
            format,
            mipmap_levels: levels,
            caps2: None,
        })
        .unwrap();
        for (i, b) in d.data.iter_mut().enumerate() {
            *b = fill.wrapping_add(i as u8);
        }
        let mut out = Vec::new();
        d.write(&mut out).unwrap();
        out
    }

    /// An L8 file with `levels` mip levels. ddsfile does not write L8 as the game's files have it,
    /// so the pixel format struct (file offset 76) is patched by hand.
    fn l8(side: u32, levels: u32, fill: u8) -> Vec<u8> {
        let mut f = dds(D3DFormat::A8R8G8B8, side, side, Some(levels), fill);
        let texels: u32 = (0..levels).map(|l| (side >> l).max(1).pow(2)).sum();
        f.truncate(128 + texels as usize);
        for (at, v) in [(76, 32u32), (80, 0x2_0000), (84, 0), (88, 8), (92, 0xff)] {
            f[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        for at in [96, 100, 104] {
            f[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
        }
        f
    }

    /// A `.lean` file: the header and bytes that count up.
    fn lean(w: u32, h: u32, n: u32, scale: f32) -> Vec<u8> {
        let mips = 32 - w.max(h).leading_zeros();
        let texels: u32 = (0..mips).map(|l| (w >> l).max(1) * (h >> l).max(1)).sum();
        let (b1, b2) = (texels * 4 * n, texels * n);
        let mut out = b"LEAN".to_vec();
        out.extend_from_slice(&scale.to_le_bytes());
        for v in [w, h, mips, n, 36, 36 + b1, 0xcccc_cc01] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend((0..b1 + b2).map(|i| (i * 13 + 1) as u8));
        out
    }

    const COAST: &str = r#"<CoastRoot><SplineCharacterList>
        <SplineCharacter Filename="CoastCharacterSpline_01_Water.txt" />
        <SplineCharacter Filename="CoastCharacterSpline_01_Surf.txt" /></SplineCharacterList>
        <CoastStyleList><CoastStyle type="Default"><SmoothingOffset HighSide="20" LowSide="20" />
        <HighBeach Height="0.2" Offset="1.5" Character="1" TerrainType="5"/>
        <LowBeach Height="0.19" Offset="1.5" Character="1" TerrainType="5"/>
        <Waterline Height="0.198" Offset="0" Character="0" /></CoastStyle></CoastStyleList></CoastRoot>"#;

    const WAVES: &str = r#"<WaveSettings WavesPerEdge="1.13" RandomSeed="0.0" CrestPagesU="8" CrestPagesV="2"
        CrestPagesInUse="16" MinSize="0.6" MaxSize="1.4" StartDistance="0.08" EndDistance="0.02" CycleTime="9"
        FadeInStart="0.05" FadeInEnd="4.0" FadeOutStart="6.5" FadeOutEnd="8.7" RestartDelayMin="2.1"
        RestartDelayMax="5" Color_R="64" Color_G="64" Color_B="64" />"#;

    fn ice(samples: &str) -> String {
        let mut s = String::from("<IceSettings DumpInput=\"0\" ");
        for (k, v) in [
            ("FarSampleDist", "2.8"),
            ("FarSamples", samples),
            ("NearSampleDist", "0.1"),
            ("NearSamples", "16"),
            ("CrackCost", "1"),
            ("InitialPotential", "40"),
            ("CrackMinClip", "0.05"),
            ("CrackMaxClip", "0.8"),
            ("EdgePullAmount", "0.25"),
            ("SmallPolyShrinkAmount", "0.35"),
            ("LatticeVertsPerHex", "12.0"),
            ("DisplaceFrequency", "0.1"),
            ("DisplaceMin", "-8"),
            ("DisplaceMax", "15"),
            ("DisplaceExponent", "1"),
            ("SmallPolyDepthMin", "6.1"),
            ("SmallPolyDepthMax", "6.6"),
            ("SmallPolyHeightMin", "0.1"),
            ("SmallPolyHeightMax", "3.6"),
            ("LargePolyEdgeHeightMin", "0"),
            ("LargePolyEdgeHeightMax", "0"),
            ("LargePolyEdgeHeightFreq", "0.5"),
            ("SkirtSlopeSmall", "0.0015625"),
            ("SkirtSlopeLarge", "0.0078125"),
            ("LargePolyHeight", "10"),
            ("LargePolyDepth", "8"),
            ("BoundaryKillRate", "0.60"),
            ("InteriorKillRate", "0.05"),
            ("Subsurface_R", "19"),
            ("Subsurface_G", "142"),
            ("Subsurface_B", "205"),
            ("SpecularPower", "150"),
            ("TextureRepeat", "0.005"),
            ("BumpRepeat", "0.009"),
            ("SideTextureRepeat_U", " 0.005"),
            ("SideBumpRepeat_U", "0.05"),
            ("SideTextureRepeat_V", " 0.005"),
            ("SideBumpRepeat_V", "0.05"),
            ("Underwater_R1", "54"),
            ("Underwater_G1", "154"),
            ("Underwater_B1", "106"),
            ("Underwater_R2", "54"),
            ("Underwater_G2", "154"),
            ("Underwater_B2", "106"),
            ("Underwater_ColorRamp", "10"),
            ("Underwater_AlphaRamp", "2"),
        ] {
            s.push_str(&format!("{k}=\"{v}\" "));
        }
        s.push_str("/>");
        s
    }

    fn fake_install() -> Tree {
        let t = Tree::new();
        std::fs::create_dir_all(t.0.join("Resource")).unwrap();
        for (i, (file, keep)) in WATER_TEXTURES.iter().enumerate() {
            let bytes = match *file {
                "marsh_stamp.dds" | "floodplains_depth.dds" | "icedisplacement.dds" => {
                    l8(8, 4, i as u8)
                }
                f if f.starts_with("wave_coastline") || f.ends_with("_alpha.dds") => {
                    dds(D3DFormat::DXT5, 8, 8, Some(4), i as u8)
                }
                _ if *keep == Keep::Level0 => dds(D3DFormat::A8R8G8B8, 4, 2, Some(3), i as u8),
                _ => dds(D3DFormat::DXT1, 8, 8, Some(4), i as u8),
            };
            t.write(&format!("Assets/Water/{file}"), &bytes);
        }
        for (i, file) in LEAN_FILES.iter().enumerate() {
            let (n, side) = if file.starts_with("ice") {
                (1, 8)
            } else {
                (2, 16)
            };
            t.write(
                &format!("Assets/Water/{file}"),
                &lean(side, side / 2, n, 0.5 + i as f32),
            );
        }
        t.write("Assets/Water/coaststyles.xml", COAST.as_bytes());
        t.write(
            "Assets/Water/CoastCharacterSpline_01_Water.txt",
            b"[0,1,0]\r\n[1,2,0]\r\n",
        );
        t.write(
            "Assets/Water/CoastCharacterSpline_01_Surf.txt",
            b"[0,3,0]\r\n[1,4,0]\r\n",
        );
        t.write("Assets/Water/coastspline01.txt", b"[-4,5,0]\r\n");
        t.write("Assets/Water/wavesettings.xml", WAVES.as_bytes());
        t.write("Assets/Water/IceSettings.xml", ice("48").as_bytes());
        t.write("Assets/Water/IceSettings_Low.xml", ice("16").as_bytes());
        t
    }

    fn out_dir() -> Tree {
        Tree::new()
    }

    #[test]
    fn a_small_install_builds_closes_and_decodes() {
        let install = fake_install();
        let game = GameSource::open(&install.0).unwrap();
        let out = out_dir();
        let s = build_water(&game, &out.0).unwrap();
        assert_eq!((s.textures, s.lean.len()), (20, 6));
        assert_eq!(s.lean_notes.len(), 6);
        assert!(
            s.lean_notes[0].contains("waterbumps16.lean: 16x8"),
            "{:?}",
            s.lean_notes
        );
        assert!(s.notes.iter().any(|n| n.contains("_new")), "{:?}", s.notes);

        // Only the missing `boot` group is wrong in a directory without `b2-light`, once `build`
        // has added the texture variants.
        crate::textures::variants::add_variants(&out.0, &Default::default()).unwrap();
        let r = verify(&out.0, &StubAllow::default()).unwrap();
        assert!(
            r.problems.iter().all(|p| p.contains("no `boot` group")),
            "{:?}",
            r.problems
        );

        let store = BlobStore::open(&out.0).unwrap();
        let manifest = ManifestBuilder::open(&store).unwrap();
        let m = manifest.manifest();
        // The set's own variant (the pass added the phones' ASTC next to it).
        let blob = |id: &str| {
            let v: Vec<_> = m
                .variants(&LogicalId::new(id))
                .iter()
                .filter(|v| v.format != asset_format::VariantFormat::Astc)
                .collect();
            assert_eq!(v.len(), 1, "{id}");
            std::fs::read(store.blob_path(&v[0].hash)).unwrap()
        };

        // Textures: the formats of the files, ramps cut to level 0, masks as R8.
        let bumps = TextureBlob::decode(&blob("tx:waterbumps.dds")).unwrap();
        assert_eq!((bumps.format, bumps.mips.len()), (TextureFormat::Bc1, 4));
        let depth = TextureBlob::decode(&blob("tx:waterdepthcolor_new.dds")).unwrap();
        assert_eq!((depth.format, depth.mips.len()), (TextureFormat::Rgba8, 1));
        let stamp = TextureBlob::decode(&blob("tx:marsh_stamp.dds")).unwrap();
        assert_eq!((stamp.format, stamp.mips.len()), (TextureFormat::R8, 4));
        let wave = TextureBlob::decode(&blob("tx:wave_coastline_01.dds")).unwrap();
        assert_eq!(wave.format, TextureFormat::Bc3);

        // The .lean files: Rgba16F with every mip, and the cross moment as R16F.
        let slopes = TextureBlob::decode(&blob("tx:waterbumps16.lean")).unwrap();
        assert_eq!(
            (
                slopes.format,
                slopes.dim,
                slopes.width,
                slopes.height,
                slopes.mips.len()
            ),
            (TextureFormat::Rgba16F, TexDim::D2, 16, 8, 5)
        );
        assert_eq!(slopes.mips[0].len(), 16 * 8 * 8);
        let cross = TextureBlob::decode(&blob("tx:waterbumps16.lean#cross")).unwrap();
        assert_eq!((cross.format, cross.mips.len()), (TextureFormat::R16F, 5));
        let ice = TextureBlob::decode(&blob("tx:ice_bump.lean")).unwrap();
        assert_eq!(
            (ice.width, ice.height),
            (8, 4),
            "8 bit files are widened too"
        );
        assert_eq!(ice.format, TextureFormat::Rgba16F);

        // Settings.
        let w = WorldSettings::decode(&blob(WORLD_SETTINGS_ID)).unwrap();
        assert_eq!((w.coast.profiles.len(), w.coast.styles.len()), (2, 1));
        assert_eq!(w.coast.spline, [[-4.0, 5.0]]);
        assert_eq!(w.waves.crest_pages_in_use, 16);
        assert_eq!(
            (w.ice.full.far_samples, w.ice.low.far_samples),
            (48.0, 16.0)
        );
        assert_eq!(w.lean.len(), 6);
        assert!(w.lean.windows(2).all(|p| p[0].file < p[1].file), "sorted");
        let bumps16 = w
            .lean
            .iter()
            .find(|l| l.file == "waterbumps16.lean")
            .unwrap();
        assert_eq!(
            (bumps16.width, bumps16.mips, bumps16.bytes_per_channel),
            (16, 5, 2)
        );
        assert!(w.extras().is_empty(), "{:?}", w.extras());

        // The group.
        let t = GroupTable::decode(&blob(GROUP_TABLE_ID)).unwrap();
        let g = t.get(GROUP).unwrap();
        assert_eq!(g.members.len(), 20 + 12);
        assert!(g.bytes > 0);
        // The sum holds the largest variant of each member: the ASTC ones the pass added are
        // larger than some BC1 originals.
        assert!(g.bytes >= s.group_bytes);
    }

    #[test]
    fn two_builds_give_the_same_manifest_and_the_settings_merge() {
        let install = fake_install();
        let game = GameSource::open(&install.0).unwrap();
        let (a, b) = (out_dir(), out_dir());
        build_water(&game, &a.0).unwrap();
        build_water(&game, &b.0).unwrap();
        let manifest = |t: &Tree| std::fs::read_to_string(t.0.join("manifest")).unwrap();
        assert_eq!(manifest(&a), manifest(&b));
        build_water(&game, &a.0).unwrap();
        assert_eq!(manifest(&a), manifest(&b), "a second build over the first");
    }

    #[test]
    fn a_missing_file_or_a_damaged_lean_is_an_error() {
        let install = fake_install();
        std::fs::remove_file(install.0.join("Assets/Water/river_rough.dds")).unwrap();
        let game = GameSource::open(&install.0).unwrap();
        let e = build_water(&game, &out_dir().0).unwrap_err().to_string();
        assert!(
            e.contains("river_rough.dds") && e.contains("missing"),
            "{e}"
        );

        let install = fake_install();
        let mut f = lean(8, 4, 2, 0.5);
        f.truncate(f.len() - 3);
        install.write("Assets/Water/ice_bump.lean", &f);
        let game = GameSource::open(&install.0).unwrap();
        let e = build_water(&game, &out_dir().0).unwrap_err().to_string();
        assert!(e.contains("ice_bump.lean"), "{e}");

        let install = fake_install();
        install.write("Assets/Water/coastspline01.txt", b"[1,2,3]\r\n");
        let game = GameSource::open(&install.0).unwrap();
        let e = build_water(&game, &out_dir().0).unwrap_err().to_string();
        assert!(
            e.contains("coastspline01.txt") && e.contains("third"),
            "{e}"
        );
    }

    #[test]
    fn verify_finds_a_lean_header_that_disagrees_with_its_texture() {
        let install = fake_install();
        let game = GameSource::open(&install.0).unwrap();
        let out = out_dir();
        build_water(&game, &out.0).unwrap();
        let store = BlobStore::open(&out.0).unwrap();
        let mut manifest = ManifestBuilder::open(&store).unwrap();
        let mut w = settings::load_existing(&store, &manifest).unwrap();
        w.lean[0].mips += 1;
        settings::store_settings(&store, &mut manifest, &w).unwrap();
        manifest.finish(&store).unwrap();
        let r = verify(&out.0, &StubAllow::default()).unwrap();
        assert!(
            r.problems
                .iter()
                .any(|p| p.contains("lean[ice_bump.lean]") && p.contains("mips")),
            "{:?}",
            r.problems
        );

        // And a settings blob without the water sections while the group is there.
        let mut w = settings::load_existing(&store, &manifest).unwrap();
        w.lean.clear();
        w.coast = Default::default();
        let mut manifest = ManifestBuilder::open(&store).unwrap();
        settings::store_settings(&store, &mut manifest, &w).unwrap();
        manifest.finish(&store).unwrap();
        let r = verify(&out.0, &StubAllow::default()).unwrap();
        assert!(
            r.problems
                .iter()
                .any(|p| p.contains("`coast` is empty") && p.contains("water group")),
            "{:?}",
            r.problems
        );
    }

    #[test]
    fn the_lean_ids_are_the_file_and_its_cross_moment() {
        assert_eq!(tx_id("ice_bump.lean").as_str(), "tx:ice_bump.lean");
        assert_eq!(
            lean_cross_id("ice_bump.lean").as_str(),
            "tx:ice_bump.lean#cross"
        );
    }
}
