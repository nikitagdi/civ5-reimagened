//! The state machine of the replica.
//!
//! ```text
//! Empty --GcBase--> WaitBlob --body in--> Loading --loaded--> Live --diverged / gap / bad body--> Resyncing
//!                                                              ^  |                                  |
//!                                                              |  +--GcTurn: stays Live, `switching`  |
//!                                                              +-------------GcBase-------------------+
//! ```
//!
//! Numbers: `jseq` counts the journal batches of a game. A snapshot (`GcBase`, `GcTurn`) holds the state after
//! every batch up to its `jseq`; the next batch is `jseq + 1`. A batch not above the snapshot's point is dropped,
//! one that skips a number is a gap (`GcResync(Gap)`), one of another `epoch` is not ours.
//!
//! The machine does no I/O. The shell feeds it messages ([`Replica::on_msg`]), fetches the bodies it asks for
//! ([`Replica::wanted_blobs`], [`Replica::blob`]), gives it time ([`Replica::pump`]) and sends what it wants sent
//! ([`Replica::take_outgoing`]).

use std::collections::BTreeMap;

use gamecore::Applied;
use gamecore::replica::{
    JournalRecord, decode_journal, encode_journal, encode_net, net_message_from_ui,
};
use platform::Clock;
use protocol::xxh64;
use wire::replica::{BodyKind, decode_snapshot, encode_snapshot, parse_header};

use crate::engine::{CoreEngine, Engine};
use crate::game_side::{GameSide, NetMessage, PathNode, TradePath, UiEvent, UiOp};
use crate::read::GameRead;
use crate::{
    GcBase, GcBlobRef, GcJournal, GcPatchRef, GcResyncReason, GcTurn, ReplicaError, ReplicaMsg,
};

/// The largest `GcNet` payload (it stays inside the client frame).
const GC_NET_MAX: usize = protocol::GC_NET_MAX;
/// A body is fetched again this many times before the machine falls back (patch to snapshot) or resyncs.
const BLOB_RETRIES: u8 = 2;

/// Where the machine is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No base yet.
    Empty,
    /// A base is named; its body has not arrived.
    WaitBlob,
    /// The body is in; it is being unpacked and loaded into the game.
    Loading,
    /// The game follows the journal.
    Live,
    /// The replica left the server's game (or a batch is missing, or a body is bad); the game is not touched
    /// and batches are kept until the answer to `GcResync` (a `GcBase`) arrives.
    Resyncing,
}

/// What one [`Replica::pump`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaStep {
    /// Nothing to do.
    Idle,
    /// Journal batches applied up to this `jseq`.
    Applied { jseq: u64 },
    /// A stage of a load or a housekeeping step is done; call again.
    Working,
    /// The base snapshot is loaded (join, or the answer to a resync): the game is a new one.
    Loaded { turn: i32 },
    /// A turn change is loaded (patch or snapshot): the game is a new one.
    TurnSwitched { turn: i32 },
    /// The machine asked the server for a new base and waits for it.
    Resyncing,
}

/// What the shell does for the replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// `ClientCore::send_gc_net`: `GCN1` bytes of the UI's messages, at most `GC_NET_MAX`.
    GcNet(Vec<u8>),
    /// `sendTurnComplete`: the end of turn is a command (`Command::EndTurn`).
    EndTurn,
    /// `sendTurnUnready`: `CancelEndTurn`.
    CancelEndTurn,
    /// `ClientCore::send_gc_resync`.
    Resync {
        epoch: u32,
        have_raw_hash: Option<u64>,
        at_jseq: u64,
        reason: GcResyncReason,
    },
}

/// Settings of a [`Replica`].
#[derive(Debug, Clone)]
pub struct ReplicaConfig {
    /// Verify every `CHECK` record (tests). Otherwise only those that cost little (see [`Replica::pump`]).
    pub check_always: bool,
    /// No input for this long counts as idle (the player is not looking at a move), ms.
    pub idle_ms: u64,
    /// Idle `CHECK`s at most this often, ms.
    pub check_every_ms: u64,
    /// The answer to a `GcResync` is awaited this long before it is asked again, ms.
    pub resync_retry_ms: u64,
    /// Use the turn-change patches (variant A, decoded by `wire::replica::apply_patch` with the server's constant
    /// dictionary header). `false`: every turn loads the full snapshot.
    pub patches: bool,
    /// Pack the base snapshot when idle (a patched base is raw until then).
    pub pack_base: bool,
}

impl Default for ReplicaConfig {
    fn default() -> Self {
        ReplicaConfig {
            check_always: false,
            idle_ms: 2_000,
            check_every_ms: 10_000,
            resync_retry_ms: 10_000,
            patches: true,
            pack_base: true,
        }
    }
}

/// Counters for `__civ5stats.replica.*`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplicaStats {
    /// Journal batches applied.
    pub applied: u64,
    /// Batches that ended in a divergence (a failed `CHECK` or a refused record).
    pub diverged: u64,
    /// `GcResync` requests made.
    pub resyncs: u64,
    /// `CHECK` records cut out (not verified) and those verified.
    pub checks_skipped: u64,
    pub checks_run: u64,
    /// UI messages sent to the server (in `GcNet` frames, and end-of-turn commands).
    pub net_sent: u64,
    /// UI messages the server refused (the shell reports them: [`Replica::note_net_rejected`]).
    pub net_rejected: u64,
    /// UI messages dropped here: not UI messages (the handshake of the turn is the server's).
    pub net_dropped: u64,
    /// Ends of turn the router took back ahead of a gameplay message (`CancelEndTurn` before an order).
    pub turn_reopened: u64,
    /// Snapshot loads (the first, turn changes, resyncs).
    pub loads: u64,
    /// Turn changes by patch and by full snapshot; patches that failed and fell back.
    pub turn_patch: u64,
    pub turn_snapshot: u64,
    pub patch_fallbacks: u64,
    /// Messages ignored: another epoch, a batch the snapshot already holds, a base already held.
    pub stale_epoch: u64,
    pub stale_batches: u64,
    pub bases_skipped: u64,
    /// Time of the last turn change in the main thread: unpacking/patching plus the load, ms.
    pub turn_switch_ms: f64,
    /// Time from the `GcTurn` to the loaded game, the download included, ms.
    pub turn_wait_ms: f64,
    /// Time from the first `GcBase` to the first loaded game, ms (the download is part of it).
    pub join_ms: f64,
    /// Size of the body (patch or snapshot) of the last turn change, bytes.
    pub last_body_bytes: u64,
    /// Linear memory of the module in MiB (wasm), 0 elsewhere.
    pub memory_mb: u32,
    /// Bytes of the base snapshot kept for the next patch and whether they are packed.
    pub base_bytes: u64,
    pub base_packed: bool,
    /// Batches waiting.
    pub queued: u32,
}

