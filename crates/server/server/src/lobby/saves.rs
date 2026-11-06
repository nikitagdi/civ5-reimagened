//! The saves the lobby offers: an index, `SaveId`s and the room a `LoadSave` makes.
//!
//! The spec builds the index from the `C5SV v3` headers on disk (`game_host::saves`, H7). That reader is
//! not there yet, so this index is fed by the games themselves: every `WorkerMsg::Saved` becomes a
//! [`SaveEntry`] with the settings and the seats of the game at that moment ([`crate::game`] sends it as
//! `LobbyMsg::SaveWritten`). A gateway restart starts with an empty index until the disk scan of H7
//! and the persistence of G7 (N3.35) exist; the files themselves are not touched here.

use protocol::{
    CivPick, GameId, GameSettings, LobbyChoices, Occupant, PlayerId, RoomSlot, SaveId, SaveKind,
    SaveSummary,
};

use super::validate;

/// Entries the list shows at most.
pub const MAX_LISTED: usize = 200;

/// One seat of a saved game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSeat {
    pub player: PlayerId,
    pub civ: CivPick,
    pub civ_name: String,
    /// The human who sat there; `None`: an AI player.
    pub nick: Option<String>,
}

/// A save of a game as the lobby knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveEntry {
    pub id: SaveId,
    pub game: GameId,
    pub seq: u64,
    pub kind: SaveKind,
    /// Path relative to the directory of its game (`ring/…`, `keep/…`).
    pub file: String,
    pub name: String,
    pub turn: i32,
    pub saved_at_ms: u64,
    /// Core build id with the flags; only saves of the running build can be loaded (GS-D12).
    pub build_id: String,
    pub settings: GameSettings,
    pub seats: Vec<SaveSeat>,
    /// The nick that hosted the game at save time (empty: unknown, the first human seat stands in).
    pub host: String,
}

impl SaveEntry {
    fn summary(&self) -> SaveSummary {
        SaveSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            turn: self.turn,
            saved_at_ms: self.saved_at_ms,
            seats: self
                .seats
                .iter()
                .map(|s| (s.player, s.civ_name.clone(), s.nick.clone()))
                .collect(),
            game: self.game,
            host: if self.host.is_empty() {
                self.seats
                    .iter()
                    .find_map(|s| s.nick.clone())
                    .unwrap_or_default()
            } else {
                self.host.clone()
            },
        }
    }
}

/// `SaveId = "{game}-{seq:08}"`.
pub fn save_id(game: GameId, seq: u64) -> SaveId {
    format!("{game}-{seq:08}")
}

/// Parses a `SaveId` strictly: a canonical decimal game number, `-`, eight digits.
pub fn parse_save_id(id: &str) -> Option<(GameId, u64)> {
    let (game, seq) = id.rsplit_once('-')?;
    if seq.len() != 8 || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let game_no: GameId = game.parse().ok()?;
    // No signs, no leading zeros: one spelling per save.
    if game_no.to_string() != game {
        return None;
    }
    Some((game_no, seq.parse().ok()?))
}

/// The saves, newest first.
#[derive(Debug, Default)]
pub struct SaveIndex {
    entries: Vec<SaveEntry>,
}

impl SaveIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a save. Per game the index keeps every `keep/` file and the newest `ring/` file.
    pub fn insert(&mut self, entry: SaveEntry) {
        self.entries.retain(|e| e.id != entry.id);
        if entry.kind == SaveKind::Ring {
            self.entries
                .retain(|e| !(e.game == entry.game && e.kind == SaveKind::Ring));
        }
        self.entries.push(entry);
        self.entries
            .sort_by_key(|e| std::cmp::Reverse((e.saved_at_ms, e.game, e.seq)));
    }

    /// Forgets every save of `game` (its files are deleted, admin `purge`); how many were listed.
    pub fn remove_game(&mut self, game: GameId) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.game != game);
        before - self.entries.len()
    }

    pub fn get(&self, id: &str) -> Option<&SaveEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Entries of the running core build, newest first, at most [`MAX_LISTED`].
    pub fn list(&self, build: Option<&str>) -> Vec<SaveSummary> {
        self.entries
            .iter()
            .filter(|e| usable(e, build))
            .take(MAX_LISTED)
            .map(SaveEntry::summary)
            .collect()
    }

    /// The save of `id` if it can be loaded by the running core build.
    pub fn loadable(&self, id: &str, build: Option<&str>) -> Option<&SaveEntry> {
        self.get(id).filter(|e| usable(e, build))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Only the build that wrote a save reads it (`GC_ERR_BAD_DATA`, GS-D12). Unknown running build: all.
fn usable(e: &SaveEntry, build: Option<&str>) -> bool {
    build.is_none_or(|b| b == e.build_id)
}

/// Where a room that grew from a save starts its game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveRef {
    pub id: SaveId,
    pub game: GameId,
    pub seq: u64,
    pub file: String,
    pub turn: i32,
    /// Players of the save: only their slots exist in the room.
    pub players: Vec<PlayerId>,
}

