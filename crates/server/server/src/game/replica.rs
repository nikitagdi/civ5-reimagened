//! The replica stream at the gateway: the bodies of the snapshots and patches the worker
//! packed ([`ReplicaBlobs`], served at `GET /replica/{hash:016x}`), what the game actor remembers of the chain
//! ([`ReplicaChain`]: epoch, the last batch number, which bodies are held) and its counters ([`ReplicaStats`]).
//!
//! The messages themselves (`GcBase`, `GcJournal`, `GcTurn`) go through the seat pipelines like any part of a
//! batch (`stream.rs`); each seat stream keeps the base and the batches after it for a `Fresh` attach.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Bytes;
use protocol::{GameId, GcResyncReason, PlayerId, xxh64};

/// Announcements (a base, a turn, a resync) whose bodies are kept: the current turn, the previous one (a client
/// that is a turn behind still fetches its patch) and a few resyncs.
pub const GROUPS_KEPT: usize = 6;

/// A body in the store.
#[derive(Debug)]
struct Blob {
    game: GameId,
    body: Bytes,
}

/// The bodies of all games, by the xxh64 of the body. Immutable: a hash names one body.
#[derive(Debug, Default)]
pub struct ReplicaBlobs {
    map: Mutex<HashMap<u64, Blob>>,
    bytes: AtomicU64,
}