/// The base snapshot the next patch is made against.
struct Base {
    raw_hash: u64,
    data: BaseData,
}

enum BaseData {
    /// The GCS1 bytes.
    Raw(Vec<u8>),
    /// A `C5RB` snapshot body (zstd frame), a few hundred KB for tens of MB.
    Packed(Vec<u8>),
}

/// A timed piece of the replica's work for the shell's timeline (`c.fetch`, `c.unpack`, `c.load`, `c.check`, and the mark
/// `c.turn_named`: the `GcTurn` arrived). The times are on the replica's clock ([`Replica::set_clock`]). The replica
/// stays free of the recorder: the shell takes these with [`Replica::take_spans`].
#[derive(Debug, Clone, PartialEq)]
pub struct ReplicaSpan {
    pub name: &'static str,
    /// Start, ms of the replica's clock.
    pub t0_ms: f64,
    /// Length, ms; negative for a mark (an instant).
    pub dur_ms: f64,
    /// The turn of the snapshot the work was for (a turn change: the new turn), or of the game for a check.
    pub turn: i32,
    /// The work was for a turn change (not the first load or a resync).
    pub turn_change: bool,
    pub args: Vec<(&'static str, f64)>,
}

/// Spans kept until the shell takes them (a shell that never does leaves the oldest dropped).
const SPANS_MAX: usize = 256;

/// A snapshot to move to: a base, or a turn change.
struct Target {
    turn_change: bool,
    epoch: u32,
    turn: i32,
    jseq: u64,
    raw_hash: u64,
    snapshot: GcBlobRef,
    patch: Option<GcPatchRef>,
    /// The blob being fetched (the patch while it is usable, else the snapshot).
    asked: GcBlobRef,
    asked_is_patch: bool,
    /// Handed out by `wanted_blobs`.
    announced: bool,
    retries: u8,
    /// The body, once it arrived and its hash checked.
    body: Option<Vec<u8>>,
    /// The GCS1 bytes, once unpacked; `body_is_snapshot` says whether the body can serve as the packed base.
    raw: Option<Vec<u8>>,
    packed_base: Option<Vec<u8>>,
    /// Time the target was named, and the work spent on it so far (ms).
    named_at: f64,
    work_ms: f64,
    /// When the body was handed out for the fetch (ms), for the `c.fetch` span.
    fetch_at: f64,
    /// The size of the body that arrived, bytes.
    body_bytes: u64,
}

/// The replica.
pub struct Replica {
    engine: Box<dyn Engine>,
    cfg: ReplicaConfig,
    clock: Option<Box<dyn Clock>>,
    state: State,
    /// The chain: epoch, last batch applied (or contained in the loaded snapshot), game turn.
    epoch: Option<u32>,
    applied_jseq: u64,
    turn: i32,
    /// Something was loaded into the engine.
    loaded: bool,
    generation: u64,
    base: Option<Base>,
    target: Option<Target>,
    /// Batches not applied yet, by `jseq`.
    queue: BTreeMap<u64, Vec<u8>>,
    /// The epoch the queued batches belong to when there is no chain yet.
    queue_epoch: Option<u32>,
    outgoing: Vec<Outgoing>,
    /// A `GcResync` is out and its answer is not in; when it was sent.
    resync_sent_at: Option<f64>,
    resync_reason: GcResyncReason,
    last_input: f64,
    last_check: f64,
    first_base_at: Option<f64>,
    stats: ReplicaStats,
    wanted: Vec<GcBlobRef>,
    /// The UI's end of turn is out (`sendTurnComplete` routed, not taken back since): the exe's
    /// `CvGameController::m_sentTurnComplete`.
    turn_sent: bool,
    /// The router took the end of turn back on its own, ahead of a gameplay message ([`GameSide::take_turn_reopened`]).
    turn_reopened: bool,
    /// Every human has ended the turn ([`GameSide::set_turn_locked`]): nothing takes the end of turn back.
    turn_locked: bool,
    /// Players whose end of turn the journal applied with no pump after it yet ([`GameSide::turn_ended_by_journal`]).
    journal_ended: u64,
    /// UI messages of the seat sent in `GcNet` frames whose `NET` records the journal has not brought back yet, and
    /// when the last one went out ([`GameSide::own_orders_pending`]).
    own_pending: u64,
    own_sent_at: f64,
    spans: Vec<ReplicaSpan>,
}

/// A UI message of the seat that is not back in the journal after this long is taken as lost (a refusal the shell
/// did not report, a resync): the auto unit cycle does not wait for ever.
pub const OWN_PENDING_MAX_MS: f64 = 3_000.0;

impl Replica {
    /// Boots the core as the replica of `seat` (takes seconds on a big game: databases and Lua contexts).
    /// `host` serves the game data (see [`crate::GameDataHost`]).
    pub fn new(
        host: impl gamecore::HostServices + 'static,
        seat: u8,
    ) -> Result<Replica, ReplicaError> {
        let engine = CoreEngine::new(host, seat).map_err(ReplicaError::Core)?;
        Ok(Replica::with_engine(
            Box::new(engine),
            ReplicaConfig::default(),
        ))
    }

