//! The B2 art table `tb:art-b2` (`C5AT` v2): the unit records built from the
//! DB snapshot, and the `VisEffectArtInfo` rows of the XML defines.
//!
//! The pipeline puts every row in, it chooses nothing: which style or era variant draws is the
//! client's (`world-objects`, RW-14/RW-15), and a NULL `Scale` / `ZOffset` stays NULL.

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{
    ArtTable, FigureB2, LogicalId, PrefetchGroup, UnitArtB2, UnitRecordB2, UnitVariantB2, Variant,
    VariantFormat, VisEffectArt,
};

use crate::db::{ArtInfoRow, MemberInfoRow, UnitRow};
use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder};

/// Manifest id of the table (AB-D11).
pub const ART_TABLE_B2_ID: &str = "tb:art-b2";

/// Era tags that follow a base art info in the type names of the snapshot (`..._WORKER_LATE`,
/// `..._CARAVAN_POSTMODERN`). The first is the one the base game uses for the late art of the
/// civilian units; the rest are the era names of `Eras`.
// measured: 2026-10-21, ArtDefine_UnitInfos of the snapshot: `_LATE` 8 types, `_RENAISSANCE` 1
// (CARGO_SHIP), `_POSTMODERN` 4 (CARAVAN, CARGO_SHIP, GREAT_ARTIST/MUSICIAN/WRITER).
pub const ERA_TAGS: [&str; 9] = [
    "_LATE",
    "_ANCIENT",
    "_CLASSICAL",
    "_MEDIEVAL",
    "_RENAISSANCE",
    "_INDUSTRIAL",
    "_MODERN",
    "_POSTMODERN",
    "_FUTURE",
];

/// `md:<stem of the fxsxml, lower case>`.
pub fn md_id(fxsxml: &str) -> LogicalId {
    let flat = fxsxml
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(fxsxml)
        .trim()
        .to_ascii_lowercase();
    let stem = flat.strip_suffix(".fxsxml").unwrap_or(&flat);
    LogicalId::new(format!("md:{stem}"))
}

/// Flat lower-case fxsxml file name of a `Model` cell.
pub fn flat_fxsxml(model: &str) -> String {
    let flat = model
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(model)
        .trim()
        .to_ascii_lowercase();
    if flat.ends_with(".fxsxml") {
        flat
    } else {
        format!("{flat}.fxsxml")
    }
}

/// The art table already in the manifest (empty if there is none).
pub fn load_table(store: &BlobStore, manifest: &ManifestBuilder) -> Result<ArtTable, ModelError> {
    let id = LogicalId::new(ART_TABLE_B2_ID);
    let Some(v) = manifest.manifest().variants(&id).first() else {
        return Ok(ArtTable::new());
    };
    let path = store.blob_path(&v.hash);
    let bytes = std::fs::read(&path).map_err(|e| {
        ModelError::invalid(format!("reading the art table {}: {e}", path.display()))
    })?;
    ArtTable::decode(&bytes)
        .map_err(|e| ModelError::invalid(format!("art table {ART_TABLE_B2_ID}: {e}")))
}

/// Stores `table` as `tb:art-b2` (group 0: by the group table). Returns the blob size.
pub fn store_table(
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    table: &ArtTable,
) -> Result<u64, ModelError> {
    let (hash, size) = store.put(&table.encode())?;
    manifest.add(
        LogicalId::new(ART_TABLE_B2_ID),
        Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: PrefetchGroup(0),
        },
    );
    Ok(size)
}

// ---------------------------------------------------------------- units

/// What the unit records are built from (all of it from the snapshot).
#[derive(Debug, Clone, Default)]
pub struct UnitsInput {
    pub units: Vec<UnitRow>,
    pub arts: BTreeMap<String, ArtInfoRow>,
    /// Art info to its members `(member type, NumMembers)` in table order.
    pub art_members: BTreeMap<String, Vec<(String, u32)>>,
    pub members: BTreeMap<String, MemberInfoRow>,
    /// `Civilizations.ArtStyleSuffix` values.
    pub style_suffixes: Vec<String>,
}

