//! Set `b1-cities`: one model per city culture and era bucket, the ruins,
//! their textures, and the art table records `city/<culture>/<era_bucket>`.
//!
//! # Table keys
//! `culture` is the `Civilizations.ArtStylePrefix` value (`AMERICAN`, `ASIAN`, `AFRICAN`,
//! `EUROPEAN`, `MEDITERRANEAN`; the snapshot has no other), so the client looks a city up with the
//! owner's prefix as it is. `RUINS` holds the ruins. `era_bucket` is the name of the `<Era>`
//! element of `Civ5ArtDefines_CityBuildings.xml`: `Ancient`, and `Rennaisance` (sic) for the
//! Renaissance. The ruins have the same model in both buckets (as in the XML). Later eras are not
//! built (B1 needs none).
//!
//! # The catalog as it is
//! A city gr2 (`Euro_Anc_City.gr2`) is a catalogue, not a city: 20 models laid out in a row about
//! 3400 units wide, each a flat base quad (material 0, `CityBase_*`) plus one building mesh
//! (material 1). The B1 client composed one compact city out of it; that composition
//! (`compose_city`) is gone with the B2 city records (`b2-city`, SIMP-021
//! narrowed to "the layout inside the hex is on sight"): the model is the whole catalogue as the
//! gr2 has it, and the choice of the parts and their places are the client's (RW-18). This set only
//! keeps the B1 records `city/<culture>/<era_bucket>` complete until the B2 client replaces them.

use std::collections::BTreeMap;
use std::path::Path;

use asset_format::{
    ArtTable, CityArt, ContentHash, LogicalId, ModelBlob, PrefetchGroup, VariantFormat,
};

use super::{
    TextureOptions, add_model, add_textures, cached_model, load_art_table, store_art_table,
};
use crate::game::GameSource;
use crate::models::set::{LoadedMember, load_fxsxml, model_id, rgba_texture, texture_id};
use crate::models::{ModelError, write_glb};
use crate::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder, StoreStats};

pub const SET_NAME: &str = "b1-cities";

/// Flat name of the building art definitions.
pub const CITY_DEFS_FILE: &str = "civ5artdefines_citybuildings.xml";

/// `<Culture name>` of the XML to the `ArtStylePrefix` key of the table.
pub const CULTURES: [(&str, &str); 6] = [
    ("America", "AMERICAN"),
    ("Asia", "ASIAN"),
    ("Africa", "AFRICAN"),
    ("Europe", "EUROPEAN"),
    ("Mediterranean", "MEDITERRANEAN"),
    ("Ruins", "RUINS"),
];

/// `<Era name>` values built (the second is spelled so in the game files).
pub const ERAS: [&str; 2] = ["Ancient", "Rennaisance"];

/// A `<Granny>` entry of the definitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityDef {
    pub culture: String,
    pub era: String,
    /// gr2 name as written (`Euro_Anc_City.gr2`).
    pub gr2: String,
}

/// Parses `Civ5ArtDefines_CityBuildings.xml`: `<Culture name>` / `<Era name>` /
/// `<PopulationBuildings><Granny>`.
pub fn parse_city_defs(xml: &str) -> Result<Vec<CityDef>, String> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("{CITY_DEFS_FILE}: {e}"))?;
    let mut out = Vec::new();
    for culture in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("Culture"))
    {
        let c = culture
            .attribute("name")
            .ok_or_else(|| format!("{CITY_DEFS_FILE}: <Culture> without name"))?;
        for era in culture.children().filter(|n| n.has_tag_name("Era")) {
            let e = era
                .attribute("name")
                .ok_or_else(|| format!("{CITY_DEFS_FILE}: <Era> of {c} without name"))?;
            let gr2 = era
                .children()
                .find(|n| n.has_tag_name("PopulationBuildings"))
                .and_then(|p| p.children().find(|n| n.has_tag_name("Granny")))
                .and_then(|g| g.text())
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .ok_or_else(|| format!("{CITY_DEFS_FILE}: {c}/{e} has no <Granny>"))?;
            out.push(CityDef {
                culture: c.to_owned(),
                era: e.to_owned(),
                gr2: gr2.to_owned(),
            });
        }
    }
    if out.is_empty() {
        return Err(format!("{CITY_DEFS_FILE} has no cities"));
    }
    Ok(out)
}

/// One `city/<culture>/<era>` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityEntry {
    /// `ArtStylePrefix` key.
    pub culture: &'static str,
    pub era: &'static str,
    /// File stem of the model id: `city_european_ancient`, `city_ruins`.
    pub model: String,
}

/// Records and models of the set, before any conversion.
#[derive(Debug, Default)]
pub struct CityPlan {
    pub entries: Vec<CityEntry>,
    /// Model stem to the flat fxsxml name.
    pub models: BTreeMap<String, String>,
}

