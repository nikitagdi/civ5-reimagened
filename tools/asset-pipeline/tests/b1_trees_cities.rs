//! Sets `b1-trees` and `b1-cities`: synthetic checks always; the real install when `CIV5_ROOT`
//! (and `CIV5_DB_SNAPSHOT` for the culture cross-check) is set. Slow in debug (Granny decoding):
//! run with `cargo test --release -p asset-pipeline --test b1_trees_cities -- --nocapture`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{ArtTable, ContentHash, LogicalId, Manifest, ModelBlob, VariantFormat};
use asset_pipeline::db::{civilizations, open_snapshot};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::sets::cities::{self, build_cities, export_city_glb};
use asset_pipeline::sets::formations::{FOREST_KEYS, parse_forests, parse_unit_formations};
use asset_pipeline::sets::trees::{self, build_trees, export_tree_glb};
use asset_pipeline::sets::{ART_TABLE_ID, TextureOptions};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-b1tc-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
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

fn game() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(&root).expect("scan failed"))
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
fn trees_build_needs_its_parameter_files() {
    let tmp = TempDir::new();
    let nothing = |_: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(None) };
    let e = build_trees(&nothing, tmp.path(), TextureOptions::default()).unwrap_err();
    assert!(e.to_string().contains("unitformations.xml"), "{e}");

    let only_formations = |n: &str| -> Result<Option<Vec<u8>>, ModelError> {
        Ok((n == "unitformations.xml").then(|| {
            b"<Formations><Formation><Name>A</Name><SpacingX>1</SpacingX><SpacingY>1</SpacingY>\
              <TotalWidth>1</TotalWidth><UnitRandomness>0</UnitRandomness><Stagger>0</Stagger>\
              </Formation></Formations>"
                .to_vec()
        }))
    };
    let e = build_trees(&only_formations, tmp.path(), TextureOptions::default()).unwrap_err();
    assert!(e.to_string().contains("forests.xml"), "{e}");
}

#[test]
fn real_install_parameter_files_parse() {
    let Some(src) = game() else { return };
    let text =
        |n: &str| String::from_utf8_lossy(&src.read_by_name(n).unwrap().expect(n)).into_owned();
    let f = parse_unit_formations(&text(trees::UNIT_FORMATIONS_FILE)).unwrap();
    println!(
        "{} formations, first {:?}",
        f.len(),
        f.first().map(|f| &f.0)
    );
    assert!(f.len() >= 10);
    let p = parse_forests(&text(trees::FORESTS_FILE)).unwrap();
    println!("forests.xml: {p:?}");
    assert_eq!(p["num_tiles"], 4.0);
    assert_eq!(p["random_trees_per_tile"], 360.0);
    assert_eq!(p["gridded_trees_per_tile"], 0.0);
    assert_eq!(p["overlay_alpha"], 0.6);
    assert_eq!(p["space_between"], 1.33);
    // The tile masks exist.
    for i in 1..=4 {
        assert!(src.lookup(&format!("forest_tile{i}.dds")).is_some());
    }
}

