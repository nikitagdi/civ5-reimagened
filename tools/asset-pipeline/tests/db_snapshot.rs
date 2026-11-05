//! Snapshot readers (`asset_pipeline::db`): unit tests on a mini SQLite built here, and
//! `real_snapshot_*` against the real snapshot when CIV5_DB_SNAPSHOT is set.

use std::collections::BTreeSet;
use std::path::Path;

use asset_pipeline::db::{self, DbError, SnapDb};

/// Mini snapshot with the same columns as the real tables (only those the readers use).
fn mini() -> SnapDb {
    let db = SnapDb::open_memory().unwrap();
    db.exec(
        "CREATE TABLE Technologies (Type text, Era text);
         INSERT INTO Technologies VALUES ('TECH_AGRICULTURE','ERA_ANCIENT'),
           ('TECH_STEEL','ERA_INDUSTRIAL'), ('TECH_CHIVALRY','ERA_MEDIEVAL');
         CREATE TABLE Units (Type text, UnitArtInfo text, PrereqTech text,
           UnitArtInfoCulturalVariation integer default 0, UnitArtInfoEraVariation integer default 0);
         INSERT INTO Units VALUES
           ('UNIT_WARRIOR','ART_DEF_UNIT__WARRIOR',NULL,0,0),
           ('UNIT_SETTLER','ART_DEF_UNIT__SETTLER',NULL,1,0),
           ('UNIT_BURMA_SETTLER','ART_DEF_UNIT__SETTLER',NULL,1,0),
           ('UNIT_WORKER','ART_DEF_UNIT__WORKER','TECH_AGRICULTURE',0,1),
           ('UNIT_KNIGHT','ART_DEF_UNIT_KNIGHT','TECH_CHIVALRY',0,0),
           ('UNIT_MARINE','ART_DEF_UNIT_MARINE','TECH_STEEL',0,0),
           ('UNIT_ORPHAN','ART_DEF_UNIT_ORPHAN','TECH_UNKNOWN',0,0);
         CREATE TABLE Civilizations (Type text, ArtStyleSuffix text, ArtStylePrefix text,
           DefaultPlayerColor text);
         INSERT INTO Civilizations VALUES
           ('CIVILIZATION_ROME','_EURO','EUROPEAN','PLAYERCOLOR_ROME'),
           ('CIVILIZATION_CHINA','_ASIA','ASIAN','PLAYERCOLOR_CHINA'),
           ('CIVILIZATION_AMERICA','_EURO','EUROPEAN','PLAYERCOLOR_ROME'),
           ('CIVILIZATION_BARBARIAN',NULL,NULL,NULL);
         CREATE TABLE ArtDefine_UnitInfos (Type text primary key, DamageStates int, Formation text);
         INSERT INTO ArtDefine_UnitInfos VALUES
           ('ART_DEF_UNIT__WARRIOR',1,'Warrior'), ('ART_DEF_UNIT__SETTLER',1,'LooseCivilian'),
           ('ART_DEF_UNIT__SETTLER_ASIA',NULL,NULL), ('ART_DEF_UNIT__SETTLER_AFRI',1,NULL),
           ('ART_DEF_UNIT__WORKER',1,'LooseCivilian'), ('ART_DEF_UNIT_KNIGHT',1,'Cavalry');
         CREATE TABLE ArtDefine_UnitInfoMemberInfos (UnitInfoType text, UnitMemberInfoType text, NumMembers int);
         INSERT INTO ArtDefine_UnitInfoMemberInfos VALUES
           ('ART_DEF_UNIT__WARRIOR','M_WARRIOR_V2',3), ('ART_DEF_UNIT__WARRIOR','M_WARRIOR',6),
           ('ART_DEF_UNIT_KNIGHT','M_BROKEN',2);
         CREATE TABLE ArtDefine_UnitMemberInfos (Type text primary key, Scale float, ZOffset float,
           Domain text, Model text not null);
         INSERT INTO ArtDefine_UnitMemberInfos VALUES
           ('M_WARRIOR_V2',0.14000000059605,NULL,NULL,'Assets/Units/Warrior/Warrior_V2.fxsxml'),
           ('M_WARRIOR',0.25,1.5,NULL,'Warrior.fxsxml');
         CREATE TABLE ArtDefine_StrategicView (StrategicViewType text, TileType text not null,
           Asset text not null, primary key (StrategicViewType, TileType));
         INSERT INTO ArtDefine_StrategicView VALUES
           ('ART_DEF_UNIT__WARRIOR','Unit','sv_warrior.dds'),
           ('ART_DEF_IMPROVEMENT_FARM','Improvement','sv_farm.dds');
         CREATE TABLE Colors (Type text, Red float, Green float, Blue float, Alpha float);
         INSERT INTO Colors VALUES ('C_RED',1.0,0.0,0.0,1.0), ('C_HALF',0.5,0.5,0.5,1.0),
           ('C_WHITE',1.0,1.0,1.0,1.0);
         CREATE TABLE PlayerColors (Type text, PrimaryColor text, SecondaryColor text, TextColor text);
         INSERT INTO PlayerColors VALUES ('PLAYERCOLOR_ROME','C_RED','C_HALF','C_WHITE');",
    )
    .unwrap();
    db
}

