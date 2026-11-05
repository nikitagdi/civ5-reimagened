//! Set `b2-city` (4.15 item 4): the city catalogs of every
//! culture and era, the walls, the wonders, the art of every building type, the civilizations and
//! the damage models of cities, as `tb:art-b2` records, `C5MD` v2 models and `buildings.csv`.
//!
//! # What is a record of what
//! * `city/<culture>/<era>` ([`CityArtB2`]): the catalog gr2 of `Civ5ArtDefines_CityBuildings*.xml`
//!   (7 cultures of 4 eras, the ruins share one model). The catalog is not a city: its gr2 holds
//!   many `Model`s (base quads and buildings) laid out in a row. The model keeps every part with its
//!   name and placement and the record says how many there are; which parts a city of a given size
//!   takes and where they stand is the client's (RW-18). SIMPLIFIED(SIMP-021): narrowed to that; the
//!   composition of one fixed compact city is gone.
//! * `wall/<culture>/<era>` ([`WallArt`]): the install has one wall model per culture
//!   (`<prefix>_walls.fxsxml`), not per era, so the four eras of a culture hold the same model.
//!   The wall of Babylon (`WallArtInfo`) is `wall/ART_DEF_BUILDING_WALLS_OF_BABYLON/Any`.
//! * `wonder/<Type>` ([`WonderArt`]): every `WonderArtInfo` of every `*Wonders*.xml` of the install
//!   (the base game, the expansions and the DLC; a later file of the same type wins and the report
//!   says when the two differ). The model is built whole.
//! * `building/<tag>` ([`BuildingArt`]): one record per `Buildings.ArtDefineTag`. The model is the
//!   wonder art whose `Type` is the tag; variants are the wonder arts whose `Type` is `<WORD> <tag>`
//!   (a culture or an era in front of the tag: `AFRICAN CASTLE`, `ANCIENT HARBOR`) or `<tag> <WORD>`
//!   (a random pick: `STADIUM ONE`). A tag without any of them has no art of its own: the building
//!   is drawn as part of the city catalog (RW-18), and the record says so in `note`.
//! * `civ/<CIV>` ([`CivArt`]): the art columns of `Civilizations`; `style_culture` is the culture of
//!   the city catalog when the art style type points elsewhere than the prefix (Polynesia: prefix
//!   `AMERICAN`, style `ARTSTYLE_POLYNESIAN`; the style type id is the culture ordinal, RW-18).
//! * `fxm_city_damage_state_*`: the nine damage models of cities, plain effect models (`md:`).
//! * `hb_great_wall` ([`GREAT_WALL_FLAT`]): the wall and towers of the Great Wall, parts kept
//!   (`md:hb_great_wall`); no record names it, the client knows the id (R5.11).
//!
//! # `buildings.csv`
//! One line per city catalog, wall, wonder art, building type and damage model, with what was built
//! and `ok` or the reason. A building without art of its own says so and is not a failure.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::{
    ArtTable, BuildingArt, BuildingTypeArt, BuildingVariant, CityArtB2, CivArt, PrefetchGroup,
    VariantFormat, WallArt, WonderArt, WonderState,
};

use super::b2_models::{AnimSummary, BuiltModels, CsvRow, ModelJob, Options, build_models};
use crate::anim::idle::ModelKind;
use crate::db::{self, BuildingRow, CivArtRow, DbError};
use crate::game::GameSource;
use crate::models::ModelError;
use crate::models::merge::MergeScope;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreStats};
use crate::world::art_table::{load_table, md_id, store_table};

pub const SET_NAME: &str = "b2-city";

/// Name of the per-model table in the output directory.
pub const BUILDINGS_CSV: &str = "buildings.csv";

/// Fetched on demand, by the group table (AB-D10).
const GROUP: PrefetchGroup = PrefetchGroup(0);

/// `<Era name>` values of the city definitions (the second is spelled so in the game files).
pub const ERAS: [&str; 4] = ["Ancient", "Rennaisance", "Industrial", "Modern"];

/// `<Culture name>` of the XML to the culture key of the table (`ArtStylePrefix` spelling).
pub const CULTURES: [(&str, &str); 7] = [
    ("America", "AMERICAN"),
    ("Asia", "ASIAN"),
    ("Africa", "AFRICAN"),
    ("Europe", "EUROPEAN"),
    ("Mediterranean", "MEDITERRANEAN"),
    ("Polynesian", "POLYNESIAN"),
    ("Ruins", "RUINS"),
];

/// The file-name prefix of a culture's wall model (`euro_walls.fxsxml`) to the culture key.
pub const WALL_PREFIXES: [(&str, &str); 6] = [
    ("amer", "AMERICAN"),
    ("asia", "ASIAN"),
    ("afri", "AFRICAN"),
    ("euro", "EUROPEAN"),
    ("medit", "MEDITERRANEAN"),
    ("poly", "POLYNESIAN"),
];

/// The model of the Great Wall (`HB_Great_Wall.gr2`: `WALL1`, `WALL2`, `TOWER1`, `TOWER2` and their
/// `*_TREECUT`): the exe draws it along the border of the owner (`cvGreatWallSim`), no art define
/// names it. Built with its parts as `md:hb_great_wall`.
// re: the strings after `Wall_of_Babylon.gr2` in the .rodata of Civ5XP (cvGreatWallFilenames)
pub const GREAT_WALL_FLAT: &str = "hb_great_wall.fxsxml";

/// The art tag of the Great Wall's building, which has no wonder art.
pub const GREAT_WALL_TAG: &str = "ART_DEF_BUILDING_GREAT_WALL";

/// `ArtStyleTypes.Type` to the culture of the city catalog it indexes (the id of the style type is
/// the culture ordinal of the exe, RW-18). The barbarian style has none.
pub fn culture_of_style(style_type: &str) -> Option<&'static str> {
    match style_type {
        "ARTSTYLE_SOUTH_AMERICA" => Some("AMERICAN"),
        "ARTSTYLE_ASIAN" => Some("ASIAN"),
        "ARTSTYLE_MIDDLE_EAST" => Some("AFRICAN"),
        "ARTSTYLE_EUROPEAN" => Some("EUROPEAN"),
        "ARTSTYLE_GRECO_ROMAN" => Some("MEDITERRANEAN"),
        "ARTSTYLE_POLYNESIAN" => Some("POLYNESIAN"),
        _ => None,
    }
}

const CULTURE_WORDS: [&str; 6] = [
    "AFRICAN",
    "AMERICAN",
    "ASIAN",
    "EUROPEAN",
    "MEDITERRANEAN",
    "POLYNESIAN",
];

/// The numbers that follow a tag for a random pick (`STADIUM ONE`).
const RANDOM_WORDS: [&str; 5] = ["ONE", "TWO", "THREE", "FOUR", "FIVE"];

