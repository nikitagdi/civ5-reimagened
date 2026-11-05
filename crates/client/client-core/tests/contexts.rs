//! Contexts and focus through the stream (net-client 4.6, NC-D6, NC-D7): stored as the server sent them,
//! stale after a diff, fresh again by `Valid`, gone with a keyframe; one `Focus` per change.

mod common;

use client_core::{FocusPlan, OrderSink, UnitContextExt};
use common::world::World;
use common::*;
use protocol::{
    CityContext, ClientMsg, Command, ContextMsg, GameMsg, ReachFlags, ReachNode, Token, UnitContext,
};

fn game() -> (Net, Token) {
    let mut n = Net::start();
    n.srv.world = Some(World::new(7, 11, 6, 5));
    n.srv.plan = Some(GamePlan {
        game: 7,
        player: 1,
        stream: 3,
        last_cmd_id: 0,
        last_seq: 0,
    });
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    n.core.set_catalog(protocol::Catalog::default());
    (n, token)
}

fn world(n: &mut Net) -> &mut World {
    n.srv.world.as_mut().unwrap()
}

fn unit_ctx(unit: i32, view_seq: u64) -> UnitContext {
    UnitContext {
        unit,
        view_seq,
        reach: vec![
            ReachNode {
                x: 1,
                y: 1,
                parent: 0,
                turn: 0,
                moves_left: 120,
                flags: ReachFlags(0),
            },
            ReachNode {
                x: 2,
                y: 1,
                parent: 0,
                turn: 0,
                moves_left: 60,
                flags: ReachFlags(0),
            },
        ],
        ..UnitContext::default()
    }
}

fn focus_msgs(n: &Net) -> Vec<protocol::Focus> {
    n.srv
        .log
        .iter()
        .filter_map(|(_, m)| match m {
            ClientMsg::Focus(f) => Some(f.clone()),
            _ => None,
        })
        .collect()
}

fn send(n: &mut Net, token: Token, m: protocol::ServerMsg) {
    n.push(token, m);
    n.settle();
}

#[test]
fn a_context_is_stored_and_fresh_for_the_view_it_names() {
    let (mut n, token) = game();
    let seq = world(&mut n).view_seq;
    assert!(n.core.contexts().unit(4).is_missing());
    let m = world(&mut n).push(vec![GameMsg::Context(ContextMsg::Unit(unit_ctx(4, seq)))]);
    send(&mut n, token, m);
    let c = n.core.contexts().unit(4);
    assert!(c.is_fresh());
    assert_eq!(c.get().unwrap().path_to(2, 1).unwrap().len(), 2);
}

#[test]
fn a_diff_makes_it_stale_and_valid_makes_it_fresh_again() {
    let (mut n, token) = game();
    let seq = world(&mut n).view_seq;
    let m = world(&mut n).push(vec![
        GameMsg::Context(ContextMsg::Unit(unit_ctx(4, seq))),
        GameMsg::Context(ContextMsg::City(CityContext {
            city: 2,
            view_seq: seq,
            ..CityContext::default()
        })),
    ]);
    send(&mut n, token, m);
    let m = world(&mut n).step();
    send(&mut n, token, m);
    assert!(n.core.contexts().unit(4).is_stale(), "the view moved on");
    assert!(n.core.contexts().city(2).is_stale());
    assert!(n.core.contexts().unit(4).get().is_some(), "still shown");
    // `Valid` in the batch of the next diff: the far-away move did not touch them.
    let new = world(&mut n).mutated();
    let mut batch = match world(&mut n).step_to(new) {
        protocol::ServerMsg::Game(b) => b,
        _ => unreachable!(),
    };
    let view_seq = batch.seq;
    batch.msgs.push(GameMsg::Context(ContextMsg::Valid {
        units: vec![4],
        city: Some(2),
        view_seq,
    }));
    send(&mut n, token, protocol::ServerMsg::Game(batch));
    assert!(n.core.contexts().unit(4).is_fresh());
    assert!(n.core.contexts().city(2).is_fresh());
}

#[test]
fn dropped_and_a_keyframe_forget_contexts() {
    let (mut n, token) = game();
    let seq = world(&mut n).view_seq;
    let m = world(&mut n).push(vec![
        GameMsg::Context(ContextMsg::Unit(unit_ctx(4, seq))),
        GameMsg::Context(ContextMsg::Unit(unit_ctx(5, seq))),
        GameMsg::Context(ContextMsg::Dropped { unit: 4 }),
    ]);
    send(&mut n, token, m);
    assert!(n.core.contexts().unit(4).is_missing());
    assert!(n.core.contexts().unit(5).is_fresh());
    // A keyframe: the old contexts go, the ones that come after it in the batch stay.
    let mut batch = match world(&mut n).keyframe(false) {
        protocol::ServerMsg::Game(b) => b,
        _ => unreachable!(),
    };
    let view_seq = batch.seq;
    batch
        .msgs
        .push(GameMsg::Context(ContextMsg::Unit(unit_ctx(6, view_seq))));
    send(&mut n, token, protocol::ServerMsg::Game(batch));
    assert!(n.core.contexts().unit(5).is_missing());
    assert!(n.core.contexts().unit(6).is_fresh());
}

