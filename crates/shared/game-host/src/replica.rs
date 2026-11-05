//! The replica stream of the worker: the journal of the core's inputs
//! (GCJ1) goes out as numbered batches while the humans move, a snapshot (GCS1) of the start of every human
//! phase goes out as a packed body, the UI messages of the seats (GCN1) go into the core.
//!
//! The core has to be the server of the replica (`Core::replica`); the test world is not, and the worker then
//! sends nothing of this.
//!
//! * Entering a human phase (or a new turn inside it): the journal of the AI phase is dropped, the batches stop
//!   (`blocked`), a snapshot is made at the first safe point and packed on the save thread
//!   ([`ReplicaJob::pack`]); when the packed body is back ([`Worker::replica_done`]) the gateway hears
//!   `ReplicaBlob` and `ReplicaBase` (first time) or `ReplicaTurn`, and the batches go on.
//! * The human phase: after every step the batch is taken (`journal_take`), `jseq` grows by one for each batch
//!   that is sent. `CHECK` is asked for at most once a second, and always for the batch of the step that ends the
//!   human phase.
//! * The end of the human phase: the step that applies an end-of-turn order asks for a `CHECK` batch before the
//!   core runs on (`Worker::replica_before_pump`), so the last batch of the phase is checked and holds nothing of
//!   the AI phase. The rest of the journal of the step is dropped.
//! * The AI phase: nothing goes out; the journal of the core is dropped when the humans are up again. With
//!   `WorkerConfig.record` the dropped batches go to the gateway as `WorkerMsg::RecJournal` (`Ai`, and `Cut` for what
//!   the core did right before a snapshot), so the recording holds every input of the game.
//! * A snapshot of a later turn goes out with a patch from the previous one when the shell gives the job a patch
//!   encoder (zstd, server side); the full body is always there as well.
//! * `ToWorker::ReplicaSnapshot { player }`: a snapshot at the next safe point, answered with `ReplicaBase` for
//!   that seat only; the batches are not held back for it.
//! * `ToWorker::Net`: handed to the core in the next step, all or nothing; a refusal is `NetRejected`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use protocol::{Command, PlayerId, RecPhase, TurnPhase, WorkerMsg, xxh64};

use crate::core::{Core, CoreError, CoreErrorKind};
use crate::timeline::PackTimes;
use crate::worker::{Fb, Out, Worker};

/// What the worker needs of a core that is the server of a replica (`Core::replica`).
pub trait ReplicaOps {
    /// The core's inputs since the last call as GCJ1. `check` ends a non-empty batch with the replica digest.
    /// The answer may be a batch of no records.
    ///
    /// # Errors
    /// The core failed.
    fn journal_take(&mut self, check: bool) -> Result<Vec<u8>, CoreError>;

    /// The GCN1 messages of the UI of `player`, all or nothing. `Err` is the reason for `NetRejected`.
    fn net_submit(&mut self, player: PlayerId, msgs: &[u8]) -> Result<(), String>;
}

/// `CHECK` records cost a digest of the whole game: not more often than this.
pub const CHECK_EVERY_MS: u64 = 1000;
/// The wait before the next look for a safe point for a snapshot.
const RETRY_MS: u64 = 50;
/// The wait after `gc_save` refused a snapshot (`NotSafe`: a human's unit is still busy); the core is pumped
/// meanwhile.
const SNAP_NOT_SAFE_RETRY_MS: u64 = 250;
/// The biggest packed body the IPC frame holds with room to spare (`wire::ipc::MAX_FRAME` is 16 MiB).
const MAX_BODY: usize = 15 << 20;

/// What a snapshot is for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    /// The first snapshot of the game: `ReplicaBase` to everybody.
    Base,
    /// The start of a later human phase: `ReplicaTurn` to everybody.
    Turn,
    /// A seat asked for it: `ReplicaBase` for these seats.
    Resync(Vec<PlayerId>),
}

