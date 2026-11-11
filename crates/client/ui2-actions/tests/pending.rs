//! Orders waiting for an answer: the rows of `ui.pending`, `ui.turn_pending` and the toasts the answers leave.

mod common;

use common::{Rig, key, kstr, s, unit_at};
use protocol::{CommandOutcome, RejectCode};
use ui2_actions::{ActionEvent, TOAST_MS};
use ui2_model::generated::{Meta, ToastKind, Turn, TurnPending, TurnUi};
use ui2_model::{Op, Value};

fn rig_with_order() -> (Rig, u32) {
    let mut rig = Rig::new();
    rig.units(vec![unit_at(1, 3, 4, true, true)]);
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("sleep"))]);
    assert_eq!(rig.ui().pending.len(), 1);
    (rig, 1)
}

fn answer(id: u32, outcome: CommandOutcome) -> ActionEvent {
    ActionEvent::Answered { id, outcome }
}

fn meta(catching_up: bool) -> Meta {
    Meta {
        connected: true,
        live: true,
        catching_up,
        ..Meta::default()
    }
}

#[test]
fn applied_clears_the_row_once_the_replica_has_caught_up() {
    let (mut rig, id) = rig_with_order();
    rig.put(meta(true));
    rig.event(answer(id, CommandOutcome::Applied));
    assert_eq!(
        rig.ui().pending.len(),
        1,
        "the replica is still catching up"
    );

    rig.put(meta(false));
    let ops = rig.reconcile().expect("the row goes");
    assert!(rig.ui().pending.is_empty());
    assert!(rig.toasts().is_empty());
    assert!(
        ops.iter().any(|o| matches!(o, Op::Remove { .. })),
        "{ops:?}"
    );
}

#[test]
fn applied_with_a_replica_that_is_current_clears_in_the_same_frame() {
    let (mut rig, id) = rig_with_order();
    rig.event(answer(id, CommandOutcome::Applied)).unwrap();
    assert!(rig.ui().pending.is_empty());
    assert!(rig.toasts().is_empty());
}

#[test]
fn queued_keeps_the_row_and_says_nothing() {
    let (mut rig, id) = rig_with_order();
    rig.event(answer(id, CommandOutcome::Queued));
    assert_eq!(rig.ui().pending.len(), 1);
    assert!(rig.toasts().is_empty());
    // The final answer comes later.
    rig.event(answer(id, CommandOutcome::Applied));
    assert!(rig.ui().pending.is_empty());
}

#[test]
fn a_rejection_shows_a_toast_with_the_code_and_the_text() {
    let table = [
        (RejectCode::Core, "core"),
        (RejectCode::TurnChanged, "turn_changed"),
        (RejectCode::TurnEnded, "turn_ended"),
        (RejectCode::NotYourTurn, "not_your_turn"),
        (RejectCode::NoGame, "no_game"),
        (RejectCode::Restarting, "restarting"),
        (RejectCode::TooMany, "too_many"),
        (RejectCode::Invalid, "invalid"),
        (RejectCode::Expired, "expired"),
        (RejectCode::NotSeated, "not_seated"),
    ];
    for (code, why) in table {
        let (mut rig, id) = rig_with_order();
        rig.event(answer(
            id,
            CommandOutcome::Rejected {
                code,
                reason: "unit cannot sleep here".into(),
            },
        ));
        assert!(rig.ui().pending.is_empty(), "{why}");
        let toasts = rig.toasts();
        assert_eq!(toasts.len(), 1, "{why}");
        assert_eq!(toasts[0].kind, ToastKind::Rejected);
        assert_eq!(toasts[0].why.name(), why);
        assert_eq!(toasts[0].detail.as_deref(), Some("unit cannot sleep here"));
    }
}

#[test]
fn a_rejection_without_text_has_no_detail() {
    let (mut rig, id) = rig_with_order();
    rig.event(answer(
        id,
        CommandOutcome::Rejected {
            code: RejectCode::Invalid,
            reason: String::new(),
        },
    ));
    assert_eq!(rig.toasts()[0].detail, None);
}

#[test]
fn a_lost_order_and_a_rollback_show_a_lost_toast() {
    let (mut rig, id) = rig_with_order();
    rig.event(answer(id, CommandOutcome::Lost));
    assert!(rig.ui().pending.is_empty());
    assert_eq!(rig.toasts()[0].kind, ToastKind::Lost);

    let (mut rig, id) = rig_with_order();
    rig.event(ActionEvent::RolledBack {
        id,
        text: "no answer".into(),
    });
    assert!(rig.ui().pending.is_empty());
    let t = &rig.toasts()[0];
    assert_eq!(t.kind, ToastKind::Lost);
    assert_eq!(t.detail.as_deref(), Some("no answer"));
}

#[test]
fn an_order_the_module_did_not_send_changes_nothing() {
    let (mut rig, _) = rig_with_order();
    let ops = rig.event(answer(
        999,
        CommandOutcome::Rejected {
            code: RejectCode::Invalid,
            reason: "x".into(),
        },
    ));
    assert!(ops.is_none());
    assert_eq!(rig.ui().pending.len(), 1);
    assert!(rig.toasts().is_empty());
    assert!(
        rig.event(ActionEvent::RolledBack {
            id: 5,
            text: "x".into()
        })
        .is_none()
    );
}