    /// A replica over any engine (the tests of the state machine use a fake).
    pub fn with_engine(engine: Box<dyn Engine>, cfg: ReplicaConfig) -> Replica {
        Replica {
            engine,
            cfg,
            clock: None,
            state: State::Empty,
            epoch: None,
            applied_jseq: 0,
            turn: 0,
            loaded: false,
            generation: 0,
            base: None,
            target: None,
            queue: BTreeMap::new(),
            queue_epoch: None,
            outgoing: Vec::new(),
            resync_sent_at: None,
            resync_reason: GcResyncReason::Gap,
            last_input: f64::NEG_INFINITY,
            last_check: f64::NEG_INFINITY,
            first_base_at: None,
            stats: ReplicaStats::default(),
            wanted: Vec::new(),
            turn_sent: false,
            turn_reopened: false,
            turn_locked: false,
            journal_ended: 0,
            own_pending: 0,
            own_sent_at: f64::NEG_INFINITY,
            spans: Vec::new(),
        }
    }

    /// The timed work since the last call (see [`ReplicaSpan`]).
    pub fn take_spans(&mut self) -> Vec<ReplicaSpan> {
        std::mem::take(&mut self.spans)
    }

    fn span(&mut self, s: ReplicaSpan) {
        if self.spans.len() >= SPANS_MAX {
            self.spans.remove(0);
        }
        self.spans.push(s);
    }

    /// The clock for timings, idle detection and the resync timeout. Without one all times are 0.
    pub fn set_clock(&mut self, clock: Box<dyn Clock>) {
        self.clock = Some(clock);
    }

    pub fn config_mut(&mut self) -> &mut ReplicaConfig {
        &mut self.cfg
    }

    fn now(&self) -> f64 {
        self.clock.as_ref().map_or(0.0, |c| c.now_ms_f64())
    }

    /// The player did something (a click, a key): idle `CHECK`s wait and the base is not packed meanwhile.
    pub fn note_input(&mut self) {
        self.last_input = self.now();
    }

    /// The server refused `n` UI messages (a line of the journal told so).
    pub fn note_net_rejected(&mut self, n: u64) {
        self.stats.net_rejected += n;
        self.own_pending = self.own_pending.saturating_sub(n);
    }

    /// Whether UI messages of the seat are out and not back in the journal (see [`OWN_PENDING_MAX_MS`]).
    pub fn own_pending(&self) -> bool {
        self.own_pending > 0 && self.now() - self.own_sent_at < OWN_PENDING_MAX_MS
    }

    // ------------------------------------------------------------ reading

    pub fn state(&self) -> State {
        self.state
    }

    /// A turn change or a base is being fetched or loaded while the game is still the old one.
    pub fn switching(&self) -> bool {
        self.target.is_some()
    }

    pub fn epoch(&self) -> Option<u32> {
        self.epoch
    }

    /// The last batch applied (or contained in the loaded snapshot).
    pub fn applied_jseq(&self) -> u64 {
        self.applied_jseq
    }

    /// The game turn of the last snapshot or pump record.
    pub fn turn(&self) -> i32 {
        self.turn
    }

    /// +1 on every snapshot load or resync: what the UI holds of the game is then stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn stats(&self) -> ReplicaStats {
        let mut s = self.stats.clone();
        s.memory_mb = self.engine.memory_mb();
        s.queued = self.queue.len() as u32;
        if let Some(b) = &self.base {
            s.base_bytes = match &b.data {
                BaseData::Raw(v) | BaseData::Packed(v) => v.len() as u64,
            };
            s.base_packed = matches!(b.data, BaseData::Packed(_));
        }
        s
    }

    /// The replica digest of the loaded game (the value of a server's `CHECK` at this point). Slow: for tests,
    /// the self-test and the debug overlay.
    pub fn digest(&mut self) -> Result<u64, String> {
        if !self.loaded {
            return Err("nothing is loaded".into());
        }
        self.engine.digest()
    }

    /// The game, read only, for the UI v2 source: `Some` only while the replica follows the journal
    /// ([`State::Live`]) and its engine has a core to read.
    pub fn read(&mut self) -> Option<&mut dyn GameRead> {
        if self.state != State::Live {
            return None;
        }
        self.engine.read()
    }

    /// The engine, for tests and the registration of the Lua API.
    pub fn engine_mut(&mut self) -> &mut dyn Engine {
        self.engine.as_mut()
    }

    // ------------------------------------------------------------ the stream

    /// A message of the replica stream, in the order the gateway sent it.
    pub fn on_msg(&mut self, msg: ReplicaMsg) {
        match msg {
            ReplicaMsg::Base(b) => self.on_base(b),
            ReplicaMsg::Journal(j) => self.on_journal(j),
            ReplicaMsg::Turn(t) => self.on_turn(t),
        }
    }

    /// The epoch of the chain being followed: the pending snapshot's, else the game's, else the queue's.
    fn chain_epoch(&self) -> Option<u32> {
        self.target
            .as_ref()
            .map(|t| t.epoch)
            .or(self.epoch)
            .or(self.queue_epoch)
    }

    fn on_base(&mut self, b: GcBase) {
        let now = self.now();
        self.first_base_at.get_or_insert(now);
        // The same base again (a fresh attach of the same turn): nothing to load, the journal that follows is
        // deduplicated by `jseq`.
        if self.state == State::Live
            && self.target.is_none()
            && self.epoch == Some(b.epoch)
            && self.base.as_ref().is_some_and(|x| x.raw_hash == b.raw_hash)
        {
            self.stats.bases_skipped += 1;
            return;
        }
        if self.chain_epoch() != Some(b.epoch) {
            // Another chain: what was queued for the old one is not ours.
            self.queue.clear();
        }
        self.queue_epoch = Some(b.epoch);
        self.name_target(Target {
            turn_change: false,
            epoch: b.epoch,
            turn: b.turn,
            jseq: b.jseq,
            raw_hash: b.raw_hash,
            snapshot: b.snapshot,
            patch: None,
            asked: b.snapshot,
            asked_is_patch: false,
            announced: false,
            retries: 0,
            body: None,
            raw: None,
            packed_base: None,
            named_at: now,
            work_ms: 0.0,
            fetch_at: now,
            body_bytes: 0,
        });
        // A new base answers a request for one. The game stays what it was until the body is in.
        self.resync_sent_at = None;
        self.state = State::WaitBlob;
        self.drop_below(b.jseq);
    }

