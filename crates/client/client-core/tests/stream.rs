//! The game stream (net-client 4.3, 9): batches by `seq`, keyframe and diffs with hashes, `NeedKeyframe`,
//! the ladder, the map basis, turn state, seats, chat and journal, on the scripted server with a game.

mod common;

use client_core::{BreakWhy, ClientEvent, ClientPhase, ConnState};
use common::world::{Damage, World};
use common::*;
use protocol::{
    AttachMode, ChatLine, ChatTo, ClientMsg, GameMsg, JournalEntry, JournalKind, KeyframeReason,
    SeatTurn, ServerMsg, TimerState, Token, TurnMode, TurnPhase, TurnState, ViewDiff, diff,
};

fn plan(stream: u64) -> GamePlan {
    GamePlan {
        game: 7,
        player: 1,
        stream,
        last_cmd_id: 0,
        last_seq: 0,
    }
}

/// Attached to game 7 with the keyframe and the basis in, catalog loaded.
fn game() -> (Net, Token) {
    let mut n = Net::start();
    n.srv.world = Some(World::new(7, 11, 6, 5));
    n.srv.plan = Some(plan(3));
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    n.core.set_catalog(protocol::Catalog::default());
    (n, token)
}

fn world(n: &mut Net) -> &mut World {
    n.srv.world.as_mut().unwrap()
}

/// The client shows exactly what the server's game is.
fn in_sync(n: &mut Net) -> bool {
    let (view, hash, basis) = {
        let w = n.srv.world.as_ref().unwrap();
        (w.view.clone(), w.view_hash(), w.basis_hash())
    };
    n.core.shown() == Some(&view)
        && n.core.base_hash() == Some(hash)
        && n.core.basis_hash() == Some(basis)
}

fn send(n: &mut Net, token: Token, m: ServerMsg) {
    n.push(token, m);
    n.settle();
}

fn step(n: &mut Net, token: Token) {
    let m = world(n).step();
    send(n, token, m);
}

fn need_keyframes(n: &Net) -> Vec<KeyframeReason> {
    n.srv.need_keyframe.clone()
}

// ---------------------------------------------------------------- attach, keyframe, diffs

#[test]
fn fresh_attach_brings_the_basis_and_the_keyframe_in_the_first_batch() {
    let (mut n, _) = game();
    assert!(in_sync(&mut n));
    assert_eq!(n.core.phase(), ClientPhase::Playing);
    assert_eq!(n.core.view_seq(), 1);
    assert_eq!(n.core.revision(), 1);
    let s = n.core.stats();
    assert_eq!((s.keyframes, s.basis_msgs, s.diffs), (1, 1, 0));
    assert!(s.basis_bytes > 0);
    // The screens learn it from the change: everything, and a whole new basis.
    let c = n.core.take_change();
    assert!(c.reset && c.dirty_plots.is_none() && c.dirty_units.is_none());
    assert!(c.basis_plots.is_none());
    assert_eq!(c.sections.0 & 0x3FF, 0x3FF);
    assert!(n.core.take_change().is_empty());
    assert_eq!(n.core.basis().map(|b| b.plots.len()), Some(30));
    assert!(n.has_event(|e| matches!(e, ClientEvent::ViewChanged)));
}

#[test]
fn diffs_apply_in_order_and_the_change_says_what_moved() {
    let (mut n, token) = game();
    n.core.take_change();
    for _ in 0..30 {
        let old = world(&mut n).view.clone();
        let new = world(&mut n).mutated();
        let expect: ViewDiff = diff(&old, &new, None);
        let m = world(&mut n).step_to(new);
        send(&mut n, token, m);
        assert!(in_sync(&mut n));
        let c = n.core.take_change();
        if expect.is_empty() {
            continue;
        }
        assert_eq!(c.sections, expect.touched());
        let plots: Vec<u32> = expect.plots.iter().map(|(i, _)| *i).collect();
        assert_eq!(c.dirty_plots, Some(plots));
        let keys = c.dirty_units.expect("units are listed by key");
        let up: Vec<(i8, i32)> = expect
            .units
            .upsert
            .iter()
            .map(|u| (u.owner, u.id))
            .collect();
        assert_eq!(keys.upsert, up);
        assert_eq!(keys.remove, expect.units.remove);
        assert!(!c.reset && c.basis_plots == Some(vec![]));
    }
    assert_eq!(n.core.stats().need_keyframe.total(), 0);
    assert_eq!(n.core.stats().diffs, 30);
}

