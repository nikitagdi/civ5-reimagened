//! Peace between humans (game-host-mp GH-D13, 4.9, SIMP-057): the worker keeps the offers, the core only says
//! whether peace is possible (`can_make_peace`) and makes it (`AnswerPeace { accept: true }`, the core's
//! `MAKE_PEACE`).
//!
//! An offer is in the view of both sides until it is answered or the next human phase begins, when it
//! expires with a `System` line for both. It is not saved. Answers and refusals are journaled as
//! `Diplomacy` lines for both; the peace itself is journaled by the core.

use protocol::{
    CmdId, Command, CommandOutcome, JournalKind, PeaceOffer, PlayerId, RejectCode, TurnPhase,
};

use crate::core::{Core, CoreError, CoreErrorKind, EventTarget};
use crate::journal;
use crate::worker::{Fb, Worker};

impl<C: Core> Worker<C> {
    /// `ProposePeace` and `AnswerPeace` of `player`. Answers the order itself; returns `true` when the game
    /// changed (a round of views is due).
    pub(crate) fn peace_cmd(
        &mut self,
        player: PlayerId,
        id: CmdId,
        cmd: &Command,
        fb: &mut Fb,
    ) -> Result<bool, CoreError> {
        match *cmd {
            Command::ProposePeace { to } => self.propose_peace(player, to, id, fb),
            Command::AnswerPeace { from, accept } => {
                self.answer_peace(player, from, accept, id, fb)
            }
            _ => Ok(false),
        }
    }

    fn nick_of(&self, player: PlayerId) -> String {
        self.seats
            .get(&player)
            .map_or_else(|| format!("player {player}"), |s| s.nick.clone())
    }

    /// A line for one seat, filed for the next round of views.
    fn tell(&mut self, player: PlayerId, kind: JournalKind, text: String) {
        let entry = journal::worker_entry(self.turn, kind, text);
        self.journal_pending.route(
            EventTarget::Player(player),
            self.seats.keys().copied(),
            &entry,
        );
    }

    fn applied(&mut self, player: PlayerId, id: CmdId, fb: &mut Fb) {
        fb.result(player, id, CommandOutcome::Applied);
        if let Some(seat) = self.seats.get_mut(&player) {
            seat.applied_upto = Some(id);
        }
        self.last_order_seat = Some(player);
    }

    /// Every seat holds the offers it takes part in, in one order.
    fn sync_offers(&mut self) {
        self.offers.sort_by_key(|o| (o.from, o.to));
        for (&p, seat) in &mut self.seats {
            seat.peace_offers = self
                .offers
                .iter()
                .filter(|o| o.from == p || o.to == p)
                .copied()
                .collect();
        }
    }

    fn propose_peace(
        &mut self,
        player: PlayerId,
        to: PlayerId,
        id: CmdId,
        fb: &mut Fb,
    ) -> Result<bool, CoreError> {
        if to == player || !self.seats.contains_key(&to) {
            fb.reject(
                player,
                id,
                RejectCode::Invalid,
                "peace can be offered to another human seat only",
            );
            return Ok(false);
        }
        if self
            .offers
            .iter()
            .any(|o| (o.from, o.to) == (player, to) || (o.from, o.to) == (to, player))
        {
            fb.reject(
                player,
                id,
                RejectCode::Invalid,
                "there is an offer between the two seats already",
            );
            return Ok(false);
        }
        match self.core.can_make_peace(player, to) {
            Ok(true) => {}
            Ok(false) => {
                fb.reject(player, id, RejectCode::Core, "peace is not possible now");
                return Ok(false);
            }
            Err(e) if e.kind == CoreErrorKind::Failed => {
                fb.reject(player, id, RejectCode::Core, &e.message);
                return Ok(false);
            }
            Err(e) => return Err(e),
        }
        self.offers.push(PeaceOffer {
            from: player,
            to,
            turn: self.turn,
        });
        let (a, b) = (self.nick_of(player), self.nick_of(to));
        self.tell(
            player,
            JournalKind::Diplomacy,
            format!("You offered peace to {b}"),
        );
        self.tell(to, JournalKind::Diplomacy, format!("{a} offers peace"));
        self.sync_offers();
        self.applied(player, id, fb);
        Ok(true)
    }

    fn answer_peace(
        &mut self,
        player: PlayerId,
        from: PlayerId,
        accept: bool,
        id: CmdId,
        fb: &mut Fb,
    ) -> Result<bool, CoreError> {
        let Some(i) = self
            .offers
            .iter()
            .position(|o| o.from == from && o.to == player)
        else {
            fb.reject(player, id, RejectCode::Invalid, "there is no such offer");
            return Ok(false);
        };
        let (a, b) = (self.nick_of(player), self.nick_of(from));
        if accept {
            let r = self
                .core
                .command(player, &Command::AnswerPeace { from, accept: true });
            self.collect_events();
            if let Err(r) = r {
                // The offer stays: the seat may try again.
                fb.reject(player, id, RejectCode::Core, &r.text);
                return Ok(false);
            }
            self.offers.remove(i);
            self.sync_offers();
            self.applied(player, id, fb);
            self.pump_after_order(false)?;
        } else {
            self.offers.remove(i);
            self.tell(
                player,
                JournalKind::Diplomacy,
                format!("You declined the peace offer of {b}"),
            );
            self.tell(
                from,
                JournalKind::Diplomacy,
                format!("{a} declined your peace offer"),
            );
            self.sync_offers();
            self.applied(player, id, fb);
        }
        Ok(true)
    }

    /// At the start of a human phase the offers of the turns before expire, with a line for both sides.
    pub(crate) fn expire_offers(&mut self) {
        if self.phase != TurnPhase::Humans || self.offers.is_empty() {
            return;
        }
        let turn = self.turn;
        let (old, keep): (Vec<PeaceOffer>, Vec<PeaceOffer>) =
            self.offers.iter().partition(|o| o.turn < turn);
        if old.is_empty() {
            return;
        }
        self.offers = keep;
        for o in old {
            let text = format!(
                "The peace offer of {} to {} expired",
                self.nick_of(o.from),
                self.nick_of(o.to)
            );
            for p in [o.from, o.to] {
                let entry = journal::system_entry(turn, text.clone());
                self.journal_pending.route(
                    EventTarget::Player(p),
                    self.seats.keys().copied(),
                    &entry,
                );
            }
        }
        self.sync_offers();
    }

    /// The offers of a seat that leaves are void; the other side is told. Call it while the seat is still
    /// there.
    pub(crate) fn drop_offers_of(&mut self, player: PlayerId) {
        let nick = self.nick_of(player);
        let (gone, keep): (Vec<PeaceOffer>, Vec<PeaceOffer>) = self
            .offers
            .iter()
            .partition(|o| o.from == player || o.to == player);
        if gone.is_empty() {
            return;
        }
        self.offers = keep;
        for o in gone {
            let other = if o.from == player { o.to } else { o.from };
            self.tell(
                other,
                JournalKind::Diplomacy,
                format!("The peace offer between you and {nick} is void: {nick} left"),
            );
        }
        self.sync_offers();
    }
}