/// Era variants of `base` among the art infos `all`: `(tag, art info)`. The name is the base type
/// plus an era tag ([`ERA_TAGS`]); a base that starts `ART_DEF_UNIT_GREAT_` is also tried without
/// `GREAT_` (the artist's late art is `ART_DEF_UNIT_ARTIST_LATE`). The first match of a tag wins.
pub fn era_variants(base: &str, all: &BTreeSet<&str>) -> Vec<(String, String)> {
    let alias = base
        .strip_prefix("ART_DEF_UNIT_GREAT_")
        .map(|rest| format!("ART_DEF_UNIT_{rest}"));
    let mut out = Vec::new();
    for tag in ERA_TAGS {
        for stem in std::iter::once(base).chain(alias.as_deref()) {
            let name = format!("{stem}{tag}");
            if all.contains(name.as_str()) {
                out.push((tag.to_owned(), name));
                break;
            }
        }
    }
    out
}

/// A unit art info as a record: its members, in table order.
fn art_of(art_info: &str, input: &UnitsInput) -> Result<UnitArtB2, String> {
    let row = input
        .arts
        .get(art_info)
        .ok_or_else(|| format!("{art_info} has no ArtDefine_UnitInfos row"))?;
    let members = input
        .art_members
        .get(art_info)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| format!("{art_info} has no ArtDefine_UnitInfoMemberInfos rows"))?;
    let mut figures = Vec::new();
    for (member, count) in members {
        let m = input
            .members
            .get(member)
            .ok_or_else(|| format!("member {member} has no ArtDefine_UnitMemberInfos row"))?;
        figures.push(FigureB2 {
            member: member.clone(),
            model: md_id(&m.model),
            count: *count,
            scale: m.scale,
            z_offset: m.z_offset,
            domain: m.domain.clone(),
        });
    }
    Ok(UnitArtB2 {
        art_info: art_info.to_owned(),
        figures,
        damage_states: row.damage_states.and_then(|d| u8::try_from(d).ok()),
        formation: row.formation.clone(),
    })
}

/// The records `unit/<UNIT_TYPE>` of every unit, and what could not be built: unit type to the
/// reason (a broken art chain). Variants whose chain is broken are left out and named in the
/// reason of the unit (the base art decides whether the unit is complete; a broken variant is an
/// error too, nothing is dropped silently).
pub fn unit_records(
    input: &UnitsInput,
) -> (BTreeMap<String, UnitRecordB2>, BTreeMap<String, String>) {
    let all: BTreeSet<&str> = input.arts.keys().map(String::as_str).collect();
    let mut records = BTreeMap::new();
    let mut broken = BTreeMap::new();
    for u in &input.units {
        let mut problems: Vec<String> = Vec::new();
        let base = art_of(&u.art_info, input).unwrap_or_else(|e| {
            problems.push(e);
            UnitArtB2 {
                art_info: u.art_info.clone(),
                figures: Vec::new(),
                damage_states: None,
                formation: None,
            }
        });
        let mut style = Vec::new();
        if u.cultural_variation {
            for suffix in &input.style_suffixes {
                let name = format!("{}{suffix}", u.art_info);
                if all.contains(name.as_str()) {
                    match art_of(&name, input) {
                        Ok(art) => style.push(UnitVariantB2 {
                            suffix: suffix.clone(),
                            art,
                        }),
                        Err(e) => problems.push(e),
                    }
                }
            }
        }
        let mut era = Vec::new();
        if u.era_variation {
            for (tag, name) in era_variants(&u.art_info, &all) {
                match art_of(&name, input) {
                    Ok(art) => era.push(UnitVariantB2 { suffix: tag, art }),
                    Err(e) => problems.push(e),
                }
            }
        }
        if problems.is_empty() {
            records.insert(
                u.unit_type.clone(),
                UnitRecordB2 {
                    base,
                    style_variants: style,
                    era_variation: u.era_variation,
                    era_variants: era,
                },
            );
        } else {
            broken.insert(u.unit_type.clone(), problems.join("; "));
        }
    }
    (records, broken)
}

/// The art infos an embarked unit is drawn with, whatever its own type: the three of the engine and
/// the two the DLL sets as a player's `EmbarkedGraphicOverride`.
// re: CvDLLUtility::GameplayUnitEmbark 0x087a3924 names `ART_DEF_UNIT_TRANSPORT`, `_GALLEON` and
// `_GALLEY` (0x087a3ddc..0x087a3e23); CvPlayer::init (Lekmod CvPlayer.cpp) sets the Polynesian war
// canoe and the Danish longboat.
pub const EMBARK_ARTS: [&str; 5] = [
    "ART_DEF_UNIT_GALLEY",
    "ART_DEF_UNIT_GALLEON",
    "ART_DEF_UNIT_TRANSPORT",
    "ART_DEF_UNIT_U_POLYNESIAN_WAR_CANOE",
    "ART_DEF_UNIT_U_DANISH_LONGBOAT",
];

