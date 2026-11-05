//! Set `b2-light` (R1.1): a synthetic install always, the real one when `CIV5_ROOT` is set.

use std::path::PathBuf;

use asset_format::{
    ContentHash, GROUP_TABLE_ID, GroupTable, HighlightKind, LogicalId, Manifest, PrefetchGroup,
    PriorityClass, TexDim, TextureBlob, TextureFormat, VariantFormat, WORLD_SETTINGS_ID,
    WorldSettings,
};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::sets::b2_light::{self, BOOT, Source};
use asset_pipeline::sets::tx_id;
use asset_pipeline::store::BlobStore;
use asset_pipeline::world::settings::{self, SETTINGS_FILES};
use ddsfile::{Caps2, D3DFormat, Dds, NewD3dParams};

// ---- synthetic files -----------------------------------------------------------------------

fn dds(format: D3DFormat, side: u32, h: u32, levels: u32, cube: bool, fill: u8) -> Vec<u8> {
    let mut d = Dds::new_d3d(NewD3dParams {
        height: h,
        width: side,
        depth: None,
        format,
        mipmap_levels: (levels > 1).then_some(levels),
        caps2: cube.then_some(Caps2::CUBEMAP | Caps2::CUBEMAP_ALLFACES),
    })
    .unwrap();
    if cube {
        // ddsfile allocates one face for a new cubemap; the file has six.
        d.data.resize(d.data.len() * 6, 0);
    }
    for (i, b) in d.data.iter_mut().enumerate() {
        *b = fill.wrapping_add((i / 3) as u8);
    }
    let mut out = Vec::new();
    d.write(&mut out).unwrap();
    out
}

fn tga_1600x21() -> Vec<u8> {
    let mut v = vec![0u8; 18];
    v[2] = 2;
    v[12..14].copy_from_slice(&1600u16.to_le_bytes());
    v[14..16].copy_from_slice(&21u16.to_le_bytes());
    v[16] = 32;
    for i in 0..1600 * 21 {
        v.extend_from_slice(&[(i / 1024) as u8, (i / 32) as u8, i as u8, 255]);
    }
    v
}

/// Small versions of the settings files, with the attributes of the originals.
fn synthetic_settings(name: &str) -> Option<Vec<u8>> {
    let text = match name {
        "fogcolor.csv" => "146,182,243\r\n",
        "fluidfowsettings.xml" => {
            "<FluidFOWSettings><SpawnFrequency>22</SpawnFrequency><SizeMean>2.5</SizeMean>\
             <SizeMin>1</SizeMin><SizeMax>4</SizeMax><SizeDeviation>1</SizeDeviation>\
             <MassMean>1</MassMean><MassMin>.25</MassMin><MassMax>4</MassMax>\
             <MassDeviation>1</MassDeviation><MassAndSizeCorrelation>.5f</MassAndSizeCorrelation>\
             <HeightVariation>50.0f</HeightVariation><FluidDensity>45</FluidDensity>\
             <Speed>3.5</Speed><Drag>.004</Drag><MasterSize>0.8</MasterSize>\
             <ForceStrength>.000035</ForceStrength><MaxRotate>.0025</MaxRotate>\
             <FogHeight>11</FogHeight></FluidFOWSettings>"
        }
        "hexgrid.xml" => r#"<HexGrid color = "255,255,255,75" width=".2"/>"#,
        "cultureborders.xml" => {
            r#"<CultureBorders primary_width="1.4" secondary_width="20" primary_aa_width="0.5"
             secondary_aa_width=".3" primary_alpha="200" secondary_alpha="90"
             corner_bias="0.105" cs_primary_width="2.8" cs_outline_width=".4"
             cs_aa_width=".5" cs_primary_alpha="130" cs_outline_alpha="160"
             cs_dash_width="8" randomness="6" expansion="2"/>"#
        }
        "highlights.xml" => {
            r#"<Highlights>
             <style name="" type="HexContour" width=".2" texture="hex_contour2.dds"/>
             <style name="EdgeA" type="StaticTexture" texture="Static_Texture_Highlight.dds"/>
             <style name="EnemyOutline" type="SplineBorder" width="7"
              texture="hex_contour1.dds" color="255,0,0,0,200"/></Highlights>"#
        }
        "civ5artdefines_overlay.xml" => {
            r#"<Overlays><UnitSelectionHighlight><Color red=".9" green=".9" blue=".45" alpha="1"/>
             <Pulse>true</Pulse><PulseSpeed>2</PulseSpeed><Rotation>clockwise</Rotation>
             <RotationSpeed>1</RotationSpeed></UnitSelectionHighlight></Overlays>"#
        }
        "arrowsettings.xml" => {
            r#"<Arrow min_width="3" max_width="5" width2="1.5" arc_height="75"
             outline_width=".65" outline_width2=".65" body_color="255,0,0"
             outline_color="255,255,255" alpha=".5" target_height="20" anim_speed=".5"
             ambient_light_level=".7"/>"#
        }
        _ => return None,
    };
    Some(text.as_bytes().to_vec())
}

