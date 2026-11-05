//! Lobby, rooms and slots (net-protocol 4.11, NP-D20). Types for `game-server` and `lobby-ui`; who may do
//! what is the server's spec. Field and variant order is the wire format.

use serde::{Deserialize, Serialize};

use crate::session::BasisClaim;
use crate::stream::{SeatControl, TurnMode};
use crate::world::{AutoChoices, GameId, LobbyChoice, PlayerId};

pub type RoomId = u32;
pub type SaveId = String;

/// `ClientMsg::Lobby` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyCmd {
    CreateRoom {
        settings: GameSettings,
    },
    JoinRoom {
        room: RoomId,
    },
    LeaveRoom,
    /// Host.
    Settings(GameSettings),
    /// Host: set a slot to open, AI or closed.
    Slot {
        index: u8,
        occupant: SlotOccupant,
        civ: CivPick,
    },
    PickCiv {
        civ: CivPick,
    },
    TakeSlot {
        index: u8,
    },
    Ready(bool),
    /// Host; every human is ready.
    Start,
    /// Host: a room from a save.
    LoadSave {
        save: SaveId,
    },
    /// Take an AI slot of a running game (N-D8) or return to the own seat. `basis` is the map basis the
    /// client already has (NP-D22).
    JoinGame {
        game: GameId,
        player: PlayerId,
        basis: Option<BasisClaim>,
    },
    /// Give up the own seat in a game (running or asleep): the slot goes to the AI, the nick is free to
    /// create, load or join. Appended at the end: the existing tags are unchanged.
    LeaveSeat,
}

/// What the host puts into a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotOccupant {
    Open,
    Ai,
    Closed,
}

/// What a slot holds now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Occupant {
    Open,
    Human { nick: String, ready: bool },
    Ai,
    Closed,
}

/// `ServerMsg::LobbyState`: the lobby whole, on every change. `room` and `takeable` are per receiver.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyState {
    pub rooms: Vec<RoomSummary>,
    pub games: Vec<GameSummary>,
    pub saves: Vec<SaveSummary>,
    pub room: Option<RoomState>,
    /// v11: every signed-in player and where they are, sorted by nick (case-insensitive).
    pub online: Vec<OnlinePlayer>,
}

/// v11: one signed-in player of the lobby's online list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnlinePlayer {
    pub nick: String,
    pub place: PlayerPlace,
}

/// v11: where a signed-in player is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerPlace {
    Lobby,
    Room {
        room: RoomId,
        name: String,
        host: bool,
        ready: bool,
    },
    Game {
        game: GameId,
        name: String,
        turn: i32,
    },
}

