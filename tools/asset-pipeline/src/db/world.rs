//! Rows the world-object sets read: landmarks, buildings, civilizations in
//! full, routes and the unique-unit overrides. One query each, plain structs out.

use super::{DbError, SnapDb, opt_f32, opt_i64, req};

/// One `ArtDefine_Landmarks` row, in table order (`rowid`).
#[derive(Debug, Clone, PartialEq)]
pub struct LandmarkRow {
    pub era: String,
    pub state: String,
    pub scale: f32,
    pub improvement: String,
    pub layout: String,
    pub resource: String,
    /// `Model` as written (a path to the `.fxsxml`); NULL stays `None`.
    pub model: Option<String>,
    pub terrain_contour: i32,
    pub tech: Option<String>,
}

/// Every `ArtDefine_Landmarks` row (631 in the snapshot), in table order.
pub fn all_landmarks(db: &SnapDb) -> Result<Vec<LandmarkRow>, DbError> {
    db.query(
        "SELECT Era, State, Scale, ImprovementType, LayoutHandler, ResourceType, Model, \
                TerrainContour, Tech FROM ArtDefine_Landmarks ORDER BY rowid",
        10,
    )?
    .iter()
    .map(|r| {
        Ok(LandmarkRow {
            era: r[0].clone().unwrap_or_else(|| "Any".to_owned()),
            state: r[1].clone().unwrap_or_else(|| "Any".to_owned()),
            scale: opt_f32(r, 2, "ArtDefine_Landmarks.Scale")?.unwrap_or(1.0),
            improvement: req(r, 3, "ArtDefine_Landmarks.ImprovementType")?,
            layout: req(r, 4, "ArtDefine_Landmarks.LayoutHandler")?,
            resource: req(r, 5, "ArtDefine_Landmarks.ResourceType")?,
            model: r[6].clone().filter(|m| !m.trim().is_empty()),
            terrain_contour: i32::try_from(
                opt_i64(r, 7, "ArtDefine_Landmarks.TerrainContour")?.unwrap_or(0),
            )
            .map_err(|_| DbError::data("ArtDefine_Landmarks.TerrainContour", "out of range"))?,
            tech: r[8].clone().filter(|t| !t.trim().is_empty()),
        })
    })
    .collect()
}

/// A `Buildings` row, with the era of its prerequisite technology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildingRow {
    pub building_type: String,
    pub building_class: Option<String>,
    pub art_tag: Option<String>,
    pub cultural_variation: bool,
    pub era_variation: bool,
    pub random_variation: bool,
    pub city_wall: bool,
    pub prereq_tech: Option<String>,
    /// `Technologies.Era` of the prerequisite.
    pub era: Option<String>,
    /// `DisplayPosition`: the placement kind of the building's model in the city (a bit value), 0
    /// for none.
    pub display_position: u8,
}

/// Every building (262 in the snapshot), ordered by type.
pub fn all_buildings(db: &SnapDb) -> Result<Vec<BuildingRow>, DbError> {
    db.query(
        "SELECT b.Type, b.BuildingClass, b.ArtDefineTag, b.ArtInfoCulturalVariation, \
                b.ArtInfoEraVariation, b.ArtInfoRandomVariation, b.CityWall, b.PrereqTech, t.Era, \
                b.DisplayPosition \
         FROM Buildings b LEFT JOIN Technologies t ON t.Type = b.PrereqTech ORDER BY b.Type",
        10,
    )?
    .iter()
    .map(|r| {
        let flag = |i: usize, what: &str| -> Result<bool, DbError> {
            Ok(opt_i64(r, i, what)?.unwrap_or(0) != 0)
        };
        Ok(BuildingRow {
            building_type: req(r, 0, "Buildings.Type")?,
            building_class: r[1].clone(),
            art_tag: r[2].clone().filter(|t| !t.trim().is_empty()),
            cultural_variation: flag(3, "ArtInfoCulturalVariation")?,
            era_variation: flag(4, "ArtInfoEraVariation")?,
            random_variation: flag(5, "ArtInfoRandomVariation")?,
            city_wall: flag(6, "CityWall")?,
            prereq_tech: r[7].clone(),
            era: r[8].clone(),
            display_position: u8::try_from(opt_i64(r, 9, "DisplayPosition")?.unwrap_or(0))
                .map_err(|_| DbError::data("Buildings.DisplayPosition", "out of range"))?,
        })
    })
    .collect()
}

/// A civilization with everything the art needs of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CivArtRow {
    pub civ_type: String,
    pub art_define_tag: Option<String>,
    pub art_style_type: Option<String>,
    pub art_style_prefix: Option<String>,
    pub art_style_suffix: Option<String>,
    pub default_player_color: Option<String>,
}

/// Every civilization (115 in the snapshot, the minor and barbarian ones included), by type.
pub fn all_civ_art(db: &SnapDb) -> Result<Vec<CivArtRow>, DbError> {
    db.query(
        "SELECT Type, ArtDefineTag, ArtStyleType, ArtStylePrefix, ArtStyleSuffix, \
                DefaultPlayerColor FROM Civilizations ORDER BY Type",
        6,
    )?
    .iter()
    .map(|r| {
        Ok(CivArtRow {
            civ_type: req(r, 0, "Civilizations.Type")?,
            art_define_tag: r[1].clone(),
            art_style_type: r[2].clone(),
            art_style_prefix: r[3].clone(),
            art_style_suffix: r[4].clone(),
            default_player_color: r[5].clone(),
        })
    })
    .collect()
}

/// `(civilization, unit type)` of `Civilization_UnitClassOverrides`: the unique units. A row
/// without a unit (the class is blocked for the civilization) is left out.
pub fn unit_overrides(db: &SnapDb) -> Result<Vec<(String, String)>, DbError> {
    db.query(
        "SELECT CivilizationType, UnitType FROM Civilization_UnitClassOverrides \
         WHERE UnitType IS NOT NULL ORDER BY CivilizationType, UnitType",
        2,
    )?
    .iter()
    .map(|r| {
        Ok((
            req(r, 0, "Civilization_UnitClassOverrides.CivilizationType")?,
            req(r, 1, "Civilization_UnitClassOverrides.UnitType")?,
        ))
    })
    .collect()
}

/// `Eras.Type` in game order (`ERA_ANCIENT` first).
pub fn eras(db: &SnapDb) -> Result<Vec<String>, DbError> {
    db.query("SELECT Type FROM Eras ORDER BY ID", 1)?
        .iter()
        .map(|r| req(r, 0, "Eras.Type"))
        .collect()
}

/// `Routes.Type` in table order (`ROUTE_ROAD`, `ROUTE_RAILROAD`).
pub fn routes(db: &SnapDb) -> Result<Vec<String>, DbError> {
    db.query("SELECT Type FROM Routes ORDER BY ID", 1)?
        .iter()
        .map(|r| req(r, 0, "Routes.Type"))
        .collect()
}
