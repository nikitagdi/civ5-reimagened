//! The game stream (net-client 4.3, NC-D2–D4, NC-D18): batches strictly by `seq`, the keyframe and the diffs
//! with their hashes, the undo log, `NeedKeyframe` and the map basis. Pure state: `ClientCore` feeds batches
//! and executes the effects that come back.
//!
//! `base` is the view exactly as the server sent it; only it is checked by a hash. A batch is applied whole
//! and in order, and `revision` grows after it, so a frame never sees a half-applied batch.

use protocol::{
    BasisClaim, BasisState, ChatLine, GameBatch, GameId, GameMsg, JournalEntry, Keyframe,
    KeyframeReason, MapBasis, SeatView, TurnState, UndoLog, ViewDelta, ViewHash, WorldView, apply,
    check, check_pair,
};
use protocol::{CityKey, UnitKey};

use crate::change::{ChangeAcc, ViewChange};
use crate::contexts::Contexts;
use crate::lobby_model::{HostChange, HostWatch};
use crate::terrain_share::SharedTerrain;
use crate::{ClientEvent, ClientStats, ReplicaMsg};

/// Lines kept of the journal and of the chat; older ones are dropped.
const JOURNAL_CAP: usize = 2_000;
const CHAT_CAP: usize = 500;
/// The ladder of stream failures counts them over this window.
pub(crate) const LADDER_WINDOW_MS: u64 = 60_000;

/// A chat line and the time the client received it: that time is shown, not `ChatLine.server_ms` (NP 4.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatEntry {
    pub line: ChatLine,
    pub received_ms: u64,
}

/// Why a stream is broken as a whole (not just one view).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakWhy {
    /// A `seq` was skipped.
    Gap,
    /// A frame or a batch could not be read.
    Undecodable,
    /// A keyframe failed again while one was already awaited.
    KeyframeFailed,
    /// No keyframe came in `keyframe_wait_ms`, twice.
    KeyframeTimeout,
}

/// What the core has to do after the stream looked at a batch.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Effect {
    /// Send `ClientMsg::NeedKeyframe`.
    NeedKeyframe(KeyframeReason),
    /// The stream is broken: reconnect (the ladder decides how).
    Break(BreakWhy),
    /// The map basis changed: tell the session its claim.
    Basis(Option<BasisClaim>),
    HostChanged(HostChange),
}

/// What the stream needs to know from outside, for one call.
pub(crate) struct Cx<'a> {
    pub now: u64,
    /// The game of the current `Attached`.
    pub game: GameId,
    pub nick: &'a str,
    pub host: &'a mut HostWatch,
    pub stats: &'a mut ClientStats,
}

#[derive(Debug, Clone)]
struct Base {
    view: WorldView,
    hash: ViewHash,
}

#[derive(Debug, Clone, Copy)]
struct Waiting {
    reason: KeyframeReason,
    since: u64,
    retried: bool,
}

pub(crate) struct Stream {
    base: Option<Base>,
    basis: Option<BasisState>,
    /// `seq` of the batch with the last view message.
    view_seq: u64,
    waiting: Option<Waiting>,
    revision: u64,
    change: ChangeAcc,
    turn_state: Option<TurnState>,
    /// The AI civilizations the server started and ended in the AI phase of a turn (`GameMsg::AiTurn`), in order. It is
    /// replaced by the log of the next turn and kept past the load of the snapshot (the UI takes the events it has not seen
    /// even when the new turn arrived in the same frame).
    ai_turn: Option<protocol::AiTurn>,
    seats: Vec<SeatView>,
    chat: Vec<ChatEntry>,
    /// Chat lines ever received (never reset): the original UI tells the new ones from the old.
    chat_total: u64,
    journal: Vec<JournalEntry>,
    /// Contexts of the units and the city in focus (NC5).
    contexts: Contexts,
    /// Times of recent stream failures, for the ladder.
    breaks: Vec<u64>,
    /// Replica messages of the batch being applied, in the order they came; `ClientCore` takes them after
    /// the batch is accepted (a broken batch is not taken, so its replica messages must not leak).
    replica: Vec<ReplicaMsg>,
    /// The mod messages of the batch being applied (same rule as `replica`).
    mods: Vec<ClientEvent>,
    /// The terrain shares of the batch being applied (same rule), and the plots shared so far (mod API 1.2).
    terrain_in: Vec<protocol::TerrainShare>,
    terrain: SharedTerrain,
}

