//! Set `b1-units`: synthetic fixtures always (a mini snapshot in memory, a game directory with
//! no usable art); the real install when `CIV5_ROOT` and `CIV5_DB_SNAPSHOT` are set. The real
//! tests decode Granny files: run them with `cargo test --release -p asset-pipeline --test
//! b1_units -- --nocapture`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{ArtTable, LogicalId, Manifest, ModelBlob, VariantFormat};
use asset_pipeline::db::SnapDb;
use asset_pipeline::game::GameSource;
use asset_pipeline::models::ModelError;
use asset_pipeline::models::set::{SETS, build_sets, model_id};
use asset_pipeline::report::{REPORT_JSON, Report};
use asset_pipeline::sets::units::{
    SET_NAME, assemble, build_units, model_users, read_input, static_model_id,
};
use asset_pipeline::sets::{ART_TABLE_ID, TextureOptions, sv};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-b1units-test-{}-{}",
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

/// Mini snapshot: a warrior with two members, a settler with style variants (one of them with a
/// broken chain), a knight without an `ArtDefine_UnitInfos` row, a worker with an era variation
/// whose model cannot be built, and a marine of an excluded era.
fn mini() -> SnapDb {
    let db = SnapDb::open_memory().unwrap();
    db.exec(
        "CREATE TABLE Technologies (Type text, Era text);
         INSERT INTO Technologies VALUES ('TECH_AGRICULTURE','ERA_ANCIENT'),
           ('TECH_STEEL','ERA_INDUSTRIAL'), ('TECH_CHIVALRY','ERA_MEDIEVAL');
         CREATE TABLE Units (Type text, UnitArtInfo text, PrereqTech text,
           UnitArtInfoCulturalVariation integer default 0, UnitArtInfoEraVariation integer default 0);
         INSERT INTO Units VALUES
           ('UNIT_WARRIOR','ART_W',NULL,0,0),
           ('UNIT_SETTLER','ART_S',NULL,1,0),
           ('UNIT_WORKER','ART_WK','TECH_AGRICULTURE',0,1),
           ('UNIT_KNIGHT','ART_K','TECH_CHIVALRY',0,0),
           ('UNIT_MARINE','ART_MARINE','TECH_STEEL',0,0);
         CREATE TABLE Civilizations (Type text, ArtStyleSuffix text);
         INSERT INTO Civilizations VALUES ('CIVILIZATION_ROME','_EURO'),
           ('CIVILIZATION_CHINA','_ASIA'), ('CIVILIZATION_ZULU','_AFRI');
         CREATE TABLE ArtDefine_UnitInfos (Type text primary key, DamageStates int, Formation text);
         INSERT INTO ArtDefine_UnitInfos VALUES
           ('ART_W',1,'Warrior'), ('ART_S',1,'LooseCivilian'), ('ART_S_ASIA',1,NULL),
           ('ART_S_AFRI',1,NULL), ('ART_WK',1,'LooseCivilian'), ('ART_MARINE',1,'Marine');
         CREATE TABLE ArtDefine_UnitInfoMemberInfos (UnitInfoType text, UnitMemberInfoType text, NumMembers int);
         INSERT INTO ArtDefine_UnitInfoMemberInfos VALUES
           ('ART_W','M_W1',3), ('ART_W','M_W2',6),
           ('ART_S','M_S',1), ('ART_S_ASIA','M_S',1), ('ART_S_AFRI','M_NOROW',1),
           ('ART_WK','M_WK',1);
         CREATE TABLE ArtDefine_UnitMemberInfos (Type text primary key, Scale float, ZOffset float,
           Domain text, Model text not null);
         INSERT INTO ArtDefine_UnitMemberInfos VALUES
           ('M_W1',0.14,NULL,NULL,'Assets/Units/Warrior/Warrior.fxsxml'),
           ('M_W2',0.25,1.5,NULL,'Warrior.fxsxml'),
           ('M_S',0.2,NULL,NULL,'Settler.fxsxml'),
           ('M_WK',NULL,NULL,NULL,'Worker.fxsxml');
         CREATE TABLE ArtDefine_StrategicView (StrategicViewType text, TileType text not null,
           Asset text not null, primary key (StrategicViewType, TileType));
         INSERT INTO ArtDefine_StrategicView VALUES
           ('ART_W','Unit','sv_w.dds'), ('ART_S','Unit','sv_s.dds'), ('ART_WK','Unit','sv_wk.dds');",
    )
    .unwrap();
    db
}

