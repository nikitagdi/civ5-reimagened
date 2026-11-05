//! The optimistic layer (net-client 4.4, NC-D6, NC-D13, NP-D23, 9) on the scripted gateway: a picture of an
//! order at once, taken off by the answer; no picture without a fresh context; a rolled-back order leaves a
//! line; the keyframe wait, the guard, the end of the turn and the blocker.

mod common;

use client_core::{ClientEvent, HostEvent, UnitContextExt};
use common::world::{Damage, World};
use common::*;
use protocol::{
    Blocker, BlockerTarget, CityBuildFlags, CityBuildView, CityContext, CityView, CmdId, Command,
    CommandOutcome, CommandResult, ContextMsg, GameMsg, JournalKind, PlayerView, ReachFlags,
    ReachNode, RejectCode, SeatTurn, ServerMsg, Token, TurnMode, TurnPhase, TurnState,
    UnitActionView, UnitContext, UnitFlags, UnitView, ViewHash, WorldView,
};

const ME: i8 = 1;

fn unit(id: i32, x: i16, y: i16) -> UnitView {
    UnitView {
        id,
        owner: ME,
        x,
        y,
        hp: 10,
        max_hp: 10,
        moves_left: 120,
        max_moves: 120,
        flags: UnitFlags(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS),
        name: format!("unit {id}"),
        ..UnitView::default()
    }
}

fn turn_state(finished: bool) -> TurnState {
    TurnState {
        turn: 5,
        phase: TurnPhase::Humans,
        mode: TurnMode::Simultaneous,
        seats: vec![SeatTurn {
            player: ME,
            finished,
            may_act: true,
            group: 0,
        }],
        timers: vec![],
        paused: false,
    }
}

fn node(x: i16, y: i16, parent: u16, moves_left: i16) -> ReachNode {
    ReachNode {
        x,
        y,
        parent,
        turn: 0,
        moves_left,
        flags: ReachFlags(0),
    }
}

fn unit_ctx(id: i32, at: (i16, i16), view_seq: u64) -> UnitContext {
    UnitContext {
        unit: id,
        view_seq,
        actions: (4..=12)
            .map(|a| UnitActionView {
                action: a,
                enabled: true,
                reason: String::new(),
            })
            .collect(),
        reach: vec![
            node(at.0, at.1, 0, 120),
            node(at.0 + 1, at.1, 0, 60),
            node(at.0 + 2, at.1, 1, 0),
        ],
        ..UnitContext::default()
    }
}

fn city_ctx(view_seq: u64) -> CityContext {
    CityContext {
        city: 2,
        view_seq,
        builds: vec![CityBuildView {
            order_type: 0,
            item: 1,
            turns: 7,
            production_cost: 80,
            gold_cost: -1,
            flags: CityBuildFlags(CityBuildFlags::ENABLED),
            faith_cost: -1,
        }],
        targets: vec![],
    }
}

fn plan() -> GamePlan {
    GamePlan {
        game: 7,
        player: ME,
        stream: 3,
        last_cmd_id: 0,
        last_seq: 0,
    }
}

/// Our units 4 at (1,1) and 6 at (3,3), our city 2; the human turn, `blocker` as given. The server holds
/// every order until the test answers it. Contexts for all three are fresh.
fn scripted_with(blocker: Option<Blocker>, finished: bool) -> (Net, Token) {
    let mut n = Net::start();
    let mut w = World::new(7, 11, 6, 5);
    w.view.human = ME;
    w.view.turn = 5;
    w.view.blocker = blocker;
    w.view.status = protocol::TurnStatus::HumanTurn;
    w.view.units = vec![unit(4, 1, 1), unit(6, 3, 3)];
    w.view.cities = vec![CityView {
        id: 2,
        owner: ME,
        x: 2,
        y: 2,
        production_item: -1,
        production_order: -1,
        ..CityView::default()
    }];
    w.view.players = vec![PlayerView {
        id: ME,
        ..PlayerView::default()
    }];
    w.hash = ViewHash::of(&w.view);
    n.srv.world = Some(w);
    n.srv.plan = Some(plan());
    n.srv.book.auto = false;
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    n.core.set_catalog(protocol::Catalog::default());
    let seq = world(&mut n).view_seq;
    let m = world(&mut n).push(vec![
        GameMsg::TurnState(turn_state(finished)),
        GameMsg::Context(ContextMsg::Unit(unit_ctx(4, (1, 1), seq))),
        GameMsg::Context(ContextMsg::Unit(unit_ctx(6, (3, 3), seq))),
        GameMsg::Context(ContextMsg::City(city_ctx(seq))),
    ]);
    n.push(token, m);
    n.settle();
    n.take_events();
    n.core.take_change();
    (n, token)
}

