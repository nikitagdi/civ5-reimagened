//! The game core as the worker sees it (game-host-mp GH-D1, section 5): the questions the `Worker` asks and
//! the orders it gives. `RealCore` (gamecore) and `FixtureCore` (test world) implement it; all game rules
//! live behind it.
//!
//! Fog is applied by the implementation: everything that takes a `team` or a `player` is already filtered
//! for that viewer. The only data without fog is `Core::basis` (map basis, NP-D19) and `MapInfo`.

use std::fmt;

use crate::replica::ReplicaOps;
use protocol::{
    AiTurnEvent, Catalog, CityBuildView, CityView, CombatPreview, Command, GameSetup, HumanWar,
    JournalEntry, LobbyChoices, NotificationView, PathNode, PlayerId, PlayerView, PlotBasisView,
    PlotView, ReachNode, SeatTurnInfo, Shooter, SlotControl, SlotSetup, TeamId, TopBar,
    TradeRouteView, TurnMode, UnitActionView, UnitView, ViewHeader,
};

/// How `Core::start` begins a game.
#[derive(Debug, Clone, Copy)]
pub enum CoreStart<'a> {
    /// A new game from the setup; `seed` is the game seed of `WorkerInit`.
    New { setup: &'a GameSetup, seed: u64 },
    /// Continue from the core part (`GCS1`, already decompressed) of a C5SV save.
    Load(&'a [u8]),
}

/// Why a core call failed (not an order refusal: see [`Refusal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreErrorKind {
    /// The call failed; the game may go on.
    Failed,
    /// `start(Load)`: the save does not load. The worker exits with code 4 (game-host-mp 4.12).
    BadSave,
    /// `start(New)`: the game cannot start. The worker exits with code 5.
    BadStart,
    /// The core is stuck (internal error). The worker exits with code 3.
    Stuck,
    /// `save`: the core is not at a save point right now (a human's unit or city is busy, a command waits for
    /// a pump). Never fatal: the worker pumps and tries again later (live game 931, 2026-10-30).
    NotSafe,
}

/// A failed core call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreError {
    pub kind: CoreErrorKind,
    pub message: String,
}

impl CoreError {
    pub fn new(kind: CoreErrorKind, message: impl Into<String>) -> Self {
        CoreError {
            kind,
            message: message.into(),
        }
    }

    /// A call that failed, the game goes on.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(CoreErrorKind::Failed, message)
    }
}

impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CoreError {}

/// Why the core refused an order. The codes are `GC_REASON_*` of `gc-abi-v2` 4.4 reduced to what the
/// worker translates (game-host-mp 4.5 item 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    /// `NOT_HUMAN`: the player is not a human (any more).
    NotHuman,
    /// `NOT_YOUR_TURN`: hybrid mode, a human at war with the seat moves now.
    NotYourTurn,
    /// `TURN_ENDED`: the seat has ended its turn, or the turn is already changing.
    TurnEnded,
    /// `AI_RUNNING`: the core is busy with the AI phase; the order waits in the queue.
    AiRunning,
    /// `PUMP_FIRST`: a command waits for a pump or a unit / city of the seat is busy (a move the core still
    /// times, `CvUnit::IsBusy`); the worker pumps and gives the order again.
    PumpFirst,
    /// The host's own check of an order failed before the core saw it (a v15 queue or research order that
    /// names no own city, an item the city cannot start, a slot that is not in the queue, an unknown
    /// technology); the worker answers `RejectCode::Invalid`.
    Invalid,
    /// Any other reason; the text is the core's.
    Other,
}

/// A refused order: code and the core's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: RefusalCode,
    pub text: String,
}

impl Refusal {
    pub fn new(code: RefusalCode, text: impl Into<String>) -> Self {
        Refusal {
            code,
            text: text.into(),
        }
    }

    /// Refusal with code `Other`.
    pub fn other(text: impl Into<String>) -> Self {
        Self::new(RefusalCode::Other, text)
    }

    /// Refusal with code `Invalid`: the host's check of the order failed.
    pub fn invalid(text: impl Into<String>) -> Self {
        Self::new(RefusalCode::Invalid, text)
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.text)
    }
}

impl std::error::Error for Refusal {}