impl Stream {
    pub fn new() -> Self {
        Stream {
            base: None,
            basis: None,
            view_seq: 0,
            waiting: None,
            revision: 0,
            change: ChangeAcc::new(),
            turn_state: None,
            ai_turn: None,
            seats: Vec::new(),
            chat: Vec::new(),
            chat_total: 0,
            journal: Vec::new(),
            contexts: Contexts::new(),
            breaks: Vec::new(),
            replica: Vec::new(),
            mods: Vec::new(),
            terrain_in: Vec::new(),
            terrain: SharedTerrain::default(),
        }
    }

    /// The mod messages of the last batch, in order (empty after a broken batch).
    pub fn take_mods(&mut self) -> Vec<ClientEvent> {
        std::mem::take(&mut self.mods)
    }

    /// The replica messages of the last batch, in order (empty after a broken batch).
    pub fn take_replica(&mut self) -> Vec<ReplicaMsg> {
        std::mem::take(&mut self.replica)
    }

    // ------------------------------------------------------------ reading

    pub fn view(&self) -> Option<&WorldView> {
        self.base.as_ref().map(|b| &b.view)
    }

    pub fn base_hash(&self) -> Option<u64> {
        self.base.as_ref().map(|b| b.hash.value())
    }

    pub fn basis(&self) -> Option<&MapBasis> {
        self.basis.as_ref().map(BasisState::map)
    }

    pub fn basis_hash(&self) -> Option<u64> {
        self.basis.as_ref().map(BasisState::hash)
    }

    /// The plots the mods let this seat draw without revealing them (mod API 1.2); not part of the view.
    pub fn terrain_share(&self) -> &SharedTerrain {
        &self.terrain
    }

