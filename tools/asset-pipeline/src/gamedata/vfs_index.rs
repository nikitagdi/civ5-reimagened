//! `data:vfs-index`: the contract with `gc-runtime` (`rt_vfs`) and `game-worker`
//!. `rt_vfs` reads `vfs-index.json` and opens the Lua files by `path`;
//! the worker fetches the blobs by `id`.
//!
//! `include_roots` is the search order for `include`: by file stem, case-insensitive, the first
//! root that has the stem wins; inside a root files of a folder come before its subfolders and
//! subfolders go in name order. The first root (the map script's own folder) is not searched
//! into subfolders.

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{LogicalId, Manifest};
use serde::{Deserialize, Serialize};

use super::lua_closure::Closure;
use super::{GAMEPLAY_DB_ID, GameDataError, LOC_EN_US_ID, Lock};

pub const INDEX_ID: &str = "data:vfs-index";
/// File name of the index in the exported folder.
pub const INDEX_FILE: &str = "vfs-index.json";
pub const FORMAT: u32 = 1;

/// Asset id of a Lua file: `lua:` + normalised path.
pub fn lua_id(path: &str) -> String {
    format!("lua:{path}")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub gameplay_db_sha256: String,
    pub loc_sha256: String,
    pub lekmod: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbIds {
    pub gameplay: String,
    pub localization: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFile {
    pub path: String,
    pub id: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VfsIndex {
    pub format: u32,
    pub snapshot: Snapshot,
    pub db: DbIds,
    pub include_roots: Vec<String>,
    pub files: Vec<IndexFile>,
    /// Context files in `LoadNewContext` order.
    pub contexts: Vec<String>,
    /// The file whose `LoadNewContext` lines name the contexts (`InGame.lua`).
    pub context_entry: String,
    pub map_scripts: BTreeMap<String, String>,
    pub known_missing: Vec<String>,
    /// WorldBuilder maps of the server package; absent in the b1 index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub civ5_maps: Vec<IndexMap>,
}

/// A `.Civ5Map` of the package: the gamecore opens `path` (`maps/<rel>`) through the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMap {
    pub path: String,
    pub size: u64,
}

impl VfsIndex {
    /// Index of `closure`; `lekmod` is the Lekmod version (`v34.15`).
    pub fn from_closure(closure: &Closure, lock: &Lock, lekmod: &str) -> Self {
        Self {
            format: FORMAT,
            snapshot: Snapshot {
                gameplay_db_sha256: lock.gameplay_db_sha256.clone(),
                loc_sha256: lock.loc_sha256.clone(),
                lekmod: lekmod.to_owned(),
            },
            db: DbIds {
                gameplay: GAMEPLAY_DB_ID.to_owned(),
                localization: LOC_EN_US_ID.to_owned(),
            },
            include_roots: closure.include_roots.clone(),
            files: closure
                .files
                .iter()
                .map(|(path, f)| IndexFile {
                    path: path.clone(),
                    id: lua_id(path),
                    size: f.size,
                })
                .collect(),
            contexts: closure.contexts.clone(),
            context_entry: closure.context_entry.clone(),
            map_scripts: closure.map_scripts.clone(),
            known_missing: closure.known_missing.iter().cloned().collect(),
            civ5_maps: Vec::new(),
        }
    }

    /// Pretty JSON with a trailing newline; the same index always gives the same bytes.
    pub fn to_json(&self) -> Result<Vec<u8>, GameDataError> {
        let mut v = serde_json::to_vec_pretty(self)
            .map_err(|e| GameDataError::Invalid(format!("vfs-index: {e}")))?;
        v.push(b'\n');
        Ok(v)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, GameDataError> {
        serde_json::from_slice(bytes)
            .map_err(|e| GameDataError::Invalid(format!("vfs-index.json: {e}")))
    }

    /// Internal consistency: unique paths, `id == lua:<path>`, every context, the context entry and
    /// every map script is a file of the index, paths are normalised.
    pub fn check(&self) -> Result<(), GameDataError> {
        let bad = |m: String| Err(GameDataError::Invalid(format!("vfs-index: {m}")));
        if self.format != FORMAT {
            return bad(format!("format {} (expected {FORMAT})", self.format));
        }
        let mut paths = BTreeSet::new();
        for f in &self.files {
            if !paths.insert(f.path.as_str()) {
                return bad(format!("duplicate path {}", f.path));
            }
            if f.path != super::lua_closure::normalize_path(&f.path) {
                return bad(format!("path {} is not normalised", f.path));
            }
            if f.id != lua_id(&f.path) {
                return bad(format!("id {} does not match path {}", f.id, f.path));
            }
        }
        let listed = |p: &String| paths.contains(p.as_str());
        for p in self.contexts.iter().chain(self.map_scripts.values()) {
            if !listed(p) {
                return bad(format!("{p} is not in files"));
            }
        }
        let mut maps = BTreeSet::new();
        for m in &self.civ5_maps {
            if !m.path.starts_with("maps/") || !m.path.to_ascii_lowercase().ends_with(".civ5map") {
                return bad(format!("civ5_maps: {} is not maps/<rel>.Civ5Map", m.path));
            }
            if !maps.insert(m.path.as_str()) {
                return bad(format!("civ5_maps: duplicate path {}", m.path));
            }
        }
        if !listed(&self.context_entry) {
            return bad(format!(
                "context entry {} is not in files",
                self.context_entry
            ));
        }
        Ok(())
    }

    /// `check`, and every id (Lua files, both DBs) is in `manifest` with the size the index says.
    pub fn check_against(&self, manifest: &Manifest) -> Result<(), GameDataError> {
        self.check()?;
        let bad = |m: String| Err(GameDataError::Invalid(format!("vfs-index: {m}")));
        for id in [&self.db.gameplay, &self.db.localization] {
            if manifest.variants(&LogicalId::new(id)).is_empty() {
                return bad(format!("{id} is not in the manifest"));
            }
        }
        for f in &self.files {
            let v = manifest.variants(&LogicalId::new(&f.id));
            match v.first() {
                None => return bad(format!("{} is not in the manifest", f.id)),
                Some(v) if v.size != f.size => {
                    return bad(format!(
                        "{} has size {} in the manifest, {} in the index",
                        f.id, v.size, f.size
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
