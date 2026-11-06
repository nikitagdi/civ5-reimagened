//! The multiplayer worker (game-host-mp GH-D1, 4.2, 4.3, section 5): one game, several humans, no I/O.
//! `Worker<C: Core>` holds the phase, the seats, their views and the basis of the map; the core answers the
//! questions and takes the orders. The shell (`civ5-server worker`) feeds it `ToWorker` frames and the clock
//! and performs the `Out`s.
//!
//! One call returns at most one frame; in it, per seat: `Basis`, `View`, `Results`, `Journal`; then `Turn`.
//! While the AI moves no view is built (GH-D14, SIMP-058): the seats get one update when the humans move
//! again.
//! SIMPLIFIED(SIMP-058): the AI turn is shown as one update at its end; the original shows visible AI moves as they are made.
//!
//! Contexts and focus: `context.rs` (H5). Seat changes: `slots.rs`; peace offers: `peace.rs` (H6). Saves,
//! `Load` and `Shutdown { save }`: `checkpoint.rs` and `saves.rs` (H7).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Instant;

use protocol::{
    BasisState, Catalog, Codec, CommandOutcome, CommandResult, ContextMsg, GameSettings,
    IPC_VERSION, JournalEntry, MapBasis, PeaceOffer, PlayerId, Postcard, Query, QueryAnswer,
    RejectCode, SeatChange, SeatTurnInfo, SlotControl, Start, StepKind, TeamId, ToWorker, TopBar,
    TurnPhase, ViewDiff, ViewHash, ViewUpdate, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg,
    WorkerReady, WorldView, basis_hash_of, diff_basis,
};

pub use crate::checkpoint::{SaveDone, SaveFile, SaveJob};

use crate::audit::Auditor;
use crate::checkpoint::SaveState;
use crate::context::{Key, Touch};
use crate::core::{Core, CoreError, CoreErrorKind, CorePhase, CorePump, CoreStart, MapInfo};
use crate::journal;
use crate::orders::OrderQueue;
use crate::replica::{ReplicaJob, ReplicaState};
use crate::saves::Save;
use crate::seats::{Mirror, Seat, SeatParts, TeamData, assemble_view};
pub(crate) use crate::stats::{StepTimes, micros};
use crate::timeline::Timeline;

/// What the shell does with a call of the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// Send to the gateway; one frame per call, never an empty one.
    Frame(WorkerFrame),
    /// Pack and write this save (the save thread of the shell), then call `Worker::save_done`.
    Save(Box<SaveJob>),
    /// Pack this snapshot of the replica stream (the save thread), then call `Worker::replica_done`.
    Replica(Box<ReplicaJob>),
    /// End the process with this code (game-host-mp 4.12).
    Exit(u8),
}

/// What one call puts into its frame, ordered when the frame is made.
#[derive(Default)]
pub(crate) struct Fb {
    /// First: `Ready`, `Catalog`, `Choices`.
    pub head: Vec<WorkerMsg>,
    /// The table of owners (v9), right after `head`, before everything of the seats.
    pub owners: Option<protocol::OwnersUpdate>,
    /// Seats that get the map basis before their `Full`.
    pub basis_for: Vec<PlayerId>,
    pub views: BTreeMap<PlayerId, ViewUpdate>,
    pub results: BTreeMap<PlayerId, Vec<CommandResult>>,
    pub journal: BTreeMap<PlayerId, Vec<JournalEntry>>,
    /// Contexts by seat, each with the revision of the view it is for.
    pub ctx: BTreeMap<PlayerId, Vec<(u32, ContextMsg)>>,
    pub turn: Option<protocol::TurnInfo>,
    /// The replica stream: journal batches, bodies, bases. Before `Turn`.
    pub replica: Vec<WorkerMsg>,
    /// After `Turn`: query answers, `Stats`, `Fatal`.
    pub tail: Vec<WorkerMsg>,
    pub kind: Option<StepKind>,
}

impl Fb {
    pub fn result(&mut self, player: PlayerId, id: protocol::CmdId, outcome: CommandOutcome) {
        self.results
            .entry(player)
            .or_default()
            .push(CommandResult { id, outcome });
    }

    pub fn reject(&mut self, player: PlayerId, id: protocol::CmdId, code: RejectCode, text: &str) {
        self.result(
            player,
            id,
            CommandOutcome::Rejected {
                code,
                reason: text.to_owned(),
            },
        );
    }

    pub fn context(&mut self, player: PlayerId, rev: u32, msg: ContextMsg) {
        self.ctx.entry(player).or_default().push((rev, msg));
    }