fn scripted() -> (Net, Token) {
    scripted_with(None, false)
}

fn world(n: &mut Net) -> &mut World {
    n.srv.world.as_mut().unwrap()
}

fn deliver(n: &mut Net, token: Token, msgs: Vec<ServerMsg>) {
    for m in msgs {
        n.push(token, m);
    }
    n.settle();
}

fn base_unit(n: &Net, id: i32) -> UnitView {
    n.core
        .base()
        .unwrap()
        .units
        .iter()
        .find(|u| u.id == id)
        .unwrap()
        .clone()
}

fn shown_unit(n: &mut Net, id: i32) -> UnitView {
    n.core
        .shown()
        .unwrap()
        .units
        .iter()
        .find(|u| u.id == id)
        .unwrap()
        .clone()
}

fn finished(n: &Net) -> bool {
    n.core
        .turn_state()
        .unwrap()
        .seats
        .iter()
        .find(|s| s.player == ME)
        .unwrap()
        .finished
}

fn cancelled(n: &Net) -> Vec<bool> {
    n.events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::EndTurnCancelled { by_order } => Some(*by_order),
            _ => None,
        })
        .collect()
}

fn rolled_back(n: &Net) -> Vec<(CmdId, String)> {
    n.events
        .iter()
        .filter_map(|e| match e {
            ClientEvent::OrderRolledBack { id, text } => Some((*id, text.clone())),
            _ => None,
        })
        .collect()
}

/// The server applies order `id`: the new view as a diff, the turn state, then the answer, in one batch
/// (NP 4.8: the answer comes after the view with the consequences), and the contexts valid again.
fn server_applies(
    n: &mut Net,
    token: Token,
    id: CmdId,
    change: impl FnOnce(&mut WorldView),
    turn: Option<TurnState>,
) {
    let mut new = world(n).view.clone();
    change(&mut new);
    let mut batch = match world(n).step_to(new) {
        ServerMsg::Game(b) => b,
        _ => unreachable!(),
    };
    let seq = batch.seq;
    if let Some(t) = turn {
        batch.msgs.push(GameMsg::TurnState(t));
    }
    batch.msgs.push(GameMsg::Results(vec![CommandResult {
        id,
        outcome: CommandOutcome::Applied,
    }]));
    batch.msgs.push(GameMsg::Context(ContextMsg::Valid {
        units: vec![4, 6],
        city: Some(2),
        view_seq: seq,
    }));
    n.srv.book.held.retain(|h| h.id != id);
    deliver(n, token, vec![ServerMsg::Game(batch)]);
}

fn rejected(code: RejectCode, reason: &str) -> CommandOutcome {
    CommandOutcome::Rejected {
        code,
        reason: reason.into(),
    }
}

fn sleep(unit: i32) -> Command {
    Command::UnitSleep { unit }
}

// ---------------------------------------------------------------- the picture and its answer

