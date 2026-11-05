//! Orders (net-client 4.5, NC-D5, 9): numbers, 32 in flight, the answers, the repeat after `Attached`, and
//! no orders without a connection. On the scripted gateway with a ring of 64 answers.

mod common;

use client_core::{ClientEvent, ClientPhase, ConnState, HostEvent, OrderRefused};
use common::world::World;
use common::*;
use protocol::{
    AttachMode, ClientMsg, Command, CommandOutcome, CommandResult, GameMsg, Query, QueryAnswer,
    RejectCode, ServerMsg, Token,
};

fn plan(stream: u64, last_cmd_id: u32) -> GamePlan {
    GamePlan {
        game: 7,
        player: 1,
        stream,
        last_cmd_id,
        last_seq: 0,
    }
}

/// Attached to game 7 with the view in, the server's last order number `last_cmd_id`.
fn game_from(last_cmd_id: u32) -> (Net, Token) {
    let mut n = Net::start();
    n.srv.world = Some(World::new(7, 11, 6, 5));
    n.srv.plan = Some(plan(3, last_cmd_id));
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    n.core.set_catalog(protocol::Catalog::default());
    n.take_events();
    (n, token)
}

fn game() -> (Net, Token) {
    game_from(0)
}

fn mv(unit: i32) -> Command {
    Command::UnitMoveTo { unit, x: 2, y: 3 }
}

fn rejected(code: RejectCode, reason: &str) -> CommandOutcome {
    CommandOutcome::Rejected {
        code,
        reason: reason.into(),
    }
}

/// The server pushes the batches to the client and the net settles.
fn deliver(n: &mut Net, token: Token, msgs: Vec<ServerMsg>) {
    for m in msgs {
        n.push(token, m);
    }
    n.settle();
}

fn cmds_seen(n: &Net) -> Vec<(u32, i32)> {
    n.srv.book.seen.iter().map(|c| (c.id, c.turn)).collect()
}

fn answered(n: &Net) -> Vec<(u32, CommandOutcome)> {
    n.events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::OrderAnswered { id, outcome } => Some((*id, outcome.clone())),
            _ => None,
        })
        .collect()
}

