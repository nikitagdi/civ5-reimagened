//! The unit art chain: `ArtDefine_UnitInfos` -> `ArtDefine_UnitInfoMemberInfos` ->
//! `ArtDefine_UnitMemberInfos`, style variants, and `ArtDefine_StrategicView`.

use std::collections::BTreeMap;

use super::{DbError, SnapDb, opt_f32, opt_i64, quote, req};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtInfoRow {
    pub art_info: String,
    /// Formation name (key of `Assets/Units/UnitFormations.xml`); NULL for civilians and ships.
    pub formation: Option<String>,
    pub damage_states: Option<i64>,
}

/// One figure kind of a squad.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    /// `ArtDefine_UnitMemberInfos.Type`.
    pub member_type: String,
    /// `NumMembers` of the squad.
    pub count: u32,
    /// `Scale`; NULL stays `None`.
    pub scale: Option<f32>,
    pub z_offset: Option<f32>,
    /// `Model`: a path to the `.fxsxml` as written in the DB (some have no directory).
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategicViewRow {
    pub tag: String,
    /// `Unit`, `Improvement`, `Resource`, `Feature`, `Terrain`.
    pub tile_type: String,
    /// Texture file name, e.g. `sv_dhow.dds`.
    pub asset: String,
}

/// One `ArtDefine_UnitInfos` row; `None` when the type is not defined there.
pub fn art_info(db: &SnapDb, art_info: &str) -> Result<Option<ArtInfoRow>, DbError> {
    let sql = format!(
        "SELECT Type, Formation, DamageStates FROM ArtDefine_UnitInfos WHERE Type = {}",
        quote(art_info)
    );
    let rows = db.query(&sql, 3)?;
    rows.first()
        .map(|r| {
            Ok(ArtInfoRow {
                art_info: req(r, 0, "ArtDefine_UnitInfos.Type")?,
                formation: r[1].clone(),
                damage_states: opt_i64(r, 2, "ArtDefine_UnitInfos.DamageStates")?,
            })
        })
        .transpose()
}

/// Members of an art info in table order (`rowid` of `ArtDefine_UnitInfoMemberInfos`). A member
/// type without an `ArtDefine_UnitMemberInfos` row is an error: the chain is broken.
pub fn members(db: &SnapDb, art_info: &str) -> Result<Vec<MemberRow>, DbError> {
    let sql = format!(
        "SELECT m.UnitMemberInfoType, m.NumMembers, i.Scale, i.ZOffset, i.Model \
         FROM ArtDefine_UnitInfoMemberInfos m \
         LEFT JOIN ArtDefine_UnitMemberInfos i ON i.Type = m.UnitMemberInfoType \
         WHERE m.UnitInfoType = {} ORDER BY m.rowid",
        quote(art_info)
    );
    db.query(&sql, 5)?
        .iter()
        .map(|r| {
            let member_type = req(r, 0, "UnitInfoMemberInfos.UnitMemberInfoType")?;
            let model = r[4].clone().ok_or_else(|| {
                DbError::data(
                    art_info,
                    format!("member {member_type} has no ArtDefine_UnitMemberInfos row"),
                )
            })?;
            let count = opt_i64(r, 1, "UnitInfoMemberInfos.NumMembers")?.unwrap_or(1);
            Ok(MemberRow {
                scale: opt_f32(r, 2, "UnitMemberInfos.Scale")?,
                z_offset: opt_f32(r, 3, "UnitMemberInfos.ZOffset")?,
                count: u32::try_from(count)
                    .map_err(|_| DbError::data(art_info, format!("NumMembers {count}")))?,
                member_type,
                model,
            })
        })
        .collect()
}

/// Distinct non-NULL `Civilizations.ArtStyleSuffix` values, sorted (`_AFRI`, `_AMER`, `_ASIA`,
/// `_EURO`, `_MED` in the snapshot).
pub fn style_suffixes(db: &SnapDb) -> Result<Vec<String>, DbError> {
    let rows = db.query(
        "SELECT DISTINCT ArtStyleSuffix FROM Civilizations \
         WHERE ArtStyleSuffix IS NOT NULL ORDER BY 1",
        1,
    )?;
    rows.iter().map(|r| req(r, 0, "ArtStyleSuffix")).collect()
}

