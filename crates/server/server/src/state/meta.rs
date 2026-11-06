//! What the gateway keeps of its games between runs: `games/<id>/meta.json` per game, `ids.json`
//! with the next game and room numbers, and the check of `gamedata_dir` against `manifest.sha256`
//! The saves themselves belong to the worker; their index is made from file names
//! ([`crate::supervisor::policy::SaveBook::scan`]).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context, bail};
use protocol::{CivPick, GameId, GameSettings, PlayerId, RoomId, SeatControl};
use serde::{Deserialize, Serialize};

use super::files::{self, FileMode};
use super::sha256::Sha256;

/// Format of `meta.json` and `ids.json`.
pub const META_FORMAT: u32 = 1;

/// Where a game stands on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetaStatus {
    /// A worker runs it, or a gateway restart brings it back (`Hibernating` if there is no room for it).
    Running,
    /// On disk without a worker; a login of one of its players or a `JoinGame` wakes it.
    Hibernating,
    /// `GameOver`: only the saves are left.
    Finished,
    /// Too many failures: not restarted; the newest save can be loaded from the lobby as a new game.
    Faulted,
}

/// One seat as stored: who plays it, or whose it was.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaSeat {
    pub player: PlayerId,
    pub civ: CivPick,
    /// The human who sits there; `None`: the AI.
    pub nick: Option<String>,
    /// The nick of the human a kick took this seat from.
    pub kicked: Option<String>,
}

impl MetaSeat {
    pub fn of(player: PlayerId, civ: &CivPick, control: &SeatControl) -> Self {
        let (nick, kicked) = match control {
            SeatControl::Human { nick } => (Some(nick.clone()), None),
            SeatControl::Ai { kicked } => (None, kicked.clone()),
            SeatControl::Closed => (None, None),
        };
        Self {
            player,
            civ: civ.clone(),
            nick,
            kicked,
        }
    }
}

/// `games/<id>/meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameMeta {
    pub id: GameId,
    pub name: String,
    pub status: MetaStatus,
    pub settings: GameSettings,
    pub host: String,
    pub seats: Vec<MetaSeat>,
    /// The seed the game started with (a restart loads with it).
    pub seed: u64,
    /// The core build that wrote the saves; only that build reads them (GS-D12).
    pub core_build: String,
    /// Calendar time, Unix milliseconds.
    pub created_ms: u64,
    pub finished_ms: Option<u64>,
    /// The turn the game was at when it was last written.
    #[serde(default)]
    pub turn: i32,
    /// The frozen mod set (`mods.lock` text) of a game with mods; absent for a game without. `settings.mods` then
    /// lists the same mods pinned to their versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mods_lock: Option<String>,
}

impl GameMeta {
    pub fn path(game_dir: &Path) -> PathBuf {
        game_dir.join("meta.json")
    }

    pub fn write(&self, game_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(game_dir)
            .with_context(|| format!("cannot create {}", game_dir.display()))?;
        files::write_versioned(&Self::path(game_dir), META_FORMAT, self, FileMode::Normal)
    }

    /// [`write`](Self::write) with the failure logged (a full disk or a permission problem must not pass
    /// unseen: the disk then says another status than the memory). `false`: not written.
    pub fn write_logged(&self, game_dir: &Path) -> bool {
        match self.write(game_dir) {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(game = self.id, status = ?self.status, error = %format!("{e:#}"), "the meta of a game cannot be written, the disk keeps the old status");
                false
            }
        }
    }

    /// The humans and their seats.
    pub fn humans(&self) -> Vec<(PlayerId, String)> {
        self.seats
            .iter()
            .filter_map(|s| s.nick.clone().map(|n| (s.player, n)))
            .collect()
    }
}

/// Reads the meta of every game under `<state_dir>/games`, by game number. A game whose meta is
/// damaged is skipped with a warning; one written by a newer build stops the start (its text says so).
pub fn read_all(state_dir: &Path) -> anyhow::Result<Vec<GameMeta>> {
    let games = state_dir.join("games");
    let Ok(read) = std::fs::read_dir(&games) else {
        return Ok(Vec::new());
    };
    let mut by_id: BTreeMap<GameId, GameMeta> = BTreeMap::new();
    for item in read.flatten() {
        let dir = item.path();
        let path = GameMeta::path(&dir);
        if !path.is_file() {
            continue;
        }
        match files::read_versioned::<GameMeta>(&path, META_FORMAT) {
            Ok(Some(meta)) => {
                by_id.insert(meta.id, meta);
            }
            Ok(None) => {}
            Err(e) if format!("{e:#}").contains("newer civ5-server") => return Err(e),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %format!("{e:#}"), "a game meta cannot be read, skipping it")
            }
        }
    }
    Ok(by_id.into_values().collect())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct IdsData {
    next_game: GameId,
    next_room: RoomId,
}