fn rolled_back(n: &Net) -> Vec<(u32, String)> {
    n.events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::OrderRolledBack { id, text } => Some((*id, text.clone())),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- the turn an order is stamped with (B1)

/// Attached to a game whose server view is in turn `turn`.
fn game_in_turn(turn: i32) -> Net {
    let mut n = Net::start();
    let mut w = World::new(7, 11, 6, 5);
    w.view.turn = turn;
    w.hash = protocol::ViewHash::of(&w.view);
    n.srv.world = Some(w);
    n.srv.plan = Some(plan(3, 0));
    n.connect();
    n.login();
    n.core.set_catalog(protocol::Catalog::default());
    n.take_events();
    n
}

#[test]
fn an_order_is_stamped_with_the_turn_of_the_replica_not_the_servers_view() {
    let mut n = game_in_turn(5);
    assert_eq!(n.core.shown().unwrap().turn, 5);
    n.srv.book.auto = false;
    let server_turn = n.core.shown().unwrap().turn;
    // The replica still shows the turn before: the order for it must not land in the server's turn.
    let id = n
        .core
        .order_at(Command::EndTurn { force: false }, Some(server_turn - 1))
        .expect("accepted");
    n.settle();
    assert_eq!(cmds_seen(&n), [(id, server_turn - 1)]);
    // No replica turn yet (0): the view's turn, as before.
    let id2 = n.core.order_at(mv(1), Some(0)).expect("accepted");
    let id3 = n.core.order(mv(2)).expect("accepted");
    n.settle();
    assert_eq!(cmds_seen(&n)[1..], [(id2, server_turn), (id3, server_turn)]);
}

#[test]
fn ui_messages_given_on_a_turn_the_server_has_left_are_refused_not_kept() {
    let mut n = game_in_turn(5);
    let server_turn = n.core.shown().unwrap().turn;
    let before = n.core.stats().gc_net_sent;
    assert_eq!(
        n.core.send_gc_net_at(vec![1, 2, 3], server_turn - 1),
        Err(OrderRefused::TurnChanged)
    );
    assert_eq!(n.core.stats().gc_net_sent, before);
    assert_eq!(n.core.stats().gc_net_stale, 1);
    // The same turn, a later replica turn, or an unknown one: they go out.
    assert_eq!(n.core.send_gc_net_at(vec![1], server_turn), Ok(()));
    assert_eq!(n.core.send_gc_net_at(vec![1], server_turn + 1), Ok(()));
    assert_eq!(n.core.send_gc_net_at(vec![1], 0), Ok(()));
    assert_eq!(n.core.stats().gc_net_sent, before + 3);
}

// ---------------------------------------------------------------- numbers and the pipeline

#[test]
fn numbers_start_after_the_servers_last_and_orders_do_not_wait_for_answers() {
    let (mut n, _) = game_from(41);
    n.srv.book.auto = false;
    assert_eq!(n.core.next_cmd_id(), 42);
    let turn = n.core.shown().unwrap().turn;
    // Three orders in a row, no settling between them (AC-N2.4).
    let ids: Vec<_> = [mv(1), mv(2), Command::EndTurn { force: false }]
        .into_iter()
        .map(|c| n.core.order(c).expect("accepted"))
        .collect();
    assert_eq!(ids, [42, 43, 44]);
    assert_eq!(n.core.orders_in_flight(), 3);
    n.settle();
    assert_eq!(cmds_seen(&n), [(42, turn), (43, turn), (44, turn)]);
    assert_eq!(n.core.stats().orders_sent, 3);
    assert_eq!(n.core.next_cmd_id(), 45);
}

#[test]
fn the_33rd_order_in_flight_is_refused_and_takes_no_number() {
    let (mut n, token) = game();
    n.srv.book.auto = false;
    for i in 0..32 {
        assert_eq!(n.core.order(mv(i)), Ok(i as u32 + 1));
    }
    assert_eq!(n.core.order(mv(99)), Err(OrderRefused::TooMany));
    assert_eq!(n.core.next_cmd_id(), 33, "a refusal spends no number");
    assert_eq!(n.core.orders_in_flight(), 32);
    n.settle();
    assert_eq!(n.srv.book.seen.len(), 32);
    // One answer frees one place.
    let a = n.srv.answer_order(5, CommandOutcome::Applied);
    deliver(&mut n, token, a);
    assert_eq!(n.core.orders_in_flight(), 31);
    assert_eq!(n.core.order(mv(99)), Ok(33));
}

// ---------------------------------------------------------------- answers

#[test]
fn every_kind_of_answer_ends_in_one_event_and_a_text_for_the_failures() {
    let (mut n, token) = game();
    n.srv.book.auto = false;
    let ids: Vec<u32> = (0..6).map(|i| n.core.order(mv(i)).unwrap()).collect();
    n.settle();
    let outcomes = [
        CommandOutcome::Applied,
        rejected(RejectCode::Core, "Cannot go there"),
        CommandOutcome::Lost,
        rejected(RejectCode::TurnChanged, ""),
        rejected(RejectCode::Expired, ""),
    ];
    // The sixth is queued first, and answered later.
    let q = n.srv.answer_order(ids[5], CommandOutcome::Queued);
    deliver(&mut n, token, q);
    assert!(answered(&n).is_empty(), "Queued is not an answer");
    assert_eq!(n.core.orders_in_flight(), 6);
    for (id, o) in ids.iter().zip(outcomes.iter()) {
        let m = n.srv.answer_order(*id, o.clone());
        deliver(&mut n, token, m);
    }
    let m = n.srv.answer_order(ids[5], CommandOutcome::Applied);
    deliver(&mut n, token, m);

    let got = answered(&n);
    assert_eq!(got.len(), 6);
    for (i, (id, o)) in got.iter().enumerate() {
        assert_eq!(*id, ids[i]);
        let want = outcomes.get(i).cloned().unwrap_or(CommandOutcome::Applied);
        assert_eq!(*o, want);
    }
    assert_eq!(n.core.orders_in_flight(), 0);
    let texts = rolled_back(&n);
    assert_eq!(
        texts.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        [ids[1], ids[2], ids[3], ids[4]],
        "Applied needs no text, the failures do"
    );
    assert_eq!(texts[0].1, "Cannot go there");
    assert_eq!(
        texts[1].1,
        "The server restarted before it applied the order"
    );
    assert_eq!(texts[2].1, "The order was given in the previous turn");
    assert!(texts[3].1.starts_with("The result of the order is unknown"));
    let s = n.core.stats();
    assert_eq!(
        (
            s.orders_applied,
            s.orders_rejected,
            s.orders_lost,
            s.orders_expired,
            s.orders_queued
        ),
        (2, 2, 1, 1, 1)
    );
}

#[test]
fn a_repeated_answer_and_an_unknown_number_are_ignored() {
    let (mut n, token) = game();
    n.srv.book.auto = false;
    let id = n.core.order(mv(1)).unwrap();
    n.settle();
    let m = n.srv.answer_order(id, CommandOutcome::Applied);
    deliver(&mut n, token, m.clone());
    assert_eq!(answered(&n).len(), 1);
    // The same answer again (a second batch), and an answer to an order of an earlier page.
    let w = n.srv.world.as_mut().unwrap();
    let dup = w.push(vec![GameMsg::Results(vec![
        CommandResult {
            id,
            outcome: CommandOutcome::Applied,
        },
        CommandResult {
            id: 500,
            outcome: CommandOutcome::Applied,
        },
    ])]);
    deliver(&mut n, token, vec![dup]);
    assert_eq!(answered(&n).len(), 1, "exactly one final answer");
    let s = n.core.stats();
    assert_eq!((s.dup_results, s.unknown_results), (1, 1));
    assert_eq!(n.core.phase(), ClientPhase::Playing);
}

// ---------------------------------------------------------------- repeat after Attached

#[test]
fn an_order_whose_answer_was_lost_is_repeated_after_replay_and_applied_once() {
    let (mut n, token) = game();
    let id = {
        // The answer never reaches the client: the line is cut after the order went out.
        n.mute = true;
        let id = n.core.order(mv(1)).unwrap();
        n.settle();
        id
    };
    assert_eq!(n.srv.book.applied, [id]);
    assert!(answered(&n).is_empty());
    n.mute = false;
    n.server_drops(token, 1006);
    n.advance(3_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Replay,
            ..
        }
    )));
    // The tail brought the answer; the repeat of the order brought the same answer once more.
    assert_eq!(answered(&n), [(id, CommandOutcome::Applied)]);
    assert_eq!(n.srv.book.applied, [id], "the worker applied it once");
    assert_eq!(cmds_seen(&n).len(), 2, "the order and its repeat");
    assert_eq!(n.srv.book.seen[0], n.srv.book.seen[1], "same id and turn");
    assert_eq!(n.core.orders_in_flight(), 0);
    assert_eq!(n.core.stats().orders_resent, 1);
    assert_eq!(n.core.stats().dup_results, 1);
}

