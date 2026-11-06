//! Games on the gateway: the game actor ([`actor`]), the seat pipelines
//! ([`seat`], [`stream`]), order numbers ([`orders`]), the turn state and its timers ([`turn`],
//! [`timer`]) and the counters ([`stats`]). The registry
//! [`Games`] is how the lobby, the sessions and the admin API find a running game.

pub mod actor;
pub mod chat;
pub mod gamelog;
pub mod mods;
pub mod orders;
pub mod recording;
pub mod replica;
pub mod restore;
pub mod seat;
pub mod stats;
pub mod stream;
pub mod timer;
pub mod trace;
pub mod turn;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use protocol::{GameId, GameStatus, PlayerId, SeatControl, TurnMode, TurnPhase};
use tokio::sync::mpsc;

pub use self::actor::{
    ActorMsg, AdminCtl, AttachRequest, ClientGameMsg, Fault, GameEnv, GameInit, GameLoad,
    JoinRequest, spawn,
};
use self::stats::{GameStats, SeatStatsMap};
use crate::state::meta::{GameMeta, IdStore};
use crate::supervisor::GameState;

/// How a session reaches the game it plays. The game sets it when it takes the session's attach
/// and clears it when the session loses the seat; the session reads it for every game message.
#[derive(Clone)]
pub struct GameLink {
    pub game: GameId,
    pub player: PlayerId,
    pub tx: mpsc::UnboundedSender<ActorMsg>,
}

/// The slot of a session that holds its [`GameLink`].
pub type SessionLink = Arc<Mutex<Option<GameLink>>>;

/// Reads a link.
pub fn link_of(link: &SessionLink) -> Option<GameLink> {
    link.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// A seat as the lobby list and the admin API show it.
#[derive(Debug, Clone)]
pub struct SeatSnap {
    pub player: PlayerId,
    pub civ_name: String,
    pub control: SeatControl,
    pub online: bool,
    pub conn: &'static str,
}

/// What the game actor publishes about itself after every change.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub state: GameState,
    pub turn: i32,
    pub phase: Option<TurnPhase>,
    pub mode: Option<TurnMode>,
    /// Map size `(width, height)`; zeros before the worker is ready.
    pub map: (u16, u16),
    pub core_build: String,
    pub seats: Vec<SeatSnap>,
    pub last_exit: Option<i32>,
    /// The host now: it changes when the host is away for `host_lost_s` (GS-D21).
    pub host: String,
}

/// Static data and the latest snapshot of a game, shared with whoever lists games.
#[derive(Debug)]
pub struct GameInfo {
    pub id: GameId,
    pub name: String,
    pub world_size: String,
    /// The host the game started with; the current one is in the [`Snapshot`].
    pub host: String,
    /// The settings the game started with, echoed by the metrics.
    pub settings: protocol::GameSettings,
    snap: Mutex<Snapshot>,
    pub stats: GameStats,
    pub seat_stats: SeatStatsMap,
    /// The journal of the game (`logs/game-<id>.jsonl`).
    pub log: gamelog::GameLog,
    /// The recording of the game (`recordings/g<id>-<ms>/`, `recording.rs`).
    pub rec: recording::Recorder,
    /// The timeline of the end of each turn (`trace.rs`).
    pub trace: trace::GameTrace,
}