/// The next game and room numbers; kept in `ids.json` so that a number is never used twice.
#[derive(Debug)]
pub struct IdStore {
    path: Option<PathBuf>,
    data: Mutex<IdsData>,
}

impl IdStore {
    /// Numbers in memory only (no `state_dir`).
    pub fn in_memory() -> Self {
        Self {
            path: None,
            data: Mutex::new(IdsData {
                next_game: 1,
                next_room: 1,
            }),
        }
    }

    /// Reads `ids.json` (missing: numbers start at 1).
    pub fn open(path: PathBuf) -> anyhow::Result<Self> {
        let data = files::read_versioned::<IdsData>(&path, META_FORMAT)?.unwrap_or(IdsData {
            next_game: 1,
            next_room: 1,
        });
        Ok(Self {
            path: Some(path),
            data: Mutex::new(data),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, IdsData> {
        self.data.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn save(&self, data: &IdsData) {
        if let Some(path) = &self.path
            && let Err(e) = files::write_versioned(path, META_FORMAT, data, FileMode::Normal)
        {
            tracing::error!(error = %format!("{e:#}"), "cannot write ids.json");
        }
    }

    pub fn next_game(&self) -> GameId {
        let mut d = self.lock();
        let id = d.next_game;
        d.next_game += 1;
        self.save(&d);
        id
    }

    pub fn next_room(&self) -> RoomId {
        let mut d = self.lock();
        let id = d.next_room;
        d.next_room += 1;
        self.save(&d);
        id
    }

    /// A game number found on disk is never handed out again, whatever `ids.json` said.
    pub fn seen_game(&self, id: GameId) {
        let mut d = self.lock();
        if d.next_game <= id {
            d.next_game = id + 1;
            self.save(&d);
        }
    }
}

/// Checks `gamedata_dir` against its `manifest.sha256` (`core = real`): one line per file,
/// `<sha256 hex>  <path relative to the directory>` (the `sha256sum` format). A missing file or a sum
/// that differs is an error that names every such file.
pub fn verify_gamedata(dir: &Path) -> anyhow::Result<()> {
    let manifest = dir.join("manifest.sha256");
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("cannot read {}", manifest.display()))?;
    let mut bad: Vec<String> = Vec::new();
    let mut checked = 0usize;
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((sum, rel)) = line.split_once(char::is_whitespace) else {
            bail!(
                "{}: line {}: expected `<sha256>  <path>`",
                manifest.display(),
                n + 1
            );
        };
        let rel = rel.trim_start().trim_start_matches('*');
        if sum.len() != 64 || !sum.bytes().all(|b| b.is_ascii_hexdigit()) || rel.is_empty() {
            bail!(
                "{}: line {}: expected `<sha256>  <path>`",
                manifest.display(),
                n + 1
            );
        }
        checked += 1;
        match hash_file(&dir.join(rel)) {
            Ok(have) if have.eq_ignore_ascii_case(sum) => {}
            Ok(_) => bad.push(format!("{rel} (changed)")),
            Err(_) => bad.push(format!("{rel} (missing)")),
        }
    }
    if checked == 0 {
        bail!("{} lists no files", manifest.display());
    }
    if !bad.is_empty() {
        bail!(
            "gamedata_dir {} does not match its manifest.sha256: {}",
            dir.display(),
            bad.join(", ")
        );
    }
    Ok(())
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize_hex())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::sha256::sha256_hex;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-meta-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_failed_meta_write_is_reported_not_swallowed() {
        let dir = scratch("write-logged");
        let m = meta(3, MetaStatus::Running);
        assert!(m.write_logged(&dir.join("games").join("3")));
        // A file where the directory should be: the write cannot succeed.
        let blocker = dir.join("blocked");
        std::fs::write(&blocker, b"x").unwrap();
        assert!(!m.write_logged(&blocker));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn meta(id: GameId, status: MetaStatus) -> GameMeta {
        GameMeta {
            id,
            name: format!("game {id}"),
            status,
            settings: GameSettings {
                name: "g".into(),
                world_size: "DUEL".into(),
                map_script: "m".into(),
                map_options: vec![],
                speed: "s".into(),
                start_era: "e".into(),
                handicap: "h".into(),
                minor_civs: 0,
                turn_mode: protocol::TurnMode::Hybrid,
                timer: protocol::TimerMode::Fixed { seconds: 30 },
                game_options: vec![],
                seed: Some(5),
                map_seed: None,
                auto_choices: protocol::AutoChoices::default(),
                diplo_auto_decline: true,
                mods: Vec::new(),
            },
            host: "alice".into(),
            seats: vec![
                MetaSeat::of(
                    0,
                    &CivPick::Random,
                    &SeatControl::Human {
                        nick: "alice".into(),
                    },
                ),
                MetaSeat::of(
                    1,
                    &CivPick::Civ("CIVILIZATION_A".into()),
                    &SeatControl::Ai {
                        kicked: Some("bob".into()),
                    },
                ),
            ],
            seed: 9,
            core_build: "core-a".into(),
            created_ms: 1,
            finished_ms: None,
            turn: 0,
            mods_lock: None,
        }
    }

    #[test]
    fn meta_roundtrips_and_is_read_by_game_number() {
        let dir = scratch("roundtrip");
        for id in [10, 2] {
            meta(id, MetaStatus::Running)
                .write(&dir.join("games").join(id.to_string()))
                .unwrap();
        }
        let all = read_all(&dir).unwrap();
        assert_eq!(all.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 10]);
        assert_eq!(all[0], meta(2, MetaStatus::Running));
        assert_eq!(all[0].humans(), [(0, "alice".to_owned())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The frozen mod set rides in `meta.json`; a game without mods writes exactly the file it always did, and a
    /// meta from before the mod system reads as a game without mods.
    #[test]
    fn the_frozen_mod_set_is_kept_and_a_game_without_mods_writes_no_trace() {
        let dir = scratch("modslock");
        let g = dir.join("games").join("3");
        let mut m = meta(3, MetaStatus::Running);
        m.write(&g).unwrap();
        let text = std::fs::read_to_string(GameMeta::path(&g)).unwrap();
        assert!(!text.contains("mods_lock"), "{text}");
        assert_eq!(read_all(&dir).unwrap()[0].mods_lock, None);

        m.mods_lock = Some(
            "# mods.lock
version = 1
"
            .into(),
        );
        m.write(&g).unwrap();
        assert_eq!(read_all(&dir).unwrap()[0], m);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_meta_is_skipped_and_a_newer_one_stops_the_start() {
        let dir = scratch("damaged");
        let g1 = dir.join("games").join("1");
        let g2 = dir.join("games").join("2");
        meta(1, MetaStatus::Hibernating).write(&g1).unwrap();
        std::fs::create_dir_all(&g2).unwrap();
        std::fs::write(GameMeta::path(&g2), b"{broken").unwrap();
        assert_eq!(read_all(&dir).unwrap().len(), 1);
        files::write_versioned(
            &GameMeta::path(&g2),
            META_FORMAT + 1,
            &meta(2, MetaStatus::Running),
            FileMode::Normal,
        )
        .unwrap();
        let err = read_all(&dir).unwrap_err().to_string();
        assert!(err.contains("newer civ5-server"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ids_are_kept_over_a_restart_and_never_reused() {
        let dir = scratch("ids");
        let path = dir.join("ids.json");
        let ids = IdStore::open(path.clone()).unwrap();
        assert_eq!(
            (ids.next_game(), ids.next_game(), ids.next_room()),
            (1, 2, 1)
        );
        drop(ids);
        let ids = IdStore::open(path).unwrap();
        assert_eq!((ids.next_game(), ids.next_room()), (3, 2));
        ids.seen_game(10);
        assert_eq!(ids.next_game(), 11);
        ids.seen_game(4);
        assert_eq!(ids.next_game(), 12);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_manifest_names_every_file_that_differs() {
        let dir = scratch("manifest");
        std::fs::write(dir.join("a.txt"), b"alpha").unwrap();
        std::fs::create_dir_all(dir.join("db")).unwrap();
        std::fs::write(dir.join("db").join("b.db"), b"beta").unwrap();
        let line = |name: &str, body: &[u8]| format!("{}  {name}\n", sha256_hex(body));
        let ok = format!("{}{}", line("a.txt", b"alpha"), line("db/b.db", b"beta"));
        std::fs::write(dir.join("manifest.sha256"), &ok).unwrap();
        verify_gamedata(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), b"changed").unwrap();
        std::fs::remove_file(dir.join("db").join("b.db")).unwrap();
        let err = verify_gamedata(&dir).unwrap_err().to_string();
        assert!(
            err.contains("a.txt (changed)") && err.contains("db/b.db (missing)"),
            "{err}"
        );
        std::fs::write(dir.join("manifest.sha256"), "nonsense\n").unwrap();
        assert!(verify_gamedata(&dir).is_err());
        std::fs::remove_file(dir.join("manifest.sha256")).unwrap();
        assert!(verify_gamedata(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