#[test]
fn units_are_filtered_by_prereq_era() {
    let db = mini();
    let units = db::b1_units(&db).unwrap();
    let names: Vec<&str> = units.iter().map(|u| u.unit_type.as_str()).collect();
    // No prereq, ancient and medieval are in; industrial and an unknown tech are not.
    assert_eq!(
        names,
        [
            "UNIT_BURMA_SETTLER",
            "UNIT_KNIGHT",
            "UNIT_SETTLER",
            "UNIT_WARRIOR",
            "UNIT_WORKER"
        ]
    );
    let worker = units.iter().find(|u| u.unit_type == "UNIT_WORKER").unwrap();
    assert_eq!(worker.era.as_deref(), Some("ERA_ANCIENT"));
    assert_eq!(worker.prereq_tech.as_deref(), Some("TECH_AGRICULTURE"));
    assert!(worker.era_variation && !worker.cultural_variation);
    let warrior = units
        .iter()
        .find(|u| u.unit_type == "UNIT_WARRIOR")
        .unwrap();
    assert!(warrior.prereq_tech.is_none() && warrior.era.is_none());
    let settler = units
        .iter()
        .find(|u| u.unit_type == "UNIT_SETTLER")
        .unwrap();
    assert!(settler.cultural_variation);
}

#[test]
fn art_infos_are_distinct_and_include_style_variants() {
    let db = mini();
    let units = db::b1_units(&db).unwrap();
    let infos = db::b1_art_infos(&db, &units).unwrap();
    // Two settlers share one art info. Its _ASIA variant is a style suffix of a civilization;
    // _AFRI exists as a row but no civilization uses it here, so it is not a variant.
    assert_eq!(
        infos,
        [
            "ART_DEF_UNIT_KNIGHT",
            "ART_DEF_UNIT__SETTLER",
            "ART_DEF_UNIT__SETTLER_ASIA",
            "ART_DEF_UNIT__WARRIOR",
            "ART_DEF_UNIT__WORKER"
        ]
    );
    assert_eq!(db::style_suffixes(&db).unwrap(), ["_ASIA", "_EURO"]);
    assert_eq!(
        db::style_variants(&db, "ART_DEF_UNIT__SETTLER").unwrap(),
        [("_ASIA".to_owned(), "ART_DEF_UNIT__SETTLER_ASIA".to_owned())]
    );
    assert!(
        db::style_variants(&db, "ART_DEF_UNIT_KNIGHT")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn art_chain_rows_in_table_order() {
    let db = mini();
    let info = db::art_info(&db, "ART_DEF_UNIT__WARRIOR").unwrap().unwrap();
    assert_eq!(info.formation.as_deref(), Some("Warrior"));
    assert_eq!(info.damage_states, Some(1));
    let nulls = db::art_info(&db, "ART_DEF_UNIT__SETTLER_ASIA")
        .unwrap()
        .unwrap();
    assert_eq!((nulls.formation, nulls.damage_states), (None, None));
    assert!(db::art_info(&db, "NOPE").unwrap().is_none());

    let m = db::members(&db, "ART_DEF_UNIT__WARRIOR").unwrap();
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].member_type, "M_WARRIOR_V2"); // insertion order, not alphabetical
    assert_eq!(m[0].count, 3);
    assert_eq!(m[0].scale, Some(0.14));
    assert_eq!(m[0].z_offset, None);
    assert_eq!(m[0].model, "Assets/Units/Warrior/Warrior_V2.fxsxml");
    assert_eq!(
        (m[1].count, m[1].scale, m[1].z_offset),
        (6, Some(0.25), Some(1.5))
    );
    assert_eq!(m[1].model, "Warrior.fxsxml");
    assert!(db::members(&db, "NOPE").unwrap().is_empty());
}

#[test]
fn broken_member_chain_is_an_error() {
    let db = mini();
    let err = db::members(&db, "ART_DEF_UNIT_KNIGHT").unwrap_err();
    assert!(matches!(err, DbError::Data { .. }));
    assert!(err.to_string().contains("M_BROKEN"), "{err}");
}

#[test]
fn strategic_view_lookup() {
    let db = mini();
    let rows = db::strategic_view_rows(&db).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].tag, "ART_DEF_IMPROVEMENT_FARM");
    assert_eq!(rows[1].tile_type, "Unit");
    assert_eq!(
        db::strategic_view(&db, "ART_DEF_UNIT__WARRIOR")
            .unwrap()
            .as_deref(),
        Some("sv_warrior.dds")
    );
    assert_eq!(db::strategic_view(&db, "x'y").unwrap(), None);
}

