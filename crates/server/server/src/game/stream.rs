//! The stream of one seat: the materialized view, the map basis, the batches built from what the
//! worker says, and the replay ring (net-protocol 4.4).
//!
//! [`SeatStream`] is plain synchronous state: the seat task ([`super::seat`]) feeds it and pushes
//! what it returns into the socket queue. It runs on the blocking pool when a step is big, so it
//! is `Send` and owns everything it touches.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use protocol::{
    AttachMode, Attached, BasisClaim, BasisState, CatalogRef, ChatLine, CmdId, CommandResult,
    ContextMsg, GameBatch, GameId, GameMsg, GcBase, JournalEntry, Keyframe, KeyframeReason,
    MAX_TERRAIN_SHARE_PLOTS, MapBasis, ModS2C, ModSetView, PlayerId, Resume, SeatView, ServerMsg,
    TerrainShare, TurnState, UndoLog, ViewDelta, ViewHash, ViewUpdate, WorldView, apply, check,
    check_pair,
};
use protocol::{Codec, Postcard};
use wire::Encoder;

use super::stats::{self, SeatStats, StreamSnapshot};

/// Batches kept for replay (NP-D16).
pub const RING_BATCHES: usize = 1024;
/// Bytes kept for replay (NP-D16).
pub const RING_BYTES: usize = 2 << 20;
/// Journal entries of a seat kept for a `Fresh` (NP-D16: 300).
pub const JOURNAL_KEEP: usize = 300;
/// Chat lines kept for a `Fresh` (NP-D16: 200).
pub const CHAT_KEEP: usize = 200;
/// `WantFull` is asked at most this often per seat.
pub const WANT_FULL_EVERY_MS: u64 = 5_000;
/// Bodies at least this big are packed on the blocking pool.
pub const BLOCKING_FROM: usize = 8 << 10;
/// The raw length of a keyframe is counted again after this many diffs (NP 4.7).
pub const RECOUNT_EVERY: u32 = 64;
/// Replica journal batches a seat keeps for a `Fresh` attach (the human phase of one turn). Past it the oldest
/// go; a client that attaches then finds a gap and asks for a new base.
pub const GC_BATCHES_KEEP: usize = 4096;

/// The last batches as sent, framed. Oldest out first when it is over the limit.
#[derive(Debug)]
pub struct Ring {
    items: VecDeque<(u64, Vec<u8>)>,
    bytes: usize,
    max_batches: usize,
    max_bytes: usize,
    overflow_total: u64,
}

impl Ring {
    pub fn new(max_batches: usize, max_bytes: usize) -> Self {
        Self {
            items: VecDeque::new(),
            bytes: 0,
            max_batches,
            max_bytes,
            overflow_total: 0,
        }
    }

    /// Adds the newest batch. The newest one always stays, whatever its size.
    pub fn push(&mut self, seq: u64, frame: Vec<u8>) {
        self.bytes += frame.len();
        self.items.push_back((seq, frame));
        while self.items.len() > 1
            && (self.items.len() > self.max_batches || self.bytes > self.max_bytes)
        {
            if let Some((_, old)) = self.items.pop_front() {
                self.bytes -= old.len();
                self.overflow_total += 1;
            }
        }
    }

    /// `seq` of the oldest batch still here.
    pub fn first_seq(&self) -> Option<u64> {
        self.items.front().map(|(s, _)| *s)
    }

    /// The frames of the batches after `seq`, oldest first.
    pub fn after(&self, seq: u64) -> impl Iterator<Item = &Vec<u8>> {
        self.items
            .iter()
            .filter(move |(s, _)| *s > seq)
            .map(|(_, f)| f)
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Batches pushed out since the start.
    pub fn overflow_total(&self) -> u64 {
        self.overflow_total
    }
}

/// What one step of the game gives one seat: at most one batch comes out of it.
#[derive(Debug, Default, Clone)]
pub struct Parts {
    /// The map basis the worker sent for this seat; used with the next `Full`.
    pub basis: Option<MapBasis>,
    pub view: Option<ViewUpdate>,
    pub results: Vec<CommandResult>,
    pub journal: Vec<JournalEntry>,
    /// `(rev, context)` as the worker sent them.
    pub contexts: Vec<(u32, ContextMsg)>,
    pub turn: Option<TurnState>,
    pub seats: Option<Vec<SeatView>>,
    pub chat: Vec<ChatLine>,
    /// Replica stream messages (`GcBase`, `GcJournal`, `GcTurn`), in order.
    pub gc: Vec<GameMsg>,
    /// Messages of the server halves of mods (`GameMsg::Mod`), in order.
    pub mods: Vec<ModS2C>,
    /// Terrain shares of the mods (API 1.2), in order: they go out as `GameMsg::TerrainShare` with the records of
    /// the seat's map basis.
    pub terrain: Vec<TerrainSharePart>,
}

/// One terrain share of a mod for one seat as the host made it: plot indexes, no records yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerrainSharePart {
    pub mod_idx: u16,
    pub reset: bool,
    pub plots: Vec<u32>,
}

