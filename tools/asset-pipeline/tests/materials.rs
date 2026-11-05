//! Several materials per model on real data: the B1 units whose main
//! fxsxml lists several diffuse textures, and the city building sets. Needs `CIV5_ROOT` and,
//! for the units, `CIV5_DB_SNAPSHOT`; skipped otherwise. Slow in debug (Granny decoding): run
//! with `cargo test --release -p asset-pipeline --test materials -- --nocapture`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::ModelBlob;
use asset_pipeline::art::TextureSlot;
use asset_pipeline::db;
use asset_pipeline::game::GameSource;
use asset_pipeline::models::set::{LoadedMember, load_fxsxml, loaded_glb};
use asset_pipeline::models::{MaterialNote, NoteKind};

fn game() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(&root).expect("scan failed"))
}

/// Converts `m` and checks the blob: decodes, every mesh names an existing material, every
/// material is drawn by some mesh. Returns the number of engine materials.
fn check_model(m: &LoadedMember) -> usize {
    let blobs = m
        .convert()
        .unwrap_or_else(|e| panic!("{}: {e}", m.resolved.fxsxml));
    let model = ModelBlob::decode(&blobs.model.encode()).expect("blob decodes");
    let used: BTreeSet<u16> = model.meshes.iter().map(|x| x.material).collect();
    assert_eq!(
        used.len(),
        model.materials.len(),
        "{}: every material is used ({used:?} of {})",
        m.resolved.fxsxml,
        model.materials.len()
    );
    assert!(
        model.materials.iter().all(|x| x.diffuse.is_some()),
        "{}: {:?}",
        m.resolved.fxsxml,
        model.materials
    );
    model.materials.len()
}

fn print_plan(m: &LoadedMember) {
    for (i, (d, s)) in m.plan.materials.iter().enumerate() {
        println!(
            "    material {i}: {} + {}",
            d.as_deref().unwrap_or("-"),
            s.as_deref().unwrap_or("-")
        );
    }
}

