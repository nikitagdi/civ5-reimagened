//! The checks of the bot's protocol brain on synthetic streams: `seq` in a row, `base`, the view hash after
//! `apply`, the map basis, the order inside a batch, the knobs of a raw peer.

use std::time::{Duration, Instant};

use net_test::bot::conn::Frame;
use net_test::bot::session::{Effect, Session};
use protocol::{
    AttachMode, Attached, CatalogRef, ClientMsg, CommandOutcome, CommandResult, GameBatch, GameMsg,
    Keyframe, KeyframeReason, MapBasis, MapView, PlotBasisView, PlotView, SeatTurn, ServerMsg,
    TopBar, TurnMode, TurnPhase, TurnState, TurnStatus, ViewDelta, ViewHash, WorldView,
    basis_hash_of, diff,
};

fn basis() -> MapBasis {
    let mut b = MapBasis {
        game: 1,
        basis_hash: 0,
        width: 2,
        height: 1,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 5,
        plots: vec![PlotBasisView::default(); 2],
    };
    b.basis_hash = basis_hash_of(&b);
    b
}

fn world(turn: i32, gold: i32) -> WorldView {
    WorldView {
        turn,
        year: -4000,
        human: 0,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker: None,
        map: MapView {
            width: 2,
            height: 1,
            wrap_x: false,
            wrap_y: false,
            plots: vec![PlotView::default(); 2],
            terrain_seed: 5,
        },
        units: vec![],
        cities: vec![],
        players: vec![],
        top_bar: TopBar {
            gold,
            ..TopBar::default()
        },
        notifications: vec![],
        peace_offers: vec![],
        trade_routes: vec![],
    }
}

fn turn_state(turn: i32) -> TurnState {
    TurnState {
        turn,
        phase: TurnPhase::Humans,
        mode: TurnMode::Simultaneous,
        seats: vec![SeatTurn {
            player: 0,
            finished: false,
            may_act: true,
            group: 0,
        }],
        timers: vec![],
        paused: false,
    }
}

fn frame(msg: ServerMsg) -> Frame {
    Frame {
        msg,
        wire_bytes: 100,
        at: Instant::now(),
    }
}

fn batch(seq: u64, msgs: Vec<GameMsg>) -> Frame {
    frame(ServerMsg::Game(GameBatch { seq, msgs }))
}

fn keyframe(v: &WorldView) -> GameMsg {
    GameMsg::Keyframe(Keyframe {
        view: v.clone(),
        hash: ViewHash::of(v).value(),
        basis_hash: basis().basis_hash,
    })
}

fn delta(base: u64, old: &WorldView, new: &WorldView, hash: u64) -> GameMsg {
    GameMsg::Diff(ViewDelta {
        base,
        diff: diff(old, new, None),
        hash,
        basis_hash: None,
    })
}

fn session() -> Session {
    let mut s = Session::new("bot", Instant::now());
    s.handle(&frame(ServerMsg::Attached(Attached {
        game: 1,
        player: 0,
        stream: 9,
        mode: AttachMode::Fresh,
        catalog: CatalogRef { hash: 0, len: 0 },
        last_cmd_id: 0,
    })));
    s
}

/// A session after the first batch: basis, keyframe of turn 1, turn state.
fn started() -> (Session, WorldView) {
    let mut s = session();
    let v = world(1, 100);
    let fx = s.handle(&batch(
        1,
        vec![
            GameMsg::MapBasis(basis()),
            keyframe(&v),
            GameMsg::TurnState(turn_state(1)),
        ],
    ));
    assert!(
        !fx.iter()
            .any(|e| matches!(e, Effect::Send(_) | Effect::Reconnect(_)))
    );
    (s, v)
}