const ERA_WORDS: [&str; 7] = [
    "ANCIENT",
    "CLASSICAL",
    "MEDIEVAL",
    "RENAISSANCE",
    "INDUSTRIAL",
    "MODERN",
    "FUTURE",
];

// ---------------------------------------------------------------- the definitions

/// A `<Granny>` file name (`Euro_Anc_City.gr2`, `Assets/.../Colossus.fxsxml`, `Blythe.fxsxml`) as
/// the flat lower-case name of the `.fxsxml` that holds the model.
pub fn model_flat(granny: &str) -> String {
    let name = granny
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(granny)
        .trim()
        .to_ascii_lowercase();
    let stem = name
        .strip_suffix(".fxsxml")
        .or_else(|| name.strip_suffix(".gr2"))
        .unwrap_or(&name);
    format!("{stem}.fxsxml")
}

/// A `<Culture>/<Era>` entry of a city definition file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityDef {
    /// `<Culture name>` as written.
    pub culture: String,
    /// `<Era name>` as written.
    pub era: String,
    /// Flat `.fxsxml` of the catalog.
    pub flat: String,
    /// The `lighting` attribute of the `<Culture>`.
    pub lighting: Option<String>,
}

/// Parses a `<BuildingArtInfos>` file; another root gives nothing.
pub fn parse_city_defs(xml: &str) -> Result<Vec<CityDef>, String> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("city definitions: {e}"))?;
    if doc.root_element().tag_name().name() != "BuildingArtInfos" {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for culture in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("Culture"))
    {
        let c = culture
            .attribute("name")
            .ok_or("a <Culture> without a name")?;
        for era in culture.children().filter(|n| n.has_tag_name("Era")) {
            let e = era
                .attribute("name")
                .ok_or_else(|| format!("an <Era> of {c} without a name"))?;
            let gr2 = era
                .children()
                .find(|n| n.has_tag_name("PopulationBuildings"))
                .and_then(|p| p.children().find(|n| n.has_tag_name("Granny")))
                .and_then(|g| g.text())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| format!("{c}/{e} has no <Granny>"))?;
            out.push(CityDef {
                culture: c.to_owned(),
                era: e.to_owned(),
                flat: model_flat(gr2),
                lighting: culture.attribute("lighting").map(str::to_owned),
            });
        }
    }
    Ok(out)
}

/// One `WonderArtState`.
#[derive(Debug, Clone, PartialEq)]
pub struct WonderStateDef {
    pub state: String,
    pub scale: f32,
    pub flat: String,
}

/// A `WonderArtInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct WonderDef {
    pub art_type: String,
    pub states: Vec<WonderStateDef>,
}

fn child_text<'a>(n: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Parses a `<WonderArtInfos>` file; another root gives nothing.
pub fn parse_wonder_defs(xml: &str) -> Result<Vec<WonderDef>, String> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("wonder definitions: {e}"))?;
    if doc.root_element().tag_name().name() != "WonderArtInfos" {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for info in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("WonderArtInfo"))
    {
        let art_type = child_text(info, "Type").ok_or("a <WonderArtInfo> without a <Type>")?;
        let mut states = Vec::new();
        for st in info.children().filter(|n| n.has_tag_name("WonderArtState")) {
            let granny = child_text(st, "Granny")
                .ok_or_else(|| format!("{art_type}: a state without a <Granny>"))?;
            let scale = child_text(st, "fScale")
                .map(|s| {
                    s.parse::<f32>()
                        .map_err(|_| format!("{art_type}: fScale {s:?} is not a number"))
                })
                .transpose()?
                .unwrap_or(0.0);
            states.push(WonderStateDef {
                state: st.attribute("state").unwrap_or("Any").to_owned(),
                scale,
                flat: model_flat(granny),
            });
        }
        if states.is_empty() {
            return Err(format!("{art_type} has no <WonderArtState>"));
        }
        out.push(WonderDef {
            art_type: art_type.to_owned(),
            states,
        });
    }
    Ok(out)
}

/// A `WallArtInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct WallDef {
    pub art_type: String,
    pub scale: f32,
    pub flat: String,
}

/// Parses a `<WallArtInfos>` file; another root gives nothing.
pub fn parse_wall_defs(xml: &str) -> Result<Vec<WallDef>, String> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("wall definitions: {e}"))?;
    if doc.root_element().tag_name().name() != "WallArtInfos" {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for info in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("WallArtInfo"))
    {
        let art_type = child_text(info, "Type").ok_or("a <WallArtInfo> without a <Type>")?;
        let granny =
            child_text(info, "Granny").ok_or_else(|| format!("{art_type}: no <Granny>"))?;
        out.push(WallDef {
            art_type: art_type.to_owned(),
            scale: child_text(info, "fScale")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0),
            flat: model_flat(granny),
        });
    }
    Ok(out)
}

/// What the set reads from the install and the snapshot.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub cities: Vec<CityDef>,
    pub wonders: Vec<WonderDef>,
    pub walls: Vec<WallDef>,
    /// Flat names of the culture wall models: `(culture key, flat)`.
    pub culture_walls: Vec<(String, String)>,
    /// Flat names of the damage models of cities.
    pub damage: Vec<String>,
    /// The flat name of the model of the Great Wall, if the install has it (R5.11).
    pub great_wall: Option<String>,
    pub buildings: Vec<BuildingRow>,
    pub civs: Vec<CivArtRow>,
    /// Wonder types that two files define differently: `(type, files)`; the later wins.
    pub conflicts: Vec<(String, String)>,
}

/// Reads the definitions out of the install. `exists` says whether a flat `.fxsxml` is in it.
pub fn read_defs(game: &GameSource) -> Result<Input, ModelError> {
    let mut names: Vec<String> = game
        .iter_ext("xml")
        .map(|e| e.name.to_string())
        .filter(|n| n.starts_with("civ5artdefines_"))
        .collect();
    names.sort();
    let mut input = Input::default();
    let mut seen: BTreeMap<String, (WonderDef, String)> = BTreeMap::new();
    for name in &names {
        let is_city = name.contains("citybuildings");
        let is_wonder = name.contains("wonders");
        let is_wall = name.contains("_walls");
        if !(is_city || is_wonder || is_wall) {
            continue;
        }
        let bytes = game
            .read_by_name(name)?
            .ok_or_else(|| ModelError::invalid(format!("{name} is missing")))?;
        let text = String::from_utf8_lossy(&bytes);
        let ctx = |e: String| ModelError::invalid(format!("{name}: {e}"));
        input.cities.extend(parse_city_defs(&text).map_err(ctx)?);
        input.walls.extend(parse_wall_defs(&text).map_err(ctx)?);
        for w in parse_wonder_defs(&text).map_err(ctx)? {
            match seen.get(&w.art_type) {
                Some((old, from)) if *old != w => input
                    .conflicts
                    .push((w.art_type.clone(), format!("{from} then {name}"))),
                _ => {}
            }
            seen.insert(w.art_type.clone(), (w, name.clone()));
        }
    }
    input.wonders = seen.into_values().map(|(w, _)| w).collect();
    for (prefix, key) in WALL_PREFIXES {
        let flat = format!("{prefix}_walls.fxsxml");
        if game.lookup(&flat).is_some() {
            input.culture_walls.push((key.to_owned(), flat));
        }
    }
    let mut damage: Vec<String> = game
        .iter_ext("fxsxml")
        .map(|e| e.name.to_string())
        .filter(|n| n.starts_with("fxm_city_damage_state_"))
        .collect();
    damage.sort();
    input.damage = damage;
    input.great_wall = game
        .lookup(GREAT_WALL_FLAT)
        .is_some()
        .then(|| GREAT_WALL_FLAT.to_owned());
    Ok(input)
}

