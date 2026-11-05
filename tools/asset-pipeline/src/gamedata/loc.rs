//! en_US cut of `Localization-Merged.db`.
//! SIMPLIFIED(SIMP-017): only en_US is cut; the original ships every language.

use std::path::Path;

use super::GameDataError;
use super::db::{Db, read_only_uri_literal};

/// Name of the table in the cut file.
pub const LOC_TABLE: &str = "Language_en_US";

/// Writes a new SQLite file at `dest` holding `Language_en_US(Tag, Text)` with the en_US rows of
/// `LocalizedText` from `source` (the merged DB keeps all languages in that one table; the
/// `Language_en_US` object there is a view). Rows go in ordered by Tag; `Text` stays NULL where
/// the source has NULL. Returns the row count. An existing `dest` is replaced.
pub fn cut_en_us(source: &Path, dest: &Path) -> Result<u64, GameDataError> {
    if dest.exists() {
        std::fs::remove_file(dest).map_err(|e| GameDataError::Io {
            path: dest.display().to_string(),
            source: e,
        })?;
    }
    let db = Db::create(dest)?;
    let uri = read_only_uri_literal(source)?;
    db.exec(&format!("ATTACH DATABASE {uri} AS src"))?;
    db.exec(&format!(
        "CREATE TABLE {LOC_TABLE}(Tag TEXT NOT NULL PRIMARY KEY, Text TEXT) WITHOUT ROWID;
         INSERT INTO {LOC_TABLE}(Tag, Text)
             SELECT Tag, Text FROM src.LocalizedText
             WHERE Language = 'en_US' AND Tag IS NOT NULL ORDER BY Tag;
         DETACH DATABASE src;"
    ))?;
    let n = db.scalar_i64(&format!("SELECT count(*) FROM {LOC_TABLE}"))?;
    db.exec("VACUUM")?;
    u64::try_from(n).map_err(|_| GameDataError::Invalid("negative row count".into()))
}
