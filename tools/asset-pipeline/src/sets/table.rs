//! Set `b1-table`: the final step of the composite `b1`. Every other
//! set already merged its records into `table:art-b1`, so this one only reads the table back and
//! checks that it is complete and that every id it names is in the manifest.
//!
//! The checks, each failing with a list of what is missing:
//! - every model and texture id named by a record has a variant in the manifest;
//! - every `Terrains.Type` of the snapshot has a `terrain/<type>/<0-4>` record;
//! - forests and jungles have `feature/<type>/<0-4>` records with a tree model and placement;
//! - every culture x era of the city set has a `city/<culture>/<era>` record;
//! - every unit of the B1 eras has a `unit/<type>` record, and its formation a `formation/` one;
//! - the river record exists.

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{ArtRecord, ArtTable, LogicalId, Manifest, UnitArt};

use super::cities::{CULTURES, ERAS};
use super::terrain::STYLES;
use super::trees::KINDS;
use super::{ART_TABLE_ID, load_art_table};
use crate::models::ModelError;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder};

pub const SET_NAME: &str = "b1-table";

/// What the table must cover, from the snapshot and the other sets' constants.
#[derive(Debug, Clone, Default)]
pub struct Expected {
    /// `Terrains.Type`.
    pub terrains: Vec<String>,
    /// `Units.Type` of the B1 eras (and the units outside any era).
    pub units: Vec<String>,
}

/// Ids a record names, with a label of where they come from.
fn referenced(table: &ArtTable) -> Vec<(String, LogicalId)> {
    let mut out = Vec::new();
    let unit_art = |key: &str, what: &str, art: &UnitArt, out: &mut Vec<(String, LogicalId)>| {
        for f in &art.figures {
            out.push((format!("{key} {what} figure"), f.model.clone()));
        }
        if let Some(sv) = &art.sv_icon {
            out.push((format!("{key} {what} sv icon"), sv.clone()));
        }
    };
    for (key, rec) in table.iter() {
        let mut add = |what: &str, id: &LogicalId| out.push((format!("{key} {what}"), id.clone()));
        match rec {
            ArtRecord::Unit(u) => {
                unit_art(key, "base", &u.base, &mut out);
                for v in &u.variants {
                    unit_art(key, &v.suffix, &v.art, &mut out);
                }
            }
            ArtRecord::Terrain(t) => add("texture", &t.texture),
            ArtRecord::Feature(f) => {
                if let Some(t) = &f.texture {
                    add("texture", t);
                }
                if let Some(m) = &f.tree_model {
                    add("tree model", m);
                }
            }
            ArtRecord::River(r) => r.textures.iter().for_each(|t| add("texture", t)),
            ArtRecord::City(c) => add("model", &c.model),
            ArtRecord::Sv(s) => add("icon", &s.icon),
            ArtRecord::PlayerColor(_) | ArtRecord::Formation(_) => {}
            // B2 records live in `tb:art-b2`; named here so a table that holds them is checked
            // the same way.
            ArtRecord::UnitB2(u) => {
                let arts = std::iter::once(("base", &u.base))
                    .chain(u.style_variants.iter().map(|v| (v.suffix.as_str(), &v.art)))
                    .chain(u.era_variants.iter().map(|v| (v.suffix.as_str(), &v.art)));
                for (what, art) in arts {
                    for f in &art.figures {
                        out.push((format!("{key} {what} figure"), f.model.clone()));
                    }
                }
            }
            ArtRecord::VisEffect(_) | ArtRecord::Civ(_) | ArtRecord::BuildingType(_) => {}
            ArtRecord::Landmark(l) => {
                if let Some(m) = &l.model {
                    add("model", m);
                }
            }
            ArtRecord::Building(b) => {
                if let Some(m) = &b.model {
                    add("model", m);
                }
                b.variants
                    .iter()
                    .for_each(|v| add("variant model", &v.model));
            }
            ArtRecord::Wonder(w) => w.states.iter().for_each(|s| add("model", &s.model)),
            ArtRecord::CityB2(c) => add("model", &c.model),
            ArtRecord::Wall(w) => add("model", &w.model),
            ArtRecord::Route(r) => r.models.iter().for_each(|m| add("model", &m.model)),
            ArtRecord::Overlay(o) => {
                if let Some(m) = &o.model {
                    add("model", m);
                }
                o.textures.iter().for_each(|t| add("texture", t));
            }
            ArtRecord::Tree(t) => {
                add("model", &t.model);
                add("overlay", &t.overlay);
                t.masks.iter().for_each(|m| add("mask", m));
            }
        }
    }
    out
}