fn sends(fx: &[Effect]) -> Vec<&ClientMsg> {
    fx.iter()
        .filter_map(|e| {
            if let Effect::Send(m) = e {
                Some(&**m)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn a_clean_stream_is_applied_and_every_hash_is_checked() {
    let (mut s, v1) = started();
    assert_eq!(
        s.view.as_ref().unwrap().1.value(),
        ViewHash::of(&v1).value()
    );
    assert!(s.may_act());
    let v2 = world(1, 120);
    let fx = s.handle(&batch(
        2,
        vec![delta(1, &v1, &v2, ViewHash::of(&v2).value())],
    ));
    assert!(sends(&fx).is_empty());
    assert_eq!(s.view.as_ref().unwrap().0.top_bar.gold, 120);
    assert_eq!((s.stats.hash_checked, s.stats.hash_mismatch), (2, 0));
    assert!(s.stats.errors.is_empty(), "{:?}", s.stats.errors);
    // The basis, the keyframe and the cold batch are measured.
    assert_eq!(
        (s.stats.basis_n, s.stats.keyframes_n, s.stats.cold_n),
        (1, 1, 1)
    );
}

#[test]
fn a_missing_batch_asks_for_a_reconnect() {
    let (mut s, v1) = started();
    let v2 = world(1, 120);
    let fx = s.handle(&batch(
        3,
        vec![delta(1, &v1, &v2, ViewHash::of(&v2).value())],
    ));
    assert!(fx.iter().any(|e| matches!(e, Effect::Reconnect(_))));
    assert_eq!(
        s.view.as_ref().unwrap().0.top_bar.gold,
        100,
        "the diff is not applied"
    );
    assert!(!s.stats.errors.is_empty());
}

#[test]
fn a_wrong_hash_rolls_the_view_back_and_asks_for_a_keyframe_once() {
    let (mut s, v1) = started();
    let v2 = world(1, 120);
    let fx = s.handle(&batch(2, vec![delta(1, &v1, &v2, 12345)]));
    assert_eq!(
        sends(&fx),
        vec![&ClientMsg::NeedKeyframe {
            reason: KeyframeReason::HashMismatch
        }]
    );
    assert_eq!(s.stats.hash_mismatch, 1);
    assert_eq!(
        s.view.as_ref().unwrap().0.top_bar.gold,
        100,
        "rolled back to the last good view"
    );
    assert_eq!(
        s.view.as_ref().unwrap().1.value(),
        ViewHash::of(&v1).value()
    );
    // Diffs that come before the keyframe are not applied and the request is not repeated.
    let v3 = world(1, 130);
    let fx = s.handle(&batch(
        3,
        vec![delta(2, &v2, &v3, ViewHash::of(&v3).value())],
    ));
    assert!(sends(&fx).is_empty());
    assert_eq!(s.stats.ignored_diffs, 1);
    assert_eq!(s.stats.need_keyframe.get("HashMismatch"), Some(&1));
    // The keyframe heals it.
    s.handle(&batch(4, vec![keyframe(&v3)]));
    assert!(!s.awaiting_keyframe);
    assert_eq!(s.view.as_ref().unwrap().0.top_bar.gold, 130);
    assert_eq!(s.view_seq, 4);
}

#[test]
fn a_diff_to_another_batch_asks_for_a_keyframe() {
    let (mut s, v1) = started();
    let v2 = world(1, 120);
    let fx = s.handle(&batch(
        2,
        vec![delta(7, &v1, &v2, ViewHash::of(&v2).value())],
    ));
    assert_eq!(
        sends(&fx),
        vec![&ClientMsg::NeedKeyframe {
            reason: KeyframeReason::BaseMismatch
        }]
    );
    assert_eq!(
        s.stats.hash_mismatch, 0,
        "a base mismatch is not a hash mismatch"
    );
}

#[test]
fn a_keyframe_naming_another_basis_is_not_taken() {
    let mut s = session();
    let v = world(1, 100);
    let mut other = basis();
    other.terrain_seed = 5;
    other.plots[0].terrain = 3;
    other.basis_hash = basis_hash_of(&other);
    let kf = GameMsg::Keyframe(Keyframe {
        view: v.clone(),
        hash: ViewHash::of(&v).value(),
        basis_hash: basis().basis_hash,
    });
    // The basis in the batch is another one than the frame was built with.
    let fx = s.handle(&batch(1, vec![GameMsg::MapBasis(other), kf]));
    assert_eq!(
        sends(&fx),
        vec![&ClientMsg::NeedKeyframe {
            reason: KeyframeReason::BasisMismatch
        }]
    );
    assert!(s.view.is_none());
}

#[test]
fn a_keyframe_without_a_basis_is_not_taken() {
    let mut s = session();
    let fx = s.handle(&batch(1, vec![keyframe(&world(1, 100))]));
    assert_eq!(
        sends(&fx),
        vec![&ClientMsg::NeedKeyframe {
            reason: KeyframeReason::BasisMismatch
        }]
    );
}

#[test]
fn the_order_inside_a_batch_is_checked() {
    let mut s = session();
    let v = world(1, 100);
    s.handle(&batch(
        1,
        vec![
            GameMsg::TurnState(turn_state(1)),
            GameMsg::MapBasis(basis()),
            keyframe(&v),
        ],
    ));
    assert!(
        s.stats.errors.iter().any(|e| e.contains("out of order")),
        "{:?}",
        s.stats.errors
    );
}

#[test]
fn results_close_the_ledger_and_measure_the_latency() {
    let (mut s, v1) = started();
    let cmd = protocol::Cmd {
        id: s.alloc_id(),
        turn: 1,
        cmd: protocol::Command::UnitSleep { unit: 1 },
    };
    s.ledger
        .sent(&cmd, Instant::now() - Duration::from_millis(30), false);
    let v2 = world(1, 120);
    s.handle(&batch(
        2,
        vec![
            delta(1, &v1, &v2, ViewHash::of(&v2).value()),
            GameMsg::Results(vec![CommandResult {
                id: cmd.id,
                outcome: CommandOutcome::Applied,
            }]),
        ],
    ));
    assert_eq!(s.ledger.open(), 0);
    assert_eq!(s.stats.latency_ms.len(), 1);
    let h = s.stats.latency_ms.summary();
    assert!(h.p50.unwrap() >= 30.0, "{h:?}");
}

#[test]
fn corrupt_view_spoils_the_copy_and_the_next_diff_disagrees() {
    let (mut s, v1) = started();
    s.knobs.corrupt_view_at_turn = Some(1);
    // Any batch brings the knob into play.
    let v2 = world(1, 120);
    s.handle(&batch(
        2,
        vec![delta(1, &v1, &v2, ViewHash::of(&v2).value())],
    ));
    assert!(s.corrupted);
    assert_ne!(
        s.view.as_ref().unwrap().1.value(),
        ViewHash::of(&v2).value()
    );
    let v3 = world(1, 130);
    let fx = s.handle(&batch(
        3,
        vec![delta(2, &v2, &v3, ViewHash::of(&v3).value())],
    ));
    assert_eq!(s.stats.hash_mismatch, 1);
    assert_eq!(
        sends(&fx),
        vec![&ClientMsg::NeedKeyframe {
            reason: KeyframeReason::HashMismatch
        }]
    );
}

#[test]
fn drop_basis_asks_for_the_basis_again() {
    let (mut s, _) = started();
    s.knobs.drop_basis_at_turn = Some(1);
    let v = world(1, 100);
    let fx = s.handle(&batch(2, vec![GameMsg::TurnState(turn_state(1))]));
    assert!(s.basis.is_none());
    assert_eq!(
        sends(&fx),
        vec![&ClientMsg::NeedKeyframe {
            reason: KeyframeReason::BasisMismatch
        }]
    );
    // The answer: the basis and the keyframe in one batch.
    s.handle(&batch(3, vec![GameMsg::MapBasis(basis()), keyframe(&v)]));
    assert!(s.basis.is_some() && !s.awaiting_keyframe);
    assert_eq!(s.stats.basis_n, 2);
}

#[test]
fn a_fresh_entry_with_the_basis_in_memory_counts_a_skipped_basis() {
    let (mut s, v) = started();
    // The link drops; the new login names the basis it holds and gets `Fresh` without `MapBasis`.
    s.begin_login(true);
    s.handle(&frame(ServerMsg::Attached(Attached {
        game: 1,
        player: 0,
        stream: 10,
        mode: AttachMode::Fresh,
        catalog: CatalogRef { hash: 0, len: 0 },
        last_cmd_id: 0,
    })));
    s.handle(&batch(
        1,
        vec![keyframe(&v), GameMsg::TurnState(turn_state(1))],
    ));
    assert_eq!(s.stats.basis_skipped, 1);
    assert!(s.view.is_some());
}

#[test]
fn claim_basis_knob_changes_what_is_named() {
    use net_test::bot::knobs::ClaimBasis;
    let (mut s, _) = started();
    let own = s.basis_claim().unwrap();
    assert_eq!(own.hash, basis().basis_hash);
    s.knobs.claim_basis = ClaimBasis::None;
    assert!(s.basis_claim().is_none());
    s.knobs.claim_basis = ClaimBasis::WrongGame;
    assert_ne!(s.basis_claim().unwrap().game, own.game);
    s.knobs.claim_basis = ClaimBasis::Stale;
    assert_ne!(
        s.basis_claim().unwrap().hash,
        own.hash,
        "no change happened: the claim is spoiled"
    );
}