    fn on_turn(&mut self, t: GcTurn) {
        let now = self.now();
        if self.chain_epoch().is_some_and(|e| e != t.epoch) {
            self.stats.stale_epoch += 1;
            return;
        }
        if self.state == State::Resyncing {
            // The answer to the request is a `GcBase`; a turn change before it is the old chain's.
            self.stats.stale_batches += 1;
            return;
        }
        if self.loaded && self.target.is_none() {
            if t.jseq < self.applied_jseq {
                // Older than the game we have.
                self.stats.stale_batches += 1;
                return;
            }
            if self.base.as_ref().is_some_and(|b| b.raw_hash == t.raw_hash) {
                // The turn we already hold (a fresh attach).
                self.stats.bases_skipped += 1;
                return;
            }
        }
        if self.epoch.is_none() {
            self.queue_epoch = Some(t.epoch);
        }
        // The patch is usable only against the base we hold, and only when patches are enabled.
        let use_patch = match (&t.patch, &self.base, self.cfg.patches) {
            (Some(p), Some(b), true) => p.from_raw_hash == b.raw_hash,
            _ => false,
        };
        let asked = if use_patch {
            t.patch.map_or(t.snapshot, |p| p.blob)
        } else {
            t.snapshot
        };
        self.name_target(Target {
            turn_change: true,
            epoch: t.epoch,
            turn: t.turn,
            jseq: t.jseq,
            raw_hash: t.raw_hash,
            snapshot: t.snapshot,
            patch: if use_patch { t.patch } else { None },
            asked,
            asked_is_patch: use_patch,
            announced: false,
            retries: 0,
            body: None,
            raw: None,
            packed_base: None,
            named_at: now,
            work_ms: 0.0,
            fetch_at: now,
            body_bytes: 0,
        });
        self.span(ReplicaSpan {
            name: "c.turn_named",
            t0_ms: now,
            dur_ms: -1.0,
            turn: t.turn,
            turn_change: true,
            args: vec![
                ("patch", f64::from(u8::from(use_patch))),
                ("snapshot_bytes", f64::from(t.snapshot.size)),
                (
                    "patch_bytes",
                    t.patch.map_or(0.0, |p| f64::from(p.blob.size)),
                ),
            ],
        });
        tracing::debug!(
            turn = t.turn,
            jseq = t.jseq,
            patch = use_patch,
            "gc.turn: a turn change is named"
        );
        if !self.loaded {
            self.state = State::WaitBlob;
        }
        self.drop_below(t.jseq);
    }

    fn name_target(&mut self, t: Target) {
        // A pending target is replaced by a newer one; its body is not needed any more.
        if let Some(old) = self.target.take() {
            self.wanted.retain(|w| w.hash != old.asked.hash);
        }
        self.target = Some(t);
    }

    /// Batches at or below `jseq` are in the snapshot.
    fn drop_below(&mut self, jseq: u64) {
        let before = self.queue.len();
        self.queue = self.queue.split_off(&(jseq + 1));
        self.stats.stale_batches += (before - self.queue.len()) as u64;
        self.check_queue_gap(jseq);
    }

    /// With the chain at `floor`, the lowest queued batch must be `floor + 1`.
    fn check_queue_gap(&mut self, floor: u64) {
        if let Some((&first, _)) = self.queue.iter().next()
            && first > floor + 1
        {
            tracing::warn!(first, floor, "replica: a journal batch is missing");
            self.request_resync(GcResyncReason::Gap);
        }
    }

    fn on_journal(&mut self, j: GcJournal) {
        match self.chain_epoch() {
            Some(e) if e != j.epoch => {
                self.stats.stale_epoch += 1;
                return;
            }
            Some(_) => {}
            None => self.queue_epoch = Some(j.epoch),
        }
        let floor = self.floor();
        let known = self
            .queue
            .keys()
            .next_back()
            .copied()
            .unwrap_or(0)
            .max(floor);
        let hole = j.first_jseq > known + 1 && (self.loaded || self.target.is_some());
        for (n, b) in (j.first_jseq..).zip(j.batches) {
            if n <= known {
                // Already queued, applied, or in the snapshot.
                self.stats.stale_batches += 1;
            } else {
                self.queue.insert(n, b);
            }
        }
        if hole {
            tracing::warn!(first = j.first_jseq, known, "replica: a gap in the journal");
            // What came is kept: the base that answers the request may cover the hole.
            self.request_resync(GcResyncReason::Gap);
        }
    }

    /// The point the next batch builds on: the pending snapshot's, else the game's.
    fn floor(&self) -> u64 {
        self.target.as_ref().map_or(self.applied_jseq, |t| t.jseq)
    }

    // ------------------------------------------------------------ bodies

    /// Bodies to fetch with `GET /replica/{hash:016x}`; each is handed out once.
    pub fn wanted_blobs(&mut self) -> Vec<GcBlobRef> {
        if let Some(t) = &mut self.target
            && !t.announced
            && t.body.is_none()
        {
            t.announced = true;
            t.fetch_at = self.clock.as_ref().map_or(0.0, |c| c.now_ms_f64());
            self.wanted.push(t.asked);
        }
        std::mem::take(&mut self.wanted)
    }

