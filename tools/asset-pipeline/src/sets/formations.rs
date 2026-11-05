//! Art table inputs for unit formations and forest/jungle placement.
//!
//! Pure functions: the caller reads `Assets/Units/UnitFormations.xml` and `forests.xml` from the
//! install and passes their text in.
//!
//! `UnitFormations.xml` (checked against the install) is `<Formations>` with one `<Formation>` per
//! row; each row holds a `<Name>` and numeric child elements (`<SpacingX>`, `<TotalWidth>`, ...).
//!
//! `forests.xml` is not a loose file: it is packed in the game archives (`Base`, 1119 bytes, found
//! through `GameSource::lookup`). Its real format (checked against the install) is one element
//! `<Forest num_tiles="4" random_trees_per_tile="360" gridded_trees_per_tile="0" overlay_alpha=".60"
//! space_between="1.33"/>` after a comment that documents the five attributes. [`parse_forests`]
//! returns them under their attribute names.

use std::collections::BTreeMap;

use asset_format::{ArtTable, FeatureArt, Formation};

/// Features whose placement parameters come from `forests.xml`.
pub const PLACEMENT_FEATURES: &[&str] = &["FEATURE_FOREST", "FEATURE_JUNGLE"];

/// Continent styles (`terrain::STYLES`).
const CONTINENTS: std::ops::RangeInclusive<u8> = 0..=4;

/// Child elements of `<Formation>` that go to named [`Formation`] fields.
const NAMED: [&str; 5] = [
    "SpacingX",
    "SpacingY",
    "TotalWidth",
    "UnitRandomness",
    "Stagger",
];

/// Parses `UnitFormations.xml`: one `(name, Formation)` per `<Formation>` element, in file order.
/// The five named fields come from `SpacingX`, `SpacingY`, `TotalWidth`, `UnitRandomness` and
/// `Stagger`; every other numeric child goes to `extra` under its element name. Non-numeric
/// children other than `Name` are ignored. Unknown root children are an error.
pub fn parse_unit_formations(xml: &str) -> Result<Vec<(String, Formation)>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("UnitFormations.xml: {e}"))?;
    let mut out = Vec::new();
    for node in doc.root_element().children().filter(|n| n.is_element()) {
        if node.tag_name().name() != "Formation" {
            return Err(format!(
                "UnitFormations.xml: unexpected element <{}>",
                node.tag_name().name()
            ));
        }
        let mut name = None;
        let mut named: BTreeMap<&str, f32> = BTreeMap::new();
        let mut extra = BTreeMap::new();
        for field in node.children().filter(|n| n.is_element()) {
            let tag = field.tag_name().name();
            let text = field.text().unwrap_or("").trim();
            if tag == "Name" {
                name = Some(text.to_owned());
                continue;
            }
            match text.parse::<f32>() {
                Ok(v) if NAMED.contains(&tag) => {
                    named.insert(tag, v);
                }
                Ok(v) => {
                    extra.insert(tag.to_owned(), v);
                }
                Err(_) if NAMED.contains(&tag) => {
                    return Err(format!("UnitFormations.xml: bad {tag} {text:?}"));
                }
                Err(_) => {}
            }
        }
        let name =
            name.ok_or_else(|| "UnitFormations.xml: <Formation> without <Name>".to_owned())?;
        let get = |tag: &str| {
            named
                .get(tag)
                .copied()
                .ok_or_else(|| format!("UnitFormations.xml formation {name:?}: missing {tag}"))
        };
        let rec = Formation {
            spacing_x: get("SpacingX")?,
            spacing_y: get("SpacingY")?,
            total_width: get("TotalWidth")?,
            unit_randomness: get("UnitRandomness")?,
            stagger: get("Stagger")?,
            extra,
        };
        out.push((name, rec));
    }
    if out.is_empty() {
        return Err("UnitFormations.xml has no formations".to_owned());
    }
    Ok(out)
}

/// Numeric attributes of `forests.xml`: `num_tiles` (number of `forest_tile<i>.dds` masks),
/// `random_trees_per_tile`, `gridded_trees_per_tile`, `overlay_alpha`, `space_between`.
pub const FOREST_KEYS: [&str; 5] = [
    "num_tiles",
    "random_trees_per_tile",
    "gridded_trees_per_tile",
    "overlay_alpha",
    "space_between",
];

/// Parses `forests.xml`: the root must be `<Forest>`; every attribute must be a number and is
/// returned under its name. A missing key of [`FOREST_KEYS`] is an error (the client reads them).
pub fn parse_forests(xml: &str) -> Result<BTreeMap<String, f32>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("forests.xml: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "Forest" {
        return Err(format!(
            "forests.xml: unexpected root <{}>, expected <Forest>",
            root.tag_name().name()
        ));
    }
    let mut out = BTreeMap::new();
    for attr in root.attributes() {
        let v = attr
            .value()
            .trim()
            .parse::<f32>()
            .map_err(|_| format!("forests.xml: bad {} {:?}", attr.name(), attr.value()))?;
        out.insert(attr.name().to_owned(), v);
    }
    if let Some(missing) = FOREST_KEYS.iter().find(|k| !out.contains_key(**k)) {
        return Err(format!("forests.xml: missing attribute {missing}"));
    }
    Ok(out)
}

