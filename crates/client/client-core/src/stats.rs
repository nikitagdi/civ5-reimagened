//! `ClientStats`: counters for the test port and `net-test` (net-client 4.12). NC1 started it with the
//! connection and lobby counters, NC2 adds the stream; NC3 and later cards add theirs.

use std::collections::BTreeMap;

/// `NeedKeyframe` sent, by reason (`KeyframeReason`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NeedKeyframeCounts {
    pub hash_mismatch: u64,
    pub base_mismatch: u64,
    pub apply_error: u64,
    pub decode: u64,
    pub basis_mismatch: u64,
}

impl NeedKeyframeCounts {
    pub fn total(&self) -> u64 {
        self.hash_mismatch
            + self.base_mismatch
            + self.apply_error
            + self.decode
            + self.basis_mismatch
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientStats {
    /// Bytes received and sent, as frames.
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// Frames from the server that did not decode.
    pub decode_errors: u64,
    /// Orders, lobby commands, chat and `GameCtl` refused for lack of a connection (NC-D5).
    pub refused_offline: u64,
    /// `HostChanged` events (NP-D25).
    pub host_changes: u64,
    /// Batches taken into the stream, and those dropped as duplicates.
    pub batches: u64,
    pub dup_batches: u64,
    pub keyframes: u64,
    pub diffs: u64,
    /// Diffs not applied because a keyframe was awaited.
    pub diffs_skipped: u64,
    /// Views put back by the undo log after a hash that did not match.
    pub undo_restores: u64,
    pub need_keyframe: NeedKeyframeCounts,
    /// `MapBasis` messages and the bytes of their bodies (AC-N3.5).
    pub basis_msgs: u64,
    pub basis_bytes: u64,
    /// `Attached` by mode: reconnects that continued the stream and those that started it over.
    pub attached_replay: u64,
    pub attached_fresh: u64,
    /// Broken streams (gap, unreadable batch, failed keyframe) and the reconnects they caused.
    pub stream_breaks: u64,
    /// Orders sent for the first time, and sent again after an `Attached` (same `id`, 4.5).
    /// Replica stream messages handed to the shell; `GcNet` frames and `GcResync` requests sent.
    pub replica_msgs: u64,
    pub gc_net_sent: u64,
    /// `GcNet` frames dropped because the replica's turn is behind the server's (an order for a turn that passed).
    pub gc_net_stale: u64,
    pub gc_resync_sent: u64,
    pub orders_sent: u64,
    pub orders_resent: u64,
    /// Final answers by kind. `orders_expired` is `Rejected { Expired }`, counted apart from `orders_rejected`.
    pub orders_applied: u64,
    pub orders_rejected: u64,
    pub orders_lost: u64,
    pub orders_expired: u64,
    /// `Queued` answers (not final).
    pub orders_queued: u64,
    /// A result for an `id` that already had its final answer.
    pub dup_results: u64,
    /// A result for an `id` the client never gave (an order of an earlier page).
    pub unknown_results: u64,
    /// Orders in flight dropped because the game ended or changed.
    pub orders_dropped: u64,
    /// Queries sent, by kind of `Query` (AC-N2.3: the count of `UnitActions`, `Reachable`, `Path` ...).
    pub queries: BTreeMap<&'static str, u64>,
    pub queries_answered: u64,
    /// Answers thrown away: unknown number, or another game or stream than the query was sent on.
    pub queries_dropped: u64,
    /// The context of the unit the player selected: fresh or stale at that moment, and stale when an order
    /// was given to a unit (the picture of that order is not drawn, NC-D6).
    pub ctx_fresh_at_select: u64,
    pub ctx_stale_at_select: u64,
    pub ctx_stale_at_order: u64,
    /// `Focus` messages sent.
    pub focus_sent: u64,
    /// Pictures of orders drawn by the layer; taken off by a failed order (with its line in the journal);
    /// taken off by the guard `order_stuck_ms`.
    pub layer_records: u64,
    pub layer_rollbacks: u64,
    pub layer_stuck: u64,
    /// `EndTurnCancelled { by_order: true }` given (NP-D23).
    pub end_turn_cancelled_by_order: u64,
    /// The newest round trips of `Ping` (ms, 64) and the times from an order's sending to its final answer
    /// (ms, 256): AC-N2.1. Oldest first.
    pub rtt_ring: Vec<u32>,
    pub order_ms_ring: Vec<u32>,
}

/// Capacity of `ClientStats::rtt_ring` and `order_ms_ring`.
pub const RTT_RING: usize = 64;
pub const ORDER_RING: usize = 256;

/// Adds `value` to a ring of at most `cap` numbers, the oldest falling out.
pub(crate) fn push_ring(ring: &mut Vec<u32>, cap: usize, value: u64) {
    if ring.len() >= cap {
        ring.remove(0);
    }
    ring.push(u32::try_from(value).unwrap_or(u32::MAX));
}
