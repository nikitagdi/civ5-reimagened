//! The root `turn`: the state of the turn button, the end-turn blocker, the units waiting for orders, who ended
//! the turn, and the turn timer.

use std::sync::Arc;

use protocol::{Conn, SeatControl};
use replica::{Activity, GameInfo, ReadError, Unit};
use ui2_model::KeyedList;
use ui2_model::r#gen::{Blocker, Phase, Plot, Seat, Turn, TurnUi};

use crate::CountingRead;
use crate::build::{Cx, entity_key, game, player_key};
use crate::session;

/// `EndTurnBlockingTypes` of the core the turn button names (the others are "something else blocks").
pub mod blocking {
    pub const POLICY: i8 = 0;
    pub const RESEARCH: i8 = 1;
    pub const PRODUCTION: i8 = 2;
    pub const UNITS: i8 = 3;
    pub const FREE_TECH: i8 = 6;
    pub const STACKED_UNITS: i8 = 7;
    pub const UNIT_NEEDS_ORDERS: i8 = 8;
    pub const UNIT_PROMOTION: i8 = 9;
    pub const FREE_POLICY: i8 = 11;
    pub const CHOOSE_IDEOLOGY: i8 = 23;
}

/// The button state a blocker of the core asks for; `None` for no blocker (-1).
#[must_use]
pub fn blocker_state(kind: i8) -> Option<TurnUi> {
    use blocking::*;
    Some(match kind {
        k if k < 0 => return None,
        UNITS | STACKED_UNITS | UNIT_NEEDS_ORDERS | UNIT_PROMOTION => TurnUi::UnitNeedsOrders,
        PRODUCTION => TurnUi::CityNeedsProduction,
        RESEARCH | FREE_TECH => TurnUi::ResearchNeeded,
        POLICY | FREE_POLICY | CHOOSE_IDEOLOGY => TurnUi::PolicyNeeded,
        _ => TurnUi::OtherBlocker,
    })
}

/// What decides the turn button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnFacts {
    pub phase: Phase,
    /// `EndTurnBlockingTypes` of the seat, -1 = none.
    pub blocker: i8,
    /// We ended the turn.
    pub finished: bool,
    /// Every human ended the turn: it is ending and cannot be taken back.
    pub all_finished: bool,
    /// Hybrid turns: our seat may act now.
    pub may_act: bool,
}

/// The state of the turn button: the game's end, the AI's move, our own end of turn, a seat that waits for its
/// slot, then the core's blocker, else "next turn".
#[must_use]
pub fn turn_ui(f: &TurnFacts) -> TurnUi {
    match f.phase {
        Phase::GameOver => TurnUi::GameOver,
        Phase::Ai | Phase::Starting => TurnUi::AiMoving,
        Phase::Humans if f.finished && f.all_finished => TurnUi::Ended,
        Phase::Humans if f.finished || !f.may_act => TurnUi::WaitingOthers,
        Phase::Humans => blocker_state(f.blocker).unwrap_or(TurnUi::NextTurn),
    }
}

/// A unit of the seat that waits for orders.
// SIMPLIFIED(SIMP-241): "waits for orders" is decided here from the unit's moves, activity and automation, not by
// the core's own end-turn test for each unit, so a unit with a queued multi-turn move may count as waiting.
#[must_use]
pub fn waiting(u: &Unit) -> bool {
    u.moves_left > 0 && u.activity == Activity::Awake && !u.flags.automated()
}