/// Adds the formations as `formation/<name>` and merges `forest_params` into the `placement` of
/// `feature/FEATURE_FOREST/<0-4>` and `feature/FEATURE_JUNGLE/<0-4>`. A missing feature record is
/// created with no texture and no tree model; an existing one keeps its texture and tree model.
pub fn apply(
    table: &mut ArtTable,
    formations: &[(String, Formation)],
    forest_params: &BTreeMap<String, f32>,
) {
    for (name, rec) in formations {
        table.insert_formation(name, rec.clone());
    }
    for feature in PLACEMENT_FEATURES {
        for continent in CONTINENTS {
            let mut rec = table
                .feature(feature, continent)
                .cloned()
                .unwrap_or(FeatureArt {
                    texture: None,
                    tree_model: None,
                    placement: BTreeMap::new(),
                });
            rec.placement
                .extend(forest_params.iter().map(|(k, v)| (k.clone(), *v)));
            table.insert_feature(feature, continent, rec);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::LogicalId;

    const FORMATIONS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Formations>
	<Formation>
		<Name>DefaultCavalry</Name>
		<SpacingX>12</SpacingX>
		<SpacingY>25</SpacingY>
		<SpacingMultiplier>1</SpacingMultiplier>
		<Stagger>1</Stagger>
		<UnitRandomness>0.2</UnitRandomness>
		<TotalWidth>3</TotalWidth>
		<CenterWidth>4</CenterWidth>
	</Formation>
	<Formation>
		<Name>Warrior</Name>
		<SpacingX>13</SpacingX>
		<SpacingY>13.5</SpacingY>
		<Stagger>0</Stagger>
		<UnitRandomness>0.4</UnitRandomness>
		<TotalWidth>4</TotalWidth>
	</Formation>
</Formations>"#;

    const FORESTS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!--
num_tiles:
  This specifies how many tile textures there are to load.
-->

<Forest num_tiles="4" random_trees_per_tile="360" gridded_trees_per_tile="0" overlay_alpha=".60" space_between="1.33"/>"#;

    fn forest_params() -> BTreeMap<String, f32> {
        BTreeMap::from([
            ("num_tiles".to_owned(), 3.0),
            ("overlay_alpha".to_owned(), 0.5),
        ])
    }

    #[test]
    fn parses_formation_fields_and_extra() {
        let f = parse_unit_formations(FORMATIONS).unwrap();
        assert_eq!(f.len(), 2);
        let (name, cav) = &f[0];
        assert_eq!(name, "DefaultCavalry");
        assert_eq!(cav.spacing_x, 12.0);
        assert_eq!(cav.spacing_y, 25.0);
        assert_eq!(cav.total_width, 3.0);
        assert_eq!(cav.unit_randomness, 0.2);
        assert_eq!(cav.stagger, 1.0);
        assert_eq!(
            cav.extra,
            BTreeMap::from([
                ("SpacingMultiplier".to_owned(), 1.0),
                ("CenterWidth".to_owned(), 4.0)
            ])
        );
        assert_eq!(f[1].0, "Warrior");
        assert_eq!(f[1].1.spacing_y, 13.5);
        assert!(f[1].1.extra.is_empty());
    }

    #[test]
    fn formation_missing_named_field_is_err() {
        let xml =
            "<Formations><Formation><Name>X</Name><SpacingX>1</SpacingX></Formation></Formations>";
        assert!(parse_unit_formations(xml).is_err());
    }

    #[test]
    fn parses_forest_attributes() {
        let p = parse_forests(FORESTS).unwrap();
        assert_eq!(p.len(), 5);
        assert_eq!(p["num_tiles"], 4.0);
        assert_eq!(p["random_trees_per_tile"], 360.0);
        assert_eq!(p["gridded_trees_per_tile"], 0.0);
        assert_eq!(p["overlay_alpha"], 0.6);
        assert_eq!(p["space_between"], 1.33);
    }

    #[test]
    fn apply_keeps_existing_texture_and_merges_placement() {
        let mut table = ArtTable::new();
        table.insert_feature(
            "FEATURE_FOREST",
            0,
            FeatureArt {
                texture: Some(LogicalId::new("tex:forest")),
                tree_model: Some(LogicalId::new("model:forest_euro")),
                placement: BTreeMap::from([("Jitter".to_owned(), 0.1)]),
            },
        );
        let formations = parse_unit_formations(FORMATIONS).unwrap();
        apply(&mut table, &formations, &forest_params());

        let kept = table.feature("FEATURE_FOREST", 0).unwrap();
        assert_eq!(kept.texture, Some(LogicalId::new("tex:forest")));
        assert_eq!(kept.tree_model, Some(LogicalId::new("model:forest_euro")));
        assert_eq!(kept.placement["Jitter"], 0.1);
        assert_eq!(kept.placement["num_tiles"], 3.0);

        let created = table.feature("FEATURE_JUNGLE", 4).unwrap();
        assert!(created.texture.is_none() && created.tree_model.is_none());
        assert_eq!(created.placement, forest_params());

        assert_eq!(table.formation("Warrior").unwrap().spacing_x, 13.0);
        assert!(table.formation("DefaultCavalry").is_some());
    }

    #[test]
    fn bad_forests_are_errors() {
        assert!(parse_unit_formations("<Formations><Formation>").is_err());
        assert!(parse_forests("<Forest").is_err());
        assert!(parse_forests("<Forests num_tiles=\"4\"/>").is_err());
        // A key is missing.
        assert!(parse_forests("<Forest num_tiles=\"4\"/>").is_err());
        // A non-numeric attribute.
        let bad = FORESTS.replace("\"360\"", "\"many\"");
        assert!(parse_forests(&bad).is_err());
    }
}
