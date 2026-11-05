//! Units of the B1 eras.

use std::collections::BTreeSet;

use super::{DbError, SnapDb, art, opt_i64, quote, req};

/// Eras whose units (by prerequisite tech) go into B1. Units without a prerequisite tech are
/// always in; barbarians are plain rows of `Units` and need no special case.
pub const B1_ERAS: [&str; 3] = ["ERA_ANCIENT", "ERA_CLASSICAL", "ERA_MEDIEVAL"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRow {
    pub unit_type: String,
    /// `Units.UnitArtInfo` (`ArtDefine_UnitInfos.Type`).
    pub art_info: String,
    pub prereq_tech: Option<String>,
    /// `Technologies.Era` of the prerequisite.
    pub era: Option<String>,
    /// `UnitArtInfoCulturalVariation`: the art has `ArtStyleSuffix` variants.
    pub cultural_variation: bool,
    /// `UnitArtInfoEraVariation`: the art changes with the owner's era.
    pub era_variation: bool,
}

/// All units of the B1 eras, ordered by unit type. Query: `Units` left-joined to the
/// prerequisite's `Technologies` row, kept when there is no prerequisite or its era is in
/// [`B1_ERAS`].
pub fn b1_units(db: &SnapDb) -> Result<Vec<UnitRow>, DbError> {
    let eras = B1_ERAS
        .iter()
        .map(|e| quote(e))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT u.Type, u.UnitArtInfo, u.PrereqTech, t.Era, \
                u.UnitArtInfoCulturalVariation, u.UnitArtInfoEraVariation \
         FROM Units u LEFT JOIN Technologies t ON t.Type = u.PrereqTech \
         WHERE u.PrereqTech IS NULL OR t.Era IN ({eras}) \
         ORDER BY u.Type"
    );
    db.query(&sql, 6)?
        .iter()
        .map(|r| {
            Ok(UnitRow {
                unit_type: req(r, 0, "Units.Type")?,
                art_info: req(r, 1, "Units.UnitArtInfo")?,
                prereq_tech: r[2].clone(),
                era: r[3].clone(),
                cultural_variation: opt_i64(r, 4, "UnitArtInfoCulturalVariation")?.unwrap_or(0)
                    != 0,
                era_variation: opt_i64(r, 5, "UnitArtInfoEraVariation")?.unwrap_or(0) != 0,
            })
        })
        .collect()
}

/// Distinct `ArtDefine_UnitInfos` types needed by `units`, sorted: each unit's own art info plus,
/// for units with a cultural variation, the style variants that exist (see
/// [`art::style_variants`]). Era variants are not included: they depend on the owner's era, not
/// on the style suffix.
pub fn b1_art_infos(db: &SnapDb, units: &[UnitRow]) -> Result<Vec<String>, DbError> {
    let mut out = BTreeSet::new();
    for u in units {
        out.insert(u.art_info.clone());
        if u.cultural_variation {
            for (_, name) in art::style_variants(db, &u.art_info)? {
                out.insert(name);
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// Every unit of the snapshot (all eras), ordered by unit type: the set `b2-units` builds the art
/// of all of them.
pub fn all_units(db: &SnapDb) -> Result<Vec<UnitRow>, DbError> {
    let sql = "SELECT u.Type, u.UnitArtInfo, u.PrereqTech, t.Era,                       u.UnitArtInfoCulturalVariation, u.UnitArtInfoEraVariation                FROM Units u LEFT JOIN Technologies t ON t.Type = u.PrereqTech                ORDER BY u.Type";
    db.query(sql, 6)?
        .iter()
        .map(|r| {
            Ok(UnitRow {
                unit_type: req(r, 0, "Units.Type")?,
                art_info: req(r, 1, "Units.UnitArtInfo")?,
                prereq_tech: r[2].clone(),
                era: r[3].clone(),
                cultural_variation: opt_i64(r, 4, "UnitArtInfoCulturalVariation")?.unwrap_or(0)
                    != 0,
                era_variation: opt_i64(r, 5, "UnitArtInfoEraVariation")?.unwrap_or(0) != 0,
            })
        })
        .collect()
}