fn failed(models: &[(&str, &str)]) -> BTreeMap<String, String> {
    models
        .iter()
        .map(|(m, r)| ((*m).to_owned(), (*r).to_owned()))
        .collect()
}

#[test]
fn records_carry_figures_formation_icon_and_style_variants() {
    let input = read_input(&mini()).unwrap();
    // Units of excluded eras are not in the input; the marine's art info is not read.
    assert_eq!(input.units.len(), 4);
    assert!(!input.arts.contains_key("ART_MARINE"));
    let a = assemble(&input, &BTreeMap::new(), &|f| {
        f == "sv_w.dds" || f == "sv_s.dds"
    });

    let w = &a.records["UNIT_WARRIOR"];
    assert!(!w.base.placeholder);
    assert_eq!(w.base.art_info, "ART_W");
    assert_eq!(w.base.formation.as_deref(), Some("Warrior"));
    assert_eq!(
        w.base.sv_icon.as_ref().map(LogicalId::as_str),
        Some("tex:sv_w.dds")
    );
    // Both members name the same fxsxml, so they share one model and differ in scale/offset.
    assert_eq!(w.base.figures.len(), 2);
    assert_eq!(w.base.figures[0].model, w.base.figures[1].model);
    assert_eq!(w.base.figures[0].model.as_str(), "model:static_warrior");
    assert_eq!(
        (
            w.base.figures[0].count,
            w.base.figures[0].scale,
            w.base.figures[0].z_offset
        ),
        (3, 0.14, 0.0)
    );
    assert_eq!(
        (
            w.base.figures[1].count,
            w.base.figures[1].scale,
            w.base.figures[1].z_offset
        ),
        (6, 0.25, 1.5)
    );

    // Variants only for units with a cultural variation, sorted by suffix.
    assert!(w.variants.is_empty());
    let s = &a.records["UNIT_SETTLER"];
    let suffixes: Vec<&str> = s.variants.iter().map(|v| v.suffix.as_str()).collect();
    assert_eq!(suffixes, ["_AFRI", "_ASIA"]);
    assert_eq!(s.for_style(Some("_ASIA")).art_info, "ART_S_ASIA");
    assert_eq!(s.for_style(Some("_EURO")).art_info, "ART_S");
    assert_eq!(a.variant_arts, 2);

    // Scale and z offset have defaults when the snapshot has NULL.
    let wk = &a.records["UNIT_WORKER"].base;
    assert_eq!((wk.figures[0].scale, wk.figures[0].z_offset), (1.0, 0.0));
}

#[test]
fn a_model_that_did_not_build_makes_the_art_a_placeholder_with_the_reason() {
    let input = read_input(&mini()).unwrap();
    let a = assemble(
        &input,
        &failed(&[("warrior.fxsxml", "mesh w.gr2 is missing")]),
        &|_| true,
    );
    let w = &a.records["UNIT_WARRIOR"].base;
    assert!(w.placeholder, "SIMP-011: the marker flag is set");
    assert!(
        w.figures.is_empty(),
        "no model of another unit is substituted"
    );
    assert_eq!(w.formation.as_deref(), Some("Warrior"), "formation is kept");
    let e = a
        .entries
        .iter()
        .find(|e| e.kind == "placeholder" && e.subject == "ART_W")
        .expect("report entry");
    assert!(e.reason.contains("mesh w.gr2 is missing"), "{}", e.reason);
    assert!(e.reason.contains("UNIT_WARRIOR"), "{}", e.reason);
    assert_eq!(
        a.placeholder_units, 2,
        "the warrior and the knight (no art row)"
    );
}