/// Every problem found, empty when the table is complete. Sorted and without repeats.
pub fn validate(table: &ArtTable, manifest: &Manifest, expected: &Expected) -> Vec<String> {
    let mut bad: BTreeSet<String> = BTreeSet::new();
    for (what, id) in referenced(table) {
        if manifest.variants(&id).is_empty() {
            bad.insert(format!("{what}: {} is not in the manifest", id.as_str()));
        }
    }
    for t in &expected.terrains {
        for style in STYLES {
            if table.terrain(t, style).is_none() {
                bad.insert(format!("terrain/{t}/{style}: no record"));
            }
        }
    }
    for kind in KINDS {
        for style in STYLES {
            match table.feature(kind.feature, style) {
                None => {
                    bad.insert(format!("feature/{}/{style}: no record", kind.feature));
                }
                Some(f) => {
                    if f.tree_model.is_none() {
                        bad.insert(format!("feature/{}/{style}: no tree model", kind.feature));
                    }
                    if f.placement.is_empty() {
                        bad.insert(format!("feature/{}/{style}: no placement", kind.feature));
                    }
                }
            }
        }
    }
    for (_, culture) in CULTURES {
        for era in ERAS {
            if !matches!(
                table.get(&format!("city/{culture}/{era}")),
                Some(ArtRecord::City(_))
            ) {
                bad.insert(format!("city/{culture}/{era}: no record"));
            }
        }
    }
    if !matches!(table.get("river"), Some(ArtRecord::River(_))) {
        bad.insert("river: no record".to_owned());
    }
    for u in &expected.units {
        let Some(rec) = table.unit(u) else {
            bad.insert(format!("unit/{u}: no record"));
            continue;
        };
        if rec.base.figures.is_empty() {
            bad.insert(format!("unit/{u}: no figures"));
        }
        let arts = std::iter::once(&rec.base).chain(rec.variants.iter().map(|v| &v.art));
        for name in arts.filter_map(|a| a.formation.as_deref()) {
            if !matches!(
                table.get(&format!("formation/{name}")),
                Some(ArtRecord::Formation(_))
            ) {
                bad.insert(format!("unit/{u}: formation/{name} has no record"));
            }
        }
    }
    bad.into_iter().collect()
}

/// Counts per key kind, placeholder units and the table size for the report.
pub fn summary(table: &ArtTable, table_bytes: u64) -> SetReport {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut placeholders = 0;
    for (key, rec) in table.iter() {
        let kind = key.split('/').next().unwrap_or(key);
        *counts.entry(format!("records {kind}")).or_default() += 1;
        if let ArtRecord::Unit(u) = rec
            && u.base.placeholder
        {
            placeholders += 1;
        }
    }
    counts.insert("records".to_owned(), table.len() as u64);
    counts.insert("placeholder units".to_owned(), placeholders);
    SetReport {
        entries: Vec::new(),
        counts,
        bytes: BTreeMap::from([("table".to_owned(), table_bytes)]),
    }
}