/// The seat's units that wait for orders.
#[must_use]
pub fn units_waiting(units: &[Unit], seat: u8) -> i32 {
    let n = units
        .iter()
        .filter(|u| u.owner.cast_unsigned() == seat && waiting(u))
        .count();
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// The end-turn blocker of the seat with its target.
#[must_use]
pub fn blocker(info: &GameInfo, seat: u8) -> Option<Blocker> {
    if info.blocker < 0 {
        return None;
    }
    let me = seat.cast_signed();
    Some(Blocker {
        kind: info.blocker,
        unit: (info.blocker_unit >= 0).then(|| entity_key(me, info.blocker_unit)),
        city: (info.blocker_city >= 0).then(|| entity_key(me, info.blocker_city)),
        plot: (info.blocker_x >= 0 && info.blocker_y >= 0).then_some(Plot {
            x: info.blocker_x,
            y: info.blocker_y,
        }),
    })
}

/// `turn` from `Q_GAME` (the blocker), `Q_UNITS` (the units waiting), the session's turn state, seats and timer;
/// `Q_PLAYERS` only for a seat without a human nick.
pub(crate) fn turn(read: &mut CountingRead<'_, '_>, cx: &Cx<'_>) -> Result<Turn, ReadError> {
    let seat = read.seat();
    let info = read.game_info()?;
    let units = read.units()?;
    let ts = cx.session.turn_state();
    let finished = cx.session.my_finished().unwrap_or(false);

    let mut seats = KeyedList::new();
    let (ready, humans, all_finished, may_act) = match ts {
        Some(ts) => {
            let views = cx.session.seats();
            for s in &ts.seats {
                let view = views.iter().find(|v| v.player == s.player);
                let name: Arc<str> = match view.map(|v| &v.control) {
                    Some(SeatControl::Human { nick }) => Arc::from(nick.as_str()),
                    _ => match read.players() {
                        Ok(p) => super::players::find(&p, s.player)
                            .map(|p| Arc::from(p.name.as_str()))
                            .unwrap_or_else(|| Arc::from("")),
                        Err(_) => Arc::from(""),
                    },
                };
                let _ = seats.try_push(Seat {
                    player: player_key(s.player),
                    name,
                    me: i16::from(s.player) == i16::from(seat),
                    finished: s.finished,
                    may_act: s.may_act,
                    online: view.is_some_and(|v| v.conn == Conn::Online),
                });
            }
            let ready = ts.seats.iter().filter(|s| s.finished).count();
            let all = !ts.seats.is_empty() && ready == ts.seats.len();
            let may_act = ts
                .seats
                .iter()
                .find(|s| i16::from(s.player) == i16::from(seat))
                .is_none_or(|s| s.may_act);
            (
                i32::try_from(ready).unwrap_or(i32::MAX),
                i32::try_from(ts.seats.len()).unwrap_or(i32::MAX),
                all,
                may_act,
            )
        }
        // No turn state from a server: the core's count of humans.
        None => (
            i32::from(info.humans.saturating_sub(info.humans_left)),
            i32::from(info.humans),
            info.humans > 0 && info.humans_left == 0,
            true,
        ),
    };

    let state = turn_ui(&TurnFacts {
        phase: game::phase(ts, &info),
        blocker: info.blocker,
        finished,
        all_finished,
        may_act,
    });
    Ok(Turn {
        state,
        blocker: blocker(&info, seat),
        units_waiting: units_waiting(&units, seat),
        finished,
        can_cancel: finished && !all_finished,
        timer: session::timer(cx.session, cx.now_ms),
        ready,
        humans,
        seats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(phase: Phase, blocker: i8) -> TurnFacts {
        TurnFacts {
            phase,
            blocker,
            finished: false,
            all_finished: false,
            may_act: true,
        }
    }

    #[test]
    fn blockers_name_the_button() {
        use blocking::*;
        assert_eq!(blocker_state(-1), None);
        assert_eq!(blocker_state(UNITS), Some(TurnUi::UnitNeedsOrders));
        assert_eq!(blocker_state(UNIT_PROMOTION), Some(TurnUi::UnitNeedsOrders));
        assert_eq!(blocker_state(PRODUCTION), Some(TurnUi::CityNeedsProduction));
        assert_eq!(blocker_state(RESEARCH), Some(TurnUi::ResearchNeeded));
        assert_eq!(blocker_state(FREE_TECH), Some(TurnUi::ResearchNeeded));
        assert_eq!(blocker_state(POLICY), Some(TurnUi::PolicyNeeded));
        assert_eq!(blocker_state(CHOOSE_IDEOLOGY), Some(TurnUi::PolicyNeeded));
        assert_eq!(blocker_state(4), Some(TurnUi::OtherBlocker));
    }

    #[test]
    fn the_phase_and_our_end_of_turn_come_before_the_blocker() {
        assert_eq!(turn_ui(&facts(Phase::GameOver, 3)), TurnUi::GameOver);
        assert_eq!(turn_ui(&facts(Phase::Ai, 3)), TurnUi::AiMoving);
        assert_eq!(turn_ui(&facts(Phase::Starting, -1)), TurnUi::AiMoving);
        let mut f = facts(Phase::Humans, 3);
        f.finished = true;
        assert_eq!(turn_ui(&f), TurnUi::WaitingOthers);
        f.all_finished = true;
        assert_eq!(turn_ui(&f), TurnUi::Ended);
        let mut f = facts(Phase::Humans, -1);
        f.may_act = false;
        assert_eq!(turn_ui(&f), TurnUi::WaitingOthers);
        assert_eq!(turn_ui(&facts(Phase::Humans, -1)), TurnUi::NextTurn);
    }
}