#[test]
fn broken_chains_are_placeholders_and_variants_are_reported_as_variants() {
    let input = read_input(&mini()).unwrap();
    let a = assemble(&input, &BTreeMap::new(), &|_| true);
    // The knight has no ArtDefine_UnitInfos row.
    let k = &a.records["UNIT_KNIGHT"].base;
    assert!(k.placeholder && k.figures.is_empty());
    let ke = a.entries.iter().find(|e| e.subject == "ART_K").unwrap();
    assert_eq!(ke.kind, "placeholder");
    assert!(
        ke.reason.contains("no ArtDefine_UnitInfos row"),
        "{}",
        ke.reason
    );
    // The _AFRI variant of the settler names a member without an ArtDefine_UnitMemberInfos row.
    let s = &a.records["UNIT_SETTLER"];
    assert!(!s.base.placeholder);
    let afri = s.variants.iter().find(|v| v.suffix == "_AFRI").unwrap();
    assert!(afri.art.placeholder);
    let e = a
        .entries
        .iter()
        .find(|e| e.subject == "ART_S_AFRI" && e.kind != "missing-icon")
        .unwrap();
    assert_eq!(e.kind, "placeholder-variant");
    assert!(e.reason.contains("M_NOROW"), "{}", e.reason);
    assert_eq!(a.placeholder_units, 1, "only the knight");
    assert_eq!(a.placeholder_variants, 1);
}

#[test]
fn missing_icons_and_era_variations_are_reported() {
    let input = read_input(&mini()).unwrap();
    // sv_s.dds is not in the manifest; the worker's and the warrior's are.
    let a = assemble(&input, &BTreeMap::new(), &|f| f != "sv_s.dds");
    let s = &a.records["UNIT_SETTLER"].base;
    assert!(
        s.sv_icon.is_none() && !s.placeholder,
        "a missing icon is not a placeholder"
    );
    let icon: Vec<&str> = a
        .entries
        .iter()
        .filter(|e| e.kind == "missing-icon")
        .map(|e| e.subject.as_str())
        .collect();
    // ART_S has a row whose file is missing; its variants have no row at all.
    assert_eq!(icon, ["ART_K", "ART_S", "ART_S_AFRI", "ART_S_ASIA"]);
    let era: Vec<&str> = a
        .entries
        .iter()
        .filter(|e| e.kind == "era-variation")
        .map(|e| e.subject.as_str())
        .collect();
    assert_eq!(era, ["UNIT_WORKER"]);
}

#[test]
fn model_users_lists_each_flat_fxsxml_once() {
    let input = read_input(&mini()).unwrap();
    let users = model_users(&input);
    let names: Vec<&str> = users.keys().map(String::as_str).collect();
    assert_eq!(names, ["settler.fxsxml", "warrior.fxsxml", "worker.fxsxml"]);
    assert_eq!(users["settler.fxsxml"].len(), 2, "ART_S and ART_S_ASIA");
}

#[test]
fn static_ids_do_not_collide_with_the_animated_warrior_set() {
    let animated = model_id("ART_DEF_UNIT_MEMBER_WARRIOR");
    let stat = static_model_id("Assets/Units/Warrior/Warrior.fxsxml");
    assert_ne!(animated, stat);
    assert!(stat.as_str().starts_with("model:static_"));
}

