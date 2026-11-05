//! Order numbers of one seat (net-protocol 4.8): duplicates, the answer
//! ring, the window and the pending set. Pure bookkeeping; the actor owns one [`OrderBook`] per
//! seat and decides what to send.
//!
//! Every `id` gets exactly one final answer (`Applied`, `Rejected`, `Lost`); `Queued` is not final.

use std::collections::{BTreeSet, VecDeque};

use protocol::{CmdId, CommandOutcome, CommandResult};

/// Answers kept for repeats (NP-D16).
pub const ANSWER_RING: usize = 64;
/// Orders the gateway holds for one seat at most (GH-D7: the seat's queue is 64; the client itself keeps
/// no more than 32 on the way, NP-D11).
pub const WINDOW: usize = 64;

/// What to do with an incoming `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A new number: it may go on.
    New {
        /// The number skipped numbers (`id_gap`).
        gap: bool,
    },
    /// Seen before and answered: repeat the answer.
    Repeat(CommandResult),
    /// Seen before, still with the worker: say nothing, the answer will come.
    Pending,
    /// Older than the ring remembers.
    Expired,
}

#[derive(Debug, Default)]
pub struct OrderBook {
    /// The last number taken; 0 before the first.
    last_id: CmdId,
    answers: VecDeque<CommandResult>,
    pending: BTreeSet<CmdId>,
}

impl OrderBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Orders continue from here (`Attached.last_cmd_id`).
    pub fn last_id(&self) -> CmdId {
        self.last_id
    }

    /// Orders sent to the worker without a final answer.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// The window is full: another order is refused with `TooMany`.
    pub fn window_full(&self) -> bool {
        self.pending.len() >= WINDOW
    }

    /// Classifies `id`. A new number is taken (`last_id` moves) and must get an answer through
    /// [`Self::sent`] or [`Self::finish`].
    pub fn check(&mut self, id: CmdId) -> Verdict {
        if id <= self.last_id {
            if let Some(r) = self.answers.iter().find(|r| r.id == id) {
                return Verdict::Repeat(r.clone());
            }
            if self.pending.contains(&id) {
                return Verdict::Pending;
            }
            return Verdict::Expired;
        }
        let gap = id > self.last_id + 1;
        self.last_id = id;
        Verdict::New { gap }
    }

    /// The order went to the worker.
    pub fn sent(&mut self, id: CmdId) {
        self.pending.insert(id);
    }

    /// The worker's answer or the gateway's own refusal. Returns `true` if it was final (and is
    /// now in the ring); a `Queued` leaves the order pending.
    pub fn finish(&mut self, r: &CommandResult) -> bool {
        if r.outcome == CommandOutcome::Queued {
            return false;
        }
        self.pending.remove(&r.id);
        self.answers.retain(|a| a.id != r.id);
        self.answers.push_back(r.clone());
        while self.answers.len() > ANSWER_RING {
            self.answers.pop_front();
        }
        true
    }

    /// The worker is gone: everything pending is lost. Returns the numbers, oldest first, and
    /// records `Lost` as their final answer.
    pub fn lose_pending(&mut self) -> Vec<CmdId> {
        let ids: Vec<CmdId> = std::mem::take(&mut self.pending).into_iter().collect();
        for id in &ids {
            self.answers.retain(|a| a.id != *id);
            self.answers.push_back(CommandResult {
                id: *id,
                outcome: CommandOutcome::Lost,
            });
        }
        while self.answers.len() > ANSWER_RING {
            self.answers.pop_front();
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::RejectCode;

    fn applied(id: CmdId) -> CommandResult {
        CommandResult {
            id,
            outcome: CommandOutcome::Applied,
        }
    }

    #[test]
    fn numbers_grow_and_gaps_are_noted() {
        let mut b = OrderBook::new();
        assert_eq!(b.check(1), Verdict::New { gap: false });
        assert_eq!(b.check(2), Verdict::New { gap: false });
        assert_eq!(b.check(5), Verdict::New { gap: true });
        assert_eq!(b.last_id(), 5);
    }

    #[test]
    fn a_repeat_gets_the_same_answer_or_silence_while_pending() {
        let mut b = OrderBook::new();
        assert_eq!(b.check(1), Verdict::New { gap: false });
        b.sent(1);
        assert_eq!(b.check(1), Verdict::Pending);
        assert!(b.finish(&applied(1)));
        assert_eq!(b.check(1), Verdict::Repeat(applied(1)));
        assert_eq!(b.pending_count(), 0);
    }

    #[test]
    fn queued_is_not_final() {
        let mut b = OrderBook::new();
        b.check(1);
        b.sent(1);
        let queued = CommandResult {
            id: 1,
            outcome: CommandOutcome::Queued,
        };
        assert!(!b.finish(&queued));
        assert_eq!(b.check(1), Verdict::Pending);
    }

    #[test]
    fn an_id_older_than_the_ring_expires() {
        let mut b = OrderBook::new();
        for id in 1..=70 {
            b.check(id);
            b.sent(id);
            b.finish(&applied(id));
        }
        assert_eq!(b.check(70), Verdict::Repeat(applied(70)));
        // The ring holds the last 64: 7..=70.
        assert_eq!(b.check(6), Verdict::Expired);
        assert!(matches!(b.check(7), Verdict::Repeat(_)));
    }

    #[test]
    fn the_window_is_64() {
        let mut b = OrderBook::new();
        for id in 1..=63 {
            b.check(id);
            b.sent(id);
        }
        assert!(!b.window_full());
        b.check(64);
        b.sent(64);
        assert!(b.window_full());
        b.finish(&applied(1));
        assert!(!b.window_full());
    }

    #[test]
    fn a_refusal_is_a_final_answer() {
        let mut b = OrderBook::new();
        b.check(1);
        let r = CommandResult {
            id: 1,
            outcome: CommandOutcome::Rejected {
                code: RejectCode::TooMany,
                reason: String::new(),
            },
        };
        assert!(b.finish(&r));
        assert_eq!(b.check(1), Verdict::Repeat(r));
    }

    #[test]
    fn lost_orders_get_lost_as_their_final_answer() {
        let mut b = OrderBook::new();
        for id in 1..=3 {
            b.check(id);
            b.sent(id);
        }
        b.finish(&applied(2));
        assert_eq!(b.lose_pending(), vec![1, 3]);
        assert_eq!(
            b.check(1),
            Verdict::Repeat(CommandResult {
                id: 1,
                outcome: CommandOutcome::Lost
            })
        );
        assert_eq!(b.pending_count(), 0);
    }
}