#[test]
fn an_answer_settles_each_order_by_its_own_id() {
    let mut rig = Rig::new();
    rig.units(vec![
        unit_at(1, 0, 0, true, true),
        unit_at(2, 0, 0, true, true),
    ]);
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("sleep"))]);
    rig.one(
        "unit.action",
        &[("unit", kstr(2)), ("action", s("fortify"))],
    );
    assert_eq!(rig.ui().pending.len(), 2);
    rig.event(answer(2, CommandOutcome::Applied));
    let left: Vec<String> = rig
        .ui()
        .pending
        .iter()
        .map(|p| p.target.to_string())
        .collect();
    assert_eq!(left, vec!["unit/0:1".to_string()]);
}

fn end(rig: &mut Rig) {
    rig.put(Turn {
        state: TurnUi::NextTurn,
        ..Turn::default()
    });
    rig.one("turn.end", &[("force", Value::Bool(false))]);
    assert_eq!(rig.ui().turn_pending, TurnPending::Ending);
}

#[test]
fn turn_pending_clears_when_the_turn_shows_as_ended() {
    let mut rig = Rig::new();
    end(&mut rig);
    // The replica shows the end of turn before the answer reaches the module.
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        ..Turn::default()
    });
    rig.reconcile();
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
    assert_eq!(
        rig.ui().pending.len(),
        1,
        "the order is still waiting for its answer"
    );
    rig.event(answer(1, CommandOutcome::Applied));
    assert!(rig.ui().pending.is_empty());
}

#[test]
fn turn_pending_clears_with_the_answer() {
    let mut rig = Rig::new();
    end(&mut rig);
    rig.event(answer(1, CommandOutcome::Applied));
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
}

#[test]
fn a_rejected_end_or_cancel_clears_the_mark_and_toasts() {
    let mut rig = Rig::new();
    end(&mut rig);
    rig.event(answer(
        1,
        CommandOutcome::Rejected {
            code: RejectCode::NotYourTurn,
            reason: String::new(),
        },
    ));
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
    assert_eq!(rig.why(), vec!["not_your_turn"]);

    let mut rig = Rig::new();
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        can_cancel: true,
        ..Turn::default()
    });
    rig.one("turn.cancel", &[]);
    assert_eq!(rig.ui().turn_pending, TurnPending::Cancelling);
    rig.event(answer(
        1,
        CommandOutcome::Rejected {
            code: RejectCode::TurnEnded,
            reason: String::new(),
        },
    ));
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
    assert_eq!(rig.why(), vec!["turn_ended"]);
}

#[test]
fn a_cancel_shows_until_the_turn_stops_showing_as_ended() {
    let mut rig = Rig::new();
    rig.put(Turn {
        state: TurnUi::Ended,
        finished: true,
        can_cancel: true,
        ..Turn::default()
    });
    rig.one("turn.cancel", &[]);
    rig.reconcile();
    assert_eq!(rig.ui().turn_pending, TurnPending::Cancelling);
    rig.put(Turn {
        state: TurnUi::NextTurn,
        finished: false,
        ..Turn::default()
    });
    rig.reconcile();
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
}

#[test]
fn the_server_taking_the_end_back_clears_the_mark() {
    let mut rig = Rig::new();
    end(&mut rig);
    rig.event(ActionEvent::EndTurnCancelled { by_order: true });
    assert_eq!(rig.ui().turn_pending, TurnPending::None);
    // Nothing to clear: no change at all.
    assert!(
        rig.event(ActionEvent::EndTurnCancelled { by_order: false })
            .is_none()
    );
}

#[test]
fn toasts_go_after_six_seconds_in_one_delta() {
    let mut rig = Rig::new();
    for _ in 0..3 {
        rig.one("unit.select", &[("unit", kstr(99))]);
    }
    assert_eq!(rig.toasts().len(), 3);
    rig.now += TOAST_MS - 1;
    assert!(rig.send(vec![]).is_none(), "not yet");
    rig.now += 1;
    let ops = rig.send(vec![]).expect("one delta");
    assert_eq!(
        ops.iter()
            .filter(|o| matches!(o, Op::Remove { .. }))
            .count(),
        3,
        "{ops:?}"
    );
    assert!(rig.toasts().is_empty());
    assert!(rig.send(vec![]).is_none());
}

#[test]
fn toasts_of_answers_expire_too() {
    let (mut rig, id) = rig_with_order();
    rig.event(answer(id, CommandOutcome::Lost));
    assert_eq!(rig.toasts().len(), 1);
    rig.now += TOAST_MS;
    rig.reconcile().expect("the toast goes");
    assert!(rig.toasts().is_empty());
}

#[test]
fn a_new_game_forgets_the_old_one() {
    let (mut rig, _) = rig_with_order();
    rig.one("unit.select", &[("unit", kstr(1))]);
    rig.one("window.open", &[("window", s("research"))]);
    assert_eq!(rig.ui().selection.unit, Some(key(1)));
    rig.event(ActionEvent::NewGame);
    assert!(rig.ui().pending.is_empty());
    assert_eq!(rig.ui().selection.unit, None);
    assert!(rig.ui().windows.research.open, "windows of the skin stay");
    // An answer for an order of the old game does nothing.
    assert!(rig.event(answer(1, CommandOutcome::Lost)).is_none());
}

#[test]
fn a_pending_row_is_a_marker_only() {
    // Nothing but `ui` changes when an order is sent: the model of the game is not predicted.
    let mut rig = Rig::new();
    rig.units(vec![unit_at(1, 3, 4, true, true)]);
    let ops = rig
        .one("unit.action", &[("unit", kstr(1)), ("action", s("sleep"))])
        .unwrap();
    assert!(!ops.is_empty());
    let before = rig.store.get::<ui2_model::generated::Map>().cloned();
    rig.event(answer(1, CommandOutcome::Applied));
    assert_eq!(
        rig.store.get::<ui2_model::generated::Map>().cloned(),
        before
    );
}