    pub fn view_seq(&self) -> u64 {
        self.view_seq
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn turn_state(&self) -> Option<&TurnState> {
        self.turn_state.as_ref()
    }

    pub fn ai_turn(&self) -> Option<&protocol::AiTurn> {
        self.ai_turn.as_ref()
    }

    pub fn seats(&self) -> &[SeatView] {
        &self.seats
    }

    pub fn chat_total(&self) -> u64 {
        self.chat_total
    }

    pub fn chat(&self) -> &[ChatEntry] {
        &self.chat
    }

    pub fn journal(&self) -> &[JournalEntry] {
        &self.journal
    }

    pub fn contexts(&self) -> &Contexts {
        &self.contexts
    }

    pub fn contexts_mut(&mut self) -> &mut Contexts {
        &mut self.contexts
    }

    /// A keyframe was asked for and has not come: diffs are skipped, and the layer keeps its records.
    pub fn awaiting_keyframe(&self) -> bool {
        self.waiting.is_some()
    }

    /// A line of the journal made by the client itself (a rolled-back order); the server's lines come
    /// with the `Journal` message.
    pub fn push_journal(&mut self, entry: JournalEntry) {
        self.journal.push(entry);
        trim(&mut self.journal, JOURNAL_CAP);
    }

    /// Test port only (`ClientCore::debug_corrupt_view`): the hash kept for the base no longer matches the base,
    /// so the next diff finds it apart from the server's.
    pub fn debug_corrupt(&mut self) {
        if let Some(b) = self.base.as_mut() {
            let mut other = b.view.clone();
            match other.units.first_mut() {
                Some(u) => u.hp = u.hp.wrapping_add(1),
                None => other.top_bar.gold = other.top_bar.gold.wrapping_add(7),
            }
            b.hash = ViewHash::of(&other);
        }
    }

    /// The optimistic layer touched these records (4.10).
    pub fn layer_changed(&mut self, units: &[UnitKey], cities: &[CityKey]) {
        self.change.layer(units, cities);
    }

    pub fn claim(&self) -> Option<BasisClaim> {
        self.basis.as_ref().map(|b| BasisClaim {
            game: b.map().game,
            hash: b.hash(),
        })
    }

    pub fn take_change(&mut self) -> ViewChange {
        self.change.take()
    }

    // ------------------------------------------------------------ resets

    /// `Attached { Fresh }`: base, journal, chat, turn state and seats start over; the map basis of the same
    /// game stays (NC-D18).
    pub fn reset_fresh(&mut self) {
        self.base = None;
        self.view_seq = 0;
        self.waiting = None;
        self.turn_state = None;
        self.ai_turn = None;
        self.seats.clear();
        self.chat.clear();
        self.journal.clear();
        self.contexts.clear();
        self.terrain.clear();
        self.change.all();
    }

    /// Another game: the basis goes too.
    pub fn reset_game(&mut self) {
        self.reset_fresh();
        self.basis = None;
        self.change.basis_all();
        self.breaks.clear();
    }

    /// `Detached` or `Replaced`: nothing of the game is shown any more.
    pub fn clear_view(&mut self) {
        self.reset_fresh();
    }

    // ------------------------------------------------------------ NeedKeyframe, ladder

    /// One `NeedKeyframe` per case: while one is awaited, further failures wait for it.
    fn request(&mut self, now: u64, reason: KeyframeReason, cx: &mut Cx<'_>) -> Option<Effect> {
        if self.waiting.is_some() {
            return None;
        }
        self.waiting = Some(Waiting {
            reason,
            since: now,
            retried: false,
        });
        count_need(cx.stats, reason);
        Some(Effect::NeedKeyframe(reason))
    }

    /// After a `Replay`: if a keyframe was awaited, the old `NeedKeyframe` may have died with the socket.
    pub fn resend_if_waiting(&mut self, now: u64, stats: &mut ClientStats) -> Option<Effect> {
        let w = self.waiting.as_mut()?;
        w.since = now;
        w.retried = false;
        count_need(stats, w.reason);
        Some(Effect::NeedKeyframe(w.reason))
    }

    /// Waiting for a keyframe: after `keyframe_wait_ms` one repeat, then the stream is given up.
    pub fn tick(&mut self, now: u64, wait_ms: u64, stats: &mut ClientStats) -> Option<Effect> {
        let w = self.waiting.as_mut()?;
        if now.saturating_sub(w.since) < wait_ms {
            return None;
        }
        w.since = now;
        if w.retried {
            return Some(Effect::Break(BreakWhy::KeyframeTimeout));
        }
        w.retried = true;
        count_need(stats, w.reason);
        Some(Effect::NeedKeyframe(w.reason))
    }

    /// Records a stream failure and returns how many there were in the last minute (this one included).
    pub fn register_break(&mut self, now: u64) -> usize {
        self.breaks
            .retain(|t| now.saturating_sub(*t) < LADDER_WINDOW_MS);
        self.breaks.push(now);
        self.breaks.len()
    }

    // ------------------------------------------------------------ batches

    /// Applies a batch whole and in order. `Break` among the effects means the batch was not accepted and
    /// `seq` must not advance.
    pub fn on_batch(&mut self, batch: GameBatch, cx: &mut Cx<'_>) -> Vec<Effect> {
        let seq = batch.seq;
        let mut fx = Vec::new();
        cx.stats.batches += 1;
        // A `MapBasis` is staged until its keyframe checks out: on any failure the view and the basis stay
        // as they were.
        let mut staged: Option<BasisState> = None;
        let mut basis_failed = false;
        for msg in batch.msgs {
            match msg {
                GameMsg::MapBasis(m) => match self.stage_basis(m, cx) {
                    Ok(b) => staged = Some(b),
                    Err(()) => {
                        basis_failed = true;
                        staged = None;
                        fx.extend(self.request(cx.now, KeyframeReason::BasisMismatch, cx));
                    }
                },
                GameMsg::Keyframe(k) => {
                    if basis_failed {
                        // A pair was already asked for; this keyframe would not fit anyway.
                        continue;
                    }
                    self.keyframe(seq, k, staged.take(), cx, &mut fx);
                }
                GameMsg::Diff(d) => self.diff(seq, d, cx, &mut fx),
                GameMsg::TurnState(t) => {
                    self.turn_state = Some(t);
                }
                GameMsg::AiTurn(p) => match self.ai_turn.as_mut() {
                    Some(log) if log.turn == p.turn => log.events.extend(p.events),
                    _ => self.ai_turn = Some(p),
                },
                GameMsg::Seats(s) => {
                    if let Some(c) = cx.host.seats(&s, cx.nick) {
                        fx.push(Effect::HostChanged(c));
                    }
                    self.seats = s;
                }
                GameMsg::Chat(lines) => {
                    self.chat_total += lines.len() as u64;
                    self.chat.extend(lines.into_iter().map(|line| ChatEntry {
                        line,
                        received_ms: cx.now,
                    }));
                    trim(&mut self.chat, CHAT_CAP);
                }
                GameMsg::Journal(entries) => {
                    self.journal.extend(entries);
                    trim(&mut self.journal, JOURNAL_CAP);
                }
                GameMsg::Context(c) => self.contexts.apply(c),
                // The replica stream (v4): not part of the view; the core hands it to the replica in order.
                GameMsg::GcBase(b) => self.replica.push(ReplicaMsg::Base(b)),
                GameMsg::GcJournal(j) => self.replica.push(ReplicaMsg::Journal(j)),
                GameMsg::GcTurn(t) => self.replica.push(ReplicaMsg::Turn(t)),
                // The answers to orders are taken by the orders module, after the view.
                GameMsg::Results(_) => {}
                // v13: the set of the game and the messages of its mods, for the mod layer, in the order they came.
                GameMsg::Mods(v) => self.mods.push(ClientEvent::ModSet(v)),
                GameMsg::Mod(m) if m.within_limits() => self.mods.push(ClientEvent::ModMsg(m)),
                GameMsg::Mod(m) => {
                    tracing::warn!(?m, "mod message over the payload limit, dropped");
                }
                // v14: the cosmetic terrain share of the mods, beside the view (never in it).
                GameMsg::TerrainShare(t) if t.within_limits() => self.terrain_in.push(t),
                GameMsg::TerrainShare(t) => {
                    tracing::warn!(
                        mod_idx = t.mod_idx,
                        plots = t.plots.len(),
                        "terrain share over the limit, dropped"
                    );
                }
            }
        }
        if staged.is_some() {
            tracing::warn!("MapBasis without a Keyframe in its batch, dropped");
        }
        if fx.iter().any(|e| matches!(e, Effect::Break(_))) {
            self.replica.clear();
            self.mods.clear();
            self.terrain_in.clear();
            return fx;
        }
        for t in std::mem::take(&mut self.terrain_in) {
            self.terrain.apply(t);
        }
        self.revision += 1;
        fx
    }

    fn stage_basis(&mut self, m: MapBasis, cx: &mut Cx<'_>) -> Result<BasisState, ()> {
        cx.stats.basis_msgs += 1;
        cx.stats.basis_bytes += basis_bytes(&m);
        if m.game != cx.game {
            return Err(());
        }
        BasisState::new(m).map_err(|e| {
            tracing::warn!(%e, "map basis refused");
        })
    }

    fn keyframe(
        &mut self,
        seq: u64,
        k: Keyframe,
        staged: Option<BasisState>,
        cx: &mut Cx<'_>,
        fx: &mut Vec<Effect>,
    ) {
        if check(&k.view).is_err() {
            return self.keyframe_failed(KeyframeReason::ApplyError, cx, fx);
        }
        let hash = ViewHash::of(&k.view);
        if hash.value() != k.hash {
            return self.keyframe_failed(KeyframeReason::HashMismatch, cx, fx);
        }
        // The pair: a basis of this game whose hash is the one the frame names, and the same map header.
        let basis = staged.as_ref().or(self.basis.as_ref());
        let fits = basis.is_some_and(|b| {
            b.hash() == k.basis_hash
                && b.map().game == cx.game
                && check_pair(&k.view, b.map()).is_ok()
        });
        if !fits {
            return self.keyframe_failed(KeyframeReason::BasisMismatch, cx, fx);
        }
        let new_basis = staged.is_some();
        if let Some(b) = staged {
            self.basis = Some(b);
            self.change.basis_all();
        }
        self.base = Some(Base { view: k.view, hash });
        self.view_seq = seq;
        self.waiting = None;
        // The contexts of the old view are no use to the new one; those of this batch follow the keyframe.
        self.contexts.clear();
        self.change.all();
        cx.stats.keyframes += 1;
        if new_basis {
            fx.push(Effect::Basis(self.claim()));
        }
    }

    fn keyframe_failed(&mut self, reason: KeyframeReason, cx: &mut Cx<'_>, fx: &mut Vec<Effect>) {
        if self.waiting.is_some() {
            // The answer to our own request is bad too: the stream is given up.
            fx.push(Effect::Break(BreakWhy::KeyframeFailed));
        } else {
            fx.extend(self.request(cx.now, reason, cx));
        }
    }

    fn diff(&mut self, seq: u64, d: ViewDelta, cx: &mut Cx<'_>, fx: &mut Vec<Effect>) {
        if self.waiting.is_some() {
            cx.stats.diffs_skipped += 1;
            return;
        }
        let (Some(base), Some(basis)) = (self.base.as_mut(), self.basis.as_mut()) else {
            // A diff with nothing to apply it to.
            fx.extend(self.request(cx.now, KeyframeReason::BaseMismatch, cx));
            return;
        };
        if d.base != self.view_seq {
            fx.extend(self.request(cx.now, KeyframeReason::BaseMismatch, cx));
            return;
        }
        let prev_basis_hash = basis.hash();
        let undo = UndoLog::capture(&base.view, &base.hash, basis, &d.diff);
        if apply(&mut base.view, &mut base.hash, basis, &d.diff).is_err() {
            // `apply` checks before it changes anything.
            fx.extend(self.request(cx.now, KeyframeReason::ApplyError, cx));
            return;
        }
        let want_basis = d.basis_hash.unwrap_or(prev_basis_hash);
        let reason = if base.hash.value() != d.hash {
            Some(KeyframeReason::HashMismatch)
        } else if basis.hash() != want_basis || d.basis_hash.is_some() == d.diff.basis.is_empty() {
            Some(KeyframeReason::BasisMismatch)
        } else {
            None
        };
        if let Some(reason) = reason {
            // The previous view is shown until the keyframe comes (NP 4.6).
            undo.restore(&mut base.view, &mut base.hash, basis);
            cx.stats.undo_restores += 1;
            fx.extend(self.request(cx.now, reason, cx));
            return;
        }
        self.view_seq = seq;
        self.change.diff(&d.diff);
        cx.stats.diffs += 1;
        if !d.diff.basis.is_empty() {
            fx.push(Effect::Basis(self.claim()));
        }
    }
}

fn trim<T>(list: &mut Vec<T>, cap: usize) {
    if list.len() > cap {
        list.drain(..list.len() - cap);
    }
}

fn count_need(stats: &mut ClientStats, reason: KeyframeReason) {
    let n = &mut stats.need_keyframe;
    match reason {
        KeyframeReason::HashMismatch => n.hash_mismatch += 1,
        KeyframeReason::BaseMismatch => n.base_mismatch += 1,
        KeyframeReason::ApplyError => n.apply_error += 1,
        KeyframeReason::Decode => n.decode += 1,
        KeyframeReason::BasisMismatch => n.basis_mismatch += 1,
    }
}

/// Size of a `MapBasis` body for the counters; the postcard length, as the server sends it.
fn basis_bytes(m: &MapBasis) -> u64 {
    use protocol::{Codec, Postcard};
    Postcard.encode(m).map_or(0, |b| b.len() as u64)
}