/// Plans a record for every culture of [`CULTURES`] and era of [`ERAS`]. A missing definition is
/// an error.
pub fn plan_cities(defs: &[CityDef]) -> Result<CityPlan, String> {
    let mut plan = CityPlan::default();
    for (name, key) in CULTURES {
        for era in ERAS {
            let def = defs
                .iter()
                .find(|d| d.culture == name && d.era == era)
                .ok_or_else(|| format!("{CITY_DEFS_FILE} has no {name}/{era}"))?;
            let stem = def
                .gr2
                .rsplit_once('.')
                .map_or(def.gr2.as_str(), |(s, _)| s)
                .to_ascii_lowercase();
            let model = if key == "RUINS" {
                "city_ruins".to_owned()
            } else {
                format!(
                    "city_{}_{}",
                    key.to_ascii_lowercase(),
                    era.to_ascii_lowercase()
                )
            };
            let fx = format!("{stem}.fxsxml");
            if let Some(old) = plan.models.insert(model.clone(), fx.clone())
                && old != fx
            {
                return Err(format!("{model}: two different models {old} and {fx}"));
            }
            plan.entries.push(CityEntry {
                culture: key,
                era,
                model,
            });
        }
    }
    Ok(plan)
}

impl CityPlan {
    pub fn apply(&self, table: &mut ArtTable) {
        for e in &self.entries {
            table.insert_city(
                e.culture,
                e.era,
                CityArt {
                    model: model_id(&e.model),
                },
            );
        }
    }
}

fn hash_of(bytes: &[u8]) -> ContentHash {
    ContentHash(*blake3::hash(bytes).as_bytes())
}

/// Converts one loaded city catalogue as it is. Returns the blob and the notes.
fn city_blob(m: &LoadedMember) -> Result<(ModelBlob, Vec<String>), ModelError> {
    let blobs = m.convert()?;
    let notes = blobs
        .warnings
        .into_iter()
        .filter(|w| !w.contains("InitialPlacement"))
        .collect();
    Ok((blobs.model, notes))
}

/// What a build of the set produced.
#[derive(Debug)]
pub struct CitiesSummary {
    pub models: usize,
    pub textures: usize,
    pub bytes: BTreeMap<VariantFormat, u64>,
    pub model_bytes: u64,
    pub city_records: usize,
    pub table_entries: usize,
    pub table_bytes: u64,
    /// Material notes and conversion warnings, one line each.
    pub report: Vec<String>,
    pub stats: StoreStats,
}

impl CitiesSummary {
    pub fn total_bytes(&self) -> u64 {
        self.bytes.values().sum::<u64>() + self.model_bytes + self.table_bytes
    }
}

/// Builds the set into `out`: models, textures and table records, added to the manifest already
/// there.
pub fn build_cities(
    game: &GameSource,
    out: &Path,
    opts: TextureOptions,
) -> Result<CitiesSummary, ModelError> {
    let xml = game
        .read_by_name(CITY_DEFS_FILE)?
        .ok_or_else(|| ModelError::invalid(format!("{CITY_DEFS_FILE} is missing")))?;
    let defs = parse_city_defs(&String::from_utf8_lossy(&xml)).map_err(ModelError::invalid)?;
    let plan = plan_cities(&defs).map_err(ModelError::invalid)?;

    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut manifest = ManifestBuilder::open(&store)?;

    let mut report = Vec::new();
    let mut textures: BTreeMap<String, PrefetchGroup> = BTreeMap::new();
    let mut model_bytes = 0;
    for (stem, fx) in &plan.models {
        let m = load_fxsxml(game, stem, fx, 1.0)?;
        report.extend(m.plan.notes.iter().map(|n| format!("{stem}: {n}")));
        for f in m.texture_files() {
            textures.insert(f.to_ascii_lowercase(), PrefetchGroup(0));
        }
        let mesh_name = m.resolved.meshes()[0].clone();
        let fx_bytes = game.read_by_name(&m.resolved.fxsxml)?.unwrap_or_default();
        let mesh_bytes = game.read_by_name(&mesh_name)?.unwrap_or_default();
        let params = format!("{stem}|whole|{:?}", m.model_materials());
        let key = CacheKey::new(
            "city-model-b1-whole",
            super::MODEL_STEP_VERSION,
            &[hash_of(&fx_bytes), hash_of(&mesh_bytes)],
            params.as_bytes(),
        );
        let mut notes = Vec::new();
        let blob = cached_model(&cache, &store, &key, || {
            let (model, n) = city_blob(&m)?;
            notes = n;
            Ok(model)
        })?;
        report.extend(notes.into_iter().map(|n| format!("{stem}: {n}")));
        model_bytes += blob.1;
        add_model(&mut manifest, model_id(stem), blob, PrefetchGroup(0));
    }

    let read = |name: &str| -> Result<Option<Vec<u8>>, ModelError> { Ok(game.read_by_name(name)?) };
    let bytes = add_textures(&cache, &store, &mut manifest, &read, &textures, opts)?;

    let mut table = load_art_table(&store, &manifest)?;
    plan.apply(&mut table);
    let table_bytes = store_art_table(&store, &mut manifest, &table)?;
    manifest.finish(&store)?;
    Ok(CitiesSummary {
        models: plan.models.len(),
        textures: textures.len(),
        bytes,
        model_bytes,
        city_records: plan.entries.len(),
        table_entries: table.len(),
        table_bytes,
        report,
        stats: store.stats(),
    })
}