    /// A body arrived (`hash` is what the URL named). Checked against the hash and size the stream gave.
    pub fn blob(&mut self, hash: u64, bytes: Vec<u8>) {
        let Some(t) = &mut self.target else {
            return;
        };
        if t.asked.hash != hash || t.body.is_some() {
            return;
        }
        if bytes.len() != t.asked.size as usize || xxh64(0, &bytes) != hash {
            tracing::warn!(
                hash,
                len = bytes.len(),
                "replica: a body does not match its reference"
            );
            self.blob_bad();
            return;
        }
        let (t0, patch, turn, turn_change, retries) = (
            t.fetch_at,
            t.asked_is_patch,
            t.turn,
            t.turn_change,
            t.retries,
        );
        t.body_bytes = bytes.len() as u64;
        t.body = Some(bytes);
        let (now, bytes) = (self.now(), self.target.as_ref().map_or(0, |t| t.body_bytes));
        self.span(ReplicaSpan {
            name: "c.fetch",
            t0_ms: t0,
            dur_ms: (now - t0).max(0.0),
            turn,
            turn_change,
            args: vec![
                ("bytes", bytes as f64),
                ("patch", f64::from(u8::from(patch))),
                ("retries", f64::from(retries)),
            ],
        });
    }

    /// A fetch failed (network, 404). Asked again a couple of times, then the patch gives way to the snapshot,
    /// and the snapshot to a resync.
    pub fn blob_failed(&mut self, hash: u64) {
        if self
            .target
            .as_ref()
            .is_none_or(|t| t.asked.hash != hash || t.body.is_some())
        {
            return;
        }
        let retry = self.target.as_mut().is_some_and(|t| {
            t.retries += 1;
            t.retries <= BLOB_RETRIES
        });
        if retry {
            if let Some(t) = &mut self.target {
                t.announced = false;
            }
        } else {
            self.blob_bad();
        }
    }

    /// The body asked for is unusable.
    fn blob_bad(&mut self) {
        let Some(t) = &mut self.target else {
            return;
        };
        if t.asked_is_patch {
            // Fall back to the full snapshot of the same turn.
            self.stats.patch_fallbacks += 1;
            t.asked = t.snapshot;
            t.asked_is_patch = false;
            t.patch = None;
            t.announced = false;
            t.retries = 0;
            t.body = None;
            return;
        }
        self.target = None;
        self.request_resync(GcResyncReason::BadBlob);
    }

    // ------------------------------------------------------------ work

    /// Does some work: a stage of loading a snapshot, or journal batches for about `budget_ms` (at least one
    /// when one is ready). Call it once per frame. The whole load of a snapshot is done in stages (unpack, load):
    /// each is one call and is not interrupted.
    pub fn pump(&mut self, budget_ms: u32) -> ReplicaStep {
        if self.loaded {
            self.drain_engine();
        }
        if self.state == State::Resyncing {
            // The server did not answer: ask again. A request that could not be sent (`unsend`) is made again.
            if let Some(at) = self.resync_sent_at
                && self.now() - at >= self.cfg.resync_retry_ms as f64
            {
                self.resync_sent_at = None;
            }
            if self.resync_sent_at.is_none() {
                let reason = self.resync_reason;
                self.request_resync(reason);
            }
        }
        if self
            .target
            .as_ref()
            .is_some_and(|t| t.body.is_some() || t.raw.is_some())
        {
            return self.load_stage();
        }
        match self.state {
            State::Resyncing => ReplicaStep::Resyncing,
            State::Live if self.target.is_none() => {
                let step = self.apply_ready(budget_ms);
                if step != ReplicaStep::Idle {
                    step
                } else if self.state == State::Resyncing {
                    ReplicaStep::Resyncing
                } else if self.idle_housekeeping() {
                    ReplicaStep::Working
                } else {
                    ReplicaStep::Idle
                }
            }
            _ => ReplicaStep::Idle,
        }
    }

    fn apply_ready(&mut self, budget_ms: u32) -> ReplicaStep {
        let start = self.now();
        let mut last = None;
        while let Some(batch) = self.queue.remove(&(self.applied_jseq + 1)) {
            let checks_before = self.stats.checks_run;
            let Some((ready, own)) = self.prepare(&batch) else {
                self.request_resync(GcResyncReason::BadBlob);
                return ReplicaStep::Resyncing;
            };
            let applied_at = self.now();
            let result = self.engine.apply(&ready);
            if self.stats.checks_run != checks_before {
                let ms = self.now() - applied_at;
                let ok = matches!(result, Ok(Applied::InStep));
                self.span(ReplicaSpan {
                    name: "c.check",
                    t0_ms: applied_at,
                    dur_ms: ms,
                    turn: self.turn,
                    turn_change: false,
                    args: vec![
                        ("jseq", (self.applied_jseq + 1) as f64),
                        ("ok", f64::from(u8::from(ok))),
                    ],
                });
                tracing::debug!(jseq = self.applied_jseq + 1, ms, ok, "gc.check");
            }
            match result {
                Ok(Applied::InStep) => {
                    self.applied_jseq += 1;
                    self.stats.applied += 1;
                    self.own_pending = self.own_pending.saturating_sub(own);
                    last = Some(self.applied_jseq);
                }
                Ok(Applied::Diverged(why)) => {
                    tracing::warn!(
                        jseq = self.applied_jseq + 1,
                        why,
                        "gc.check: replica diverged"
                    );
                    self.stats.diverged += 1;
                    self.request_resync(GcResyncReason::Diverged);
                    return ReplicaStep::Resyncing;
                }
                Err(e) => {
                    tracing::warn!(
                        jseq = self.applied_jseq + 1,
                        "replica: a batch was refused: {e}"
                    );
                    self.stats.diverged += 1;
                    self.request_resync(GcResyncReason::Rejected);
                    return ReplicaStep::Resyncing;
                }
            }
            if self.now() - start >= f64::from(budget_ms) {
                break;
            }
        }
        // The game may have produced UI messages while it ran (a Lua event handler of the core).
        self.drain_engine();
        if last.is_none() {
            // Batches above a hole (a lost one): nothing will fill it.
            let floor = self.applied_jseq;
            self.check_queue_gap(floor);
            if self.state == State::Resyncing {
                return ReplicaStep::Resyncing;
            }
        }
        last.map_or(ReplicaStep::Idle, |jseq| ReplicaStep::Applied { jseq })
    }