#[test]
fn b1_units_with_several_diffuse_textures_build_with_their_materials() {
    let Some(src) = game() else { return };
    let Some(snap) = std::env::var_os("CIV5_DB_SNAPSHOT") else {
        println!("skipped: CIV5_DB_SNAPSHOT not set");
        return;
    };
    let d = db::open_snapshot(Path::new(&snap)).unwrap();
    let units = db::b1_units(&d).unwrap();
    let infos = db::b1_art_infos(&d, &units).unwrap();

    // Members whose fxsxml lists more than one diffuse texture, by art info.
    let mut multi: BTreeMap<String, Vec<(String, String, f32)>> = BTreeMap::new();
    // Art infos whose first ("main") member is one of them.
    let mut main_multi: BTreeSet<String> = BTreeSet::new();
    for ai in &infos {
        for (k, m) in db::members(&d, ai).unwrap().into_iter().enumerate() {
            let flat = m
                .model
                .rsplit(['/', '\\'])
                .next()
                .unwrap()
                .to_ascii_lowercase();
            let Some(bytes) = src.read_by_name(&flat).unwrap() else {
                continue; // Missing art is B9's report, not this test's.
            };
            let fx = asset_pipeline::art::parse_fxsxml(&String::from_utf8_lossy(&bytes)).unwrap();
            let diffuse: BTreeSet<String> = fx
                .textures
                .iter()
                .filter(|t| t.slot == TextureSlot::Diffuse)
                .map(|t| t.file.to_ascii_lowercase())
                .collect();
            if diffuse.len() > 1 {
                if k == 0 {
                    main_multi.insert(ai.clone());
                }
                multi.entry(ai.clone()).or_default().push((
                    m.member_type,
                    m.model,
                    m.scale.unwrap_or(1.0),
                ));
            }
        }
    }
    let types_with = |set: &dyn Fn(&str) -> bool| -> Vec<&str> {
        units
            .iter()
            .filter(|u| {
                set(&u.art_info)
                    || u.cultural_variation
                        && db::style_variants(&d, &u.art_info)
                            .unwrap()
                            .iter()
                            .any(|(_, v)| set(v))
            })
            .map(|u| u.unit_type.as_str())
            .collect()
    };
    let main = types_with(&|a| main_multi.contains(a));
    let any = types_with(&|a| multi.contains_key(a));
    println!(
        "{} unit types (main member): {}",
        main.len(),
        main.join(", ")
    );
    println!("{} unit types (any member): {}", any.len(), any.join(", "));
    // The plan estimated 16 units; the snapshot gives 17 unit types (15 art
    // infos, 18 fxsxml): two Lekmod uniques share the art of a listed unit.
    assert_eq!(
        main,
        [
            "UNIT_ARMENIA_HORSEMAN",
            "UNIT_ASSYRIAN_SIEGE_TOWER",
            "UNIT_BARBARIAN_ARCHER",
            "UNIT_CATAPULT",
            "UNIT_CROSSBOWMAN",
            "UNIT_DANISH_LONGSHIP",
            "UNIT_GALLEASS",
            "UNIT_GREAT_ADMIRAL",
            "UNIT_GREAT_GENERAL",
            "UNIT_MONGOLIAN_KHAN",
            "UNIT_NUBIAN_BOW",
            "UNIT_QUINQ",
            "UNIT_ROMAN_BALLISTA",
            "UNIT_SPEARMAN",
            "UNIT_TREBUCHET",
            "UNIT_VENETIAN_GALLEASS",
            "UNIT_WORKBOAT"
        ]
    );
    assert_eq!(any, main);
    assert_eq!(multi.len(), 15);
    for u in ["UNIT_ARMENIA_HORSEMAN", "UNIT_NUBIAN_BOW"] {
        let ai = &units.iter().find(|x| x.unit_type == u).unwrap().art_info;
        println!("{u} uses {ai}");
    }

    let mut notes: Vec<MaterialNote> = Vec::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (ai, members) in &multi {
        for (id, fxsxml, scale) in members {
            let m = load_fxsxml(&src, id, fxsxml, *scale).unwrap();
            let n = check_model(&m);
            println!("{ai} {} ({id}): {n} material(s)", m.resolved.fxsxml);
            print_plan(&m);
            counts.insert(m.resolved.fxsxml.clone(), n);
            notes.extend(m.plan.notes.iter().cloned());
        }
    }
    println!("report:");
    for n in &notes {
        println!("  {n}");
    }

    // Engine materials per fxsxml: one per distinct Granny material.
    let want = [
        ("barbarian_archer_01.fxsxml", 1),
        ("barbarian_archer_v2.fxsxml", 1),
        ("barbarian_archer_v3.fxsxml", 1),
        ("catapult.fxsxml", 2),
        ("crossbowman.fxsxml", 1),
        ("galleass.fxsxml", 2),
        ("greatgeneral_early.fxsxml", 2),
        ("greatgeneral_early_khan.fxsxml", 2),
        ("great_admiral_early.fxsxml", 2),
        ("spearman_v2.fxsxml", 2),
        ("spearman_v3.fxsxml", 2),
        ("trebuchet.fxsxml", 2),
        ("u_assyrian_siege_tower.fxsxml", 2),
        ("u_carthage_quinquereme.fxsxml", 2),
        ("u_denmark_longboat.fxsxml", 2),
        ("u_roman_ballista.fxsxml", 1),
        ("u_venetian_galleass.fxsxml", 2),
        ("workboat.fxsxml", 2),
    ];
    let got: Vec<(&str, usize)> = counts.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let mut want = want.to_vec();
    want.sort();
    assert_eq!(got, want);

    // Ambiguous: the one-material models with two diffuse textures in their fxsxml.
    let ambiguous: BTreeSet<&str> = notes
        .iter()
        .filter(|n| n.kind == NoteKind::Ambiguous)
        .map(|n| n.model.as_str())
        .collect();
    assert_eq!(
        ambiguous.into_iter().collect::<Vec<_>>(),
        [
            "barbarian_archer_01.fxsxml",
            "barbarian_archer_v2.fxsxml",
            "barbarian_archer_v3.fxsxml",
            "crossbowman.fxsxml",
            "u_roman_ballista.fxsxml"
        ]
    );
    // Spot checks of evidence-based assignments.
    let catapult = load_fxsxml(&src, "catapult", "catapult.fxsxml", 1.0).unwrap();
    assert_eq!(
        catapult.plan.materials[0].0.as_deref(),
        Some("CatapultOperator_DIFF.dds")
    );
    assert_eq!(
        catapult.plan.materials[1].0.as_deref(),
        Some("Catapult_DIFF.dds")
    );
    let workboat = load_fxsxml(&src, "workboat", "workboat.fxsxml", 1.0).unwrap();
    assert_eq!(
        workboat.plan.materials[0],
        (
            Some("Workboat_Net_DIFF.dds".to_owned()),
            Some("Black_SREF.dds".to_owned())
        )
    );
}

