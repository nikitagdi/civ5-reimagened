//! `session.op` and `screen.act` with `then`: the session operations, their checks against the `session` root, the
//! gateway's refusals and the operation that waits for its screen action.
mod common;

use common::{Rig, intent, s};
use protocol::{CommandOutcome, GameCtlReason, RejectCode};
use ui2_actions::{ActionEvent, Refused, Sent, SessionOp};
use ui2_model::generated::{SeatControl, Session, SessionSeat};
use ui2_model::{Key, KeyedList, Value};

fn seat(player: i32, control: SeatControl, host: bool, me: bool) -> SessionSeat {
    SessionSeat {
        player: Key::from_i32(player),
        nick: (control == SeatControl::Human).then(|| format!("p{player}").into()),
        control,
        host,
        me,
        ..SessionSeat::default()
    }
}

/// Seat 0 is ours; `host` says whether we host. Seat 1 is a human, seat 2 an AI.
fn session(rig: &mut Rig, host: bool) {
    rig.put(Session {
        is_host: host,
        seats: KeyedList::from_vec(vec![
            seat(0, SeatControl::Human, host, true),
            seat(1, SeatControl::Human, !host, false),
            seat(2, SeatControl::Ai, false, false),
        ])
        .unwrap(),
        ..Session::default()
    });
    rig.store.take_frame();
}

fn op(rig: &mut Rig, name: &str, player: Option<i32>) {
    let mut args = vec![("op", s(name))];
    if let Some(p) = player {
        args.push(("player", s(&Key::from_i32(p).to_js())));
    }
    rig.one("session.op", &args);
}

#[test]
fn the_host_saves_pauses_resumes_and_kicks_through_the_session() {
    let mut rig = Rig::new();
    session(&mut rig, true);
    op(&mut rig, "save", None);
    op(&mut rig, "pause", None);
    op(&mut rig, "resume", None);
    op(&mut rig, "kick", Some(1));
    assert_eq!(
        rig.sent(),
        &[
            Sent::Session(SessionOp::Save),
            Sent::Session(SessionOp::Pause),
            Sent::Session(SessionOp::Resume),
            Sent::Session(SessionOp::Kick(1)),
        ]
    );
    assert!(rig.toasts().is_empty());
    assert!(rig.commands().is_empty(), "no game order is involved");
}

#[test]
fn a_seat_that_does_not_host_is_refused_the_host_operations_but_may_leave() {
    for name in ["save", "pause", "resume"] {
        let mut rig = Rig::new();
        session(&mut rig, false);
        op(&mut rig, name, None);
        rig.assert_refused("not_host");
    }
    let mut rig = Rig::new();
    session(&mut rig, false);
    op(&mut rig, "leave", None);
    assert_eq!(rig.sent(), &[Sent::Session(SessionOp::Leave)]);
}

#[test]
fn kicks_are_checked_as_the_gateway_checks_them() {
    for (target, why) in [
        (Some(0), "self_kick"),
        (Some(2), "not_human"),
        (Some(7), "no_such_player"),
        (None, "no_such_player"),
    ] {
        let mut rig = Rig::new();
        session(&mut rig, true);
        op(&mut rig, "kick", target);
        rig.assert_refused(why);
    }
}

#[test]
fn without_a_session_root_host_operations_are_refused_and_a_closed_sink_says_why() {
    let mut rig = Rig::new();
    op(&mut rig, "save", None);
    rig.assert_refused("no_game");
    let mut rig = Rig::new();
    session(&mut rig, true);
    rig.sink.refuse = Some(Refused::NoConnection);
    op(&mut rig, "save", None);
    rig.assert_refused("no_connection");
}

#[test]
fn the_gateways_refusal_is_a_toast_with_its_reason() {
    let mut rig = Rig::new();
    session(&mut rig, true);
    for (reason, why) in [
        (GameCtlReason::NotHost, "not_host"),
        (GameCtlReason::LastHuman, "last_human"),
        (GameCtlReason::NoGame, "no_game"),
    ] {
        rig.event(ActionEvent::SessionRefused { reason });
        assert_eq!(rig.why().last(), Some(&why));
    }
}

fn confirm_exit(rig: &mut Rig) {
    rig.send(vec![intent(
        "screen.act",
        &[
            ("screen", s("confirm")),
            ("action", s("choose")),
            ("a", Value::I32(4)),
            ("b", Value::I32(0)),
            ("then", s("leave")),
        ],
    )]);
}

#[test]
fn a_chained_leave_waits_for_the_server_to_apply_the_screen_action() {
    let mut rig = Rig::new();
    session(&mut rig, false);
    confirm_exit(&mut rig);
    let id = match rig.sent() {
        [Sent::Command { id, .. }] => *id,
        other => panic!("one screen action, got {other:?}"),
    };
    // Nothing leaves before the answer, not even in a later frame.
    rig.send(vec![]);
    assert_eq!(rig.sent().len(), 1);
    rig.event(ActionEvent::Answered {
        id,
        outcome: CommandOutcome::ScreenApplied {
            screen: "confirm".into(),
            open: None,
            popup_done: Some(4),
            close: true,
        },
    });
    rig.send(vec![]);
    assert_eq!(rig.sent().last(), Some(&Sent::Session(SessionOp::Leave)));
    rig.send(vec![]);
    assert_eq!(rig.sent().len(), 2, "the operation runs once");
}

#[test]
fn a_chained_leave_is_dropped_with_a_refused_or_lost_screen_action() {
    for outcome in [
        CommandOutcome::Rejected {
            code: RejectCode::Core,
            reason: "no longer pending".into(),
        },
        CommandOutcome::Lost,
    ] {
        let mut rig = Rig::new();
        session(&mut rig, false);
        confirm_exit(&mut rig);
        let Sent::Command { id, .. } = rig.sent()[0].clone() else {
            panic!("a screen action");
        };
        rig.event(ActionEvent::Answered { id, outcome });
        rig.send(vec![]);
        assert_eq!(rig.sent().len(), 1, "no session operation after a failure");
    }
}

#[test]
fn a_chained_host_operation_is_checked_before_the_action_is_sent() {
    let mut rig = Rig::new();
    session(&mut rig, false);
    rig.send(vec![intent(
        "screen.act",
        &[
            ("screen", s("game_menu")),
            ("action", s("noop")),
            ("then", s("save")),
        ],
    )]);
    rig.assert_refused("not_host");
}
