//! The game actor: one task per game. It consumes the supervisor's
//! [`GameEvent`]s, turns each worker frame into one step per seat pipeline, answers the orders
//! the worker cannot (`Lost`, `Rejected { Restarting }`, repeats, `Expired`), and keeps the seats:
//! who sits where, who is connected. It never waits for a socket.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use protocol::{
    AiTurn, BasisClaim, CatalogRef, ChatLine, ChatSend, CivPick, Cmd, CmdId, CommandOutcome,
    CommandResult, Conn, DetachReason, Focus, GameCtl, GameCtlReason, GameId, GameMsg, GameSetup,
    GcBase, GcJournal, GcResyncReason, GcTurn, JournalEntry, JournalKind, KeyframeReason,
    LobbyChoices, PlayerId, Query, RejectCode, Resume, RoomId, SaveReason, SaveRecord, SeatChange,
    SeatControl, SeatView, ServerMsg, SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnInfo,
    TurnPhase, TurnState, WorkerFrame, WorkerMsg,
};
use tokio::sync::{mpsc, oneshot};

use super::chat;
use super::gamelog;
use super::modhost::{Delivery, ModHost, UnknownFacts};
use super::orders::{OrderBook, Verdict};
use super::recording::{KeyframeArgs, Recorder};
use super::replica::{ReplicaBlobs, ReplicaChain, resync_index};
use super::seat::{self, AttachReq, SeatFault, SeatHandle, SeatInput, Then};
use super::stats::{self, GameStats};
use super::stream::Parts;
use super::timer::{Action, Timers};
use super::turn::turn_state;
use super::{GameHandle, GameInfo, GameLink, Games, SeatSnap, SessionLink, Snapshot};
use crate::clock::{Clock, WallClock};
use crate::lobby::rooms::MAX_HUMANS;
use crate::lobby::saves::{SaveEntry, SaveSeat, save_id};
use crate::lobby::{LobbyMsg, LobbyTx};
use crate::session::limits::{LimitsConfig, SeatLimits};
use crate::session::outbox::Outbox;
use crate::state::meta::{GameMeta, MetaSeat, MetaStatus};
use crate::supervisor::policy::{PolicyConfig, SaveBook};
use crate::supervisor::{
    FaultedInfo, GameEvent, GameSpec, GameState, GameSupervisor, RestartingInfo, StartedInfo,
    Supervisor,
};
use recording::format::Verdict as RecVerdict;

/// What a game needs from the gateway around it.
#[derive(Clone)]
pub struct GameEnv {
    pub supervisor: Supervisor,
    pub clock: Arc<dyn Clock>,
    pub limits: LimitsConfig,
    pub lobby: LobbyTx,
    pub games: Arc<Games>,
    pub policy: PolicyConfig,
    pub shutdown_grace: Duration,
    /// The pick lists, for the civilization names of the seats.
    pub choices: Arc<LobbyChoices>,
    /// How long the host may be without a connection before the rights pass on (GS-D21), ms.
    pub host_lost_ms: u64,
    /// Nobody online for this long and the game sleeps, ms; 0: never.
    pub hibernate_idle_ms: u64,
    /// Nobody online for this long and the game is closed (seats freed, saves kept), ms; 0: never.
    pub empty_close_ms: u64,
    /// Where `games/<id>/meta.json` goes; `None`: nothing is kept.
    pub state_dir: Option<std::path::PathBuf>,
    /// Calendar time for the meta.
    pub wall: Arc<dyn WallClock>,
    /// The bodies of the replica snapshots, served at `/replica/{hash}`.
    pub replica: Arc<ReplicaBlobs>,
    /// The recording of every game (`[recording]`).
    pub recording: crate::config::Recording,
    /// The installed mods and the code of the server halves (T17): the game's frozen set is instantiated from them.
    pub mod_store: Arc<mod_host_server::ModStore>,
    pub mod_registry: Arc<mod_host_server::HostRegistry>,
    /// What the sessions have built in (`ModHello`).
    pub mod_hellos: Arc<super::modhost::ModHellos>,
}

/// A new game.
pub struct GameInit {
    pub id: GameId,
    pub setup: GameSetup,
    pub host: String,
    /// The room it grows from; the lobby is told when the game is up.
    pub room: Option<RoomId>,
    pub seed: u64,
    /// A game that continues a save instead of a new one (`LoadSave`, a game back from the disk).
    pub load: Option<GameLoad>,
    /// Seats the AI holds after a kick, from the stored meta of a game that comes back.
    pub kicked: BTreeMap<PlayerId, String>,
    /// When the game was made (Unix ms), if it is not new.
    pub created_ms: Option<u64>,
    /// The frozen mod set (`mods.lock` text) of a game with mods; `None` for a game without.
    pub mods_lock: Option<String>,
}

/// The save a game continues: the file as the worker's `Start::Load` names it (absolute when it
/// belongs to another game) and its turn.
#[derive(Debug, Clone)]
pub struct GameLoad {
    pub file: String,
    pub turn: i32,
    /// The saves of the game on disk, for its restarts; `None`: only `file`.
    pub book: Option<SaveBook>,
}

/// A connection that takes an AI slot of the running game (`JoinGame`).
pub struct JoinRequest {
    pub player: PlayerId,
    pub nick: String,
    pub session: u64,
    pub out: Outbox,
    pub link: SessionLink,
    pub claim: Option<BasisClaim>,
}

/// How long the worker may take for a `Claim`.
const CLAIM_WAIT_MS: u64 = 20_000;
/// How often the actor looks at its clocks while something waits on them.
const TICK: Duration = Duration::from_millis(100);
/// How long a lost connection stays hidden from the other seats (no flicker on a reload).
const SHOW_LOST_MS: u64 = 3_000;
/// How long the worker may take for a `Kick` before it is asked again.
const KICK_WAIT_MS: u64 = 20_000;
/// A finished game stays open this long after the last word of a player.
const OVER_QUIET_MS: u64 = 5 * 60 * 1000;
/// A finished game with no human online closes after this (long enough for a page reload to come back).
const OVER_EMPTY_MS: u64 = 15_000;

/// A slot being taken: the worker has the `Claim`, the answer is `SeatDone`.
struct PendingClaim {
    nick: String,
    session: u64,
    out: Outbox,
    link: SessionLink,
    claim: Option<BasisClaim>,
    since_ms: u64,
    at: std::time::Instant,
    /// The wait is over; a late `SeatDone` undoes the claim.
    expired: bool,
}

/// A connection that takes a seat.
pub struct AttachRequest {
    pub player: PlayerId,
    pub nick: String,
    pub session: u64,
    pub out: Outbox,
    pub link: SessionLink,
    pub resume: Option<Resume>,
    pub claim: Option<BasisClaim>,
}

/// What a session sends to its game.
#[derive(Debug)]
pub enum ClientGameMsg {
    Cmd(Cmd),
    Focus(Focus),
    Query {
        id: u32,
        q: Query,
    },
    NeedKeyframe(KeyframeReason),
    /// Host controls: `Pause`, `Kick`, and `Leave` (a voluntary leave is a disconnect, NP-D21).
    GameCtl(GameCtl),
    Chat(ChatSend),
    /// `GcNet`: the GCN1 messages of the seat's UI (at most `GC_NET_MAX` bytes).
    GcNet(Vec<u8>),
    /// `GcResync`: the replica of the seat cannot go on.
    GcResync {
        epoch: u32,
        have_raw_hash: Option<u64>,
        at_jseq: u64,
        reason: GcResyncReason,
    },
    /// `Mod` (v13): a message of a mod's client half.
    Mod(protocol::ModC2S),
}

/// Administrator actions (`/admin/games/{id}/...`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminCtl {
    Save,
    /// Save and stop the worker; the game sleeps with its seats kept.
    Stop,
    Restart,
    /// Save, stop the worker and close the game for good (`Finished` on disk, seats freed, saves kept).
    Close,
    /// The gateway stops: save, stop the worker, the game stays `Running` on disk.
    Drain,
}

/// A test hook on one seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The next order of the seat is refused.
    RejectNextCmd,
    Seat(SeatFault),
}

/// What reaches the actor.
pub enum ActorMsg {
    Attach(AttachRequest),
    /// The session of a seat ended.
    SessionGone {
        player: PlayerId,
        session: u64,
    },
    /// The queue of a session overflowed or closed (from the seat pipeline).
    SessionLost {
        player: PlayerId,
        session: u64,
    },
    /// A seat of a game with mods got a `Fresh` stream (from the seat pipeline): the mods send it their snapshot.
    FreshAttached {
        player: PlayerId,
        session: u64,
    },
    /// A pipeline needs a full view (from the seat pipeline).
    WantFull {
        player: PlayerId,
        basis: bool,
    },
    Client {
        player: PlayerId,
        session: u64,
        msg: ClientGameMsg,
        /// Size of the frame the client sent (the `rx` counters).
        bytes: usize,
    },
    Admin(AdminCtl),
    /// An administrator kicks a player without being the host; the answer says why not.
    AdminKick {
        player: PlayerId,
        reply: oneshot::Sender<Result<(), GameCtlReason>>,
    },
    /// A test hook on a seat (`--test-hooks`).
    Fault {
        player: PlayerId,
        fault: Fault,
    },
    /// A connection wants an AI slot (`JoinGame`).
    Join(JoinRequest),
    /// The owner of a seat gives it up (`LeaveSeat`): a safe kick, or, for the last human, the game goes to
    /// sleep with the slot changed to the AI's. The lobby is told `Kicked` (or `GiveUpFailed`).
    GiveUp {
        player: PlayerId,
        nick: String,
    },
}

/// Why the worker of a game is stopped on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Nobody was there: the game goes to sleep.
    Hibernate,
    /// `POST /admin/games/{id}/stop`: saved and put to sleep at once.
    Admin,
    /// Nobody human was there for `empty_close_ms` (or an administrator asked): the game is closed like a finished one.
    Close,
    /// The gateway stops: the game stays `Running` on disk and is brought back at the next start.
    Drain,
}

struct Seat {
    control: SeatControl,
    civ: CivPick,
    conn: Conn,
    /// Humans have a pipeline.
    pipe: Option<SeatHandle>,
    orders: OrderBook,
    limits: SeatLimits,
    session: Option<u64>,
    link: Option<SessionLink>,
}

/// A `Kick` the worker has been asked for and has not answered yet.
struct PendingKick {
    since_ms: u64,
    /// For `safe_point_wait_ms`.
    at: std::time::Instant,
    /// The nick of the one who kicks, for the journal line (`None`: an administrator).
    by: Option<String>,
    /// The owner of the seat asked for it (`LeaveSeat`).
    own: bool,
}

struct QueryOwner {
    player: PlayerId,
    session: u64,
    id: u32,
}

/// Starts a game: its worker, its actor, its place in the registry.
pub fn spawn(env: GameEnv, init: GameInit) -> GameHandle {
    let GameInit {
        id,
        setup,
        host,
        room,
        seed,
        load,
        kicked,
        created_ms,
        mods_lock,
    } = init;
    let mods = match ModHost::build(
        id,
        Arc::clone(&env.mod_store),
        Arc::clone(&env.mod_registry),
        mods_lock.as_deref(),
    ) {
        Ok(m) => m,
        Err(why) => {
            // The lobby and the restore check this before the game starts; here it is a race (the mods changed on
            // disk meanwhile). The game runs without its mods rather than not at all.
            tracing::error!(game = id, %why, "the mods of the game cannot be set up, it runs without them");
            ModHost::empty(
                id,
                Arc::clone(&env.mod_store),
                Arc::clone(&env.mod_registry),
            )
        }
    };
    let log = match &env.state_dir {
        Some(dir) => gamelog::GameLog::open(
            dir.join("logs").join(format!("game-{id}.jsonl")),
            Arc::clone(&env.wall),
        ),
        None => gamelog::GameLog::disabled(),
    };
    let rec = match &env.state_dir {
        Some(dir) => Recorder::open(
            dir,
            id,
            &setup.settings,
            seed,
            Arc::clone(&env.wall),
            &env.recording,
        ),
        None => Recorder::disabled(),
    };
    let info = Arc::new(GameInfo::new(
        id,
        host.clone(),
        setup.settings.clone(),
        log,
        rec,
    ));
    let timer_mode: TimerMode = setup.settings.timer.clone();
    let mut saves = SaveBook::new();
    let start = match load {
        Some(l) => {
            match l.book {
                Some(book) => saves = book,
                None => saves.seed(l.file.clone(), l.turn),
            }
            Start::Load {
                file: l.file,
                seats: setup.slots.clone(),
            }
        }
        None => Start::New(setup.clone()),
    };
    let spec = GameSpec {
        game: id,
        start,
        seed,
        saves,
        seats: setup.slots.clone(),
        policy: env.policy.clone(),
        shutdown_grace: env.shutdown_grace,
        clock: Arc::clone(&env.clock),
    };
    let (sup, events) = env.supervisor.supervise(spec);
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = GameHandle {
        id,
        tx: tx.clone(),
        info: Arc::clone(&info),
    };
    env.games.insert(handle.clone());
    let wall_now = env.wall.unix_ms();
    let env_replica = Arc::clone(&env.replica);
    let actor = GameActor {
        id,
        env,
        info,
        host,
        room,
        sup,
        events,
        rx,
        tx,
        seats: BTreeMap::new(),
        catalog: None,
        waiting: Vec::new(),
        turn: None,
        ready: false,
        queries: HashMap::new(),
        core_build: String::new(),
        map: (0, 0),
        phase_info: None,
        last_exit: None,
        claims: BTreeMap::new(),
        timers: Timers::new(timer_mode),
        last_info: None,
        paused_since: None,
        kicks: BTreeMap::new(),
        give_ups: BTreeMap::new(),
        boot_at: std::time::Instant::now(),
        restart_at: None,
        cmd_times: HashMap::new(),
        reject_next: std::collections::BTreeSet::new(),
        faulted: false,
        seed,
        created_ms: created_ms.unwrap_or(wall_now),
        mods_lock,
        mods,
        mod_turn: None,
        kicked_seed: kicked,
        stopping: None,
        idle_since: None,
        pending_joins: Vec::new(),
        sent_views: Vec::new(),
        over: false,
        over_done: false,
        last_word_ms: 0,
        ai_acc: stats::AiPhaseAcc::default(),
        last_fatal: None,
        replica: ReplicaChain::new(id, Arc::clone(&env_replica)),
        rec_turn: std::cell::Cell::new(0),
        rec_last_turn: None,
    };
    tokio::spawn(actor.run());
    handle
}

