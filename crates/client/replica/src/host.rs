//! The game data of the replica core: `HostServices::read_file` over the blobs of the `game-data` set that the
//! client fetched (the set is `tools/asset-pipeline/src/gamedata`).
//!
//! The core asks for files by Civ5 VFS path (`db/gameplay.db`, `db/loc-en_us.db`, `vfs-index.json`, the Lua files
//! of the index). The index (`data:vfs-index`) maps each path to the manifest id of its blob; this module reads
//! it, and keeps the blobs the client has loaded under their ids. A path whose blob is not loaded is `None`, as a
//! missing file is on the server (map scripts and WorldBuilder maps are not part of the client's set: the replica
//! never generates a map).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gamecore::{GameEvent, HostServices};
use serde_json::Value;

/// Manifest ids of the fixed members of the set (`tools/asset-pipeline/src/gamedata`).
pub const GAMEPLAY_DB_ID: &str = "data:gameplay-db";
pub const LOC_DB_ID: &str = "data:loc-en_us";
pub const VFS_INDEX_ID: &str = "data:vfs-index";
/// The load group of the set in the group table.
pub const GROUP_ID: &str = "gc:replica";

/// The manifest ids of the replica's game data, from the bytes of the group table (`tb:groups-b2`): the shell asks
/// for the group `gc:replica` (`GroupScheduler::need`), fetches these blobs, and feeds them to [`GameFiles::put`]
/// by id. The index (`data:vfs-index`) comes first in the list: [`GameFiles::from_index`] needs it before the rest.
pub fn group_members(group_table: &[u8]) -> Result<Vec<String>, GameDataError> {
    let table = asset_format::GroupTable::decode(group_table)
        .map_err(|e| GameDataError::Index(format!("group table: {e}")))?;
    let group = table
        .get(GROUP_ID)
        .ok_or_else(|| GameDataError::Missing(format!("group {GROUP_ID}")))?;
    let mut ids: Vec<String> = group
        .members
        .iter()
        .map(|m| m.as_str().to_owned())
        .collect();
    ids.sort_by_key(|id| id != VFS_INDEX_ID);
    Ok(ids)
}

/// Why the set cannot serve the core.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GameDataError {
    #[error("vfs-index: {0}")]
    Index(String),
    #[error("game data: {0} is not loaded")]
    Missing(String),
}

/// Paths of the core's files to manifest ids, and the loaded bytes by id.
#[derive(Debug, Default)]
pub struct GameFiles {
    /// Lower-case path to id.
    ids: HashMap<String, String>,
    /// The index file as the core reads it.
    index_json: Vec<u8>,
    blobs: HashMap<String, Arc<Vec<u8>>>,
    /// Paths the core asked for that were not there (each once).
    missed: Vec<String>,
    /// Paths served (each once, in the order of the first read).
    served: Vec<String>,
}

impl GameFiles {
    /// From the bytes of `data:vfs-index`.
    pub fn from_index(index_json: Vec<u8>) -> Result<Self, GameDataError> {
        let v: Value =
            serde_json::from_slice(&index_json).map_err(|e| GameDataError::Index(e.to_string()))?;
        let bad = |what: &str| GameDataError::Index(format!("no {what}"));
        let mut ids = HashMap::new();
        let db = v.get("db").ok_or_else(|| bad("db"))?;
        let id_of = |k: &str| -> Result<String, GameDataError> {
            db.get(k)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| bad(k))
        };
        ids.insert("db/gameplay.db".to_owned(), id_of("gameplay")?);
        ids.insert("db/loc-en_us.db".to_owned(), id_of("localization")?);
        ids.insert("vfs-index.json".to_owned(), VFS_INDEX_ID.to_owned());
        for f in v
            .get("files")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("files"))?
        {
            let path = f.get("path").and_then(Value::as_str);
            let id = f.get("id").and_then(Value::as_str);
            if let (Some(p), Some(i)) = (path, id) {
                ids.insert(p.to_ascii_lowercase(), i.to_owned());
            }
        }
        Ok(GameFiles {
            ids,
            index_json,
            blobs: HashMap::new(),
            missed: Vec::new(),
            served: Vec::new(),
        })
    }

    /// Manifest ids the set needs besides the index: both databases and every Lua file of the index.
    pub fn member_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .ids
            .iter()
            .filter(|(p, _)| p.as_str() != "vfs-index.json")
            .map(|(_, id)| id.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// A blob arrived (any id of the set).
    pub fn put(&mut self, id: &str, bytes: Vec<u8>) {
        if id == VFS_INDEX_ID {
            self.index_json = bytes;
        } else {
            self.blobs.insert(id.to_owned(), Arc::new(bytes));
        }
    }

    /// Ids of members not loaded yet, for the loading screen and the check before `gc_create`.
    pub fn missing(&self, required: &[&str]) -> Vec<String> {
        required
            .iter()
            .filter(|id| !self.blobs.contains_key(**id))
            .map(|s| (*s).to_owned())
            .collect()
    }

    /// Number of blobs loaded and their bytes.
    pub fn loaded(&self) -> (usize, u64) {
        (
            self.blobs.len(),
            self.blobs.values().map(|b| b.len() as u64).sum(),
        )
    }

    /// The file the core asks for.
    pub fn read(&mut self, path: &str) -> Option<Vec<u8>> {
        let norm = path.replace('\\', "/");
        if norm.split('/').any(|p| p == "..") {
            return None;
        }
        let key = norm.trim_start_matches("./").to_ascii_lowercase();
        if key == "vfs-index.json" {
            return Some(self.index_json.clone());
        }
        let hit = self
            .ids
            .get(&key)
            .and_then(|id| self.blobs.get(id))
            .map(|b| b.as_ref().clone());
        if hit.is_none() && !self.missed.contains(&key) {
            self.missed.push(key);
        } else if hit.is_some() && !self.served.contains(&key) {
            self.served.push(key);
        }
        hit
    }

    /// Paths the core asked for that the set did not have (each once), for the log.
    pub fn missed(&self) -> &[String] {
        &self.missed
    }

    /// Paths the core read and got, in the order of the first read: what the client's set has to hold.
    pub fn served(&self) -> &[String] {
        &self.served
    }
}