#[test]
fn a_basis_diff_changes_the_basis_not_the_view_hash() {
    let (mut n, token) = game();
    n.core.take_change();
    let view_hash = n.core.base_hash();
    let basis_hash = n.core.basis_hash();
    let m = world(&mut n).basis_step(None);
    send(&mut n, token, m);
    assert!(in_sync(&mut n));
    assert_eq!(
        n.core.base_hash(),
        view_hash,
        "the basis is not in the view hash"
    );
    assert_ne!(n.core.basis_hash(), basis_hash);
    let c = n.core.take_change();
    assert!(c.sections.contains(8) && !c.sections.contains(1));
    assert!(matches!(&c.basis_plots, Some(v) if !v.is_empty()));
    assert_eq!(c.dirty_plots, Some(vec![]));
    // The claim for the next login follows the basis.
    assert_eq!(n.core.stats().need_keyframe.total(), 0);
}

#[test]
fn duplicates_are_ignored() {
    let (mut n, token) = game();
    step(&mut n, token);
    let again = world(&mut n).log[1].clone();
    let rev = n.core.revision();
    send(&mut n, token, again.clone());
    let first = world(&mut n).log[0].clone();
    send(&mut n, token, first);
    assert_eq!(n.core.stats().dup_batches, 2);
    assert_eq!(n.core.revision(), rev);
    assert!(in_sync(&mut n));
    assert_eq!(n.core.attach_info().unwrap().last_seq, 2);
}

#[test]
fn a_skipped_seq_breaks_the_stream_and_the_tail_is_replayed() {
    let (mut n, token) = game();
    step(&mut n, token);
    world(&mut n).lose_one();
    let m = world(&mut n).step();
    n.take_events();
    send(&mut n, token, m);
    assert!(n.has_event(|e| matches!(e, ClientEvent::StreamBroken(BreakWhy::Gap))));
    n.advance(2_000);
    let (_, login) = n.srv.logins.last().unwrap();
    assert_eq!(login.resume.map(|r| (r.stream, r.last_seq)), Some((3, 2)));
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Replay,
            ..
        }
    )));
    assert!(
        in_sync(&mut n),
        "the lost batch and the next one came as a tail"
    );
    assert_eq!(n.core.stats().need_keyframe.total(), 0);
    assert_eq!(n.core.conn(), ConnState::Online);
}

// ---------------------------------------------------------------- failures of the view

#[test]
fn every_damaged_diff_gives_one_need_keyframe_and_keeps_the_view() {
    for (damage, reason) in [
        (Damage::ViewHash, KeyframeReason::HashMismatch),
        (Damage::Base, KeyframeReason::BaseMismatch),
        (Damage::OutOfRange, KeyframeReason::ApplyError),
        (Damage::UnknownKey, KeyframeReason::ApplyError),
    ] {
        let (mut n, token) = game();
        step(&mut n, token);
        n.srv.ignore_need_keyframe = true;
        let shown_before = n.core.shown().cloned();
        let hash_before = n.core.base_hash();
        let basis_before = n.core.basis_hash();
        let new = world(&mut n).mutated();
        let m = world(&mut n).step_damaged(new, Some(damage));
        send(&mut n, token, m);
        assert_eq!(need_keyframes(&n), [reason], "{damage:?}");
        // The previous view stays, bit for bit.
        assert_eq!(n.core.shown().cloned(), shown_before, "{damage:?}");
        assert_eq!(n.core.base_hash(), hash_before);
        assert_eq!(n.core.basis_hash(), basis_before);
        if damage == Damage::ViewHash {
            assert_eq!(
                n.core.stats().undo_restores,
                1,
                "the diff was applied and put back"
            );
        }
        // Until the keyframe comes the diffs are not applied and no second request goes out.
        step(&mut n, token);
        step(&mut n, token);
        assert_eq!(n.core.stats().diffs_skipped, 2, "{damage:?}");
        assert_eq!(need_keyframes(&n).len(), 1, "once per case");
        assert_eq!(n.core.shown().cloned(), shown_before);
        // The keyframe heals; the connection never dropped.
        let kf = world(&mut n).answer(reason);
        send(&mut n, token, kf);
        assert!(in_sync(&mut n), "{damage:?}");
        step(&mut n, token);
        assert!(in_sync(&mut n));
        assert_eq!(n.core.conn(), ConnState::Online);
        assert_eq!(n.core.stats().stream_breaks, 0);
    }
}