#[test]
fn colors_and_civilizations() {
    let db = mini();
    let civs = db::civilizations(&db).unwrap();
    assert_eq!(civs.len(), 4);
    let barb = civs
        .iter()
        .find(|c| c.civ_type == "CIVILIZATION_BARBARIAN")
        .unwrap();
    assert!(barb.art_style_suffix.is_none() && barb.default_player_color.is_none());
    let china = civs
        .iter()
        .find(|c| c.civ_type == "CIVILIZATION_CHINA")
        .unwrap();
    assert_eq!(china.art_style_prefix.as_deref(), Some("ASIAN"));

    let pcs = db::player_colors(&db).unwrap();
    assert_eq!(pcs.len(), 1);
    assert_eq!(pcs[0].color_type, "PLAYERCOLOR_ROME");
    assert_eq!(pcs[0].color.primary, [255, 0, 0, 255]);
    assert_eq!(pcs[0].color.secondary, [128, 128, 128, 255]);
    assert_eq!(pcs[0].color.text, [255, 255, 255, 255]);

    db.exec("INSERT INTO PlayerColors VALUES ('PLAYERCOLOR_BAD','C_RED','C_MISSING','C_WHITE')")
        .unwrap();
    let err = db::player_colors(&db).unwrap_err();
    assert!(err.to_string().contains("C_MISSING"), "{err}");
}

#[test]
fn missing_table_is_a_sqlite_error() {
    let db = SnapDb::open_memory().unwrap();
    assert!(matches!(db::b1_units(&db), Err(DbError::Sqlite { .. })));
}

fn real_db() -> Option<SnapDb> {
    let dir = std::env::var_os("CIV5_DB_SNAPSHOT")?;
    Some(db::open_snapshot(Path::new(&dir)).expect("open snapshot"))
}

/// The numbers of the DB snapshot: 128 units; 101 ArtInfo = 98 distinct `UnitArtInfo`
/// plus the 3 style variants (`_ASIA`, `_AMER`, `_AFRI`) of the settler.
#[test]
fn real_snapshot_b1_units_and_art_infos() {
    let Some(db) = real_db() else { return };
    let units = db::b1_units(&db).unwrap();
    assert_eq!(units.len(), 128);
    for t in [
        "UNIT_WARRIOR",
        "UNIT_BARBARIAN_WARRIOR",
        "UNIT_BARBARIAN_AXMAN",
        "UNIT_SETTLER",
        "UNIT_WORKBOAT",
        "UNIT_GREAT_GENERAL",
    ] {
        assert!(units.iter().any(|u| u.unit_type == t), "{t}");
    }
    assert_eq!(
        units
            .iter()
            .filter(|u| u.unit_type.starts_with("UNIT_BARBARIAN_"))
            .count(),
        6
    );
    let own: BTreeSet<&str> = units.iter().map(|u| u.art_info.as_str()).collect();
    assert_eq!(own.len(), 98);
    let infos = db::b1_art_infos(&db, &units).unwrap();
    assert_eq!(infos.len(), 101);
    assert_eq!(
        db::style_suffixes(&db).unwrap(),
        ["_AFRI", "_AMER", "_ASIA", "_EURO", "_MED"]
    );

    // The whole chain resolves for every art info.
    let mut members = BTreeSet::new();
    let mut sv = 0;
    for ai in &infos {
        assert!(
            db::art_info(&db, ai).unwrap().is_some(),
            "{ai} missing in ArtDefine_UnitInfos"
        );
        let m = db::members(&db, ai).unwrap();
        assert!(!m.is_empty(), "{ai} has no members");
        members.extend(m.into_iter().map(|x| x.member_type));
        if db::strategic_view(&db, ai).unwrap().is_some() {
            sv += 1;
        }
    }
    eprintln!(
        "B1 art infos {}, members {}, with SV icon {sv}",
        infos.len(),
        members.len()
    );
    let w_info = db::art_info(&db, "ART_DEF_UNIT__WARRIOR").unwrap().unwrap();
    assert_eq!(w_info.formation.as_deref(), Some("Warrior"));
    let w = db::members(&db, "ART_DEF_UNIT__WARRIOR").unwrap();
    assert_eq!(w.len(), 3);
    assert_eq!(w.iter().map(|m| m.count).sum::<u32>(), 12);
    assert_eq!(w[0].scale, Some(0.14));
    assert!(w[0].model.ends_with(".fxsxml"));
}

#[test]
fn real_snapshot_strategic_view_and_colors() {
    let Some(db) = real_db() else { return };
    let rows = db::strategic_view_rows(&db).unwrap();
    assert_eq!(rows.len(), 351);
    assert_eq!(rows.iter().filter(|r| r.tile_type == "Unit").count(), 217);
    let pcs = db::player_colors(&db).unwrap();
    assert_eq!(pcs.len(), 180);
    let blue = pcs
        .iter()
        .find(|p| p.color_type == "PLAYERCOLOR_BLUE")
        .unwrap();
    assert_eq!(blue.color.primary[3], 255);
    let civs = db::civilizations(&db).unwrap();
    let rome = civs
        .iter()
        .find(|c| c.civ_type == "CIVILIZATION_ROME")
        .unwrap();
    assert!(
        pcs.iter()
            .any(|p| Some(&p.color_type) == rome.default_player_color.as_ref())
    );
}