/// A patch encoder: the zstd frame of `cur` against `prev` as its raw prefix (long-distance matching), the
/// shell's part (the C library is not in this crate).
pub type PatchFn = dyn Fn(&[u8], &[u8]) -> Result<Vec<u8>, String> + Send + Sync;

/// A snapshot encoder: the zstd frame of the GCS1 bytes (the shell's part, like [`PatchFn`]).
pub type SnapFn = dyn Fn(&[u8]) -> Result<Vec<u8>, String> + Send + Sync;

/// A snapshot to pack: the work of the pack thread, answered with [`ReplicaDone`] (`Worker::replica_done`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaJob {
    pub id: u64,
    /// The core's part of a save (GCS1), not packed.
    pub gcs1: Arc<Vec<u8>>,
    /// The snapshot of the previous turn start, for the patch.
    pub prev: Option<(Arc<Vec<u8>>, u64)>,
}

/// The packed snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaDone {
    pub id: u64,
    /// The packed snapshot and its patch, or why it failed.
    pub result: Result<ReplicaPacked, String>,
}

/// What a job gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaPacked {
    pub snapshot: ReplicaBody,
    /// The patch from `prev` and the `raw_hash` of `prev`; `None`: no previous snapshot, no encoder, a patch that
    /// is not smaller, or a failure (the full body serves).
    pub patch: Option<(ReplicaBody, u64)>,
    /// Time spent: the full body, the patch.
    pub pack_us: u64,
    pub patch_us: u64,
}

/// A packed body, `wire::replica` encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplicaBody {
    /// xxh64 of `bytes`.
    pub hash: u64,
    /// xxh64 of the unpacked GCS1.
    pub raw_hash: u64,
    pub bytes: Vec<u8>,
}

impl ReplicaJob {
    /// Packs the snapshot and, with `patch` and a previous snapshot, the patch. Pure CPU, meant for the pack
    /// thread.
    #[must_use]
    pub fn pack(self, snap: Option<&SnapFn>, patch: Option<&PatchFn>) -> ReplicaDone {
        ReplicaDone {
            id: self.id,
            result: self.pack_inner(snap, patch),
        }
    }

    fn pack_inner(
        self,
        snap: Option<&SnapFn>,
        patch: Option<&PatchFn>,
    ) -> Result<ReplicaPacked, String> {
        let t = Instant::now();
        let raw_hash = xxh64(0, &self.gcs1);
        let bytes = match snap {
            Some(encode) => {
                let frame = encode(&self.gcs1)?;
                wire::replica::snapshot_body(self.gcs1.len(), raw_hash, &frame)
            }
            None => wire::replica::encode_snapshot(&self.gcs1),
        }
        .map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BODY {
            return Err(format!("the packed snapshot has {} bytes", bytes.len()));
        }
        let pack_us = micros_since(t);
        let snapshot = ReplicaBody {
            hash: xxh64(0, &bytes),
            raw_hash,
            bytes,
        };
        let t = Instant::now();
        let mut made = None;
        if let (Some(encode), Some((prev, from_hash))) = (patch, &self.prev) {
            match encode(prev, &self.gcs1) {
                Ok(frame) => {
                    match wire::replica::patch_body(self.gcs1.len(), raw_hash, *from_hash, &frame) {
                        Ok(body) if body.len() < snapshot.bytes.len() => {
                            made = Some((
                                ReplicaBody {
                                    hash: xxh64(0, &body),
                                    raw_hash,
                                    bytes: body,
                                },
                                *from_hash,
                            ));
                        }
                        Ok(_) => tracing::debug!("the patch is not smaller than the snapshot"),
                        Err(e) => tracing::warn!("the replica patch is refused: {e}"),
                    }
                }
                Err(e) => tracing::warn!("the replica patch failed: {e}"),
            }
        }
        Ok(ReplicaPacked {
            snapshot,
            patch: made,
            pack_us,
            patch_us: micros_since(t),
        })
    }
}