/// Static map facts, no fog (`Core::map`); the same numbers go to `WorkerReady` and `MapBasis`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MapInfo {
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub wrap_y: bool,
    pub terrain_seed: u32,
}

impl MapInfo {
    /// Plot count, `width * height`.
    pub fn plot_count(&self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }
}

/// Where the game is after a `pump`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorePhase {
    /// The humans move.
    Humans,
    /// The AI computes; `active` is the player moving now.
    Ai { active: PlayerId },
    /// The game is over.
    GameOver { winner: Option<TeamId> },
}

/// Result of `Core::pump`: where the core stopped and in which turn. Calling `pump` again in `Ai` goes on
/// with the AI phase (at most `budget` core events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorePump {
    pub phase: CorePhase,
    pub turn: i32,
}

/// Turn data of the core (`Core::turn`); the worker builds `TurnInfo` from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreTurn {
    pub mode: TurnMode,
    /// One entry per human seat.
    pub seats: Vec<SeatTurnInfo>,
    /// `getMaxTurnLen()` of the timer row in use; 0: no timer.
    pub turn_len_s: u32,
    /// `getBaseTime()` of the same row; 0: no row.
    pub timer_base_s: u32,
    /// Wars declared human -> human since the previous call (the call drains them).
    pub human_wars: Vec<HumanWar>,
}

/// Whom a core event is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventTarget {
    All,
    Player(PlayerId),
}

/// A message of the core's own log, with its addressee (GH-R6); the worker files it into the journal of
/// the seats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreEvent {
    pub to: EventTarget,
    pub entry: JournalEntry,
}

/// Core for the worker. Methods that take a `team` or a `player` return what that viewer may know.
pub trait Core {
    /// Begins the game. Called once.
    fn start(&mut self, start: CoreStart<'_>) -> Result<(), CoreError>;
    /// Reference data of the game.
    fn catalog(&mut self) -> Result<Catalog, CoreError>;
    /// Pick lists of the lobby (`Start::Describe`).
    fn choices(&mut self) -> Result<LobbyChoices, CoreError>;
    /// Build id of the core with its FIX/HYG flag set: `WorkerReady.build_id`, the header of a C5SV save,
    /// `civ5-server describe --json`. `RealCore` (H8) fills it from the library.
    fn build_id(&self) -> String {
        "unknown".into()
    }
    /// Width, height, wrap, terrain seed.
    fn map(&mut self) -> MapInfo;
    /// Who sits where now.
    fn seats(&mut self) -> Result<Vec<SlotSetup>, CoreError>;

    /// Runs the core for at most `budget` events and says where it stopped.
    fn pump(&mut self, budget: u32) -> Result<CorePump, CoreError>;
    /// The slices the last `pump` ran (the core's own unit of work; the AI of one civilization takes several). The
    /// default is one per call, as the test world has it.
    fn last_pump_slices(&self) -> u32 {
        1
    }
    /// The `setTurnActive` of the non-human civilizations (AI, city-states, barbarians) the core did since the previous
    /// call, in order, drained: every AI is started in a burst when the new turn begins and ended one by one, ascending
    /// by player id, as it has moved (SIMP-exe-820). A core that cannot say answers none.
    fn turn_status(&mut self) -> Vec<AiTurnEvent> {
        Vec::new()
    }
    /// Mode, seats (`finished`, `may_act`), timer data.
    fn turn(&mut self) -> Result<CoreTurn, CoreError>;
    /// An order of a human.
    fn command(&mut self, player: PlayerId, cmd: &Command) -> Result<(), Refusal>;
    /// The end of the seat's turn by the timer (`END_TURN_TIMEOUT`).
    fn end_turn_timeout(&mut self, player: PlayerId) -> Result<(), Refusal>;
    /// Hands the player to a human or to the AI.
    fn set_control(&mut self, player: PlayerId, control: &SlotControl) -> Result<(), CoreError>;
    /// `Ok` when the core can save or change a slot now; `Err` says why not.
    fn safe_point(&mut self) -> Result<(), String>;
    /// The core part of a save (`GCS1`, not compressed).
    fn save(&mut self) -> Result<Vec<u8>, CoreError>;

    /// Plot indexes (`y * width + x`, ascending) whose record changed for some team since the previous
    /// call. `None`: not known, take all plots (the ABI stub, tests). The real core and the test world
    /// answer always (GH-R7).
    fn dirty_plots(&mut self) -> Option<Vec<u32>>;
    /// The plots of `team` (all, or the `subset` of indexes), ascending by index, appended to `out`.
    fn plots(
        &mut self,
        team: TeamId,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, PlotView)>,
    ) -> Result<(), CoreError>;
    /// Map basis (NP-D19): the relief of the whole map without fog, natural wonders included, no mask by
    /// team. All plots, or the `subset` of indexes, ascending, appended to `out`.
    fn basis(
        &mut self,
        subset: Option<&[u32]>,
        out: &mut Vec<(u32, PlotBasisView)>,
    ) -> Result<(), CoreError>;
    /// The owner of every plot, or of the `subset` of indexes (ascending), as `(index, player)` appended to `out`,
    /// `-1` nobody. The whole truth: no team's view, no fog (`gc_query` viewer `-1`). For the table of owners the
    /// gateway keeps for server mods (`ToWorker::WantOwners`).
    fn owners(&mut self, subset: Option<&[u32]>, out: &mut Vec<(u32, i8)>)
    -> Result<(), CoreError>;
    /// Units the team sees (its own, and others on visible plots).
    fn units(&mut self, team: TeamId) -> Result<Vec<UnitView>, CoreError>;
    // No `combat_turns`: there is no combat readiness in B2 (user decision 2026-10-21, NP-D24, RW-28).
    /// Cities the team knows. A foreign city on a revealed plot that is not visible comes with name and
    /// position only, the rest zero (the worker keeps the last seen values, 4.3 item 6).
    fn cities(&mut self, team: TeamId) -> Result<Vec<CityView>, CoreError>;
    /// Players as the team knows them.
    fn players(&mut self, team: TeamId) -> Result<Vec<PlayerView>, CoreError>;
    /// Trade route lines the team's view shows (net-protocol 4.15). A core without routes answers none.
    fn trade_routes(&mut self, _team: TeamId) -> Result<Vec<TradeRouteView>, CoreError> {
        Ok(Vec::new())
    }
    /// Turn, year, status, blocker of the player.
    fn header(&mut self, player: PlayerId) -> Result<ViewHeader, CoreError>;
    fn top_bar(&mut self, player: PlayerId) -> Result<TopBar, CoreError>;
    fn notifications(&mut self, player: PlayerId) -> Result<Vec<NotificationView>, CoreError>;