#[test]
fn a_fresh_attach_clears_the_contexts() {
    let (mut n, token) = game();
    let seq = world(&mut n).view_seq;
    let m = world(&mut n).push(vec![GameMsg::Context(ContextMsg::Unit(unit_ctx(4, seq)))]);
    send(&mut n, token, m);
    assert!(n.core.contexts().unit(4).is_fresh());
    n.srv.plan.as_mut().unwrap().stream = 4;
    n.server_drops(token, 1012);
    n.advance(3_000);
    assert!(n.core.contexts().unit(4).is_missing());
}

#[test]
fn the_focus_goes_once_per_change_and_again_after_a_new_attach() {
    let (mut n, token) = game();
    assert!(focus_msgs(&n).is_empty());
    let plan = FocusPlan::new(Some(4), [4, 1, 7], Some(2));
    n.core.set_focus(&plan);
    n.core.set_focus(&plan);
    n.settle();
    assert_eq!(
        focus_msgs(&n),
        [protocol::Focus {
            units: vec![4, 1, 7],
            city: Some(2)
        }]
    );
    // Another selection: one more. Nothing selected: an empty focus, so the server counts nothing.
    n.core.set_focus(&FocusPlan::new(Some(1), [4, 7], Some(2)));
    n.core.set_focus(&FocusPlan::default());
    n.settle();
    let sent = focus_msgs(&n);
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[2], protocol::Focus::default());
    assert_eq!(n.core.stats().focus_sent, 3);
    // The server forgets the focus with the socket: after the next `Attached` it is sent again.
    n.core.set_focus(&plan);
    n.settle();
    assert_eq!(focus_msgs(&n).len(), 4);
    n.srv.plan.as_mut().unwrap().last_seq = 99;
    n.server_drops(token, 1012);
    n.advance(3_000);
    n.core.set_focus(&plan);
    n.settle();
    assert_eq!(focus_msgs(&n).len(), 5);
}

#[test]
fn contexts_of_units_that_left_the_focus_are_dropped() {
    let (mut n, token) = game();
    let seq = world(&mut n).view_seq;
    let m = world(&mut n).push(vec![
        GameMsg::Context(ContextMsg::Unit(unit_ctx(4, seq))),
        GameMsg::Context(ContextMsg::Unit(unit_ctx(5, seq))),
    ]);
    send(&mut n, token, m);
    n.core.set_focus(&FocusPlan::new(Some(5), [], None));
    assert!(n.core.contexts().unit(4).is_missing());
    assert!(n.core.contexts().unit(5).is_fresh());
}

#[test]
fn no_focus_without_a_game_and_the_counters_count_what_the_player_met() {
    // No game: nothing is sent.
    let mut n = Net::start();
    n.connect();
    n.login();
    n.core.set_focus(&FocusPlan::new(Some(4), [], None));
    n.settle();
    assert!(focus_msgs(&n).is_empty());

    let (mut n, token) = game();
    let seq = world(&mut n).view_seq;
    let m = world(&mut n).push(vec![GameMsg::Context(ContextMsg::Unit(unit_ctx(4, seq)))]);
    send(&mut n, token, m);
    // Selecting a unit whose context is in: fresh; one that has none: stale.
    n.core.set_focus(&FocusPlan::new(Some(4), [5], None));
    n.core.set_focus(&FocusPlan::new(Some(5), [4], None));
    let s = n.core.stats();
    assert_eq!((s.ctx_fresh_at_select, s.ctx_stale_at_select), (1, 1));
    // An order to a unit without a fresh context is counted and still sent.
    assert!(n.core.order(Command::UnitFortify { unit: 5 }).is_ok());
    assert!(n.core.order(Command::UnitFortify { unit: 4 }).is_ok());
    assert!(
        n.core.order(Command::EndTurn { force: false }).is_ok(),
        "not a unit order"
    );
    assert_eq!(n.core.stats().ctx_stale_at_order, 1);
}

#[test]
fn the_core_is_an_order_sink() {
    let (mut n, _) = game();
    fn through_the_sink(sink: &mut dyn OrderSink) -> u32 {
        sink.set_focus(&FocusPlan::default());
        sink.order(Command::UnitSkip { unit: 1 }).unwrap()
    }
    let id = through_the_sink(&mut n.core);
    assert_eq!(id, 1);
    n.settle();
    assert_eq!(focus_msgs(&n).len(), 1);
}