impl Parts {
    /// Names of the parts that are in (the `batch` line of the journal).
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut k = Vec::new();
        if self.basis.is_some() {
            k.push("basis");
        }
        match &self.view {
            Some(ViewUpdate::Full { .. }) => k.push("full"),
            Some(ViewUpdate::Delta { .. }) => k.push("delta"),
            None => {}
        }
        for (name, present) in [
            ("results", !self.results.is_empty()),
            ("journal", !self.journal.is_empty()),
            ("context", !self.contexts.is_empty()),
            ("turn", self.turn.is_some()),
            ("seats", self.seats.is_some()),
            ("chat", !self.chat.is_empty()),
            ("gc", !self.gc.is_empty()),
            ("mod", !self.mods.is_empty()),
            ("terrain_share", !self.terrain.is_empty()),
        ] {
            if present {
                k.push(name);
            }
        }
        k
    }

    pub fn is_empty(&self) -> bool {
        self.basis.is_none()
            && self.view.is_none()
            && self.results.is_empty()
            && self.journal.is_empty()
            && self.contexts.is_empty()
            && self.turn.is_none()
            && self.seats.is_none()
            && self.chat.is_empty()
            && self.gc.is_empty()
            && self.mods.is_empty()
            && self.terrain.is_empty()
    }

    /// A rough guess of the body size in bytes, to choose between the task and the blocking pool.
    pub fn weight(&self) -> usize {
        let mut w = 64;
        if let Some(b) = &self.basis {
            w += b.plots.len() * 8;
        }
        match &self.view {
            Some(ViewUpdate::Full { view, .. }) => {
                w += view.map.plots.len() * 40 + view.units.len() * 100 + view.cities.len() * 200;
            }
            Some(ViewUpdate::Delta { diff, .. }) => {
                w += diff.plots.len() * 40
                    + diff.basis.len() * 8
                    + (diff.units.upsert.len() + diff.players.upsert.len()) * 100
                    + diff.cities.upsert.len() * 200;
            }
            None => {}
        }
        w + self.results.len() * 16
            + self.journal.len() * 80
            + self.contexts.len() * 500
            + self.chat.len() * 100
            + self.gc.len() * 200
            + self
                .mods
                .iter()
                .map(|m| m.payload.len() + 16)
                .sum::<usize>()
    }
}

/// A worker update that did not check out; the worker is asked for a full view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fail {
    /// The map basis is in doubt too.
    basis: bool,
}

/// What a step produced.
#[derive(Debug, Default)]
pub struct StepOut {
    /// The framed batch, already in the ring.
    pub frame: Option<Vec<u8>>,
    /// Ask the worker for a full view (`true`: with the basis).
    pub want_full: Option<bool>,
}

/// The answer to an attach: the `Attached` message and the frames that follow it.
#[derive(Debug)]
pub struct AttachOut {
    pub attached: ServerMsg,
    pub frames: Vec<Vec<u8>>,
    pub mode: AttachMode,
}

/// What a connection that attaches brings.
#[derive(Debug, Clone, Copy)]
pub struct AttachArgs {
    pub resume: Option<Resume>,
    /// The basis the client holds, from `JoinGame`; a `resume.basis` counts the same way.
    pub claim: Option<BasisClaim>,
    pub catalog: CatalogRef,
    pub last_cmd_id: CmdId,
}