/// Reads the table and the manifest of `out`, checks them and returns the summary; a table that
/// is missing, or incomplete, is an error listing every problem. The report entries of a failed
/// check come back in `Err` too, so the caller can put them into the report.
pub fn check(
    store: &BlobStore,
    expected: &Expected,
) -> Result<SetReport, Box<(SetReport, ModelError)>> {
    let fail = |e: ModelError| Box::new((SetReport::default(), e));
    let manifest = ManifestBuilder::open(store).map_err(|e| fail(e.into()))?;
    let id = LogicalId::new(ART_TABLE_ID);
    let Some(v) = manifest.manifest().variants(&id).first() else {
        return Err(fail(ModelError::invalid(format!(
            "{ART_TABLE_ID} is not in the manifest: build the other b1 sets first"
        ))));
    };
    let size = v.size;
    let table = load_art_table(store, &manifest).map_err(fail)?;
    let mut report = summary(&table, size);
    let problems = validate(&table, manifest.manifest(), expected);
    if problems.is_empty() {
        return Ok(report);
    }
    report.entries = problems
        .iter()
        .map(|p| ReportEntry::new("table-missing", p.as_str(), ""))
        .collect();
    let shown: Vec<&str> = problems.iter().take(60).map(String::as_str).collect();
    let more = problems.len().saturating_sub(shown.len());
    let err = ModelError::invalid(format!(
        "{ART_TABLE_ID} is incomplete, {} problem(s):\n  {}{}",
        problems.len(),
        shown.join("\n  "),
        if more > 0 {
            format!("\n  ... and {more} more (see report-b1.txt)")
        } else {
            String::new()
        }
    ));
    Err(Box::new((report, err)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{
        CityArt, FeatureArt, Figure, PrefetchGroup, Projection, RiverArt, TerrainArt, UnitRecord,
        Variant, VariantFormat,
    };

    fn present(m: &mut Manifest, id: &str) {
        m.insert(
            LogicalId::new(id),
            Variant {
                format: VariantFormat::Generic,
                hash: asset_format::ContentHash([1; 32]),
                size: 1,
                prefetch: PrefetchGroup(1),
            },
        );
    }

    fn unit(model: &str) -> UnitRecord {
        UnitRecord {
            base: UnitArt {
                art_info: "ART_DEF_UNIT_X".into(),
                figures: vec![Figure {
                    model: LogicalId::new(model),
                    count: 1,
                    scale: 1.0,
                    z_offset: 0.0,
                }],
                formation: None,
                sv_icon: None,
                placeholder: false,
            },
            variants: Vec::new(),
        }
    }

    /// A complete table for terrain `TERRAIN_T` and unit `UNIT_X`, and its manifest.
    fn complete() -> (ArtTable, Manifest, Expected) {
        let mut t = ArtTable::new();
        let mut m = Manifest::new();
        present(&mut m, "tex:t.dds");
        present(&mut m, "tex:river.dds");
        present(&mut m, "model:tree");
        present(&mut m, "model:city");
        present(&mut m, "model:u");
        for s in STYLES {
            t.insert_terrain(
                "TERRAIN_T",
                s,
                TerrainArt {
                    texture: LogicalId::new("tex:t.dds"),
                    texture_scale: 1.0,
                    projection: Projection::WorldXz,
                },
            );
            for k in KINDS {
                t.insert_feature(
                    k.feature,
                    s,
                    FeatureArt {
                        texture: None,
                        tree_model: Some(LogicalId::new("model:tree")),
                        placement: BTreeMap::from([("density".to_owned(), 1.0)]),
                    },
                );
            }
        }
        for (_, c) in CULTURES {
            for e in ERAS {
                t.insert_city(
                    c,
                    e,
                    CityArt {
                        model: LogicalId::new("model:city"),
                    },
                );
            }
        }
        t.insert_river(RiverArt {
            textures: vec![LogicalId::new("tex:river.dds")],
        });
        t.insert_unit("UNIT_X", unit("model:u"));
        let e = Expected {
            terrains: vec!["TERRAIN_T".into()],
            units: vec!["UNIT_X".into()],
        };
        (t, m, e)
    }

    #[test]
    fn a_complete_table_passes() {
        let (t, m, e) = complete();
        assert_eq!(validate(&t, &m, &e), Vec::<String>::new());
    }

    #[test]
    fn a_missing_manifest_id_is_listed() {
        let (t, mut m, e) = complete();
        m.remove(&LogicalId::new("model:u"));
        let bad = validate(&t, &m, &e);
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(
            bad[0].contains("unit/UNIT_X") && bad[0].contains("model:u"),
            "{bad:?}"
        );
    }

    #[test]
    fn missing_records_are_listed() {
        let (t, m, mut e) = complete();
        e.terrains.push("TERRAIN_U".into());
        e.units.push("UNIT_Y".into());
        let bad = validate(&t, &m, &e);
        assert_eq!(bad.len(), 5 + 1, "{bad:?}");
        assert!(bad.contains(&"terrain/TERRAIN_U/4: no record".to_owned()));
        assert!(bad.contains(&"unit/UNIT_Y: no record".to_owned()));
    }

    #[test]
    fn a_feature_without_tree_model_or_placement_is_listed() {
        let (mut t, m, e) = complete();
        t.insert_feature(
            "FEATURE_FOREST",
            0,
            FeatureArt {
                texture: None,
                tree_model: None,
                placement: BTreeMap::new(),
            },
        );
        let bad = validate(&t, &m, &e);
        assert_eq!(bad.len(), 2, "{bad:?}");
    }

    #[test]
    fn an_unknown_formation_is_listed() {
        let (mut t, m, e) = complete();
        let mut u = unit("model:u");
        u.base.formation = Some("Nope".into());
        t.insert_unit("UNIT_X", u);
        let bad = validate(&t, &m, &e);
        assert_eq!(
            bad,
            vec!["unit/UNIT_X: formation/Nope has no record".to_owned()]
        );
    }

    #[test]
    fn summary_counts_by_key_kind() {
        let (t, _, _) = complete();
        let s = summary(&t, 7);
        assert_eq!(s.counts["records terrain"], 5);
        assert_eq!(
            s.counts["records city"],
            (CULTURES.len() * ERAS.len()) as u64
        );
        assert_eq!(s.counts["records river"], 1);
        assert_eq!(s.counts["records"], t.len() as u64);
        assert_eq!(s.bytes["table"], 7);
    }
}