#[test]
fn an_order_without_an_answer_is_repeated_after_fresh_with_the_same_numbers() {
    let (mut n, token) = game();
    n.srv.book.auto = false;
    let a = n.core.order(mv(1)).unwrap();
    let b = n.core.order(mv(2)).unwrap();
    n.settle();
    // The gateway restarted: another stream, so `Attached` says `Fresh`. The worker still holds both.
    n.srv.plan.as_mut().unwrap().stream = 4;
    n.server_drops(token, 1012);
    n.advance(3_000);
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Fresh,
            ..
        }
    )));
    assert_eq!(n.core.orders_in_flight(), 2);
    let seen = cmds_seen(&n);
    assert_eq!(seen.len(), 4);
    assert_eq!([seen[2].0, seen[3].0], [a, b], "in order, same numbers");
    assert_eq!(
        n.srv.book.held.len(),
        2,
        "the gateway kept silent for orders the worker has"
    );
    let token = n.core.token().unwrap();
    for id in [a, b] {
        let m = n.srv.answer_order(id, CommandOutcome::Applied);
        deliver(&mut n, token, m);
    }
    assert_eq!(answered(&n).len(), 2);
    assert_eq!(n.core.orders_in_flight(), 0);
    // The counter went on: the next order is a new number.
    assert_eq!(n.core.order(mv(3)), Ok(b + 1));
}

#[test]
fn a_ring_of_64_answers_serves_the_repeat_and_an_older_number_is_expired() {
    let (mut n, token) = game();
    // 70 orders answered; the client then repeats one that is still unanswered on its side only.
    for i in 0..70 {
        n.core.order(mv(i)).unwrap();
        n.settle();
    }
    assert_eq!(n.core.orders_in_flight(), 0);
    assert_eq!(n.srv.book.applied.len(), 70);
    // An old order the client "still has": drive the double directly with a number older than the ring.
    let w = n.srv.world.as_mut().unwrap();
    let old = protocol::Cmd {
        id: 2,
        turn: 1,
        cmd: mv(1),
    };
    let out = n.srv.book.on_cmd(old, w);
    deliver(&mut n, token, out);
    // The client does not know number 2 any more, so the Expired is ignored as unknown.
    assert_eq!(n.core.stats().unknown_results, 1);
    assert_eq!(n.srv.book.applied.len(), 70);
}