struct GameActor {
    id: GameId,
    env: GameEnv,
    info: Arc<GameInfo>,
    host: String,
    room: Option<RoomId>,
    sup: GameSupervisor,
    events: mpsc::Receiver<GameEvent>,
    rx: mpsc::UnboundedReceiver<ActorMsg>,
    tx: mpsc::UnboundedSender<ActorMsg>,
    seats: BTreeMap<PlayerId, Seat>,
    catalog: Option<CatalogRef>,
    /// Attaches that came before the first `Ready`.
    waiting: Vec<AttachRequest>,
    turn: Option<TurnState>,
    ready: bool,
    queries: HashMap<u64, QueryOwner>,
    core_build: String,
    map: (u16, u16),
    phase_info: Option<(protocol::TurnPhase, protocol::TurnMode)>,
    last_exit: Option<i32>,
    /// Slots a `JoinGame` is taking: the worker has the `Claim`.
    claims: BTreeMap<PlayerId, PendingClaim>,
    /// The turn timers and the host's pause (GS-D7).
    timers: Timers,
    /// The last `TurnInfo` of the worker: `TurnState` is built from it again when a timer or the
    /// pause changes.
    last_info: Option<TurnInfo>,
    /// When the host's pause began.
    paused_since: Option<u64>,
    /// Kicks the worker has not answered.
    kicks: BTreeMap<PlayerId, PendingKick>,
    /// Seats whose owner gave them up and that change to the AI when the game goes to sleep.
    give_ups: BTreeMap<PlayerId, String>,
    seed: u64,
    created_ms: u64,
    /// The frozen mod set of the game (T15), written to `meta.json`.
    mods_lock: Option<String>,
    /// The mods of the game (T17-T19): the frozen set, the server halves, the block the worker saves.
    mods: ModHost,
    /// The turn the mods were last told about.
    mod_turn: Option<i32>,
    boot_at: std::time::Instant,
    /// When the worker last failed (the restore time is measured from here).
    restart_at: Option<std::time::Instant>,
    /// When an order was taken, by seat and id, until its answer is out (`cmd_gateway_ms`).
    cmd_times: HashMap<(PlayerId, CmdId), std::time::Instant>,
    /// Seats whose next order a test hook refuses.
    reject_next: std::collections::BTreeSet<PlayerId>,
    /// The game is `Faulted`: no worker, the seats are let go, an administrator may restart it.
    faulted: bool,
    /// Who the kicked seats belonged to, from the stored meta.
    kicked_seed: BTreeMap<PlayerId, String>,
    /// Why the worker is being stopped on purpose (`None`: it is not).
    stopping: Option<Stop>,
    /// Since when nobody has been online (`hibernate_idle_ms`).
    idle_since: Option<u64>,
    /// `JoinGame` requests that came before the worker was ready (a sleeping game that woke).
    pending_joins: Vec<JoinRequest>,
    /// The `Seats` the pipelines were sent last: a change that shows nothing sends nothing.
    sent_views: Vec<SeatView>,
    /// `TurnPhase::GameOver` came: the game is saved and `Ended`.
    over: bool,
    /// The quiet after the end is over and the seats were let go.
    over_done: bool,
    /// The last time a player said anything to the game (the quiet that closes a finished game).
    last_word_ms: u64,
    /// The AI phase under way: its steps add up, and the phase is recorded when it ends.
    ai_acc: stats::AiPhaseAcc,
    /// The newest `Fatal` of the worker: what the room is told when the game never comes up.
    last_fatal: Option<String>,
    /// The replica stream: epoch, batch numbers, which bodies are held.
    replica: ReplicaChain,
    /// The turn of the last keyframe: the human batches that follow belong to it (recording).
    rec_turn: std::cell::Cell<i32>,
    /// The last `Turn` the recording holds, to write a change only.
    rec_last_turn: Option<(i32, String, Vec<i8>)>,
}

/// Tells a connection why it cannot play (`Reject(ModMismatch)`), then ends its session once that is written.
fn refuse_mods(out: &Outbox, problems: Vec<protocol::ModProblem>) {
    let msg = ServerMsg::Reject(protocol::RejectReason::ModMismatch(problems));
    match wire::Encoder::default().frame_msg(&msg) {
        Ok(frame) => {
            let _ = out.push(frame);
        }
        Err(e) => tracing::error!(error = %e, "cannot encode a Reject"),
    }
    let _ = out.close_after_flush();
}