    fn unit_actions(
        &mut self,
        player: PlayerId,
        unit: i32,
    ) -> Result<Vec<UnitActionView>, CoreError>;
    /// Move tree of the unit, node 0 is its plot, parents before children.
    fn reach(
        &mut self,
        player: PlayerId,
        unit: i32,
        horizon: u8,
    ) -> Result<Vec<ReachNode>, CoreError>;
    /// `(x, y, flags)` of the plots the shooter can hit (`flags` as `TargetView::flags`).
    fn targets(
        &mut self,
        player: PlayerId,
        shooter: Shooter,
    ) -> Result<Vec<(i16, i16, u16)>, CoreError>;
    fn preview(
        &mut self,
        player: PlayerId,
        attacker: Shooter,
        x: i16,
        y: i16,
    ) -> Result<Option<CombatPreview>, CoreError>;
    fn city_builds(&mut self, player: PlayerId, city: i32)
    -> Result<Vec<CityBuildView>, CoreError>;
    fn path(
        &mut self,
        player: PlayerId,
        unit: i32,
        x: i16,
        y: i16,
    ) -> Result<Vec<PathNode>, CoreError>;
    /// The core allows white peace between the two players now.
    fn can_make_peace(&mut self, a: PlayerId, b: PlayerId) -> Result<bool, CoreError>;

    /// Drains the core's log since the previous call.
    fn events(&mut self) -> Vec<CoreEvent>;
    /// Digest of the state seen by the team (`-1`: the whole state), for tests and query noise.
    fn digest(&mut self, viewer_team: i32) -> Result<u64, CoreError>;

    /// The replica side of the core (the journal and the UI messages of the seats, `replica.rs`). `None`: the
    /// core is not the server of a replica (the test world), the worker sends no replica stream.
    fn replica(&mut self) -> Option<&mut dyn ReplicaOps> {
        None
    }
}