/// The snapshot rows the set reads.
pub fn read_rows(snapshot: &Path) -> Result<(Vec<BuildingRow>, Vec<CivArtRow>), DbError> {
    let d = db::open_snapshot(snapshot)?;
    Ok((db::all_buildings(&d)?, db::all_civ_art(&d)?))
}

// ---------------------------------------------------------------- the plan

/// How a model is built.
fn job(flat: &str, kind: ModelKind, scope: MergeScope) -> ModelJob {
    ModelJob {
        flat: flat.to_owned(),
        kind,
        scope,
        xml: None,
        diffuse: None,
    }
}

/// The models to build, each once. A catalog or a wall keeps its parts (the client places them); a
/// wonder merges whole (the retry with parts for a model whose skinned part has a placement is the
/// builder's); a damage model is an effect.
pub fn model_jobs(input: &Input) -> Vec<ModelJob> {
    let mut jobs: BTreeMap<String, ModelJob> = BTreeMap::new();
    let mut put = |j: ModelJob| {
        jobs.entry(j.flat.clone())
            .and_modify(|old| {
                if j.scope == MergeScope::Part {
                    old.scope = MergeScope::Part;
                }
            })
            .or_insert(j);
    };
    for c in &input.cities {
        put(job(&c.flat, ModelKind::Feature, MergeScope::Part));
    }
    for (_, flat) in &input.culture_walls {
        put(job(flat, ModelKind::Feature, MergeScope::Part));
    }
    for w in &input.walls {
        put(job(&w.flat, ModelKind::Feature, MergeScope::Part));
    }
    for w in &input.wonders {
        for s in &w.states {
            put(job(&s.flat, ModelKind::Feature, MergeScope::Model));
        }
    }
    for d in &input.damage {
        put(job(d, ModelKind::Effect, MergeScope::Model));
    }
    // The wall and the towers are models of their own in the gr2: the client takes them by name.
    if let Some(flat) = &input.great_wall {
        put(job(flat, ModelKind::Feature, MergeScope::Part));
    }
    jobs.into_values().collect()
}

/// The culture key of a `<Culture name>`.
pub fn culture_key(name: &str) -> Option<&'static str> {
    CULTURES
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, k)| *k)
}

/// `(word, kind)` of a wonder art type `ty` that varies the art tag `tag`: `AFRICAN CASTLE` is a
/// culture, `ANCIENT HARBOR` an era, `STADIUM ONE` a random pick.
pub fn variant_of(tag: &str, ty: &str) -> Option<(String, &'static str)> {
    if let Some(word) = ty.strip_suffix(tag).and_then(|w| w.strip_suffix(' ')) {
        let kind = if CULTURE_WORDS.contains(&word) {
            "culture"
        } else if ERA_WORDS.contains(&word) {
            "era"
        } else {
            return None;
        };
        return Some((word.to_owned(), kind));
    }
    if let Some(word) = ty.strip_prefix(tag).and_then(|w| w.strip_prefix(' '))
        && RANDOM_WORDS.contains(&word)
    {
        return Some((word.to_owned(), "random"));
    }
    None
}

/// The first state of a wonder art (the exe reads the first, RW-18).
fn first_state(w: &WonderDef) -> &WonderStateDef {
    &w.states[0]
}

/// One `building/<tag>` record, before the models are known to exist.
pub fn building_records(input: &Input) -> Vec<BuildingArt> {
    let wonders: BTreeMap<&str, &WonderDef> = input
        .wonders
        .iter()
        .map(|w| (w.art_type.as_str(), w))
        .collect();
    let mut by_tag: BTreeMap<&str, Vec<&BuildingRow>> = BTreeMap::new();
    for b in &input.buildings {
        if let Some(t) = &b.art_tag {
            by_tag.entry(t.as_str()).or_default().push(b);
        }
    }
    let wall_models: BTreeMap<&str, &WallDef> = input
        .walls
        .iter()
        .map(|w| (w.art_type.as_str(), w))
        .collect();
    by_tag
        .into_iter()
        .map(|(tag, rows)| {
            let mut types: Vec<String> = rows.iter().map(|r| r.building_type.clone()).collect();
            types.sort();
            let own = wonders.get(tag);
            let mut variants: Vec<BuildingVariant> = input
                .wonders
                .iter()
                .filter_map(|w| {
                    let (word, kind) = variant_of(tag, &w.art_type)?;
                    let s = first_state(w);
                    Some(BuildingVariant {
                        kind: kind.to_owned(),
                        key: word,
                        art_type: w.art_type.clone(),
                        model: md_id(&s.flat),
                        scale: s.scale,
                    })
                })
                .collect();
            variants.sort_by(|a, b| (&a.kind, &a.key).cmp(&(&b.kind, &b.key)));
            let wall = wall_models.get(tag);
            let (model, scale) = match (own, wall) {
                (Some(w), _) => (Some(md_id(&first_state(w).flat)), first_state(w).scale),
                (None, Some(w)) => (Some(md_id(&w.flat)), w.scale),
                _ => (None, 1.0),
            };
            let is_city_wall = rows.iter().any(|r| r.city_wall);
            let note = if model.is_some() || !variants.is_empty() {
                None
            } else if is_city_wall {
                Some(
                    "a city wall: the wall of the owner's culture, `wall/<culture>/<era>` (RW-18)"
                        .to_owned(),
                )
            } else {
                Some(
                    "no art of its own: drawn as part of the city catalog (RW-18), no wonder art has this tag"
                        .to_owned(),
                )
            };
            BuildingArt {
                art_tag: tag.to_owned(),
                building_types: types,
                cultural_variation: rows.iter().any(|r| r.cultural_variation),
                era_variation: rows.iter().any(|r| r.era_variation),
                random_variation: rows.iter().any(|r| r.random_variation),
                model,
                scale,
                variants,
                note,
            }
        })
        .collect()
}

