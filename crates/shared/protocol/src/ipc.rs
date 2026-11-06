//! IPC between the gateway and a game worker (net-protocol 4.14, game-host-mp 4.12-4.16, section 5).
//! Transport: the worker's stdin carries `ToWorker` frames, its stdout `WorkerFrame`s; one frame is a `u32 LE`
//! length and a postcard body (`wire::ipc`); stderr is for logs only. Variant and field order is the wire
//! format: append only, one test pins the tag of every variant.

use serde::{Deserialize, Serialize};

use crate::context::{ContextMsg, Focus};
use crate::lobby::{GameSetup, LobbyChoices, SlotSetup};
use crate::replica::{GcBlobRef, GcPatchRef};
use crate::stream::{Cmd, CmdId, CommandResult, TurnMode, TurnPhase};
use crate::view::ViewDiff;
use crate::world::{
    GameId, JournalEntry, MapBasis, PlayerId, Query, QueryAnswer, SaveMeta, WorldView,
};

/// Version of this IPC contract. The first frame of the gateway (`ToWorker::Init`) names it; a worker of
/// another version answers `Fatal`.
pub const IPC_VERSION: u32 = 7;

// ---------------------------------------------------------------- gateway -> worker

/// A frame from the gateway to the worker.
// Short-lived IPC messages, built and matched once per frame: the size difference is not worth a Box in the API.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToWorker {
    /// Tag 0: the first frame, exactly one.
    Init(WorkerInit),
    /// Tag 1: an order of a seat, in the order the gateway received it.
    Cmd { player: PlayerId, cmd: Cmd },
    /// Tag 2: a query of a seat; `qid` is unique in the gateway.
    Query {
        player: PlayerId,
        qid: u64,
        q: Query,
    },
    /// Tag 3: the focus (units and city whose contexts the seat wants) of a seat.
    Focus { player: PlayerId, focus: Focus },
    /// Tag 4: the gateway needs the whole view of the seat (a hash did not match). `basis`: also the map
    /// basis of the seat (the `basis_hash` did not match).
    WantFull { player: PlayerId, basis: bool },
    /// Tag 5: a seat change, applied at a safe point. `CONNECT` after loading a save the worker does itself.
    Seat {
        player: PlayerId,
        change: SeatChange,
    },
    /// Tag 6: the turn timer ran out for these seats.
    ForceEndTurn { players: Vec<PlayerId> },
    /// Tag 7: save at the next safe point.
    SaveNow { reason: SaveReason },
    /// Tag 8: stop; with `save` after a final save.
    Shutdown { save: bool },
    /// Tag 9 (v4): GCN1 messages of a seat's UI (`ClientMsg::GcNet`), submitted at the next step, all or nothing.
    Net { player: PlayerId, msgs: Vec<u8> },
    /// Tag 10 (v4): a fresh replica snapshot for this seat at the next safe point (`ClientMsg::GcResync`).
    ReplicaSnapshot { player: PlayerId },
}

/// What happens to a seat (`ToWorker::Seat`, `WorkerMsg::SeatDone`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeatChange {
    /// Tag 0: the human leaves, the AI takes the seat over.
    Kick,
    /// Tag 1: the human `nick` takes the seat from the AI.
    Claim { nick: String },
}

/// Why the gateway asks for a save.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveReason {
    /// Tag 0: the server stops.
    Shutdown,
    /// Tag 1: an administrator asked.
    Admin,
}

/// The first frame of a worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInit {
    /// `IPC_VERSION` of the gateway.
    pub ipc: u32,
    pub game: GameId,
    pub core: CoreKind,
    pub start: Start,
    /// Directory of the autosave ring (`ring/`, `keep/`).
    pub save_dir: String,
    pub seed: u64,
    pub cfg: WorkerConfig,
}

/// Which game core the worker builds. The seats come from `GameSetup`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreKind {
    /// Tag 0: the test world, no game files (Windows, tests).
    Fixture { huge: bool, wrap_x: bool },
    /// Tag 1: the real core with the game data read from `gamedata_dir`.
    Real { gamedata_dir: String },
}

/// How the worker starts.
// Short-lived IPC messages, built and matched once per frame: the size difference is not worth a Box in the API.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Start {
    /// Tag 0: a new game.
    New(GameSetup),
    /// Tag 1: continue a C5SV save; `seats` say who sits where now.
    Load { file: String, seats: Vec<SlotSetup> },
    /// Tag 2: no game; answer one `Choices` with the pick lists (`civ5-server describe`) and exit.
    Describe,
}