/// `fault corruptView` of the test port (4.12): the hash kept for `base` is spoiled, so the next diff finds
/// it apart from the server's, and the keyframe the client asks for heals it without a reconnect.
#[test]
fn a_view_spoiled_by_the_test_port_asks_for_a_keyframe_at_the_next_diff() {
    let (mut n, token) = game();
    step(&mut n, token);
    assert!(need_keyframes(&n).is_empty());
    n.core.debug_corrupt_view();
    for _ in 0..5 {
        step(&mut n, token);
        if !need_keyframes(&n).is_empty() {
            break;
        }
    }
    assert!(!need_keyframes(&n).is_empty(), "the next diff must notice");
    assert!(in_sync(&mut n), "the keyframe healed it");
    assert_eq!(n.core.conn(), ConnState::Online);
    assert_eq!(n.core.stats().stream_breaks, 0);
}

#[test]
fn a_damaged_diff_heals_within_one_round_trip_when_the_server_answers() {
    let (mut n, token) = game();
    n.one_way = 100;
    n.advance(300);
    let new = world(&mut n).mutated();
    let m = world(&mut n).step_damaged(new, Some(Damage::ViewHash));
    n.push(token, m);
    let t0 = n.now;
    let healed = n.advance_until(1_000, |n| {
        let w = n.srv.world.as_ref().unwrap();
        n.core.base_hash() == Some(w.view_hash()) && !n.srv.need_keyframe.is_empty()
    });
    assert!(healed);
    // The diff needs one way, the request one way, the keyframe one way back: three legs, no more.
    assert!(
        n.now - t0 <= 100 + 200 + 20,
        "healed after {} ms",
        n.now - t0
    );
    assert_eq!(need_keyframes(&n), [KeyframeReason::HashMismatch]);
}

#[test]
fn a_diff_with_a_wrong_basis_hash_puts_the_basis_back() {
    let (mut n, token) = game();
    n.srv.ignore_need_keyframe = true;
    let (view, hash, basis) = (
        n.core.shown().cloned(),
        n.core.base_hash(),
        n.core.basis_hash(),
    );
    let m = world(&mut n).basis_step(Some(Damage::BasisHash));
    send(&mut n, token, m);
    assert_eq!(need_keyframes(&n), [KeyframeReason::BasisMismatch]);
    assert_eq!(n.core.shown().cloned(), view);
    assert_eq!(n.core.base_hash(), hash);
    assert_eq!(
        n.core.basis_hash(),
        basis,
        "the basis went back with the undo log"
    );
    assert_eq!(n.core.stats().undo_restores, 1);
    // The answer to a basis mismatch is the pair.
    n.srv.ignore_need_keyframe = false;
    let kf = world(&mut n).answer(KeyframeReason::BasisMismatch);
    send(&mut n, token, kf);
    assert!(in_sync(&mut n));
    assert_eq!(n.core.stats().basis_msgs, 2);
}

#[test]
fn a_keyframe_naming_another_basis_asks_for_the_pair() {
    let (mut n, token) = game();
    let old = n.core.shown().cloned();
    let mut kf = world(&mut n).keyframe(false);
    if let ServerMsg::Game(b) = &mut kf
        && let GameMsg::Keyframe(k) = &mut b.msgs[0]
    {
        k.basis_hash ^= 1;
    }
    n.srv.ignore_need_keyframe = true;
    send(&mut n, token, kf);
    assert_eq!(need_keyframes(&n), [KeyframeReason::BasisMismatch]);
    assert_eq!(n.core.shown().cloned(), old);
    n.srv.ignore_need_keyframe = false;
    let pair = world(&mut n).answer(KeyframeReason::BasisMismatch);
    send(&mut n, token, pair);
    assert!(in_sync(&mut n));
}

