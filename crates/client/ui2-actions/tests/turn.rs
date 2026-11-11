//! The turn button, end and cancel, and the screens of the host.

mod common;

use common::{Rig, city_at, key, s, unit_at};
use protocol::Command;
use ui2_actions::{CameraReq, Sent};
use ui2_model::generated::{Blocker, Meta, Phase, ScreenId, Turn, TurnPending, TurnUi};

fn turn_in(state: TurnUi) -> Turn {
    Turn {
        state,
        ..Turn::default()
    }
}

fn rig_in(state: TurnUi) -> Rig {
    let mut rig = Rig::new();
    rig.put(turn_in(state));
    rig
}

#[test]
fn end_turn_sends_the_order_and_marks_the_turn_pending() {
    let mut rig = rig_in(TurnUi::NextTurn);
    end(&mut rig);
    assert_eq!(rig.commands(), vec![Command::EndTurn { force: false }]);
    assert!(matches!(rig.sent()[0], Sent::Command { turn: 42, .. }));
    assert_eq!(rig.ui().turn_pending, TurnPending::Ending);
    let row = rig.ui().pending.iter().next().unwrap().clone();
    assert_eq!((&*row.intent, &*row.target), ("turn.end", "turn"));
    assert!(rig.toasts().is_empty());
}

#[test]
fn end_turn_can_be_forced() {
    let mut rig = rig_in(TurnUi::NextTurn);
    rig.one("turn.end", &[("force", ui2_model::Value::Bool(true))]);
    assert_eq!(rig.commands(), vec![Command::EndTurn { force: true }]);
}

fn end(rig: &mut Rig) {
    rig.one("turn.end", &[("force", ui2_model::Value::Bool(false))]);
}

#[test]
fn end_turn_refusals() {
    // The turn is over for the player already.
    let mut rig = rig_in(TurnUi::Ended);
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        ..Turn::default()
    });
    end(&mut rig);
    rig.assert_refused("cannot_end_turn");

    // The AI is moving.
    let mut rig = rig_in(TurnUi::AiMoving);
    rig.put(ui2_model::generated::Game {
        turn: 42,
        phase: Phase::Ai,
        ..Default::default()
    });
    end(&mut rig);
    rig.assert_refused("cannot_end_turn");

    // The link is down.
    let mut rig = rig_in(TurnUi::NextTurn);
    rig.put(Meta {
        connected: false,
        live: true,
        ..Meta::default()
    });
    end(&mut rig);
    rig.assert_refused("no_connection");

    // No game model at all.
    let mut rig = Rig::new();
    end(&mut rig);
    rig.assert_refused("no_game");
}

#[test]
fn a_second_end_turn_while_one_waits_is_dropped_quietly() {
    let mut rig = rig_in(TurnUi::NextTurn);
    end(&mut rig);
    end(&mut rig);
    assert_eq!(rig.commands().len(), 1);
    assert!(rig.toasts().is_empty());
}

#[test]
fn cancel_sends_the_order_when_the_model_allows_it() {
    let mut rig = Rig::new();
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        can_cancel: true,
        ..Turn::default()
    });
    rig.one("turn.cancel", &[]);
    assert_eq!(rig.commands(), vec![Command::CancelEndTurn]);
    assert_eq!(rig.ui().turn_pending, TurnPending::Cancelling);
}

#[test]
fn cancel_is_refused_when_it_cannot_be_taken_back() {
    let mut rig = Rig::new();
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        can_cancel: false,
        ..Turn::default()
    });
    rig.one("turn.cancel", &[]).unwrap();
    rig.assert_refused("cannot_cancel");
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
}

