//! The game stream (net-protocol 4.4, 4.8, 4.10): batches the gateway sends to one seat, orders and
//! their answers, turn state, seats and chat. Field and variant order is the wire format.

use serde::{Deserialize, Serialize};

use crate::context::ContextMsg;
use crate::replica::{GcBase, GcJournal, GcTurn};
use crate::view::ViewDiff;
use crate::world::{Command, JournalEntry, MapBasis, PlayerId, TeamId, WorldView};

/// One WebSocket message of the game stream: the result of one server step for one seat. `seq` starts at 1
/// in `Attached` and grows by 1 with every batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameBatch {
    pub seq: u64,
    pub msgs: Vec<GameMsg>,
}

/// Messages inside a batch. Order inside a batch: `MapBasis`, the view message (`Keyframe` or `Diff`, at
/// most one), `Results`, `Journal`, `Context`, `TurnState`, `Seats`, `Chat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameMsg {
    /// Tag 0, gateway: the whole view of the seat, without the map basis.
    Keyframe(Keyframe),
    /// Tag 1, worker, forwarded by the gateway: the difference to the view of batch `base`.
    Diff(ViewDelta),
    /// Tag 2, worker (gateway for repeats and `Lost`): answers to orders.
    Results(Vec<CommandResult>),
    /// Tag 3, worker (gateway for system lines): journal entries of the seat.
    Journal(Vec<JournalEntry>),
    /// Tag 4, gateway: turn state, whole.
    TurnState(TurnState),
    /// Tag 5, gateway: seats and their connections, whole.
    Seats(Vec<SeatView>),
    /// Tag 6, worker; the gateway sets `view_seq`: a unit or city context.
    Context(ContextMsg),
    /// Tag 7, gateway: chat lines (history is several lines at once).
    Chat(Vec<ChatLine>),
    /// Tag 8, gateway: the map basis (NP-D22). Not a view message. Only in a batch with a `Keyframe`, before it.
    MapBasis(MapBasis),
    /// Tag 9 (v4), gateway: the base of the replica chain (snapshot of the start of the turn).
    GcBase(GcBase),
    /// Tag 10 (v4), worker via gateway: journal batches of the human phase, in order.
    GcJournal(GcJournal),
    /// Tag 11 (v4), gateway: turn change, a new snapshot (and perhaps a patch).
    GcTurn(GcTurn),
    /// Tag 12 (v7, was the AI progress of v6), worker via gateway: which civilizations the core started and ended during
    /// the AI phase (SIMP-exe-820), as they happen.
    AiTurn(AiTurn),
}

/// One `setTurnActive` of a civilization that is not a human, as the server's core did it in the AI phase: the original
/// DLL activates every AI at the start of the new turn (`started`) and deactivates each one when it has moved (`ended`),
/// ascending by player id (`CvGame::doTurn`, `updateMoves`, `CheckPlayerTurnDeactivate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiTurnEvent {
    pub player: PlayerId,
    /// `false`: the civilization became active (`AIProcessingStartedForPlayer`); `true`: it is done
    /// (`AIProcessingEndedForPlayer`).
    pub ended: bool,
}

/// A piece of the AI phase of `turn` (the turn that is being ended), in the order the core did it. Not part of any state:
/// the client lights the AI icons of the turn queue and counts "N of M" with it while "the turn is being processed".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiTurn {
    pub turn: i32,
    pub events: Vec<AiTurnEvent>,
}

/// Full view of the seat without the map basis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keyframe {
    pub view: WorldView,
    /// View hash.
    pub hash: u64,
    /// Hash of the map basis this frame was built with; the client compares it with its own basis.
    pub basis_hash: u64,
}

/// A view diff. Applied only when `base` equals the client's `view_seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDelta {
    /// `seq` of the batch with the previous view message.
    pub base: u64,
    pub diff: ViewDiff,
    /// View hash after applying.
    pub hash: u64,
    /// Basis hash after applying; `Some` exactly when `diff.basis` is not empty.
    pub basis_hash: Option<u64>,
}

/// Why the client asks for a keyframe (`ClientMsg::NeedKeyframe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyframeReason {
    HashMismatch,
    BaseMismatch,
    ApplyError,
    Decode,
    /// No basis, its hash differs or the frame names another one. The answer is `MapBasis` and `Keyframe`.
    BasisMismatch,
}

// ---------------------------------------------------------------- orders

/// Order number: grows by 1 within a seat, continues from `Attached.last_cmd_id`.
pub type CmdId = u32;

/// An order with its number (`ClientMsg::Cmd`). `turn` is the turn the client saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cmd {
    pub id: CmdId,
    pub turn: i32,
    pub cmd: Command,
}

/// Who gets a chat line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatTo {
    All,
    Player(PlayerId),
}

/// Answer to one order. Every `id` gets exactly one final answer (`Queued` may come before it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    pub id: CmdId,
    pub outcome: CommandOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandOutcome {
    /// The consequences are already in the view (they came earlier in the stream).
    Applied,
    Rejected {
        code: RejectCode,
        reason: String,
    },
    /// The core is busy; the final answer comes later.
    Queued,
    /// The order was not applied: the worker died before it.
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectCode {
    /// The core refused; `reason` is its text.
    Core,
    /// The order was given in a previous turn.
    TurnChanged,
    /// The turn is ending already, the end of turn cannot be taken back.
    TurnEnded,
    /// Hybrid mode: a human at war with the seat is moving now.
    NotYourTurn,
    NoGame,
    Restarting,
    TooMany,
    Invalid,
    Expired,
    NotSeated,
}

// ---------------------------------------------------------------- turn, seats, chat

/// Turn state of the whole game, sent to every seat on every change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnState {
    pub turn: i32,
    pub phase: TurnPhase,
    pub mode: TurnMode,
    /// Human seats.
    pub seats: Vec<SeatTurn>,
    /// One per running timer group; empty: no timer.
    pub timers: Vec<TimerState>,
    /// Host pause: only the timer stands (N-D13).
    pub paused: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnPhase {
    Starting,
    Ai { active: PlayerId },
    Humans,
    GameOver { winner: Option<TeamId> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnMode {
    Simultaneous,
    Hybrid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatTurn {
    pub player: PlayerId,
    pub finished: bool,
    /// Hybrid: the seat may act now.
    pub may_act: bool,
    /// Timer group of the seat in this turn: 0 is the simultaneous seats, 1 and up a sequential seat of
    /// hybrid mode (a new number for every new sequential seat in the turn).
    pub group: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimerState {
    pub group: u8,
    /// Gateway clock, the one of `Pong.server_ms`.
    pub deadline_ms: u64,
    pub len_ms: u32,
    /// Pause or disconnect (N-D7): what was left when the timer stopped.
    pub left_ms_when_stopped: Option<u32>,
}

/// A seat of the game and its connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatView {
    pub player: PlayerId,
    pub control: SeatControl,
    pub conn: Conn,
    pub host: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeatControl {
    Human {
        nick: String,
    },
    /// `kicked`: the nick of the human this seat was taken from.
    Ai {
        kicked: Option<String>,
    },
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Conn {
    Online,
    /// The connection dropped at `since_ms` (gateway clock).
    Lost {
        since_ms: u64,
    },
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub from: String,
    pub from_player: Option<PlayerId>,
    pub to: ChatTo,
    pub text: String,
    /// Monotonic gateway milliseconds (the clock of `Pong.server_ms`), not a calendar: the order of lines.
    pub server_ms: u64,
}
