//! Orders and queries (net-client 4.5, NC-D5): the numbers, the list of orders in flight, the answers.
//!
//! Pure state: `ClientCore` decides whether an order may go out (no connection, no game), asks this module
//! for the bookkeeping and sends the frames. There is no queue: an order the client took is either sent or
//! refused on the spot. What waits here has been sent and waits for its one final answer; after every
//! `Attached` it is sent again with the same `id` and `turn`, and the gateway answers each `id` once.

use std::collections::VecDeque;

use protocol::{Cmd, CmdId, Command, CommandOutcome, CommandResult, GameId, Query, RejectCode};

use crate::ClientStats;

/// Final answers remembered to tell a repeated one from an unknown one; the gateway keeps 64 (NP 4.8).
const DONE_RING: usize = 64;
/// Queries awaiting an answer; older ones are forgotten (their answers are thrown away).
const QUERIES_CAP: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Sent,
    /// `Queued` came: the core is busy, the final answer follows.
    Queued,
}

/// An order sent and not answered finally.
#[derive(Debug, Clone, Copy)]
struct Pending {
    id: CmdId,
    cmd: Command,
    turn: i32,
    sent_ms: u64,
    #[allow(dead_code)] // a queued order keeps its record; no picture is made in the rivals' turn
    state: State,
}

/// What an answer meant for the client.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settled {
    pub id: CmdId,
    /// The order, to find its unit or city for the journal line.
    pub cmd: Command,
    pub outcome: CommandOutcome,
    /// When the order was first sent (the client's clock).
    pub sent_ms: u64,
    /// Text for the player when the order did not (or maybe did not) go through: the line of the journal and
    /// of `OrderRolledBack`. `None` for `Applied`.
    pub text: Option<String>,
}

struct PendingQuery {
    id: u32,
    game: GameId,
    stream: u64,
}

pub(crate) struct Orders {
    /// By `id`, ascending.
    inflight: Vec<Pending>,
    /// Ids that had a final answer, newest last.
    done: VecDeque<CmdId>,
    queries: VecDeque<PendingQuery>,
    next_query: u32,
}

impl Orders {
    pub fn new() -> Self {
        Orders {
            inflight: Vec::new(),
            done: VecDeque::new(),
            queries: VecDeque::new(),
            next_query: 1,
        }
    }

    pub fn len(&self) -> usize {
        self.inflight.len()
    }

    /// The order was sent (the caller checked the limit).
    pub fn add(&mut self, id: CmdId, cmd: Command, turn: i32, now: u64) {
        self.inflight.push(Pending {
            id,
            cmd,
            turn,
            sent_ms: now,
            state: State::Sent,
        });
    }

    /// Everything without a final answer, in the order of the numbers, as it was sent: after an `Attached`
    /// all of it goes again (4.5). `Queued` ones too: the gateway keeps silent about an order that is still
    /// with the worker.
    pub fn unanswered(&self) -> Vec<Cmd> {
        self.inflight
            .iter()
            .map(|p| Cmd {
                id: p.id,
                turn: p.turn,
                cmd: p.cmd,
            })
            .collect()
    }

    /// The game ended or changed: the orders of the old one mean nothing. Returns how many there were.
    pub fn clear(&mut self) -> usize {
        let n = self.inflight.len();
        self.inflight.clear();
        self.done.clear();
        self.queries.clear();
        n
    }