/// The slots of a room made from `entry`: one per player of the save, humans'
/// slots `Open` and reserved for their nicks, AI players as `Ai`; slots the save does not have are
/// closed and stay closed. The count follows the map size of the save.
pub fn room_slots(entry: &SaveEntry, choices: &LobbyChoices) -> Result<Vec<RoomSlot>, String> {
    let size = validate::world_size(choices, &entry.settings.world_size)
        .ok_or_else(|| "the map size of the save is unknown to this server".to_owned())?;
    let count = usize::from(size.max_players);
    let mut slots: Vec<RoomSlot> = (0..size.max_players)
        .map(|i| RoomSlot {
            index: i,
            occupant: Occupant::Closed,
            civ: CivPick::Random,
            reserved_for: None,
        })
        .collect();
    for s in &entry.seats {
        let i = usize::try_from(s.player)
            .ok()
            .filter(|i| *i < count)
            .ok_or_else(|| "the save does not fit its map size".to_owned())?;
        slots[i].civ = s.civ.clone();
        match &s.nick {
            Some(nick) => {
                slots[i].occupant = Occupant::Open;
                slots[i].reserved_for = Some(nick.clone());
            }
            None => slots[i].occupant = Occupant::Ai,
        }
    }
    Ok(slots)
}

impl SaveRef {
    pub fn of(entry: &SaveEntry) -> Self {
        Self {
            id: entry.id.clone(),
            game: entry.game,
            seq: entry.seq,
            file: entry.file.clone(),
            turn: entry.turn,
            players: entry.seats.iter().map(|s| s.player).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seat(player: PlayerId, nick: Option<&str>) -> SaveSeat {
        SaveSeat {
            player,
            civ: CivPick::Random,
            civ_name: format!("civ{player}"),
            nick: nick.map(str::to_owned),
        }
    }

    pub(crate) fn entry(game: GameId, seq: u64, kind: SaveKind, at: u64) -> SaveEntry {
        SaveEntry {
            id: save_id(game, seq),
            game,
            seq,
            kind,
            file: format!("ring/{seq:08}-t0001.c5sv"),
            name: format!("game {game}"),
            turn: 1,
            saved_at_ms: at,
            build_id: "core-a".into(),
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
                timer: protocol::TimerMode::Off,
                game_options: vec![],
                seed: None,
                map_seed: None,
                auto_choices: protocol::AutoChoices::default(),
                diplo_auto_decline: true,
                mods: Vec::new(),
            },
            seats: vec![seat(0, Some("alice")), seat(1, None)],
            host: "alice".into(),
        }
    }

    #[test]
    fn save_ids_are_parsed_strictly() {
        assert_eq!(save_id(3, 12), "3-00000012");
        assert_eq!(parse_save_id("3-00000012"), Some((3, 12)));
        for bad in [
            "",
            "3",
            "3-12",
            "3-0000001",
            "3-000000012",
            "03-00000012",
            "+3-00000012",
            "x-00000012",
            "3-0000001x",
            "-3-00000012",
            "3-00000012-",
            " 3-00000012",
        ] {
            assert_eq!(parse_save_id(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_index_keeps_every_keep_file_and_the_newest_ring_file_newest_first() {
        let mut ix = SaveIndex::new();
        ix.insert(entry(1, 1, SaveKind::Ring, 10));
        ix.insert(entry(1, 2, SaveKind::Ring, 20));
        ix.insert(entry(1, 3, SaveKind::Keep, 15));
        ix.insert(entry(2, 1, SaveKind::Ring, 30));
        let ids: Vec<_> = ix.list(None).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["2-00000001", "1-00000002", "1-00000003"]);
        // The same save again replaces it.
        ix.insert(entry(1, 3, SaveKind::Keep, 99));
        assert_eq!(ix.len(), 3);
        assert_eq!(ix.list(None)[0].id, "1-00000003");
    }

    #[test]
    fn saves_of_another_core_build_are_not_listed_or_loadable() {
        let mut ix = SaveIndex::new();
        ix.insert(entry(1, 1, SaveKind::Keep, 1));
        let mut other = entry(2, 1, SaveKind::Keep, 2);
        other.build_id = "core-b".into();
        ix.insert(other);
        assert_eq!(ix.list(Some("core-a")).len(), 1);
        assert!(ix.loadable("2-00000001", Some("core-a")).is_none());
        assert!(ix.loadable("1-00000001", Some("core-a")).is_some());
        assert_eq!(ix.list(None).len(), 2);
    }

    #[test]
    fn the_list_is_capped() {
        let mut ix = SaveIndex::new();
        for g in 0..250u32 {
            ix.insert(entry(g + 1, 1, SaveKind::Keep, u64::from(g)));
        }
        assert_eq!(ix.list(None).len(), MAX_LISTED);
    }
}