/// Autosave settings (game-host-mp 4.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutosaveCfg {
    /// Least seconds between two saves in a phase.
    pub min_interval_s: u32,
    /// Files kept in `ring/`.
    pub ring: u32,
    /// A save of the start of turn `T` with `T % keep_every == 0` also goes to `keep/`.
    pub keep_every: u32,
}

impl Default for AutosaveCfg {
    fn default() -> Self {
        AutosaveCfg {
            min_interval_s: 60,
            ring: 10,
            keep_every: 10,
        }
    }
}

/// Which map basis the worker builds (NP 4.5). Only `All` exists in B2; the key stays because the `golden`
/// scenes name it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BasisNw {
    /// Tag 0: all plots, natural wonders of the whole map included (NP-D19).
    #[default]
    All,
}

/// Test hooks of the test world (`WorkerConfig.fixture_test`, only with the gateway's `test_hooks`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureTest {
    /// Delay of every AI step, milliseconds.
    pub ai_step_delay_ms: u32,
    /// Put an artificial fog leak into the views (the fog auditor must catch it).
    pub leak_fog: bool,
    /// Change one record of the map basis at the start of this turn (a `basis` diff), if set.
    pub change_basis_at_turn: Option<i32>,
}

/// Worker settings (game-host-mp section 5). Defaults in `Default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Most core events pumped per step.
    pub pump_budget: u32,
    /// An order batch ends after this many orders...
    pub batch_max_cmds: u32,
    /// ...or after this many milliseconds of applying.
    pub batch_max_ms: u32,
    /// Default focus radius of units whose context is built without a `Focus`.
    pub focus_default: u8,
    /// Default `Query::Reach` horizon in turns.
    pub reach_horizon: u8,
    pub autosave: AutosaveCfg,
    /// Interval of AI-phase views, milliseconds; 0: only at the end of the phase.
    pub ai_view_interval_ms: u32,
    pub basis_nw: BasisNw,
    /// Check every message to a seat against the core (fog auditor, game-host-mp 4.17).
    pub audit_fog: bool,
    /// Put `gc_state_digest(-1)` of every turn end into `StepStats`.
    pub digests: bool,
    /// Random queries between orders (`net-test`, AC-N1.4): how many per order; 0 - none.
    pub query_noise: u32,
    /// Send `WorkerMsg::Stats` after every step.
    pub stats: bool,
    /// Test hooks, the test world only.
    pub fixture_test: Option<FixtureTest>,
    /// The gateway records the game (v4): the worker sends the journal it would drop
    /// (`WorkerMsg::RecJournal`) and marks the long core calls (`WorkerMsg::RecMark`).
    pub record: bool,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            pump_budget: 32,
            batch_max_cmds: 8,
            batch_max_ms: 10,
            focus_default: 3,
            reach_horizon: 1,
            autosave: AutosaveCfg::default(),
            ai_view_interval_ms: 0,
            basis_nw: BasisNw::All,
            audit_fog: false,
            digests: false,
            query_noise: 0,
            stats: false,
            fixture_test: None,
            record: false,
        }
    }
}

// ---------------------------------------------------------------- worker -> gateway

/// Everything one call of the worker produced. Contract: at most one `View` and one `Basis` per seat; order
/// for a seat: `Basis`, `View`, `Results`, `Journal`, `Context`; `Turn` after all seats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFrame {
    pub msgs: Vec<WorkerMsg>,
}