#[test]
fn an_order_is_drawn_for_the_next_frame_and_the_answer_takes_the_record_off() {
    let (mut n, token) = scripted();
    let rev = n.core.revision();
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    // The very next frame sees the picture; base is untouched.
    let shown = shown_unit(&mut n, 4);
    assert!(
        shown.flags.contains(UnitFlags::SLEEPING) && !shown.flags.contains(UnitFlags::NEEDS_ORDERS)
    );
    assert!(base_unit(&n, 4).flags.contains(UnitFlags::NEEDS_ORDERS));
    assert_eq!(n.core.overlay_len(), 1);
    assert!(
        n.core.revision() > rev,
        "a frame that reads the revision sees it"
    );
    // The change says: the layer alone, units and header, this unit.
    let c = n.core.take_change();
    assert!(c.overlay && !c.reset);
    assert!(c.sections.contains(0) && c.sections.contains(2) && !c.sections.contains(1));
    assert_eq!(c.dirty_units.unwrap().upsert, [(ME, 4)]);
    assert_eq!(c.dirty_plots, Some(vec![]));
    // Another unit is untouched.
    assert!(
        shown_unit(&mut n, 6)
            .flags
            .contains(UnitFlags::NEEDS_ORDERS)
    );

    // Applied: the view already says so, the record goes.
    server_applies(
        &mut n,
        token,
        id,
        |v| {
            let u = v.units.iter_mut().find(|u| u.id == 4).unwrap();
            u.flags = UnitFlags(UnitFlags::SLEEPING | UnitFlags::CAN_MOVE);
        },
        None,
    );
    assert_eq!(n.core.overlay_len(), 0);
    assert_eq!(n.core.shown().cloned(), n.core.base().cloned());
    assert!(shown_unit(&mut n, 4).flags.contains(UnitFlags::SLEEPING));
    let c = n.core.take_change();
    assert!(!c.overlay, "a diff came with it");
    assert_eq!(n.core.stats().layer_records, 1);
}

#[test]
fn a_move_is_drawn_from_the_tree_and_the_shown_view_is_base_without_records() {
    let (mut n, _) = scripted();
    assert_eq!(n.core.shown().cloned(), n.core.base().cloned());
    n.core
        .order(Command::UnitMoveTo {
            unit: 4,
            x: 2,
            y: 1,
        })
        .unwrap();
    let u = shown_unit(&mut n, 4);
    assert_eq!((u.x, u.y, u.moves_left), (2, 1, 60));
    assert_eq!(base_unit(&n, 4).x, 1);
    // The tree is the server's: a plot outside it gets no picture.
    n.core
        .order(Command::UnitMoveTo {
            unit: 6,
            x: 9,
            y: 9,
        })
        .unwrap();
    assert_eq!(shown_unit(&mut n, 6).x, 3);
    assert_eq!(n.core.overlay_len(), 1);
    assert_eq!(n.core.orders_in_flight(), 2, "the order still goes");
    let ctx = n.core.contexts().unit(4).get().unwrap().clone();
    assert_eq!(ctx.path_to(3, 1).unwrap().len(), 3);
}

#[test]
fn a_rejected_order_gives_the_view_back_and_writes_a_line() {
    let (mut n, token) = scripted();
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    assert_eq!(n.core.overlay_len(), 1);
    let msgs = n
        .srv
        .answer_order(id, rejected(RejectCode::Core, "Cannot sleep here"));
    deliver(&mut n, token, msgs);
    assert_eq!(n.core.overlay_len(), 0);
    assert_eq!(n.core.shown().cloned(), n.core.base().cloned());
    assert!(
        shown_unit(&mut n, 4)
            .flags
            .contains(UnitFlags::NEEDS_ORDERS)
    );
    // The line: the core's reason, a `System` line, with the unit's plot.
    let line = n.core.journal().last().unwrap();
    assert_eq!(line.text, "Cannot sleep here");
    assert_eq!((line.kind, line.at), (JournalKind::System, Some((1, 1))));
    assert_eq!(line.turn, 5);
    assert_eq!(rolled_back(&n), [(id, "Cannot sleep here".to_string())]);
    assert_eq!(n.core.stats().layer_rollbacks, 1);
    // The unit is free: the next order is drawn again.
    n.core.order(sleep(4)).unwrap();
    assert_eq!(n.core.overlay_len(), 1);
}

