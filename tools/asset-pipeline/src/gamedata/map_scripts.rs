//! The map scripts of the server package: every row of the game DB table
//! `MapScripts` (48 with Lekmod v34.15), and the entry file that map-only games need.

use std::collections::BTreeMap;
use std::path::Path;

use super::GameDataError;
use super::db::Db;
use super::lua_closure::{MAP_SCRIPTS, stem_of};

/// `WorldBuilderRandomItems.lua`: `CvWorldBuilderMapLoader` loads it by bare name when a game
/// starts from a `.Civ5Map` without its scenario data; its include closure comes with it.
pub const WORLD_BUILDER_ENTRY: &str = "WorldBuilderRandomItems";

/// Name of a map script in `vfs-index.map_scripts`: the lower-case file stem. The script of the b1
/// pack keeps its b1 name (`lekmap_pangaea`).
pub fn key_of(path: &str) -> String {
    let normalised = path.replace('\\', "/");
    for (name, b1_path) in MAP_SCRIPTS {
        if normalised.eq_ignore_ascii_case(b1_path) {
            return (*name).to_owned();
        }
    }
    stem_of(&normalised)
}

/// (name, path from the install root with `/`) of every `MapScripts.FileName` in the gameplay DB,
/// in path order. Two scripts with one name are an error.
pub fn read(gameplay_db: &Path) -> Result<Vec<(String, String)>, GameDataError> {
    let db = Db::open_read_only(gameplay_db)?;
    let joined = db
        .scalar_text(
            "SELECT group_concat(FileName, char(10)) \
             FROM (SELECT FileName FROM MapScripts ORDER BY FileName)",
        )?
        .unwrap_or_default();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for line in joined.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = line.replace('\\', "/");
        let key = key_of(&path);
        if let Some(prev) = seen.insert(key.clone(), path.clone()) {
            return Err(GameDataError::Invalid(format!(
                "map scripts {prev} and {path} have the same name {key:?}"
            )));
        }
    }
    let mut out: Vec<(String, String)> = seen.into_iter().collect();
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_lower_case_stems_and_the_b1_script_keeps_its_name() {
        assert_eq!(key_of("Assets\\Maps\\Four_Corners.lua"), "four_corners");
        assert_eq!(
            key_of("Assets/DLC/Expansion/Maps/Amazon_XP.lua"),
            "amazon_xp"
        );
        assert_eq!(
            key_of("Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua"),
            "lekmap_pangaea"
        );
        assert_eq!(
            key_of("Assets/Maps/Lekmap v5.2/LekmapPangaeaRectangularv4.lua"),
            "lekmappangaearectangularv4"
        );
    }
}