#[test]
fn a_basis_that_does_not_match_its_hash_is_refused_with_its_keyframe() {
    let (mut n, token) = game();
    let old = n.core.shown().cloned();
    n.srv.ignore_need_keyframe = true;
    let mut b = world(&mut n).map_basis();
    b.plots[3].terrain = b.plots[3].terrain.wrapping_add(1);
    let kf = world(&mut n).keyframe(false);
    let ServerMsg::Game(mut batch) = kf else {
        panic!()
    };
    batch.msgs.insert(0, GameMsg::MapBasis(b));
    send(&mut n, token, ServerMsg::Game(batch));
    assert_eq!(need_keyframes(&n), [KeyframeReason::BasisMismatch]);
    assert_eq!(
        n.core.shown().cloned(),
        old,
        "its keyframe was not applied either"
    );
    assert!(n.core.basis().is_some());
}

#[test]
fn a_keyframe_whose_hash_is_wrong_is_asked_for_again_and_a_second_failure_breaks_the_stream() {
    let (mut n, token) = game();
    n.srv.ignore_need_keyframe = true;
    let old = n.core.shown().cloned();
    let bad = |n: &mut Net| {
        let kf = world(n).keyframe(false);
        let ServerMsg::Game(mut batch) = kf else {
            panic!()
        };
        if let GameMsg::Keyframe(k) = &mut batch.msgs[0] {
            k.hash ^= 1;
        }
        ServerMsg::Game(batch)
    };
    let m = bad(&mut n);
    send(&mut n, token, m);
    assert_eq!(need_keyframes(&n), [KeyframeReason::HashMismatch]);
    assert_eq!(n.core.shown().cloned(), old);
    // The answer is bad too: the stream is given up, not asked for in a loop.
    let m = bad(&mut n);
    n.take_events();
    send(&mut n, token, m);
    assert!(n.has_event(|e| matches!(e, ClientEvent::StreamBroken(BreakWhy::KeyframeFailed))));
    assert_eq!(need_keyframes(&n).len(), 1);
}

#[test]
fn a_diff_with_nothing_to_apply_it_to_asks_for_a_keyframe() {
    let (mut n, token) = game();
    n.srv.ignore_need_keyframe = true;
    // A Fresh attach whose first batch is a diff.
    world(&mut n).reset_stream();
    n.push(
        token,
        ServerMsg::Attached(protocol::Attached {
            game: 7,
            player: 1,
            stream: 3,
            mode: AttachMode::Fresh,
            catalog: catalog_ref(),
            last_cmd_id: 0,
        }),
    );
    let m = world(&mut n).step();
    send(&mut n, token, m);
    assert!(n.core.shown().is_none());
    assert_eq!(need_keyframes(&n), [KeyframeReason::BaseMismatch]);
    assert_eq!(n.core.phase(), ClientPhase::Loading);
}

// ---------------------------------------------------------------- basis across reconnects and games

#[test]
fn a_fresh_attach_with_the_same_basis_comes_without_it() {
    let (mut n, token) = game();
    step(&mut n, token);
    assert_eq!(n.core.stats().basis_msgs, 1);
    // The gateway restarted: a new stream, the same game. The claim in the login names our basis.
    n.srv.plan = Some(plan(4));
    n.server_drops(token, 1012);
    n.advance(3_000);
    let (_, login) = n.srv.logins.last().unwrap();
    let claim = login.resume.and_then(|r| r.basis).expect("a claim");
    assert_eq!((claim.game, claim.hash), (7, n.core.basis_hash().unwrap()));
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Fresh,
            ..
        }
    )));
    assert_eq!(n.core.stats().basis_msgs, 1, "no MapBasis the second time");
    assert!(in_sync(&mut n));
    assert_eq!(n.core.view_seq(), 1);
    let c = n.core.take_change();
    assert!(c.reset);
    assert_eq!(n.core.stats().need_keyframe.total(), 0);
}