#[test]
fn an_order_given_before_the_drop_gets_its_answer_when_the_line_returns_and_numbers_go_on() {
    let (mut n, token) = game();
    n.srv.book.auto = false;
    let id = n.core.order(mv(1)).unwrap();
    n.settle();
    n.outage_start();
    n.advance(11_000);
    assert!(matches!(
        n.core.conn(),
        ConnState::Lost | ConnState::Reconnecting
    ));
    assert_eq!(
        n.core.orders_in_flight(),
        1,
        "kept: it was given with a line"
    );
    // The worker answers during the outage; the answer waits in the log.
    let _ = n.srv.answer_order(id, CommandOutcome::Applied);
    n.outage_end();
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert_eq!(answered(&n), [(id, CommandOutcome::Applied)]);
    assert_eq!(n.srv.book.applied, [id]);
    let _ = token;
    assert_eq!(n.core.order(mv(2)), Ok(id + 1));
}

// ---------------------------------------------------------------- no connection (NC-D5)

#[test]
fn without_a_connection_an_order_is_refused_spends_no_number_and_is_never_sent() {
    let (mut n, _) = game();
    n.outage_start();
    n.advance(11_000);
    assert!(matches!(
        n.core.conn(),
        ConnState::Lost | ConnState::Reconnecting
    ));
    assert!(!n.core.orders_enabled());
    let status = n
        .core
        .reconnect_status()
        .expect("status for the grey panel");
    assert!(status.attempt >= 1);
    let next = n.core.next_cmd_id();
    let before = n.core.stats().refused_offline;
    assert_eq!(n.core.order(mv(1)), Err(OrderRefused::NoConnection));
    assert_eq!(
        n.core.order(Command::EndTurn { force: false }),
        Err(OrderRefused::NoConnection)
    );
    assert_eq!(n.core.next_cmd_id(), next);
    assert_eq!(n.core.orders_in_flight(), 0);
    assert_eq!(n.core.stats().refused_offline, before + 2);
    assert_eq!(n.core.query(Query::Units), 0, "nothing was asked");
    n.outage_end();
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Online);
    assert!(n.srv.book.seen.is_empty(), "nothing waited and went later");
    // The panel takes orders again; the numbering goes on.
    assert!(n.core.orders_enabled());
    assert_eq!(n.core.order(mv(1)), Ok(next));
}

#[test]
fn offline_refuses_too_and_poor_still_takes_orders() {
    let (mut n, _) = game();
    n.host_event(HostEvent::Offline);
    assert_eq!(n.core.conn(), ConnState::Offline);
    assert_eq!(n.core.order(mv(1)), Err(OrderRefused::NoConnection));
    assert!(n.core.reconnect_status().is_some());
    n.host_event(HostEvent::Online);
    n.advance(500);
    assert_eq!(n.core.conn(), ConnState::Online);

    // Four seconds of silence: `Poor`, the socket is open and the order goes out.
    n.mute = true;
    n.advance(5_000);
    assert_eq!(n.core.conn(), ConnState::Poor);
    assert!(n.core.orders_enabled());
    assert_eq!(n.core.order(mv(1)), Ok(1));
    n.settle();
    assert_eq!(n.srv.book.seen.len(), 1);
}

#[test]
fn rejected_replaced_and_no_game_are_no_game() {
    // In the lobby.
    let mut n = Net::start();
    n.connect();
    n.login();
    assert_eq!(n.core.order(mv(1)), Err(OrderRefused::NoGame));
    assert_eq!(n.core.query(Query::Units), 0);

    // `Replaced` is final.
    let (mut n, token) = game();
    n.srv.book.auto = false;
    n.core.order(mv(1)).unwrap();
    n.push(
        token,
        ServerMsg::Detached {
            reason: protocol::DetachReason::Replaced,
        },
    );
    n.settle();
    assert_eq!(n.core.conn(), ConnState::Replaced);
    assert_eq!(n.core.order(mv(2)), Err(OrderRefused::NoGame));
    assert_eq!(
        n.core.orders_in_flight(),
        0,
        "the orders of a lost seat are dropped"
    );
    assert_eq!(n.core.stats().orders_dropped, 1);

    // A refused login.
    let mut n = Net::start();
    n.srv.password = "other".into();
    n.connect();
    n.login();
    assert_eq!(n.core.order(mv(1)), Err(OrderRefused::NoGame));
}