/// A message of the worker.
// Short-lived IPC messages, built and matched once per frame: the size difference is not worth a Box in the API.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerMsg {
    /// Tag 0: the game is up (answer to `Init`, after `Catalog`).
    Ready(WorkerReady),
    /// Tag 1: the catalog body for `/catalog/{hash}`.
    Catalog { hash: u64, bytes: Vec<u8> },
    /// Tag 2: the pick lists (mode `Start::Describe`).
    Choices(LobbyChoices),
    /// Tag 3: the map basis (NP-D22) for these seats; one basis for all seats (NP-D19). Once per seat before
    /// its first `Full` after a start, restart or `Claim`, and before a `Full` that answers
    /// `WantFull { basis: true }`.
    Basis {
        players: Vec<PlayerId>,
        basis: MapBasis,
    },
    /// Tag 4: the view of a seat.
    View {
        player: PlayerId,
        update: ViewUpdate,
    },
    /// Tag 5: answers to orders.
    Results {
        player: PlayerId,
        results: Vec<CommandResult>,
    },
    /// Tag 6: journal entries of a seat.
    Journal {
        player: PlayerId,
        entries: Vec<JournalEntry>,
    },
    /// Tag 7: a context for the view of revision `rev`.
    Context {
        player: PlayerId,
        rev: u32,
        ctx: ContextMsg,
    },
    /// Tag 8: the answer to `ToWorker::Query`.
    QueryResult { qid: u64, answer: QueryAnswer },
    /// Tag 9: turn state (turn, phase, mode, seats, timer data).
    Turn(TurnInfo),
    /// Tag 10: a seat change was done, or `error`.
    SeatDone {
        player: PlayerId,
        change: SeatChange,
        error: Option<String>,
    },
    /// Tag 11: a save was written.
    Saved(SaveRecord),
    /// Tag 12: a save failed; the game goes on.
    SaveFailed { reason: String },
    /// Tag 13: the worker is alive (also during long AI phases).
    Heartbeat { phase: TurnPhase, queued: u32 },
    /// Tag 14: timing of a step (`WorkerConfig.stats`).
    Stats(StepStats),
    /// Tag 15: the worker cannot go on and exits with `code`.
    Fatal { message: String, code: u8 },
    /// Tag 16 (v4): journal batches (GCJ1) of the human phase, `jseq` from `first_jseq` on.
    ReplicaJournal {
        epoch: u32,
        first_jseq: u64,
        batches: Vec<Vec<u8>>,
    },
    /// Tag 17 (v4): the body for `/replica/{hash}` (`wire::replica` encoding).
    ReplicaBlob { hash: u64, bytes: Vec<u8> },
    /// Tag 18 (v4): a replica base; `for_player` set: the answer to `ReplicaSnapshot`, for that seat only.
    ReplicaBase {
        epoch: u32,
        turn: i32,
        jseq: u64,
        raw_hash: u64,
        snapshot: GcBlobRef,
        for_player: Option<PlayerId>,
    },
    /// Tag 19 (v4): the snapshot of the start of a new human phase (and a patch when made).
    ReplicaTurn {
        epoch: u32,
        turn: i32,
        jseq: u64,
        raw_hash: u64,
        snapshot: GcBlobRef,
        patch: Option<GcPatchRef>,
    },
    /// Tag 20 (v4): the core refused the seat's `Net` message.
    NetRejected { player: PlayerId, reason: String },
    /// Tag 21 (v3): the timeline of one end of a turn (`turntrace::TurnTrace`, postcard) once the snapshot of the
    /// next turn is packed. `turn` is the turn that was ended. Opaque here (the protocol crate takes no
    /// `turntrace` dependency); the gateway decodes it.
    Trace { turn: i32, bytes: Vec<u8> },
    /// Tag 22 (v5, was the AI progress of v3): the `setTurnActive` of the non-human civilizations the core did in the
    /// pump calls since the previous frame (SIMP-exe-820), in order; sent with the frame of the call, never held back.
    /// `turn` is the turn that is being ended.
    AiTurn {
        turn: i32,
        events: Vec<crate::stream::AiTurnEvent>,
    },
    /// Tag 23 (v4, `WorkerConfig.record`): a GCJ1 batch that the replica stream does not carry, for the recording:
    /// the AI phase's and the cut right before a turn snapshot.
    RecJournal {
        phase: RecPhase,
        /// The game turn the worker was in when it took the batch.
        turn: i32,
        bytes: Vec<u8>,
    },
    /// Tag 24 (v4, `WorkerConfig.record`): a core call that may run long begins (`begin`) or, after one that ran past a
    /// second, ends. Sent when the call starts, so a call that never returns is named in the recording.
    RecMark {
        turn: i32,
        what: String,
        begin: bool,
    },
}

/// Which part of the game a recorded journal batch belongs to (`WorkerMsg::RecJournal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecPhase {
    /// Tag 0: a batch of the human phase (the recording builds it from `ReplicaJournal`; the worker does not send it).
    Human,
    /// Tag 1: the AI phase and the turn change after it (the replica stream drops it).
    Ai,
    /// Tag 2: what the core did between the last batch and the snapshot of a turn start (normally nothing).
    Cut,
}