fn micros_since(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// A snapshot being packed.
#[derive(Debug)]
struct Inflight {
    kind: Kind,
    turn: i32,
    jseq: u64,
    /// The raw snapshot: after a base or a turn it is the previous snapshot of the next one.
    gcs1: Arc<Vec<u8>>,
    /// When the job was made (the timeline's clock).
    queued_us: u64,
}

/// The replica state of the worker.
#[derive(Debug)]
pub(crate) struct ReplicaState {
    pub epoch: u32,
    /// Number of the last batch sent.
    jseq: u64,
    /// The first snapshot has been made: later ones are turns.
    announced: bool,
    /// The humans were up at the end of the last step, in this turn.
    in_humans: Option<i32>,
    /// The snapshot of the end of the game was asked for (once).
    over_snap: bool,
    /// A snapshot of the turn start is due (waits for a safe point).
    snap_due: bool,
    last_check_ms: Option<u64>,
    retry_ms: u64,
    /// Seats that asked for a snapshot and wait for a safe point.
    resyncs: BTreeSet<PlayerId>,
    nets: VecDeque<(PlayerId, Vec<u8>)>,
    /// The core took UI messages in this step: the views of the seats are built after the step's work, once
    /// (`build_round` clears it). Without it the server views (the 3D map of the clients) stayed at the start
    /// of the turn while the seats played by GCN1 (live report 2026-10-30).
    pub views_due: bool,
    next_job: u64,
    inflight: BTreeMap<u64, Inflight>,
    jobs: Vec<ReplicaJob>,
    /// The core's part saved in this step for the save file, shared with the snapshot.
    shared: Option<Arc<Vec<u8>>>,
    /// The snapshot of the last turn start that went out, and its `raw_hash`: the base of the next patch.
    prev: Option<(Arc<Vec<u8>>, u64)>,
    /// Messages for the gateway made outside `replica_after_step` (before the pump of an order).
    out: Vec<WorkerMsg>,
}

impl ReplicaState {
    pub fn new() -> Self {
        ReplicaState {
            epoch: 1,
            jseq: 0,
            announced: false,
            in_humans: None,
            over_snap: false,
            snap_due: false,
            last_check_ms: None,
            retry_ms: 0,
            resyncs: BTreeSet::new(),
            nets: VecDeque::new(),
            views_due: false,
            next_job: 1,
            inflight: BTreeMap::new(),
            jobs: Vec::new(),
            shared: None,
            prev: None,
            out: Vec::new(),
        }
    }

    /// The jobs made in this call, for the shell.
    pub fn take_jobs(&mut self) -> Vec<Out> {
        self.jobs
            .drain(..)
            .map(|j| Out::Replica(Box::new(j)))
            .collect()
    }

    /// The batches are held back: a snapshot of a turn start is due or is being packed.
    pub fn blocked(&self) -> bool {
        self.snap_due
            || self
                .inflight
                .values()
                .any(|i| !matches!(i.kind, Kind::Resync(_)))
    }

    /// Messages for the gateway wait for the end of the step (the batch of an end of turn, `replica_before_pump`).
    pub fn has_out(&self) -> bool {
        !self.out.is_empty()
    }

    /// Work that needs a step now.
    pub fn wants_step(&self) -> bool {
        !self.nets.is_empty()
    }

    /// The time a snapshot that waits for a safe point is tried again.
    pub fn wake_ms(&self) -> Option<u64> {
        (self.snap_due || !self.resyncs.is_empty()).then_some(self.retry_ms)
    }

    /// A snapshot is being made or packed for the start of a phase.
    pub fn snapshot_pending(&self) -> bool {
        self.snap_due || !self.resyncs.is_empty()
    }

    /// The core's part saved for a save file in this step: kept for the snapshot, which the same state needs.
    pub fn offer(&mut self, gcs1: &[u8]) {
        if self.snapshot_pending() {
            self.shared = Some(Arc::new(gcs1.to_vec()));
        }
    }
}

/// True when a GCJ1 batch holds no record.
fn journal_is_empty(batch: &[u8]) -> bool {
    match batch.get(8..12) {
        Some(n) => n == [0, 0, 0, 0],
        None => true,
    }
}

impl<C: Core> Worker<C> {
    /// The shell's epoch of this process: a new one whenever the worker is started again (a restart, a load), so
    /// the clients begin their chain with a new base.
    pub fn set_replica_epoch(&mut self, epoch: u32) {
        self.rep.epoch = epoch;
    }

    /// `ToWorker::Net`.
    pub(crate) fn replica_net(&mut self, player: PlayerId, msgs: Vec<u8>, fb: &mut Fb) {
        let why = if self.core.replica().is_none() {
            Some("this game has no replica stream")
        } else if self.phase != TurnPhase::Humans {
            Some("the AI is moving")
        } else if !self.seats.contains_key(&player) {
            Some("the player has no seat")
        } else {
            None
        };
        match why {
            Some(reason) => fb.tail.push(WorkerMsg::NetRejected {
                player,
                reason: reason.to_owned(),
            }),
            None => self.rep.nets.push_back((player, msgs)),
        }
    }

    /// `ToWorker::ReplicaSnapshot`.
    pub(crate) fn replica_resync(&mut self, player: PlayerId) {
        if self.core.replica().is_none() || !self.seats.contains_key(&player) {
            return;
        }
        // A snapshot for everybody is on its way: it answers the seat too.
        if self.rep.blocked()
            || !matches!(self.phase, TurnPhase::Humans | TurnPhase::GameOver { .. })
        {
            return;
        }
        self.rep.resyncs.insert(player);
    }

    /// Before the orders of a step: the UI messages the gateway passed on. The core takes them at its next pump:
    /// it runs after an accepted message like after an order.
    pub(crate) fn replica_before_step(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        // An end of turn waits for its pump (`pump_due`): the UI messages wait for the step after it (the human phase
        // may be over by then, and they are refused as the AI's).
        if self.rep.nets.is_empty() || self.pump_due {
            return Ok(());
        }
        let nets = std::mem::take(&mut self.rep.nets);
        let mut taken = false;
        for (player, msgs) in nets {
            // A UI message of a seat that has ended the turn takes the end of turn back first, as the exe's
            // `CvGameController::BroadcastGameplayMessage` (0x08758d76) sends NetTurnUnready ahead of a gameplay
            // message. The replica's `CancelEndTurn` order comes too, but in the batch after this (the UI messages
            // go first in a step), and the core refuses a message of a player whose turn is complete: the first
            // move after END TURN was lost (live report 2026-10-31). The order that follows finds nothing to take
            // back (the core's CANCEL_END_TURN is idempotent).
            if self.phase == TurnPhase::Humans
                && self.seats.get(&player).is_some_and(|s| s.finished)
            {
                let r = self.core.command(player, &Command::CancelEndTurn);
                self.collect_events();
                match r {
                    Ok(()) => {
                        if let Some(seat) = self.seats.get_mut(&player) {
                            seat.finished = false;
                        }
                        self.rep.views_due = true;
                    }
                    Err(e) => tracing::info!(
                        "seat {player}: the end of turn ahead of a UI message is not taken back: {}",
                        e.text
                    ),
                }
            }
            let r = match self.core.replica() {
                Some(ops) if self.phase == TurnPhase::Humans => ops.net_submit(player, &msgs),
                Some(_) => Err("the AI is moving".to_owned()),
                None => Err("this game has no replica stream".to_owned()),
            };
            match r {
                Ok(()) => taken = true,
                Err(reason) => fb.tail.push(WorkerMsg::NetRejected { player, reason }),
            }
        }
        if taken {
            self.note_applied();
            self.pump_after_order(false)?;
            self.rep.views_due = true;
        }
        Ok(())
    }

    /// After the step's work: the views of the UI messages the core took, unless the step built them already (an
    /// order batch or a turn change runs `build_round`, which clears the flag). One round per step: a second one
    /// would replace the seat's update in the frame and break its chain of revisions.
    pub(crate) fn replica_views(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        if !std::mem::take(&mut self.rep.views_due) {
            return Ok(());
        }
        if !matches!(self.phase, TurnPhase::Humans | TurnPhase::GameOver { .. }) {
            // The AI moves (or the game starts): the views come when the humans are up again.
            return Ok(());
        }
        self.build_round(fb)?;
        self.refresh_turn(fb)
    }

    /// The core's part for a save file and for a snapshot: one call when both are due.
    pub(crate) fn core_save_shared(&mut self) -> Result<Vec<u8>, CoreError> {
        let gcs1 = self.core.save()?;
        self.rep.offer(&gcs1);
        Ok(gcs1)
    }

    /// Before the core runs on after an order. An end-of-turn order may end the human phase: the batch up to it
    /// goes out now, ending with a `CHECK`, so that the last batch of the phase is checked and holds nothing the
    /// AI phase does.
    pub(crate) fn replica_before_pump(&mut self, ends_turn: bool) -> Result<(), CoreError> {
        if !ends_turn || self.phase != TurnPhase::Humans || self.rep.in_humans != Some(self.turn) {
            return Ok(());
        }
        if self.core.replica().is_none() || self.rep.blocked() || self.rep.snapshot_pending() {
            return Ok(());
        }
        let mut fb = Fb::default();
        let (t0, jseq) = (self.tl_now(), self.rep.jseq);
        self.replica_batch(true, &mut fb)?;
        self.tl_check_batch(t0, jseq, self.rep.jseq);
        self.rep.out.append(&mut fb.replica);
        Ok(())
    }

    /// After a step: the batch of the journal, the end of the human phase, the snapshots.
    pub(crate) fn replica_after_step(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        fb.replica.append(&mut self.rep.out);
        if self.core.replica().is_none() {
            return Ok(());
        }
        let humans = self.phase == TurnPhase::Humans;
        let was = self.rep.in_humans;
        // B4 K19: the game is over. The clients' replicas hold the state of the start of the last turn; the final
        // state (the winner, the victory, the replay messages the end-of-game windows read) goes out once as the
        // snapshot of one more turn change, and again to a seat that asks for a resync.
        if matches!(self.phase, TurnPhase::GameOver { .. }) {
            if !self.rep.over_snap {
                self.rep.over_snap = true;
                self.rec_take(RecPhase::Ai, fb)?;
                self.rep.in_humans = None;
                self.rep.snap_due = true;
            }
            if self.rep.snap_due || !self.rep.resyncs.is_empty() {
                self.replica_snapshot(fb)?;
            }
            self.rep.shared = None;
            return Ok(());
        }
        // The humans ended their turn: the last batch went out before the pump (`replica_before_pump`); what the
        // core did since is the AI phase, which the snapshot of the next turn carries.
        if was.is_some() && !humans {
            self.rec_take(RecPhase::Ai, fb)?;
            self.rep.in_humans = None;
            self.rep.shared = None;
            return Ok(());
        }
        if !humans {
            self.rec_take(RecPhase::Ai, fb)?;
            self.rep.shared = None;
            return Ok(());
        }
        if was != Some(self.turn) {
            // A new turn of the humans: what the core did meanwhile is in the snapshot.
            self.rec_take(RecPhase::Ai, fb)?;
            self.rep.in_humans = Some(self.turn);
            self.rep.snap_due = true;
            self.rep.resyncs.clear();
        }
        if self.rep.snap_due || !self.rep.resyncs.is_empty() {
            self.replica_snapshot(fb)?;
        } else if !self.rep.blocked() {
            self.replica_batch(false, fb)?;
        }
        self.rep.shared = None;
        Ok(())
    }

    /// Takes the journal and drops it, as the replica stream always did; with `WorkerConfig.record` a batch that holds
    /// records goes to the gateway as `RecJournal` instead of being lost.
    fn rec_take(&mut self, phase: RecPhase, fb: &mut Fb) -> Result<(), CoreError> {
        let Some(ops) = self.core.replica() else {
            return Ok(());
        };
        let batch = ops.journal_take(false)?;
        if self.cfg.record && !journal_is_empty(&batch) {
            fb.replica.push(WorkerMsg::RecJournal {
                phase,
                turn: self.turn,
                bytes: batch,
            });
        }
        Ok(())
    }

    /// Takes the journal and sends it as the next batch, if it holds anything.
    fn replica_batch(&mut self, force_check: bool, fb: &mut Fb) -> Result<(), CoreError> {
        let due = self
            .rep
            .last_check_ms
            .is_none_or(|t| self.now_ms.saturating_sub(t) >= CHECK_EVERY_MS);
        let check = force_check || due;
        let Some(ops) = self.core.replica() else {
            return Ok(());
        };
        let batch = ops.journal_take(check)?;
        if journal_is_empty(&batch) {
            return Ok(());
        }
        if check {
            self.rep.last_check_ms = Some(self.now_ms);
        }
        let first = self.rep.jseq + 1;
        self.rep.jseq = first;
        fb.replica.push(WorkerMsg::ReplicaJournal {
            epoch: self.rep.epoch,
            first_jseq: first,
            batches: vec![batch],
        });
        Ok(())
    }

    /// Makes the snapshot that is due, if the core is at a safe point.
    fn replica_snapshot(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        let snap_t0 = self.tl_now();
        if self.rep.shared.is_none()
            && let Err(why) = self.core.safe_point()
        {
            self.note_no_safe_point("a replica snapshot", &why);
            self.rep.retry_ms = self.now_ms + RETRY_MS;
            return Ok(());
        }
        self.note_safe_point();
        // The core's part first: `gc_save` may refuse where `Q_GAME.safe_point` did not (a human's unit is still
        // busy, live game 931 2026-10-30, where it ended the worker with a fatal error). Then nothing is taken
        // yet, the snapshot stays due, the core is pumped so the unit settles, and it is tried again later.
        let gcs1 = match self.rep.shared.take() {
            Some(bytes) => bytes,
            None => match self.core.save() {
                Ok(bytes) => Arc::new(bytes),
                Err(e) if e.kind == CoreErrorKind::NotSafe => {
                    tracing::info!("no save point for a replica snapshot yet: {e}");
                    self.rep.retry_ms = self.now_ms + SNAP_NOT_SAFE_RETRY_MS;
                    return self.settle(fb);
                }
                Err(e) => return Err(e),
            },
        };
        let kind = if self.rep.snap_due {
            if self.rep.announced {
                Kind::Turn
            } else {
                Kind::Base
            }
        } else {
            Kind::Resync(std::mem::take(&mut self.rep.resyncs).into_iter().collect())
        };
        if matches!(kind, Kind::Resync(_)) {
            // The batches the seat has not seen yet go first: the base is the state after them.
            self.replica_batch(false, fb)?;
        } else {
            self.rec_take(RecPhase::Cut, fb)?;
        }
        let id = self.rep.next_job;
        self.rep.next_job += 1;
        self.rep.inflight.insert(
            id,
            Inflight {
                kind: kind.clone(),
                turn: self.turn,
                jseq: self.rep.jseq,
                gcs1: Arc::clone(&gcs1),
                queued_us: self.tl_now(),
            },
        );
        // Only a later turn is a patch: a base or a resync goes whole.
        let prev = if matches!(kind, Kind::Turn) {
            self.rep.prev.clone()
        } else {
            None
        };
        self.rep.jobs.push(ReplicaJob { id, gcs1, prev });
        if !matches!(kind, Kind::Resync(_)) {
            self.rep.snap_due = false;
        }
        if matches!(kind, Kind::Turn) {
            self.tl_snapshot(snap_t0);
        }
        Ok(())
    }

    /// The pack thread packed a snapshot. The gateway hears the bodies, then the base or the turn; the batches
    /// go on.
    pub fn replica_done(&mut self, done: ReplicaDone, now_ms: u64) -> Vec<Out> {
        if self.dead {
            return Vec::new();
        }
        self.now_ms = now_ms;
        let mut fb = Fb::default();
        let Some(job) = self.rep.inflight.remove(&done.id) else {
            return Vec::new();
        };
        match done.result {
            Ok(packed) => {
                let ReplicaPacked {
                    snapshot: body,
                    patch,
                    pack_us,
                    patch_us,
                } = packed;
                tracing::info!(
                    turn = job.turn,
                    raw = job.gcs1.len(),
                    snapshot = body.bytes.len(),
                    patch = patch.as_ref().map(|(p, _)| p.bytes.len()),
                    pack_ms = pack_us / 1000,
                    patch_ms = patch_us / 1000,
                    "replica snapshot packed"
                );
                if job.kind == Kind::Turn {
                    self.tl_pack_done(
                        &PackTimes {
                            queued_us: job.queued_us,
                            pack_us,
                            patch_us,
                            snapshot_bytes: body.bytes.len(),
                            patch_bytes: patch.as_ref().map(|(p, _)| p.bytes.len()),
                        },
                        job.turn,
                    );
                }
                let snapshot = protocol::GcBlobRef {
                    hash: body.hash,
                    size: u32::try_from(body.bytes.len()).unwrap_or(u32::MAX),
                };
                let raw_hash = body.raw_hash;
                let patch_ref = patch.map(|(p, from_raw_hash)| {
                    let blob = protocol::GcBlobRef {
                        hash: p.hash,
                        size: u32::try_from(p.bytes.len()).unwrap_or(u32::MAX),
                    };
                    fb.replica.push(WorkerMsg::ReplicaBlob {
                        hash: p.hash,
                        bytes: p.bytes,
                    });
                    protocol::GcPatchRef {
                        from_raw_hash,
                        blob,
                    }
                });
                fb.replica.push(WorkerMsg::ReplicaBlob {
                    hash: body.hash,
                    bytes: body.bytes,
                });
                let epoch = self.rep.epoch;
                match job.kind {
                    Kind::Base | Kind::Turn => {
                        let msg = if job.kind == Kind::Base {
                            WorkerMsg::ReplicaBase {
                                epoch,
                                turn: job.turn,
                                jseq: job.jseq,
                                raw_hash,
                                snapshot,
                                for_player: None,
                            }
                        } else {
                            WorkerMsg::ReplicaTurn {
                                epoch,
                                turn: job.turn,
                                jseq: job.jseq,
                                raw_hash,
                                snapshot,
                                patch: patch_ref,
                            }
                        };
                        fb.replica.push(msg);
                        self.rep.announced = true;
                        self.rep.prev = Some((job.gcs1, raw_hash));
                    }
                    Kind::Resync(players) => {
                        for p in players {
                            fb.replica.push(WorkerMsg::ReplicaBase {
                                epoch,
                                turn: job.turn,
                                jseq: job.jseq,
                                raw_hash,
                                snapshot,
                                for_player: Some(p),
                            });
                        }
                    }
                }
            }
            Err(why) => {
                tracing::error!("the replica snapshot could not be packed: {why}");
                // The chain cannot go on without a base: try again at the next safe point. A seat that waits for
                // a resync asks again.
                if !matches!(job.kind, Kind::Resync(_)) {
                    self.rep.snap_due = true;
                    self.rep.retry_ms = now_ms + 1000;
                }
            }
        }
        self.finish(fb, now_ms, Ok(()))
    }
}