    /// An answer of the stream. `Queued` marks the order and settles nothing; a final answer removes it.
    /// A repeated final answer and an unknown `id` are ignored and counted.
    pub fn answer(&mut self, r: CommandResult, stats: &mut ClientStats) -> Option<Settled> {
        let Some(at) = self.inflight.iter().position(|p| p.id == r.id) else {
            if self.done.contains(&r.id) {
                stats.dup_results += 1;
            } else {
                stats.unknown_results += 1;
            }
            return None;
        };
        if r.outcome == CommandOutcome::Queued {
            stats.orders_queued += 1;
            self.inflight[at].state = State::Queued;
            return None;
        }
        let sent = self.inflight.remove(at);
        let (cmd, sent_ms) = (sent.cmd, sent.sent_ms);
        self.done.push_back(r.id);
        if self.done.len() > DONE_RING {
            self.done.pop_front();
        }
        let text = rollback_text(&r.outcome);
        match &r.outcome {
            CommandOutcome::Applied => stats.orders_applied += 1,
            CommandOutcome::Lost => stats.orders_lost += 1,
            CommandOutcome::Rejected {
                code: RejectCode::Expired,
                ..
            } => stats.orders_expired += 1,
            CommandOutcome::Rejected { .. } => stats.orders_rejected += 1,
            CommandOutcome::Queued => {}
        }
        Some(Settled {
            id: r.id,
            cmd,
            outcome: r.outcome,
            sent_ms,
            text,
        })
    }

    // ------------------------------------------------------------ queries

    /// Remembers a query sent on this game and stream; returns its number (never 0).
    pub fn new_query(&mut self, game: GameId, stream: u64) -> u32 {
        let id = self.next_query;
        self.next_query = self.next_query.checked_add(1).unwrap_or(1);
        self.queries.push_back(PendingQuery { id, game, stream });
        if self.queries.len() > QUERIES_CAP {
            self.queries.pop_front();
        }
        id
    }

    /// The answer to a query counts only on the game and stream it was asked on, once.
    pub fn take_query(&mut self, id: u32, game: GameId, stream: u64) -> bool {
        let Some(at) = self.queries.iter().position(|q| q.id == id) else {
            return false;
        };
        let q = self.queries.remove(at);
        q.is_some_and(|q| q.game == game && q.stream == stream)
    }

    /// Queries sent before a new `Attached` or a `Detached` die with their socket.
    pub fn clear_queries(&mut self) {
        self.queries.clear();
    }
}

/// The name a query is counted under in `ClientStats::queries`.
pub(crate) fn query_kind(q: &Query) -> &'static str {
    match q {
        Query::UnitActions { .. } => "UnitActions",
        Query::CityBuilds { .. } => "CityBuilds",
        Query::Path { .. } => "Path",
        Query::Reachable { .. } => "Reachable",
        Query::RangeTargets { .. } => "RangeTargets",
        Query::Units => "Units",
        Query::Cities => "Cities",
        Query::TopBar => "TopBar",
        Query::Notifications => "Notifications",
        Query::Reach { .. } => "Reach",
        Query::CombatPreview { .. } => "CombatPreview",
    }
}