/// One `building_type/<Type>` record per building with an art tag: its `DisplayPosition`, which
/// says whether the client draws the model of the tag for it and how it is placed (re-b17 5.1).
pub fn building_type_records(input: &Input) -> Vec<BuildingTypeArt> {
    input
        .buildings
        .iter()
        .filter_map(|b| {
            Some(BuildingTypeArt {
                building_type: b.building_type.clone(),
                art_tag: b.art_tag.clone()?,
                display_position: b.display_position,
            })
        })
        .collect()
}

/// The `civ/` records.
pub fn civ_records(civs: &[CivArtRow]) -> Vec<CivArt> {
    civs.iter()
        .map(|c| {
            let style = c
                .art_style_type
                .as_deref()
                .and_then(culture_of_style)
                .filter(|s| Some(*s) != c.art_style_prefix.as_deref());
            CivArt {
                civ: c.civ_type.clone(),
                art_define_tag: c.art_define_tag.clone(),
                art_style: c.art_style_type.clone(),
                prefix: c.art_style_prefix.clone(),
                suffix: c.art_style_suffix.clone(),
                default_color: c.default_player_color.clone(),
                style_culture: style.map(str::to_owned),
            }
        })
        .collect()
}

/// The records of the set, after the models are built. `parts` is the number of parts of every
/// model that was built; a record whose model is not in it is left out and named in the result (the
/// set fails after it has written its reports).
pub fn apply(input: &Input, parts: &BTreeMap<String, usize>, table: &mut ArtTable) -> Vec<String> {
    let mut missing = Vec::new();
    fn part_count(
        parts: &BTreeMap<String, usize>,
        missing: &mut Vec<String>,
        flat: &str,
        what: &str,
    ) -> Option<u16> {
        match parts.get(flat) {
            Some(n) => Some(u16::try_from(*n).unwrap_or(u16::MAX)),
            None => {
                missing.push(format!("{what}: {flat} was not built"));
                None
            }
        }
    }
    for c in &input.cities {
        let (Some(key), era) = (culture_key(&c.culture), c.era.as_str()) else {
            missing.push(format!(
                "city {}/{}: the culture is not one of the known ones",
                c.culture, c.era
            ));
            continue;
        };
        if let Some(n) = part_count(parts, &mut missing, &c.flat, &format!("city {key}/{era}")) {
            table.insert_city_b2(
                key,
                era,
                CityArtB2 {
                    model: md_id(&c.flat),
                    parts: n,
                    lighting: c.lighting.clone(),
                },
            );
        }
    }
    for (key, flat) in &input.culture_walls {
        if let Some(n) = part_count(parts, &mut missing, flat, &format!("wall {key}")) {
            for era in ERAS {
                table.insert_wall(
                    key,
                    era,
                    WallArt {
                        model: md_id(flat),
                        parts: n,
                    },
                );
            }
        }
    }
    for w in &input.walls {
        if let Some(n) = part_count(
            parts,
            &mut missing,
            &w.flat,
            &format!("wall {}", w.art_type),
        ) {
            table.insert_wall(
                &w.art_type,
                "Any",
                WallArt {
                    model: md_id(&w.flat),
                    parts: n,
                },
            );
        }
    }
    for w in &input.wonders {
        let states: Vec<WonderState> = w
            .states
            .iter()
            .filter(|s| parts.contains_key(&s.flat))
            .map(|s| WonderState {
                state: s.state.clone(),
                scale: s.scale,
                model: md_id(&s.flat),
            })
            .collect();
        for s in &w.states {
            if !parts.contains_key(&s.flat) {
                missing.push(format!("wonder {}: {} was not built", w.art_type, s.flat));
            }
        }
        if states.len() == w.states.len() {
            table.insert_wonder(WonderArt {
                art_type: w.art_type.clone(),
                states,
            });
        }
    }
    for mut b in building_records(input) {
        // A model that was not built is a failure listed above; the record is not written.
        let built = |id: &asset_format::LogicalId| parts.keys().any(|f| md_id(f) == *id);
        if b.model.as_ref().is_some_and(|m| !built(m))
            || b.variants.iter().any(|v| !built(&v.model))
        {
            continue;
        }
        if b.model.is_none() && b.variants.is_empty() && b.note.is_none() {
            b.note = Some("no model".to_owned());
        }
        table.insert_building(b);
    }
    for b in building_type_records(input) {
        table.insert_building_type(b);
    }
    for c in civ_records(&input.civs) {
        table.insert_civ(c);
    }
    missing
}

// ---------------------------------------------------------------- buildings.csv

pub const CSV_HEADER: &str =
    "kind,key,art,model,parts,meshes,triangles,materials,textures,clips,anim_bytes,status";

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_owned()
    }
}

/// One line of `buildings.csv`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildingCsvRow {
    /// `city`, `wall`, `wonder`, `building`, `damage` or `great-wall`.
    pub kind: &'static str,
    pub key: String,
    /// The art tag or type that names the model.
    pub art: String,
    pub model: String,
    pub parts: usize,
    pub meshes: usize,
    pub triangles: usize,
    pub materials: usize,
    pub textures: usize,
    pub clips: usize,
    pub anim_bytes: u64,
    pub status: String,
}

pub fn csv_text(rows: &[BuildingCsvRow]) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for r in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}\n",
            r.kind,
            csv_field(&r.key),
            csv_field(&r.art),
            csv_field(&r.model),
            r.parts,
            r.meshes,
            r.triangles,
            r.materials,
            r.textures,
            r.clips,
            r.anim_bytes,
            csv_field(&r.status),
        ));
    }
    out
}

/// What was built for a flat name: the csv row of the model builder and the part count.
struct ModelFacts<'a> {
    rows: &'a BTreeMap<String, CsvRow>,
    parts: &'a BTreeMap<String, usize>,
}

impl ModelFacts<'_> {
    /// A row of the csv for the model `flat` (`status` is `ok` or why not).
    fn row(&self, kind: &'static str, key: &str, art: &str, flat: &str) -> BuildingCsvRow {
        let mut r = BuildingCsvRow {
            kind,
            key: key.to_owned(),
            art: art.to_owned(),
            model: flat.to_owned(),
            ..BuildingCsvRow::default()
        };
        match self.rows.get(flat) {
            None => r.status = format!("FAILED: {flat} was not built"),
            Some(b) if b.status != "ok" => r.status = b.status.clone(),
            Some(b) if b.meshes == 0 => r.status = format!("FAILED: {flat} has no mesh"),
            Some(b) => {
                r.parts = self.parts.get(flat).copied().unwrap_or(0);
                r.meshes = b.meshes;
                r.triangles = b.triangles;
                r.materials = b.materials;
                r.textures = b.textures;
                r.clips = b.clips;
                r.anim_bytes = b.anim_bytes;
                r.status = "ok".to_owned();
            }
        }
        r
    }
}