#[test]
fn lost_and_expired_orders_leave_neutral_lines() {
    let (mut n, token) = scripted();
    let a = n.core.order(sleep(4)).unwrap();
    let b = n.core.order(sleep(6)).unwrap();
    let msgs = n.srv.answer_order(a, CommandOutcome::Lost);
    deliver(&mut n, token, msgs);
    let msgs = n.srv.answer_order(b, rejected(RejectCode::Expired, ""));
    deliver(&mut n, token, msgs);
    assert_eq!(n.core.overlay_len(), 0);
    let texts: Vec<&str> = n
        .core
        .journal()
        .iter()
        .rev()
        .take(2)
        .map(|e| e.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "The result of the order is unknown after reconnecting; the view is up to date",
            "The server restarted before it applied the order"
        ]
    );
}

#[test]
fn queued_keeps_the_record() {
    let (mut n, token) = scripted();
    let id = n.core.order(sleep(4)).unwrap();
    let msgs = n.srv.answer_order(id, CommandOutcome::Queued);
    deliver(&mut n, token, msgs);
    assert_eq!(n.core.overlay_len(), 1);
}

// ---------------------------------------------------------------- when there is no picture

#[test]
fn no_picture_from_a_stale_context() {
    let (mut n, token) = scripted();
    // A diff far from the unit: no `Valid`, so the contexts are stale (4.6).
    let m = world(&mut n).step();
    deliver(&mut n, token, vec![m]);
    assert!(n.core.contexts().unit(4).is_stale());
    n.core.order(sleep(4)).unwrap();
    assert_eq!(n.core.overlay_len(), 0);
    assert_eq!(n.core.orders_in_flight(), 1, "the order goes out as usual");
    assert_eq!(n.core.stats().ctx_stale_at_order, 1);
    assert_eq!(n.core.shown().cloned(), n.core.base().cloned());
}

#[test]
fn a_second_order_to_the_same_unit_or_city_gets_no_picture() {
    let (mut n, _) = scripted();
    n.core.order(sleep(4)).unwrap();
    n.core.order(Command::UnitFortify { unit: 4 }).unwrap();
    assert_eq!(n.core.overlay_len(), 1);
    let u = shown_unit(&mut n, 4);
    assert!(u.flags.contains(UnitFlags::SLEEPING) && !u.flags.contains(UnitFlags::FORTIFIED));
    n.core
        .order(Command::CitySetProduction {
            city: 2,
            order: 0,
            item: 1,
        })
        .unwrap();
    n.core
        .order(Command::CitySetProduction {
            city: 2,
            order: 0,
            item: 1,
        })
        .unwrap();
    assert_eq!(n.core.overlay_len(), 2);
    let c = &n.core.shown().unwrap().cities[0];
    assert_eq!(
        (c.production_item, c.production_turns, c.production_needed),
        (1, 7, 80)
    );
    assert_eq!(n.core.base().unwrap().cities[0].production_item, -1);
}

#[test]
fn nothing_is_drawn_in_the_rivals_turn_or_for_what_the_table_leaves_out() {
    let (mut n, token) = scripted();
    n.core
        .order(Command::UnitRangeAttack {
            unit: 4,
            x: 2,
            y: 2,
        })
        .unwrap();
    n.core.order(Command::UnitFoundCity { unit: 6 }).unwrap();
    n.core.order(Command::DeclareWar { team: 1 }).unwrap();
    assert_eq!(n.core.overlay_len(), 0);
    // Hybrid: the rivals move.
    let mut ts = turn_state(false);
    ts.phase = TurnPhase::Ai { active: 2 };
    let m = world(&mut n).push(vec![GameMsg::TurnState(ts)]);
    deliver(&mut n, token, vec![m]);
    n.core.order(sleep(4)).unwrap();
    assert_eq!(n.core.overlay_len(), 0);
}

// ---------------------------------------------------------------- the keyframe wait and the guard