    /// The batch as the engine gets it: `CHECK` records are cut out unless they are worth their cost.
    ///
    /// A digest costs the client 0.3 s on a big game, so a `CHECK` is verified when (a) the
    /// config says always (tests), (b) the batch moves the game to a later turn (the end of the human phase,
    /// where the server always puts one), or (c) nothing is queued behind it, the player has been idle for
    /// `idle_ms` and the last verified `CHECK` is `check_every_ms` old. `None`: the batch does not decode. With the
    /// bytes comes the number of the seat's own `NET` records (the echo of its UI messages).
    fn prepare(&mut self, batch: &[u8]) -> Option<(Vec<u8>, u64)> {
        let records = match decode_journal(batch) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("replica: a journal batch does not decode: {e}");
                return None;
            }
        };
        let seat = self.engine.seat();
        let own = records
            .iter()
            .filter(|r| matches!(r, JournalRecord::Net { player, .. } if Some(*player) == seat))
            .count() as u64;
        self.note_turn_ends(&records);
        self.prepare_records(batch, records).map(|b| (b, own))
    }

    /// The core marks a human's turn complete (`HasReceivedNetTurnComplete`) when it delivers the end of turn's net
    /// message, in the pump after the `END_TURN` command. The pump after the last human's end of turn is the AI phase,
    /// which the journal does not carry, so the replicas never saw the last human done: the others' "Waiting for
    /// players" listed him until the new turn was loaded (live report 2026-10-31). The ends of turn the journal applied
    /// with no pump after them yet are kept here; a pump hands the flag back to the core.
    fn note_turn_ends(&mut self, records: &[JournalRecord]) {
        let kind = |c: gamecore::Command| c.encode().0;
        let (end, force, timeout, cancel) = (
            kind(gamecore::Command::EndTurn),
            kind(gamecore::Command::ForceEndTurn),
            kind(gamecore::Command::EndTurnTimeout),
            kind(gamecore::Command::CancelEndTurn),
        );
        for r in records {
            match r {
                JournalRecord::Command { player, kind, .. } if *player < 64 => {
                    let bit = 1u64 << *player;
                    if *kind == end || *kind == force || *kind == timeout {
                        self.journal_ended |= bit;
                    } else if *kind == cancel {
                        self.journal_ended &= !bit;
                    }
                }
                JournalRecord::Pump { .. } => self.journal_ended = 0,
                _ => {}
            }
        }
    }

    fn prepare_records(&mut self, batch: &[u8], records: Vec<JournalRecord>) -> Option<Vec<u8>> {
        let checks = records
            .iter()
            .filter(|r| matches!(r, JournalRecord::Check { .. }))
            .count();
        let turn_after = records.iter().filter_map(JournalRecord::turn_after).max();
        let ends_phase = turn_after.is_some_and(|t| t > self.turn);
        if let Some(t) = turn_after {
            self.turn = self.turn.max(t);
        }
        if checks == 0 {
            return Some(batch.to_vec());
        }
        let now = self.now();
        let idle = self.queue.is_empty()
            && now - self.last_input >= self.cfg.idle_ms as f64
            && now - self.last_check >= self.cfg.check_every_ms as f64;
        if self.cfg.check_always || ends_phase || idle {
            self.stats.checks_run += checks as u64;
            self.last_check = now;
            return Some(batch.to_vec());
        }
        self.stats.checks_skipped += checks as u64;
        let kept: Vec<JournalRecord> = records
            .into_iter()
            .filter(|r| !matches!(r, JournalRecord::Check { .. }))
            .collect();
        Some(encode_journal(&kept))
    }

    fn drain_engine(&mut self) {
        match self.engine.take_ui_messages() {
            Ok(msgs) if !msgs.is_empty() => self.route(msgs),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("replica: the UI messages of the core could not be taken: {e}")
            }
        }
    }

    // ------------------------------------------------------------ loading a snapshot

    fn load_stage(&mut self) -> ReplicaStep {
        let start = self.now();
        let Some(mut t) = self.target.take() else {
            return ReplicaStep::Idle;
        };
        // Stage 1: unpack the body to the GCS1 bytes.
        if t.raw.is_none() {
            match self.unpack(&mut t) {
                Ok(()) => {
                    let end = self.now();
                    t.work_ms += end - start;
                    let raw = t.raw.as_ref().map_or(0, Vec::len);
                    self.span(ReplicaSpan {
                        name: "c.unpack",
                        t0_ms: start,
                        dur_ms: end - start,
                        turn: t.turn,
                        turn_change: t.turn_change,
                        args: vec![
                            ("raw", raw as f64),
                            ("patch", f64::from(u8::from(t.asked_is_patch))),
                        ],
                    });
                    self.state = State::Loading;
                    self.target = Some(t);
                    return ReplicaStep::Working;
                }
                Err(why) => {
                    tracing::warn!("replica: a body could not be unpacked: {why}");
                    t.body = None;
                    self.target = Some(t);
                    self.blob_bad();
                    return if self.state == State::Resyncing {
                        ReplicaStep::Resyncing
                    } else {
                        ReplicaStep::Working
                    };
                }
            }
        }
        // Stage 2: load into the game.
        let raw = t.raw.take().unwrap_or_default();
        let loaded = self.engine.load(&raw);
        let end = self.now();
        t.work_ms += end - start;
        self.span(ReplicaSpan {
            name: "c.load",
            t0_ms: start,
            dur_ms: end - start,
            turn: t.turn,
            turn_change: t.turn_change,
            args: vec![
                ("raw", raw.len() as f64),
                ("ok", f64::from(u8::from(loaded.is_ok()))),
            ],
        });
        match loaded {
            Err(e) => {
                tracing::error!("replica: the snapshot did not load: {e}");
                self.loaded = false;
                self.target = None;
                self.base = None;
                self.request_resync(GcResyncReason::BadBlob);
                ReplicaStep::Resyncing
            }
            Ok(()) => {
                let turn_change = t.turn_change;
                self.finish_load(t, raw);
                let turn = self.turn;
                if turn_change {
                    ReplicaStep::TurnSwitched { turn }
                } else {
                    ReplicaStep::Loaded { turn }
                }
            }
        }
    }

    /// Body to GCS1 bytes: a snapshot body unpacks; a patch is applied to the held base.
    fn unpack(&mut self, t: &mut Target) -> Result<(), String> {
        let body = t.body.as_ref().ok_or("no body")?;
        let head = parse_header(body).map_err(|e| e.to_string())?;
        if head.raw_hash != t.raw_hash {
            return Err(format!(
                "the body holds the snapshot {:016x}, the stream named {:016x}",
                head.raw_hash, t.raw_hash
            ));
        }
        match head.kind {
            BodyKind::Snapshot => {
                if t.asked_is_patch {
                    return Err("a snapshot where a patch was named".into());
                }
                t.raw = Some(decode_snapshot(body).map_err(|e| e.to_string())?);
                t.packed_base = t.body.take();
            }
            BodyKind::Patch => {
                if !t.asked_is_patch {
                    return Err("a patch where a snapshot was named".into());
                }
                let from = head.from_raw_hash.ok_or("a patch without its base")?;
                let base = self.base.take().ok_or("no base for the patch")?;
                if base.raw_hash != from {
                    self.base = Some(base);
                    return Err("the patch is against another snapshot".into());
                }
                let base_raw = match base.data {
                    BaseData::Raw(v) => v,
                    BaseData::Packed(body) => decode_snapshot(&body).map_err(|e| e.to_string())?,
                };
                let body = t.body.take().ok_or("no body")?;
                // The base is consumed: a failure costs the base, and the fallback loads the full snapshot, which
                // is the new base. `wire` trusts the hash it is given, so the base bytes are hashed here.
                let base_hash = protocol::xxh64(0, &base_raw);
                t.raw = Some(
                    wire::replica::apply_patch(&body, &base_raw, base_hash)
                        .map_err(|e| e.to_string())?,
                );
                t.packed_base = None;
            }
        }
        Ok(())
    }

    fn finish_load(&mut self, t: Target, raw: Vec<u8>) {
        let now = self.now();
        let by_patch = t.asked_is_patch;
        self.base = Some(Base {
            raw_hash: t.raw_hash,
            data: match t.packed_base {
                Some(body) => BaseData::Packed(body),
                None => BaseData::Raw(raw),
            },
        });
        self.epoch = Some(t.epoch);
        self.queue_epoch = Some(t.epoch);
        self.applied_jseq = t.jseq;
        self.turn = t.turn;
        self.loaded = true;
        self.generation += 1;
        // A new game state: the exe-api tells the router what the loaded game says of the seat's end of turn.
        self.turn_sent = false;
        self.turn_reopened = false;
        self.turn_locked = false;
        self.journal_ended = 0;
        // What was out is in the snapshot, or was never taken: nothing comes back for it in the journal.
        self.own_pending = 0;
        self.state = State::Live;
        self.resync_sent_at = None;
        self.stats.loads += 1;
        self.stats.last_body_bytes = t.body_bytes;
        tracing::info!(
            turn = t.turn,
            patch = by_patch,
            bytes = t.body_bytes,
            work_ms = t.work_ms,
            wait_ms = now - t.named_at,
            "gc.blob: a snapshot is loaded"
        );
        if t.turn_change {
            self.stats.turn_switch_ms = t.work_ms;
            self.stats.turn_wait_ms = now - t.named_at;
            if by_patch {
                self.stats.turn_patch += 1;
            } else {
                self.stats.turn_snapshot += 1;
            }
        }
        if self.stats.loads == 1
            && let Some(first) = self.first_base_at
        {
            self.stats.join_ms = now - first;
        }
        // Batches the snapshot holds are out; those above it wait for `apply_ready`.
        self.drop_below(t.jseq);
    }

    /// Work for a quiet moment: pack a raw base. True if something was done.
    fn idle_housekeeping(&mut self) -> bool {
        if !self.cfg.pack_base || self.now() - self.last_input < self.cfg.idle_ms as f64 {
            return false;
        }
        let Some(base) = &mut self.base else {
            return false;
        };
        let BaseData::Raw(raw) = &base.data else {
            return false;
        };
        match encode_snapshot(raw) {
            Ok(body) => {
                base.data = BaseData::Packed(body);
                true
            }
            Err(e) => {
                tracing::warn!("replica: the base could not be packed: {e}");
                // Keep it raw; do not try again every frame.
                self.cfg.pack_base = false;
                false
            }
        }
    }

    // ------------------------------------------------------------ resync

    /// The replica cannot go on. The game is left as it is, new batches are kept, and the server is asked for a
    /// base (once; again after `resync_retry_ms`).
    fn request_resync(&mut self, reason: GcResyncReason) {
        if self.resync_sent_at.is_some() {
            return;
        }
        let Some(epoch) = self.epoch.or(self.queue_epoch) else {
            // No chain at all: the join brings the base.
            return;
        };
        tracing::warn!(
            ?reason,
            epoch,
            at_jseq = self.applied_jseq,
            "gc.resync: the replica asks for a new base"
        );
        self.state = State::Resyncing;
        self.resync_reason = reason;
        self.resync_sent_at = Some(self.now());
        self.stats.resyncs += 1;
        self.outgoing.push(Outgoing::Resync {
            epoch,
            have_raw_hash: self.base.as_ref().map(|b| b.raw_hash),
            at_jseq: self.applied_jseq,
            reason,
        });
    }

    // ------------------------------------------------------------ outgoing

    /// What the shell has to send. Messages are kept while there is no connection (give back what could not be
    /// sent with [`unsend`](Self::unsend)), so the player's input survives a resync.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        if self.loaded {
            self.drain_engine();
        }
        std::mem::take(&mut self.outgoing)
    }

    /// Puts back, at the front, what the shell could not send. A `GcResync` among it is made again on the next pump.
    pub fn unsend(&mut self, mut back: Vec<Outgoing>) {
        let before = back.len();
        back.retain(|o| !matches!(o, Outgoing::Resync { .. }));
        if back.len() != before {
            self.resync_sent_at = None;
            self.stats.resyncs = self.stats.resyncs.saturating_sub(1);
        }
        self.outgoing.splice(0..0, back);
    }

    /// The router of the UI's messages (`gDLL->send*`): `sendTurnComplete` is the command `EndTurn`,
    /// `sendTurnUnready` is `CancelEndTurn`, the other UI messages go in `GcNet` frames, the rest (the handshake of
    /// the turn, which is the server's) is dropped.
    ///
    /// A gameplay message while the end of turn is out takes it back first, as the exe's
    /// `CvGameController::BroadcastGameplayMessage` (0x08758d76) does: `m_sentTurnComplete && !m_sentTurnUnready` ->
    /// `SendTurnUnready` (NetTurnUnready), then the message. A unit moved after END TURN reopens the turn.
    fn route(&mut self, msgs: Vec<NetMessage>) {
        let mut batch: Vec<NetMessage> = Vec::new();
        for m in msgs {
            match m.name() {
                "sendTurnComplete" => {
                    self.flush_net(&mut batch);
                    self.outgoing.push(Outgoing::EndTurn);
                    self.stats.net_sent += 1;
                    self.turn_sent = true;
                }
                "sendTurnUnready" if self.turn_locked => {
                    // `CvGameController::SendTurnUnready` refuses after NetTurnAllComplete.
                    self.stats.net_dropped += 1;
                }
                "sendTurnUnready" => {
                    self.flush_net(&mut batch);
                    self.outgoing.push(Outgoing::CancelEndTurn);
                    self.stats.net_sent += 1;
                    self.turn_sent = false;
                }
                _ if net_message_from_ui(m.id) => {
                    // Once every human has ended the turn the exe's `SendTurnUnready` fails and the message goes alone.
                    if self.turn_sent && !self.turn_locked {
                        self.flush_net(&mut batch);
                        self.outgoing.push(Outgoing::CancelEndTurn);
                        self.stats.net_sent += 1;
                        self.stats.turn_reopened += 1;
                        self.turn_sent = false;
                        self.turn_reopened = true;
                    }
                    batch.push(m)
                }
                name => {
                    tracing::debug!(name, "replica: a message that is not the UI's is dropped");
                    self.stats.net_dropped += 1;
                }
            }
        }
        self.flush_net(&mut batch);
    }

    /// `batch` into `GcNet` frames under the size limit (a frame is all or nothing on the server).
    fn flush_net(&mut self, batch: &mut Vec<NetMessage>) {
        let mut frame: Vec<NetMessage> = Vec::new();
        for m in std::mem::take(batch) {
            frame.push(m);
            if encode_net(&frame).len() > GC_NET_MAX && frame.len() > 1 {
                let last = frame.pop();
                self.push_frame(std::mem::take(&mut frame));
                frame.extend(last);
            }
        }
        if !frame.is_empty() {
            self.push_frame(frame);
        }
    }

    fn push_frame(&mut self, frame: Vec<NetMessage>) {
        self.stats.net_sent += frame.len() as u64;
        self.own_pending += frame.len() as u64;
        self.own_sent_at = self.now();
        self.outgoing.push(Outgoing::GcNet(encode_net(&frame)));
    }
}