/// The lines of `buildings.csv` in a fixed order: cities, walls, wonders, buildings, damage models.
fn csv_rows(
    input: &Input,
    rows: &BTreeMap<String, CsvRow>,
    parts: &BTreeMap<String, usize>,
) -> Vec<BuildingCsvRow> {
    let facts = ModelFacts { rows, parts };
    let mut out = Vec::new();
    for c in &input.cities {
        let key = culture_key(&c.culture).unwrap_or("?");
        out.push(facts.row("city", &format!("{key}/{}", c.era), &c.culture, &c.flat));
    }
    for (key, flat) in &input.culture_walls {
        out.push(facts.row("wall", key, flat, flat));
    }
    for w in &input.walls {
        out.push(facts.row("wall", &w.art_type, &w.art_type, &w.flat));
    }
    for w in &input.wonders {
        for s in &w.states {
            out.push(facts.row("wonder", &w.art_type, &w.art_type, &s.flat));
        }
    }
    let recs: BTreeMap<String, BuildingArt> = building_records(input)
        .into_iter()
        .map(|b| (b.art_tag.clone(), b))
        .collect();
    let flat_of =
        |id: &asset_format::LogicalId| format!("{}.fxsxml", id.as_str().trim_start_matches("md:"));
    for b in &input.buildings {
        let Some(tag) = &b.art_tag else {
            out.push(BuildingCsvRow {
                kind: "building",
                key: b.building_type.clone(),
                status: "ok: the building has no ArtDefineTag (no art)".to_owned(),
                ..BuildingCsvRow::default()
            });
            continue;
        };
        let rec = &recs[tag];
        let mut r = match &rec.model {
            Some(m) => facts.row("building", &b.building_type, tag, &flat_of(m)),
            None => BuildingCsvRow {
                kind: "building",
                key: b.building_type.clone(),
                art: tag.clone(),
                status: format!(
                    "ok: {}",
                    rec.note.as_deref().unwrap_or("art by variants only")
                ),
                ..BuildingCsvRow::default()
            },
        };
        r.key = b.building_type.clone();
        for v in &rec.variants {
            let vr = facts.row("building", &b.building_type, tag, &flat_of(&v.model));
            if vr.status != "ok" {
                r.status = vr.status;
            }
        }
        if rec.model.is_none() && !rec.variants.is_empty() && r.status.starts_with("ok:") {
            r.status = format!(
                "ok: {} variant(s) ({} / {})",
                rec.variants.len(),
                tag,
                rec.variants[0].kind
            );
        }
        out.push(r);
    }
    for d in &input.damage {
        out.push(facts.row("damage", d, d, d));
    }
    if let Some(flat) = &input.great_wall {
        out.push(facts.row("great-wall", "BUILDING_GREAT_WALL", GREAT_WALL_TAG, flat));
    }
    out
}

// ---------------------------------------------------------------- the layout system

/// The file stems of the density fields: `DF_flat` and `DF_<digits of the set bits>` of the six base
/// patterns `ms_amCityHexPatterns` (0x0920c378) of the original.
pub const LAYOUT_STEMS: [&str; 7] = [
    "df_flat", "df_0", "df_1", "df_05", "df_15", "df_045", "df_135",
];

/// The tensor fields of the layout (small city up to population 6, large above).
pub const LAYOUT_TENSORS: [&str; 2] = ["tf_small_city.dds", "tf_large_city.dds"];

/// The files of `assets\LayoutSystem` the client lays the cities out with: the ten population
/// fields of each pattern and the two tensor fields.
pub fn layout_files() -> Vec<String> {
    let mut out: Vec<String> = LAYOUT_STEMS
        .iter()
        .flat_map(|s| (1..=10).map(move |p| format!("{s}_a{p}.dds")))
        .collect();
    out.extend(LAYOUT_TENSORS.iter().map(|t| (*t).to_owned()));
    out
}

/// Adds the fields of the layout system as `tx:<file>` textures: level 0 of the file, its bytes
/// (A8R8G8B8 as `Rgba8`), fetched on demand. The client reads them on the CPU (WOB5). Returns how
/// many; a file the install lacks is an error.
fn add_layout_fields(
    game: &GameSource,
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    bytes: &mut BTreeMap<VariantFormat, u64>,
) -> Result<usize, ModelError> {
    let files = layout_files();
    for f in &files {
        let raw = game
            .read_by_name(f)?
            .ok_or_else(|| ModelError::invalid(format!("{f} is missing from the install")))?;
        let blob = crate::textures::level0_only(crate::textures::dds_passthrough(&raw).map_err(
            |source| ModelError::Texture {
                name: f.clone(),
                source,
            },
        )?);
        let format = crate::textures::variant_format(blob.format);
        let (hash, size) = store.put(&blob.encode())?;
        *bytes.entry(format).or_default() += size;
        manifest.add(
            super::tx_id(f),
            asset_format::Variant {
                format,
                hash,
                size,
                prefetch: GROUP,
            },
        );
    }
    Ok(files.len())
}
// ---------------------------------------------------------------- the build

#[derive(Debug)]
pub struct CitySummary {
    pub cities: usize,
    pub walls: usize,
    pub wonders: usize,
    pub buildings: usize,
    pub buildings_with_model: usize,
    pub buildings_with_variants: usize,
    pub buildings_without_art: usize,
    pub civs: usize,
    pub damage_models: usize,
    pub models: usize,
    pub failed_models: usize,
    /// Records left out because their model was not built.
    pub missing: Vec<String>,
    pub meshes: u64,
    pub meshes_before_merge: u64,
    pub triangles: u64,
    pub materials: u64,
    pub textures: usize,
    pub substitutes: Vec<String>,
    pub model_bytes: u64,
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    /// The fields of the layout system (`tx:df_*`, `tx:tf_*`).
    pub layout_fields: usize,
    pub table_bytes: u64,
    pub anims: AnimSummary,
    pub report: SetReport,
    pub stats: StoreStats,
    pub seconds: f64,
}

impl CitySummary {
    pub fn failures(&self) -> usize {
        self.failed_models + self.missing.len()
    }

    pub fn total_bytes(&self) -> u64 {
        self.model_bytes
            + self.anims.bytes
            + self.anims.fsm.bytes
            + self.texture_bytes.values().sum::<u64>()
            + self.table_bytes
    }
}