/// `HostServices` over a shared [`GameFiles`]; the shell keeps the other handle to fill it.
#[derive(Clone)]
pub struct GameDataHost {
    files: Arc<Mutex<GameFiles>>,
}

impl GameDataHost {
    pub fn new(files: Arc<Mutex<GameFiles>>) -> Self {
        Self { files }
    }
}

impl HostServices for GameDataHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.files.lock().ok()?.read(path)
    }

    fn game_event(&self, _ev: GameEvent) {
        // The replica's UI hears events through `GameSide::take_events` (WO-1), not through this callback.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> Vec<u8> {
        br#"{"format":1,"db":{"gameplay":"data:gameplay-db","localization":"data:loc-en_us"},
            "files":[{"path":"assets/dlc/X/Lua/A.lua","id":"lua:assets/dlc/X/Lua/A.lua","size":3}]}"#
            .to_vec()
    }

    #[test]
    fn paths_resolve_to_loaded_blobs() {
        let mut f = GameFiles::from_index(index()).unwrap();
        assert_eq!(f.read("db/gameplay.db"), None);
        f.put("data:gameplay-db", vec![1, 2, 3]);
        f.put("lua:assets/dlc/X/Lua/A.lua", b"abc".to_vec());
        assert_eq!(f.read("db/gameplay.db"), Some(vec![1, 2, 3]));
        // Case and separators do not matter, as in the VFS.
        assert_eq!(f.read("Assets\\DLC\\x\\lua\\a.lua"), Some(b"abc".to_vec()));
        assert_eq!(f.read("vfs-index.json"), Some(index()));
        assert_eq!(f.read("db/../vfs-index.json"), None);
        assert_eq!(f.read("assets/none.lua"), None);
        assert_eq!(f.missed(), ["db/gameplay.db", "assets/none.lua"]);
        assert_eq!(
            f.member_ids(),
            [
                "data:gameplay-db",
                "data:loc-en_us",
                "lua:assets/dlc/X/Lua/A.lua"
            ]
        );
        assert_eq!(f.loaded(), (2, 6));
    }

    #[test]
    fn the_group_lists_its_members_with_the_index_first() {
        use asset_format::{Group, GroupTable, LogicalId, PriorityClass, Trigger};
        let mut t = GroupTable::new();
        t.upsert(Group {
            id: GROUP_ID.into(),
            class: PriorityClass::Map,
            trigger: Trigger::Preset("original-ui".into()),
            members: [
                "data:gameplay-db",
                "data:loc-en_us",
                VFS_INDEX_ID,
                "lua:a.lua",
            ]
            .map(LogicalId::new)
            .to_vec(),
            bytes: 0,
            requires: Vec::new(),
        });
        let ids = group_members(&t.encode()).unwrap();
        assert_eq!(ids[0], VFS_INDEX_ID);
        assert_eq!(ids.len(), 4);
        assert!(group_members(&GroupTable::new().encode()).is_err());
        assert!(group_members(b"x").is_err());
    }

    #[test]
    fn a_bad_index_is_an_error() {
        assert!(GameFiles::from_index(b"not json".to_vec()).is_err());
        assert!(GameFiles::from_index(br#"{"files":[]}"#.to_vec()).is_err());
    }
}