/// A game directory whose art does not build: no packs, one fxsxml with a mesh that is not there.
#[test]
fn build_without_usable_art_gives_placeholders_and_a_report_instead_of_failing() {
    let game_dir = TempDir::new();
    std::fs::create_dir_all(game_dir.path().join("Resource")).unwrap();
    let assets = game_dir.path().join("Assets/Units");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(
        assets.join("Warrior.fxsxml"),
        r#"<Asset><Mesh file="Warrior.gr2"/><Texture file="Warrior_DIFF.dds"/></Asset>"#,
    )
    .unwrap();
    let game = GameSource::open(game_dir.path()).unwrap();
    let input = read_input(&mini()).unwrap();
    let out = TempDir::new();
    let s = build_units(&game, &input, out.path(), TextureOptions::default()).unwrap();
    assert_eq!((s.models, s.failed_models), (0, 3));
    assert_eq!(s.units, 4);
    assert_eq!(
        s.placeholder_units, 4,
        "warrior, settler, worker and the knight"
    );
    let failed: Vec<&str> = s
        .report
        .entries
        .iter()
        .filter(|e| e.kind == "model-failed")
        .map(|e| e.subject.as_str())
        .collect();
    assert_eq!(
        failed,
        ["settler.fxsxml", "warrior.fxsxml", "worker.fxsxml"]
    );
    // Every placeholder has a report entry with a reason.
    let ph: Vec<_> = s
        .report
        .entries
        .iter()
        .filter(|e| e.kind == "placeholder")
        .collect();
    assert_eq!(ph.len(), 4);
    assert!(ph.iter().all(|e| !e.reason.is_empty()));

    // The art table is written with the flagged records.
    let manifest = Manifest::decode(
        &std::fs::read(
            out.path().join(
                std::fs::read_to_string(out.path().join("manifest"))
                    .unwrap()
                    .trim(),
            ),
        )
        .unwrap(),
    )
    .unwrap();
    let id = LogicalId::new(ART_TABLE_ID);
    let v = &manifest.variants(&id)[0];
    let table =
        ArtTable::decode(&std::fs::read(out.path().join(v.hash.to_string())).unwrap()).unwrap();
    assert!(table.unit("UNIT_WARRIOR").unwrap().base.placeholder);
    assert_eq!(SET_NAME, "b1-units");
}

// ------------------------------------------------------------------ real install

fn real() -> Option<(GameSource, PathBuf)> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    let Some(snap) = std::env::var_os("CIV5_DB_SNAPSHOT") else {
        println!("skipped: CIV5_DB_SNAPSHOT not set");
        return None;
    };
    Some((
        GameSource::open(PathBuf::from(root)).unwrap(),
        PathBuf::from(snap),
    ))
}

fn read_manifest(out: &Path) -> Manifest {
    let hash = std::fs::read_to_string(out.join("manifest")).unwrap();
    Manifest::decode(&std::fs::read(out.join(hash.trim())).unwrap()).unwrap()
}

fn read_table(out: &Path, m: &Manifest) -> ArtTable {
    let v = &m.variants(&LogicalId::new(ART_TABLE_ID))[0];
    ArtTable::decode(&std::fs::read(out.join(v.hash.to_string())).unwrap()).unwrap()
}

fn build_real_sv(game: &GameSource, snap: &Path, out: &Path) {
    let input = sv::load_input(snap).unwrap();
    let read = |name: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(name)?) };
    sv::build_sv(&read, &input, out, TextureOptions::default()).unwrap();
}