#[test]
fn the_record_stays_while_a_keyframe_is_awaited() {
    let (mut n, token) = scripted();
    n.srv.ignore_need_keyframe = true;
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    // A diff the client cannot apply: it asks for a keyframe and waits.
    let new = world(&mut n).mutated();
    let m = world(&mut n).step_damaged(new, Some(Damage::ViewHash));
    deliver(&mut n, token, vec![m]);
    assert_eq!(n.srv.need_keyframe.len(), 1);
    // The answer comes: the event and the line are there, the picture is not taken off (the unit must
    // not jump back before the view that explains it).
    let msgs = n.srv.answer_order(id, CommandOutcome::Applied);
    deliver(&mut n, token, msgs);
    assert!(n.has_event(|e| matches!(e, ClientEvent::OrderAnswered { .. })));
    assert_eq!(n.core.overlay_len(), 1);
    assert!(shown_unit(&mut n, 4).flags.contains(UnitFlags::SLEEPING));
    // The keyframe: the view is the server's and the record goes.
    let k = world(&mut n).keyframe(false);
    deliver(&mut n, token, vec![k]);
    assert_eq!(n.core.overlay_len(), 0);
    assert_eq!(n.core.shown().cloned(), n.core.base().cloned());
}

#[test]
fn the_guard_takes_a_record_off_after_15_s_with_a_line_and_the_order_stays_in_flight() {
    let (mut n, token) = scripted();
    let id = n.core.order(sleep(4)).unwrap();
    n.advance_coarse(14_900, 100);
    assert_eq!(n.core.overlay_len(), 1);
    n.advance_coarse(200, 100);
    assert_eq!(n.core.overlay_len(), 0);
    assert_eq!(n.core.orders_in_flight(), 1);
    assert_eq!(
        rolled_back(&n),
        [(id, "No answer from the server".to_string())]
    );
    assert_eq!(
        n.core.journal().last().unwrap().text,
        "No answer from the server"
    );
    assert_eq!(n.core.stats().layer_stuck, 1);
    assert!(
        shown_unit(&mut n, 4)
            .flags
            .contains(UnitFlags::NEEDS_ORDERS)
    );
    // A late `Applied` is no rollback: no second line.
    let lines = n.core.journal().len();
    server_applies(
        &mut n,
        token,
        id,
        |v| {
            let u = v.units.iter_mut().find(|u| u.id == 4).unwrap();
            u.flags = UnitFlags(UnitFlags::SLEEPING);
        },
        None,
    );
    assert_eq!(n.core.journal().len(), lines);
}

#[test]
fn the_guard_does_not_run_while_the_line_is_down() {
    let (mut n, _) = scripted();
    n.core.order(sleep(4)).unwrap();
    n.host_event(HostEvent::Offline);
    n.advance_coarse(40_000, 250);
    assert_eq!(n.core.overlay_len(), 1, "an outage is not a lost answer");
    assert!(rolled_back(&n).is_empty());
}

// ---------------------------------------------------------------- the end of the turn (NP-D23, NC-D13)

#[test]
fn end_turn_and_its_cancellation_show_in_the_turn_state_at_once() {
    let (mut n, token) = scripted();
    assert!(!finished(&n));
    let id = n.core.order(Command::EndTurn { force: false }).unwrap();
    assert!(finished(&n));
    assert_eq!(n.core.overlay_len(), 1);
    // The server's answer with its turn state: the button does not blink.
    let m = world(&mut n).push(vec![
        GameMsg::TurnState(turn_state(true)),
        GameMsg::Results(vec![CommandResult {
            id,
            outcome: CommandOutcome::Applied,
        }]),
    ]);
    n.srv.book.held.retain(|h| h.id != id);
    deliver(&mut n, token, vec![m]);
    assert!(finished(&n) && n.core.overlay_len() == 0);
    // The explicit cancel: `by_order: false`, once.
    n.take_events();
    let id = n.core.order(Command::CancelEndTurn).unwrap();
    assert!(!finished(&n));
    assert_eq!(
        cancelled(&n),
        Vec::<bool>::new(),
        "events come with the pump"
    );
    n.settle();
    assert_eq!(cancelled(&n), [false]);
    let m = world(&mut n).push(vec![
        GameMsg::TurnState(turn_state(false)),
        GameMsg::Results(vec![CommandResult {
            id,
            outcome: CommandOutcome::Applied,
        }]),
    ]);
    n.srv.book.held.retain(|h| h.id != id);
    deliver(&mut n, token, vec![m]);
    assert!(!finished(&n));
    assert_eq!(cancelled(&n), [false], "no second event");
    assert_eq!(n.core.stats().end_turn_cancelled_by_order, 0);
}

