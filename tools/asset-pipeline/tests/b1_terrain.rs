//! Set `b1-terrain`: synthetic fixtures always; the real install when `CIV5_ROOT` and
//! `CIV5_DB_SNAPSHOT` are set.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{
    ArtTable, ContentHash, FeatureArt, LogicalId, Manifest, PlatformProfile, Projection,
    TextureBlob, VariantFormat,
};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::sets::terrain::{
    FEATURE_DECALS, FOG_CLOUD_TEXTURES, MOUNTAIN_TOP, RIVER_TEXTURES, STYLES, TERRAIN_DEF,
    WATER_TEXTURES, build_terrain, parse_terraindef, plan_terrain, terrain_types_from_snapshot,
};
use asset_pipeline::sets::{ART_TABLE_ID, TextureOptions};
use ddsfile::{D3DFormat, Dds, NewD3dParams};

const TERRAINS: [&str; 9] = [
    "TERRAIN_GRASS",
    "TERRAIN_PLAINS",
    "TERRAIN_DESERT",
    "TERRAIN_TUNDRA",
    "TERRAIN_SNOW",
    "TERRAIN_COAST",
    "TERRAIN_OCEAN",
    "TERRAIN_MOUNTAIN",
    "TERRAIN_HILL",
];

/// The layout of the real file (names and IDs), with made-up scales.
const CSV: &str = "Continent ID,Diffuse,Heightmap,Specmap,BaseDominance,Texture Scale,Projection(0=horizontal 1=vertical), ID (unique for continent)\r
0,Asia_Tundra_D.dds,Generic_Tundra_H.dds,NULL,0,0.014,0,0\r
0,Worl_Ocean_Floor_D.dds,Worl_Ocean_Floor_H.dds,NULL,0,0.1,0,15\r
0,Generic_Snow_D.dds,Generic_Snow_H.dds,Generic_Snow_S.dds,0,0.015,0,30\r
0,Euro_Coast_D.dds,Euro_Coast_H.dds,Euro_Coast_S.dds,0,0.17,0,5\r
0,Generic_Mesa_D.dds,Generic_Mesa_H.dds,Generic_Mesa_S.dds,0,0.05,1,40\r
1,Amer_Mountain_Base_D.dds,Amer_Mountain_Base_H.dds,NULL,1,0.07,0,10\r
1,Amer_Mountain_Top_D.dds,Euro_Mountain_Top_H.dds,NULL,1,0.1,0,35\r
1,Amer_Grassland_D.dds,Euro_Grassland_H.dds,NULL,0,0.0141,0,2\r
1,Amer_Desert_D.dds,Afri_Desert_H.dds,NULL,0,0.014,0,26\r
1,Amer_Plain_D.dds,Euro_Plain_H.dds,NULL,0,0.014,0,27\r
1,Amer_Tundra_D.dds,Generic_Tundra_H.dds,NULL,0,0.014,0,44\r
1,Amer_Tundra_Hill_D.dds,Generic_Tundra_Hill_H.dds,NULL,0,0.056,0,32\r
2,Asia_Mountain_Base_D.dds,Asia_Mountain_Base_H.dds,NULL,1,0.09,0,10\r
2,Asia_Grassland_D.dds,Euro_Grassland_H.dds,NULL,0,0.014,0,2\r
2,Asia_Desert_D.dds,Euro_Desert_H.dds,NULL,0,0.014,0,26\r
2,Asia_Plain_D.dds,Euro_Plain_H.dds,NULL,0,0.014,0,27\r
2,Asia_Tundra_D.dds,Generic_Tundra_H.dds,NULL,0,0.014,0,44\r
3,Afri_Mountain_Base_D.dds,Afri_Mountain_Base_H.dds,NULL,1,0.04,0,10\r
3,Afri_Grassland_D.dds,Euro_Grassland_H.dds,NULL,0,0.014,0,2\r
3,Afri_Desert_D.dds,Afri_Desert_H.dds,NULL,0,0.014,0,26\r
3,Afri_Plain_D.dds,Euro_Plain_H.dds,NULL,0,0.014,0,27\r
3,Afri_Tundra_D.dds,Generic_Tundra_H.dds,NULL,0,0.014,0,44\r
4,Euro_Mountain_Top_D.dds,Euro_Mountain_Top_H.dds,NULL,0,0.1,0,8\r
4,Euro_Mountain_Base_D.dds,Euro_Mountain_Base_H.dds,NULL,1,0.15,0,10\r
4,Euro_Grassland_D.dds,Euro_Grassland_H.dds,NULL,0,0.014,0,2\r
4,Euro_Desert_D.dds,Euro_Desert_H.dds,NULL,0,0.014,0,26\r
4,Euro_Plain_D.dds,Euro_Plain_H.dds,NULL,0,0.014,0,27\r
4,Euro_Shallow_Seas_D.dds,Euro_Shallow_Seas_H.dds,NULL,0,0.004,0,30\r
4,Euro_Tundra_D.dds,Generic_Tundra_H.dds,NULL,0,0.014,0,44\r
";