#[test]
fn real_b1_units_build_without_placeholders_and_with_resolvable_ids() {
    let Some((game, snap)) = real() else { return };
    let out = TempDir::new();
    build_real_sv(&game, &snap, out.path());
    let input = asset_pipeline::sets::units::load_input(&snap).unwrap();
    let s = build_units(&game, &input, out.path(), TextureOptions::default()).unwrap();
    println!(
        "units {} (variant arts {}), models {} ({} B), textures {} ({:?}), meshes {}, triangles {}, dropped {}, table {} B",
        s.units,
        s.variant_arts,
        s.models,
        s.model_bytes,
        s.textures,
        s.texture_bytes,
        s.meshes,
        s.triangles,
        s.dropped_meshes,
        s.table_bytes
    );
    let placeholders: Vec<String> = s
        .report
        .entries
        .iter()
        .filter(|e| e.kind.starts_with("placeholder") || e.kind == "model-failed")
        .map(|e| format!("{} {}: {}", e.kind, e.subject, e.reason))
        .collect();
    assert!(
        s.placeholder_units == 0 && s.placeholder_variants == 0,
        "placeholders (AC-4.1):\n{}",
        placeholders.join("\n")
    );
    assert!(s.units >= 120, "{}", s.units);

    // Every id in a unit record is in the manifest and every model decodes.
    let m = read_manifest(out.path());
    let table = read_table(out.path(), &m);
    let mut models = std::collections::BTreeSet::new();
    let mut icons = 0;
    for u in &input.units {
        let rec = table.unit(&u.unit_type).expect("record");
        let arts = std::iter::once(&rec.base).chain(rec.variants.iter().map(|v| &v.art));
        for art in arts {
            assert!(!art.placeholder, "{}", u.unit_type);
            assert!(!art.figures.is_empty(), "{}", u.unit_type);
            for f in &art.figures {
                assert!(f.count > 0 && f.scale > 0.0, "{}: {f:?}", u.unit_type);
                assert!(
                    !m.variants(&f.model).is_empty(),
                    "{}: {} not in the manifest",
                    u.unit_type,
                    f.model.as_str()
                );
                models.insert(f.model.clone());
            }
            if let Some(i) = &art.sv_icon {
                assert!(!m.variants(i).is_empty(), "{}: {}", u.unit_type, i.as_str());
                icons += 1;
            }
        }
    }
    assert_eq!(models.len(), s.models);
    for id in &models {
        let v = &m.variants(id)[0];
        assert_eq!(v.format, VariantFormat::Generic);
        let model = ModelBlob::decode(&std::fs::read(out.path().join(v.hash.to_string())).unwrap())
            .unwrap();
        assert!(
            model.skeleton.bones.is_empty() && model.animations.is_empty(),
            "{id:?}"
        );
        assert!(!model.meshes.is_empty(), "{id:?}");
        for me in &model.meshes {
            assert!(usize::from(me.material) < model.materials.len(), "{id:?}");
            assert!(
                me.joints.is_empty() && me.weights.is_empty(),
                "{id:?} is rigid"
            );
        }
        for mat in &model.materials {
            let d = mat.diffuse.as_ref().expect("diffuse");
            assert!(!m.variants(d).is_empty(), "{id:?}: {}", d.as_str());
        }
    }
    println!("{} models, {icons} sv icons resolved", models.len());

    // The report lists the B8 and B7 findings and sizes.
    let kinds = s.report.kinds();
    println!("report kinds: {kinds:?}");
    assert!(
        kinds.contains_key("dropped-mesh"),
        "damage-state hulls (SIMP-010)"
    );
    assert!(s.report.bytes["models"] > 0);

    // A second build changes nothing in the manifest and the report entries are the same.
    let before = std::fs::read_to_string(out.path().join("manifest")).unwrap();
    let again = build_units(&game, &input, out.path(), TextureOptions::default()).unwrap();
    assert_eq!(
        std::fs::read_to_string(out.path().join("manifest")).unwrap(),
        before
    );
    assert_eq!(
        again.stats.cache_misses, 0,
        "everything comes from the cache"
    );
    assert_eq!(again.report.entries, s.report.entries);
    let mut r = Report::default();
    r.put(SET_NAME, again.report);
    r.write(out.path()).unwrap();
    assert!(Report::load(out.path()).sets.contains_key(SET_NAME));
    assert!(out.path().join(REPORT_JSON).is_file());
}

#[test]
fn real_static_units_coexist_with_the_animated_warrior_set() {
    let Some((game, snap)) = real() else { return };
    let out = TempDir::new();
    let warrior = SETS.iter().find(|d| d.name == "warrior").unwrap();
    build_sets(&game, &[warrior], out.path()).unwrap();
    let before = read_manifest(out.path());
    let animated = before.variants(&model_id("ART_DEF_UNIT_MEMBER_WARRIOR"));
    assert_eq!(animated.len(), 1);
    let hash = animated[0].hash;
    let input = asset_pipeline::sets::units::load_input(&snap).unwrap();
    build_units(&game, &input, out.path(), TextureOptions::default()).unwrap();
    let after = read_manifest(out.path());
    assert_eq!(
        after.variants(&model_id("ART_DEF_UNIT_MEMBER_WARRIOR"))[0].hash,
        hash,
        "the animated Warrior model is unchanged"
    );
    let stat = after.variants(&static_model_id("Warrior.fxsxml"));
    assert_eq!(stat.len(), 1);
    assert_ne!(stat[0].hash, hash, "the static model is a different blob");
}