#[test]
fn city_building_sets_build_with_a_base_and_a_building_material() {
    let Some(src) = game() else { return };
    let mut notes = Vec::new();
    for culture in ["afri", "amer", "asia", "euro", "medit"] {
        for era in ["anc", "ren"] {
            let fx = format!("{culture}_{era}_city.fxsxml");
            let m = load_fxsxml(&src, &fx, &fx, 1.0).unwrap();
            let n = check_model(&m);
            println!("{fx}: {n} material(s)");
            print_plan(&m);
            assert_eq!(n, 2, "{fx}");
            let (base, city) = (&m.plan.materials[0], &m.plan.materials[1]);
            assert!(
                base.0.as_deref().unwrap().starts_with("CityBase_"),
                "{fx}: {base:?}"
            );
            assert!(
                city.0
                    .as_deref()
                    .unwrap()
                    .to_ascii_lowercase()
                    .starts_with(&format!("{culture}_{era}_city")),
                "{fx}: {city:?}"
            );
            notes.extend(m.plan.notes.iter().cloned());
        }
    }
    let ruins = load_fxsxml(&src, "city_ruins", "city_ruins.fxsxml", 1.0).unwrap();
    assert_eq!(check_model(&ruins), 2);
    print_plan(&ruins);
    notes.extend(ruins.plan.notes.iter().cloned());
    println!("report:");
    for n in &notes {
        println!("  {n}");
    }
    assert!(
        notes.iter().all(|n| n.kind != NoteKind::Ambiguous),
        "{notes:?}"
    );

    // The Europe Ancient set: all 20 buildings, laid out as in the source file, as glTF.
    let euro = load_fxsxml(&src, "euro_anc_city", "euro_anc_city.fxsxml", 1.0).unwrap();
    let blobs = euro.convert().unwrap();
    assert_eq!(blobs.model.meshes.len(), 40);
    assert!(blobs.model.meshes.iter().all(|m| m.joints.is_empty()));
    let xs: Vec<f32> = blobs
        .model
        .meshes
        .iter()
        .map(|m| m.positions.iter().map(|p| p[0]).sum::<f32>() / m.positions.len() as f32)
        .collect();
    let spread =
        xs.iter().copied().fold(f32::MIN, f32::max) - xs.iter().copied().fold(f32::MAX, f32::min);
    assert!(
        spread > 1000.0,
        "pieces keep their InitialPlacement: {spread}"
    );
    let glb = loaded_glb(&src, &euro).unwrap();
    let (doc, _, images) = gltf::import_slice(&glb).expect("glTF loads");
    assert_eq!(images.len(), 2);
    assert_eq!(doc.materials().count(), 2);
}