/// Header of a started game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerReady {
    /// Core build id together with the FIX/HYG flag set; the same as in the C5SV header and in
    /// `civ5-server describe --json`.
    pub build_id: String,
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub wrap_y: bool,
    pub terrain_seed: u32,
    /// Who sits where.
    pub seats: Vec<SlotSetup>,
    pub turn: i32,
}

/// The view of a seat from the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewUpdate {
    /// Tag 0: the whole view without the map basis.
    Full {
        rev: u32,
        view: WorldView,
        hash: u64,
    },
    /// Tag 1: the difference to revision `base_rev`. `basis_hash` is set exactly when `diff.basis` is not
    /// empty (as in `ViewDelta`).
    Delta {
        base_rev: u32,
        rev: u32,
        diff: ViewDiff,
        hash: u64,
        basis_hash: Option<u64>,
    },
}

/// Turn state from the worker; the gateway builds `TurnState` from it and runs the timers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInfo {
    pub turn: i32,
    pub phase: TurnPhase,
    pub mode: TurnMode,
    pub seats: Vec<SeatTurnInfo>,
    /// `getMaxTurnLen()` of the `TurnTimers` row (`TimerMode::Original`); 0: no timer. Updated at the start
    /// of the human phase.
    pub turn_len_s: u32,
    /// `CvTurnTimerInfo::getBaseTime` of the same row; 0: no row (`Off`, `Fixed`).
    pub timer_base_s: u32,
    /// Wars declared human -> human since the previous `Turn`; they extend the timer by the Lekmod rule.
    pub human_wars: Vec<HumanWar>,
}

/// A human seat in `TurnInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatTurnInfo {
    pub player: PlayerId,
    pub finished: bool,
    pub may_act: bool,
    /// The seat moves simultaneously this turn (the `SIMULTANEOUS` flag of the core).
    pub simultaneous: bool,
}

/// A war declared by a human on a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanWar {
    /// The one who declared.
    pub from: PlayerId,
    pub to: PlayerId,
}

/// Which kind of file a save is (game-host-mp 4.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaveKind {
    /// Tag 0: a file of the `ring/` directory.
    Ring,
    /// Tag 1: a file of `keep/` (start of turn with `turn % keep_every == 0`), kept for good.
    Keep,
}

/// A written save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveRecord {
    pub meta: SaveMeta,
    /// Path relative to `save_dir`.
    pub file: String,
    pub kind: SaveKind,
    /// The last order of every seat that is in the save.
    pub applied_upto: Vec<(PlayerId, CmdId)>,
}

/// What a step of the worker is (`StepStats.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepKind {
    /// Tag 0: a batch of orders.
    Orders,
    /// Tag 1: an AI phase.
    AiPhase,
    /// Tag 2: a turn change.
    TurnChange,
    /// Tag 3: a seat change.
    Seat,
    /// Tag 4: anything else (queries, focus, full view on request).
    Other,
}