/// Builds the set into `out`: the models, textures and clips, the `city/`, `wall/`, `wonder/`,
/// `building/` and `civ/` records of `tb:art-b2` and `buildings.csv`.
pub fn build_city(
    game: &GameSource,
    snapshot: &Path,
    out: &Path,
    opts: &Options,
) -> Result<CitySummary, ModelError> {
    let started = Instant::now();
    let mut input = read_defs(game)?;
    let (buildings, civs) = read_rows(snapshot)
        .map_err(|e| ModelError::invalid(format!("reading {}: {e}", snapshot.display())))?;
    input.buildings = buildings;
    input.civs = civs;

    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let jobs = model_jobs(&input);
    let BuiltModels {
        good,
        failed,
        rows: model_rows,
        mut entries,
        texture_files,
        stub_ids,
        texture_bytes,
        model_bytes,
        meshes,
        meshes_before_merge,
        triangles,
        materials,
        anims,
        ..
    } = build_models(game, &jobs, &store, &mut manifest, opts, GROUP)?;

    let parts: BTreeMap<String, usize> = good
        .iter()
        .map(|(l, b)| (l.flat.clone(), b.model.parts.len()))
        .collect();
    let mut table = load_table(&store, &manifest)?;
    let missing = apply(&input, &parts, &mut table);
    let table_bytes = store_table(&store, &mut manifest, &table)?;
    let mut texture_bytes = texture_bytes;
    let layout_fields = add_layout_fields(game, &store, &mut manifest, &mut texture_bytes)?;
    manifest.finish(&store)?;

    let csv = csv_rows(&input, &model_rows, &parts);
    std::fs::write(out.join(BUILDINGS_CSV), csv_text(&csv)).map_err(|e| {
        ModelError::invalid(format!(
            "writing {}: {e}",
            out.join(BUILDINGS_CSV).display()
        ))
    })?;

    // The report.
    let recs = building_records(&input);
    for r in csv.iter().filter(|r| r.status.starts_with("FAILED")) {
        entries.push(ReportEntry::new(
            "building-failed",
            format!("{} {}", r.kind, r.key),
            r.status.clone(),
        ));
    }
    for m in &missing {
        entries.push(ReportEntry::new("record-left-out", SET_NAME, m.clone()));
    }
    for b in recs.iter().filter(|b| b.note.is_some()) {
        entries.push(ReportEntry::new(
            "building-no-art",
            b.art_tag.clone(),
            format!(
                "{}: {} building type(s)",
                b.note.as_deref().unwrap_or(""),
                b.building_types.len()
            ),
        ));
    }
    for t in input.buildings.iter().filter(|b| b.art_tag.is_none()) {
        entries.push(ReportEntry::new(
            "building-no-art",
            t.building_type.clone(),
            "no ArtDefineTag",
        ));
    }
    let used: BTreeSet<&str> = recs
        .iter()
        .flat_map(|b| {
            b.model
                .iter()
                .map(|m| m.as_str())
                .chain(b.variants.iter().map(|v| v.model.as_str()))
        })
        .collect();
    for w in &input.wonders {
        if !used.contains(md_id(&w.states[0].flat).as_str()) {
            entries.push(ReportEntry::new(
                "wonder-unused",
                w.art_type.clone(),
                "no building type has this art type (or a variant of it) as its tag",
            ));
        }
    }
    for (ty, files) in &input.conflicts {
        entries.push(ReportEntry::new(
            "wonder-redefined",
            ty.clone(),
            format!("defined differently in {files}; the later file wins"),
        ));
    }
    for c in &input.cities {
        if culture_key(&c.culture).is_none() {
            entries.push(ReportEntry::new(
                "city-culture-unknown",
                c.culture.clone(),
                "not a culture of the table",
            ));
        }
    }
    let with_model = recs.iter().filter(|b| b.model.is_some()).count();
    let with_variants = recs
        .iter()
        .filter(|b| b.model.is_none() && !b.variants.is_empty())
        .count();
    let without_art = recs
        .iter()
        .filter(|b| b.model.is_none() && b.variants.is_empty())
        .count();
    let mut report = SetReport {
        entries,
        ..SetReport::default()
    };
    for (k, v) in [
        ("city records", input.cities.len()),
        (
            "wall records",
            input.culture_walls.len() * ERAS.len() + input.walls.len(),
        ),
        ("wonder records", input.wonders.len()),
        ("building art records", recs.len()),
        ("building art records with a model", with_model),
        ("building art records with variants only", with_variants),
        ("building art records without art", without_art),
        ("building types", input.buildings.len()),
        ("civ records", input.civs.len()),
        ("damage models", input.damage.len()),
        ("models", good.len()),
        ("failed models", failed.len()),
        ("meshes", meshes as usize),
        ("meshes before merge", meshes_before_merge as usize),
        ("triangles", triangles as usize),
        ("materials", materials as usize),
        ("textures", texture_files.len()),
        ("stub textures", stub_ids.len()),
        ("clip packs", anims.packs),
        ("clips", anims.clips),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    report.bytes.insert("models".to_owned(), model_bytes);
    for (f, b) in &texture_bytes {
        report.bytes.insert(format!("textures {f:?}"), *b);
    }
    report.bytes.insert("table".to_owned(), table_bytes);
    report.bytes.insert("clip packs".to_owned(), anims.bytes);
    Ok(CitySummary {
        cities: input.cities.len(),
        walls: input.culture_walls.len() * ERAS.len() + input.walls.len(),
        wonders: input.wonders.len(),
        buildings: recs.len(),
        buildings_with_model: with_model,
        buildings_with_variants: with_variants,
        buildings_without_art: without_art,
        civs: input.civs.len(),
        damage_models: input.damage.len(),
        models: good.len(),
        failed_models: failed.len(),
        missing,
        meshes,
        meshes_before_merge,
        triangles,
        materials,
        textures: texture_files.len(),
        substitutes: stub_ids,
        model_bytes,
        texture_bytes,
        layout_fields,
        table_bytes,
        anims,
        report,
        stats: store.stats(),
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wonder(ty: &str, flat: &str, scale: f32) -> WonderDef {
        WonderDef {
            art_type: ty.into(),
            states: vec![WonderStateDef {
                state: "Any".into(),
                scale,
                flat: flat.into(),
            }],
        }
    }

    fn bld(ty: &str, tag: Option<&str>) -> BuildingRow {
        BuildingRow {
            building_type: ty.into(),
            building_class: None,
            art_tag: tag.map(str::to_owned),
            cultural_variation: false,
            era_variation: false,
            random_variation: false,
            city_wall: false,
            prereq_tech: None,
            era: None,
            display_position: 0,
        }
    }

    #[test]
    fn granny_names_become_flat_fxsxml_names() {
        assert_eq!(model_flat("Euro_Anc_City.gr2"), "euro_anc_city.fxsxml");
        assert_eq!(
            model_flat("Assets/Buildings/Wonders/Colossus/Colossus.fxsxml"),
            "colossus.fxsxml"
        );
        assert_eq!(model_flat("Blythe.fxsxml"), "blythe.fxsxml");
        assert_eq!(model_flat("Wall_of_Babylon.gr2"), "wall_of_babylon.fxsxml");
    }

    const CITIES: &str = r#"<BuildingArtInfos>
  <Culture name="Polynesian" lighting="Asia">
    <Era name="Ancient"><PopulationBuildings><Granny>Poly_Anc_City.gr2</Granny></PopulationBuildings></Era>
    <Era name="Modern"><PopulationBuildings><Granny>Poly_Mod_City.gr2</Granny></PopulationBuildings></Era>
  </Culture>
  <Culture name="Ruins">
    <Era name="Ancient"><PopulationBuildings><Granny>City_Ruins.gr2</Granny></PopulationBuildings></Era>
  </Culture>
</BuildingArtInfos>"#;

    #[test]
    fn city_definitions_keep_the_lighting_and_other_roots_give_nothing() {
        let d = parse_city_defs(CITIES).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0].lighting.as_deref(), Some("Asia"));
        assert_eq!(d[1].flat, "poly_mod_city.fxsxml");
        assert_eq!(d[2].lighting, None);
        assert!(parse_city_defs("<WonderArtInfos/>").unwrap().is_empty());
        assert!(
            parse_city_defs("<BuildingArtInfos><Culture name=\"X\"><Era name=\"A\"/></Culture></BuildingArtInfos>")
                .is_err()
        );
    }

    #[test]
    fn wonder_and_wall_definitions_parse() {
        let w = parse_wonder_defs(
            r#"<WonderArtInfos><WonderArtInfo><Type>STONEHENGE</Type>
            <WonderArtState state="Any"><fScale>0.05</fScale><Granny>Assets/B/Stonehenge.fxsxml</Granny></WonderArtState>
            </WonderArtInfo></WonderArtInfos>"#,
        )
        .unwrap();
        assert_eq!(w[0].art_type, "STONEHENGE");
        assert_eq!(w[0].states[0].flat, "stonehenge.fxsxml");
        assert_eq!(w[0].states[0].scale, 0.05);
        assert!(
            parse_wonder_defs(
                "<WonderArtInfos><WonderArtInfo><Type>X</Type></WonderArtInfo></WonderArtInfos>"
            )
            .is_err()
        );
        let wl = parse_wall_defs(
            "<WallArtInfos><WallArtInfo><Type>W</Type><fScale>0.05</fScale><Granny>Wall_of_Babylon.gr2</Granny></WallArtInfo></WallArtInfos>",
        )
        .unwrap();
        assert_eq!(wl[0].flat, "wall_of_babylon.fxsxml");
        assert!(parse_wall_defs("<WonderArtInfos/>").unwrap().is_empty());
    }

    #[test]
    fn variants_are_a_culture_an_era_or_a_random_pick() {
        assert_eq!(
            variant_of("CASTLE", "AFRICAN CASTLE"),
            Some(("AFRICAN".into(), "culture"))
        );
        assert_eq!(
            variant_of("HARBOR", "ANCIENT HARBOR"),
            Some(("ANCIENT".into(), "era"))
        );
        assert_eq!(
            variant_of("STADIUM", "STADIUM ONE"),
            Some(("ONE".into(), "random"))
        );
        assert_eq!(variant_of("CASTLE", "CASTLE"), None);
        // Another tag that merely ends with the same letters is not a variant.
        assert_eq!(variant_of("CASTLE", "NEUSCHWANSTEIN CASTLE"), None);
        assert_eq!(variant_of("PALACE", "THE PALACE OF X"), None);
    }

    #[test]
    fn a_tag_has_its_own_model_its_variants_or_says_why_not() {
        let input = Input {
            wonders: vec![
                wonder("STONEHENGE", "stonehenge.fxsxml", 0.05),
                wonder("AFRICAN CASTLE", "african_castle.fxsxml", 0.2),
                wonder("EUROPEAN CASTLE", "euro_castle.fxsxml", 0.2),
                wonder("STADIUM ONE", "stadium1.fxsxml", 0.2),
            ],
            buildings: vec![
                bld("BUILDING_STONEHENGE", Some("STONEHENGE")),
                bld("BUILDING_CASTLE", Some("CASTLE")),
                bld("BUILDING_STADIUM", Some("STADIUM")),
                bld("BUILDING_LIBRARY", Some("ART_DEF_BUILDING_LIBRARY")),
                bld("BUILDING_PANAMA", None),
            ],
            ..Input::default()
        };
        let recs = building_records(&input);
        // One record per tag, the building without a tag has none.
        assert_eq!(recs.len(), 4);
        let by = |t: &str| recs.iter().find(|r| r.art_tag == t).unwrap();
        assert_eq!(by("STONEHENGE").model, Some(md_id("stonehenge.fxsxml")));
        assert_eq!(by("STONEHENGE").scale, 0.05);
        assert_eq!(by("STONEHENGE").note, None);
        let castle = by("CASTLE");
        assert_eq!(castle.model, None);
        assert_eq!(castle.variants.len(), 2);
        assert_eq!(castle.variants[0].key, "AFRICAN");
        assert_eq!(castle.note, None);
        assert_eq!(by("STADIUM").variants[0].kind, "random");
        let lib = by("ART_DEF_BUILDING_LIBRARY");
        assert!(lib.model.is_none() && lib.variants.is_empty());
        assert!(lib.note.as_deref().unwrap().contains("city catalog"));
    }

    #[test]
    fn a_city_wall_building_says_it_uses_the_culture_wall() {
        let mut w = bld("BUILDING_WALLS", Some("ART_DEF_BUILDING_WALLS"));
        w.city_wall = true;
        let input = Input {
            buildings: vec![w],
            ..Input::default()
        };
        let r = building_records(&input);
        assert!(r[0].note.as_deref().unwrap().contains("wall/<culture>"));
    }

    #[test]
    fn each_building_with_a_tag_carries_its_own_display_position() {
        let mut colosseum = bld("BUILDING_COLOSSEUM", Some("COLESSEUM"));
        colosseum.display_position = 2;
        let input = Input {
            buildings: vec![
                colosseum,
                bld("BUILDING_AMPHITHEATER", Some("COLESSEUM")),
                bld("BUILDING_NONE", None),
            ],
            ..Input::default()
        };
        let r = building_type_records(&input);
        assert_eq!(r.len(), 2, "the building without a tag has none");
        let by = |t: &str| r.iter().find(|b| b.building_type == t).unwrap();
        assert_eq!(by("BUILDING_COLOSSEUM").display_position, 2);
        // The same tag, but no model of its own in the city.
        assert_eq!(by("BUILDING_AMPHITHEATER").display_position, 0);
        assert_eq!(by("BUILDING_AMPHITHEATER").art_tag, "COLESSEUM");
    }

    #[test]
    fn civs_name_the_culture_of_a_style_that_differs_from_the_prefix() {
        let civ = |t: &str, style: &str, prefix: Option<&str>| CivArtRow {
            civ_type: t.into(),
            art_define_tag: None,
            art_style_type: Some(style.into()),
            art_style_prefix: prefix.map(str::to_owned),
            art_style_suffix: Some("_MED".into()),
            default_player_color: None,
        };
        let r = civ_records(&[
            civ(
                "CIVILIZATION_POLYNESIA",
                "ARTSTYLE_POLYNESIAN",
                Some("AMERICAN"),
            ),
            civ("CIVILIZATION_ROME", "ARTSTYLE_EUROPEAN", Some("EUROPEAN")),
            civ("CIVILIZATION_BARBARIAN", "ARTSTYLE_BARBARIAN", None),
        ]);
        assert_eq!(r[0].style_culture.as_deref(), Some("POLYNESIAN"));
        assert_eq!(r[1].style_culture, None);
        assert_eq!(r[2].style_culture, None);
        assert_eq!(r[0].prefix.as_deref(), Some("AMERICAN"));
    }

    #[test]
    fn every_model_is_built_once_and_a_catalog_keeps_its_parts() {
        let input = Input {
            cities: parse_city_defs(CITIES).unwrap(),
            wonders: vec![wonder("A", "a.fxsxml", 1.0), wonder("B", "a.fxsxml", 1.0)],
            culture_walls: vec![("ASIAN".into(), "asia_walls.fxsxml".into())],
            damage: vec!["fxm_city_damage_state_first_01.fxsxml".into()],
            ..Input::default()
        };
        let jobs = model_jobs(&input);
        let scope = |f: &str| jobs.iter().find(|j| j.flat == f).unwrap().scope;
        let kind = |f: &str| jobs.iter().find(|j| j.flat == f).unwrap().kind;
        assert_eq!(jobs.len(), 6);
        assert_eq!(scope("poly_anc_city.fxsxml"), MergeScope::Part);
        assert_eq!(scope("asia_walls.fxsxml"), MergeScope::Part);
        assert_eq!(scope("a.fxsxml"), MergeScope::Model);
        assert_eq!(
            kind("fxm_city_damage_state_first_01.fxsxml"),
            ModelKind::Effect
        );
        assert_eq!(kind("poly_anc_city.fxsxml"), ModelKind::Feature);
    }

    #[test]
    fn the_great_wall_is_built_with_its_parts_and_has_a_line() {
        let input = Input {
            great_wall: Some(GREAT_WALL_FLAT.into()),
            ..Input::default()
        };
        let jobs = model_jobs(&input);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].flat, "hb_great_wall.fxsxml");
        assert_eq!(jobs[0].scope, MergeScope::Part);
        assert_eq!(md_id(GREAT_WALL_FLAT).as_str(), "md:hb_great_wall");
        let rows = csv_rows(&input, &BTreeMap::new(), &BTreeMap::new());
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].kind, rows[0].art.as_str()),
            ("great-wall", GREAT_WALL_TAG)
        );
        // Without the model in the install nothing is asked for.
        assert!(model_jobs(&Input::default()).is_empty());
    }

    #[test]
    fn records_are_written_for_built_models_and_the_rest_is_named() {
        let input = Input {
            cities: parse_city_defs(CITIES).unwrap(),
            culture_walls: vec![("ASIAN".into(), "asia_walls.fxsxml".into())],
            wonders: vec![wonder("STONEHENGE", "stonehenge.fxsxml", 0.05)],
            buildings: vec![bld("BUILDING_STONEHENGE", Some("STONEHENGE"))],
            ..Input::default()
        };
        let mut parts = BTreeMap::new();
        for f in [
            "poly_anc_city.fxsxml",
            "city_ruins.fxsxml",
            "asia_walls.fxsxml",
            "stonehenge.fxsxml",
        ] {
            parts.insert(f.to_owned(), 3);
        }
        let mut t = ArtTable::new();
        let missing = apply(&input, &parts, &mut t);
        // `poly_mod_city` was not built: one record is left out and named.
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("poly_mod_city"));
        let c = t.city_b2("POLYNESIAN", "Ancient").unwrap();
        assert_eq!(c.parts, 3);
        assert_eq!(c.lighting.as_deref(), Some("Asia"));
        assert!(t.city_b2("POLYNESIAN", "Modern").is_none());
        assert!(t.city_b2("RUINS", "Ancient").is_some());
        // The wall holds for every era of its culture.
        for era in ERAS {
            assert!(t.wall("ASIAN", era).is_some());
        }
        assert_eq!(t.wonder("STONEHENGE").unwrap().states[0].scale, 0.05);
        assert!(t.building("STONEHENGE").is_some());
    }

    #[test]
    fn the_csv_has_a_line_per_thing_and_says_why_a_building_has_no_model() {
        let input = Input {
            wonders: vec![wonder("STONEHENGE", "stonehenge.fxsxml", 0.05)],
            buildings: vec![
                bld("BUILDING_STONEHENGE", Some("STONEHENGE")),
                bld("BUILDING_LIBRARY", Some("ART_DEF_BUILDING_LIBRARY")),
                bld("BUILDING_PANAMA", None),
            ],
            ..Input::default()
        };
        let mut rows = BTreeMap::new();
        rows.insert(
            "stonehenge.fxsxml".to_owned(),
            CsvRow {
                kind: "model",
                id: String::new(),
                art_info: String::new(),
                model: "stonehenge.fxsxml".into(),
                bones: 1,
                meshes: 2,
                triangles: 30,
                materials: 1,
                damage: [0; 4],
                textures: 2,
                clips: 0,
                anim_bytes: 0,
                status: "ok".into(),
            },
        );
        let mut parts = BTreeMap::new();
        parts.insert("stonehenge.fxsxml".to_owned(), 1);
        let csv = csv_rows(&input, &rows, &parts);
        // The wonder, the three building types.
        assert_eq!(csv.len(), 4);
        assert_eq!(csv[0].status, "ok");
        assert_eq!(csv[1].model, "stonehenge.fxsxml");
        assert_eq!(csv[1].meshes, 2);
        assert!(
            csv[2].status.starts_with("ok: no art of its own"),
            "{:?}",
            csv[2]
        );
        assert!(csv[3].status.contains("no ArtDefineTag"));
        // A model that was not built is a failure.
        let csv = csv_rows(&input, &BTreeMap::new(), &BTreeMap::new());
        assert!(csv[0].status.starts_with("FAILED"));
        let text = csv_text(&csv);
        assert_eq!(text.lines().next(), Some(CSV_HEADER));
        assert_eq!(CSV_HEADER.split(',').count(), 12);
    }
}