fn nick_eq(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn rejected(id: CmdId, code: RejectCode, reason: &str) -> CommandResult {
    CommandResult {
        id,
        outcome: CommandOutcome::Rejected {
            code,
            reason: reason.to_owned(),
        },
    }
}

fn count_result(stats: &GameStats, r: &CommandResult) {
    match &r.outcome {
        CommandOutcome::Applied => stats::bump(&stats.applied),
        CommandOutcome::Queued => stats::bump(&stats.queued),
        CommandOutcome::Lost => stats::bump(&stats.lost),
        CommandOutcome::Rejected { code, .. } => {
            stats::bump(&stats.rejected[stats::reject_index(*code)]);
        }
    }
}

/// A seat is connected: its clock may run.
fn online(seats: &BTreeMap<PlayerId, Seat>, player: PlayerId) -> bool {
    seats
        .get(&player)
        .is_some_and(|s| matches!(s.conn, Conn::Online))
}

impl GameActor {
    async fn run(mut self) {
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                ev = self.events.recv() => match ev {
                    Some(ev) => {
                        if !self.on_event(ev) {
                            break;
                        }
                    }
                    None => break,
                },
                msg = self.rx.recv() => match msg {
                    Some(m) => self.on_msg(m),
                    // `self.tx` keeps the queue open.
                    None => break,
                },
                _ = tick.tick(), if self.needs_tick() => self.on_tick(),
            }
        }
        self.finish();
        self.info.rec.finish();
    }

    /// Something waits on the clock.
    fn needs_tick(&self) -> bool {
        !self.claims.is_empty()
            || !self.kicks.is_empty()
            || self.timers.active()
            || (self.over && !self.over_done)
            || ((self.env.hibernate_idle_ms > 0 || self.env.empty_close_ms > 0)
                && self.ready
                && self.stopping.is_none())
            || self.lost_pending(self.env.clock.now_ms())
    }

    /// A lost connection that is not shown yet, or a host that is away (its rights pass in time).
    fn lost_pending(&self, now: u64) -> bool {
        self.seats.values().any(|s| {
            matches!(s.conn, Conn::Lost { since_ms } if now.saturating_sub(since_ms) < SHOW_LOST_MS)
        }) || self.host_lost_since().is_some()
            // The time to show a loss came (the clock may jump): the next tick tells the seats.
            || self.seat_views(now) != self.sent_views
    }

    fn on_tick(&mut self) {
        let now = self.env.clock.now_ms();
        self.run_timers(now);
        self.broadcast_seats();
        self.check_host(now);
        self.check_kicks(now);
        self.check_over(now);
        self.check_idle(now);
        self.serve_deferred_resyncs(now);
        // A claim that the worker does not answer: the player is told, a late answer undoes it.
        let late: Vec<PlayerId> = self
            .claims
            .iter()
            .filter(|(_, c)| !c.expired && now.saturating_sub(c.since_ms) >= CLAIM_WAIT_MS)
            .map(|(p, _)| *p)
            .collect();
        for p in late {
            if let Some(c) = self.claims.get_mut(&p) {
                c.expired = true;
                let nick = c.nick.clone();
                self.join_failed(p, &nick, "the game did not answer in time");
            }
        }
    }

    /// `TurnState` is built again from the last `TurnInfo`; `true`: it differs from the one sent.
    fn refresh_turn(&mut self) -> bool {
        let Some(info) = &self.last_info else {
            return false;
        };
        let ts = turn_state(info, &self.timers);
        if self.turn.as_ref() == Some(&ts) {
            return false;
        }
        self.turn = Some(ts);
        true
    }

    /// A change of the timers or of the pause outside a worker frame: `TurnState` goes to every
    /// human seat in a short batch of its own.
    fn push_turn(&mut self) {
        if !self.refresh_turn() {
            return;
        }
        let Some(ts) = self.turn.clone() else { return };
        let humans: Vec<PlayerId> = self
            .seats
            .iter()
            .filter(|(_, s)| s.pipe.is_some())
            .map(|(p, _)| *p)
            .collect();
        for p in humans {
            self.step(
                p,
                Parts {
                    turn: Some(ts.clone()),
                    ..Parts::default()
                },
            );
        }
    }

    /// A seat came or went: the clocks that must stand stop, the others run (N-D7).
    fn reconcile_timers(&mut self) {
        let now = self.env.clock.now_ms();
        let seats = &self.seats;
        if self.timers.reconcile(&|p| online(seats, p), now) {
            self.push_turn();
        }
    }

    /// A timer ran out: `ForceEndTurn` for the seats of its group, a line for everybody.
    fn run_timers(&mut self, now: u64) {
        for action in self.timers.poll(now) {
            match action {
                Action::Force {
                    group,
                    players,
                    attempt,
                } => {
                    let sent = self
                        .sup
                        .send(ToWorker::ForceEndTurn {
                            players: players.clone(),
                        })
                        .is_ok();
                    tracing::info!(
                        game = self.id,
                        group,
                        ?players,
                        attempt,
                        sent,
                        "the turn timer ran out"
                    );
                    self.info.log.event(
                        "timer",
                        serde_json::json!({"group": group, "players": players, "attempt": attempt, "sent": sent}),
                    );
                    for p in &players {
                        self.info.rec.seat(
                            *p,
                            "force_end_turn",
                            None,
                            format!("group {group}, attempt {attempt}, sent {sent}"),
                        );
                    }
                    if attempt == 1 {
                        stats::bump(&self.info.stats.timer_expired);
                        let names = self.names_of(&players);
                        self.system_line(format!(
                            "The turn timer ran out; the turn is ended for: {names}"
                        ));
                    }
                }
                Action::GaveUp { group, players } => {
                    tracing::error!(
                        game = self.id,
                        group,
                        ?players,
                        "the turn did not end after three requests"
                    );
                    let names = self.names_of(&players);
                    self.system_line(format!(
                        "The turn timer could not end the turn for: {names}"
                    ));
                }
            }
        }
    }

    fn names_of(&self, players: &[PlayerId]) -> String {
        players
            .iter()
            .map(|p| match self.seats.get(p).map(|s| &s.control) {
                Some(SeatControl::Human { nick }) => nick.clone(),
                _ => format!("player {p}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A `System` line in the journal of every human seat.
    fn system_line(&self, text: String) {
        let entry = JournalEntry {
            turn: self.turn.as_ref().map_or(0, |t| t.turn),
            kind: JournalKind::System,
            text,
            at: None,
            simplification: None,
        };
        for (p, seat) in &self.seats {
            if seat.pipe.is_some() {
                self.step(
                    *p,
                    Parts {
                        journal: vec![entry.clone()],
                        ..Parts::default()
                    },
                );
            }
        }
    }

    /// The seat of the host: the human whose nick is the host's.
    fn host_seat(&self) -> Option<PlayerId> {
        self.seats
            .iter()
            .find(|(_, s)| {
                matches!(&s.control, SeatControl::Human { nick } if nick_eq(nick, &self.host))
            })
            .map(|(p, _)| *p)
    }

    /// Since when the host has been without a connection.
    fn host_lost_since(&self) -> Option<u64> {
        match self.seats.get(&self.host_seat()?)?.conn {
            Conn::Lost { since_ms } => Some(since_ms),
            _ => None,
        }
    }

    /// Host controls: only the host pauses and kicks; the pause stops the timer, orders go
    /// on; `Leave` is a disconnect that the owner of the seat chose.
    fn on_game_ctl(&mut self, player: PlayerId, session: u64, ctl: GameCtl) {
        if matches!(ctl, GameCtl::Leave) {
            self.leave(player, session);
            return;
        }
        if self.host_seat() != Some(player) {
            self.reject_ctl(player, session, ctl, GameCtlReason::NotHost);
            return;
        }
        match ctl {
            GameCtl::Pause(on) => {
                let now = self.env.clock.now_ms();
                let seats = &self.seats;
                if !self.timers.set_paused(on, &|p| online(seats, p), now) {
                    return;
                }
                if on {
                    self.paused_since = Some(now);
                } else if let Some(since) = self.paused_since.take() {
                    self.info.stats.timer_paused_ms.fetch_add(
                        now.saturating_sub(since),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
                self.system_line(
                    if on {
                        "The host paused the turn timer"
                    } else {
                        "The host resumed the turn timer"
                    }
                    .to_owned(),
                );
                self.push_turn();
            }
            GameCtl::Kick { player: target } => {
                let by = self.nick_of(player);
                if let Err(reason) = self.kick(target, by, Some(player)) {
                    self.reject_ctl(player, session, ctl, reason);
                }
            }
            GameCtl::Save => {
                // B4 K20: the host's "Save game". The save is the one an administrator asks for; it lands in the
                // game's `keep`/`ring` dir and in the lobby's list of saves (`report_save`) like any other.
                let by = self.nick_of(player).unwrap_or_default();
                let _ = self.sup.send(ToWorker::SaveNow {
                    reason: SaveReason::Admin,
                });
                self.system_line(format!("{by} saved the game"));
            }
            GameCtl::Leave => {}
        }
    }

    fn nick_of(&self, player: PlayerId) -> Option<String> {
        match &self.seats.get(&player)?.control {
            SeatControl::Human { nick } => Some(nick.clone()),
            _ => None,
        }
    }

    /// Asks the worker to give the seat of `target` to the AI. `by_seat` is the
    /// host's seat: the host cannot kick itself. `Err`: why not.
    pub(crate) fn kick(
        &mut self,
        target: PlayerId,
        by: Option<String>,
        by_seat: Option<PlayerId>,
    ) -> Result<(), GameCtlReason> {
        let Some(seat) = self.seats.get(&target) else {
            return Err(GameCtlReason::NoSuchPlayer);
        };
        if !matches!(seat.control, SeatControl::Human { .. }) {
            return Err(GameCtlReason::NotHuman);
        }
        if by_seat == Some(target) {
            return Err(GameCtlReason::SelfKick);
        }
        let humans = self
            .seats
            .values()
            .filter(|s| matches!(s.control, SeatControl::Human { .. }))
            .count();
        if humans <= 1 {
            return Err(GameCtlReason::LastHuman);
        }
        if self.kicks.contains_key(&target) {
            // Asked already; the worker's answer is on its way.
            return Ok(());
        }
        if self
            .sup
            .send(ToWorker::Seat {
                player: target,
                change: SeatChange::Kick,
            })
            .is_err()
        {
            return Err(GameCtlReason::NoGame);
        }
        self.kicks.insert(
            target,
            PendingKick {
                since_ms: self.env.clock.now_ms(),
                at: std::time::Instant::now(),
                by,
                own: false,
            },
        );
        Ok(())
    }

    /// The worker did the `Kick`: the seat is the AI's, the nick is free, the lobby is told.
    fn kick_done(&mut self, player: PlayerId, error: Option<String>) {
        let Some(pending) = self.kicks.remove(&player) else {
            // The undo of a `Claim` that came too late, or a repeated answer.
            tracing::warn!(
                game = self.id,
                player,
                ?error,
                "a kick answer nobody waits for"
            );
            return;
        };
        self.info.stats.safe_point_wait.record(pending.at.elapsed());
        self.info.log.event(
            "seat_change",
            serde_json::json!({"player": player, "change": "kick", "ok": error.is_none()}),
        );
        self.info
            .rec
            .seat(player, "kick", None, format!("ok: {}", error.is_none()));
        if let Some(why) = error {
            tracing::warn!(game = self.id, player, %why, "the worker refused a kick");
            if pending.own
                && let Some(nick) = self.nick_of(player)
            {
                let _ = self.env.lobby.send(LobbyMsg::GiveUpFailed {
                    nick,
                    why: why.clone(),
                });
            }
            self.system_line(format!("The kick was refused: {why}"));
            return;
        }
        let Some(nick) = self.nick_of(player) else {
            return;
        };
        let game = self.id;
        let lobby = self.env.lobby.clone();
        let mut told = false;
        if let Some(seat) = self.seats.get_mut(&player) {
            seat.control = SeatControl::Ai {
                kicked: Some(nick.clone()),
            };
            seat.conn = Conn::None;
            seat.orders = OrderBook::new();
            if let Some(link) = seat.link.take() {
                *link
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            if let (Some(session), Some(pipe)) = (seat.session.take(), seat.pipe.take()) {
                pipe.send(SeatInput::Release {
                    session,
                    reason: DetachReason::Kicked,
                    then: Then::lobby(
                        lobby.clone(),
                        LobbyMsg::Kicked {
                            game,
                            nick: nick.clone(),
                        },
                    ),
                });
                told = true;
            }
            seat.pipe = None;
        }
        if !told {
            let _ = lobby.send(LobbyMsg::Kicked {
                game,
                nick: nick.clone(),
            });
        }
        self.forget_queries(player, None);
        self.sync_seats();
        self.write_meta(MetaStatus::Running);
        let text = match &pending.by {
            _ if pending.own => format!("{nick} gave up the seat"),
            Some(by) => format!("{by} kicked {nick}"),
            None => format!("{nick} was kicked by an administrator"),
        };
        self.session_line(text);
        // The kicked one may have been the host (an administrator's kick).
        self.fix_host();
        self.broadcast_seats();
        self.reconcile_timers();
    }

    /// `LeaveSeat`: the owner gives the seat up. With another human in the game this is a safe kick; the last
    /// human puts the game to sleep, and the slot is the AI's in the stored seats ([`Self::apply_give_ups`]).
    fn give_up(&mut self, player: PlayerId, nick: &str) {
        let fail = |this: &Self, why: &str| {
            let _ = this.env.lobby.send(LobbyMsg::GiveUpFailed {
                nick: nick.to_owned(),
                why: why.to_owned(),
            });
        };
        let owns = self
            .nick_of(player)
            .is_some_and(|n| n.eq_ignore_ascii_case(nick));
        if !owns {
            fail(self, "that seat is not yours");
            return;
        }
        let humans = self
            .seats
            .values()
            .filter(|s| matches!(s.control, SeatControl::Human { .. }))
            .count();
        if humans > 1 && self.stopping.is_none() {
            if !self.ready {
                fail(self, "the game is starting; try again in a moment");
                return;
            }
            match self.kick(player, Some(nick.to_owned()), None) {
                Ok(()) => {
                    if let Some(k) = self.kicks.get_mut(&player) {
                        k.own = true;
                    }
                }
                Err(reason) => fail(self, &format!("{reason:?}")),
            }
            return;
        }
        // The last human, or the game is going to sleep anyway: the slot changes when it sleeps.
        if self.over || !self.ready {
            fail(self, "the game is not ready; try again in a moment");
            return;
        }
        self.give_ups.insert(player, nick.to_owned());
        if self.stopping.is_none() {
            tracing::info!(
                game = self.id,
                player,
                "the last human gave up the seat: the game goes to sleep"
            );
            self.stopping = Some(Stop::Hibernate);
            self.sup.shutdown(true);
        }
    }

    /// The seats given up become the AI's (the stored seats of a game that sleeps); the lobby frees the nicks.
    fn apply_give_ups(&mut self) {
        for (player, nick) in std::mem::take(&mut self.give_ups) {
            if let Some(seat) = self.seats.get_mut(&player)
                && matches!(&seat.control, SeatControl::Human { nick: n } if *n == nick)
            {
                seat.control = SeatControl::Ai {
                    kicked: Some(nick.clone()),
                };
            }
            let _ = self.env.lobby.send(LobbyMsg::Kicked {
                game: self.id,
                nick,
            });
        }
    }

    /// Kicks the worker has not answered for a while are asked again.
    fn check_kicks(&mut self, now: u64) {
        let late: Vec<PlayerId> = self
            .kicks
            .iter()
            .filter(|(_, k)| now.saturating_sub(k.since_ms) >= KICK_WAIT_MS)
            .map(|(p, _)| *p)
            .collect();
        for p in late {
            let sent = self
                .sup
                .send(ToWorker::Seat {
                    player: p,
                    change: SeatChange::Kick,
                })
                .is_ok();
            if let Some(k) = self.kicks.get_mut(&p) {
                k.since_ms = now;
            }
            tracing::warn!(
                game = self.id,
                player = p,
                sent,
                "a kick is not done yet, asking again"
            );
            let who = self.nick_of(p).unwrap_or_else(|| format!("player {p}"));
            self.system_line(format!(
                "The kick of {who} was not carried out; trying again"
            ));
        }
    }

    /// `GameCtl::Leave` (NP-D21): a disconnect the player chose. The seat stays the player's, the turn
    /// waits for the return or for a kick; the session is told `Detached { Left }` and sees the lobby.
    fn leave(&mut self, player: PlayerId, session: u64) {
        let now = self.env.clock.now_ms();
        let game = self.id;
        let lobby = self.env.lobby.clone();
        let Some(nick) = self.nick_of(player) else {
            return;
        };
        let was_host = self.host_seat() == Some(player);
        let Some(seat) = self.seats.get_mut(&player) else {
            return;
        };
        if seat.session != Some(session) {
            return;
        }
        seat.session = None;
        seat.conn = Conn::Lost { since_ms: now };
        if let Some(link) = seat.link.take() {
            *link
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(pipe) = &seat.pipe {
            pipe.send(SeatInput::Release {
                session,
                reason: DetachReason::Left,
                then: Then::lobby(lobby, LobbyMsg::Away { game, nick }),
            });
        }
        self.forget_queries(player, Some(session));
        self.info.log.event(
            "detach",
            serde_json::json!({"player": player, "session": session, "why": "left"}),
        );
        self.info
            .rec
            .seat(player, "detach", None, format!("session {session}, left"));
        self.broadcast_seats();
        self.reconcile_timers();
        // "Return to the main menu" after the end: the host closes the finished game; so does the last one out.
        if self.over && (was_host || self.humans_gone_since().is_some()) {
            tracing::info!(game = self.id, player, "left a finished game: it is closed");
            self.close_over();
        }
    }

    /// Queries of a session (of every session of the seat when `session` is `None`) will never be read.
    fn forget_queries(&mut self, player: PlayerId, session: Option<u64>) {
        let gone: Vec<u64> = self
            .queries
            .iter()
            .filter(|(_, o)| o.player == player && session.is_none_or(|s| o.session == s))
            .map(|(q, _)| *q)
            .collect();
        for q in gone {
            self.queries.remove(&q);
            if let Some(seat) = self.seats.get_mut(&player) {
                seat.limits.query_done();
            }
        }
    }

    /// The host passes its rights on once it has been without a connection for `host_lost_ms`: to the
    /// next human online after its seat, round the table (GS-D21).
    fn check_host(&mut self, now: u64) {
        let Some(since) = self.host_lost_since() else {
            return;
        };
        let lost_for = now.saturating_sub(since);
        if lost_for < self.env.host_lost_ms {
            return;
        }
        let Some(host_seat) = self.host_seat() else {
            return;
        };
        // Nobody else is here: the rights stay and pass when somebody is.
        let Some(next) = self.next_online_human(host_seat) else {
            return;
        };
        let Some(nick) = self.nick_of(next) else {
            return;
        };
        let old = std::mem::replace(&mut self.host, nick.clone());
        let st = &self.info.stats;
        stats::bump(&st.host_changes);
        st.host_last_change_after_lost_ms
            .store(lost_for, std::sync::atomic::Ordering::Relaxed);
        tracing::info!(game = self.id, %old, new = %nick, lost_for, "the host passes on");
        self.session_line(format!(
            "{nick} is the host now: {old} was not connected for {} s",
            self.env.host_lost_ms / 1000
        ));
        self.broadcast_seats();
        self.write_meta(MetaStatus::Running);
        self.publish();
    }

    /// The next human after `from` (round the table) whose connection is up, other than `from`.
    fn next_online_human(&self, from: PlayerId) -> Option<PlayerId> {
        let ok = |s: &Seat| {
            matches!(s.control, SeatControl::Human { .. }) && matches!(s.conn, Conn::Online)
        };
        self.seats
            .iter()
            .filter(|(p, s)| **p > from && ok(s))
            .chain(self.seats.iter().filter(|(p, s)| **p < from && ok(s)))
            .map(|(p, _)| *p)
            .next()
    }

    /// The seat of the host is gone (an administrator kicked it): the rights go to somebody now.
    fn fix_host(&mut self) {
        if self.host_seat().is_some() {
            return;
        }
        let next = self
            .seats
            .iter()
            .filter(|(_, s)| matches!(s.control, SeatControl::Human { .. }))
            .min_by_key(|(_, s)| !matches!(s.conn, Conn::Online))
            .map(|(p, _)| *p)
            .and_then(|p| self.nick_of(p));
        if let Some(nick) = next {
            let old = std::mem::replace(&mut self.host, nick.clone());
            stats::bump(&self.info.stats.host_changes);
            self.session_line(format!(
                "{nick} is the host now: {old} is no longer in the game"
            ));
        }
    }

    /// A `Session` line (seat events) in the journal of every human seat.
    fn session_line(&self, text: String) {
        self.journal_line(JournalKind::Session, text);
    }

    /// One journal line for every human seat.
    fn journal_line(&self, kind: JournalKind, text: String) {
        let entry = JournalEntry {
            turn: self.turn.as_ref().map_or(0, |t| t.turn),
            kind,
            text,
            at: None,
            simplification: None,
        };
        for (p, seat) in &self.seats {
            if seat.pipe.is_some() {
                self.step(
                    *p,
                    Parts {
                        journal: vec![entry.clone()],
                        ..Parts::default()
                    },
                );
            }
        }
    }

    /// One journal line for one seat.
    fn private_line(&self, player: PlayerId, text: String) {
        let entry = JournalEntry {
            turn: self.turn.as_ref().map_or(0, |t| t.turn),
            kind: JournalKind::System,
            text,
            at: None,
            simplification: None,
        };
        self.step(
            player,
            Parts {
                journal: vec![entry],
                ..Parts::default()
            },
        );
    }

    /// The seats of the game as the supervisor must load them after a failure (the truth about who
    /// sits where).
    fn sync_seats(&self) {
        self.sup.set_seats(
            self.seats
                .iter()
                .map(|(p, s)| SlotSetup {
                    player: *p,
                    civ: s.civ.clone(),
                    control: match &s.control {
                        SeatControl::Human { nick } => SlotControl::Human { nick: nick.clone() },
                        _ => SlotControl::Ai,
                    },
                })
                .collect(),
        );
    }

    /// A chat line.
    fn on_chat(&mut self, player: PlayerId, c: ChatSend) {
        let now = self.env.clock.now_ms();
        let allowed = self
            .seats
            .get_mut(&player)
            .is_some_and(|s| s.limits.chat_ok(now));
        if !allowed {
            stats::bump(&self.info.stats.chat_dropped_rate);
            self.private_line(player, "You are sending chat lines too fast".to_owned());
            return;
        }
        let (Some(text), Some(from)) = (chat::clean(&c.text), self.nick_of(player)) else {
            return;
        };
        let humans: Vec<PlayerId> = self
            .seats
            .iter()
            .filter(|(_, s)| s.pipe.is_some() && matches!(s.control, SeatControl::Human { .. }))
            .map(|(p, _)| *p)
            .collect();
        let to = match chat::recipients(c.to, player, &humans) {
            Ok(to) => to,
            Err(why) => {
                self.private_line(player, why.to_owned());
                return;
            }
        };
        stats::bump(&self.info.stats.chat_lines);
        self.info.log.event(
            "chat_meta",
            serde_json::json!({"from": player, "to": format!("{:?}", c.to), "len": text.chars().count()}),
        );
        let line = ChatLine {
            from,
            from_player: Some(player),
            to: c.to,
            text,
            server_ms: now,
        };
        for p in to {
            self.step(
                p,
                Parts {
                    chat: vec![line.clone()],
                    ..Parts::default()
                },
            );
        }
    }

    /// Nobody online for `hibernate_idle_ms`: the game is saved and goes to sleep.
    fn check_idle(&mut self, now: u64) {
        let (sleep, close) = (self.env.hibernate_idle_ms, self.env.empty_close_ms);
        if (sleep == 0 && close == 0)
            || !self.ready
            || self.stopping.is_some()
            || self.over
            || !self.claims.is_empty()
            || self.sup.state() != GameState::Running
        {
            self.idle_since = None;
            return;
        }
        let someone = self.seats.values().any(|s| {
            matches!(s.control, SeatControl::Human { .. }) && matches!(s.conn, Conn::Online)
        });
        if someone {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert(now);
        let idle = now.saturating_sub(since);
        if close > 0 && idle >= close {
            // The user's rule: no real player for 5 minutes and the game is closed, its saves stay.
            tracing::info!(game = self.id, "nobody is here: the game is closed");
            self.close_empty();
        } else if sleep > 0 && idle >= sleep {
            tracing::info!(game = self.id, "nobody is here: the game goes to sleep");
            self.stopping = Some(Stop::Hibernate);
            self.sup.shutdown(true);
        }
    }

    /// The worker saves and stops; `finish` then writes `Finished` and lets the seats go (the nicks are free).
    fn close_empty(&mut self) {
        self.stopping = Some(Stop::Close);
        self.sup.shutdown(true);
    }

    /// What is kept of the game on disk now.
    fn meta(&self, status: MetaStatus) -> GameMeta {
        GameMeta {
            id: self.id,
            name: self.info.name.clone(),
            status,
            settings: self.info.settings.clone(),
            host: self.host.clone(),
            seats: self
                .seats
                .iter()
                .map(|(p, s)| MetaSeat::of(*p, &s.civ, &s.control))
                .collect(),
            seed: self.seed,
            core_build: self.core_build.clone(),
            created_ms: self.created_ms,
            finished_ms: (status == MetaStatus::Finished).then(|| self.env.wall.unix_ms()),
            turn: self.turn.as_ref().map_or(0, |t| t.turn),
            mods_lock: self.mods_lock.clone(),
        }
    }

    /// Writes `games/<id>/meta.json` (atomic); a game whose seats are not known yet has none.
    fn write_meta(&self, status: MetaStatus) -> GameMeta {
        let meta = self.meta(status);
        if self.ready
            && let Some(dir) = &self.env.state_dir
            && let Err(e) = meta.write(&dir.join("games").join(self.id.to_string()))
        {
            tracing::error!(game = self.id, error = %format!("{e:#}"), "cannot write the game meta");
        }
        meta
    }

    /// The game is over (`TurnPhase::GameOver`): saved, `Ended`; the seats may look at the last view
    /// until it is quiet for a while, then the worker stops.
    fn check_over(&mut self, now: u64) {
        if !self.over || self.over_done {
            return;
        }
        let quiet = now.saturating_sub(self.last_word_ms) >= OVER_QUIET_MS;
        // Nobody is left to look at the last view (everybody went to the lobby or dropped): close it soon.
        let empty = self
            .humans_gone_since()
            .is_some_and(|since| now.saturating_sub(since) >= OVER_EMPTY_MS);
        if quiet || empty {
            self.close_over();
        }
    }

    /// The game is over for good: the seats are let go, the worker stops, and `finish` marks the game `Finished`.
    fn close_over(&mut self) {
        if self.over_done {
            return;
        }
        self.over_done = true;
        self.release_all(DetachReason::GameOver);
        self.sup.shutdown(true);
    }

    /// Since when no human has been online: `None` while one is. A human who never connected counts from 0.
    fn humans_gone_since(&self) -> Option<u64> {
        let mut since = 0;
        for s in self.seats.values() {
            if !matches!(s.control, SeatControl::Human { .. }) {
                continue;
            }
            match s.conn {
                Conn::Online => return None,
                Conn::Lost { since_ms } => since = since.max(since_ms),
                Conn::None => {}
            }
        }
        Some(since)
    }

    /// `GameCtlRejected` to the session that asked; it is no part of the stream.
    fn reject_ctl(&mut self, player: PlayerId, session: u64, ctl: GameCtl, reason: GameCtlReason) {
        stats::bump(&self.info.stats.gamectl_rejected[stats::ctl_reason_index(reason)]);
        if let Some(pipe) = self.seats.get(&player).and_then(|s| s.pipe.as_ref()) {
            pipe.send(SeatInput::Control {
                session,
                msg: ServerMsg::GameCtlRejected { ctl, reason },
            });
        }
    }

    fn join_failed(&self, player: PlayerId, nick: &str, why: &str) {
        let _ = self.env.lobby.send(LobbyMsg::JoinFailed {
            game: self.id,
            player,
            nick: nick.to_owned(),
            why: why.to_owned(),
        });
    }

    /// Every slot being taken fails (the worker went away): pipelines of the claims are dropped.
    fn drop_claims(&mut self, why: &str) {
        for (player, c) in std::mem::take(&mut self.claims) {
            if let Some(seat) = self.seats.get_mut(&player) {
                seat.pipe = None;
            }
            if !c.expired {
                self.join_failed(player, &c.nick, why);
            }
        }
    }

    /// The worker is gone for good: a game that was put to sleep stays on disk, a finished or failed one
    /// lets its players go, the registry forgets it either way.
    fn finish(&mut self) {
        if self.faulted {
            // Already let go (`fault_out`); only the registry is left.
            self.env.games.remove(self.id);
            return;
        }
        self.drop_claims("the game stopped");
        for req in std::mem::take(&mut self.pending_joins) {
            self.join_failed(req.player, &req.nick, "the game stopped");
        }
        let state = self.sup.state();
        if self.stopping == Some(Stop::Drain) && state == GameState::Finished {
            // The gateway stops: the sockets are closed with 1012 by the gateway itself; the game stays
            // `Running` on disk and comes back at the next start.
            self.apply_give_ups();
            self.write_meta(MetaStatus::Running);
            self.publish();
            self.env.games.remove(self.id);
            return;
        }
        let closed = self.over || self.stopping == Some(Stop::Close);
        let asleep = self.stopping.is_some() && !closed && state == GameState::Finished;
        if asleep {
            let why = match self.stopping {
                Some(Stop::Admin) => "the game was stopped by an administrator",
                _ => "the game went to sleep",
            };
            self.release_all_away(DetachReason::GameStopped {
                why: why.to_owned(),
            });
            self.publish();
            self.env.games.remove(self.id);
            self.apply_give_ups();
            if self.ready {
                let meta = self.write_meta(MetaStatus::Hibernating);
                self.env.games.add_dormant(meta);
            }
            let _ = self.env.lobby.send(LobbyMsg::GamesChanged);
            return;
        }
        let nicks: Vec<String> = self
            .seats
            .values()
            .filter_map(|s| match &s.control {
                SeatControl::Human { nick } => Some(nick.clone()),
                _ => None,
            })
            .collect();
        self.release_all(DetachReason::GameStopped {
            why: if state == GameState::Faulted {
                "the game worker failed too often".to_owned()
            } else if self.stopping == Some(Stop::Close) {
                "the game was closed".to_owned()
            } else {
                "the game stopped".to_owned()
            },
        });
        if state == GameState::Faulted {
            self.write_meta(MetaStatus::Faulted);
        } else if closed {
            self.write_meta(MetaStatus::Finished);
        }
        self.publish();
        self.env.games.remove(self.id);
        let _ = self.env.lobby.send(LobbyMsg::GameEnded {
            game: self.id,
            nicks,
        });
    }

    /// Like `release_all`, but the players keep their seats: each one sees the lobby after `Detached`.
    fn release_all_away(&mut self, reason: DetachReason) {
        let game = self.id;
        let lobby = self.env.lobby.clone();
        let nicks: BTreeMap<PlayerId, String> = self
            .seats
            .iter()
            .filter_map(|(p, s)| match &s.control {
                SeatControl::Human { nick } => Some((*p, nick.clone())),
                _ => None,
            })
            .collect();
        for (p, seat) in &mut self.seats {
            if let (Some(session), Some(pipe)) = (seat.session.take(), &seat.pipe) {
                let then = nicks.get(p).map_or_else(Then::default, |nick| {
                    Then::lobby(
                        lobby.clone(),
                        LobbyMsg::Away {
                            game,
                            nick: nick.clone(),
                        },
                    )
                });
                pipe.send(SeatInput::Release {
                    session,
                    reason: reason.clone(),
                    then,
                });
            }
            if let Some(link) = seat.link.take() {
                *link
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            seat.conn = Conn::None;
        }
    }

    fn release_all(&mut self, reason: DetachReason) {
        for seat in self.seats.values_mut() {
            if let (Some(session), Some(pipe)) = (seat.session.take(), &seat.pipe) {
                pipe.send(SeatInput::Release {
                    session,
                    reason: reason.clone(),
                    then: Then::default(),
                });
            }
            if let Some(link) = seat.link.take() {
                *link
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
            seat.conn = Conn::None;
        }
    }

    // ------------------------------------------------------------ events

    /// `false`: the game ends.
    fn on_event(&mut self, ev: GameEvent) -> bool {
        match ev {
            GameEvent::Started(info) => self.on_started(info),
            GameEvent::Frame(frame) => self.on_frame(frame),
            GameEvent::Rejected { player, id } => {
                // The order raced a failure: it never reached a worker.
                self.answer(
                    player,
                    rejected(id, RejectCode::Restarting, "the game is restarting"),
                );
            }
            GameEvent::Restarting(info) => self.on_restarting(&info),
            GameEvent::Faulted(info) => {
                self.on_faulted(&info);
                if !self.ready {
                    // A game that never came up (a save that does not load): the room is back.
                    let _ = self.env.lobby.send(LobbyMsg::GameFailed {
                        room: self.room,
                        why: self.failure_text(&info.why),
                    });
                    return false;
                }
                // A game that played stays: an administrator may restart it from its newest save.
                self.fault_out();
            }
            GameEvent::StartRefused { why } => {
                tracing::warn!(game = self.id, %why, "the game did not start");
                let _ = self.env.lobby.send(LobbyMsg::GameFailed {
                    room: self.room,
                    why,
                });
                // Nobody sits in a game that never started.
                self.seats.clear();
                return false;
            }
            GameEvent::Finished { exit } => {
                self.last_exit = exit.and_then(|e| e.code);
                return false;
            }
        }
        self.publish();
        true
    }

    /// What a room that never got its game is told: the supervisor's reason and the worker's last word.
    fn failure_text(&self, why: &str) -> String {
        match &self.last_fatal {
            Some(f) => format!("{why} ({f})"),
            None => why.to_owned(),
        }
    }

    fn on_started(&mut self, info: StartedInfo) {
        stats::bump(&self.info.stats.worker_starts);
        let StartedInfo {
            ready,
            catalog,
            mods: loaded_mods,
            restart,
            pid,
        } = info;
        let st = &self.info.stats;
        st.worker_pid
            .store(pid.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.restart_at.take() {
            st.note_restart(u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX));
        } else if st.boot_ms.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            st.boot_ms.store(
                u64::try_from(self.boot_at.elapsed().as_millis()).unwrap_or(u64::MAX),
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        self.info.log.event(
            "worker_start",
            serde_json::json!({"pid": pid, "turn": ready.turn, "restart": restart.is_some()}),
        );
        if self.info.rec.is_on() {
            self.info
                .rec
                .put(recording::Body::Header(recording::format::HeaderRec {
                    format: recording::format::VERSION,
                    game: self.id,
                    gateway: crate::BUILD.to_owned(),
                    core_build: ready.build_id.clone(),
                    seed: self.seed,
                    wall_at_mono0_ms: 0,
                    created_ms: self.created_ms,
                    name: self.info.name.clone(),
                    settings: Some(self.info.settings.clone()),
                    seats: ready.seats.clone(),
                }));
            self.info.rec.worker(recording::format::WorkerEv::Start {
                pid,
                turn: ready.turn,
                restart: restart.is_some(),
            });
        }
        if let Some((hash, bytes)) = &catalog {
            self.catalog = Some(CatalogRef {
                hash: *hash,
                len: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
            });
        }
        self.core_build = ready.build_id.clone();
        self.map = (ready.width, ready.height);
        // The server halves of the mods start from the state of the save this worker loaded (a worker that restarts
        // from an older save rewinds them with it); a new game starts them fresh.
        self.start_mods(loaded_mods);
        if !self.ready {
            self.create_seats(&ready.seats);
            self.ready = true;
            let seats = self
                .seats
                .iter()
                .filter_map(|(p, s)| match &s.control {
                    SeatControl::Human { nick } => Some((*p, nick.clone())),
                    _ => None,
                })
                .collect();
            let _ = self.env.lobby.send(LobbyMsg::GameStarted {
                room: self.room,
                game: self.id,
                seats,
            });
            for req in std::mem::take(&mut self.waiting) {
                self.attach(req);
            }
            self.write_meta(MetaStatus::Running);
            self.sync_seats();
            for req in std::mem::take(&mut self.pending_joins) {
                self.join(req);
            }
        }
        // A game that was `Faulted` and is restarted by an administrator: its players are seated again.
        if std::mem::take(&mut self.faulted) {
            let seats = self
                .seats
                .iter()
                .filter_map(|(p, s)| match &s.control {
                    SeatControl::Human { nick } => Some((*p, nick.clone())),
                    _ => None,
                })
                .collect();
            let _ = self.env.lobby.send(LobbyMsg::GameStarted {
                room: None,
                game: self.id,
                seats,
            });
            self.write_meta(MetaStatus::Running);
        }
        // After a restart the mods may have been rewound: every seat that is there gets their snapshot again.
        if restart.is_some() {
            self.resend_mod_snapshots();
        }
        // After a restart: the first `Full` of each seat follows as a keyframe in the same stream;
        // the player is told what the restart cost (AC-N1.5).
        if let Some(r) = restart {
            for (player, text) in r.journal {
                let entry = JournalEntry {
                    turn: ready.turn,
                    kind: JournalKind::System,
                    text,
                    at: None,
                    simplification: None,
                };
                self.step(
                    player,
                    Parts {
                        journal: vec![entry],
                        ..Parts::default()
                    },
                );
            }
        }
    }

    fn create_seats(&mut self, slots: &[SlotSetup]) {
        for s in slots {
            let control = match &s.control {
                SlotControl::Human { nick } => SeatControl::Human { nick: nick.clone() },
                SlotControl::Ai => SeatControl::Ai {
                    kicked: self.kicked_seed.get(&s.player).cloned(),
                },
            };
            let pipe = matches!(control, SeatControl::Human { .. }).then(|| {
                seat::spawn(
                    self.id,
                    s.player,
                    self.info.seat_stats.seat(s.player),
                    self.tx.clone(),
                    Arc::clone(&self.env.clock),
                    self.info.log.clone(),
                    self.info.rec.clone(),
                )
            });
            self.seats.insert(
                s.player,
                Seat {
                    control,
                    civ: s.civ.clone(),
                    conn: Conn::None,
                    pipe,
                    orders: OrderBook::new(),
                    limits: SeatLimits::new(&self.env.limits, self.env.clock.now_ms()),
                    session: None,
                    link: None,
                },
            );
        }
    }

    /// One worker frame becomes one step per seat: what the seat gets is in one batch.
    fn on_frame(&mut self, frame: WorkerFrame) {
        let mut parts: BTreeMap<PlayerId, Parts> = BTreeMap::new();
        let mut turn_changed = false;
        for msg in frame.msgs {
            match msg {
                WorkerMsg::Basis { players, basis } => {
                    for p in players {
                        if self.has_pipe(p) {
                            parts.entry(p).or_default().basis = Some(basis.clone());
                        }
                    }
                }
                WorkerMsg::View { player, update } => {
                    if self.has_pipe(player) {
                        parts.entry(player).or_default().view = Some(update);
                    }
                }
                WorkerMsg::Results { player, results } => {
                    if let Some(seat) = self.seats.get_mut(&player) {
                        for r in &results {
                            seat.orders.finish(r);
                            count_result(&self.info.stats, r);
                        }
                    }
                    for r in &results {
                        self.note_result(player, r);
                    }
                    if self.has_pipe(player) {
                        parts.entry(player).or_default().results.extend(results);
                    }
                }
                WorkerMsg::Journal { player, entries } => {
                    if self.has_pipe(player) {
                        parts.entry(player).or_default().journal.extend(entries);
                    }
                }
                WorkerMsg::Context { player, rev, ctx } => {
                    if self.has_pipe(player) {
                        parts.entry(player).or_default().contexts.push((rev, ctx));
                    }
                }
                WorkerMsg::ReplicaBlob { hash, bytes } => {
                    let st = &self.info.stats.replica;
                    if self.replica.blob(hash, bytes) {
                        stats::bump(&st.blobs);
                    } else {
                        stats::bump(&st.blobs_bad);
                        tracing::error!(
                            game = self.id,
                            "the worker sent a replica body with a wrong hash"
                        );
                    }
                }
                WorkerMsg::ReplicaJournal {
                    epoch,
                    first_jseq,
                    batches,
                } => {
                    let st = &self.info.stats.replica;
                    if self.replica.journal(epoch, first_jseq, batches.len()) {
                        let bytes: usize = batches.iter().map(Vec::len).sum();
                        st.batches
                            .fetch_add(batches.len() as u64, std::sync::atomic::Ordering::Relaxed);
                        st.batch_bytes
                            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
                        if self.info.rec.is_on() {
                            for (i, b) in batches.iter().enumerate() {
                                self.info.rec.journal(recording::format::JournalRec {
                                    phase: protocol::RecPhase::Human,
                                    epoch,
                                    jseq: first_jseq + i as u64,
                                    turn: self.rec_turn.get(),
                                    bytes: b.clone(),
                                });
                            }
                        }
                        let msg = GameMsg::GcJournal(GcJournal {
                            epoch,
                            first_jseq,
                            batches,
                        });
                        self.gc_all(&mut parts, &msg);
                    } else {
                        stats::bump(&st.batches_dropped);
                        tracing::warn!(
                            game = self.id,
                            epoch,
                            first_jseq,
                            "a journal batch does not fit the replica chain, dropped"
                        );
                    }
                }
                WorkerMsg::ReplicaBase {
                    epoch,
                    turn,
                    jseq,
                    raw_hash,
                    snapshot,
                    for_player,
                } => {
                    let st = &self.info.stats.replica;
                    st.last_snapshot_bytes.store(
                        u64::from(snapshot.size),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    self.replica.base(epoch, jseq, for_player.is_some());
                    if for_player.is_none() {
                        self.rec_keyframe(turn, epoch, jseq, raw_hash, &snapshot, None, true);
                    }
                    let msg = GameMsg::GcBase(GcBase {
                        epoch,
                        turn,
                        jseq,
                        raw_hash,
                        snapshot,
                    });
                    match for_player {
                        Some(p) => {
                            stats::bump(&st.resync_bases);
                            if self.has_pipe(p) {
                                parts.entry(p).or_default().gc.push(msg);
                            }
                        }
                        None => {
                            stats::bump(&st.bases);
                            self.gc_all(&mut parts, &msg);
                        }
                    }
                }
                WorkerMsg::ReplicaTurn {
                    epoch,
                    turn,
                    jseq,
                    raw_hash,
                    snapshot,
                    patch,
                } => {
                    let fwd_t0 = self.info.trace.now_us();
                    self.info.trace.note_blob(snapshot.hash, turn - 1);
                    if let Some(p) = &patch {
                        self.info.trace.note_blob(p.blob.hash, turn - 1);
                    }
                    let st = &self.info.stats.replica;
                    stats::bump(&st.turns);
                    st.last_snapshot_bytes.store(
                        u64::from(snapshot.size),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    if let Some(p) = &patch {
                        stats::bump(&st.turns_with_patch);
                        st.last_patch_bytes
                            .store(u64::from(p.blob.size), std::sync::atomic::Ordering::Relaxed);
                    }
                    self.replica.turn(epoch, jseq);
                    self.rec_keyframe(
                        turn,
                        epoch,
                        jseq,
                        raw_hash,
                        &snapshot,
                        patch.as_ref(),
                        false,
                    );
                    let msg = GameMsg::GcTurn(GcTurn {
                        epoch,
                        turn,
                        jseq,
                        raw_hash,
                        snapshot,
                        patch,
                    });
                    self.gc_all(&mut parts, &msg);
                    let seats = i64::try_from(parts.len()).unwrap_or(0);
                    self.info
                        .trace
                        .span(turn - 1, "g.turn_fwd", "io", fwd_t0, &[("seats", seats)]);
                }
                WorkerMsg::Trace { turn, bytes } => {
                    if let Some(s) = self.info.trace.on_worker_trace(turn, &bytes) {
                        tracing::info!(
                            game = self.id,
                            turn,
                            total_ms = s.total_us / 1000,
                            ai_ms = s.server_ai_us / 1000,
                            post_ms = s.server_post_us / 1000,
                            other_ms = s.other_us / 1000,
                            "turn timeline"
                        );
                    }
                }
                WorkerMsg::AiTurn { turn, events } => {
                    // SIMP-exe-820: the AI civilizations the core started and ended, to every seat as they come.
                    self.gc_all(&mut parts, &GameMsg::AiTurn(AiTurn { turn, events }));
                }
                WorkerMsg::RecJournal { phase, turn, bytes } => {
                    self.info.rec.journal(recording::format::JournalRec {
                        phase,
                        epoch: self.replica.epoch(),
                        jseq: 0,
                        turn,
                        bytes,
                    });
                }
                WorkerMsg::RecMark { turn, what, begin } => {
                    self.info.rec.worker(if begin {
                        recording::format::WorkerEv::CallBegin { turn, what }
                    } else {
                        recording::format::WorkerEv::CallEnd { turn, what }
                    });
                }
                WorkerMsg::NetRejected { player, reason } => {
                    stats::bump(&self.info.stats.replica.net_rejected);
                    tracing::info!(game = self.id, player, %reason, "the core refused a UI message");
                    if self.has_pipe(player) {
                        self.private_line(player, format!("A UI message was refused: {reason}"));
                    }
                }
                WorkerMsg::QueryResult { qid, answer } => self.on_query_result(qid, answer),
                WorkerMsg::Turn(info) => {
                    let now = self.env.clock.now_ms();
                    let seats = &self.seats;
                    let u = self.timers.update(&info, &|p| online(seats, p), now);
                    let st = &self.info.stats;
                    st.timer_started
                        .fetch_add(u64::from(u.started), std::sync::atomic::Ordering::Relaxed);
                    st.timer_extended
                        .fetch_add(u64::from(u.extended), std::sync::atomic::Ordering::Relaxed);
                    self.phase_info = Some((info.phase, info.mode));
                    self.rec_turn_state(&info);
                    if matches!(info.phase, TurnPhase::GameOver { .. }) && !self.over {
                        self.over = true;
                        self.last_word_ms = now;
                        let _ = self.sup.send(ToWorker::SaveNow {
                            reason: SaveReason::Shutdown,
                        });
                        self.system_line("The game is over".to_owned());
                    }
                    self.note_mod_turn(info.turn);
                    self.last_info = Some(info);
                    turn_changed |= self.refresh_turn();
                }
                WorkerMsg::Saved(rec) => {
                    stats::bump(&self.info.stats.saves_ok);
                    self.report_save(&rec);
                }
                WorkerMsg::SaveFailed { reason } => {
                    tracing::warn!(game = self.id, %reason, "the worker could not save");
                    stats::bump(&self.info.stats.saves_failed);
                }
                WorkerMsg::Stats(s) => {
                    let st = &self.info.stats;
                    st.fog_violations.fetch_add(
                        u64::from(s.fog_violations),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    let total = u64::from(s.apply_us)
                        + u64::from(s.pump_us)
                        + u64::from(s.views_us)
                        + u64::from(s.diff_us)
                        + u64::from(s.ctx_us)
                        + u64::from(s.save_us);
                    st.step.record_us(total);
                    if let Some(us) = self.ai_acc.step(s.kind, u64::from(s.ai_us)) {
                        st.ai_phase.record_us(us);
                    }
                }
                WorkerMsg::SeatDone {
                    player,
                    change,
                    error,
                } => self.on_seat_done(player, change, error),
                WorkerMsg::Fatal { message, code } => {
                    tracing::error!(game = self.id, code, %message, "the worker reports a fatal error");
                    self.info.rec.worker(recording::format::WorkerEv::Fatal {
                        message: message.clone(),
                        code,
                    });
                    self.last_fatal = Some(message);
                }
                WorkerMsg::Ready(_)
                | WorkerMsg::Catalog { .. }
                | WorkerMsg::Choices(_)
                // Read by the supervisor before `Ready` (`StartedInfo.mods`); never mid-game.
                | WorkerMsg::ModsLoaded(_)
                | WorkerMsg::Heartbeat { .. } => {}
            }
        }
        if turn_changed && let Some(ts) = &self.turn {
            let humans: Vec<PlayerId> = self
                .seats
                .iter()
                .filter(|(_, s)| s.pipe.is_some())
                .map(|(p, _)| *p)
                .collect();
            for p in humans {
                parts.entry(p).or_default().turn = Some(ts.clone());
            }
        }
        for (p, part) in parts {
            self.step(p, part);
        }
    }

    /// The turn state, written when the turn, the phase or the seats that ended the turn change.
    fn rec_turn_state(&mut self, info: &TurnInfo) {
        if !self.info.rec.is_on() {
            return;
        }
        let phase = format!("{:?}", info.phase);
        let finished: Vec<i8> = info
            .seats
            .iter()
            .filter(|s| s.finished)
            .map(|s| s.player)
            .collect();
        let key = (info.turn, phase.clone(), finished.clone());
        if self.rec_last_turn.as_ref() == Some(&key) {
            return;
        }
        self.rec_last_turn = Some(key);
        self.info
            .rec
            .put(recording::Body::Turn(recording::format::TurnRec {
                turn: info.turn,
                phase,
                mode: format!("{:?}", info.mode),
                finished,
            }));
    }

    /// The snapshot of the start of a human phase goes into the recording; the bodies are the ones the replica store
    /// holds (the worker's own packing, no second compression).
    #[allow(clippy::too_many_arguments)]
    fn rec_keyframe(
        &self,
        turn: i32,
        epoch: u32,
        jseq: u64,
        raw_hash: u64,
        snapshot: &protocol::GcBlobRef,
        patch: Option<&protocol::GcPatchRef>,
        base: bool,
    ) {
        self.rec_turn.set(turn);
        if !self.info.rec.is_on() {
            return;
        }
        let Some(body) = self.env.replica.get(snapshot.hash) else {
            self.info
                .rec
                .worker(recording::format::WorkerEv::Note(format!(
                    "turn {turn}: the snapshot body {:016x} is not in the store, no keyframe",
                    snapshot.hash
                )));
            return;
        };
        let patch = patch.and_then(|p| {
            self.env
                .replica
                .get(p.blob.hash)
                .map(|b| (p.blob.hash, p.from_raw_hash, b))
        });
        self.info.rec.keyframe(KeyframeArgs {
            turn,
            epoch,
            jseq,
            raw_hash,
            snapshot: (snapshot.hash, body),
            patch,
            base,
        });
    }

    /// A replica stream message for every human seat.
    fn gc_all(&self, parts: &mut BTreeMap<PlayerId, Parts>, msg: &GameMsg) {
        for (p, seat) in &self.seats {
            if seat.pipe.is_some() {
                parts.entry(*p).or_default().gc.push(msg.clone());
            }
        }
    }

    fn has_pipe(&self, p: PlayerId) -> bool {
        self.seats.get(&p).is_some_and(|s| s.pipe.is_some())
    }

    fn step(&self, player: PlayerId, parts: Parts) {
        if parts.is_empty() {
            return;
        }
        if let Some(pipe) = self.seats.get(&player).and_then(|s| s.pipe.as_ref()) {
            pipe.send(SeatInput::Step(Box::new(parts)));
        }
    }

    /// A short batch with results of the gateway's own (repeats, refusals, `Lost`).
    fn send_results(&self, player: PlayerId, results: Vec<CommandResult>) {
        self.step(
            player,
            Parts {
                results,
                ..Parts::default()
            },
        );
    }

    /// A final answer the gateway gives itself: counted, remembered, sent.
    fn answer(&mut self, player: PlayerId, r: CommandResult) {
        if let Some(seat) = self.seats.get_mut(&player) {
            seat.orders.finish(&r);
        }
        count_result(&self.info.stats, &r);
        self.note_result(player, &r);
        self.send_results(player, vec![r]);
    }

    /// Every order without an answer gets `Lost`; queries in flight are forgotten.
    fn lose_pending(&mut self) {
        let players: Vec<PlayerId> = self.seats.keys().copied().collect();
        for p in players {
            let ids = match self.seats.get_mut(&p) {
                Some(seat) => seat.orders.lose_pending(),
                None => continue,
            };
            if ids.is_empty() {
                continue;
            }
            let results: Vec<CommandResult> = ids
                .into_iter()
                .map(|id| CommandResult {
                    id,
                    outcome: CommandOutcome::Lost,
                })
                .collect();
            for r in &results {
                count_result(&self.info.stats, r);
            }
            self.send_results(p, results);
        }
        for (_, owner) in self.queries.drain() {
            if let Some(seat) = self.seats.get_mut(&owner.player) {
                seat.limits.query_done();
            }
        }
    }

    fn on_restarting(&mut self, info: &RestartingInfo) {
        stats::bump(&self.info.stats.worker_crashes);
        self.last_exit = info.exit.and_then(|e| e.code);
        self.restart_at = Some(std::time::Instant::now());
        self.info
            .stats
            .worker_pid
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let hung = matches!(info.failure, Some(crate::supervisor::policy::Failure::Hang));
        if hung {
            stats::bump(&self.info.stats.hang_kills);
        }
        self.info.log.event(
            if hung { "worker_hang" } else { "worker_exit" },
            serde_json::json!({
                "failure": info.failure.map(|f| format!("{f:?}")),
                "code": info.exit.and_then(|e| e.code),
                "killed": info.exit.is_some_and(|e| e.killed),
                "lost": info.lost.iter().map(|(p, ids)| (p, ids.len())).collect::<Vec<_>>(),
            }),
        );
        self.info.rec.worker(recording::format::WorkerEv::Exit {
            hang: hung,
            failure: info.failure.map(|f| format!("{f:?}")),
            code: info.exit.and_then(|e| e.code),
            killed: info.exit.is_some_and(|e| e.killed),
            lost: info
                .lost
                .iter()
                .map(|(p, ids)| (*p, u32::try_from(ids.len()).unwrap_or(u32::MAX)))
                .collect(),
        });
        self.lose_pending();
        self.replica.reset();
        self.drop_claims("the game is restarting");
        // A new worker starts the timers of the turn afresh.
        self.timers.reset();
        for seat in self.seats.values() {
            if let Some(pipe) = &seat.pipe {
                pipe.send(SeatInput::WorkerLost);
            }
        }
    }

    fn on_faulted(&mut self, info: &FaultedInfo) {
        stats::bump(&self.info.stats.worker_crashes);
        self.env.games.note_faulted();
        tracing::error!(game = self.id, why = %info.why, "the game is faulted");
        self.info.log.event(
            "fault",
            serde_json::json!({"why": info.why, "failure": info.failure.map(|f| format!("{f:?}"))}),
        );
        self.info.rec.worker(recording::format::WorkerEv::Fault {
            why: info.why.clone(),
        });
        self.info
            .stats
            .worker_pid
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.lose_pending();
        self.drop_claims("the game failed");
    }

    /// A game that played and is `Faulted`: the players are let go (the lobby frees their nicks), the
    /// game stays for an administrator's restart.
    fn fault_out(&mut self) {
        self.faulted = true;
        let nicks: Vec<String> = self
            .seats
            .values()
            .filter_map(|s| match &s.control {
                SeatControl::Human { nick } => Some(nick.clone()),
                _ => None,
            })
            .collect();
        self.release_all(DetachReason::GameStopped {
            why: "the game worker failed too often".to_owned(),
        });
        self.write_meta(MetaStatus::Faulted);
        self.publish();
        let _ = self.env.lobby.send(LobbyMsg::GameEnded {
            game: self.id,
            nicks,
        });
    }

    /// The final answer to an order of the seat: how long the gateway had it, a line in the journal.
    fn note_result(&mut self, player: PlayerId, r: &CommandResult) {
        if self.info.rec.is_on() {
            let (outcome, code, reason) = match &r.outcome {
                CommandOutcome::Applied => ("Applied", None, None),
                CommandOutcome::Queued => ("Queued", None, None),
                CommandOutcome::Lost => ("Lost", None, None),
                CommandOutcome::Rejected { code, reason } => {
                    ("Rejected", Some(format!("{code:?}")), Some(reason.clone()))
                }
            };
            self.info
                .rec
                .put(recording::Body::Result(recording::format::ResultRec {
                    player,
                    id: r.id,
                    outcome: outcome.to_owned(),
                    code,
                    reason,
                }));
        }
        if matches!(r.outcome, CommandOutcome::Queued) {
            return;
        }
        if let Some(t) = self.cmd_times.remove(&(player, r.id)) {
            self.info.stats.cmd_gateway.record(t.elapsed());
            self.info.rec.order_span(
                player,
                r.id,
                u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX),
            );
        }
        let outcome = match &r.outcome {
            CommandOutcome::Applied => "Applied".to_owned(),
            CommandOutcome::Lost => "Lost".to_owned(),
            CommandOutcome::Rejected { code, .. } => format!("Rejected:{code:?}"),
            CommandOutcome::Queued => return,
        };
        let mut ev = serde_json::json!({"player": player, "id": r.id, "outcome": outcome});
        if let CommandOutcome::Rejected { code, reason } = &r.outcome {
            // The text of the core: a refused end of turn names its blocker (NP28).
            ev["reason"] = serde_json::json!(reason);
            if *code == protocol::RejectCode::Core {
                tracing::info!(
                    game = self.id,
                    player,
                    id = r.id,
                    "the core refused an order: {reason}"
                );
            }
        }
        self.info.log.event("result", ev);
    }

    fn on_query_result(&mut self, qid: u64, answer: protocol::QueryAnswer) {
        let Some(owner) = self.queries.remove(&qid) else {
            return;
        };
        let Some(seat) = self.seats.get_mut(&owner.player) else {
            return;
        };
        seat.limits.query_done();
        if let Some(pipe) = &seat.pipe {
            pipe.send(SeatInput::Control {
                session: owner.session,
                msg: ServerMsg::QueryResult {
                    id: owner.id,
                    result: answer,
                },
            });
        }
    }

    // ------------------------------------------------------------ requests

    fn on_msg(&mut self, msg: ActorMsg) {
        match msg {
            ActorMsg::Attach(req) => self.attach(req),
            ActorMsg::SessionGone { player, session }
            | ActorMsg::SessionLost { player, session } => self.session_gone(player, session),
            ActorMsg::WantFull { player, basis } => {
                let _ = self.sup.send(ToWorker::WantFull { player, basis });
            }
            ActorMsg::FreshAttached { player, session } => self.mod_snapshot(player, session),
            ActorMsg::Client {
                player,
                session,
                msg,
                bytes,
            } => self.client(player, session, msg, bytes),
            ActorMsg::AdminKick { player, reply } => {
                let r = self.kick(player, None, None);
                let _ = reply.send(r);
            }
            ActorMsg::Fault { player, fault } => match fault {
                Fault::RejectNextCmd => {
                    self.reject_next.insert(player);
                }
                Fault::Seat(f) => {
                    if let Some(pipe) = self.seats.get(&player).and_then(|s| s.pipe.as_ref()) {
                        pipe.send(SeatInput::Fault(f));
                    }
                }
            },
            ActorMsg::Join(req) => self.join(req),
            ActorMsg::GiveUp { player, nick } => self.give_up(player, &nick),
            ActorMsg::Admin(ctl) => match ctl {
                AdminCtl::Save => {
                    let _ = self.sup.send(ToWorker::SaveNow {
                        reason: SaveReason::Admin,
                    });
                }
                AdminCtl::Stop => {
                    self.stopping = Some(Stop::Admin);
                    self.sup.shutdown(true);
                }
                AdminCtl::Close => self.close_empty(),
                AdminCtl::Restart => self.sup.restart(),
                AdminCtl::Drain => {
                    self.stopping = Some(Stop::Drain);
                    let _ = self.sup.send(ToWorker::SaveNow {
                        reason: SaveReason::Shutdown,
                    });
                    self.sup.shutdown(true);
                }
            },
        }
        self.publish();
    }

    fn attach(&mut self, req: AttachRequest) {
        if !self.ready {
            // The same session may ask twice (its login, then the lobby's start message): the last one counts.
            self.waiting
                .retain(|r| !(r.player == req.player && r.session == req.session));
            self.waiting.push(req);
            return;
        }
        let AttachRequest {
            player,
            nick,
            session,
            out,
            link,
            resume,
            claim,
        } = req;
        let catalog = self.catalog.unwrap_or(CatalogRef { hash: 0, len: 0 });
        let tx = self.tx.clone();
        let id = self.id;
        let now = self.env.clock.now_ms();
        let Some(seat) = self.seats.get_mut(&player) else {
            tracing::warn!(game = id, player, "attach to a seat that does not exist");
            return;
        };
        let SeatControl::Human { nick: owner } = &seat.control else {
            tracing::warn!(game = id, player, "attach to a seat that is not human");
            return;
        };
        if !nick_eq(owner, &nick) {
            tracing::warn!(
                game = id,
                player,
                "attach by a nick that does not own the seat"
            );
            return;
        }
        // A game with mods compares what the client has built in with its frozen set (D5, T17).
        let mut mods_view = None;
        if self.mods.has_set() {
            let hello = self.env.mod_hellos.get(session);
            match self.mods.admit(hello.as_deref()) {
                Ok(a) => {
                    self.mods.seat_admitted(player, a.unavailable);
                    mods_view = Some(a.view);
                }
                Err(problems) => {
                    tracing::info!(
                        game = id,
                        player,
                        session,
                        "a client lacks a required mod: {problems:?}"
                    );
                    self.info.log.event(
                        "mod_mismatch",
                        serde_json::json!({"player": player, "session": session, "problems": problems.len()}),
                    );
                    self.info.rec.seat(
                        player,
                        "mod_mismatch",
                        Some(&nick),
                        problems
                            .iter()
                            .map(|p| p.text.as_str())
                            .collect::<Vec<_>>()
                            .join("; "),
                    );
                    refuse_mods(&out, problems);
                    return;
                }
            }
        }
        // The old connection of the seat (a second tab) loses its hold.
        if let Some(old) = seat.link.take() {
            *old.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        *link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(GameLink {
            game: id,
            player,
            tx,
        });
        seat.session = Some(session);
        seat.link = Some(link);
        seat.conn = Conn::Online;
        self.info.log.event(
            "attach",
            serde_json::json!({"player": player, "session": session, "resume": resume.is_some()}),
        );
        self.info.rec.seat(
            player,
            "attach",
            None,
            format!("session {session}, resume {}", resume.is_some()),
        );
        seat.limits = SeatLimits::new(&self.env.limits, now);
        let last_cmd_id = seat.orders.last_id();
        let pipe = seat.pipe.clone();
        if !self.broadcast_seats() {
            // Nothing changed for the others (a reload within the time a lost line stays hidden), but this
            // seat's stream may never have had the seats (a woken game): it gets them now.
            let views = self.sent_views.clone();
            if pipe.is_some() {
                self.step(
                    player,
                    Parts {
                        seats: Some(views),
                        ..Parts::default()
                    },
                );
            }
        }
        self.reconcile_timers();
        if let Some(pipe) = pipe {
            pipe.send(SeatInput::Attach(AttachReq {
                session,
                out,
                resume,
                claim,
                catalog,
                last_cmd_id,
                mods: mods_view,
            }));
        }
    }

    /// `JoinGame` into an AI slot: the worker gets a `Claim` and does it at a safe point.
    /// The pipeline of the seat exists from now on so that the first `Full` of the new seat (it comes
    /// in the frame of `SeatDone`) has a place to go.
    fn join(&mut self, req: JoinRequest) {
        let JoinRequest {
            player,
            nick,
            session,
            out,
            link,
            claim,
        } = req;
        let refuse = |this: &Self, why: &str| this.join_failed(player, &nick, why);
        if !self.ready && self.sup.state() != GameState::Faulted {
            // A game that is still waking: the request waits for the worker.
            self.pending_joins.push(JoinRequest {
                player,
                nick,
                session,
                out,
                link,
                claim,
            });
            return;
        }
        if !self.ready || self.sup.state() != GameState::Running {
            return refuse(self, "the game is not running");
        }
        let Some(seat) = self.seats.get(&player) else {
            return refuse(self, "the game has no such player");
        };
        if !matches!(seat.control, SeatControl::Ai { .. }) {
            return refuse(self, "that slot is not an AI player's");
        }
        if self.claims.contains_key(&player) {
            return refuse(self, "somebody else is taking that slot");
        }
        let humans = self
            .seats
            .values()
            .filter(|s| matches!(s.control, SeatControl::Human { .. }))
            .count()
            + self.claims.len();
        if humans >= MAX_HUMANS {
            return refuse(self, "the game has the most humans it takes");
        }
        if self
            .seats
            .values()
            .any(|s| matches!(&s.control, SeatControl::Human { nick: n } if nick_eq(n, &nick)))
        {
            return refuse(self, "you sit in this game already");
        }
        if self.mods.has_set() {
            let hello = self.env.mod_hellos.get(session);
            if let Err(problems) = self.mods.admit(hello.as_deref()) {
                let why = problems
                    .iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join("; ");
                refuse_mods(&out, problems);
                return refuse(self, &why);
            }
        }
        let pipe = seat::spawn(
            self.id,
            player,
            self.info.seat_stats.seat(player),
            self.tx.clone(),
            Arc::clone(&self.env.clock),
            self.info.log.clone(),
            self.info.rec.clone(),
        );
        if let Err(state) = self.sup.send(ToWorker::Seat {
            player,
            change: SeatChange::Claim { nick: nick.clone() },
        }) {
            let (_, why) = state_refusal(state);
            return refuse(self, why);
        }
        if let Some(seat) = self.seats.get_mut(&player) {
            seat.pipe = Some(pipe);
        }
        self.claims.insert(
            player,
            PendingClaim {
                nick,
                session,
                out,
                link,
                claim,
                since_ms: self.env.clock.now_ms(),
                at: std::time::Instant::now(),
                expired: false,
            },
        );
    }

    /// The worker did a seat change, or refused it.
    fn on_seat_done(&mut self, player: PlayerId, change: SeatChange, error: Option<String>) {
        match change {
            SeatChange::Claim { nick } => self.claim_done(player, &nick, error),
            SeatChange::Kick => self.kick_done(player, error),
        }
    }

    fn claim_done(&mut self, player: PlayerId, nick: &str, error: Option<String>) {
        let Some(c) = self.claims.remove(&player) else {
            tracing::warn!(game = self.id, player, "a Claim answer nobody waits for");
            return;
        };
        self.info.stats.safe_point_wait.record(c.at.elapsed());
        self.info.log.event(
            "seat_change",
            serde_json::json!({"player": player, "change": "claim", "nick": nick, "ok": error.is_none()}),
        );
        self.info.rec.seat(
            player,
            "claim",
            Some(nick),
            format!("ok: {}", error.is_none()),
        );
        if let Some(why) = error {
            if let Some(seat) = self.seats.get_mut(&player) {
                seat.pipe = None;
            }
            if !c.expired {
                self.join_failed(player, nick, &why);
            }
            return;
        }
        if c.expired {
            // The player was told it failed: give the slot back to the AI.
            if let Some(seat) = self.seats.get_mut(&player) {
                seat.pipe = None;
            }
            let _ = self.sup.send(ToWorker::Seat {
                player,
                change: SeatChange::Kick,
            });
            return;
        }
        if let Some(seat) = self.seats.get_mut(&player) {
            seat.control = SeatControl::Human {
                nick: nick.to_owned(),
            };
            seat.conn = Conn::None;
            seat.orders = OrderBook::new();
        }
        self.sync_seats();
        self.write_meta(MetaStatus::Running);
        self.attach(AttachRequest {
            player,
            nick: nick.to_owned(),
            session: c.session,
            out: c.out,
            link: c.link,
            resume: None,
            claim: c.claim,
        });
        // The lobby list shows the slot as taken before the lobby hears of it.
        self.publish();
        let _ = self.env.lobby.send(LobbyMsg::JoinDone {
            game: self.id,
            player,
            nick: nick.to_owned(),
        });
    }

    /// A save the worker wrote goes into the lobby's list.
    fn report_save(&self, rec: &SaveRecord) {
        let bytes = self
            .env
            .state_dir
            .as_ref()
            .and_then(|d| {
                std::fs::metadata(d.join("games").join(self.id.to_string()).join(&rec.file)).ok()
            })
            .map_or(0, |m| m.len());
        self.info.stats.set_last_save(stats::SaveLast {
            seq: rec.meta.seq,
            turn: rec.meta.turn,
            kind: rec.kind,
            bytes,
            applied_upto: rec.applied_upto.iter().map(|(p, c)| (*p, *c)).collect(),
        });
        self.info
            .rec
            .put(recording::Body::Save(recording::format::SaveRec {
                seq: rec.meta.seq,
                turn: rec.meta.turn,
                kind: format!("{:?}", rec.kind),
                file: rec.file.clone(),
                bytes,
            }));
        self.info.log.event(
            "save",
            serde_json::json!({
                "seq": rec.meta.seq,
                "turn": rec.meta.turn,
                "file": rec.file,
                "kind": format!("{:?}", rec.kind),
                "bytes": bytes,
            }),
        );
        let seats = self
            .seats
            .iter()
            .map(|(p, s)| SaveSeat {
                player: *p,
                civ: s.civ.clone(),
                civ_name: self.civ_name(&s.civ),
                nick: match &s.control {
                    SeatControl::Human { nick } => Some(nick.clone()),
                    _ => None,
                },
            })
            .collect();
        let entry = SaveEntry {
            id: save_id(self.id, rec.meta.seq),
            game: self.id,
            seq: rec.meta.seq,
            kind: rec.kind,
            file: rec.file.clone(),
            name: self.info.name.clone(),
            turn: rec.meta.turn,
            saved_at_ms: rec.meta.saved_at_ms,
            build_id: rec.meta.build_id.clone(),
            settings: self.info.settings.clone(),
            seats,
            host: self.host.clone(),
        };
        let _ = self.env.lobby.send(LobbyMsg::SaveWritten(Box::new(entry)));
    }

    fn session_gone(&mut self, player: PlayerId, session: u64) {
        let now = self.env.clock.now_ms();
        let Some(seat) = self.seats.get_mut(&player) else {
            return;
        };
        if seat.session != Some(session) {
            return;
        }
        seat.session = None;
        seat.conn = Conn::Lost { since_ms: now };
        if let Some(link) = seat.link.take() {
            *link
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
        if let Some(pipe) = &seat.pipe {
            pipe.send(SeatInput::Detach { session });
        }
        self.info.log.event(
            "detach",
            serde_json::json!({"player": player, "session": session, "why": "connection lost"}),
        );
        self.info.rec.seat(
            player,
            "detach",
            None,
            format!("session {session}, connection lost"),
        );
        self.forget_queries(player, Some(session));
        // Hidden from the others for a few seconds (`broadcast_seats`); the clock of its group
        // stands now.
        self.broadcast_seats();
        self.reconcile_timers();
    }

    /// The seats as the others see them: a connection lost a moment ago still shows as up.
    fn seat_views(&self, now: u64) -> Vec<SeatView> {
        let host = self.host_seat();
        self.seats
            .iter()
            .map(|(p, s)| SeatView {
                player: *p,
                control: s.control.clone(),
                conn: match s.conn {
                    Conn::Lost { since_ms } if now.saturating_sub(since_ms) < SHOW_LOST_MS => {
                        Conn::Online
                    }
                    c => c,
                },
                host: host == Some(*p),
            })
            .collect()
    }

    /// `Seats` to every human seat when what they show changed; `false`: nothing changed, nothing was sent.
    fn broadcast_seats(&mut self) -> bool {
        let views = self.seat_views(self.env.clock.now_ms());
        if views == self.sent_views {
            return false;
        }
        self.sent_views.clone_from(&views);
        for (p, seat) in &self.seats {
            if seat.pipe.is_some() {
                self.step(
                    *p,
                    Parts {
                        seats: Some(views.clone()),
                        ..Parts::default()
                    },
                );
            }
        }
        true
    }

    fn client(&mut self, player: PlayerId, session: u64, msg: ClientGameMsg, bytes: usize) {
        let valid = self
            .seats
            .get(&player)
            .is_some_and(|s| s.session == Some(session));
        if !valid {
            return;
        }
        let rx = self.info.seat_stats.seat(player);
        stats::bump(&rx.rx_msgs);
        rx.rx_bytes
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
        self.last_word_ms = self.env.clock.now_ms();
        match msg {
            ClientGameMsg::Chat(c) => self.on_chat(player, c),
            ClientGameMsg::Cmd(c) => self.on_cmd(player, c),
            ClientGameMsg::Focus(f) => {
                let now = self.env.clock.now_ms();
                let ok = self
                    .seats
                    .get_mut(&player)
                    .is_some_and(|s| s.limits.focus_ok(now));
                if ok {
                    // Last one wins at the worker; while it restarts there is nobody to tell.
                    let _ = self.sup.send(ToWorker::Focus { player, focus: f });
                }
            }
            ClientGameMsg::Query { id, q } => self.on_query(player, session, id, q),
            ClientGameMsg::GameCtl(ctl) => self.on_game_ctl(player, session, ctl),
            ClientGameMsg::GcNet(bytes) => self.on_gc_net(player, bytes),
            ClientGameMsg::Mod(m) => self.on_mod(player, session, &m),
            ClientGameMsg::GcResync {
                epoch,
                have_raw_hash,
                at_jseq,
                reason,
            } => self.on_gc_resync(player, epoch, have_raw_hash, at_jseq, reason),
            ClientGameMsg::NeedKeyframe(reason) => {
                if let Some(pipe) = self.seats.get(&player).and_then(|s| s.pipe.as_ref()) {
                    pipe.send(SeatInput::NeedKeyframe { session, reason });
                }
            }
        }
    }

    // ------------------------------------------------------------ mods (T18, T19)

    fn mod_facts(&self) -> UnknownFacts {
        UnknownFacts {
            map: self.map,
            turn: self.turn.as_ref().map_or(0, |t| t.turn),
        }
    }

    /// The human seats (they have a pipeline), in order: the audiences of the mods.
    fn humans(&self) -> Vec<PlayerId> {
        self.seats
            .iter()
            .filter(|(_, s)| s.pipe.is_some())
            .map(|(p, _)| *p)
            .collect()
    }

    /// (Re)starts the server halves from `saved` and hands the worker the block its saves carry.
    fn start_mods(&mut self, saved: Option<protocol::ModsSection>) {
        self.mod_turn = None;
        if !self.mods.has_set() {
            return;
        }
        let (facts, now) = (self.mod_facts(), self.env.clock.now_ms());
        self.mods.start(&facts, now, saved);
        self.flush_mods();
    }

    /// The block of the mods goes to the worker whenever it changed (it writes `MODS` into every save, T19). A game
    /// without a mod set never sends one.
    fn flush_mods(&mut self) {
        if !self.mods.has_set() {
            return;
        }
        let (facts, now) = (self.mod_facts(), self.env.clock.now_ms());
        if let Some(sec) = self.mods.persist(&facts, now)
            && self.sup.send(ToWorker::Mods(sec.clone())).is_ok()
        {
            self.mods.sent(sec);
        }
    }

    /// A message of a client to a mod: host limits, the mod, then its answers to the seats (D7).
    fn on_mod(&mut self, player: PlayerId, session: u64, m: &protocol::ModC2S) {
        // A game without mods takes no mod messages: dropped as before the mod system.
        if !self.mods.has_set() {
            return;
        }
        let (facts, now) = (self.mod_facts(), self.env.clock.now_ms());
        let humans = self.humans();
        let d = self.mods.on_client(&facts, now, player, m, &humans);
        if self.info.rec.is_on() {
            self.info
                .rec
                .put(recording::Body::Mod(recording::format::ModRec {
                    mod_idx: m.mod_idx,
                    ch: m.ch,
                    c2s: true,
                    player: Some(player),
                    to: Vec::new(),
                    taken: d.dropped.is_none(),
                    payload: m.payload.clone(),
                }));
        }
        if d.dropped.is_some() {
            tracing::debug!(game = self.id, player, dropped = ?d.dropped, "a mod message was dropped");
        }
        let disconnect = d.disconnect;
        self.deliver_mods(d);
        self.flush_mods();
        if disconnect {
            tracing::warn!(
                game = self.id,
                player,
                "a seat keeps breaking the mod limits, disconnected"
            );
            self.info.rec.seat(
                player,
                "mod_strikes",
                None,
                format!("session {session}, disconnected"),
            );
            if let Some(pipe) = self.seats.get(&player).and_then(|s| s.pipe.as_ref()) {
                pipe.send(SeatInput::Disconnect { session });
            }
        }
    }

    /// The answers of the mods go into the batches of the seats, in order; the recording gets them once.
    fn deliver_mods(&self, d: Delivery) {
        if self.info.rec.is_on() {
            for (o, to) in d.outputs.iter().zip(&d.targets) {
                let w = o.to_wire();
                self.info
                    .rec
                    .put(recording::Body::Mod(recording::format::ModRec {
                        mod_idx: o.mod_idx,
                        ch: o.ch,
                        c2s: false,
                        player: w.from,
                        to: to.clone(),
                        taken: true,
                        payload: w.payload,
                    }));
            }
        }
        for (p, msgs) in d.by_seat {
            self.step(
                p,
                Parts {
                    mods: msgs,
                    ..Parts::default()
                },
            );
        }
    }

    /// A seat got a `Fresh` stream: the mods send it their state (the pattern of the chat history, D8).
    fn mod_snapshot(&mut self, player: PlayerId, session: u64) {
        if !self.mods.has_code()
            || self
                .seats
                .get(&player)
                .is_none_or(|s| s.session != Some(session))
        {
            return;
        }
        let (facts, now) = (self.mod_facts(), self.env.clock.now_ms());
        let humans = self.humans();
        let d = self.mods.on_attach(&facts, now, player, &humans);
        self.deliver_mods(d);
        self.flush_mods();
    }

    /// After a worker restart: the mods may have been rewound with the save, so every seat that is attached gets the
    /// snapshot again (a client takes it as the new state).
    fn resend_mod_snapshots(&mut self) {
        if !self.mods.has_code() {
            return;
        }
        let online: Vec<(PlayerId, u64)> = self
            .seats
            .iter()
            .filter(|(_, s)| s.pipe.is_some() && matches!(s.conn, Conn::Online))
            .filter_map(|(p, s)| s.session.map(|x| (*p, x)))
            .collect();
        for (p, session) in online {
            self.mod_snapshot(p, session);
        }
    }

    /// A new turn began (not the first one seen, not a repeat).
    fn note_mod_turn(&mut self, turn: i32) {
        let prev = self.mod_turn.replace(turn);
        if prev.is_some_and(|p| p != turn) && self.mods.has_code() {
            let (facts, now) = (self.mod_facts(), self.env.clock.now_ms());
            self.mods.on_turn(&facts, now, turn);
            self.flush_mods();
        }
    }

    /// `GcNet`: the UI messages of a seat go to the worker, which hands them to the core (all or nothing).
    fn on_gc_net(&mut self, player: PlayerId, bytes: Vec<u8>) {
        let st = &self.info.stats.replica;
        let now = self.env.clock.now_ms();
        let ok = bytes.len() <= protocol::GC_NET_MAX
            && !bytes.is_empty()
            && self
                .seats
                .get_mut(&player)
                .is_some_and(|s| s.limits.cmd_ok(now));
        if self.info.rec.is_on() {
            self.info
                .rec
                .put(recording::Body::Net(recording::format::NetRec {
                    player,
                    bytes: bytes.clone(),
                    taken: ok,
                }));
        }
        if !ok {
            stats::bump(&st.net_dropped);
            return;
        }
        stats::bump(&st.net_in);
        st.net_bytes
            .fetch_add(bytes.len() as u64, std::sync::atomic::Ordering::Relaxed);
        // While the worker restarts the message cannot go: the player is told on the spot (it was not applied),
        // instead of the click vanishing (AC-G3).
        if let Err(state) = self.sup.send(ToWorker::Net {
            player,
            msgs: bytes,
        }) {
            stats::bump(&st.net_dropped);
            tracing::info!(
                game = self.id,
                player,
                ?state,
                "a UI message was not passed: the game is not running"
            );
            self.private_line(
                player,
                "The game is restarting: your last action was not applied, please repeat it"
                    .to_owned(),
            );
        }
    }

    /// `GcResync`: the replica of the seat cannot go on. The worker makes a snapshot at the next safe point;
    /// the answer is a `GcBase` for this seat alone. A seat is answered once in a couple of seconds.
    fn on_gc_resync(
        &mut self,
        player: PlayerId,
        epoch: u32,
        have_raw_hash: Option<u64>,
        at_jseq: u64,
        reason: GcResyncReason,
    ) {
        let st = &self.info.stats.replica;
        stats::bump(&st.resync[resync_index(reason)]);
        let now = self.env.clock.now_ms();
        if !self.has_pipe(player) {
            stats::bump(&st.resync_dropped);
            return;
        }
        if !self.replica.resync_allowed(player, now) {
            // Inside the gap: served by `on_tick` when the gap is over (the client asks again only after seconds).
            stats::bump(&st.resync_dropped);
            self.replica.defer(player);
            return;
        }
        self.serve_resync(player, epoch, have_raw_hash, at_jseq, reason);
    }

    /// SIMPLIFIED(SIMP-exe-901): deferred resyncs and a visible refusal while the worker restarts.
    ///
    /// Hands a resync of a seat to the worker; when that fails the gap stamp is undone and the request kept for
    /// the next tick (the worker may be restarting).
    fn serve_resync(
        &mut self,
        player: PlayerId,
        epoch: u32,
        have_raw_hash: Option<u64>,
        at_jseq: u64,
        reason: GcResyncReason,
    ) {
        self.info
            .rec
            .put(recording::Body::Resync(recording::format::ResyncRec {
                player,
                epoch,
                chain_epoch: self.replica.epoch(),
                have_raw_hash,
                at_jseq,
                reason: format!("{reason:?}"),
            }));
        self.info.log.event(
            "gc_resync",
            serde_json::json!({
                "player": player,
                "epoch": epoch,
                "chain_epoch": self.replica.epoch(),
                "have": have_raw_hash.map(|h| format!("{h:016x}")),
                "at_jseq": at_jseq,
                "reason": format!("{reason:?}"),
            }),
        );
        if self.sup.send(ToWorker::ReplicaSnapshot { player }).is_err() {
            self.replica.unstamp(player);
            self.replica.defer(player);
        }
    }

    /// Deferred resyncs whose gap is over (see `on_gc_resync`).
    fn serve_deferred_resyncs(&mut self, now: u64) {
        for p in self.replica.take_due(now) {
            if !self.has_pipe(p) {
                continue;
            }
            let epoch = self.replica.epoch();
            self.serve_resync(p, epoch, None, 0, GcResyncReason::Gap);
        }
    }

    /// The order checks of spec 4.6, in their order.
    fn on_cmd(&mut self, player: PlayerId, c: Cmd) {
        let trace_t0 = self.info.trace.now_us();
        stats::bump(&self.info.stats.received);
        let now = self.env.clock.now_ms();
        let id = c.id;
        self.info.log.event(
            "cmd",
            serde_json::json!({"player": player, "id": id, "turn": c.turn, "cmd": format!("{:?}", c.cmd).split(['{', '(', ' ']).next().unwrap_or("")}),
        );
        let verdict = match self.seats.get_mut(&player) {
            Some(seat) => seat.orders.check(id),
            None => return,
        };
        match verdict {
            Verdict::Repeat(r) => {
                self.rec_order(player, c, RecVerdict::Repeat);
                stats::bump(&self.info.stats.dup_answered);
                self.send_results(player, vec![r]);
            }
            Verdict::Pending => {
                self.rec_order(player, c, RecVerdict::Pending);
                stats::bump(&self.info.stats.dup_pending_dropped);
            }
            Verdict::Expired => {
                self.rec_order(player, c, RecVerdict::Expired);
                stats::bump(&self.info.stats.expired);
                // Not remembered: the id is older than the ring.
                let r = rejected(id, RejectCode::Expired, "the order is too old to answer");
                count_result(&self.info.stats, &r);
                self.send_results(player, vec![r]);
            }
            Verdict::New { gap } => {
                if gap {
                    stats::bump(&self.info.stats.id_gap);
                }
                let mut refusal = self.refusal(player, now);
                if refusal.is_none() && self.reject_next.remove(&player) {
                    // Test hook `reject_next_cmd`.
                    refusal = Some((RejectCode::Core, "refused by a test hook"));
                }
                match refusal {
                    Some((code, why)) => {
                        self.rec_order(
                            player,
                            c,
                            RecVerdict::Refused {
                                code: format!("{code:?}"),
                                why: why.to_owned(),
                            },
                        );
                        self.answer(player, rejected(id, code, why));
                    }
                    None => match self.sup.send(ToWorker::Cmd { player, cmd: c }) {
                        Ok(()) => {
                            self.rec_order(player, c, RecVerdict::Forwarded);
                            stats::bump(&self.info.stats.forwarded);
                            if matches!(c.cmd, protocol::Command::EndTurn { .. }) {
                                // `g.cmd`: the end of turn arrived -> it is on the worker's pipe.
                                self.info.trace.span(
                                    c.turn,
                                    "g.cmd",
                                    "io",
                                    trace_t0,
                                    &[("player", i64::from(player)), ("cmd", i64::from(id))],
                                );
                            }
                            self.cmd_times
                                .insert((player, id), std::time::Instant::now());
                            if let Some(seat) = self.seats.get_mut(&player) {
                                seat.orders.sent(id);
                            }
                        }
                        Err(state) => {
                            let (code, why) = state_refusal(state);
                            self.rec_order(
                                player,
                                c,
                                RecVerdict::SendFailed {
                                    why: format!("{state:?}: {why}"),
                                },
                            );
                            self.answer(player, rejected(id, code, why));
                        }
                    },
                }
            }
        }
    }

    /// The order as received and what the gateway did with it (recording).
    fn rec_order(&self, player: PlayerId, c: Cmd, verdict: RecVerdict) {
        self.info
            .rec
            .put(recording::Body::Order(recording::format::OrderRec {
                player,
                cmd: c,
                verdict,
            }));
    }

    /// Checks 3 and 4 of spec 4.6 (and the window of 64): why an order cannot go on, if so.
    fn refusal(&mut self, player: PlayerId, now: u64) -> Option<(RejectCode, &'static str)> {
        let state = self.sup.state();
        if state != GameState::Running {
            return Some(state_refusal(state));
        }
        let seat = self.seats.get_mut(&player)?;
        if seat.orders.window_full() || !seat.limits.cmd_ok(now) {
            return Some((RejectCode::TooMany, "too many orders"));
        }
        None
    }

    fn on_query(&mut self, player: PlayerId, session: u64, id: u32, q: Query) {
        let now = self.env.clock.now_ms();
        let ok = self
            .seats
            .get_mut(&player)
            .is_some_and(|s| s.limits.query_ok(now));
        if !ok {
            stats::bump(&self.info.stats.queries_dropped);
            return;
        }
        let qid = self.env.games.next_qid();
        match self.sup.send(ToWorker::Query { player, qid, q }) {
            Ok(()) => {
                stats::bump(&self.info.stats.queries);
                self.queries.insert(
                    qid,
                    QueryOwner {
                        player,
                        session,
                        id,
                    },
                );
            }
            Err(_) => {
                // While the worker restarts queries get no answer.
                stats::bump(&self.info.stats.queries_dropped);
                if let Some(seat) = self.seats.get_mut(&player) {
                    seat.limits.query_done();
                }
            }
        }
    }

    // ------------------------------------------------------------ snapshot

    fn civ_name(&self, civ: &CivPick) -> String {
        match civ {
            CivPick::Random => "Random".to_owned(),
            CivPick::Civ(id) => self
                .env
                .choices
                .civs
                .iter()
                .find(|c| &c.id == id)
                .map_or_else(|| id.clone(), |c| c.name.clone()),
        }
    }

    fn publish(&self) {
        let seats = self
            .seats
            .iter()
            .map(|(p, s)| SeatSnap {
                player: *p,
                civ_name: self.civ_name(&s.civ),
                control: s.control.clone(),
                online: matches!(s.conn, Conn::Online),
                conn: match s.conn {
                    Conn::Online => "online",
                    Conn::Lost { .. } => "lost",
                    Conn::None => "none",
                },
            })
            .collect();
        self.info.set_snapshot(Snapshot {
            state: if self.over && self.sup.state() == GameState::Running {
                GameState::Finished
            } else {
                self.sup.state()
            },
            host: self.host.clone(),
            turn: self.turn.as_ref().map_or(0, |t| t.turn),
            phase: self.phase_info.map(|(p, _)| p),
            mode: self.phase_info.map(|(_, m)| m),
            map: self.map,
            core_build: self.core_build.clone(),
            seats,
            last_exit: self.last_exit,
        });
        let _ = self.env.lobby.send(LobbyMsg::GamesChanged);
    }
}

/// `Rejected` code and text for an order that finds the worker not running.
fn state_refusal(state: GameState) -> (RejectCode, &'static str) {
    match state {
        GameState::Starting | GameState::Restarting | GameState::Running => {
            (RejectCode::Restarting, "the game is restarting")
        }
        GameState::Faulted | GameState::Finished => (RejectCode::NoGame, "the game is not running"),
    }
}