/// GLB bytes of the composed city of a table key (`EUROPEAN`, `Ancient`).
pub fn export_city_glb(game: &GameSource, culture: &str, era: &str) -> Result<Vec<u8>, ModelError> {
    let xml = game
        .read_by_name(CITY_DEFS_FILE)?
        .ok_or_else(|| ModelError::invalid(format!("{CITY_DEFS_FILE} is missing")))?;
    let defs = parse_city_defs(&String::from_utf8_lossy(&xml)).map_err(ModelError::invalid)?;
    let plan = plan_cities(&defs).map_err(ModelError::invalid)?;
    let entry = plan
        .entries
        .iter()
        .find(|e| e.culture.eq_ignore_ascii_case(culture) && e.era.eq_ignore_ascii_case(era))
        .ok_or_else(|| ModelError::invalid(format!("no city {culture}/{era}")))?;
    let m = load_fxsxml(game, &entry.model, &plan.models[&entry.model], 1.0)?;
    let (model, _) = city_blob(&m)?;
    let mut diffuse: Vec<(LogicalId, _)> = Vec::new();
    for f in model
        .materials
        .iter()
        .filter_map(|x| x.diffuse.as_ref())
        .map(|id| id.as_str().trim_start_matches("tex:").to_owned())
    {
        let id = texture_id(&f);
        if !diffuse.iter().any(|(i, _)| *i == id) {
            diffuse.push((id, rgba_texture(game, &f)?));
        }
    }
    write_glb(&model, &[], &diffuse, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<BuildingArtInfos>
  <Culture name="Europe">
    <Era name="Ancient"><PopulationBuildings><Granny>Euro_Anc_City.gr2</Granny></PopulationBuildings></Era>
    <Era name="Rennaisance"><PopulationBuildings><Granny>Euro_Ren_City.gr2</Granny></PopulationBuildings></Era>
    <Era name="Modern"><PopulationBuildings><Granny>Euro_Mod_City.gr2</Granny></PopulationBuildings></Era>
  </Culture>
  <Culture name="Ruins">
    <Era name="Ancient"><PopulationBuildings><Granny>City_Ruins.gr2</Granny></PopulationBuildings></Era>
    <Era name="Rennaisance"><PopulationBuildings><Granny>City_Ruins.gr2</Granny></PopulationBuildings></Era>
  </Culture>
</BuildingArtInfos>"#;

    #[test]
    fn parses_the_definitions() {
        let d = parse_city_defs(DEFS).unwrap();
        assert_eq!(d.len(), 5);
        assert_eq!(
            d[1],
            CityDef {
                culture: "Europe".into(),
                era: "Rennaisance".into(),
                gr2: "Euro_Ren_City.gr2".into()
            }
        );
        assert!(parse_city_defs("<BuildingArtInfos/>").is_err());
        assert!(
            parse_city_defs(
                "<BuildingArtInfos><Culture name=\"X\"><Era name=\"A\"/></Culture></BuildingArtInfos>"
            )
            .is_err()
        );
    }

    #[test]
    fn plan_needs_every_culture_and_shares_the_ruins() {
        // Only Europe and Ruins in the file: the other cultures are missing.
        let d = parse_city_defs(DEFS).unwrap();
        let e = plan_cities(&d).unwrap_err();
        assert!(e.contains("America"), "{e}");

        let mut all = String::from("<BuildingArtInfos>");
        for (name, _) in CULTURES {
            all += &format!("<Culture name=\"{name}\">");
            for era in ERAS {
                let stem = if name == "Ruins" {
                    "City_Ruins".to_owned()
                } else {
                    format!("{name}_{}_City", &era[..3])
                };
                all += &format!(
                    "<Era name=\"{era}\"><PopulationBuildings><Granny>{stem}.gr2</Granny></PopulationBuildings></Era>"
                );
            }
            all += "</Culture>";
        }
        all += "</BuildingArtInfos>";
        let plan = plan_cities(&parse_city_defs(&all).unwrap()).unwrap();
        assert_eq!(plan.entries.len(), 12);
        // 5 cultures x 2 eras + one shared ruins model.
        assert_eq!(plan.models.len(), 11);
        assert_eq!(plan.models["city_ruins"], "city_ruins.fxsxml");
        assert_eq!(
            plan.models["city_european_ancient"],
            "europe_anc_city.fxsxml"
        );
        let mut table = ArtTable::new();
        plan.apply(&mut table);
        assert_eq!(table.len(), 12);
        assert_eq!(
            table.city("EUROPEAN", "Rennaisance").unwrap().model,
            model_id("city_european_rennaisance")
        );
        assert_eq!(
            table.city("RUINS", "Ancient"),
            table.city("RUINS", "Rennaisance")
        );
    }
}