/// The records `unit/<ART_DEF_UNIT_...>` of the embark arts ([`EMBARK_ARTS`]): the art info as the
/// base art, no variants. An art info missing from the snapshot is left out and named in the second
/// list.
pub fn embark_records(input: &UnitsInput) -> (BTreeMap<String, UnitRecordB2>, Vec<String>) {
    let mut records = BTreeMap::new();
    let mut broken = Vec::new();
    for name in EMBARK_ARTS {
        match art_of(name, input) {
            Ok(base) => {
                records.insert(
                    name.to_owned(),
                    UnitRecordB2 {
                        base,
                        style_variants: Vec::new(),
                        era_variation: false,
                        era_variants: Vec::new(),
                    },
                );
            }
            Err(e) => broken.push(e),
        }
    }
    (records, broken)
}

// ---------------------------------------------------------------- VisEffectArtInfo

fn text(node: roxmltree::Node<'_, '_>, tag: &str) -> Option<String> {
    node.children()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.text())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

fn number(node: roxmltree::Node<'_, '_>, tag: &str) -> Result<Option<f32>, String> {
    text(node, tag)
        .map(|t| {
            t.parse::<f32>()
                .map_err(|_| format!("<{tag}> {t:?} is not a number"))
        })
        .transpose()
}

fn xyz(node: Option<roxmltree::Node<'_, '_>>, what: &str) -> Result<[f32; 3], String> {
    let Some(n) = node else { return Ok([0.0; 3]) };
    let mut out = [0.0f32; 3];
    for (o, axis) in out.iter_mut().zip(["x", "y", "z"]) {
        if let Some(v) = n.attribute(axis) {
            *o = v
                .trim()
                .parse()
                .map_err(|_| format!("{what} {axis}={v:?} is not a number"))?;
        }
    }
    Ok(out)
}

fn names(node: roxmltree::Node<'_, '_>, list: &str, item: &str) -> Vec<String> {
    node.children()
        .filter(|n| n.has_tag_name(list))
        .flat_map(|l| l.children().filter(|n| n.has_tag_name(item)))
        .filter_map(|n| text(n, "Name"))
        .collect()
}

/// The `VisEffectArtInfo` elements of one file, in file order. An element without a `Name` or
/// with a malformed number is an error naming it.
pub fn parse_vis_effects(xml: &str) -> Result<Vec<VisEffectArt>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("invalid XML: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "VisEffectArtInfos" {
        return Err(format!(
            "unexpected root <{}>, expected <VisEffectArtInfos>",
            root.tag_name().name()
        ));
    }
    let mut out = Vec::new();
    for e in root
        .children()
        .filter(|n| n.has_tag_name("VisEffectArtInfo"))
    {
        let name = text(e, "Name").ok_or("a VisEffectArtInfo without a <Name>")?;
        let wrap = |m: String| format!("{name}: {m}");
        let transform = e.children().find(|n| n.has_tag_name("Transform"));
        let tchild = |tag: &str| transform.and_then(|t| t.children().find(|n| n.has_tag_name(tag)));
        let scale = tchild("Scale")
            .and_then(|n| n.text())
            .map(|t| {
                t.trim()
                    .parse::<f32>()
                    .map_err(|_| wrap(format!("Scale {t:?} is not a number")))
            })
            .transpose()?
            .unwrap_or(1.0);
        let models = e
            .children()
            .filter(|n| n.has_tag_name("ModelEffects"))
            .flat_map(|l| l.children().filter(|n| n.has_tag_name("ModelEffect")))
            .filter_map(|n| {
                text(n, "Name").map(|nm| (nm, text(n, "Animation").and_then(|a| a.parse().ok())))
            })
            .collect();
        let children = e
            .children()
            .filter(|n| n.has_tag_name("ChildEffects"))
            .flat_map(|l| l.children().filter(|n| n.has_tag_name("ChildEffect")))
            .filter_map(|n| {
                text(n, "Name").map(|nm| (n.attribute("trigger").unwrap_or("").to_owned(), nm))
            })
            .collect();
        out.push(VisEffectArt {
            translation: xyz(tchild("Translation"), "Translation").map_err(wrap)?,
            rotation: xyz(tchild("Rotation"), "Rotation").map_err(wrap)?,
            scale,
            base_duration: number(e, "fBaseDuration").map_err(wrap)?.unwrap_or(0.0),
            base_duration_scale: number(e, "fBaseDurationScale")
                .map_err(wrap)?
                .unwrap_or(1.0),
            movement_speed: number(e, "fMovementSpeed").map_err(wrap)?,
            fow_mask: text(e, "FOWMask").and_then(|t| t.parse().ok()),
            particles: names(e, "ParticleEffects", "ParticleEffect"),
            models,
            children,
            sounds: names(e, "SoundEffects", "SoundEffect"),
            name,
        });
    }
    Ok(out)
}