#[test]
fn primary_in_every_state() {
    // next_turn: ends the turn.
    let mut rig = rig_in(TurnUi::NextTurn);
    rig.one("turn.primary", &[]);
    assert_eq!(rig.commands(), vec![Command::EndTurn { force: false }]);

    // unit_needs_orders: selects the next unit and moves the camera.
    let mut rig = rig_in(TurnUi::UnitNeedsOrders);
    rig.units(vec![unit_at(5, 2, 3, true, true)]);
    rig.one("turn.primary", &[]);
    assert_eq!(rig.ui().selection.unit, Some(key(5)));
    assert_eq!(rig.sent(), &[Sent::Camera(CameraReq::Plot(2, 3))]);

    // city_needs_production: the production window of the blocking city.
    let mut rig = Rig::new();
    rig.cities(vec![city_at(20, 1, 1, true)]);
    rig.put(Turn {
        state: TurnUi::CityNeedsProduction,
        blocker: Some(Blocker {
            city: Some(key(20)),
            ..Blocker::default()
        }),
        ..Turn::default()
    });
    rig.one("turn.primary", &[]);
    assert!(rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, Some(key(20)));
    assert!(rig.sent().is_empty());

    // research_needed: the research window.
    let mut rig = rig_in(TurnUi::ResearchNeeded);
    rig.one("turn.primary", &[]);
    assert!(rig.ui().windows.research.open);

    // policy_needed and other_blocker: screens of the host.
    let mut rig = rig_in(TurnUi::PolicyNeeded);
    rig.one("turn.primary", &[]);
    assert_eq!(rig.sent(), &[Sent::Screen(ScreenId::Policies)]);
    let mut rig = rig_in(TurnUi::OtherBlocker);
    rig.one("turn.primary", &[]);
    assert_eq!(rig.sent(), &[Sent::Screen(ScreenId::NotificationLog)]);

    // ended: takes the end back.
    let mut rig = Rig::new();
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        can_cancel: true,
        ..Turn::default()
    });
    rig.one("turn.primary", &[]);
    assert_eq!(rig.commands(), vec![Command::CancelEndTurn]);
}

#[test]
fn primary_has_nothing_to_do_while_others_move() {
    for state in [TurnUi::WaitingOthers, TurnUi::AiMoving, TurnUi::GameOver] {
        let mut rig = rig_in(state);
        rig.one("turn.primary", &[]).unwrap();
        rig.assert_refused("cannot_end_turn");
    }
}

#[test]
fn primary_refusals_come_from_what_it_opens() {
    // A blocking city that is not the player's, or no city at all.
    let mut rig = Rig::new();
    rig.cities(vec![city_at(21, 1, 1, false)]);
    rig.put(Turn {
        state: TurnUi::CityNeedsProduction,
        blocker: Some(Blocker {
            city: Some(key(21)),
            ..Blocker::default()
        }),
        ..Turn::default()
    });
    rig.one("turn.primary", &[]).unwrap();
    rig.assert_refused("not_your_city");

    // No unit waits although the button said so (the model is a frame behind).
    let mut rig = rig_in(TurnUi::UnitNeedsOrders);
    rig.one("turn.primary", &[]).unwrap();
    rig.assert_refused("no_unit");

    // Without a turn root the button does nothing.
    let mut rig = Rig::new();
    rig.one("turn.primary", &[]).unwrap();
    rig.assert_refused("no_game");
}

#[test]
fn screen_open_goes_through_the_sink() {
    let mut rig = Rig::new();
    rig.one("screen.open", &[("screen", s("tech_tree"))]);
    assert_eq!(rig.sent(), &[Sent::Screen(ScreenId::TechTree)]);
    assert!(rig.toasts().is_empty());

    let mut rig = Rig::new();
    rig.sink.has_screens = false;
    rig.one("screen.open", &[("screen", s("diplomacy"))])
        .unwrap();
    rig.assert_refused("no_screen");
}

#[test]
fn a_refusing_sink_turns_into_a_toast_and_leaves_no_mark() {
    let mut rig = rig_in(TurnUi::NextTurn);
    rig.sink.refuse = Some(ui2_actions::Refused::NoConnection);
    end(&mut rig);
    assert_eq!(rig.why(), vec!["no_connection"]);
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
    assert!(rig.ui().pending.is_empty());
}