/// Timing of one worker step (game-host-mp 4.17). Times in microseconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepStats {
    pub kind: StepKind,
    pub apply_us: u32,
    pub pump_us: u32,
    pub views_us: u32,
    pub diff_us: u32,
    pub ctx_us: u32,
    pub save_us: u32,
    /// Time of the AI phase in this step.
    pub ai_us: u32,
    /// Bytes of the diff (postcard) per seat.
    pub diff_bytes: Vec<(PlayerId, u32)>,
    pub fog_violations: u32,
    /// `gc_state_digest(-1)` at the end of a turn (`WorkerConfig.digests`).
    pub digest: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Codec, Json, Postcard};

    fn tag<T: Serialize>(v: &T) -> Vec<u8> {
        Postcard.encode(v).unwrap()
    }

    fn view() -> WorldView {
        WorldView {
            turn: 3,
            year: -3000,
            human: 0,
            human_team: 0,
            status: crate::TurnStatus::HumanTurn,
            blocker: None,
            map: Default::default(),
            units: vec![],
            cities: vec![],
            players: vec![],
            top_bar: Default::default(),
            notifications: vec![],
            peace_offers: vec![],
            trade_routes: vec![],
        }
    }

    fn setup() -> GameSetup {
        GameSetup {
            settings: crate::GameSettings {
                name: "g".into(),
                world_size: "WORLDSIZE_DUEL".into(),
                map_script: "m".into(),
                map_options: vec![("k".into(), -1)],
                speed: "s".into(),
                start_era: "e".into(),
                handicap: "h".into(),
                minor_civs: 3,
                turn_mode: TurnMode::Hybrid,
                timer: crate::TimerMode::Fixed { seconds: 90 },
                game_options: vec![],
                seed: Some(7),
                map_seed: None,
                auto_choices: crate::AutoChoices::default(),
                diplo_auto_decline: true,
                mods: Vec::new(),
            },
            slots: vec![SlotSetup {
                player: 0,
                civ: crate::CivPick::Random,
                control: crate::SlotControl::Human {
                    nick: "alice".into(),
                },
            }],
        }
    }

    fn init(core: CoreKind, start: Start) -> WorkerInit {
        WorkerInit {
            ipc: IPC_VERSION,
            game: 9,
            core,
            start,
            save_dir: "saves/9".into(),
            seed: u64::MAX,
            cfg: WorkerConfig {
                fixture_test: Some(FixtureTest {
                    ai_step_delay_ms: 5,
                    leak_fog: true,
                    change_basis_at_turn: Some(4),
                }),
                stats: true,
                ..WorkerConfig::default()
            },
        }
    }

    fn cmd() -> Cmd {
        Cmd {
            id: 5,
            turn: 3,
            cmd: crate::Command::CancelEndTurn,
        }
    }

    fn to_worker() -> Vec<ToWorker> {
        vec![
            ToWorker::Init(init(
                CoreKind::Fixture {
                    huge: true,
                    wrap_x: false,
                },
                Start::New(setup()),
            )),
            ToWorker::Cmd {
                player: 1,
                cmd: cmd(),
            },
            ToWorker::Query {
                player: 1,
                qid: 1 << 40,
                q: Query::Reach {
                    unit: 1,
                    horizon: 2,
                },
            },
            ToWorker::Focus {
                player: 2,
                focus: Focus {
                    units: vec![1, 2],
                    city: Some(3),
                },
            },
            ToWorker::WantFull {
                player: 2,
                basis: true,
            },
            ToWorker::Seat {
                player: 3,
                change: SeatChange::Claim { nick: "bob".into() },
            },
            ToWorker::ForceEndTurn {
                players: vec![0, 1],
            },
            ToWorker::SaveNow {
                reason: SaveReason::Admin,
            },
            ToWorker::Shutdown { save: true },
            ToWorker::Net {
                player: 1,
                msgs: vec![1, 2, 3],
            },
            ToWorker::ReplicaSnapshot { player: 2 },
        ]
    }

    fn turn_info() -> TurnInfo {
        TurnInfo {
            turn: 12,
            phase: TurnPhase::Humans,
            mode: TurnMode::Hybrid,
            seats: vec![SeatTurnInfo {
                player: 1,
                finished: false,
                may_act: true,
                simultaneous: true,
            }],
            turn_len_s: 90,
            timer_base_s: 20,
            human_wars: vec![HumanWar { from: 0, to: 1 }],
        }
    }

    fn stats() -> StepStats {
        StepStats {
            kind: StepKind::AiPhase,
            apply_us: 1,
            pump_us: 2,
            views_us: 3,
            diff_us: 4,
            ctx_us: 5,
            save_us: 6,
            ai_us: 7,
            diff_bytes: vec![(0, 100), (1, 200)],
            fog_violations: 0,
            digest: Some(u64::MAX),
        }
    }

    fn worker_msgs() -> Vec<WorkerMsg> {
        vec![
            WorkerMsg::Ready(WorkerReady {
                build_id: "gc-1+FIX".into(),
                width: 51,
                height: 20,
                wrap_x: true,
                wrap_y: false,
                terrain_seed: 12345,
                seats: setup().slots,
                turn: 0,
            }),
            WorkerMsg::Catalog {
                hash: 1 << 63,
                bytes: vec![1, 2, 3],
            },
            WorkerMsg::Choices(LobbyChoices::default()),
            WorkerMsg::Basis {
                players: vec![0, 1],
                basis: MapBasis::default(),
            },
            WorkerMsg::View {
                player: 0,
                update: ViewUpdate::Full {
                    rev: 1,
                    view: view(),
                    hash: 9,
                },
            },
            WorkerMsg::Results {
                player: 0,
                results: vec![CommandResult {
                    id: 1,
                    outcome: crate::CommandOutcome::Applied,
                }],
            },
            WorkerMsg::Journal {
                player: 0,
                entries: vec![JournalEntry {
                    turn: 1,
                    kind: crate::JournalKind::System,
                    text: "t".into(),
                    at: Some((1, 2)),
                    simplification: None,
                }],
            },
            WorkerMsg::Context {
                player: 0,
                rev: 2,
                ctx: ContextMsg::Dropped { unit: 4 },
            },
            WorkerMsg::QueryResult {
                qid: 7,
                answer: QueryAnswer::CombatPreview(None),
            },
            WorkerMsg::Turn(turn_info()),
            WorkerMsg::SeatDone {
                player: 1,
                change: SeatChange::Kick,
                error: Some("e".into()),
            },
            WorkerMsg::Saved(SaveRecord {
                meta: SaveMeta {
                    seq: 3,
                    build_id: "gc-1".into(),
                    turn: 12,
                    civ_name: "Rome".into(),
                    saved_at_ms: 1,
                },
                file: "ring/00000003-t0012.c5sv".into(),
                kind: SaveKind::Keep,
                applied_upto: vec![(0, 41), (1, 7)],
            }),
            WorkerMsg::SaveFailed { reason: "r".into() },
            WorkerMsg::Heartbeat {
                phase: TurnPhase::Ai { active: 2 },
                queued: 3,
            },
            WorkerMsg::Stats(stats()),
            WorkerMsg::Fatal {
                message: "m".into(),
                code: 5,
            },
            WorkerMsg::ReplicaJournal {
                epoch: 2,
                first_jseq: 10,
                batches: vec![vec![1], vec![]],
            },
            WorkerMsg::ReplicaBlob {
                hash: 1 << 63,
                bytes: vec![4, 5],
            },
            WorkerMsg::ReplicaBase {
                epoch: 2,
                turn: 7,
                jseq: 11,
                raw_hash: 5,
                snapshot: crate::GcBlobRef { hash: 6, size: 7 },
                for_player: Some(1),
            },
            WorkerMsg::ReplicaTurn {
                epoch: 2,
                turn: 8,
                jseq: 12,
                raw_hash: 9,
                snapshot: crate::GcBlobRef { hash: 6, size: 7 },
                patch: Some(crate::GcPatchRef {
                    from_raw_hash: 5,
                    blob: crate::GcBlobRef { hash: 8, size: 1 },
                }),
            },
            WorkerMsg::NetRejected {
                player: 1,
                reason: "r".into(),
            },
            WorkerMsg::Trace {
                turn: 126,
                bytes: vec![1, 2],
            },
            WorkerMsg::AiTurn {
                turn: 126,
                events: vec![
                    crate::stream::AiTurnEvent {
                        player: 3,
                        ended: false,
                    },
                    crate::stream::AiTurnEvent {
                        player: 3,
                        ended: true,
                    },
                ],
            },
            WorkerMsg::RecJournal {
                phase: RecPhase::Ai,
                turn: 126,
                bytes: vec![7, 8, 9],
            },
            WorkerMsg::RecMark {
                turn: 126,
                what: "a step".into(),
                begin: true,
            },
        ]
    }

    fn rt<C: Codec, T>(codec: &C, v: &T)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let bytes = codec.encode(v).expect("encode");
        assert_eq!(&codec.decode::<T>(&bytes).expect("decode"), v);
    }

    #[test]
    fn round_trips() {
        for m in to_worker() {
            rt(&Postcard, &m);
            rt(&Json, &m);
        }
        let mut msgs = worker_msgs();
        msgs.push(WorkerMsg::View {
            player: 1,
            update: ViewUpdate::Delta {
                base_rev: 1,
                rev: 2,
                diff: ViewDiff::default(),
                hash: 5,
                basis_hash: Some(6),
            },
        });
        msgs.push(WorkerMsg::Stats(StepStats {
            digest: None,
            ..stats()
        }));
        for m in &msgs {
            rt(&Postcard, m);
            rt(&Json, m);
        }
        let frame = WorkerFrame { msgs };
        rt(&Postcard, &frame);
        rt(&Json, &frame);
        rt(&Postcard, &WorkerFrame::default());
        rt(
            &Postcard,
            &init(
                CoreKind::Real {
                    gamedata_dir: "d".into(),
                },
                Start::Load {
                    file: "f".into(),
                    seats: setup().slots,
                },
            ),
        );
        rt(
            &Postcard,
            &init(
                CoreKind::Real {
                    gamedata_dir: "d".into(),
                },
                Start::Describe,
            ),
        );
        for k in [
            StepKind::Orders,
            StepKind::AiPhase,
            StepKind::TurnChange,
            StepKind::Seat,
            StepKind::Other,
        ] {
            rt(&Postcard, &k);
        }
    }

    #[test]
    fn tag_stability_to_worker() {
        let tags: Vec<u8> = to_worker().iter().map(|m| tag(m)[0]).collect();
        assert_eq!(tags, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(
            tag(&ToWorker::WantFull {
                player: 2,
                basis: true
            }),
            [4, 2, 1]
        );
        assert_eq!(tag(&ToWorker::Shutdown { save: false }), [8, 0]);
        assert_eq!(tag(&ToWorker::ForceEndTurn { players: vec![] }), [6, 0]);
        assert_eq!(
            tag(&ToWorker::SaveNow {
                reason: SaveReason::Shutdown
            }),
            [7, 0]
        );
        assert_eq!(tag(&SaveReason::Admin), [1]);
        assert_eq!(tag(&SeatChange::Kick), [0]);
        assert_eq!(tag(&SeatChange::Claim { nick: "a".into() }), [1, 1, b'a']);
        assert_eq!(
            tag(&CoreKind::Fixture {
                huge: true,
                wrap_x: false
            }),
            [0, 1, 0]
        );
        assert_eq!(
            tag(&CoreKind::Real {
                gamedata_dir: String::new()
            }),
            [1, 0]
        );
        assert_eq!(tag(&Start::Describe), [2]);
        assert_eq!(
            tag(&Start::Load {
                file: String::new(),
                seats: vec![]
            }),
            [1, 0, 0]
        );
        assert_eq!(tag(&BasisNw::All), [0]);
    }

    #[test]
    fn tag_stability_worker_msg() {
        let mut msgs = worker_msgs();
        let tags: Vec<u8> = msgs.iter().map(|m| tag(m)[0]).collect();
        assert_eq!(tags, (0..=24).collect::<Vec<u8>>());
        assert_eq!(tag(&RecPhase::Human), [0]);
        assert_eq!(tag(&RecPhase::Ai), [1]);
        assert_eq!(tag(&RecPhase::Cut), [2]);
        // `ViewUpdate` tags: Full 0, Delta 1 (after the player byte).
        let full = tag(&msgs.remove(4));
        assert_eq!((full[0], full[1], full[2]), (4, 0, 0));
        let delta = tag(&WorkerMsg::View {
            player: 0,
            update: ViewUpdate::Delta {
                base_rev: 1,
                rev: 2,
                diff: ViewDiff::default(),
                hash: 0,
                basis_hash: None,
            },
        });
        assert_eq!((delta[0], delta[1], delta[2]), (4, 0, 1));
        assert_eq!(
            tag(&WorkerMsg::SaveFailed {
                reason: String::new()
            }),
            [12, 0]
        );
        assert_eq!(tag(&SaveKind::Keep), [1]);
        assert_eq!(tag(&StepKind::Other), [4]);
    }

    #[test]
    fn worker_config_defaults() {
        let c = WorkerConfig::default();
        assert_eq!(
            (
                c.pump_budget,
                c.batch_max_cmds,
                c.batch_max_ms,
                c.focus_default,
                c.reach_horizon
            ),
            (32, 8, 10, 3, 1)
        );
        assert_eq!(
            c.autosave,
            AutosaveCfg {
                min_interval_s: 60,
                ring: 10,
                keep_every: 10
            }
        );
        assert_eq!(c.ai_view_interval_ms, 0);
        assert!(!c.audit_fog && !c.digests && !c.stats && c.fixture_test.is_none() && !c.record);
    }

    #[test]
    fn turn_info_fields_are_appended() {
        // turn, phase (Humans = 2), mode (Hybrid = 1), seats (0), turn_len_s, timer_base_s, human_wars (0)
        let t = TurnInfo {
            turn: 1,
            phase: TurnPhase::Humans,
            mode: TurnMode::Hybrid,
            seats: vec![],
            turn_len_s: 90,
            timer_base_s: 20,
            human_wars: vec![],
        };
        assert_eq!(tag(&t), [2, 2, 1, 0, 90, 20, 0]);
    }
}
