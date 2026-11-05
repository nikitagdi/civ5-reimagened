//! Typed readers over the game DB snapshot: units of the B1 eras,
//! the unit art chain, strategic-view icons, civilizations and player colors. Each reader runs one
//! SQL query and returns plain structs; mapping them to the art table is the caller's job.

mod art;
mod colors;
mod sqlite;
mod units;
mod world;

pub use art::{
    ArtInfoRow, MemberInfoRow, MemberRow, StrategicViewRow, all_art_infos, all_art_members,
    all_member_infos, art_info, members, strategic_view, strategic_view_rows, style_suffixes,
    style_variants,
};
pub use colors::{CivilizationRow, PlayerColorRow, civilizations, player_colors};
pub use sqlite::{DbError, Row, SnapDb, quote};
pub use units::{B1_ERAS, UnitRow, all_units, b1_art_infos, b1_units};
pub use world::{
    BuildingRow, CivArtRow, LandmarkRow, all_buildings, all_civ_art, all_landmarks, eras, routes,
    unit_overrides,
};

use std::path::Path;

use crate::gamedata::GAMEPLAY_DB_FILE;

/// Opens `Civ5DebugDatabase.db` inside a snapshot directory, read-only.
pub fn open_snapshot(dir: &Path) -> Result<SnapDb, DbError> {
    SnapDb::open_read_only(&dir.join(GAMEPLAY_DB_FILE))
}

/// Text cell, NULL is an error naming the column.
pub(crate) fn req(row: &Row, i: usize, what: &str) -> Result<String, DbError> {
    row[i]
        .clone()
        .ok_or_else(|| DbError::data(what, format!("unexpected NULL in column {i}")))
}

/// Nullable float cell.
pub(crate) fn opt_f32(row: &Row, i: usize, what: &str) -> Result<Option<f32>, DbError> {
    row[i]
        .as_deref()
        .map(|t| {
            t.parse::<f32>()
                .map_err(|_| DbError::data(what, format!("{t:?} is not a number")))
        })
        .transpose()
}

/// Nullable integer cell.
pub(crate) fn opt_i64(row: &Row, i: usize, what: &str) -> Result<Option<i64>, DbError> {
    row[i]
        .as_deref()
        .map(|t| {
            t.parse::<i64>()
                .map_err(|_| DbError::data(what, format!("{t:?} is not an integer")))
        })
        .transpose()
}
