//! `session.op`: the session operations (save, leave, pause, resume, kick). They go to the session layer through the
//! sink, never to the game core. The route checks what the `session` root shows (we host, the seat is a human that
//! is not ours and not the last one) so the player hears why at once; the gateway checks the same again and its
//! refusal comes back as [`ActionEvent::SessionRefused`](crate::ActionEvent::SessionRefused).

use protocol::{GameCtlReason, PlayerId};
use ui2_model::Key;
use ui2_model::generated::{RefuseWhy, SeatControl, Session, SessionOp as OpName};

use crate::intent::Intent;
use crate::route::Ctx;
use crate::sink::SessionOp;

/// The seat id of a `key<player>`.
fn seat_of(key: Key) -> Option<PlayerId> {
    PlayerId::try_from(key.0).ok()
}

/// The operation an intent names, `None` for a kick without a seat.
pub(crate) fn op_of(name: OpName, player: Option<Key>) -> Option<SessionOp> {
    Some(match name {
        OpName::Save => SessionOp::Save,
        OpName::Leave => SessionOp::Leave,
        OpName::Pause => SessionOp::Pause,
        OpName::Resume => SessionOp::Resume,
        OpName::Kick => SessionOp::Kick(seat_of(player?)?),
    })
}

/// Why the session would refuse `op`, judged from the `session` root; `None`: send it.
pub(crate) fn precheck(session: Option<&Session>, op: SessionOp) -> Option<RefuseWhy> {
    if !op.host_only() {
        return None;
    }
    let Some(s) = session else {
        return Some(RefuseWhy::NoGame);
    };
    if !s.is_host {
        return Some(RefuseWhy::NotHost);
    }
    if let SessionOp::Kick(target) = op {
        let Some(seat) = s.seats.get(Key::from_i32(i32::from(target))) else {
            return Some(RefuseWhy::NoSuchPlayer);
        };
        if seat.control != SeatControl::Human {
            return Some(RefuseWhy::NotHuman);
        }
        if seat.me {
            return Some(RefuseWhy::SelfKick);
        }
        let humans = s
            .seats
            .iter()
            .filter(|x| x.control == SeatControl::Human)
            .count();
        if humans <= 1 {
            return Some(RefuseWhy::LastHuman);
        }
    }
    None
}

/// Ask the sink for `op` after the checks; a refusal becomes a toast.
pub(crate) fn request(ctx: &mut Ctx<'_>, op: SessionOp) {
    if let Some(why) = precheck(ctx.model.get::<Session>(), op) {
        ctx.refuse(why);
        return;
    }
    if let Err(refused) = ctx.sink.session_op(op) {
        ctx.refuse(refused.why());
    }
}

/// Run a `session.op` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let Some(name) = intent.variant("op").and_then(OpName::from_u16) else {
        return;
    };
    match op_of(name, intent.key("player")) {
        Some(op) => request(ctx, op),
        None => ctx.refuse(RefuseWhy::NoSuchPlayer),
    }
}

/// The reason a toast gives for the gateway's refusal of a session operation.
#[must_use]
pub fn refusal_why(reason: GameCtlReason) -> RefuseWhy {
    match reason {
        GameCtlReason::NotHost => RefuseWhy::NotHost,
        GameCtlReason::NoSuchPlayer => RefuseWhy::NoSuchPlayer,
        GameCtlReason::NotHuman => RefuseWhy::NotHuman,
        GameCtlReason::SelfKick => RefuseWhy::SelfKick,
        GameCtlReason::LastHuman => RefuseWhy::LastHuman,
        GameCtlReason::NoGame => RefuseWhy::NoGame,
    }
}