#[test]
fn a_reloaded_page_has_no_claim_and_gets_the_basis() {
    let (n, token) = game();
    let mut m = Net::start();
    m.srv.tokens = n.srv.tokens.clone();
    m.srv.world = Some(World::new(7, 11, 6, 5));
    m.srv.plan = Some(plan(3));
    m.core.set_token(Some(token));
    m.connect();
    let (_, login) = m.srv.logins.last().unwrap();
    assert!(login.resume.is_none());
    assert_eq!(m.core.stats().basis_msgs, 1);
    assert_eq!(m.core.stats().keyframes, 1);
    assert!(in_sync(&mut m));
}

#[test]
fn another_game_replaces_the_basis_and_the_claim() {
    let (mut n, token) = game();
    n.push(
        token,
        ServerMsg::Detached {
            reason: protocol::DetachReason::Left,
        },
    );
    n.settle();
    assert!(n.core.shown().is_none(), "nothing of the old game is shown");
    let mut w = World::new(8, 99, 4, 4);
    w.reset_stream();
    n.srv.world = Some(w);
    n.srv.plan = Some(GamePlan { game: 8, ..plan(1) });
    n.push(
        token,
        ServerMsg::Attached(protocol::Attached {
            game: 8,
            player: 1,
            stream: 1,
            mode: AttachMode::Fresh,
            catalog: catalog_ref(),
            last_cmd_id: 0,
        }),
    );
    let kf = world(&mut n).keyframe(true);
    send(&mut n, token, kf);
    assert!(in_sync(&mut n));
    assert_eq!(
        n.core.basis().map(|b| (b.game, b.plots.len())),
        Some((8, 16))
    );
    let c = n.core.take_change();
    assert!(c.reset && c.basis_plots.is_none());
}

#[test]
fn a_basis_of_another_game_is_refused() {
    let (mut n, token) = game();
    n.srv.ignore_need_keyframe = true;
    let mut b = world(&mut n).map_basis();
    b.game = 9;
    let kf = world(&mut n).keyframe(false);
    let ServerMsg::Game(mut batch) = kf else {
        panic!()
    };
    batch.msgs.insert(0, GameMsg::MapBasis(b));
    send(&mut n, token, ServerMsg::Game(batch));
    assert_eq!(need_keyframes(&n), [KeyframeReason::BasisMismatch]);
}

// ---------------------------------------------------------------- ladder and waiting

#[test]
fn the_ladder_resumes_then_starts_over_then_gives_up() {
    let (mut n, token) = game();
    let gap = |n: &mut Net, token: Token| {
        world(n).lose_one();
        let m = world(n).step();
        n.push(token, m);
        n.settle();
    };
    // First: reconnect with `resume`.
    gap(&mut n, token);
    n.advance(2_000);
    assert!(n.srv.logins.last().unwrap().1.resume.is_some());
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Replay,
            ..
        }
    )));
    assert!(in_sync(&mut n));
    // Second within a minute: without `resume`, a fresh stream.
    let token = n.core.token().unwrap();
    gap(&mut n, token);
    n.advance(2_000);
    assert!(n.srv.logins.last().unwrap().1.resume.is_none());
    assert_eq!(n.core.stats().attached_fresh, 2);
    assert!(in_sync(&mut n));
    // Third: Fatal, and nothing reconnects.
    gap(&mut n, token);
    let opens = n.opens.len();
    n.advance(120_000);
    assert_eq!(n.opens.len(), opens);
    assert!(matches!(n.core.phase(), ClientPhase::Fatal(_)));
    assert_eq!(n.core.conn(), ConnState::Closed);
    assert!(n.has_event(|e| matches!(e, ClientEvent::Fatal(_))));
    assert_eq!(n.core.stats().stream_breaks, 3);
}