/// True for the file names of the XML defines that hold `VisEffectArtInfo` rows
/// (`civ5artdefines_viseffects.xml`, `..._viseffects_expansion2.xml`, ...); not the particle and
/// model lists.
pub fn is_vis_effects_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".xml")
        && (n == "civ5artdefines_viseffects.xml" || n.starts_with("civ5artdefines_viseffects_"))
}

/// Rows of several files, base file first then the others by name; a later row with the same
/// name replaces the earlier one (as the layers of the install do). Returns the rows in name
/// order and the names that were replaced.
pub fn merge_vis_effects(
    files: &[(String, Vec<VisEffectArt>)],
) -> (Vec<VisEffectArt>, Vec<String>) {
    let mut order: Vec<&(String, Vec<VisEffectArt>)> = files.iter().collect();
    order.sort_by(|a, b| {
        let key = |n: &str| (n != "civ5artdefines_viseffects.xml", n.to_owned());
        key(&a.0).cmp(&key(&b.0))
    });
    let mut by_name: BTreeMap<String, VisEffectArt> = BTreeMap::new();
    let mut replaced = BTreeSet::new();
    for (_, rows) in order {
        for r in rows {
            if by_name.insert(r.name.clone(), r.clone()).is_some() {
                replaced.insert(r.name.clone());
            }
        }
    }
    (
        by_name.into_values().collect(),
        replaced.into_iter().collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(t: &str, model: &str, scale: Option<f32>, z: Option<f32>) -> MemberInfoRow {
        MemberInfoRow {
            member_type: t.to_owned(),
            scale,
            z_offset: z,
            domain: None,
            model: model.to_owned(),
        }
    }

    fn input() -> UnitsInput {
        let art = |t: &str, d: Option<i64>| {
            (
                t.to_owned(),
                ArtInfoRow {
                    art_info: t.to_owned(),
                    formation: Some("Infantry".to_owned()),
                    damage_states: d,
                },
            )
        };
        let unit = |t: &str, a: &str, cv: bool, ev: bool| UnitRow {
            unit_type: t.to_owned(),
            art_info: a.to_owned(),
            prereq_tech: None,
            era: None,
            cultural_variation: cv,
            era_variation: ev,
        };
        UnitsInput {
            units: vec![
                unit("UNIT_ARTIST", "ART_DEF_UNIT_GREAT_ARTIST", false, true),
                unit("UNIT_SETTLER", "ART_DEF_UNIT_SETTLER", true, false),
                unit("UNIT_BROKEN", "ART_DEF_UNIT_NOPE", false, false),
            ],
            arts: [
                art("ART_DEF_UNIT_GREAT_ARTIST", Some(1)),
                art("ART_DEF_UNIT_ARTIST_LATE", None),
                art("ART_DEF_UNIT_GREAT_ARTIST_POSTMODERN", None),
                art("ART_DEF_UNIT_SETTLER", Some(1)),
                art("ART_DEF_UNIT_SETTLER_EURO", Some(1)),
                art("ART_DEF_UNIT_SETTLER_ASIA", Some(1)),
            ]
            .into(),
            art_members: [
                ("ART_DEF_UNIT_GREAT_ARTIST", "M_ARTIST"),
                ("ART_DEF_UNIT_ARTIST_LATE", "M_ARTIST_LATE"),
                ("ART_DEF_UNIT_GREAT_ARTIST_POSTMODERN", "M_ARTIST_PM"),
                ("ART_DEF_UNIT_SETTLER", "M_SETTLER"),
                ("ART_DEF_UNIT_SETTLER_EURO", "M_SETTLER_EURO"),
                // SETTLER_ASIA has no members: a broken variant.
            ]
            .into_iter()
            .map(|(a, m)| (a.to_owned(), vec![(m.to_owned(), 2)]))
            .collect(),
            members: [
                member("M_ARTIST", "Assets/Units/Artist.fxsxml", Some(0.14), None),
                member("M_ARTIST_LATE", "artist_late.fxsxml", None, Some(1.5)),
                member("M_ARTIST_PM", "ARTIST_PM", Some(1.0), Some(0.0)),
                member("M_SETTLER", "Settler.fxsxml", Some(0.1), None),
                member("M_SETTLER_EURO", "Settler_Euro.fxsxml", Some(0.1), None),
            ]
            .into_iter()
            .map(|m| (m.member_type.clone(), m))
            .collect(),
            style_suffixes: vec!["_ASIA".to_owned(), "_EURO".to_owned()],
        }
    }

    #[test]
    fn ids_and_names_are_flat_lower_case() {
        assert_eq!(md_id("Assets/Units/Foo/Bar.fxsxml").as_str(), "md:bar");
        assert_eq!(md_id("WARRIOR").as_str(), "md:warrior");
        assert_eq!(
            flat_fxsxml("U_Sumerian_Phalanx.fxsxml"),
            "u_sumerian_phalanx.fxsxml"
        );
        assert_eq!(flat_fxsxml("dhow"), "dhow.fxsxml");
    }

    #[test]
    fn era_variants_follow_the_naming_and_the_great_alias() {
        let i = input();
        let all: BTreeSet<&str> = i.arts.keys().map(String::as_str).collect();
        let v = era_variants("ART_DEF_UNIT_GREAT_ARTIST", &all);
        let tags: Vec<_> = v.iter().map(|(t, n)| (t.as_str(), n.as_str())).collect();
        assert_eq!(
            tags,
            [
                ("_LATE", "ART_DEF_UNIT_ARTIST_LATE"),
                ("_POSTMODERN", "ART_DEF_UNIT_GREAT_ARTIST_POSTMODERN")
            ]
        );
        assert!(era_variants("ART_DEF_UNIT_SETTLER", &all).is_empty());
    }

    #[test]
    fn unit_records_keep_db_values_and_report_a_broken_chain() {
        let (records, broken) = unit_records(&input());
        let a = &records["UNIT_ARTIST"];
        assert_eq!(a.base.figures[0].model.as_str(), "md:artist");
        assert_eq!(a.base.figures[0].scale, Some(0.14));
        assert_eq!(a.base.figures[0].z_offset, None, "NULL stays NULL");
        assert_eq!(a.base.damage_states, Some(1));
        assert_eq!(a.base.formation.as_deref(), Some("Infantry"));
        assert!(a.era_variation);
        assert_eq!(a.era_variants.len(), 2);
        assert_eq!(a.era_variants[0].art.figures[0].z_offset, Some(1.5));
        assert!(a.style_variants.is_empty());
        // The settler's ASIA variant has no members: the unit is broken and says why.
        assert!(!records.contains_key("UNIT_SETTLER"));
        assert!(
            broken["UNIT_SETTLER"].contains("ART_DEF_UNIT_SETTLER_ASIA"),
            "{broken:?}"
        );
        assert!(broken["UNIT_BROKEN"].contains("no ArtDefine_UnitInfos row"));
    }

    #[test]
    fn style_variants_are_found_by_suffix() {
        let mut i = input();
        i.art_members.insert(
            "ART_DEF_UNIT_SETTLER_ASIA".to_owned(),
            vec![("M_SETTLER_EURO".to_owned(), 1)],
        );
        let (records, broken) = unit_records(&i);
        assert!(!broken.contains_key("UNIT_SETTLER"), "{broken:?}");
        let s = &records["UNIT_SETTLER"];
        let suffixes: Vec<_> = s.style_variants.iter().map(|v| v.suffix.as_str()).collect();
        assert_eq!(suffixes, ["_ASIA", "_EURO"]);
        assert_eq!(
            s.for_style(Some("_EURO")).figures[0].model.as_str(),
            "md:settler_euro"
        );
    }

    const VIS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<VisEffectArtInfos>
  <VisEffectArtInfo>
    <Name>ART_DEF_VEFFECT_UNIT_SLING_TRAIL_01</Name>
    <Transform>
      <Translation x="0.0" y="1.5" z="-2"/>
      <Rotation x="0.0" y="90.0" z="90.0"/>
      <Scale>0.22</Scale>
    </Transform>
    <fMovementSpeed>3.0</fMovementSpeed>
    <fBaseDuration>0.1</fBaseDuration>
    <fBaseDurationScale>1.0</fBaseDurationScale>
    <FOWMask>2</FOWMask>
    <ParticleEffects><ParticleEffect><Name>ART_DEF_PEFFECT_A</Name></ParticleEffect></ParticleEffects>
    <ModelEffects><ModelEffect><Name>ART_DEF_MEFFECT_B</Name><Animation>1000</Animation></ModelEffect></ModelEffects>
    <ChildEffects><ChildEffect trigger="ProjectileEnd"><Name>ART_DEF_VEFFECT_C_$(TERRAIN)</Name></ChildEffect></ChildEffects>
    <SoundEffects><SoundEffect><Name>AS3D_X</Name></SoundEffect></SoundEffects>
  </VisEffectArtInfo>
  <VisEffectArtInfo><Name>ART_DEF_VEFFECT_BARE</Name></VisEffectArtInfo>
</VisEffectArtInfos>"#;

    #[test]
    fn vis_effects_parse_all_parts() {
        let v = parse_vis_effects(VIS).unwrap();
        assert_eq!(v.len(), 2);
        let e = &v[0];
        assert_eq!(e.name, "ART_DEF_VEFFECT_UNIT_SLING_TRAIL_01");
        assert_eq!(e.translation, [0.0, 1.5, -2.0]);
        assert_eq!(e.rotation, [0.0, 90.0, 90.0]);
        assert_eq!(e.scale, 0.22);
        assert_eq!(e.movement_speed, Some(3.0));
        assert_eq!(e.fow_mask, Some(2));
        assert_eq!(e.particles, ["ART_DEF_PEFFECT_A"]);
        assert_eq!(e.models, [("ART_DEF_MEFFECT_B".to_owned(), Some(1000))]);
        assert_eq!(
            e.children,
            [(
                "ProjectileEnd".to_owned(),
                "ART_DEF_VEFFECT_C_$(TERRAIN)".to_owned()
            )]
        );
        assert_eq!(e.sounds, ["AS3D_X"]);
        // Missing parts get the defaults of the schema.
        assert_eq!(v[1].scale, 1.0);
        assert_eq!(v[1].base_duration_scale, 1.0);
        assert!(v[1].particles.is_empty());
    }

    #[test]
    fn bad_vis_effect_files_are_errors() {
        assert!(parse_vis_effects("<Other/>").is_err());
        assert!(parse_vis_effects("<VisEffectArtInfos>").is_err());
        assert!(
            parse_vis_effects("<VisEffectArtInfos><VisEffectArtInfo/></VisEffectArtInfos>")
                .is_err()
        );
        let e = parse_vis_effects(
            "<VisEffectArtInfos><VisEffectArtInfo><Name>X</Name><fBaseDuration>fast</fBaseDuration></VisEffectArtInfo></VisEffectArtInfos>",
        )
        .unwrap_err();
        assert!(e.starts_with("X:"), "{e}");
    }

    #[test]
    fn later_files_replace_earlier_rows() {
        let row = |n: &str, s: f32| VisEffectArt {
            name: n.to_owned(),
            translation: [0.0; 3],
            rotation: [0.0; 3],
            scale: s,
            base_duration: 0.0,
            base_duration_scale: 1.0,
            movement_speed: None,
            fow_mask: None,
            particles: vec![],
            models: vec![],
            children: vec![],
            sounds: vec![],
        };
        let files = vec![
            (
                "civ5artdefines_viseffects_expansion.xml".to_owned(),
                vec![row("A", 2.0), row("C", 1.0)],
            ),
            (
                "civ5artdefines_viseffects.xml".to_owned(),
                vec![row("A", 1.0), row("B", 1.0)],
            ),
        ];
        let (rows, replaced) = merge_vis_effects(&files);
        assert_eq!(
            rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["A", "B", "C"]
        );
        assert_eq!(
            rows[0].scale, 2.0,
            "the expansion file wins over the base file"
        );
        assert_eq!(replaced, ["A"]);
        assert!(is_vis_effects_file("CIV5ArtDefines_VisEffects_DLC_02.xml"));
        assert!(is_vis_effects_file("civ5artdefines_viseffects.xml"));
        assert!(!is_vis_effects_file(
            "civ5artdefines_viseffectparticles.xml"
        ));
        assert!(!is_vis_effects_file("civ5artdefines_viseffects.xsd"));
    }
}