impl ReplicaBlobs {
    /// Keeps `body` under `hash`. `false`: the hash is not the body's (the worker is wrong; nothing is kept).
    pub fn put(&self, game: GameId, hash: u64, body: Vec<u8>) -> bool {
        if xxh64(0, &body) != hash {
            return false;
        }
        let len = body.len() as u64;
        let old = self
            .map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                hash,
                Blob {
                    game,
                    body: Bytes::from(body),
                },
            );
        self.bytes.fetch_add(len, Ordering::Relaxed);
        if let Some(o) = old {
            self.bytes.fetch_sub(o.body.len() as u64, Ordering::Relaxed);
        }
        true
    }

    /// The body, if it is held.
    pub fn get(&self, hash: u64) -> Option<Bytes> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&hash)
            .map(|b| b.body.clone())
    }

    /// The game a body belongs to.
    pub fn game_of(&self, hash: u64) -> Option<GameId> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&hash)
            .map(|b| b.game)
    }

    /// Drops the body of `game` under `hash`.
    pub fn remove(&self, game: GameId, hash: u64) {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        if map.get(&hash).is_some_and(|b| b.game == game)
            && let Some(b) = map.remove(&hash)
        {
            self.bytes.fetch_sub(b.body.len() as u64, Ordering::Relaxed);
        }
    }

    /// Bytes held, all games.
    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// Bodies held, all games.
    pub fn len(&self) -> usize {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Counters of the replica stream of one game (`games.<id>.replica`).
#[derive(Debug, Default)]
pub struct ReplicaStats {
    /// Journal batches taken from the worker and their bytes.
    pub batches: AtomicU64,
    pub batch_bytes: AtomicU64,
    /// Batches of an epoch or a number that does not fit the chain (dropped).
    pub batches_dropped: AtomicU64,
    /// Bases for everybody, turns, turns that carry a patch.
    pub bases: AtomicU64,
    pub turns: AtomicU64,
    pub turns_with_patch: AtomicU64,
    /// Bases for one seat (answers to `GcResync`).
    pub resync_bases: AtomicU64,
    /// Sizes of the last full body and the last patch, bytes.
    pub last_snapshot_bytes: AtomicU64,
    pub last_patch_bytes: AtomicU64,
    /// Bodies stored and rejected for a wrong hash.
    pub blobs: AtomicU64,
    pub blobs_bad: AtomicU64,
    /// `GcResync` by reason (`Diverged`, `Rejected`, `BadBlob`, `Gap`), and those dropped by the rate rule.
    pub resync: [AtomicU64; 4],
    pub resync_dropped: AtomicU64,
    /// `GcNet` messages taken, passed to the worker, dropped (too big, over the rate), and refused by the core
    /// (`NetRejected`: the client counts them as `replica.net_rejected`).
    pub net_in: AtomicU64,
    pub net_bytes: AtomicU64,
    pub net_dropped: AtomicU64,
    pub net_rejected: AtomicU64,
}

/// Names of [`ReplicaStats::resync`], in the order of [`resync_index`].
pub const RESYNC_NAMES: [&str; 4] = ["diverged", "rejected", "bad_blob", "gap"];

pub fn resync_index(r: GcResyncReason) -> usize {
    match r {
        GcResyncReason::Diverged => 0,
        GcResyncReason::Rejected => 1,
        GcResyncReason::BadBlob => 2,
        GcResyncReason::Gap => 3,
    }
}

/// What the actor knows of the chain of its game.
#[derive(Debug)]
pub struct ReplicaChain {
    game: GameId,
    store: Arc<ReplicaBlobs>,
    /// The epoch of the worker now (0: no base yet).
    epoch: u32,
    /// Number of the last batch the worker sent in this epoch.
    jseq: u64,
    /// Bodies that came since the last announcement.
    pending: Vec<u64>,
    /// The bodies of the last announcements, oldest first.
    groups: VecDeque<Vec<u64>>,
    /// When the last resync of a seat was served, ms: a seat gets one in [`MIN_RESYNC_GAP_MS`].
    last_resync: BTreeMap<PlayerId, u64>,
    /// Seats whose `GcResync` came inside the gap and was not served: it is served on a tick once the gap is over
    /// (the client does not ask again for seconds, so a plain drop would freeze its replica).
    deferred: std::collections::BTreeSet<PlayerId>,
}

/// A seat that asks again within this time is not answered again (the snapshot costs the worker 0.2-0.3 s).
pub const MIN_RESYNC_GAP_MS: u64 = 2_000;

impl ReplicaChain {
    pub fn new(game: GameId, store: Arc<ReplicaBlobs>) -> Self {
        Self {
            game,
            store,
            epoch: 0,
            jseq: 0,
            pending: Vec::new(),
            groups: VecDeque::new(),
            last_resync: BTreeMap::new(),
            deferred: std::collections::BTreeSet::new(),
        }
    }

    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// `ReplicaBlob`: stored; `false`: the hash was wrong.
    pub fn blob(&mut self, hash: u64, bytes: Vec<u8>) -> bool {
        let ok = self.store.put(self.game, hash, bytes);
        if ok {
            self.pending.push(hash);
        }
        ok
    }

    /// An announcement (`ReplicaBase`, `ReplicaTurn`) closes the group of bodies that came before it; the oldest
    /// groups go.
    fn announce(&mut self) {
        self.groups.push_back(std::mem::take(&mut self.pending));
        while self.groups.len() > GROUPS_KEPT {
            if let Some(old) = self.groups.pop_front() {
                for h in old {
                    // A body that a later group holds too (the same bytes) stays.
                    if !self.groups.iter().any(|g| g.contains(&h)) {
                        self.store.remove(self.game, h);
                    }
                }
            }
        }
    }

    /// `ReplicaBase` for everybody: a new chain (`epoch` may be new). For one seat (`for_player`): the chain
    /// goes on.
    pub fn base(&mut self, epoch: u32, jseq: u64, for_player: bool) {
        if !for_player {
            self.epoch = epoch;
            self.jseq = jseq;
        }
        self.announce();
    }

    /// `ReplicaTurn`: the chain goes on from `jseq` in the same epoch (or a new one).
    pub fn turn(&mut self, epoch: u32, jseq: u64) {
        self.epoch = epoch;
        self.jseq = jseq;
        self.announce();
    }

    /// `ReplicaJournal`: `true` when the batches belong to the chain and follow the last one.
    pub fn journal(&mut self, epoch: u32, first_jseq: u64, n: usize) -> bool {
        if epoch != self.epoch || first_jseq != self.jseq + 1 {
            return false;
        }
        self.jseq += n as u64;
        true
    }

    /// May a seat's `GcResync` go to the worker now.
    pub fn resync_allowed(&mut self, player: PlayerId, now_ms: u64) -> bool {
        match self.last_resync.get(&player) {
            Some(&t) if now_ms.saturating_sub(t) < MIN_RESYNC_GAP_MS => false,
            _ => {
                self.last_resync.insert(player, now_ms);
                true
            }
        }
    }

    /// A request that came inside the gap: served later by [`take_due`](Self::take_due).
    pub fn defer(&mut self, player: PlayerId) {
        self.deferred.insert(player);
    }

    /// The seats whose deferred request may go now (their gap is over); they are stamped like a served request.
    pub fn take_due(&mut self, now_ms: u64) -> Vec<PlayerId> {
        let due: Vec<PlayerId> = self.deferred.iter().copied().collect();
        let mut out = Vec::new();
        for p in due {
            if self.resync_allowed(p, now_ms) {
                self.deferred.remove(&p);
                out.push(p);
            }
        }
        out
    }

    /// The request could not be handed to the worker: the stamp is undone so that the next one is not refused.
    pub fn unstamp(&mut self, player: PlayerId) {
        self.last_resync.remove(&player);
    }

    /// A seat that left has no deferred request.
    pub fn forget_deferred(&mut self, player: PlayerId) {
        self.deferred.remove(&player);
    }

    /// The worker is gone: the chain is over; the next base starts a new one.
    pub fn reset(&mut self) {
        self.epoch = 0;
        self.jseq = 0;
        self.deferred.clear();
        self.last_resync.clear();
    }
}

impl Drop for ReplicaChain {
    /// The game ended: its bodies go.
    fn drop(&mut self) {
        for h in self.pending.drain(..) {
            self.store.remove(self.game, h);
        }
        for g in self.groups.drain(..) {
            for h in g {
                self.store.remove(self.game, h);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(chain: &mut ReplicaChain, body: &[u8]) -> u64 {
        let h = xxh64(0, body);
        assert!(chain.blob(h, body.to_vec()));
        h
    }

    #[test]
    fn a_wrong_hash_is_not_stored() {
        let store = Arc::new(ReplicaBlobs::default());
        let mut c = ReplicaChain::new(1, Arc::clone(&store));
        assert!(!c.blob(5, b"abc".to_vec()));
        assert!(store.is_empty());
    }

    #[test]
    fn old_groups_go_and_a_game_takes_its_bodies_with_it() {
        let store = Arc::new(ReplicaBlobs::default());
        let mut c = ReplicaChain::new(1, Arc::clone(&store));
        let mut hashes = Vec::new();
        for i in 0..8u8 {
            hashes.push(put(&mut c, &[i; 10]));
            c.turn(1, u64::from(i));
        }
        assert_eq!(store.len(), GROUPS_KEPT);
        assert!(store.get(hashes[0]).is_none());
        assert!(store.get(hashes[7]).is_some());
        assert_eq!(store.bytes(), 10 * GROUPS_KEPT as u64);
        drop(c);
        assert!(store.is_empty());
        assert_eq!(store.bytes(), 0);
    }

    #[test]
    fn the_chain_follows_the_batch_numbers_of_its_epoch() {
        let store = Arc::new(ReplicaBlobs::default());
        let mut c = ReplicaChain::new(1, store);
        assert!(!c.journal(7, 1, 1), "no base yet");
        c.base(7, 0, false);
        assert!(c.journal(7, 1, 2));
        assert!(!c.journal(7, 2, 1), "a repeat");
        assert!(!c.journal(8, 3, 1), "another epoch");
        assert!(c.journal(7, 3, 1));
        // A base for one seat does not restart the chain.
        c.base(7, 3, true);
        assert!(c.journal(7, 4, 1));
        c.turn(7, 4);
        assert!(c.journal(7, 5, 1));
    }

    #[test]
    fn a_seat_is_not_resynced_twice_in_a_row() {
        let mut c = ReplicaChain::new(1, Arc::new(ReplicaBlobs::default()));
        assert!(c.resync_allowed(0, 1_000));
        assert!(!c.resync_allowed(0, 1_500));
        assert!(c.resync_allowed(1, 1_500));
        assert!(c.resync_allowed(0, 1_000 + MIN_RESYNC_GAP_MS));
    }

    #[test]
    fn a_request_inside_the_gap_is_served_when_the_gap_is_over() {
        let mut c = ReplicaChain::new(1, Arc::new(ReplicaBlobs::default()));
        assert!(c.resync_allowed(0, 1_000));
        assert!(!c.resync_allowed(0, 1_500));
        c.defer(0);
        assert!(c.take_due(1_600).is_empty());
        assert_eq!(c.take_due(1_000 + MIN_RESYNC_GAP_MS), vec![0]);
        assert!(c.take_due(1_000 + 2 * MIN_RESYNC_GAP_MS).is_empty());
    }

    #[test]
    fn an_unsent_request_does_not_block_the_next_one() {
        let mut c = ReplicaChain::new(1, Arc::new(ReplicaBlobs::default()));
        assert!(c.resync_allowed(0, 1_000));
        c.unstamp(0);
        assert!(c.resync_allowed(0, 1_100));
    }
}