#[test]
fn breaks_a_minute_apart_start_the_ladder_over() {
    let (mut n, token) = game();
    world(&mut n).lose_one();
    let m = world(&mut n).step();
    send(&mut n, token, m);
    n.advance(61_000);
    let token = n.core.token().unwrap();
    world(&mut n).lose_one();
    let m = world(&mut n).step();
    send(&mut n, token, m);
    n.advance(2_000);
    assert!(
        n.srv.logins.last().unwrap().1.resume.is_some(),
        "the first break of a new minute resumes again"
    );
    assert!(!matches!(n.core.phase(), ClientPhase::Fatal(_)));
}

#[test]
fn an_unreadable_frame_breaks_the_stream() {
    let (mut n, _) = game();
    let sock = n.open_socks()[0];
    n.take_events();
    n.core.on_bytes(n.now, sock, &[0, 0xFF, 0xFF, 0xFF]);
    n.settle();
    assert!(n.has_event(|e| matches!(e, ClientEvent::StreamBroken(BreakWhy::Undecodable))));
    n.advance(2_000);
    assert!(in_sync(&mut n));
    assert_eq!(n.core.stats().decode_errors, 1);
}

#[test]
fn an_unanswered_need_keyframe_is_repeated_once_then_the_stream_starts_over() {
    let (mut n, token) = game();
    n.srv.ignore_need_keyframe = true;
    let new = world(&mut n).mutated();
    let m = world(&mut n).step_damaged(new, Some(Damage::ViewHash));
    send(&mut n, token, m);
    assert_eq!(need_keyframes(&n).len(), 1);
    n.advance(4_900);
    assert_eq!(need_keyframes(&n).len(), 1);
    n.advance(300);
    assert_eq!(
        need_keyframes(&n).len(),
        2,
        "one repeat after keyframe_wait_ms"
    );
    n.take_events();
    n.advance(5_300);
    assert!(n.has_event(|e| matches!(e, ClientEvent::StreamBroken(BreakWhy::KeyframeTimeout))));
    n.advance(2_000);
    let (_, login) = n.srv.logins.last().unwrap();
    assert!(login.resume.is_none(), "start over, no resume");
    assert!(in_sync(&mut n));
}

#[test]
fn a_keyframe_awaited_across_a_reconnect_is_asked_for_again() {
    let (mut n, token) = game();
    n.srv.ignore_need_keyframe = true;
    let new = world(&mut n).mutated();
    let m = world(&mut n).step_damaged(new, Some(Damage::ViewHash));
    send(&mut n, token, m);
    assert_eq!(need_keyframes(&n).len(), 1);
    // The line drops; the request may have died with it. After the Replay the server answers.
    n.srv.ignore_need_keyframe = false;
    n.server_drops(token, 1006);
    n.advance(2_000);
    assert!(n.has_event(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Replay,
            ..
        }
    )));
    assert_eq!(need_keyframes(&n).len(), 2);
    assert!(in_sync(&mut n));
}

// ---------------------------------------------------------------- the rest of a batch