impl GameSide for Replica {
    fn attach_lua(&mut self, lua: &lua_sys::Lua) -> Result<(), String> {
        self.engine.attach_lua(lua)
    }

    fn invalidate_lua(&mut self, lua: &lua_sys::Lua) {
        self.engine.invalidate_lua(lua);
    }

    fn take_events(&mut self, out: &mut Vec<UiEvent>) {
        self.engine.take_events(out);
    }

    fn ui_op(&mut self, op: UiOp, args: &[i32], out: &mut Vec<i32>) -> Result<(), String> {
        self.engine.ui_op(op, args, out)
    }

    fn subscribe_game_event(&mut self, name: &str) {
        self.engine.subscribe_game_event(name);
    }

    fn post_net(&mut self, msg: NetMessage) {
        self.note_input();
        // The core's own box first: those messages were made before this one.
        if self.loaded {
            self.drain_engine();
        }
        self.route(vec![msg]);
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn applied(&self) -> u64 {
        self.stats.applied
    }

    fn catching_up(&self) -> bool {
        self.state != State::Live || !self.queue.is_empty()
    }

    fn set_turn_sent(&mut self, sent: bool) {
        self.turn_sent = sent;
    }

    fn set_turn_locked(&mut self, locked: bool) {
        self.turn_locked = locked;
    }

    fn turn_ended_by_journal(&self) -> u64 {
        self.journal_ended
    }

    fn take_turn_reopened(&mut self) -> bool {
        std::mem::take(&mut self.turn_reopened)
    }

    fn own_orders_pending(&self) -> bool {
        self.own_pending()
    }

    fn setup_graphical(&mut self) -> Result<(), String> {
        self.engine.setup_graphical()
    }

    fn unit_path(
        &mut self,
        player: i32,
        unit: i32,
        x: i32,
        y: i32,
    ) -> Result<Vec<PathNode>, String> {
        if !self.loaded {
            return Err("no game".into());
        }
        self.engine.unit_path(player, unit, x, y)
    }

    fn unit_reach(&mut self, player: i32, unit: i32) -> Result<Vec<PathNode>, String> {
        if !self.loaded {
            return Err("no game".into());
        }
        self.engine.unit_reach(player, unit)
    }

    fn unit_range_targets(&mut self, player: i32, unit: i32) -> Result<Vec<(i16, i16)>, String> {
        if !self.loaded {
            return Err("no game".into());
        }
        self.engine.unit_range_targets(player, unit)
    }

    fn trade_paths(&mut self, viewer: i32) -> Result<Vec<TradePath>, String> {
        if !self.loaded {
            return Err("no game".into());
        }
        self.engine.trade_paths(viewer)
    }
}