/// The view of the seat as the gateway holds it.
#[derive(Debug)]
struct Mat {
    view: WorldView,
    hash: ViewHash,
    basis: BasisState,
    /// Revision of the view at the worker.
    rev: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CtxKey {
    Unit(i32),
    City(i32),
}

/// The view to put into the batch.
// One value per step, moved once: boxing would only add an allocation.
#[allow(clippy::large_enum_variant)]
enum ViewOut {
    None,
    /// A keyframe; `basis` forces the `MapBasis` before it.
    Keyframe {
        force_basis: bool,
    },
    Diff(ViewDelta),
}

pub struct SeatStream {
    game: GameId,
    player: PlayerId,
    stats: Arc<SeatStats>,
    encoder: Encoder,
    mat: Option<Mat>,
    /// A `Basis` that waits for the `Full` it belongs to.
    staged_basis: Option<BasisState>,
    /// Deltas are applied only while this holds; a failure or the start clears it until a `Full`.
    synced: bool,
    /// A worker update failed and the answer (`Full`) has not come.
    failed: bool,
    want_basis: bool,
    last_want_ms: Option<u64>,
    /// `seq` of the batch with the last view message.
    view_seq: u64,
    /// Hash of the basis the client holds at the end of the stream so far.
    sent_basis_hash: Option<u64>,
    corrupt_next_diff: bool,
    stream: u64,
    seq: u64,
    ring: Ring,
    journal: VecDeque<JournalEntry>,
    chat: VecDeque<ChatLine>,
    contexts: BTreeMap<CtxKey, (u32, ContextMsg)>,
    turn: Option<TurnState>,
    seats: Option<Vec<SeatView>>,
    /// Raw length of the last counted keyframe, and the diffs since.
    kf_raw: Option<usize>,
    since_recount: u32,
    /// The replica stream as far as this seat has been told.
    gc: GcSeat,
    /// The mod set of the game as this seat sees it: the first message of every `Fresh` stream (`None`: no mods).
    mods_view: Option<ModSetView>,
}

/// What a `Fresh` attach needs of the replica stream: the base of the chain and the batches after it.
#[derive(Debug, Default)]
struct GcSeat {
    base: Option<GcBase>,
    /// `(jseq, GCJ1 bytes)` after the base, in order.
    batches: VecDeque<(u64, Vec<u8>)>,
}

impl GcSeat {
    /// Follows a message that goes out.
    fn note(&mut self, msg: &GameMsg) {
        match msg {
            GameMsg::GcBase(b) => {
                self.base = Some(*b);
                // A base for one seat in the middle of the phase: the batches after it stay.
                self.batches.retain(|(j, _)| *j > b.jseq);
            }
            GameMsg::GcTurn(t) => {
                self.base = Some(GcBase {
                    epoch: t.epoch,
                    turn: t.turn,
                    jseq: t.jseq,
                    raw_hash: t.raw_hash,
                    snapshot: t.snapshot,
                });
                self.batches.clear();
            }
            GameMsg::GcJournal(j) => {
                let Some(base) = self.base.filter(|b| b.epoch == j.epoch) else {
                    return;
                };
                for (i, b) in j.batches.iter().enumerate() {
                    let jseq = j.first_jseq + i as u64;
                    if jseq > base.jseq {
                        self.batches.push_back((jseq, b.clone()));
                    }
                }
                while self.batches.len() > GC_BATCHES_KEEP {
                    self.batches.pop_front();
                }
            }
            _ => {}
        }
    }

    /// The messages that bring a new client to the present.
    fn fresh(&self) -> Vec<GameMsg> {
        let Some(base) = self.base else {
            return Vec::new();
        };
        let mut out = vec![GameMsg::GcBase(base)];
        if let Some((first, _)) = self.batches.front() {
            out.push(GameMsg::GcJournal(protocol::GcJournal {
                epoch: base.epoch,
                first_jseq: *first,
                batches: self.batches.iter().map(|(_, b)| b.clone()).collect(),
            }));
        }
        out
    }
}

fn random_stream() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        // Not secret, only different from the last one.
        return std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(1, |d| d.as_nanos() as u64);
    }
    u64::from_le_bytes(b)
}

fn raw_len<T: serde::Serialize>(v: &T) -> usize {
    Postcard.encode(v).map_or(0, |b| b.len())
}

/// The keyframe rule of NP-D7 for a diff that changes no more than half of the plots: `true` when a
/// keyframe goes instead. `z` is asked only when the raw sizes do not decide; it returns the packed
/// sizes `(diff, keyframe)`.
pub fn keyframe_wins(diff_raw: usize, kf_raw: usize, z: impl FnOnce() -> (usize, usize)) -> bool {
    if diff_raw * 100 < kf_raw * 30 {
        return false;
    }
    let (zd, zk) = z();
    zd * 100 > zk * 70
}

impl SeatStream {
    pub fn new(game: GameId, player: PlayerId, stats: Arc<SeatStats>) -> Self {
        Self {
            game,
            player,
            stats,
            encoder: Encoder::default(),
            mat: None,
            staged_basis: None,
            synced: false,
            failed: false,
            want_basis: false,
            last_want_ms: None,
            view_seq: 0,
            sent_basis_hash: None,
            corrupt_next_diff: false,
            stream: random_stream(),
            seq: 0,
            ring: Ring::new(RING_BATCHES, RING_BYTES),
            journal: VecDeque::new(),
            chat: VecDeque::new(),
            contexts: BTreeMap::new(),
            turn: None,
            seats: None,
            kf_raw: None,
            since_recount: 0,
            gc: GcSeat::default(),
            mods_view: None,
        }
    }

