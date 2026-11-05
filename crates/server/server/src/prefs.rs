//! The per-account store (protocol v12, `AccountData`): the settings and toggles of a player and the settings of the
//! room they hosted last, kept on the server by nick.
//!
//! `prefs.json` (format 1) in the `state_dir` holds one record per lowercase nick. Writes are atomic
//! ([`crate::state::files`]). The store is bounded: [`protocol::MAX_PREFS`] entries and [`protocol::MAX_PREFS_BYTES`]
//! bytes per account, [`MAX_ACCOUNTS`] accounts, and a last room is kept only if its encoded form fits
//! [`MAX_ROOM_BYTES`]. Everything a client sends is validated by [`protocol::apply_save`] before it lands here.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use protocol::{
    ACCOUNT_VERSION, AccountData, AccountError, AccountSave, Codec, GameSettings, Postcard,
    apply_save,
};
use serde::{Deserialize, Serialize};

use crate::identity::nick_key;
use crate::state::files::{self, FileMode};

// SIMPLIFIED(SIMP-exe-1088): the bounds below; the original game has no account store.
/// `format` of `prefs.json`.
pub const PREFS_FORMAT: u32 = 1;

/// Records kept; a new account beyond it gets no store (its client keeps its settings locally).
pub const MAX_ACCOUNTS: usize = 5_000;

/// Largest postcard size of a stored last room.
pub const MAX_ROOM_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Record {
    /// `ACCOUNT_VERSION` the record was written with.
    version: u16,
    prefs: Vec<(String, String)>,
    last_room: Option<GameSettings>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    records: BTreeMap<String, Record>,
}

#[derive(Debug)]
pub struct Prefs {
    path: Option<PathBuf>,
    data: Mutex<File>,
}

impl Prefs {
    /// Nothing is written (no `state_dir`).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            data: Mutex::new(File::default()),
        }
    }

    /// Reads `prefs.json` (missing: empty). A newer `format` is a startup error.
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let data = files::read_versioned::<File>(&path, PREFS_FORMAT)?.unwrap_or_default();
        Ok(Self {
            path: Some(path),
            data: Mutex::new(data),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self, data: &File) {
        if let Some(path) = &self.path
            && let Err(e) = files::write_versioned(path, PREFS_FORMAT, data, FileMode::Normal)
        {
            tracing::error!(error = %format!("{e:#}"), "cannot write prefs.json");
        }
    }

    /// The store of `nick` (empty for an account that has none yet).
    pub fn get(&self, nick: &str) -> AccountData {
        let d = self.lock();
        match d.records.get(&nick_key(nick)) {
            Some(r) => AccountData {
                version: ACCOUNT_VERSION,
                prefs: r.prefs.clone(),
                last_room: r.last_room.clone(),
            },
            None => AccountData {
                version: ACCOUNT_VERSION,
                ..AccountData::default()
            },
        }
    }

    /// Applies a client's change. On an error nothing changes.
    pub fn save(&self, nick: &str, save: &AccountSave) -> Result<(), AccountError> {
        let key = nick_key(nick);
        let mut d = self.lock();
        if !d.records.contains_key(&key) && d.records.len() >= MAX_ACCOUNTS {
            return Err(AccountError::TooBig);
        }
        let mut prefs = d
            .records
            .get(&key)
            .map(|r| r.prefs.clone())
            .unwrap_or_default();
        apply_save(&mut prefs, save)?;
        let rec = d.records.entry(key).or_default();
        rec.version = ACCOUNT_VERSION;
        rec.prefs = prefs;
        self.write(&d);
        Ok(())
    }

    /// The settings of the room `nick` starts a game from; the defaults of their next room. `false`: not kept.
    pub fn set_last_room(&self, nick: &str, settings: &GameSettings) -> bool {
        if Postcard
            .encode(settings)
            .map_or(true, |b| b.len() > MAX_ROOM_BYTES)
        {
            return false;
        }
        let key = nick_key(nick);
        let mut d = self.lock();
        if !d.records.contains_key(&key) && d.records.len() >= MAX_ACCOUNTS {
            return false;
        }
        let rec = d.records.entry(key).or_default();
        rec.version = ACCOUNT_VERSION;
        // The seeds are of that one game; the next room draws its own.
        let mut s = settings.clone();
        s.seed = None;
        s.map_seed = None;
        if rec.last_room.as_ref() == Some(&s) {
            return true;
        }
        rec.last_room = Some(s);
        self.write(&d);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{AutoChoices, TimerMode, TurnMode};

    fn settings(name: &str) -> GameSettings {
        GameSettings {
            name: name.into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "x.lua".into(),
            map_options: vec![],
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_PRINCE".into(),
            minor_civs: 2,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: Some(7),
            map_seed: Some(8),
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        }
    }

    fn put(k: &str, v: &str) -> AccountSave {
        AccountSave {
            set: vec![(k.into(), v.into())],
            remove: vec![],
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-prefs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_unknown_account_has_an_empty_store() {
        let p = Prefs::in_memory();
        let a = p.get("nobody");
        assert_eq!(a.version, ACCOUNT_VERSION);
        assert!(a.prefs.is_empty() && a.last_room.is_none());
    }

    #[test]
    fn the_store_is_by_nick_without_case_and_survives_a_restart() {
        let dir = scratch("restart");
        let path = dir.join("prefs.json");
        {
            let p = Prefs::open(path.clone()).unwrap();
            p.save("Alice", &put("GridOn", "0")).unwrap();
            p.save("alice", &put("Volume.0", "0.5")).unwrap();
            assert!(p.set_last_room("ALICE", &settings("r")));
        }
        let p = Prefs::open(path).unwrap();
        let a = p.get("aLiCe");
        assert_eq!(
            a.prefs,
            [
                ("GridOn".to_string(), "0".to_string()),
                ("Volume.0".to_string(), "0.5".to_string())
            ]
        );
        let room = a.last_room.unwrap();
        assert_eq!(room.name, "r");
        assert_eq!((room.seed, room.map_seed), (None, None));
        assert!(p.get("bob").prefs.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_change_is_refused_and_changes_nothing() {
        let p = Prefs::in_memory();
        p.save("a", &put("k", "1")).unwrap();
        assert!(p.save("a", &put("bad key", "1")).is_err());
        assert!(p.save("a", &put("k", &"v".repeat(300))).is_err());
        assert_eq!(p.get("a").prefs, [("k".to_string(), "1".to_string())]);
    }

    #[test]
    fn a_huge_room_is_not_kept() {
        let p = Prefs::in_memory();
        let mut s = settings("big");
        s.game_options = (0..2000).map(|i| (format!("OPTION_{i}"), i)).collect();
        assert!(!p.set_last_room("a", &s));
        assert!(p.get("a").last_room.is_none());
    }

    #[test]
    fn a_newer_file_is_a_startup_error() {
        let dir = scratch("newer");
        let path = dir.join("prefs.json");
        files::write_versioned(&path, PREFS_FORMAT + 1, &File::default(), FileMode::Normal)
            .unwrap();
        assert!(Prefs::open(path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