    /// The context of `key` is still good: one `Valid` per seat and frame lists them all.
    pub(crate) fn valid(&mut self, player: PlayerId, rev: u32, key: Key) {
        let list = self.ctx.entry(player).or_default();
        let at = match list
            .iter()
            .position(|(_, m)| matches!(m, ContextMsg::Valid { .. }))
        {
            Some(i) => i,
            None => {
                let msg = ContextMsg::Valid {
                    units: Vec::new(),
                    city: None,
                    view_seq: 0,
                };
                list.push((rev, msg));
                list.len() - 1
            }
        };
        if let Some((_, ContextMsg::Valid { units, city, .. })) = list.get_mut(at) {
            match key {
                Key::Unit(u) if !units.contains(&u) => units.push(u),
                Key::Unit(_) => {}
                Key::City(c) => *city = Some(c),
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.head.is_empty()
            && self.owners.is_none()
            && self.basis_for.is_empty()
            && self.views.is_empty()
            && self.results.is_empty()
            && self.journal.is_empty()
            && self.ctx.is_empty()
            && self.replica.is_empty()
            && self.turn.is_none()
            && self.tail.is_empty()
    }

    fn into_frame(self, basis: Option<MapBasis>) -> Option<WorkerFrame> {
        if self.is_empty() {
            return None;
        }
        let mut msgs = self.head;
        if let Some(u) = self.owners {
            msgs.push(WorkerMsg::Owners(u));
        }
        if let (false, Some(basis)) = (self.basis_for.is_empty(), basis) {
            msgs.push(WorkerMsg::Basis {
                players: self.basis_for,
                basis,
            });
        }
        let mut views = self.views;
        let mut results = self.results;
        let mut journal = self.journal;
        let mut ctx = self.ctx;
        let players: BTreeSet<PlayerId> = views
            .keys()
            .chain(results.keys())
            .chain(journal.keys())
            .chain(ctx.keys())
            .copied()
            .collect();
        for player in players {
            if let Some(update) = views.remove(&player) {
                msgs.push(WorkerMsg::View { player, update });
            }
            if let Some(results) = results.remove(&player) {
                msgs.push(WorkerMsg::Results { player, results });
            }
            if let Some(entries) = journal.remove(&player) {
                msgs.push(WorkerMsg::Journal { player, entries });
            }
            for (rev, ctx) in ctx.remove(&player).unwrap_or_default() {
                msgs.push(WorkerMsg::Context { player, rev, ctx });
            }
        }
        msgs.extend(self.replica);
        if let Some(t) = self.turn {
            msgs.push(WorkerMsg::Turn(t));
        }
        msgs.extend(self.tail);
        Some(WorkerFrame { msgs })
    }
}

/// The worker of one game.
pub struct Worker<C: Core> {
    pub(crate) core: C,
    pub(crate) cfg: WorkerConfig,
    pub(crate) game: protocol::GameId,
    pub(crate) phase: TurnPhase,
    pub(crate) turn: i32,
    /// Turn mode of the settings, for the `Turn` of the starting phase.
    pub(crate) mode: protocol::TurnMode,
    pub(crate) map: MapInfo,
    /// Human seats by player.
    pub(crate) seats: BTreeMap<PlayerId, Seat>,
    pub(crate) teams: BTreeMap<TeamId, TeamData>,
    /// The map basis of the game, one for all seats (NP-D19); changes only by `basis` diffs.
    pub(crate) basis: BasisState,
    pub(crate) queue: OrderQueue,
    /// The last `TurnInfo` sent, `human_wars` cleared.
    pub(crate) last_turn: Option<protocol::TurnInfo>,
    /// Core events not yet in a frame, by seat.
    pub(crate) journal_pending: journal::Pending,
    /// Peace offers between humans (GH-D13); every seat's view carries the ones it takes part in.
    pub(crate) offers: Vec<PeaceOffer>,
    /// Seat changes that wait for a safe point (GH-D8).
    pub(crate) pending_seats: VecDeque<(PlayerId, SeatChange)>,
    /// The core was not at a safe point; the next try is not before `seat_retry_ms`.
    pub(crate) seat_blocked: bool,
    pub(crate) seat_retry_ms: u64,
    /// An end of turn was applied and the core has not run on yet: its checked replica batch goes out with this step,
    /// the pump (which may be the whole AI phase, 10-20 s in a late game) is the next step ([`Worker::deferred_pump`]).
    pub(crate) pump_due: bool,
    /// Since when something (a seat change, a replica snapshot) has waited for a safe point, and when the wait was last
    /// reported at warn level (`note_no_safe_point`).
    pub(crate) unsafe_since_ms: Option<u64>,
    pub(crate) unsafe_warned_ms: u64,
    /// The revision a seat had when it was kicked: a seat that takes the slot again goes on from it.
    pub(crate) retired_rev: BTreeMap<PlayerId, u32>,
    /// Seats that gave an order in the batch (with the city of a city order), for the urgent contexts.
    pub(crate) ordered: BTreeMap<PlayerId, Option<i32>>,
    /// The seat of the last order the core took: its contexts come first in the background.
    pub(crate) last_order_seat: Option<PlayerId>,
    pub(crate) now_ms: u64,
    pub(crate) last_frame_ms: u64,
    /// The worker is finished: `Fatal` or `Shutdown` went out.
    pub(crate) dead: bool,
    pub(crate) auditor: Auditor,
    /// Fog violations found since the start.
    pub(crate) fog_violations: u64,
    pub(crate) times: StepTimes,
    /// The settings of the game, for the save (`SETG`).
    pub(crate) settings: Option<GameSettings>,
    /// Saves: what is due, what is being written.
    pub(crate) saves: SaveState,
    /// The mods block every save carries (`MODS`): from the loaded save, then from the gateway (`ToWorker::Mods`).
    /// `None`: a game without mods, no section is written.
    pub(crate) mods: Option<protocol::ModsSection>,
    /// The table of owners for the gateway, kept only after `ToWorker::WantOwners` (`owners.rs`).
    pub(crate) owners: crate::owners::OwnersState,
    /// After a load: the owners of the slots the gateway named, applied at the first safe point.
    pub(crate) load_controls: Option<Vec<(PlayerId, SlotControl)>>,
    /// The replica stream (`replica.rs`).
    pub(crate) rep: ReplicaState,
    /// The timeline of the end of a turn and the AI progress (`timeline.rs`).
    pub(crate) tl: Timeline,
}

/// A basis that holds nothing, for a worker that did not start.
fn empty_basis() -> BasisState {
    let mut m = MapBasis::default();
    m.basis_hash = basis_hash_of(&m);
    match BasisState::new(m) {
        Ok(b) => b,
        // An empty basis passes `BasisState::new`; this is not reachable.
        Err(_) => unreachable!("an empty map basis is well formed"),
    }
}

fn exit_code(kind: CoreErrorKind) -> u8 {
    match kind {
        CoreErrorKind::Failed | CoreErrorKind::NotSafe => 1,
        CoreErrorKind::Stuck => 3,
        CoreErrorKind::BadSave => 4,
        CoreErrorKind::BadStart => 5,
    }
}

impl<C: Core> Worker<C> {
    /// Starts the game of `init` on `core` (the shell built it from `init.core`, with the test hooks set).
    /// Returns the worker and what to send: `Catalog` and `Ready`, or `Fatal` and the exit.
    ///
    /// `Start::Load` needs the file read by the shell: use [`Worker::resume`].
    pub fn new(core: C, init: WorkerInit, now_ms: u64) -> (Self, Vec<Out>) {
        Self::start(core, init, None, now_ms)
    }

    /// Continues the game of a save (`Start::Load`): `save` is the file the shell read and decoded
    /// (`saves::decode`; a file that does not decode is the shell's `Fatal` with code 4).
    pub fn resume(core: C, init: WorkerInit, save: Save, now_ms: u64) -> (Self, Vec<Out>) {
        Self::start(core, init, Some(save), now_ms)
    }

    fn start(core: C, init: WorkerInit, save: Option<Save>, now_ms: u64) -> (Self, Vec<Out>) {
        let mut w = Worker {
            core,
            cfg: init.cfg.clone(),
            game: init.game,
            phase: TurnPhase::Starting,
            turn: 0,
            mode: protocol::TurnMode::Hybrid,
            map: MapInfo::default(),
            seats: BTreeMap::new(),
            teams: BTreeMap::new(),
            basis: empty_basis(),
            queue: OrderQueue::default(),
            last_turn: None,
            journal_pending: journal::Pending::default(),
            offers: Vec::new(),
            pending_seats: VecDeque::new(),
            seat_blocked: false,
            pump_due: false,
            seat_retry_ms: 0,
            unsafe_since_ms: None,
            unsafe_warned_ms: 0,
            retired_rev: BTreeMap::new(),
            ordered: BTreeMap::new(),
            last_order_seat: None,
            now_ms,
            last_frame_ms: now_ms,
            dead: false,
            auditor: Auditor::default(),
            fog_violations: 0,
            times: StepTimes::default(),
            settings: None,
            saves: SaveState::new(!init.save_dir.is_empty(), now_ms),
            mods: None,
            owners: crate::owners::OwnersState::default(),
            load_controls: None,
            rep: ReplicaState::new(),
            tl: Timeline::new(u64::from(init.game)),
        };
        let mut fb = Fb::default();
        if init.ipc != IPC_VERSION {
            // Exit code 2: an IPC error (game-host-mp 4.12).
            fb.tail.push(WorkerMsg::Fatal {
                message: format!(
                    "IPC version {} of the gateway, this worker speaks {IPC_VERSION}",
                    init.ipc
                ),
                code: 2,
            });
            w.dead = true;
            let frame = fb.into_frame(None).map(Out::Frame);
            return (w, frame.into_iter().chain([Out::Exit(2)]).collect());
        }
        let r = w.begin(&init, save.as_ref(), &mut fb);
        let outs = w.finish(fb, now_ms, r);
        (w, outs)
    }

    fn begin(
        &mut self,
        init: &WorkerInit,
        save: Option<&Save>,
        fb: &mut Fb,
    ) -> Result<(), CoreError> {
        let setup = match &init.start {
            Start::New(setup) => setup,
            Start::Describe => {
                fb.head.push(WorkerMsg::Choices(self.core.choices()?));
                fb.head.push(self.catalog_msg()?);
                // `Ready` carries the build id the gateway keys its `Describe` cache on (GH-D17).
                fb.head.push(WorkerMsg::Ready(WorkerReady {
                    build_id: self.core.build_id(),
                    width: 0,
                    height: 0,
                    wrap_x: false,
                    wrap_y: false,
                    terrain_seed: 0,
                    seats: Vec::new(),
                    turn: 0,
                }));
                self.dead = true;
                return Ok(());
            }
            Start::Load { seats, .. } => {
                let Some(save) = save else {
                    return Err(CoreError::new(
                        CoreErrorKind::BadSave,
                        "no save was read for Start::Load",
                    ));
                };
                return self.begin_load(save, seats, fb);
            }
        };
        self.mode = setup.settings.turn_mode;
        self.settings = Some(setup.settings.clone());
        self.core.start(CoreStart::New {
            setup,
            seed: init.seed,
        })?;
        fb.head.push(self.catalog_msg()?);
        self.map = self.core.map();
        let seats = self.core.seats()?;
        self.seat_up(&seats, fb)
    }

    /// `Start::Load` (game-host-mp 4.12): the core continues from its part of the save; the slots belong to
    /// the owners the gateway names (`seats`); the rest of the save is taken by `adopt_save`.
    ///
    /// SIMPLIFIED(SIMP-exe-831): the loaded core goes on as the worker that wrote the save would only for what the save
    /// holds; the core keeps some AI state that no save carries (as in the original game), so the first AI phase after
    /// a restart can differ from the one without it (civ5-replay verify --reload shows such turns).
    fn begin_load(
        &mut self,
        save: &Save,
        named: &[protocol::SlotSetup],
        fb: &mut Fb,
    ) -> Result<(), CoreError> {
        let have = self.core.build_id();
        if save.info.header.build_id != have {
            return Err(CoreError::new(
                CoreErrorKind::BadSave,
                format!(
                    "the save was written by the core build {:?}, this is {have:?}",
                    save.info.header.build_id
                ),
            ));
        }
        self.mode = save.info.settings.turn_mode;
        self.settings = Some(save.info.settings.clone());
        self.core.start(CoreStart::Load(&save.gcs1))?;
        self.turn = save.info.header.turn;
        fb.head.push(self.catalog_msg()?);
        if let Some(m) = &save.mods {
            // The gateway starts the server halves of the mods from the state of this very save.
            self.mods = Some(m.clone());
            fb.head.push(WorkerMsg::ModsLoaded(m.clone()));
        }
        self.map = self.core.map();
        // The owners of the gateway are the truth; a slot it does not name stays as the core holds it.
        let mut seats = self.core.seats()?;
        for s in named {
            match seats.iter_mut().find(|x| x.player == s.player) {
                Some(x) => x.control = s.control.clone(),
                None => seats.push(s.clone()),
            }
        }
        seats.sort_by_key(|s| s.player);
        self.load_controls = Some(
            seats
                .iter()
                .map(|s| (s.player, s.control.clone()))
                .collect(),
        );
        self.seat_up(&seats, fb)?;
        self.adopt_save(save);
        Ok(())
    }

    /// The human seats of `seats`, the basis of the map, `Ready` and the first `Turn`.
    fn seat_up(&mut self, seats: &[protocol::SlotSetup], fb: &mut Fb) -> Result<(), CoreError> {
        for s in seats {
            if let SlotControl::Human { nick } = &s.control {
                self.seats.insert(s.player, Seat::new(nick.clone()));
            }
        }
        self.basis = self.build_basis()?;
        fb.head.push(WorkerMsg::Ready(WorkerReady {
            build_id: self.core.build_id(),
            width: self.map.width,
            height: self.map.height,
            wrap_x: self.map.wrap_x,
            wrap_y: self.map.wrap_y,
            terrain_seed: self.map.terrain_seed,
            seats: seats.to_vec(),
            turn: self.turn,
        }));
        self.refresh_turn(fb)?;
        Ok(())
    }

    fn catalog_msg(&mut self) -> Result<WorkerMsg, CoreError> {
        let catalog: Catalog = self.core.catalog()?;
        let bytes = Postcard
            .encode(&catalog)
            .map_err(|e| CoreError::failed(format!("catalog: {e}")))?;
        Ok(WorkerMsg::Catalog {
            hash: protocol::xxh64(0, &bytes),
            bytes,
        })
    }

    /// The map basis of the whole map (once per game): `Core::basis` with no subset.
    fn build_basis(&mut self) -> Result<BasisState, CoreError> {
        let n = self.map.plot_count();
        let mut out = Vec::with_capacity(n);
        self.core.basis(None, &mut out)?;
        if out.len() != n || out.iter().enumerate().any(|(i, (j, _))| i != *j as usize) {
            return Err(CoreError::failed(format!(
                "core: the basis has {} records for {n} plots",
                out.len()
            )));
        }
        let mut m = MapBasis {
            game: self.game,
            basis_hash: 0,
            width: self.map.width,
            height: self.map.height,
            wrap_x: self.map.wrap_x,
            wrap_y: self.map.wrap_y,
            terrain_seed: self.map.terrain_seed,
            plots: out.into_iter().map(|(_, b)| b).collect(),
        };
        m.basis_hash = basis_hash_of(&m);
        BasisState::new(m).map_err(|e| CoreError::failed(format!("basis: {e}")))
    }

    /// True when `step` has something to do (game-host-mp 4.2 items 1-5).
    pub fn wants_step(&self) -> bool {
        if self.dead {
            return false;
        }
        match self.phase {
            TurnPhase::Starting | TurnPhase::Ai { .. } => true,
            TurnPhase::Humans => {
                self.pump_due
                    || !self.queue.is_empty()
                    || (!self.pending_seats.is_empty() && !self.seat_blocked)
                    || self.rep.wants_step()
                    || self.has_stale_contexts()
            }
            TurnPhase::GameOver { .. } => false,
        }
    }

    /// The time (the clock of `now_ms`) the shell should call `step` at the latest even if `wants_step` is
    /// false: the next heartbeat, or the next try of a seat change that found no safe point.
    pub fn next_wake_ms(&self) -> Option<u64> {
        let beat = self.last_frame_ms + 1000;
        (!self.dead).then(|| {
            let beat = if self.seat_blocked && !self.pending_seats.is_empty() {
                beat.min(self.seat_retry_ms)
            } else {
                beat
            };
            let beat = self.save_wake_ms().map_or(beat, |at| beat.min(at));
            self.rep.wake_ms().map_or(beat, |at| beat.min(at))
        })
    }

    /// Fog violations the auditor found since the start (`WorkerConfig.audit_fog`).
    pub fn fog_violations(&self) -> u64 {
        self.fog_violations
    }

    /// The turn the worker is in and its phase.
    pub fn phase(&self) -> (i32, TurnPhase) {
        (self.turn, self.phase)
    }

    /// The core, for a test or a bench that needs to look past the worker.
    pub fn core_mut(&mut self) -> &mut C {
        &mut self.core
    }

    /// One piece of work: a pump, a batch of orders (game-host-mp 4.2).
    pub fn step(&mut self, now_ms: u64) -> Vec<Out> {
        if self.dead {
            return Vec::new();
        }
        let mut fb = Fb::default();
        self.times = StepTimes::default();
        self.now_ms = now_ms;
        if self.seat_blocked && now_ms >= self.seat_retry_ms {
            self.seat_blocked = false;
        }
        self.final_expired();
        if self.dead {
            return self.finish(fb, now_ms, Ok(()));
        }
        let save_before = self.times.save_us;
        let r = self
            .replica_before_step(&mut fb)
            .and_then(|()| self.step_inner(&mut fb))
            .and_then(|()| self.replica_views(&mut fb))
            .and_then(|()| self.try_save(&mut fb))
            .map(|()| self.tl_step_end(save_before))
            .and_then(|()| self.replica_after_step(&mut fb));
        self.finish(fb, now_ms, r)
    }

    /// The order of 4.2: orders, then seat changes at a safe point, then one background context.
    fn step_inner(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        match self.phase {
            TurnPhase::Starting | TurnPhase::Ai { .. } => self.pump_step(fb),
            TurnPhase::Humans if self.pump_due => self.deferred_pump(fb),
            TurnPhase::Humans if !self.queue.is_empty() => self.run_batch(fb),
            TurnPhase::Humans if !self.pending_seats.is_empty() && !self.seat_blocked => {
                self.seat_step(fb)
            }
            TurnPhase::Humans => self.ctx_background(fb),
            TurnPhase::GameOver { .. } => Ok(()),
        }
    }

    /// Takes the phase and the turn the core reports.
    pub(crate) fn apply_pump(&mut self, p: CorePump) {
        self.turn = p.turn;
        self.phase = match p.phase {
            CorePhase::Humans => TurnPhase::Humans,
            CorePhase::Ai { active } => TurnPhase::Ai { active },
            CorePhase::GameOver { winner } => TurnPhase::GameOver { winner },
        };
    }

    /// `Starting` or `Ai`: runs the core; when the humans move again (or the game ends) the views of all seats
    /// are built.
    fn pump_step(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        let before = self.phase;
        let (p, us) = self.pump_core(false)?;
        self.times.pump_us = us;
        self.apply_pump(p);
        self.collect_events();
        fb.kind = Some(match (before, self.phase) {
            (TurnPhase::Ai { .. }, TurnPhase::Ai { .. }) => StepKind::AiPhase,
            _ => StepKind::TurnChange,
        });
        if matches!(self.phase, TurnPhase::Humans | TurnPhase::GameOver { .. }) {
            if self.cfg.digests {
                // The state at the end of the turn, for the tests that compare two runs (AC-N1.4).
                self.times.digest = self.core.digest(-1).ok();
            }
            // After a load: the owners the gateway named, before any view (game-host-mp 4.12).
            // A kick that is not over (the AI plays its turn) holds the views back until the humans are up.
            if self.apply_load_controls()? {
                self.build_round(fb)?;
                if self.phase == TurnPhase::Humans {
                    // The save of the start of the turn: after the views, before the queued orders (4.11).
                    self.save_at_turn_start();
                }
            }
        }
        self.refresh_turn(fb)?;
        Ok(())
    }

    /// Moves the events of the core into the journals of the seats.
    pub(crate) fn collect_events(&mut self) {
        for ev in self.core.events() {
            self.journal_pending
                .route(ev.to, self.seats.keys().copied(), &ev.entry);
        }
    }

    /// Updates the map basis from the core for the plots that changed and returns the changed records: the
    /// part of every seat's diff (game-host-mp 4.3 item 5).
    fn update_basis(
        &mut self,
        dirty: Option<&[u32]>,
    ) -> Result<Vec<(u32, protocol::PlotBasisView)>, CoreError> {
        let mut fresh = Vec::new();
        match dirty {
            Some([]) => return Ok(Vec::new()),
            Some(d) => self.core.basis(Some(d), &mut fresh)?,
            None => self.core.basis(None, &mut fresh)?,
        }
        let changes = diff_basis(self.basis.map(), &fresh);
        if !changes.is_empty() {
            let mut view = empty_view();
            let mut hash = ViewHash::of(&view);
            let d = ViewDiff {
                basis: changes.clone(),
                ..ViewDiff::default()
            };
            protocol::apply(&mut view, &mut hash, &mut self.basis, &d)
                .map_err(|e| CoreError::failed(format!("basis diff: {e}")))?;
        }
        Ok(changes)
    }

    /// Builds the view of every seat and puts the changes into `fb`: `Full` (with `Basis` before it) for a seat
    /// that has no view yet, a diff against its last view for the others (game-host-mp 4.3).
    pub(crate) fn build_round(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        let t_views = Instant::now();
        // This round carries what the UI messages of the step changed as well.
        self.rep.views_due = false;
        self.expire_offers();
        let dirty = self.core.dirty_plots();
        let dirty = dirty.as_deref();
        let basis_diff = self.update_basis(dirty)?;
        self.owners_round(dirty, fb)?;
        let players: Vec<PlayerId> = self.seats.keys().copied().collect();
        let mut parts: BTreeMap<
            PlayerId,
            (
                protocol::ViewHeader,
                TopBar,
                Vec<protocol::NotificationView>,
            ),
        > = BTreeMap::new();
        for &p in &players {
            let header = self.core.header(p)?;
            let top_bar = self.core.top_bar(p)?;
            let mut notifications = self.core.notifications(p)?;
            notifications.sort_by_key(|n| n.id);
            if let Some(seat) = self.seats.get_mut(&p) {
                seat.team = header.human_team;
            }
            parts.insert(p, (header, top_bar, notifications));
        }
        let teams: BTreeSet<TeamId> = self.seats.values().map(|s| s.team).collect();
        for t in teams {
            let data = self.teams.entry(t).or_insert_with(|| TeamData::new(t));
            data.refresh(&mut self.core, t, &self.map, dirty)?;
        }
        self.times.views_us += micros(t_views);

        let t_diff = Instant::now();
        let mut touches: Vec<(PlayerId, Touch)> = Vec::new();
        for p in players {
            let Some((header, top_bar, notifications)) = parts.remove(&p) else {
                continue;
            };
            let Some(seat) = self.seats.get_mut(&p) else {
                continue;
            };
            let Some(team) = self.teams.get(&seat.team) else {
                continue;
            };
            let seat_parts = SeatParts {
                header,
                top_bar,
                notifications,
                peace_offers: &seat.peace_offers,
            };
            match seat.mirror.as_mut() {
                None => {
                    let view = assemble_view(&self.map, team, seat_parts, true);
                    let hash = ViewHash::of(&view);
                    seat.rev += 1;
                    fb.views.insert(
                        p,
                        ViewUpdate::Full {
                            rev: seat.rev,
                            view: view.clone(),
                            hash: hash.value(),
                        },
                    );
                    fb.basis_for.push(p);
                    seat.mirror = Some(Mirror { view, hash });
                    touches.push((p, Touch::all()));
                }
                Some(mirror) => {
                    // Without a dirty list every plot is compared: the new view needs its plots then.
                    let new = assemble_view(&self.map, team, seat_parts, dirty.is_none());
                    let compare: Option<&[u32]> = if dirty.is_none() { None } else { Some(&[]) };
                    let mut d = protocol::diff(&mirror.view, &new, compare);
                    if let Some(list) = dirty {
                        for &i in list {
                            let (i, now) = (i as usize, team.plots.get(i as usize));
                            if let (Some(old), Some(now)) = (mirror.view.map.plots.get(i), now)
                                && old != now
                            {
                                d.plots.push((i as u32, now.clone()));
                            }
                        }
                        d.plots.sort_by_key(|(i, _)| *i);
                        d.plots.dedup_by_key(|(i, _)| *i);
                    }
                    d.basis = basis_diff.clone();
                    if d.is_empty() {
                        continue;
                    }
                    let touch = Touch::of(&d, &mirror.view, &self.map);
                    let basis_part = std::mem::take(&mut d.basis);
                    protocol::apply(&mut mirror.view, &mut mirror.hash, &mut self.basis, &d)
                        .map_err(|e| CoreError::failed(format!("view diff of seat {p}: {e}")))?;
                    d.basis = basis_part;
                    let basis_hash = (!d.basis.is_empty()).then(|| self.basis.hash());
                    if self.cfg.stats {
                        let bytes = Postcard.encode(&d).map_or(0, |b| b.len());
                        self.times
                            .diff_bytes
                            .push((p, u32::try_from(bytes).unwrap_or(u32::MAX)));
                    }
                    let base_rev = seat.rev;
                    seat.rev += 1;
                    fb.views.insert(
                        p,
                        ViewUpdate::Delta {
                            base_rev,
                            rev: seat.rev,
                            diff: d,
                            hash: mirror.hash.value(),
                            basis_hash,
                        },
                    );
                    touches.push((p, touch));
                }
            }
        }
        self.times.diff_us += micros(t_diff);
        for (p, touch) in &touches {
            self.ctx_after_view(*p, touch, fb);
        }
        self.flush_journal(fb);
        self.audit_round(fb);
        Ok(())
    }

    /// Hands the journal lines of the core to the frame and to the history of the seats.
    pub(crate) fn flush_journal(&mut self, fb: &mut Fb) {
        for (p, entries) in self.journal_pending.take() {
            if entries.is_empty() {
                continue;
            }
            if let Some(seat) = self.seats.get_mut(&p) {
                journal::remember(&mut seat.journal, &entries);
                fb.journal.entry(p).or_default().extend(entries);
            }
        }
    }

    /// The view of `player` built from scratch: every plot asked, nothing taken from the earlier rounds, the
    /// memo of cities only read. For tests: it must equal the view the diffs of the worker build up (the
    /// check that the dirty plots of the core were complete, game-host-mp section 9).
    ///
    /// # Errors
    /// No such seat, or the core failed.
    pub fn fresh_view(&mut self, player: PlayerId) -> Result<WorldView, CoreError> {
        let seat = self
            .seats
            .get(&player)
            .ok_or_else(|| CoreError::failed(format!("no seat {player}")))?;
        let team_id = seat.team;
        let peace = seat.peace_offers.clone();
        let header = self.core.header(player)?;
        let top_bar = self.core.top_bar(player)?;
        let mut notifications = self.core.notifications(player)?;
        notifications.sort_by_key(|n| n.id);
        let mut data = self
            .teams
            .get(&team_id)
            .map_or_else(|| TeamData::new(team_id), TeamData::reading_copy);
        data.reload(&mut self.core, &self.map)?;
        Ok(assemble_view(
            &self.map,
            &data,
            SeatParts {
                header,
                top_bar,
                notifications,
                peace_offers: &peace,
            },
            true,
        ))
    }

    /// The hash the worker holds for the view of `player`, as the gateway will see it.
    pub fn view_hash(&self, player: PlayerId) -> Option<u64> {
        self.seats
            .get(&player)?
            .mirror
            .as_ref()
            .map(|m| m.hash.value())
    }

    /// Turns what the call produced into the frame and the exit, after a core failure the `Fatal`.
    pub(crate) fn finish(&mut self, mut fb: Fb, now_ms: u64, r: Result<(), CoreError>) -> Vec<Out> {
        let mut outs = Vec::new();
        let mut code = None;
        if let Err(e) = r {
            tracing::error!(kind = ?e.kind, "worker stops: {}", e.message);
            let c = exit_code(e.kind);
            fb.tail.push(WorkerMsg::Fatal {
                message: e.message,
                code: c,
            });
            code = Some(c);
        }
        self.tl_flush(&mut fb);
        if self.cfg.stats && fb.kind.is_some() && code.is_none() {
            let t = std::mem::take(&mut self.times);
            fb.tail.push(WorkerMsg::Stats(
                t.into_stats(fb.kind.unwrap_or(StepKind::Other)),
            ));
        }
        let heartbeat_due = now_ms.saturating_sub(self.last_frame_ms) >= 1000;
        if fb.is_empty() && heartbeat_due && !self.dead && code.is_none() {
            fb.tail.push(WorkerMsg::Heartbeat {
                phase: self.phase,
                queued: u32::try_from(self.queue.len()).unwrap_or(u32::MAX),
            });
        }
        // The basis is cloned only when a frame carries it.
        let basis = (!fb.basis_for.is_empty()).then(|| self.basis.map().clone());
        if let Some(f) = fb.into_frame(basis) {
            self.last_frame_ms = now_ms;
            outs.push(Out::Frame(f));
        }
        let jobs = self.saves.take_jobs();
        let replica_jobs = self.rep.take_jobs();
        if code.is_none() {
            outs.extend(jobs);
            outs.extend(replica_jobs);
        }
        if let Some(c) = code {
            self.dead = true;
            outs.push(Out::Exit(c));
        } else if self.dead {
            outs.push(Out::Exit(0));
        }
        outs
    }

    /// A message from the gateway.
    pub fn handle(&mut self, msg: ToWorker, now_ms: u64) -> Vec<Out> {
        if self.dead {
            return Vec::new();
        }
        let mut fb = Fb::default();
        self.now_ms = now_ms;
        let r = self.handle_inner(msg, &mut fb);
        self.finish(fb, now_ms, r)
    }

    fn handle_inner(&mut self, msg: ToWorker, fb: &mut Fb) -> Result<(), CoreError> {
        match msg {
            ToWorker::Init(_) => {
                return Err(CoreError::failed("a second Init"));
            }
            ToWorker::Cmd { player, cmd } => self.accept_cmd(player, cmd, fb),
            ToWorker::Query { player, qid, q } => {
                let answer = self.answer_query(player, q);
                fb.tail.push(WorkerMsg::QueryResult { qid, answer });
            }
            ToWorker::Focus { player, focus } => self.set_focus(player, focus),
            ToWorker::WantFull { player, basis } => self.want_full(player, basis, fb),
            // A seat change waits for a safe point (the next step in the human phase).
            ToWorker::Seat { player, change } => self.pending_seats.push_back((player, change)),
            ToWorker::ForceEndTurn { players } => {
                for p in players {
                    if self.seats.contains_key(&p) {
                        self.queue.push_timeout(p, self.turn);
                    }
                }
            }
            ToWorker::SaveNow { .. } => self.save_now(fb)?,
            ToWorker::Shutdown { save } => self.shutdown(save, fb)?,
            ToWorker::Net { player, msgs } => self.replica_net(player, msgs, fb),
            ToWorker::ReplicaSnapshot { player } => self.replica_resync(player),
            // A block that does not fit the bounds of the section would make every save fail: it is dropped.
            ToWorker::WantOwners => self.want_owners(fb)?,
            ToWorker::Mods(m) if m.within_limits() => self.mods = Some(m),
            ToWorker::Mods(_) => {
                tracing::warn!("the mods block of the gateway is too big, ignored")
            }
        }
        Ok(())
    }

    /// The whole view of a seat again (a hash did not match at the gateway), with the basis on request.
    fn want_full(&mut self, player: PlayerId, with_basis: bool, fb: &mut Fb) {
        let Some(seat) = self.seats.get_mut(&player) else {
            return;
        };
        let Some(mirror) = seat.mirror.as_mut() else {
            // No view yet: the first round sends `Full`.
            return;
        };
        mirror.hash = ViewHash::of(&mirror.view);
        seat.rev += 1;
        fb.views.insert(
            player,
            ViewUpdate::Full {
                rev: seat.rev,
                view: mirror.view.clone(),
                hash: mirror.hash.value(),
            },
        );
        if with_basis {
            fb.basis_for.push(player);
        }
        // The view is the same with a new revision: the contexts it had stay good.
        self.ctx_after_view(player, &Touch::none(), fb);
    }

    /// Answers of the core to a query of a seat, in any phase with a game (as in B1).
    fn answer_query(&mut self, player: PlayerId, q: Query) -> QueryAnswer {
        let err = |e: CoreError| QueryAnswer::Error { reason: e.message };
        let Some(seat) = self.seats.get(&player) else {
            return QueryAnswer::Error {
                reason: format!("player {player} has no seat"),
            };
        };
        if matches!(self.phase, TurnPhase::Starting) {
            return QueryAnswer::Error {
                reason: "the game is starting".into(),
            };
        }
        let team = seat.team;
        let cities = seat.mirror.as_ref().map(|m| m.view.cities.clone());
        match q {
            Query::UnitActions { unit } => self
                .core
                .unit_actions(player, unit)
                .map_or_else(err, QueryAnswer::UnitActions),
            Query::CityBuilds { city } => self
                .core
                .city_builds(player, city)
                .map_or_else(err, QueryAnswer::CityBuilds),
            Query::Path { unit, x, y } => self
                .core
                .path(player, unit, x, y)
                .map_or_else(err, QueryAnswer::Path),
            Query::Reachable { unit } => {
                self.core.reach(player, unit, 1).map_or_else(err, |tree| {
                    QueryAnswer::Reachable(tree.iter().map(|n| (n.x, n.y)).collect())
                })
            }
            Query::RangeTargets { from } => self.core.targets(player, from).map_or_else(err, |t| {
                QueryAnswer::RangeTargets(t.into_iter().map(|(x, y, _)| (x, y)).collect())
            }),
            Query::Units => self.core.units(team).map_or_else(err, QueryAnswer::Units),
            Query::Cities => cities.map_or_else(
                || QueryAnswer::Error {
                    reason: "no view yet".into(),
                },
                QueryAnswer::Cities,
            ),
            Query::TopBar => self
                .core
                .top_bar(player)
                .map_or_else(err, QueryAnswer::TopBar),
            Query::Notifications => self
                .core
                .notifications(player)
                .map_or_else(err, QueryAnswer::Notifications),
            Query::Reach { unit, horizon } => self
                .core
                .reach(player, unit, horizon)
                .map_or_else(err, QueryAnswer::Reach),
            Query::CombatPreview { attacker, x, y } => self
                .core
                .preview(player, attacker, x, y)
                .map_or_else(err, QueryAnswer::CombatPreview),
        }
    }

    /// The human seats as `TurnInfo` lists them before the core has said anything.
    pub(crate) fn starting_seats(&self) -> Vec<SeatTurnInfo> {
        self.seats
            .keys()
            .map(|&player| SeatTurnInfo {
                player,
                finished: false,
                may_act: false,
                simultaneous: false,
            })
            .collect()
    }
}

/// A view with nothing in it: the carrier for applying a diff that touches the basis only.
fn empty_view() -> WorldView {
    WorldView {
        turn: 0,
        year: 0,
        human: 0,
        human_team: 0,
        status: protocol::TurnStatus::HumanTurn,
        blocker: None,
        map: protocol::MapView::default(),
        units: Vec::new(),
        cities: Vec::new(),
        players: Vec::new(),
        top_bar: TopBar::default(),
        notifications: Vec::new(),
        peace_offers: Vec::new(),
        trade_routes: Vec::new(),
    }
}
