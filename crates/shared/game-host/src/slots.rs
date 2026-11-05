//! Seat changes (game-host-mp GH-D8, GH-D11, 4.8): `Kick` and `Claim` wait for a safe point (the human phase,
//! the order queue empty, the core agrees), then the core hands the slot over and every seat gets what it
//! needs: the seat that took the slot a `Full` (with the `Basis`) and its default contexts, the others diffs,
//! all a `Session` line, `Turn` and `SeatDone`.
//!
//! In the AI phase a change waits for the human phase. A core that is not at a safe point is asked again
//! after `RETRY_MS`.

use protocol::{JournalKind, SeatChange, SlotControl, StepKind};

use crate::core::{Core, CoreError, CoreErrorKind, EventTarget};
use crate::journal;
use crate::seats::Seat;
use crate::worker::{Fb, Worker};

/// Wait before the next look for a safe point.
const RETRY_MS: u64 = 50;
/// A wait for a safe point longer than this is reported at warn level, once per this long.
const UNSAFE_WARN_MS: u64 = 30_000;

impl<C: Core> Worker<C> {
    /// `what` waits for a safe point because of `why`. Live game 931 (2026-10-30): the core had no safe point for the rest
    /// of a turn, and kicks, saves and the snapshot of a rejoining seat waited without a word in the log (the reason was
    /// only at debug level). A wait past [`UNSAFE_WARN_MS`] is a warning with the core's reason.
    pub(crate) fn note_no_safe_point(&mut self, what: &str, why: &str) {
        let since = *self.unsafe_since_ms.get_or_insert(self.now_ms);
        let waited = self.now_ms.saturating_sub(since);
        if waited >= UNSAFE_WARN_MS
            && self.now_ms.saturating_sub(self.unsafe_warned_ms) >= UNSAFE_WARN_MS
        {
            self.unsafe_warned_ms = self.now_ms;
            tracing::warn!(waited_ms = waited, "{what} waits for a safe point: {why}");
        } else {
            tracing::debug!("no safe point for {what}: {why}");
        }
    }

    /// The core is at a safe point again: the next wait is counted from its start.
    pub(crate) fn note_safe_point(&mut self) {
        self.unsafe_since_ms = None;
    }

    /// Does the seat changes that wait, if the core is at a safe point.
    pub(crate) fn seat_step(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        if let Err(why) = self.core.safe_point() {
            self.note_no_safe_point("a seat change", &why);
            self.seat_blocked = true;
            self.seat_retry_ms = self.now_ms + RETRY_MS;
            return Ok(());
        }
        self.note_safe_point();
        fb.kind = Some(StepKind::Seat);
        let mut changed = false;
        let mut claimed = Vec::new();
        while let Some((player, change)) = self.pending_seats.pop_front() {
            let refused = match &change {
                SeatChange::Kick => self.kick(player)?,
                SeatChange::Claim { nick } => self.claim(player, nick)?,
            };
            if refused.is_none() {
                changed = true;
                if matches!(change, SeatChange::Claim { .. }) {
                    claimed.push(player);
                }
            }
            fb.tail.push(protocol::WorkerMsg::SeatDone {
                player,
                change,
                error: refused,
            });
        }
        if changed {
            self.pump_after_order(false)?;
            self.build_round(fb)?;
            if self.phase == protocol::TurnPhase::Humans {
                for p in claimed {
                    self.ctx_all_now(p, fb)?;
                }
            }
        }
        self.refresh_turn(fb)
    }

    /// `Ok(Some(text))`: the change was refused, the game is as it was.
    fn set_control(
        &mut self,
        player: protocol::PlayerId,
        control: &SlotControl,
    ) -> Result<Option<String>, CoreError> {
        match self.core.set_control(player, control) {
            Ok(()) => Ok(None),
            Err(e) if e.kind == CoreErrorKind::Failed => Ok(Some(e.message)),
            Err(e) => Err(e),
        }
    }

    fn tell_all(&mut self, text: String) {
        let entry = journal::worker_entry(self.turn, JournalKind::Session, text);
        self.journal_pending
            .route(EventTarget::All, self.seats.keys().copied(), &entry);
    }

    /// The human leaves, the AI takes the seat over: no more views for it.
    fn kick(&mut self, player: protocol::PlayerId) -> Result<Option<String>, CoreError> {
        let Some(nick) = self.seats.get(&player).map(|s| s.nick.clone()) else {
            return Ok(Some(format!("player {player} has no human seat")));
        };
        if let Some(why) = self.set_control(player, &SlotControl::Ai)? {
            return Ok(Some(why));
        }
        self.drop_offers_of(player);
        if let Some(seat) = self.seats.remove(&player) {
            self.retired_rev.insert(player, seat.rev);
            // The data of a team nobody of ours is in any more would be stale when a seat of it comes
            // back: it is read again then (the memory of cities stays).
            if !self.seats.values().any(|s| s.team == seat.team)
                && let Some(t) = self.teams.get_mut(&seat.team)
            {
                t.plots.clear();
            }
        }
        self.last_order_seat = self.last_order_seat.filter(|p| *p != player);
        self.tell_all(format!("{nick} left the game, the AI plays on"));
        Ok(None)
    }

    /// The human `nick` takes the slot of an AI player.
    fn claim(
        &mut self,
        player: protocol::PlayerId,
        nick: &str,
    ) -> Result<Option<String>, CoreError> {
        if self.seats.contains_key(&player) {
            return Ok(Some(format!("the slot of player {player} is taken")));
        }
        let slot = self.core.seats()?.into_iter().find(|s| s.player == player);
        match slot.map(|s| s.control) {
            Some(SlotControl::Ai) => {}
            Some(SlotControl::Human { .. }) => {
                return Ok(Some(format!("the slot of player {player} is taken")));
            }
            None => return Ok(Some(format!("there is no slot of player {player}"))),
        }
        let control = SlotControl::Human {
            nick: nick.to_owned(),
        };
        if let Some(why) = self.set_control(player, &control)? {
            return Ok(Some(why));
        }
        let mut seat = Seat::new(nick.to_owned());
        seat.rev = self.retired_rev.remove(&player).unwrap_or(0);
        self.seats.insert(player, seat);
        self.tell_all(format!("{nick} took over the slot of player {player}"));
        Ok(None)
    }
}