fn synthetic(name: &str) -> Option<Vec<u8>> {
    if let Some(s) = synthetic_settings(name) {
        return Some(s);
    }
    let item = b2_light::plan().into_iter().find(|i| i.file == name)?;
    Some(match (item.source, item.dim) {
        (Source::Lut, _) => tga_1600x21(),
        (_, TexDim::Cube) => dds(D3DFormat::DXT1, 16, 16, 5, true, name.len() as u8),
        (Source::DdsLevel0, _) => dds(D3DFormat::A8R8G8B8, 8, 2, 4, false, 1),
        _ => dds(D3DFormat::DXT5, 16, 16, 5, false, 2),
    })
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("civ5-b2-light-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn manifest_of(out: &std::path::Path) -> (ContentHash, Manifest) {
    let text = std::fs::read_to_string(out.join("manifest")).unwrap();
    let hash: ContentHash = text.trim().parse().unwrap();
    let store = BlobStore::open(out).unwrap();
    let m = Manifest::decode(&std::fs::read(store.blob_path(&hash)).unwrap()).unwrap();
    (hash, m)
}

#[test]
fn synthetic_build_fills_the_manifest_in_the_boot_group() {
    let a = TempDir::new("a");
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(synthetic(n)) };
    let s = b2_light::build_light(&read, &a.0).unwrap();
    assert_eq!(s.textures, b2_light::plan().len());

    let (hash_a, m) = manifest_of(&a.0);
    let store = BlobStore::open(&a.0).unwrap();
    for item in b2_light::plan() {
        let id = tx_id(&item.file);
        let vs = m.variants(&id);
        assert_eq!(vs.len(), 1, "{}", item.file);
        let want = if item.boot { BOOT } else { PrefetchGroup(0) };
        assert_eq!(vs[0].prefetch, want, "{}", item.file);
        let blob = TextureBlob::decode(&std::fs::read(store.blob_path(&vs[0].hash)).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", item.file));
        assert_eq!(blob.dim, item.dim, "{}", item.file);
        assert!(!blob.srgb, "{}", item.file);
        match item.source {
            Source::Lut => {
                assert_eq!((blob.format, blob.depth), (TextureFormat::Rgba8, 32));
                assert_eq!(blob.mips.len(), 1);
            }
            Source::DdsLevel0 => assert_eq!(blob.mips.len(), 1, "{}", item.file),
            Source::Dds => assert_eq!(blob.mips.len(), 5, "{}", item.file),
        }
    }
    assert!(
        m.variants(&LogicalId::new("tx:no_such_file.dds"))
            .is_empty()
    );

    // The settings blob is in the manifest, in the boot group, and reads back.
    let vs = m.variants(&LogicalId::new(WORLD_SETTINGS_ID));
    assert_eq!(vs.len(), 1);
    assert_eq!(
        (vs[0].format, vs[0].prefetch),
        (VariantFormat::Generic, BOOT)
    );
    let ws = WorldSettings::decode(&std::fs::read(store.blob_path(&vs[0].hash)).unwrap()).unwrap();
    assert_eq!(ws.color_keys.len(), 22);
    assert_eq!(ws.highlights.len(), 3);
    assert_eq!(ws.highlights[2].kind, HighlightKind::SplineBorder);
    assert_eq!(ws.highlights[2].color, [255, 0, 0, 0, 200]);
    assert_eq!(
        ws.highlights[1].texture,
        Some(LogicalId::new("tx:static_texture_highlight.dds"))
    );
    assert!(ws.extras().is_empty());
    // The five-number colour is a warning, not an error.
    assert_eq!(s.warnings.len(), 1, "{:?}", s.warnings);
    assert!(s.warnings[0].contains("EnemyOutline"));

    // Two builds of the same inputs give the same manifest.
    let b = TempDir::new("b");
    b2_light::build_light(&read, &b.0).unwrap();
    assert_eq!(manifest_of(&b.0).0, hash_a);

    // The group table (AB-D10): `boot` is the boot textures and the settings, `minimap` the rest.
    let gv = m.variants(&LogicalId::new(GROUP_TABLE_ID));
    assert_eq!((gv.len(), gv[0].prefetch), (1, BOOT));
    let table = GroupTable::decode(&std::fs::read(store.blob_path(&gv[0].hash)).unwrap()).unwrap();
    let boot = table.get("boot").unwrap();
    assert_eq!(boot.class, PriorityClass::Boot);
    let boot_items = b2_light::plan().iter().filter(|i| i.boot).count();
    assert_eq!(boot.members.len(), boot_items + 1);
    assert!(boot.members.contains(&LogicalId::new(WORLD_SETTINGS_ID)));
    let sum: u64 = boot.members.iter().map(|id| m.variants(id)[0].size).sum();
    assert_eq!(boot.bytes, sum);
    let minimap = table.get("minimap").unwrap();
    assert_eq!(minimap.members.len(), 7);
    assert_eq!(minimap.requires, ["boot"]);
    // The whole output closes: blobs, types, groups, settings references, and (after the texture
    // variants pass `build` ends with) every texture for every device.
    asset_pipeline::textures::variants::add_variants(&a.0, &Default::default()).unwrap();
    let report = asset_pipeline::report::verify::verify(
        &a.0,
        &asset_pipeline::report::stubs::StubAllow::default(),
    )
    .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.groups, 2);
}