#[test]
fn an_order_after_our_own_end_of_turn_takes_it_back_once() {
    let (mut n, token) = scripted_with(None, true);
    assert!(finished(&n));
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    assert!(!finished(&n), "the layer shows the end of turn taken back");
    assert_eq!(cancelled(&n), [true]);
    assert_eq!(n.core.stats().end_turn_cancelled_by_order, 1);
    assert!(shown_unit(&mut n, 4).flags.contains(UnitFlags::SLEEPING));
    // `Applied` with the turn state (finished = false) in one batch: no blink, no second event.
    server_applies(
        &mut n,
        token,
        id,
        |v| {
            let u = v.units.iter_mut().find(|u| u.id == 4).unwrap();
            u.flags = UnitFlags(UnitFlags::SLEEPING);
        },
        Some(turn_state(false)),
    );
    assert!(!finished(&n));
    assert_eq!(cancelled(&n), [true]);
    assert_eq!(n.core.overlay_len(), 0);
}

#[test]
fn turn_ended_gives_the_end_of_turn_back_and_a_core_refusal_keeps_it_taken_back() {
    // `Rejected { TurnEnded }`: the core could not cancel, the turn is already passing.
    let (mut n, token) = scripted_with(None, true);
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    assert!(!finished(&n));
    let msgs = n.srv.answer_order(id, rejected(RejectCode::TurnEnded, ""));
    deliver(&mut n, token, msgs);
    assert!(finished(&n), "base never changed");
    assert_eq!(
        cancelled(&n),
        [true],
        "the event came with the picture, not with the refusal"
    );
    assert_eq!(
        n.core.journal().last().unwrap().text,
        "The turn is already ending"
    );

    // `Rejected { Core }` after a successful cancel: the batch carries finished = false, it stays.
    let (mut n, token) = scripted_with(None, true);
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    let mut m = match n
        .srv
        .answer_order(id, rejected(RejectCode::Core, "Not now"))
        .remove(0)
    {
        ServerMsg::Game(b) => b,
        _ => unreachable!(),
    };
    m.msgs.insert(0, GameMsg::TurnState(turn_state(false)));
    deliver(&mut n, token, vec![ServerMsg::Game(m)]);
    assert!(!finished(&n));
    assert_eq!(cancelled(&n), [true], "no second event");
    assert_eq!(n.core.journal().last().unwrap().text, "Not now");
}

#[test]
fn an_unpredicted_order_after_our_end_of_turn_changes_the_button_with_its_answer() {
    let (mut n, token) = scripted_with(None, true);
    let id = n
        .core
        .order(Command::UnitRangeAttack {
            unit: 4,
            x: 2,
            y: 1,
        })
        .unwrap();
    n.settle();
    assert_eq!(n.core.overlay_len(), 0);
    assert!(finished(&n), "no picture, the layer leaves the flag alone");
    assert!(cancelled(&n).is_empty());
    server_applies(
        &mut n,
        token,
        id,
        |v| {
            let u = v.units.iter_mut().find(|u| u.id == 4).unwrap();
            u.hp = 7;
        },
        Some(turn_state(false)),
    );
    assert!(!finished(&n));
    assert_eq!(cancelled(&n), [true]);
}

#[test]
fn a_new_turn_is_not_a_cancellation() {
    let (mut n, token) = scripted_with(None, true);
    let m = world(&mut n).push(vec![GameMsg::TurnState(turn_state(false))]);
    deliver(&mut n, token, vec![m]);
    assert!(!finished(&n));
    assert!(
        cancelled(&n).is_empty(),
        "no order was applied: not by an order"
    );
}

