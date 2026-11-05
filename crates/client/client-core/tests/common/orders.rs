//! The order side of the scripted gateway: numbers, a ring of 64 answers, repeats (net-protocol 4.8).
//!
//! A new `id` is passed to the worker: the double either answers it at once (`auto`) or holds it (the
//! worker is busy) until the test answers it. An `id` the gateway has seen gets the old answer again from the
//! ring, silence while the worker still has it, and `Rejected { Expired }` when it is older than the ring.
//! Every answer is a batch of the stream (it takes a `seq` and goes into the log a `Replay` serves).

#![allow(dead_code)]

use std::collections::{HashMap, VecDeque};

use protocol::{Cmd, CmdId, CommandOutcome, CommandResult, GameMsg, RejectCode, ServerMsg};

use super::world::World;

const RING: usize = 64;

pub struct OrderBook {
    /// Answer a new order `Applied` at once.
    pub auto: bool,
    /// Outcomes forced for particular numbers.
    pub forced: HashMap<CmdId, CommandOutcome>,
    /// Numbers the worker applied, in order: each exactly once, whatever the client repeats.
    pub applied: Vec<CmdId>,
    /// Every `Cmd` that came, repeats included.
    pub seen: Vec<Cmd>,
    /// The highest number accepted.
    pub last: CmdId,
    /// Orders with the worker, not answered finally.
    pub held: Vec<Cmd>,
    ring: VecDeque<CommandResult>,
}

impl OrderBook {
    pub fn new() -> Self {
        OrderBook {
            auto: true,
            forced: HashMap::new(),
            applied: Vec::new(),
            seen: Vec::new(),
            last: 0,
            held: Vec::new(),
            ring: VecDeque::new(),
        }
    }

    /// The gateway's reaction to a `Cmd`: the batches to send back (none, when it keeps silent).
    pub fn on_cmd(&mut self, c: Cmd, w: &mut World) -> Vec<ServerMsg> {
        self.seen.push(c);
        if c.id <= self.last {
            if let Some(r) = self.ring.iter().find(|r| r.id == c.id) {
                return vec![w.push(vec![GameMsg::Results(vec![r.clone()])])];
            }
            if self.held.iter().any(|h| h.id == c.id) {
                return vec![];
            }
            let r = CommandResult {
                id: c.id,
                outcome: CommandOutcome::Rejected {
                    code: RejectCode::Expired,
                    reason: String::new(),
                },
            };
            return vec![w.push(vec![GameMsg::Results(vec![r])])];
        }
        self.last = c.id;
        if let Some(o) = self.forced.remove(&c.id) {
            return self.settle(c.id, o, w);
        }
        if self.auto {
            return self.settle(c.id, CommandOutcome::Applied, w);
        }
        self.held.push(c);
        vec![]
    }

    /// The worker answers an order it held (or a `Queued` notice, which keeps it held).
    pub fn answer(&mut self, id: CmdId, outcome: CommandOutcome, w: &mut World) -> Vec<ServerMsg> {
        if outcome == CommandOutcome::Queued {
            let r = CommandResult { id, outcome };
            return vec![w.push(vec![GameMsg::Results(vec![r])])];
        }
        self.held.retain(|h| h.id != id);
        self.settle(id, outcome, w)
    }

    fn settle(&mut self, id: CmdId, outcome: CommandOutcome, w: &mut World) -> Vec<ServerMsg> {
        if outcome == CommandOutcome::Applied {
            self.applied.push(id);
        }
        let r = CommandResult { id, outcome };
        self.ring.push_back(r.clone());
        if self.ring.len() > RING {
            self.ring.pop_front();
        }
        vec![w.push(vec![GameMsg::Results(vec![r])])]
    }
}