/// Status of a room and of a game (NP-D20). Room: `Lobby` (gathering) or `Starting` (worker starting).
/// Game: `Starting` (start or waking), `Running` (worker alive, also during a restart after a crash),
/// `Hibernated` (on disk without a worker; a login with the token or `JoinGame` wakes it, the status is
/// `Starting` meanwhile; not the turn pause, which lives in `TurnState.paused`), `Ended`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameStatus {
    Lobby,
    Starting,
    Running,
    Hibernated,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSummary {
    pub id: RoomId,
    pub name: String,
    pub host: String,
    /// Slots taken by humans.
    pub humans: u8,
    /// Slots that are not closed.
    pub slots: u8,
    pub world_size: String,
    pub map_script: String,
    /// `Lobby` or `Starting`.
    pub status: GameStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomState {
    pub id: RoomId,
    pub name: String,
    pub host: String,
    pub status: GameStatus,
    pub settings: GameSettings,
    pub slots: Vec<RoomSlot>,
    pub from_save: Option<SaveId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSlot {
    pub index: u8,
    pub occupant: Occupant,
    pub civ: CivPick,
    /// A slot of a save (`LoadSave`) kept for this nick; only it can take the slot.
    pub reserved_for: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CivPick {
    Random,
    /// `CIVILIZATION_*` type.
    Civ(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSettings {
    pub name: String,
    pub world_size: String,
    pub map_script: String,
    pub map_options: Vec<(String, i32)>,
    pub speed: String,
    pub start_era: String,
    pub handicap: String,
    pub minor_civs: u8,
    /// Hybrid by default (N-D5).
    pub turn_mode: TurnMode,
    pub timer: TimerMode,
    pub game_options: Vec<(String, i32)>,
    pub seed: Option<u32>,
    pub map_seed: Option<u32>,
    pub auto_choices: AutoChoices,
    pub diplo_auto_decline: bool,
}

/// Turn timer. `Off` by default; `Original` names a row of the `TurnTimers` table (`LobbyChoices.timers`,
/// the core computes the length); `Fixed` is a server mode with no original and no extension for wars
/// between humans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerMode {
    Off,
    Original { row: String },
    Fixed { seconds: u16 },
}

/// Input of a game worker: the settings and who sits where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSetup {
    pub settings: GameSettings,
    pub slots: Vec<SlotSetup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlotSetup {
    pub player: PlayerId,
    pub civ: CivPick,
    pub control: SlotControl,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotControl {
    Human { nick: String },
    Ai,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameSummary {
    pub id: GameId,
    pub name: String,
    pub turn: i32,
    pub world_size: String,
    pub host: String,
    pub status: GameStatus,
    pub seats: Vec<SeatSummary>,
}

/// `takeable` is per receiver: an AI slot of a live major civ (N-D8) or the receiver's own seat (after
/// `Leave`, a drop or a kick), which `JoinGame` can take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatSummary {
    pub player: PlayerId,
    pub civ_name: String,
    pub control: SeatControl,
    pub online: bool,
    pub takeable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveSummary {
    pub id: SaveId,
    pub name: String,
    pub turn: i32,
    pub saved_at_ms: u64,
    /// `(player, civ name, nick of the human or none)`.
    pub seats: Vec<(PlayerId, String, Option<String>)>,
    /// v11: the game the save belongs to.
    pub game: GameId,
    /// v11: the nick that hosted the game (the first human seat's nick when unknown, or empty).
    pub host: String,
}

/// `ServerMsg::Choices`: the pick lists, once after login. The entries are the v2 `LobbyChoice`
/// (`type_name` is the database type).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyChoices {
    pub hash: u64,
    pub civs: Vec<CivChoice>,
    pub handicaps: Vec<LobbyChoice>,
    pub world_sizes: Vec<WorldSizeChoice>,
    pub map_scripts: Vec<LobbyChoice>,
    pub speeds: Vec<LobbyChoice>,
    pub eras: Vec<LobbyChoice>,
    /// Rows of `TurnTimers`.
    pub timers: Vec<LobbyChoice>,
    /// v8: filled from the `GameOptions` table (the Lekmod MP screen's list and grouping); empty before v8.
    pub options: Vec<OptionChoice>,
    /// v8: rows of `Victories` (without `VICTORY_SCRAP`). The room keeps a victory as a `game_options` entry
    /// `(VICTORY_*, 0|1)` (see [`VICTORY_PREFIX`]); a victory not listed there is on.
    pub victories: Vec<LobbyChoice>,
    /// v8: the custom options of each map script (`GetMapScriptInfo().CustomOptions`), by `map_scripts` path.
    pub map_options: Vec<MapScriptOptions>,
    /// v8: how many city-states the game data has (`#MinorCivilizations`): the original slider's upper end.
    pub max_minor_civs: u8,
}

/// A `game_options` key that is a victory condition (`VICTORY_TIME` ...), not a `GameOptions` row. The worker
/// takes these out of `game_options` and gives them to the core's `victories`. Rides in `GameSettings` so
/// that saves (which hold `GameSettings` in postcard) keep their format.
pub const VICTORY_PREFIX: &str = "VICTORY_";

/// The `game_options` key of the max turns (`PreGame.SetMaxTurns`): 0 or absent = the speed's default.
pub const MAX_TURNS_KEY: &str = "MAX_TURNS";

/// Highest max turns the room accepts.
pub const MAX_TURNS_LIMIT: i32 = 2000;

/// The custom options of one map script.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapScriptOptions {
    /// `LobbyChoices.map_scripts[].type_name`.
    pub script: String,
    /// Sorted as the original screen sorts them (`SortPriority`, then name).
    pub options: Vec<MapOptionChoice>,
}

/// One custom option of a map script. The `GameSettings.map_options` key is `id` (the 1-based option number as
/// text, what `Map.GetCustomOption(n)` asks for); the value is a 1-based index into `values`, or 0/1 for a
/// check box.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapOptionChoice {
    pub id: String,
    /// Localized (en_US).
    pub name: String,
    pub help: String,
    pub default: i32,
    /// Empty: a check box.
    pub values: Vec<(i32, String)>,
    pub read_only: bool,
}

/// v8: how the room screen groups a game option (Lekmod `MPGameOptions.lua`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum OptionGroup {
    #[default]
    General,
    /// Shown under `GAMEOPTION_AI_OPTIONS`, only while that one is on.
    Ai,
    /// Shown under `GAMEOPTION_ADDITIONAL`, only while that one is on.
    Additional,
    /// A row the original MP screen does not show (`Visible = 0` or not multiplayer); valid, under "More".
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CivChoice {
    pub id: String,
    pub name: String,
    pub leader: String,
    /// v10: the hover tip of the staging room, localized: one `"Name: description"` per trait of the leader.
    pub traits: Vec<String>,
    /// v10: one `"Unit: name"` / `"Building: name"` per unique unit or building.
    pub unique: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSizeChoice {
    pub id: String,
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub default_players: u8,
    pub max_players: u8,
    pub default_minor: u8,
    pub max_minor: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionChoice {
    pub key: String,
    pub name: String,
    pub default: i32,
    /// Empty: a check box, 0 or 1.
    pub values: Vec<(i32, String)>,
    /// v8: the localized `Help` text (tooltip), may be empty.
    pub help: String,
    /// v8.
    pub group: OptionGroup,
}
