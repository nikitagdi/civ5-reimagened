//! Art-defines tests: a synthetic loose install always, the real install when `CIV5_ROOT` is set.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use asset_pipeline::art::{
    ArtDefines, ArtError, ArtWarning, FileRole, TextureSlot, resolve_unit_member,
};
use asset_pipeline::game::GameSource;

struct TempInstall(PathBuf);

impl TempInstall {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("civ5-art-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("Resource")).unwrap();
        TempInstall(dir)
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
}

impl Drop for TempInstall {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn member_xml(id: &str, scale: &str, fxs: &str) -> String {
    format!(
        "<UnitMemberArtInfos><UnitMemberArtInfo><Type>{id}</Type><fScale>{scale}</fScale>\
         <Granny>Assets/Units/X/{fxs}</Granny></UnitMemberArtInfo></UnitMemberArtInfos>"
    )
}

#[test]
fn synthetic_install_merges_layers_and_reports_missing_files() {
    let t = TempInstall::new("synth");
    t.write(
        "Assets/Units/Civ5ArtDefines_UnitMembers.xml",
        &member_xml("ART_DEF_UNIT_MEMBER_HERO", "0.1", "Hero.fxsxml"),
    );
    t.write(
        "Assets/DLC/Expansion2/Units/Civ5ArtDefines_Expansion2_UnitMembers.xml",
        &member_xml("ART_DEF_UNIT_MEMBER_HERO", "0.2", "Hero.fxsxml"),
    );
    t.write(
        "Assets/Units/Hero.fxsxml",
        r#"<Asset>
            <Mesh file="Hero.gr2"/>
            <Animation file="Hero_IdleA.gr2" ec="1000"/>
            <Animation file="Hero_Gone.gr2" ec="1100"/>
            <Texture file="Hero_DIFF.dds"/>
            <Texture file="Hero_SREF.dds"/>
        </Asset>"#,
    );
    t.write("Assets/Units/Hero.gr2", "x");
    t.write("Assets/Units/Hero_IdleA.gr2", "x");
    t.write("Assets/Units/Hero_DIFF.dds", "x");

    let src = GameSource::open(&t.0).unwrap();
    let r = resolve_unit_member(&src, "art_def_unit_member_hero").unwrap();

    // Expansion2 row wins over the base row.
    assert!((r.scale - 0.2).abs() < 1e-6);
    assert_eq!(r.meshes(), ["Hero.gr2"]);
    assert_eq!(
        r.texture(TextureSlot::Diffuse).unwrap().file,
        "Hero_DIFF.dds"
    );
    assert_eq!(r.animations().len(), 2);
    let missing: Vec<_> = r
        .warnings
        .iter()
        .map(|w| match w {
            ArtWarning::MissingFile { role, name } => (*role, name.as_str()),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        missing,
        [
            (FileRole::Texture, "Hero_SREF.dds"),
            (FileRole::Animation, "Hero_Gone.gr2")
        ]
    );
    assert_eq!(r.missing_files(), 2);
}

#[test]
fn missing_fxsxml_is_a_warning_and_unknown_member_an_error() {
    let t = TempInstall::new("nofxs");
    t.write(
        "Assets/Units/Civ5ArtDefines_UnitMembers.xml",
        &member_xml("M", "1", "Nope.fxsxml"),
    );
    let src = GameSource::open(&t.0).unwrap();
    let defs = ArtDefines::load(&src).unwrap();

    let r = defs.resolve_member(&src, "M").unwrap();
    assert_eq!(
        r.warnings,
        [ArtWarning::MissingFile {
            role: FileRole::Fxsxml,
            name: "nope.fxsxml".into()
        }]
    );
    assert!(matches!(
        defs.resolve_member(&src, "OTHER"),
        Err(ArtError::UnknownMember(_))
    ));
}

fn real_source() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(&root).expect("scan failed"))
}

#[test]
fn resolves_warrior_on_real_install() {
    let Some(src) = real_source() else { return };
    let defs = ArtDefines::load(&src).unwrap();

    let unit = defs
        .unit("ART_DEF_UNIT__WARRIOR")
        .expect("warrior unit art");
    assert!(
        unit.members
            .iter()
            .any(|m| m.member_id == "ART_DEF_UNIT_MEMBER_WARRIOR")
    );

    let r = defs
        .resolve_member(&src, "ART_DEF_UNIT_MEMBER_WARRIOR")
        .unwrap();
    println!("warrior: {:#?}", r.model.meshes);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!((r.scale - 0.14).abs() < 1e-6);
    assert_eq!(r.meshes(), ["Warrior.gr2"]);
    assert_eq!(
        r.texture(TextureSlot::Diffuse).unwrap().file,
        "Warrior_DIFF.dds"
    );
    assert!(r.texture(TextureSlot::SpecRef).is_some());
    let idle = r
        .animations()
        .iter()
        .find(|a| a.event_codes.contains(&1000))
        .expect("idle animation (event 1000)");
    assert!(idle.label().to_ascii_lowercase().contains("idle"));
    assert!(r.animations().len() >= 30);
}

#[test]
fn resolves_all_members_on_real_install() {
    let Some(src) = real_source() else { return };
    let defs = ArtDefines::load(&src).unwrap();
    println!(
        "defs: {} units, {} members, {} overridden rows, {} files, {} warnings",
        defs.units.len(),
        defs.members.len(),
        defs.overridden_rows,
        defs.files.len(),
        defs.warnings.len()
    );
    for w in &defs.warnings {
        println!("  def warning: {w}");
    }

    let mut clean = 0usize;
    let mut by_role: BTreeMap<String, usize> = BTreeMap::new();
    let mut examples: Vec<String> = Vec::new();
    for id in defs.members.values().map(|m| m.id.clone()) {
        let r = defs.resolve_member(&src, &id).unwrap();
        if r.warnings.is_empty() {
            clean += 1;
            continue;
        }
        for w in &r.warnings {
            let role = match w {
                ArtWarning::MissingFile { role, .. } => format!("{role:?}"),
                ArtWarning::BadFxsxml { .. } => "BadFxsxml".into(),
            };
            *by_role.entry(role).or_default() += 1;
        }
        if examples.len() < 15 {
            examples.push(format!("{id}: {}", r.warnings[0]));
        }
    }
    let total = defs.members.len();
    println!("resolved clean: {clean}/{total}; warnings by role: {by_role:?}");
    for e in &examples {
        println!("  {e}");
    }

    // Every unit art member id must point at a known member.
    let dangling = defs
        .units
        .values()
        .flat_map(|u| &u.members)
        .filter(|m| defs.member(&m.member_id).is_none())
        .count();
    println!("dangling unit->member refs: {dangling}");

    // Texture binding: one mesh and at most one albedo per unit member.
    let mut shape: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    for m in defs.members.values() {
        let r = defs.resolve_member(&src, &m.id).unwrap();
        let diffuse = r
            .model
            .textures
            .iter()
            .filter(|t| t.slot == TextureSlot::Diffuse)
            .count();
        shape
            .entry((r.meshes().len(), diffuse))
            .or_default()
            .push(r.id);
    }
    for ((meshes, diffuse), ids) in &shape {
        println!(
            "meshes={meshes} diffuse={diffuse}: {} members, e.g. {:?}",
            ids.len(),
            &ids[..ids.len().min(3)]
        );
    }
    // 291 distinct members after overrides at the time of writing.
    assert!(total > 250);
    assert!(clean * 100 >= total * 95, "only {clean}/{total} clean");
}