#[test]
fn a_highlight_texture_outside_the_build_is_an_error() {
    let a = TempDir::new("ref");
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> {
        if n == "highlights.xml" {
            return Ok(Some(
                br#"<Highlights><style name="x" type="StaticTexture" texture="nowhere.dds"/></Highlights>"#
                    .to_vec(),
            ));
        }
        Ok(synthetic(n))
    };
    let e = b2_light::build_light(&read, &a.0).unwrap_err().to_string();
    assert!(e.contains("tx:nowhere.dds"), "{e}");
}

#[test]
fn a_missing_settings_file_is_an_error() {
    for gone in SETTINGS_FILES {
        let a = TempDir::new("gone");
        let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> {
            Ok(if n == gone { None } else { synthetic(n) })
        };
        let e = b2_light::build_light(&read, &a.0).unwrap_err().to_string();
        assert!(e.contains(gone) && e.contains("missing"), "{gone}: {e}");
    }
}

// ---- the real install -----------------------------------------------------------------------

fn real_source() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(&root).expect("scan failed"))
}

/// Level-major blob mips back to the file's order: face-major, each face its whole chain.
fn file_data(blob: &TextureBlob) -> Vec<u8> {
    if blob.dim != TexDim::Cube {
        return blob.mips.concat();
    }
    let mut out = Vec::new();
    for face in 0..6 {
        for level in &blob.mips {
            let n = level.len() / 6;
            out.extend_from_slice(&level[face * n..(face + 1) * n]);
        }
    }
    out
}

#[test]
fn real_textures_are_the_bytes_of_the_files() {
    let Some(src) = real_source() else { return };
    let mut checked = 0;
    for item in b2_light::plan() {
        let bytes = src
            .read_by_name(&item.file)
            .unwrap()
            .unwrap_or_else(|| panic!("{} is missing", item.file));
        let blob =
            b2_light::convert(&item, &bytes).unwrap_or_else(|e| panic!("{}: {e}", item.file));
        assert_eq!(blob.dim, item.dim, "{}", item.file);
        assert!(!blob.srgb, "{}", item.file);
        if item.source == Source::Lut {
            continue;
        }
        let file = Dds::read(&bytes[..]).unwrap_or_else(|e| panic!("{}: {e}", item.file));
        let mut data = file_data(&blob);
        let mut expected = file.data.clone();
        if blob.format == TextureFormat::Rgba8 {
            // The file is B, G, R, A; the blob R, G, B, A.
            data.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
            // X8R8G8B8 (e.g. the minimap ice): the unused byte is opaque in the
            // blob and whatever the file has there.
            if !file
                .header
                .spf
                .flags
                .contains(ddsfile::PixelFormatFlags::ALPHA_PIXELS)
            {
                assert!(data.chunks_exact(4).all(|p| p[3] == 255), "{}", item.file);
                data.chunks_exact_mut(4).for_each(|p| p[3] = 0);
                expected.chunks_exact_mut(4).for_each(|p| p[3] = 0);
            }
        }
        match item.source {
            Source::Dds => assert_eq!(data, expected, "{}", item.file),
            // Level 0 of a 2D texture: the first bytes of the file data, in RGBA order.
            _ => {
                assert_eq!(blob.mips.len(), 1, "{}", item.file);
                let n = blob.mips[0].len();
                let mut got = blob.mips[0].clone();
                got.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
                assert_eq!(got, file.data[..n], "{}", item.file);
            }
        }
        checked += 1;
    }
    println!("{checked} DDS files equal their source");
}