impl GameInfo {
    pub fn new(
        id: GameId,
        host: String,
        settings: protocol::GameSettings,
        log: gamelog::GameLog,
        rec: recording::Recorder,
    ) -> Self {
        Self {
            id,
            name: settings.name.clone(),
            world_size: settings.world_size.clone(),
            host: host.clone(),
            settings,
            snap: Mutex::new(Snapshot {
                state: GameState::Starting,
                turn: 0,
                phase: None,
                mode: None,
                map: (0, 0),
                core_build: String::new(),
                seats: Vec::new(),
                last_exit: None,
                host,
            }),
            stats: GameStats::default(),
            seat_stats: SeatStatsMap::default(),
            trace: trace::GameTrace::new(id, log.clone(), rec.clone()),
            log,
            rec,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snap
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_snapshot(&self, s: Snapshot) {
        *self.snap.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }

    /// Status for the lobby list (NP-D20): `Faulted` games are not listed.
    pub fn status(&self) -> Option<GameStatus> {
        status_of(self.snapshot().state)
    }
}

/// `GameState` of the supervisor to `GameStatus` of the protocol.
pub fn status_of(state: GameState) -> Option<GameStatus> {
    match state {
        GameState::Starting => Some(GameStatus::Starting),
        GameState::Running | GameState::Restarting => Some(GameStatus::Running),
        GameState::Finished => Some(GameStatus::Ended),
        GameState::Faulted => None,
    }
}

/// A running game: its queue and its info.
#[derive(Clone)]
pub struct GameHandle {
    pub id: GameId,
    tx: mpsc::UnboundedSender<ActorMsg>,
    pub info: Arc<GameInfo>,
}

impl GameHandle {
    /// `false`: the game has ended.
    pub fn send(&self, msg: ActorMsg) -> bool {
        self.tx.send(msg).is_ok()
    }

    pub fn sender(&self) -> mpsc::UnboundedSender<ActorMsg> {
        self.tx.clone()
    }
}

/// The games of the gateway: the running ones, and the sleeping ones (`Hibernating`: on disk, no
/// worker, [`restore`]).
pub struct Games {
    map: Mutex<BTreeMap<GameId, GameHandle>>,
    dormant: Mutex<BTreeMap<GameId, GameMeta>>,
    ids: Arc<IdStore>,
    next_qid: AtomicU64,
    faulted: AtomicU64,
}

impl Default for Games {
    fn default() -> Self {
        Self::with_ids(Arc::new(IdStore::in_memory()))
    }
}

impl Games {
    pub fn with_ids(ids: Arc<IdStore>) -> Self {
        Self {
            map: Mutex::default(),
            dormant: Mutex::default(),
            ids,
            next_qid: AtomicU64::new(0),
            faulted: AtomicU64::new(0),
        }
    }

    /// A fresh game number; kept in `ids.json`, so a number is never used twice.
    pub fn next_id(&self) -> GameId {
        self.ids.next_game()
    }

    /// A sleeping game (no worker); the lobby lists it as `Hibernated`.
    pub fn add_dormant(&self, meta: GameMeta) {
        self.dormant
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(meta.id, meta);
    }

    /// Takes a sleeping game out (to wake it).
    pub fn take_dormant(&self, id: GameId) -> Option<GameMeta> {
        self.dormant
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id)
    }

    pub fn dormant(&self, id: GameId) -> Option<GameMeta> {
        self.dormant
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
    }

    pub fn dormant_list(&self) -> Vec<GameMeta> {
        self.dormant
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// A query number, unique in the gateway (`ToWorker::Query.qid`).
    pub fn next_qid(&self) -> u64 {
        self.next_qid.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// A game became `Faulted` (it leaves the registry; the metrics keep the count).
    pub fn note_faulted(&self) {
        self.faulted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn faulted_total(&self) -> u64 {
        self.faulted.load(Ordering::Relaxed)
    }

    pub fn insert(&self, h: GameHandle) {
        self.lock().insert(h.id, h);
    }

    pub fn remove(&self, id: GameId) {
        self.lock().remove(&id);
    }

    pub fn get(&self, id: GameId) -> Option<GameHandle> {
        self.lock().get(&id).cloned()
    }

    pub fn list(&self) -> Vec<GameHandle> {
        self.lock().values().cloned().collect()
    }

    /// Games whose worker is up or coming up (the `max_games` check counts these).
    pub fn running(&self) -> usize {
        self.list()
            .iter()
            .filter(|g| {
                matches!(
                    g.info.snapshot().state,
                    GameState::Starting | GameState::Running | GameState::Restarting
                )
            })
            .count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<GameId, GameHandle>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How long `Shutdown` may take before the worker is killed.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
