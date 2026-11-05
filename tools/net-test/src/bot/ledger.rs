//! The bot's own books of its orders (NT-D15): `id` to what became of it. The gateway counts what the worker
//! got; the bot counts what really came back, after materialisation and reconnects: two independent sources.
//!
//! Invariants: every `id` ends with exactly one final outcome, `Applied`, `Rejected` or
//! `Lost`; `Queued` is not final; after a reconnect the unanswered orders go again with the same `id`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use protocol::{Cmd, CmdId, CommandOutcome, RejectCode};

/// The final answer to an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied,
    Rejected(RejectCode),
    Lost,
}

impl Outcome {
    fn of(o: &CommandOutcome) -> Option<Outcome> {
        match o {
            CommandOutcome::Applied => Some(Outcome::Applied),
            CommandOutcome::Rejected { code, .. } => Some(Outcome::Rejected(*code)),
            CommandOutcome::Lost => Some(Outcome::Lost),
            CommandOutcome::Queued => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub cmd: Cmd,
    /// `UnitSleep`, `EndTurn`, ...
    pub kind: String,
    pub first_sent: Instant,
    /// When the frame went out last (the latency is counted from it).
    pub last_sent: Instant,
    /// Frames sent after the first.
    pub resends: u32,
    pub queued_at: Option<Instant>,
    pub outcome: Option<Outcome>,
    pub outcome_at: Option<Instant>,
}

/// What an answer from the gateway did to the books.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// The first final answer; the time from the last send to the read of the batch.
    Final(Duration, Outcome),
    Queued,
    /// The same final answer again after a resend: the gateway answers repeats, not a double outcome.
    Repeat,
    /// A second, different final answer, or a second one without a resend: a bug somewhere.
    Double,
    /// An answer to an `id` the bot never sent.
    Unknown,
}

/// The `cmds` and `ledger` objects of `result.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    /// Frames put on the wire (the first of every id, resends, duplicates).
    pub sent: u64,
    pub unique: u64,
    pub applied: u64,
    pub rejected: BTreeMap<String, u64>,
    pub lost: u64,
    pub queued: u64,
    pub dup_sent: u64,
    pub open: u64,
    pub double_outcome: u64,
    pub repeated_answers: u64,
    pub unknown_answers: u64,
}

#[derive(Debug, Default)]
pub struct Ledger {
    entries: BTreeMap<CmdId, Entry>,
    frames_sent: u64,
    dup_sent: u64,
    double_outcome: u64,
    repeated_answers: u64,
    unknown_answers: u64,
}

/// The name of an order for the books: the variant name.
pub fn kind_of(cmd: &protocol::Command) -> String {
    let text = format!("{cmd:?}");
    text.split([' ', '{']).next().unwrap_or("?").to_owned()
}

impl Ledger {
    /// A frame with this order goes out. The first time it opens the entry; later (a resend after a
    /// reconnect, or a deliberate duplicate with `dup`) it only counts.
    pub fn sent(&mut self, cmd: &Cmd, now: Instant, dup: bool) {
        self.frames_sent += 1;
        match self.entries.get_mut(&cmd.id) {
            Some(e) => {
                e.resends += 1;
                e.last_sent = now;
                if dup {
                    self.dup_sent += 1;
                }
            }
            None => {
                self.entries.insert(
                    cmd.id,
                    Entry {
                        cmd: *cmd,
                        kind: kind_of(&cmd.cmd),
                        first_sent: now,
                        last_sent: now,
                        resends: 0,
                        queued_at: None,
                        outcome: None,
                        outcome_at: None,
                    },
                );
            }
        }
    }

    /// An answer came in a batch read at `now`.
    pub fn answer(&mut self, id: CmdId, outcome: &CommandOutcome, now: Instant) -> Answer {
        let Some(e) = self.entries.get_mut(&id) else {
            self.unknown_answers += 1;
            return Answer::Unknown;
        };
        let Some(fin) = Outcome::of(outcome) else {
            e.queued_at.get_or_insert(now);
            return Answer::Queued;
        };
        match &e.outcome {
            None => {
                e.outcome = Some(fin.clone());
                e.outcome_at = Some(now);
                Answer::Final(now.saturating_duration_since(e.last_sent), fin)
            }
            Some(first) if *first == fin && e.resends > 0 => {
                self.repeated_answers += 1;
                Answer::Repeat
            }
            Some(_) => {
                self.double_outcome += 1;
                Answer::Double
            }
        }
    }

    /// Orders with no final outcome yet.
    pub fn open(&self) -> usize {
        self.entries
            .values()
            .filter(|e| e.outcome.is_none())
            .count()
    }

    /// The orders to send again after a reconnect, in `id` order.
    pub fn unanswered(&self) -> Vec<Cmd> {
        self.entries
            .values()
            .filter(|e| e.outcome.is_none())
            .map(|e| e.cmd)
            .collect()
    }

    pub fn entry(&self, id: CmdId) -> Option<&Entry> {
        self.entries.get(&id)
    }

    pub fn outcome(&self, id: CmdId) -> Option<&Outcome> {
        self.entries.get(&id).and_then(|e| e.outcome.as_ref())
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts {
            sent: self.frames_sent,
            unique: self.entries.len() as u64,
            dup_sent: self.dup_sent,
            double_outcome: self.double_outcome,
            repeated_answers: self.repeated_answers,
            unknown_answers: self.unknown_answers,
            ..Counts::default()
        };
        for e in self.entries.values() {
            if e.queued_at.is_some() {
                c.queued += 1;
            }
            match &e.outcome {
                None => c.open += 1,
                Some(Outcome::Applied) => c.applied += 1,
                Some(Outcome::Lost) => c.lost += 1,
                Some(Outcome::Rejected(code)) => {
                    *c.rejected.entry(format!("{code:?}")).or_insert(0) += 1;
                }
            }
        }
        c
    }
}
