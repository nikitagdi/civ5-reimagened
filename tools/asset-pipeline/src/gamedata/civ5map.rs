//! The WorldBuilder maps of the install (`*.Civ5Map`), for the `maps/` folder of the server
//! package and the `civ5_maps` section of the index (gc-abi-v2 4.15).
//!
//! The gamecore reads a map as the game-data path `maps/<rel>`, `<rel>` being the path from the
//! install root with its own spelling (`Assets/Maps/Earth_Duel.Civ5Map`).

use std::fs;
use std::path::{Path, PathBuf};

use super::GameDataError;

/// Folder of the maps in the package.
pub const MAPS_DIR: &str = "maps";

/// A map file of the install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    /// Path from the install root, `/` separators, as spelled on disk.
    pub rel: String,
    pub abs: PathBuf,
    pub size: u64,
}

impl MapFile {
    /// The game-data path the gamecore opens: `maps/<rel>`.
    pub fn data_path(&self) -> String {
        format!("{MAPS_DIR}/{}", self.rel)
    }
}

fn io_err(path: &Path, source: std::io::Error) -> GameDataError {
    GameDataError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Every `*.Civ5Map` under `game` (the extension in any case), sorted by `rel`.
pub fn find(game: &Path) -> Result<Vec<MapFile>, GameDataError> {
    let mut out = Vec::new();
    walk(game, "", &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<MapFile>) -> Result<(), GameDataError> {
    for e in fs::read_dir(dir).map_err(|e| io_err(dir, e))? {
        let e = e.map_err(|e| io_err(dir, e))?;
        let name = e.file_name().to_string_lossy().into_owned();
        let child_rel = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let ty = e.file_type().map_err(|err| io_err(&e.path(), err))?;
        if ty.is_dir() {
            walk(&e.path(), &child_rel, out)?;
        } else if ty.is_file()
            && Path::new(&name)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("civ5map"))
        {
            let size = e.metadata().map_err(|err| io_err(&e.path(), err))?.len();
            out.push(MapFile {
                rel: child_rel,
                abs: e.path(),
                size,
            });
        }
    }
    Ok(())
}