    /// The mod set the next `Fresh` attach starts with (`GameMsg::Mods`); `None` for a game without mods.
    pub fn set_mods(&mut self, view: Option<ModSetView>) {
        self.mods_view = view;
    }

    pub fn player(&self) -> PlayerId {
        self.player
    }

    /// The worker that fed this stream is gone: what it said about contexts is stale, and the
    /// view waits for the first `Full` of the next worker (its `rev` starts over).
    pub fn worker_lost(&mut self) {
        self.synced = false;
        self.failed = false;
        self.contexts.clear();
        self.staged_basis = None;
        // The chain was the old worker's: the next one starts with its own base.
        self.gc = GcSeat::default();
    }

    /// The number of the stream (`Attached.stream`).
    pub fn stream_id(&self) -> u64 {
        self.stream
    }

    /// `seq` of the last batch.
    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// `seq` of the batch with the last view message.
    pub fn view_seq(&self) -> u64 {
        self.view_seq
    }

    /// Revision of the view at the worker; 0 before the first `Full`.
    pub fn rev(&self) -> u32 {
        self.mat.as_ref().map_or(0, |m| m.rev)
    }

    pub fn view(&self) -> Option<&WorldView> {
        self.mat.as_ref().map(|m| &m.view)
    }

    pub fn view_hash(&self) -> Option<u64> {
        self.mat.as_ref().map(|m| m.hash.value())
    }

    pub fn basis_hash(&self) -> Option<u64> {
        self.mat.as_ref().map(|m| m.basis.hash())
    }

    /// Hash of the basis the client holds, as far as the stream knows.
    pub fn sent_basis_hash(&self) -> Option<u64> {
        self.sent_basis_hash
    }

    /// True while a `Full` is awaited (the start, or after a failed update).
    pub fn waits_for_full(&self) -> bool {
        !self.synced
    }

    pub fn ring(&self) -> &Ring {
        &self.ring
    }

    /// A view is there: an attach can be answered.
    pub fn has_view(&self) -> bool {
        self.mat.is_some()
    }

