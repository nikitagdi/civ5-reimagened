//! Civilizations, player colors and their `Colors` rows.

use std::collections::BTreeMap;

use asset_format::PlayerColor;

use super::{DbError, SnapDb, req};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CivilizationRow {
    pub civ_type: String,
    pub default_player_color: Option<String>,
    /// `_EURO`, `_AFRI`, `_AMER`, `_ASIA`, `_MED`; NULL for two rows in the snapshot.
    pub art_style_suffix: Option<String>,
    /// `EUROPEAN`, `AFRICAN`, ... (city culture).
    pub art_style_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerColorRow {
    /// `PLAYERCOLOR_*`.
    pub color_type: String,
    pub color: PlayerColor,
}

/// All civilizations ordered by type.
pub fn civilizations(db: &SnapDb) -> Result<Vec<CivilizationRow>, DbError> {
    db.query(
        "SELECT Type, DefaultPlayerColor, ArtStyleSuffix, ArtStylePrefix \
         FROM Civilizations ORDER BY Type",
        4,
    )?
    .iter()
    .map(|r| {
        Ok(CivilizationRow {
            civ_type: req(r, 0, "Civilizations.Type")?,
            default_player_color: r[1].clone(),
            art_style_suffix: r[2].clone(),
            art_style_prefix: r[3].clone(),
        })
    })
    .collect()
}

/// Float channel 0..1 to a byte, rounded and clamped.
fn to_u8(v: f32) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// All `PlayerColors` with primary, secondary and text resolved through `Colors(Red, Green,
/// Blue, Alpha)`, ordered by type. A color name missing from `Colors` is an error.
pub fn player_colors(db: &SnapDb) -> Result<Vec<PlayerColorRow>, DbError> {
    let mut colors = BTreeMap::new();
    for r in db.query("SELECT Type, Red, Green, Blue, Alpha FROM Colors", 5)? {
        let name = req(&r, 0, "Colors.Type")?;
        let mut rgba = [0u8; 4];
        for (i, slot) in rgba.iter_mut().enumerate() {
            let t = req(&r, i + 1, &name)?;
            let v: f32 = t
                .parse()
                .map_err(|_| DbError::data(&name, format!("{t:?} is not a number")))?;
            *slot = to_u8(v);
        }
        colors.insert(name, rgba);
    }
    let lookup = |owner: &str, name: String| {
        colors
            .get(&name)
            .copied()
            .ok_or_else(|| DbError::data(owner, format!("color {name} is not in Colors")))
    };
    db.query(
        "SELECT Type, PrimaryColor, SecondaryColor, TextColor FROM PlayerColors ORDER BY Type",
        4,
    )?
    .iter()
    .map(|r| {
        let color_type = req(r, 0, "PlayerColors.Type")?;
        let color = PlayerColor {
            primary: lookup(&color_type, req(r, 1, "PrimaryColor")?)?,
            secondary: lookup(&color_type, req(r, 2, "SecondaryColor")?)?,
            text: lookup(&color_type, req(r, 3, "TextColor")?)?,
        };
        Ok(PlayerColorRow { color_type, color })
    })
    .collect()
}