#[test]
fn a_detached_game_drops_its_orders_and_another_game_numbers_from_its_own_start() {
    let (mut n, token) = game_from(10);
    n.srv.book.auto = false;
    n.core.order(mv(1)).unwrap();
    n.settle();
    n.push(
        token,
        ServerMsg::Detached {
            reason: protocol::DetachReason::GameOver,
        },
    );
    n.settle();
    assert_eq!(n.core.orders_in_flight(), 0);
    assert_eq!(n.core.order(mv(2)), Err(OrderRefused::NoGame));
    // Another game on the same connection: its counter is the server's.
    n.srv.plan = Some(GamePlan {
        game: 8,
        player: 1,
        stream: 9,
        last_cmd_id: 2,
        last_seq: 0,
    });
    n.srv.world = Some(World::new(8, 5, 6, 5));
    n.srv.book = common::orders::OrderBook::new();
    n.core.dismiss_notice();
    n.drop_sock(
        n.srv.sock_of(token).unwrap(),
        client_core::DisconnectWhy::Remote(1000),
    );
    n.advance(3_000);
    assert_eq!(n.core.next_cmd_id(), 3);
    assert_eq!(n.core.order(mv(3)), Ok(3));
}

// ---------------------------------------------------------------- queries

#[test]
fn queries_are_counted_by_kind_and_answered_once_on_their_stream() {
    let (mut n, token) = game();
    let a = n.core.query(Query::Path {
        unit: 1,
        x: 1,
        y: 1,
    });
    let b = n.core.query(Query::Reachable { unit: 1 });
    let c = n.core.query(Query::Path {
        unit: 2,
        x: 3,
        y: 3,
    });
    assert!(a != 0 && b != 0 && c != 0 && a != b && b != c);
    n.settle();
    let s = n.core.stats();
    assert_eq!(s.queries.get("Path"), Some(&2));
    assert_eq!(s.queries.get("Reachable"), Some(&1));
    assert_eq!(s.queries.get("UnitActions"), None);
    assert_eq!(s.queries_answered, 3);
    assert_eq!(
        n.count_events(|e| matches!(
            e,
            ClientEvent::QueryResult {
                answer: QueryAnswer::Error { .. },
                ..
            }
        )),
        3
    );
    // A second answer to the same number and an answer to a number never asked are thrown away.
    n.push(
        token,
        ServerMsg::QueryResult {
            id: a,
            result: QueryAnswer::Units(vec![]),
        },
    );
    n.push(
        token,
        ServerMsg::QueryResult {
            id: 999,
            result: QueryAnswer::Units(vec![]),
        },
    );
    n.settle();
    assert_eq!(n.core.stats().queries_dropped, 2);
    assert_eq!(n.core.stats().queries_answered, 3);
}

#[test]
fn a_query_asked_before_a_reconnect_is_not_answered_to_the_new_attach() {
    let (mut n, token) = game();
    n.srv.answer_queries = false;
    let q = n.core.query(Query::Units);
    n.settle();
    n.server_drops(token, 1006);
    n.advance(3_000);
    let token = n.core.token().unwrap();
    n.push(
        token,
        ServerMsg::QueryResult {
            id: q,
            result: QueryAnswer::Units(vec![]),
        },
    );
    n.settle();
    assert_eq!(n.core.stats().queries_answered, 0);
    assert_eq!(n.core.stats().queries_dropped, 1);
}

// ---------------------------------------------------------------- the frame on the wire

#[test]
fn the_order_goes_as_a_cmd_with_its_number_and_the_turn_of_the_view() {
    let (mut n, _) = game_from(4);
    let turn = n.core.shown().unwrap().turn;
    n.core.order(Command::CancelEndTurn).unwrap();
    n.settle();
    let sent: Vec<_> = n
        .srv
        .log
        .iter()
        .filter_map(|(_, m)| match m {
            ClientMsg::Cmd(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1);
    assert_eq!((sent[0].id, sent[0].turn), (5, turn));
    assert_eq!(sent[0].cmd, Command::CancelEndTurn);
}