/// Style variants of an art info: `(suffix, art info type)` for every style suffix such that
/// `<art_info><suffix>` is an `ArtDefine_UnitInfos` type, sorted by suffix.
pub fn style_variants(db: &SnapDb, art_info: &str) -> Result<Vec<(String, String)>, DbError> {
    let sql = format!(
        "SELECT s.ArtStyleSuffix, i.Type \
         FROM (SELECT DISTINCT ArtStyleSuffix FROM Civilizations \
               WHERE ArtStyleSuffix IS NOT NULL) s \
         JOIN ArtDefine_UnitInfos i ON i.Type = {} || s.ArtStyleSuffix \
         ORDER BY s.ArtStyleSuffix",
        quote(art_info)
    );
    db.query(&sql, 2)?
        .iter()
        .map(|r| {
            Ok((
                req(r, 0, "ArtStyleSuffix")?,
                req(r, 1, "ArtDefine_UnitInfos.Type")?,
            ))
        })
        .collect()
}

/// All `ArtDefine_StrategicView` rows ordered by tag (351 in the snapshot).
pub fn strategic_view_rows(db: &SnapDb) -> Result<Vec<StrategicViewRow>, DbError> {
    db.query(
        "SELECT StrategicViewType, TileType, Asset FROM ArtDefine_StrategicView \
         ORDER BY StrategicViewType, TileType",
        3,
    )?
    .iter()
    .map(|r| {
        Ok(StrategicViewRow {
            tag: req(r, 0, "StrategicViewType")?,
            tile_type: req(r, 1, "TileType")?,
            asset: req(r, 2, "Asset")?,
        })
    })
    .collect()
}

/// Strategic-view icon file of a tag (`UnitArtInfo` for units).
pub fn strategic_view(db: &SnapDb, tag: &str) -> Result<Option<String>, DbError> {
    let sql = format!(
        "SELECT Asset FROM ArtDefine_StrategicView WHERE StrategicViewType = {} ORDER BY TileType",
        quote(tag)
    );
    let rows = db.query(&sql, 1)?;
    rows.first().map(|r| req(r, 0, "Asset")).transpose()
}

/// Every `ArtDefine_UnitInfos` row, ordered by type.
pub fn all_art_infos(db: &SnapDb) -> Result<Vec<ArtInfoRow>, DbError> {
    db.query(
        "SELECT Type, Formation, DamageStates FROM ArtDefine_UnitInfos ORDER BY Type",
        3,
    )?
    .iter()
    .map(|r| {
        Ok(ArtInfoRow {
            art_info: req(r, 0, "ArtDefine_UnitInfos.Type")?,
            formation: r[1].clone(),
            damage_states: opt_i64(r, 2, "ArtDefine_UnitInfos.DamageStates")?,
        })
    })
    .collect()
}

/// One row of `ArtDefine_UnitMemberInfos`, in table order.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberInfoRow {
    pub member_type: String,
    pub scale: Option<f32>,
    pub z_offset: Option<f32>,
    pub domain: Option<String>,
    /// `Model`, a path to the `.fxsxml` as written in the DB.
    pub model: String,
}

/// Every `ArtDefine_UnitMemberInfos` row (329 in the snapshot), ordered by type.
pub fn all_member_infos(db: &SnapDb) -> Result<Vec<MemberInfoRow>, DbError> {
    db.query(
        "SELECT Type, Scale, ZOffset, Domain, Model FROM ArtDefine_UnitMemberInfos ORDER BY Type",
        5,
    )?
    .iter()
    .map(|r| {
        Ok(MemberInfoRow {
            member_type: req(r, 0, "ArtDefine_UnitMemberInfos.Type")?,
            scale: opt_f32(r, 1, "UnitMemberInfos.Scale")?,
            z_offset: opt_f32(r, 2, "UnitMemberInfos.ZOffset")?,
            domain: r[3].clone(),
            model: req(r, 4, "ArtDefine_UnitMemberInfos.Model")?,
        })
    })
    .collect()
}

/// Art info to its member rows `(member type, NumMembers)` in table order.
pub fn all_art_members(db: &SnapDb) -> Result<BTreeMap<String, Vec<(String, u32)>>, DbError> {
    let mut out: BTreeMap<String, Vec<(String, u32)>> = BTreeMap::new();
    for r in db.query(
        "SELECT UnitInfoType, UnitMemberInfoType, NumMembers FROM ArtDefine_UnitInfoMemberInfos          ORDER BY rowid",
        3,
    )? {
        let art = req(&r, 0, "UnitInfoMemberInfos.UnitInfoType")?;
        let member = req(&r, 1, "UnitInfoMemberInfos.UnitMemberInfoType")?;
        let n = opt_i64(&r, 2, "UnitInfoMemberInfos.NumMembers")?.unwrap_or(1);
        let n = u32::try_from(n)
            .map_err(|_| DbError::data(&art, format!("NumMembers {n}")))?;
        out.entry(art).or_default().push((member, n));
    }
    Ok(out)
}