fn types() -> Vec<String> {
    TERRAINS.iter().map(|s| (*s).to_owned()).collect()
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-b1terrain-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn dds(side: u32) -> Vec<u8> {
    let mut d = Dds::new_d3d(NewD3dParams {
        height: side,
        width: side,
        depth: None,
        format: D3DFormat::A8R8G8B8,
        mipmap_levels: Some(1),
        caps2: None,
    })
    .unwrap();
    for (i, b) in d.data.iter_mut().enumerate() {
        *b = (i * 7 % 251) as u8;
    }
    let mut out = Vec::new();
    d.write(&mut out).unwrap();
    out
}

/// A fake install: every file the set asks for exists; `euro_grassland_d.dds` is 1024 wide,
/// the rest 16 (BC7 in a debug build is slow).
fn fake_read(name: &str) -> Result<Option<Vec<u8>>, ModelError> {
    if name == TERRAIN_DEF {
        return Ok(Some(CSV.as_bytes().to_vec()));
    }
    let side = if name == "euro_grassland_d.dds" {
        1024
    } else {
        16
    };
    Ok(Some(dds(side)))
}

fn read_manifest(out: &Path) -> Manifest {
    let hash: ContentHash = std::fs::read_to_string(out.join("manifest"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    Manifest::decode(&std::fs::read(out.join(hash.to_string())).unwrap()).unwrap()
}

fn blob_bytes(out: &Path, m: &Manifest, id: &LogicalId, fmt: VariantFormat) -> Vec<u8> {
    let v = m
        .variants(id)
        .iter()
        .find(|v| v.format == fmt)
        .unwrap_or_else(|| panic!("{id:?} has no {fmt:?} variant"));
    std::fs::read(out.join(v.hash.to_string())).unwrap()
}

#[test]
fn terraindef_rows_parse_with_hex_ids_and_crlf() {
    let rows = parse_terraindef(CSV).unwrap();
    assert_eq!(rows.len(), 29);
    let snow = rows
        .iter()
        .find(|r| r.diffuse == "Generic_Snow_D.dds")
        .unwrap();
    assert_eq!((snow.continent, snow.id, snow.projection), (0, 0x30, 0));
    assert!((snow.texture_scale - 0.015).abs() < 1e-6);
    let mesa = rows
        .iter()
        .find(|r| r.diffuse == "Generic_Mesa_D.dds")
        .unwrap();
    assert_eq!((mesa.id, mesa.projection), (0x40, 1));
    assert!(parse_terraindef("header\n9,a.dds,b,c,0,1,0,0\n").is_err());
    assert!(parse_terraindef("header\n1,a.dds,b,c,0,1,0\n").is_err());
    assert!(parse_terraindef("header\n").is_err());
}

#[test]
fn every_terrain_type_has_a_record_for_every_style() {
    let rows = parse_terraindef(CSV).unwrap();
    let plan = plan_terrain(&rows, &types()).unwrap();
    let mut table = ArtTable::new();
    plan.apply(&mut table);
    for t in TERRAINS.iter().chain(&[MOUNTAIN_TOP]) {
        for s in STYLES {
            let rec = table
                .terrain(t, s)
                .unwrap_or_else(|| panic!("no record for {t} style {s}"));
            assert!(rec.texture.as_str().starts_with("tex:"), "{t} {s}");
            assert!(rec.texture_scale > 0.0);
            assert_eq!(rec.projection, Projection::WorldXz);
        }
    }
    // Picks follow the csv: continent rows first, then Generic, then Europe.
    let tex = |t: &str, s: u8| table.terrain(t, s).unwrap().texture.as_str().to_owned();
    assert_eq!(tex("TERRAIN_GRASS", 1), "tex:amer_grassland_d.dds");
    assert_eq!(tex("TERRAIN_GRASS", 0), "tex:euro_grassland_d.dds");
    assert_eq!(tex("TERRAIN_TUNDRA", 0), "tex:asia_tundra_d.dds");
    assert_eq!(tex("TERRAIN_TUNDRA", 4), "tex:euro_tundra_d.dds");
    assert_eq!(tex("TERRAIN_HILL", 3), "tex:afri_plain_d.dds");
    assert_eq!(tex(MOUNTAIN_TOP, 1), "tex:amer_mountain_top_d.dds");
    assert_eq!(tex(MOUNTAIN_TOP, 2), "tex:euro_mountain_top_d.dds");
    for s in STYLES {
        // Cell types that are Generic in the game (and not Euro_Shallow_Seas, which shares
        // ID 0x30 on continent 4).
        assert_eq!(tex("TERRAIN_SNOW", s), "tex:generic_snow_d.dds");
        assert_eq!(tex("TERRAIN_COAST", s), "tex:euro_coast_d.dds");
        assert_eq!(tex("TERRAIN_OCEAN", s), "tex:worl_ocean_floor_d.dds");
    }
    assert_eq!(
        table.terrain("TERRAIN_MOUNTAIN", 4).unwrap().texture_scale,
        0.15
    );
}

#[test]
fn unknown_terrain_and_missing_rows_are_errors() {
    let rows = parse_terraindef(CSV).unwrap();
    let e = plan_terrain(&rows, &["TERRAIN_LAVA".to_owned()]).unwrap_err();
    assert!(e.contains("TERRAIN_LAVA"), "{e}");
    let no_europe: String = CSV
        .lines()
        .filter(|l| !l.starts_with("4,") && !l.starts_with("0,Euro"))
        .collect::<Vec<_>>()
        .join("\n");
    let rows = parse_terraindef(&no_europe).unwrap();
    assert!(plan_terrain(&rows, &["TERRAIN_COAST".to_owned()]).is_err());
}

#[test]
fn build_writes_textures_table_and_keeps_other_sets_entries() {
    let tmp = TempDir::new();
    let out = tmp.path();
    let opts = TextureOptions::default();
    let s = build_terrain(&fake_read, &types(), out, opts).unwrap();
    // 4 + 4 + 5 + 3 + ... distinct diffuse files, 5 decals, ice_d, 2 river files.
    let m = read_manifest(out);
    let table_id = LogicalId::new(ART_TABLE_ID);
    let table = ArtTable::decode(&blob_bytes(out, &m, &table_id, VariantFormat::Generic)).unwrap();
    assert_eq!(s.table_entries, table.len());
    assert_eq!(table.len(), (9 + 1) * 5 + FEATURE_DECALS.len() * 5 + 1);

    // Every texture the table names is in the manifest as BC7 and ASTC 4x4, with no RGBA8.
    let mut ids: BTreeSet<LogicalId> = BTreeSet::new();
    for (_, rec) in table.iter() {
        match rec {
            asset_format::ArtRecord::Terrain(t) => {
                ids.insert(t.texture.clone());
            }
            asset_format::ArtRecord::Feature(f) => ids.extend(f.texture.clone()),
            asset_format::ArtRecord::River(r) => ids.extend(r.textures.iter().cloned()),
            other => panic!("unexpected record {other:?}"),
        }
    }
    assert_eq!(
        ids.len(),
        s.textures - 1 - FOG_CLOUD_TEXTURES.len() - WATER_TEXTURES.len(),
        "all but ice_d.dds, the fog clouds and the water textures are referenced"
    );
    for id in &ids {
        let formats: Vec<_> = m.variants(id).iter().map(|v| v.format).collect();
        // SIMP-022: BC7 and ASTC, no plain variant.
        assert_eq!(formats, [VariantFormat::Bc, VariantFormat::Astc], "{id:?}");
        assert!(
            m.resolve(id, &PlatformProfile::uncompressed_only())
                .is_none()
        );
    }
    assert!(!m.variants(&LogicalId::new("tex:ice_d.dds")).is_empty());
    for f in RIVER_TEXTURES
        .iter()
        .chain(FOG_CLOUD_TEXTURES)
        .chain(WATER_TEXTURES)
    {
        assert!(!m.variants(&LogicalId::new(format!("tex:{f}"))).is_empty());
    }

    // Terrain textures were capped to 512, the 16x16 decals were left alone.
    let big = TextureBlob::decode(&blob_bytes(
        out,
        &m,
        &LogicalId::new("tex:euro_grassland_d.dds"),
        VariantFormat::Bc,
    ))
    .unwrap();
    assert_eq!((big.width, big.height), (512, 512));
    let small = TextureBlob::decode(&blob_bytes(
        out,
        &m,
        &LogicalId::new("tex:feature_marsh.dds"),
        VariantFormat::Bc,
    ))
    .unwrap();
    assert_eq!((small.width, small.height), (16, 16));

    // Prefetch groups: Generic users 1, America 11, Europe 14; the lowest user wins.
    let group = |f: &str| {
        m.variants(&LogicalId::new(format!("tex:{f}")))[0]
            .prefetch
            .0
    };
    assert_eq!(group("amer_grassland_d.dds"), 11);
    assert_eq!(group("afri_plain_d.dds"), 13);
    assert_eq!(group("euro_grassland_d.dds"), 1, "used by Generic style");
    assert_eq!(
        group("euro_mountain_top_d.dds"),
        1,
        "Generic falls back to Europe"
    );
    assert_eq!(group("river_diffuse.dds"), 1);

    // Another set's data in the feature record survives a rebuild; the table is deterministic.
    let mut with_trees = table.clone();
    let mut rec = with_trees.feature("FEATURE_FALLOUT", 1).unwrap().clone();
    rec.tree_model = Some(LogicalId::new("model:forest_america"));
    rec.placement.insert("trees".into(), 12.0);
    with_trees.insert_feature("FEATURE_FALLOUT", 1, rec);
    with_trees.insert_feature(
        "FEATURE_FOREST",
        1,
        FeatureArt {
            texture: None,
            tree_model: Some(LogicalId::new("model:forest_america")),
            placement: Default::default(),
        },
    );
    let mut b = asset_pipeline::store::ManifestBuilder::open(
        &asset_pipeline::store::BlobStore::open(out).unwrap(),
    )
    .unwrap();
    let store = asset_pipeline::store::BlobStore::open(out).unwrap();
    let (hash, size) = store.put(&with_trees.encode()).unwrap();
    b.add(
        table_id.clone(),
        asset_format::Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: asset_format::PrefetchGroup(1),
        },
    );
    b.finish(&store).unwrap();
    build_terrain(&fake_read, &types(), out, opts).unwrap();
    let m2 = read_manifest(out);
    let t2 = ArtTable::decode(&blob_bytes(out, &m2, &table_id, VariantFormat::Generic)).unwrap();
    let f = t2.feature("FEATURE_FALLOUT", 1).unwrap();
    assert_eq!(f.tree_model, Some(LogicalId::new("model:forest_america")));
    assert_eq!(f.placement.get("trees"), Some(&12.0));
    assert_eq!(f.texture, Some(LogicalId::new("tex:feature_fallout.dds")));
    assert!(
        t2.feature("FEATURE_FOREST", 1).is_some(),
        "other sets' keys stay"
    );
}

#[test]
fn max_texture_zero_keeps_size_and_no_astc_writes_bc7_only() {
    let tmp = TempDir::new();
    let opts = TextureOptions {
        max_side: None,
        astc: true,
    };
    build_terrain(&fake_read, &types(), tmp.path(), opts).unwrap();
    let m = read_manifest(tmp.path());
    let id = LogicalId::new("tex:euro_grassland_d.dds");
    let t = TextureBlob::decode(&blob_bytes(tmp.path(), &m, &id, VariantFormat::Bc)).unwrap();
    assert_eq!(t.width, 1024);
    assert!(
        m.variants(&id)
            .iter()
            .any(|v| v.format == VariantFormat::Astc)
    );
    let tmp = TempDir::new();
    let opts = TextureOptions {
        max_side: Some(64),
        astc: false,
    };
    build_terrain(&fake_read, &types(), tmp.path(), opts).unwrap();
    let m = read_manifest(tmp.path());
    let formats: Vec<_> = m.variants(&id).iter().map(|v| v.format).collect();
    assert_eq!(formats, [VariantFormat::Bc]);
}

#[test]
fn missing_file_names_it() {
    let tmp = TempDir::new();
    let read = |name: &str| {
        if name == "river_alpha.dds" {
            Ok(None)
        } else {
            fake_read(name)
        }
    };
    let e = build_terrain(&read, &types(), tmp.path(), TextureOptions::default()).unwrap_err();
    assert!(e.to_string().contains("river_alpha.dds"), "{e}");
}

/// Real install and snapshot: every Terrains.Type x style 0-4 has a record whose textures are
/// in the manifest at 512 or less.
#[test]
fn real_install_every_terrain_has_a_record() {
    let (Some(root), Some(db)) = (
        std::env::var_os("CIV5_ROOT"),
        std::env::var_os("CIV5_DB_SNAPSHOT"),
    ) else {
        println!("skipped: CIV5_ROOT or CIV5_DB_SNAPSHOT not set");
        return;
    };
    let game = GameSource::open(PathBuf::from(root)).unwrap();
    let terrains = terrain_types_from_snapshot(Path::new(&db)).unwrap();
    assert!(terrains.len() >= 9, "{terrains:?}");
    let tmp = TempDir::new();
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(n)?) };
    let s = build_terrain(&read, &terrains, tmp.path(), TextureOptions::default()).unwrap();
    println!("{s:?}");
    let m = read_manifest(tmp.path());
    let table = ArtTable::decode(&blob_bytes(
        tmp.path(),
        &m,
        &LogicalId::new(ART_TABLE_ID),
        VariantFormat::Generic,
    ))
    .unwrap();
    for t in &terrains {
        for style in STYLES {
            let rec = table
                .terrain(t, style)
                .unwrap_or_else(|| panic!("no record for {t} style {style}"));
            let blob =
                TextureBlob::decode(&blob_bytes(tmp.path(), &m, &rec.texture, VariantFormat::Bc))
                    .unwrap();
            assert!(blob.width.max(blob.height) <= 512, "{:?}", rec.texture);
        }
    }
    assert!(table.river().is_some());
}