#[test]
fn a_blocker_the_layer_lifted_is_not_shown_and_comes_back_with_a_refusal() {
    let blocker = Blocker {
        kind: 3,
        target: BlockerTarget::Unit(4),
    };
    let (mut n, token) = scripted_with(Some(blocker), false);
    assert_eq!(n.core.shown().unwrap().blocker, Some(blocker));
    // Another unit's order: the blocker's unit still waits.
    n.core.order(sleep(6)).unwrap();
    assert_eq!(n.core.shown().unwrap().blocker, Some(blocker));
    let id = n.core.order(sleep(4)).unwrap();
    n.settle();
    assert_eq!(
        n.core.shown().unwrap().blocker,
        None,
        "the button says Next turn"
    );
    assert_eq!(
        n.core.base().unwrap().blocker,
        Some(blocker),
        "base keeps what the server said"
    );
    // The server refuses: the picture and the lifted blocker go together.
    let msgs = n.srv.answer_order(id, rejected(RejectCode::Core, "no"));
    deliver(&mut n, token, msgs);
    assert_eq!(n.core.shown().unwrap().blocker, Some(blocker));
}

#[test]
fn a_production_blocker_is_lifted_by_a_chosen_item() {
    let blocker = Blocker {
        kind: 2,
        target: BlockerTarget::City(2),
    };
    let (mut n, _) = scripted_with(Some(blocker), false);
    n.core
        .order(Command::CitySetProduction {
            city: 2,
            order: 0,
            item: 1,
        })
        .unwrap();
    assert_eq!(n.core.shown().unwrap().blocker, None);
}

#[test]
fn leaving_the_game_clears_the_layer() {
    let (mut n, token) = scripted();
    n.core.order(sleep(4)).unwrap();
    assert_eq!(n.core.overlay_len(), 1);
    deliver(
        &mut n,
        token,
        vec![ServerMsg::Detached {
            reason: protocol::DetachReason::GameOver,
        }],
    );
    assert_eq!(n.core.overlay_len(), 0);
    assert!(n.core.shown().is_none());
}

// ---------------------------------------------------------------- the AI turn queue (SIMP-exe-820, SIMP-exe-790)

#[test]
fn the_ai_turn_log_grows_within_a_turn_and_the_next_turn_replaces_it() {
    use protocol::{AiTurn, AiTurnEvent};
    let (mut n, token) = scripted();
    assert!(n.core.ai_turn().is_none());
    let ev = |player, ended| AiTurnEvent { player, ended };
    let part = |turn, events| AiTurn { turn, events };
    // The AI plays: the server says which civilizations it started, then which ended, in pieces.
    let mut ts = turn_state(true);
    ts.phase = TurnPhase::Ai { active: 0 };
    let m = world(&mut n).push(vec![
        GameMsg::TurnState(ts),
        GameMsg::AiTurn(part(1, vec![ev(2, false), ev(3, false)])),
    ]);
    deliver(&mut n, token, vec![m]);
    assert_eq!(
        n.core.ai_turn(),
        Some(&part(1, vec![ev(2, false), ev(3, false)]))
    );
    let m = world(&mut n).push(vec![GameMsg::AiTurn(part(1, vec![ev(2, true)]))]);
    deliver(&mut n, token, vec![m]);
    let m = world(&mut n).push(vec![GameMsg::AiTurn(part(1, vec![ev(3, true)]))]);
    deliver(&mut n, token, vec![m]);
    assert_eq!(
        n.core.ai_turn(),
        Some(&part(
            1,
            vec![ev(2, false), ev(3, false), ev(2, true), ev(3, true)]
        )),
        "in order, nothing dropped"
    );
    // The humans are up: the log stays (the UI may not have taken its last events yet) and names its turn.
    let m = world(&mut n).push(vec![GameMsg::TurnState(turn_state(false))]);
    deliver(&mut n, token, vec![m]);
    assert_eq!(n.core.ai_turn().map(|l| l.turn), Some(1));
    // The next turn's AI phase starts a new log.
    let m = world(&mut n).push(vec![GameMsg::AiTurn(part(2, vec![ev(3, false)]))]);
    deliver(&mut n, token, vec![m]);
    assert_eq!(n.core.ai_turn(), Some(&part(2, vec![ev(3, false)])));
}