/// The line for the player when an order failed (4.4, "Тексты отката"). `None` for `Applied` and `Queued`.
/// `Expired` and `Lost` are told plainly: after a reconnect the result of an expired order is unknown.
pub(crate) fn rollback_text(outcome: &CommandOutcome) -> Option<String> {
    let text = match outcome {
        CommandOutcome::Applied | CommandOutcome::Queued => return None,
        CommandOutcome::Lost => "The server restarted before it applied the order".to_string(),
        CommandOutcome::Rejected { code, reason } => match code {
            RejectCode::Core if reason.is_empty() => "The order was refused".to_string(),
            RejectCode::Core => reason.clone(),
            RejectCode::TurnChanged => "The order was given in the previous turn".to_string(),
            RejectCode::TurnEnded => "The turn is already ending".to_string(),
            RejectCode::NotYourTurn => {
                "You cannot act now: a player you are at war with moves first".to_string()
            }
            RejectCode::NoGame | RejectCode::Restarting => {
                "The game server is restarting, repeat the order".to_string()
            }
            RejectCode::TooMany => "Too many orders are waiting".to_string(),
            RejectCode::Invalid => "Invalid order".to_string(),
            RejectCode::NotSeated => "You no longer play this civilization".to_string(),
            RejectCode::Expired => {
                "The result of the order is unknown after reconnecting; the view is up to date"
                    .to_string()
            }
        },
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(unit: i32) -> Command {
        Command::UnitMoveTo { unit, x: 1, y: 2 }
    }

    fn res(id: CmdId, outcome: CommandOutcome) -> CommandResult {
        CommandResult { id, outcome }
    }

    fn rejected(code: RejectCode) -> CommandOutcome {
        CommandOutcome::Rejected {
            code,
            reason: "core says no".into(),
        }
    }

    #[test]
    fn a_final_answer_settles_once_and_a_repeat_is_counted() {
        let mut o = Orders::new();
        let mut st = ClientStats::default();
        o.add(5, mv(1), 12, 100);
        let s = o.answer(res(5, CommandOutcome::Applied), &mut st).unwrap();
        assert_eq!((s.id, s.text), (5, None));
        assert_eq!(o.len(), 0);
        assert!(o.answer(res(5, CommandOutcome::Applied), &mut st).is_none());
        assert!(
            o.answer(res(9, CommandOutcome::Applied), &mut st).is_none(),
            "unknown id"
        );
        assert_eq!(
            (st.dup_results, st.unknown_results, st.orders_applied),
            (1, 1, 1)
        );
    }

    #[test]
    fn queued_marks_the_order_and_keeps_it_for_the_repeat() {
        let mut o = Orders::new();
        let mut st = ClientStats::default();
        o.add(1, mv(1), 3, 0);
        assert!(o.answer(res(1, CommandOutcome::Queued), &mut st).is_none());
        assert_eq!((o.len(), st.orders_queued), (1, 1));
        assert_eq!(o.unanswered().len(), 1);
        assert!(o.answer(res(1, CommandOutcome::Lost), &mut st).is_some());
        assert_eq!((o.len(), st.orders_lost), (0, 1));
    }

    #[test]
    fn unanswered_keeps_ids_turn_and_order() {
        let mut o = Orders::new();
        let mut st = ClientStats::default();
        o.add(7, mv(1), 4, 0);
        o.add(8, Command::EndTurn { force: false }, 4, 0);
        o.add(9, mv(2), 4, 0);
        o.answer(res(8, CommandOutcome::Applied), &mut st);
        let ids: Vec<(CmdId, i32)> = o.unanswered().iter().map(|c| (c.id, c.turn)).collect();
        assert_eq!(ids, [(7, 4), (9, 4)]);
    }

    #[test]
    fn rollback_texts_follow_the_table() {
        assert_eq!(rollback_text(&CommandOutcome::Applied), None);
        assert_eq!(
            rollback_text(&rejected(RejectCode::Core)).as_deref(),
            Some("core says no")
        );
        assert_eq!(
            rollback_text(&rejected(RejectCode::TurnEnded)).as_deref(),
            Some("The turn is already ending")
        );
        assert!(
            rollback_text(&CommandOutcome::Lost)
                .unwrap()
                .starts_with("The server restarted")
        );
        let mut st = ClientStats::default();
        let mut o = Orders::new();
        o.add(1, mv(1), 1, 0);
        o.answer(res(1, rejected(RejectCode::Expired)), &mut st);
        assert_eq!((st.orders_expired, st.orders_rejected), (1, 0));
    }

    #[test]
    fn the_ring_of_done_ids_is_bounded() {
        let mut o = Orders::new();
        let mut st = ClientStats::default();
        for id in 1..=(DONE_RING as u32 + 10) {
            o.add(id, mv(1), 1, 0);
            o.answer(res(id, CommandOutcome::Applied), &mut st);
        }
        // The oldest are forgotten and look unknown, the newest are repeats.
        o.answer(res(1, CommandOutcome::Applied), &mut st);
        o.answer(res(DONE_RING as u32 + 10, CommandOutcome::Applied), &mut st);
        assert_eq!((st.unknown_results, st.dup_results), (1, 1));
    }

    #[test]
    fn a_query_answer_counts_once_on_its_game_and_stream() {
        let mut o = Orders::new();
        let a = o.new_query(7, 3);
        let b = o.new_query(7, 3);
        assert_eq!((a, b), (1, 2));
        assert!(o.take_query(a, 7, 3));
        assert!(!o.take_query(a, 7, 3), "once");
        assert!(!o.take_query(b, 7, 4), "another stream");
        assert!(!o.take_query(99, 7, 3));
    }
}