#[test]
fn real_install_trees_and_cities_build_and_export() {
    let Some(src) = game() else { return };
    let tmp = TempDir::new();
    let out = tmp.path().join("out");
    let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(src.read_by_name(n)?) };
    let opts = TextureOptions::default();

    let t = build_trees(&read, &out, opts).unwrap();
    println!("{t:?}");
    assert_eq!(t.feature_records, 10);
    assert_eq!(t.models, 9);
    let c = build_cities(&src, &out, opts).unwrap();
    println!("{c:?}");
    for r in &c.report {
        println!("report: {r}");
    }
    assert_eq!(c.city_records, 12);
    assert_eq!(c.models, 11);
    assert!(c.table_entries > t.table_entries);

    // A second run converts nothing.
    let again = build_cities(&src, &out, opts).unwrap();
    assert_eq!(again.stats.cache_misses, 0, "{again:?}");
    let again = build_trees(&read, &out, opts).unwrap();
    assert_eq!(again.stats.cache_misses, 0, "{again:?}");

    let m = read_manifest(&out);
    let table = ArtTable::decode(&blob_bytes(
        &out,
        &m,
        &LogicalId::new(ART_TABLE_ID),
        VariantFormat::Generic,
    ))
    .unwrap();

    // Every model of a record is in the manifest, decodes, and every texture it names is too.
    let mut ids = BTreeSet::new();
    let mut city_ids = BTreeSet::new();
    for feature in ["FEATURE_FOREST", "FEATURE_JUNGLE"] {
        for style in 0..=4u8 {
            let f = table
                .feature(feature, style)
                .unwrap_or_else(|| panic!("{feature} {style}"));
            ids.insert(f.tree_model.clone().expect("tree model"));
            assert!(!m.variants(f.texture.as_ref().expect("overlay")).is_empty());
            for k in FOREST_KEYS {
                assert!(f.placement.contains_key(k), "{feature} {style}: {k}");
            }
        }
    }
    for culture in [
        "AMERICAN",
        "ASIAN",
        "AFRICAN",
        "EUROPEAN",
        "MEDITERRANEAN",
        "RUINS",
    ] {
        for era in cities::ERAS {
            let id = table
                .city(culture, era)
                .unwrap_or_else(|| panic!("{culture}/{era}"))
                .model
                .clone();
            city_ids.insert(id.clone());
            ids.insert(id);
        }
    }
    assert_eq!(ids.len(), 9 + 11);
    for id in &ids {
        let model = ModelBlob::decode(&blob_bytes(&out, &m, id, VariantFormat::Generic))
            .unwrap_or_else(|e| panic!("{id:?}: {e}"));
        assert!(!model.meshes.is_empty(), "{id:?}");
        // Trees are static; a city catalogue keeps the skeleton of its gr2.
        if !city_ids.contains(id) {
            assert!(model.skeleton.bones.is_empty() && model.animations.is_empty());
        }
        for mat in &model.materials {
            for tex in mat.diffuse.iter().chain(&mat.specular) {
                assert!(
                    !m.variants(tex).is_empty(),
                    "{id:?}: {tex:?} not in the manifest"
                );
            }
        }
    }

    // A city is the whole catalogue (no composition any more): a base quad and a building mesh
    // per part, and two materials.
    let city = table.city("EUROPEAN", "Ancient").unwrap();
    let model =
        ModelBlob::decode(&blob_bytes(&out, &m, &city.model, VariantFormat::Generic)).unwrap();
    assert_eq!(model.materials.len(), 2);
    let city_meshes = model.meshes.len();
    assert!(
        city_meshes >= 2 && city_meshes.is_multiple_of(2),
        "{city_meshes}"
    );
    // Tree variants: one mesh each, at the origin.
    let forest = table
        .feature("FEATURE_FOREST", 4)
        .unwrap()
        .tree_model
        .clone()
        .unwrap();
    let forest = ModelBlob::decode(&blob_bytes(&out, &m, &forest, VariantFormat::Generic)).unwrap();
    assert_eq!(forest.meshes.len(), 12);
    assert_eq!(forest.materials.len(), 1);

    // glTF of the Europe Ancient city and the Europe forest, written out and read back.
    for (name, glb, meshes, images) in [
        (
            "euro_anc_city.glb",
            export_city_glb(&src, "EUROPEAN", "Ancient").unwrap(),
            city_meshes,
            2,
        ),
        (
            "forest_europe.glb",
            export_tree_glb(&src, "forest", "europe").unwrap(),
            12,
            1,
        ),
    ] {
        let path = tmp.path().join(name);
        std::fs::write(&path, &glb).unwrap();
        println!("{name}: {} bytes", glb.len());
        let (doc, _, imgs) = gltf::import_slice(std::fs::read(&path).unwrap()).expect("glTF loads");
        assert_eq!(imgs.len(), images, "{name}");
        let prims: usize = doc.meshes().map(|m| m.primitives().count()).sum();
        assert_eq!(prims, meshes, "{name}");
    }
}

#[test]
fn real_install_every_snapshot_culture_has_a_city() {
    let (Some(_), Some(db)) = (
        std::env::var_os("CIV5_ROOT"),
        std::env::var_os("CIV5_DB_SNAPSHOT"),
    ) else {
        println!("skipped: CIV5_ROOT or CIV5_DB_SNAPSHOT not set");
        return;
    };
    let src = game().unwrap();
    let xml = src.read_by_name(cities::CITY_DEFS_FILE).unwrap().unwrap();
    let plan =
        cities::plan_cities(&cities::parse_city_defs(&String::from_utf8_lossy(&xml)).unwrap())
            .unwrap();
    let keys: BTreeSet<&str> = plan.entries.iter().map(|e| e.culture).collect();
    let snapshot = open_snapshot(Path::new(&db)).unwrap();
    let mut used = BTreeSet::new();
    for civ in civilizations(&snapshot).unwrap() {
        if let Some(prefix) = civ.art_style_prefix {
            used.insert(prefix);
        }
    }
    println!("ArtStylePrefix values in the snapshot: {used:?}");
    assert!(!used.is_empty());
    for prefix in &used {
        assert!(keys.contains(prefix.as_str()), "no city for {prefix}");
    }
}