#[test]
fn real_cubes_match_the_measured_shapes() {
    let Some(src) = real_source() else { return };
    let cube = |name: &str| {
        let item = b2_light::plan()
            .into_iter()
            .find(|i| i.file == name)
            .unwrap();
        b2_light::convert(&item, &src.read_by_name(name).unwrap().unwrap()).unwrap()
    };
    // Terrain cubes DXT1 128^2 with 8 mips, model cubes 32^2.
    for style in ["africa", "america", "asia", "europe", "generic"] {
        for kind in ["sunlight", "shadowlight"] {
            let t = cube(&format!("{kind}_{style}_terrain.dds"));
            assert_eq!(
                (t.format, t.width, t.mips.len()),
                (TextureFormat::Bc1, 128, 8),
                "{kind}_{style}"
            );
        }
    }
    let t = cube("unit_environment_sharp.dds");
    assert_eq!(
        (t.format, t.width, t.mips.len()),
        (TextureFormat::Bc1, 32, 6)
    );
    let t = cube("cubelight_generic_32.dds");
    assert_eq!(
        (t.format, t.width, t.mips.len()),
        (TextureFormat::Rgba8, 32, 1)
    );
}

/// Pixels of a bottom-up 32 bit TGA in top-down raster order as (r, g, b, a), parsed here
/// independently of the reader.
fn tga_pixels(bytes: &[u8]) -> Vec<[u8; 4]> {
    assert_eq!(
        (bytes[2], bytes[16], bytes[17] & 0x20),
        (2, 32, 0),
        "bottom-up 32 bit"
    );
    let start = 18 + usize::from(bytes[0]);
    let w = usize::from(u16::from_le_bytes([bytes[12], bytes[13]]));
    let h = usize::from(u16::from_le_bytes([bytes[14], bytes[15]]));
    let stored: Vec<[u8; 4]> = bytes[start..start + w * h * 4]
        .chunks_exact(4)
        .map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    stored.chunks_exact(w).rev().flatten().copied().collect()
}

#[test]
fn real_colour_keys_follow_the_raster_index() {
    let Some(src) = real_source() else { return };
    let files = asset_pipeline::world::lut::lut_files();
    assert_eq!(files.len(), 22);
    for file in &files {
        let bytes = src
            .read_by_name(file)
            .unwrap()
            .unwrap_or_else(|| panic!("{file} is missing"));
        let item = b2_light::plan()
            .into_iter()
            .find(|i| &i.file == file)
            .unwrap();
        let blob = b2_light::convert(&item, &bytes).unwrap();
        assert_eq!(
            (blob.dim, blob.width, blob.height, blob.depth),
            (TexDim::D3, 32, 32, 32)
        );
        let raster = tga_pixels(&bytes);
        let mut identity = true;
        for b in 0..32usize {
            for g in 0..32usize {
                for r in 0..32usize {
                    let i = r + 32 * g + 1024 * b;
                    assert_eq!(
                        blob.mips[0][i * 4..i * 4 + 4],
                        raster[i],
                        "{file} ({r},{g},{b})"
                    );
                    identity &=
                        raster[i][..3] == [8 * r as u8 + 3, 8 * g as u8 + 3, 8 * b as u8 + 3];
                }
            }
        }
        // measured: 2026-10-21, the Terrain_*, Buildings_* and TerrainColorKey tables are identity grids
        // (8*i + 3 per channel), which fixes the raster order as top-down; the others are graded.
        let neutral = file.starts_with("terrain_")
            || file.starts_with("buildings_")
            || file == "terraincolorkey.tga";
        println!(
            "{file}: identity grid: {identity}; {} px in the file",
            raster.len()
        );
        assert_eq!(identity, neutral, "{file}");
    }
}