#[test]
fn turn_state_seats_chat_and_journal_follow_the_batches() {
    let (mut n, token) = game();
    n.advance(3_000); // a few pongs: the clock is known
    let server_now = n.core.server_ms(n.now);
    let ts = |deadline: u64| TurnState {
        turn: 4,
        phase: TurnPhase::Humans,
        mode: TurnMode::Simultaneous,
        seats: vec![
            SeatTurn {
                player: 0,
                finished: true,
                may_act: true,
                group: 0,
            },
            SeatTurn {
                player: 1,
                finished: false,
                may_act: true,
                group: 2,
            },
        ],
        timers: vec![
            TimerState {
                group: 0,
                deadline_ms: 1,
                len_ms: 1,
                left_ms_when_stopped: None,
            },
            TimerState {
                group: 2,
                deadline_ms: deadline,
                len_ms: 60_000,
                left_ms_when_stopped: None,
            },
        ],
        paused: false,
    };
    let line = |text: &str| ChatLine {
        from: "bob".into(),
        from_player: Some(0),
        to: ChatTo::All,
        text: text.into(),
        server_ms: 123,
    };
    let entry = |text: &str| JournalEntry {
        turn: 4,
        kind: JournalKind::Info,
        text: text.into(),
        at: None,
        simplification: None,
    };
    let m = world(&mut n).push(vec![
        GameMsg::TurnState(ts(server_now + 30_000)),
        GameMsg::Seats(vec![seat(0, "bob", true), seat(1, "alice", false)]),
        GameMsg::Chat(vec![line("hi"), line("there")]),
        GameMsg::Journal(vec![entry("one")]),
    ]);
    let received = n.now;
    send(&mut n, token, m);
    assert_eq!(n.core.turn_state().map(|t| t.turn), Some(4));
    assert_eq!(n.core.seats().len(), 2);
    let chat = n.core.chat_lines();
    assert_eq!(chat.len(), 2);
    assert!(
        chat.iter().all(|c| c.received_ms == received),
        "the client's own clock is shown"
    );
    assert!(chat.iter().all(|c| c.line.server_ms == 123));
    assert_eq!(n.core.journal().len(), 1);
    // Our seat is player 1, group 2: its timer is the second one.
    let left = n.core.timer_left_ms(n.now).expect("a timer");
    assert!((29_900..=30_000).contains(&left), "{left}");
    n.advance(1_000);
    let left = n.core.timer_left_ms(n.now).unwrap();
    assert!((28_900..=29_000).contains(&left), "{left}");

    // Whole replacement for turn state and seats; the rest is appended.
    let m = world(&mut n).push(vec![
        GameMsg::TurnState(TurnState { turn: 5, ..ts(0) }),
        GameMsg::Seats(vec![seat(1, "alice", true)]),
        GameMsg::Chat(vec![line("again")]),
        GameMsg::Journal(vec![entry("two"), entry("three")]),
    ]);
    send(&mut n, token, m);
    assert_eq!(n.core.turn_state().map(|t| t.turn), Some(5));
    assert_eq!(n.core.seats().len(), 1);
    assert_eq!(n.core.chat_lines().len(), 3);
    assert_eq!(n.core.journal().len(), 3);
    // A stopped timer shows what was left.
    let mut stopped = ts(0);
    stopped.timers[1].left_ms_when_stopped = Some(12_345);
    let m = world(&mut n).push(vec![GameMsg::TurnState(stopped)]);
    send(&mut n, token, m);
    assert_eq!(n.core.timer_left_ms(n.now), Some(12_345));
}

#[test]
fn a_fresh_attach_starts_journal_chat_and_seats_over() {
    let (mut n, token) = game();
    let m = world(&mut n).push(vec![
        GameMsg::Seats(vec![seat(1, "alice", true)]),
        GameMsg::Chat(vec![ChatLine {
            from: "x".into(),
            from_player: None,
            to: ChatTo::All,
            text: "t".into(),
            server_ms: 1,
        }]),
        GameMsg::Journal(vec![JournalEntry {
            turn: 1,
            kind: JournalKind::Info,
            text: "j".into(),
            at: None,
            simplification: None,
        }]),
    ]);
    send(&mut n, token, m);
    assert_eq!(
        (
            n.core.seats().len(),
            n.core.chat_lines().len(),
            n.core.journal().len()
        ),
        (1, 1, 1)
    );
    n.srv.plan = Some(plan(4));
    n.server_drops(token, 1012);
    n.advance(3_000);
    assert!(
        n.core.seats().is_empty() && n.core.chat_lines().is_empty() && n.core.journal().is_empty()
    );
    assert!(n.core.turn_state().is_none());
    assert!(in_sync(&mut n));
}

#[test]
fn results_and_contexts_do_no_harm_before_their_modules_exist() {
    let (mut n, token) = game();
    let m = world(&mut n).push(vec![
        GameMsg::Results(vec![protocol::CommandResult {
            id: 99,
            outcome: protocol::CommandOutcome::Applied,
        }]),
        GameMsg::Context(protocol::ContextMsg::Dropped { unit: 5 }),
    ]);
    send(&mut n, token, m);
    assert!(in_sync(&mut n));
    assert_eq!(n.core.revision(), 2);
    let _ = ClientMsg::Ping {
        seq: 0,
        client_ms: 0,
    };
}