    fn publish(&self) {
        let (view_hash, basis_hash, rev) = self
            .mat
            .as_ref()
            .map_or((0, 0, 0), |m| (m.hash.value(), m.basis.hash(), m.rev));
        self.stats.set_snapshot(StreamSnapshot {
            stream: self.stream,
            seq: self.seq,
            view_seq: self.view_seq,
            view_hash,
            basis_hash,
            rev,
            ring_batches: self.ring.len(),
            ring_bytes: self.ring.bytes(),
        });
        self.stats.ring_overflow_total.store(
            self.ring.overflow_total(),
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// `WantFull` with the 5 s rule; `Some(basis)` when one is to go out now.
    fn want(&mut self, now_ms: u64) -> Option<bool> {
        let due = self
            .last_want_ms
            .is_none_or(|t| now_ms.saturating_sub(t) >= WANT_FULL_EVERY_MS);
        if !due {
            return None;
        }
        self.last_want_ms = Some(now_ms);
        stats::bump(&self.stats.wantfull);
        Some(self.want_basis)
    }

    fn fail(&mut self, f: Fail) {
        stats::bump(&self.stats.hash_fail_worker);
        self.synced = false;
        self.failed = true;
        self.want_basis |= f.basis;
    }

    // ------------------------------------------------------------ worker updates

    fn on_full(&mut self, rev: u32, view: WorldView, hash: u64) -> Result<ViewOut, Fail> {
        let basis_ref = self
            .staged_basis
            .as_ref()
            .or(self.mat.as_ref().map(|m| &m.basis))
            .ok_or(Fail { basis: true })?;
        check(&view).map_err(|_| Fail { basis: false })?;
        let h = ViewHash::of(&view);
        if h.value() != hash {
            return Err(Fail { basis: false });
        }
        check_pair(&view, basis_ref.map()).map_err(|_| Fail { basis: true })?;
        let basis = match self.staged_basis.take() {
            Some(b) => b,
            None => match self.mat.take() {
                Some(m) => m.basis,
                None => return Err(Fail { basis: true }),
            },
        };
        self.mat = Some(Mat {
            view,
            hash: h,
            basis,
            rev,
        });
        self.synced = true;
        self.failed = false;
        self.want_basis = false;
        self.last_want_ms = None;
        Ok(ViewOut::Keyframe { force_basis: false })
    }

    #[allow(clippy::too_many_arguments)]
    fn on_delta(
        &mut self,
        base_rev: u32,
        rev: u32,
        diff: protocol::ViewDiff,
        hash: u64,
        basis_hash: Option<u64>,
    ) -> Result<ViewOut, Fail> {
        let Some(mat) = self.mat.as_mut() else {
            return Err(Fail { basis: false });
        };
        if base_rev != mat.rev {
            return Err(Fail { basis: false });
        }
        if basis_hash.is_some() == diff.basis.is_empty() {
            return Err(Fail { basis: true });
        }
        let undo = UndoLog::capture(&mat.view, &mat.hash, &mat.basis, &diff);
        if apply(&mut mat.view, &mut mat.hash, &mut mat.basis, &diff).is_err() {
            undo.restore(&mut mat.view, &mut mat.hash, &mut mat.basis);
            return Err(Fail { basis: false });
        }
        if mat.hash.value() != hash {
            undo.restore(&mut mat.view, &mut mat.hash, &mut mat.basis);
            return Err(Fail { basis: false });
        }
        if basis_hash.is_some_and(|h| h != mat.basis.hash()) {
            undo.restore(&mut mat.view, &mut mat.hash, &mut mat.basis);
            return Err(Fail { basis: true });
        }
        mat.rev = rev;
        let delta = ViewDelta {
            base: self.view_seq,
            diff,
            hash,
            basis_hash,
        };
        Ok(if self.keyframe_instead(&delta) {
            ViewOut::Keyframe { force_basis: false }
        } else {
            ViewOut::Diff(delta)
        })
    }

    /// NP-D7: the keyframe goes instead of this diff.
    fn keyframe_instead(&mut self, delta: &ViewDelta) -> bool {
        let Some(mat) = self.mat.as_ref() else {
            return false;
        };
        let plots = mat.view.map.plots.len();
        let basis_plots = mat.basis.map().plots.len();
        // More than half of the plots, or of the basis records: always a keyframe.
        if delta.diff.plots.len() * 2 > plots || delta.diff.basis.len() * 2 > basis_plots {
            return true;
        }
        let kf = |m: &Mat| Keyframe {
            view: m.view.clone(),
            hash: m.hash.value(),
            basis_hash: m.basis.hash(),
        };
        let kf_raw = match self.kf_raw {
            Some(n) if self.since_recount < RECOUNT_EVERY => n,
            _ => {
                let n = raw_len(&kf(mat));
                self.kf_raw = Some(n);
                self.since_recount = 0;
                n
            }
        };
        self.since_recount += 1;
        let diff_raw = raw_len(delta);
        keyframe_wins(diff_raw, kf_raw, || {
            let zd = wire::compress(&Postcard.encode(delta).unwrap_or_default()).len();
            let zk = wire::compress(&Postcard.encode(&kf(mat)).unwrap_or_default()).len();
            (zd, zk)
        })
    }

    // ------------------------------------------------------------ batches

    /// Takes what the game says about the seat and builds at most one batch. The batch is framed
    /// (packed from 256 bytes) and kept in the ring; the caller sends it when a socket is attached.
    pub fn step(&mut self, parts: Parts, now_ms: u64) -> StepOut {
        let mut out = StepOut::default();
        let Parts {
            basis,
            view,
            results,
            journal,
            contexts,
            turn,
            seats,
            chat,
            gc,
            mods,
            terrain,
        } = parts;
        if let Some(b) = basis {
            match BasisState::new(b) {
                Ok(b) => self.staged_basis = Some(b),
                Err(e) => {
                    tracing::warn!(seat = self.player, error = %e, "the worker sent a bad map basis");
                    self.fail(Fail { basis: true });
                    out.want_full = self.want(now_ms);
                }
            }
        }
        let mut view_out = ViewOut::None;
        if let Some(update) = view {
            let res = match update {
                ViewUpdate::Full { rev, view, hash } => self.on_full(rev, view, hash),
                ViewUpdate::Delta {
                    base_rev,
                    rev,
                    diff,
                    hash,
                    basis_hash,
                } => {
                    if self.synced {
                        self.on_delta(base_rev, rev, diff, hash, basis_hash)
                    } else {
                        // Waiting for a `Full`: the delta is of no use. If the last ask went
                        // unanswered for long, ask again.
                        stats::bump(&self.stats.deltas_dropped);
                        if self.failed {
                            out.want_full = out.want_full.or(self.want(now_ms));
                        }
                        Ok(ViewOut::None)
                    }
                }
            };
            match res {
                Ok(v) => view_out = v,
                Err(f) => {
                    tracing::warn!(seat = self.player, "a worker view update did not check out");
                    self.fail(f);
                    out.want_full = out.want_full.or(self.want(now_ms));
                }
            }
        }
        let terrain = self.terrain_shares(terrain);
        out.frame = self.build(
            view_out, false, results, journal, contexts, turn, seats, chat, gc, mods, terrain,
        );
        self.publish();
        out
    }

    /// The records of the shared plots, from the map basis the seat has (the materialized one, else the one staged for
    /// the next `Full`): the fog-free basis records, the encoding of `MapBasis::plots`. A plot past the basis is
    /// dropped; with no basis at all the share is dropped with a warning (the client has no map to draw it on
    /// either; the mod shares again in the next `on_attach`).
    fn terrain_shares(&self, parts: Vec<TerrainSharePart>) -> Vec<TerrainShare> {
        if parts.is_empty() {
            return Vec::new();
        }
        let basis = self
            .mat
            .as_ref()
            .map(|m| m.basis.map())
            .or_else(|| self.staged_basis.as_ref().map(BasisState::map));
        let Some(basis) = basis else {
            tracing::warn!(
                seat = self.player,
                "a terrain share before the map basis, dropped"
            );
            return Vec::new();
        };
        parts
            .into_iter()
            .map(|p| TerrainShare {
                mod_idx: p.mod_idx,
                reset: p.reset,
                plots: p
                    .plots
                    .into_iter()
                    .filter_map(|i| basis.plots.get(i as usize).map(|b| (i, b.clone())))
                    .take(MAX_TERRAIN_SHARE_PLOTS)
                    .collect(),
            })
            .filter(|s| s.reset || !s.plots.is_empty())
            .collect()
    }

    /// The batch of one step. `forced_basis`: the `MapBasis` goes before the keyframe whatever
    /// the client holds (`NeedKeyframe { BasisMismatch }`).
    #[allow(clippy::too_many_arguments)]
    fn build(
        &mut self,
        view_out: ViewOut,
        forced_basis: bool,
        results: Vec<CommandResult>,
        journal: Vec<JournalEntry>,
        contexts: Vec<(u32, ContextMsg)>,
        turn: Option<TurnState>,
        seats: Option<Vec<SeatView>>,
        chat: Vec<ChatLine>,
        gc: Vec<GameMsg>,
        mods: Vec<ModS2C>,
        terrain: Vec<TerrainShare>,
    ) -> Option<Vec<u8>> {
        let seq = self.seq + 1;
        let mut msgs: Vec<GameMsg> = Vec::new();
        let mut with_basis = false;
        let mut with_keyframe = false;
        let has_view = !matches!(view_out, ViewOut::None);
        match view_out {
            ViewOut::None => {}
            ViewOut::Keyframe { force_basis } => {
                if let Some(mat) = self.mat.as_ref() {
                    let bh = mat.basis.hash();
                    if forced_basis || force_basis || self.sent_basis_hash != Some(bh) {
                        msgs.push(GameMsg::MapBasis(mat.basis.map().clone()));
                        stats::bump(&self.stats.basis_sent);
                        with_basis = true;
                    }
                    self.sent_basis_hash = Some(bh);
                    let kf = Keyframe {
                        view: mat.view.clone(),
                        hash: mat.hash.value(),
                        basis_hash: bh,
                    };
                    self.kf_raw = Some(raw_len(&kf));
                    self.since_recount = 0;
                    msgs.push(GameMsg::Keyframe(kf));
                    stats::bump(&self.stats.keyframes);
                    with_keyframe = true;
                }
            }
            ViewOut::Diff(mut delta) => {
                if self.corrupt_next_diff {
                    // Test hook `corrupt_hash`: the client must notice and ask for a keyframe.
                    self.corrupt_next_diff = false;
                    delta.hash ^= 1;
                }
                if let Some(h) = delta.basis_hash {
                    self.sent_basis_hash = Some(h);
                }
                msgs.push(GameMsg::Diff(delta));
                stats::bump(&self.stats.diffs);
            }
        }
        if !results.is_empty() {
            msgs.push(GameMsg::Results(results));
        }
        if !journal.is_empty() {
            self.journal.extend(journal.iter().cloned());
            while self.journal.len() > JOURNAL_KEEP {
                self.journal.pop_front();
            }
            msgs.push(GameMsg::Journal(journal));
        }
        // Contexts: only for the revision the view has now; the batch with that view is `seq` if
        // it carries one, else the last one that did.
        let ctx_seq = if has_view { seq } else { self.view_seq };
        let rev = self.mat.as_ref().map_or(0, |m| m.rev);
        for (ctx_rev, mut ctx) in contexts {
            if ctx_rev != rev || self.mat.is_none() || !self.synced {
                stats::bump(&self.stats.contexts_dropped);
                continue;
            }
            set_view_seq(&mut ctx, ctx_seq);
            self.remember_context(rev, &ctx);
            msgs.push(GameMsg::Context(ctx));
        }
        if let Some(t) = turn {
            self.turn = Some(t.clone());
            msgs.push(GameMsg::TurnState(t));
        }
        if let Some(s) = seats {
            self.seats = Some(s.clone());
            msgs.push(GameMsg::Seats(s));
        }
        if !chat.is_empty() {
            self.chat.extend(chat.iter().cloned());
            while self.chat.len() > CHAT_KEEP {
                self.chat.pop_front();
            }
            msgs.push(GameMsg::Chat(chat));
        }
        for m in gc {
            self.gc.note(&m);
            msgs.push(m);
        }
        msgs.extend(mods.into_iter().map(GameMsg::Mod));
        msgs.extend(terrain.into_iter().map(GameMsg::TerrainShare));
        if msgs.is_empty() {
            return None;
        }
        self.seq = seq;
        if has_view {
            self.view_seq = seq;
        }
        Some(self.seal(seq, msgs, with_basis, with_keyframe))
    }

    /// Frames the batch, counts it and puts it into the ring.
    fn seal(
        &mut self,
        seq: u64,
        msgs: Vec<GameMsg>,
        with_basis: bool,
        with_keyframe: bool,
    ) -> Vec<u8> {
        let n_msgs = msgs.len();
        // The sizes of the big parts alone, for the counters (AC-N3.5).
        let alone = if with_basis && with_keyframe {
            let size = |m: &GameMsg| {
                self.encoder
                    .frame_msg(&ServerMsg::Game(GameBatch {
                        seq,
                        msgs: vec![m.clone()],
                    }))
                    .map_or(0, |f| f.len())
            };
            let basis = msgs
                .iter()
                .find(|m| matches!(m, GameMsg::MapBasis(_)))
                .map_or(0, size);
            let kf = msgs
                .iter()
                .find(|m| matches!(m, GameMsg::Keyframe(_)))
                .map_or(0, size);
            Some((basis, kf))
        } else {
            None
        };
        let msg = ServerMsg::Game(GameBatch { seq, msgs });
        let raw = Postcard.encode(&msg).unwrap_or_else(|e| {
            // Plain data does not fail to encode; if it does the seat gets an empty batch (a stream error, so a
            // new keyframe), and the server says so instead of hiding it.
            tracing::error!(seq, error = %e, "a batch for a seat cannot be encoded");
            Vec::new()
        });
        let frame = self.encoder.frame(&raw);
        let st = &self.stats;
        stats::bump(&st.tx_batches);
        st.tx_msgs
            .fetch_add(n_msgs as u64, std::sync::atomic::Ordering::Relaxed);
        st.tx_body_bytes
            .fetch_add(frame.len() as u64, std::sync::atomic::Ordering::Relaxed);
        st.tx_raw_bytes
            .fetch_add(raw.len() as u64, std::sync::atomic::Ordering::Relaxed);
        match alone {
            Some((basis, kf)) => {
                stats::max(&st.basis_bytes_max, basis);
                stats::max(&st.keyframe_bytes_max, kf);
                stats::max(&st.cold_batch_bytes_max, frame.len());
            }
            None => {
                if with_keyframe {
                    stats::max(&st.keyframe_bytes_max, frame.len());
                } else if with_basis {
                    stats::max(&st.basis_bytes_max, frame.len());
                }
            }
        }
        self.ring.push(seq, frame.clone());
        st.note_tx(
            self.turn.as_ref().map_or(0, |t| t.turn),
            frame.len(),
            with_keyframe,
        );
        frame
    }

    /// The next diff of this seat leaves with a wrong view hash (test hook).
    pub fn corrupt_next_diff(&mut self) {
        self.corrupt_next_diff = true;
    }

    fn remember_context(&mut self, rev: u32, ctx: &ContextMsg) {
        match ctx {
            ContextMsg::Unit(u) => {
                self.contexts
                    .insert(CtxKey::Unit(u.unit), (rev, ctx.clone()));
            }
            ContextMsg::City(c) => {
                self.contexts
                    .insert(CtxKey::City(c.city), (rev, ctx.clone()));
            }
            ContextMsg::Valid { units, city, .. } => {
                // Still good for this revision.
                for u in units {
                    if let Some(e) = self.contexts.get_mut(&CtxKey::Unit(*u)) {
                        e.0 = rev;
                    }
                }
                if let Some(c) = city
                    && let Some(e) = self.contexts.get_mut(&CtxKey::City(*c))
                {
                    e.0 = rev;
                }
            }
            ContextMsg::Dropped { unit } => {
                self.contexts.remove(&CtxKey::Unit(*unit));
            }
        }
    }

    /// The client asks for a keyframe (`NeedKeyframe`): the next batch carries one, and for
    /// `BasisMismatch` the `MapBasis` before it. Other seats are not touched. `None` while no
    /// view is there to send.
    pub fn need_keyframe(&mut self, reason: KeyframeReason) -> Option<Vec<u8>> {
        stats::bump(&self.stats.need_keyframe[stats::reason_index(reason)]);
        if self.mat.is_none() || !self.synced {
            return None;
        }
        let forced = reason == KeyframeReason::BasisMismatch;
        let out = self.build(
            ViewOut::Keyframe {
                force_basis: forced,
            },
            forced,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        self.publish();
        out
    }

    // ------------------------------------------------------------ attach

    /// Answers a connection that takes the seat: `Replay` when it names this stream and every
    /// batch after its `last_seq` is still in the ring; else `Fresh` with a new `stream`, an empty
    /// ring and a first batch built from the materialized view. `None` until the first
    /// `Full` has come.
    pub fn attach(&mut self, a: AttachArgs) -> Option<AttachOut> {
        self.mat.as_ref()?;
        if let Some(r) = a.resume
            && r.stream == self.stream
            && r.last_seq <= self.seq
            && (r.last_seq == self.seq
                || self.ring.first_seq().is_some_and(|f| f <= r.last_seq + 1))
        {
            stats::bump(&self.stats.resume_replay);
            let frames = self.ring.after(r.last_seq).cloned().collect();
            let attached = self.attached_msg(&a, AttachMode::Replay);
            self.publish();
            return Some(AttachOut {
                attached,
                frames,
                mode: AttachMode::Replay,
            });
        }
        stats::bump(&self.stats.resume_fresh);
        self.stream = random_stream();
        self.ring.clear();
        self.seq = 0;
        let claim = a.claim.or(a.resume.and_then(|r| r.basis));
        let frame = self.fresh_batch(claim);
        let attached = self.attached_msg(&a, AttachMode::Fresh);
        self.publish();
        Some(AttachOut {
            attached,
            frames: frame.into_iter().collect(),
            mode: AttachMode::Fresh,
        })
    }

    fn attached_msg(&self, a: &AttachArgs, mode: AttachMode) -> ServerMsg {
        ServerMsg::Attached(Attached {
            game: self.game,
            player: self.player,
            stream: self.stream,
            mode,
            catalog: a.catalog,
            last_cmd_id: a.last_cmd_id,
        })
    }

    /// The first batch of a `Fresh` stream.
    fn fresh_batch(&mut self, claim: Option<BasisClaim>) -> Option<Vec<u8>> {
        let mat = self.mat.as_ref()?;
        let seq = 1;
        let bh = mat.basis.hash();
        let mut msgs = Vec::new();
        // The mod set comes first: the client builds its mods before anything of the game reaches them.
        if let Some(v) = &self.mods_view {
            msgs.push(GameMsg::Mods(v.clone()));
        }
        let mut with_basis = false;
        if claim.is_some_and(|c| c.game == self.game && c.hash == bh) {
            stats::bump(&self.stats.basis_skipped);
        } else {
            msgs.push(GameMsg::MapBasis(mat.basis.map().clone()));
            stats::bump(&self.stats.basis_sent);
            with_basis = true;
        }
        self.sent_basis_hash = Some(bh);
        let kf = Keyframe {
            view: mat.view.clone(),
            hash: mat.hash.value(),
            basis_hash: bh,
        };
        self.kf_raw = Some(raw_len(&kf));
        self.since_recount = 0;
        msgs.push(GameMsg::Keyframe(kf));
        stats::bump(&self.stats.keyframes);
        if !self.journal.is_empty() {
            msgs.push(GameMsg::Journal(self.journal.iter().cloned().collect()));
        }
        let rev = mat.rev;
        for (ctx_rev, ctx) in self.contexts.values() {
            if *ctx_rev == rev {
                let mut ctx = ctx.clone();
                set_view_seq(&mut ctx, seq);
                msgs.push(GameMsg::Context(ctx));
            }
        }
        if let Some(t) = &self.turn {
            msgs.push(GameMsg::TurnState(t.clone()));
        }
        if let Some(s) = &self.seats {
            msgs.push(GameMsg::Seats(s.clone()));
        }
        if !self.chat.is_empty() {
            msgs.push(GameMsg::Chat(self.chat.iter().cloned().collect()));
        }
        msgs.extend(self.gc.fresh());
        self.seq = seq;
        self.view_seq = seq;
        Some(self.seal(seq, msgs, with_basis, true))
    }
}

fn set_view_seq(ctx: &mut ContextMsg, seq: u64) {
    match ctx {
        ContextMsg::Unit(u) => u.view_seq = seq,
        ContextMsg::City(c) => c.view_seq = seq,
        ContextMsg::Valid { view_seq, .. } => *view_seq = seq,
        ContextMsg::Dropped { .. } => {}
    }
}