#[test]
fn real_build_into_a_temp_store() {
    let Some(src) = real_source() else { return };
    let out = TempDir::new("real");
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(src.read_by_name(n)?) };
    let s = b2_light::build_light(&read, &out.0).unwrap();
    assert_eq!(s.textures, 70);
    println!(
        "boot {} B, minimap {} B, settings {} B",
        s.boot_bytes, s.other_bytes, s.settings_bytes
    );
    // Cubes, LUTs, fog and overlay textures: a part of the 30 MB boot budget.
    assert!(s.boot_bytes < 25_000_000, "{} B", s.boot_bytes);
    assert!(s.settings_bytes < 20_000, "{} B", s.settings_bytes);
    let (_, m) = manifest_of(&out.0);
    for item in b2_light::plan() {
        let vs = m.variants(&tx_id(&item.file));
        assert_eq!(vs.len(), 1, "{}", item.file);
        let want = if item.boot { BOOT } else { PrefetchGroup(0) };
        assert_eq!(vs[0].prefetch, want);
    }
    assert_eq!(m.variants(&LogicalId::new(WORLD_SETTINGS_ID)).len(), 1);

    // AB acceptance criterion 9: `boot` is at most 30 MB. Criterion 10: the output closes.
    let gv = m.variants(&LogicalId::new(GROUP_TABLE_ID));
    let store = BlobStore::open(&out.0).unwrap();
    let table = GroupTable::decode(&std::fs::read(store.blob_path(&gv[0].hash)).unwrap()).unwrap();
    let boot = table.get("boot").unwrap();
    println!("boot group: {} ids, {} B", boot.members.len(), boot.bytes);
    assert!(boot.bytes <= 30_000_000, "{} B", boot.bytes);
    let report = asset_pipeline::report::verify::verify(
        &out.0,
        &asset_pipeline::report::stubs::StubAllow::default(),
    )
    .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    let budget = asset_pipeline::report::budget::run(&out.0).unwrap();
    assert!(budget.errors.is_empty(), "{:?}", budget.errors);
}

#[test]
fn real_settings_are_complete() {
    let Some(src) = real_source() else { return };
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(src.read_by_name(n)?) };
    let (ws, warnings) = settings::build_settings(&read).unwrap();
    // Nothing the pipeline does not know by name.
    assert!(ws.extras().is_empty(), "{:?}", ws.extras());
    // Lekmod's Highlights.xml is the copy in use (AB-D15): 34 styles, EnemyOutline with 5 numbers.
    assert_eq!(ws.highlights.len(), 34);
    let enemy = ws
        .highlights
        .iter()
        .find(|h| h.name == "EnemyOutline")
        .unwrap();
    assert_eq!(enemy.color, [255, 0, 0, 0, 200]);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("EnemyOutline"), "{warnings:?}");
    assert_eq!(ws.fog_colors, [[146, 182, 243]]);
    assert_eq!(ws.fluid_fow.spawn_frequency, 22.0);
    assert_eq!(ws.fluid_fow.mass_and_size_correlation, 0.5);
    assert_eq!(ws.hex_grid.color, [255, 255, 255, 75]);
    assert_eq!(ws.culture_borders.corner_bias, 0.105);
    assert_eq!(ws.arrows.body_color, [255, 0, 0]);
    assert_eq!(ws.color_keys.len(), 22);
    assert!(ws.overlay_defs.unit_selection_highlight.pulse);
    // Every texture a style names is in the set.
    let ids: Vec<_> = b2_light::plan().iter().map(|i| tx_id(&i.file)).collect();
    settings::check_references(&ws, |t| ids.contains(t)).unwrap();
}

#[test]
fn real_overlay_textures_have_the_measured_formats() {
    let Some(src) = real_source() else { return };
    let get = |name: &str| {
        let item = b2_light::plan()
            .into_iter()
            .find(|i| i.file == name)
            .unwrap();
        b2_light::convert(&item, &src.read_by_name(name).unwrap().unwrap()).unwrap()
    };
    //
    for (name, format, w, h) in [
        ("hexgridid.dds", TextureFormat::R8, 320, 320),
        ("hex_contour1.dds", TextureFormat::R8, 128, 128),
        ("hexgrid.dds", TextureFormat::Rgba8, 512, 512),
        ("flowfield.dds", TextureFormat::Rgba8, 1024, 1024),
        ("fogramp.dds", TextureFormat::Rgba8, 128, 8),
        ("fx_fog_dif.dds", TextureFormat::Bc3, 512, 512),
        ("minimapfog.dds", TextureFormat::Rgba8, 256, 256),
        ("solid_contour.dds", TextureFormat::Rgba8, 4, 4),
    ] {
        let t = get(name);
        assert_eq!((t.format, t.width, t.height), (format, w, h), "{name}");
    }
    // fogramp keeps its whole mip chain.
    assert!(get("fogramp.dds").mips.len() > 1);
}

#[test]
fn real_layer_conflicts_are_listed() {
    let Some(src) = real_source() else { return };
    let conflicts = b2_light::layer_conflicts(&src).unwrap();
    for c in &conflicts {
        println!("layer conflict: {c}");
    }
    // Highlights.xml: the Lekmod copy wins and hides the base one.
    assert!(
        conflicts
            .iter()
            .any(|c| c.starts_with("highlights.xml: Lekmod")),
        "{conflicts:?}"
    );
    println!("{}", settings::highlights_copy_note(&src).unwrap());
}
