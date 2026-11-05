//! Game over with several humans on the real core (as in
//! the original). A Duel with two humans and two AI, run on Linux/WSL like `real_mp`
//! (`GAMECORE_LIB_DIR_x86_64`, `CIV5_GAMEDATA`).
//!
//! One human loses everything (here: disbands every unit, no city yet) and is eliminated: the game goes on for the
//! other, the turn no longer waits for the dead one, who keeps his seat and sees the end for himself (his own
//! header says `GameOver`, the others' `HumanTurn`). When the last human is dead the game is over for everyone.

use game_host::Worker;
use protocol::{CommandOutcome, PlayerId, Start, TurnPhase, TurnStatus, WorkerMsg};

#[path = "common/real.rs"]
mod common;
use common::{H, init, real_core, setup};

/// The seat disbands every unit it has; returns how many.
fn disband_all(h: &mut H, player: PlayerId) -> usize {
    let units: Vec<i32> =
        h.w.fresh_view(player)
            .unwrap()
            .units
            .iter()
            .filter(|u| u.owner == player)
            .map(|u| u.id)
            .collect();
    for &unit in &units {
        let r = h.cmd(player, protocol::Command::UnitDisband { unit });
        assert_eq!(r, CommandOutcome::Applied, "disband unit {unit}");
    }
    units.len()
}

fn alive(h: &mut H) -> Vec<(PlayerId, bool)> {
    h.w.core_mut()
        .human_states()
        .unwrap()
        .into_iter()
        .map(|(p, a, _)| (p, a))
        .collect()
}

/// The turn status in the view of the seat.
fn status_of(h: &mut H, player: PlayerId) -> TurnStatus {
    h.w.fresh_view(player).unwrap().status
}

#[test]
fn real_mp_one_human_eliminated_then_all() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (w, outs) = Worker::new(real_core(), init(Start::New(setup()), "saves"), 0);
    let mut h = H::with(w, outs);
    assert_eq!(h.w.phase().1, TurnPhase::Humans);
    let t0 = h.w.phase().0;
    assert_eq!(alive(&mut h), vec![(0, true), (1, true)]);

    // Human 1 loses everything.
    let n = disband_all(&mut h, 1);
    assert!(n > 0);
    h.run();
    println!(
        "after the disband: {:?} phase {:?}",
        alive(&mut h),
        h.w.phase()
    );
    println!(
        "status of seat 0: {:?}, of seat 1: {:?}",
        status_of(&mut h, 0),
        status_of(&mut h, 1)
    );
    assert_eq!(alive(&mut h), vec![(0, true), (1, false)], "seat 1 is dead");
    assert_eq!(h.w.phase().1, TurnPhase::Humans, "the game goes on");

    // The other human plays on; the turn does not wait for the dead one, whose seat stays.
    h.next_turn(&[0]);
    h.next_turn(&[0]);
    h.check_views(&[0]);
    assert_eq!(h.w.phase().0, t0 + 2);
    assert_eq!(status_of(&mut h, 0), TurnStatus::HumanTurn);
    assert_eq!(
        status_of(&mut h, 1),
        TurnStatus::GameOver { winner: None },
        "the dead human sees the end for himself"
    );
    assert!(h.w.view_hash(1).is_some(), "the dead human keeps his seat");
    // The turn state: the dead one is done and cannot act, so nobody waits for him.
    let turn = h
        .msgs()
        .filter_map(|m| match m {
            WorkerMsg::Turn(t) => Some(t.clone()),
            _ => None,
        })
        .last()
        .expect("a turn state");
    let seat = |p: PlayerId| turn.seats.iter().find(|s| s.player == p).unwrap();
    assert!(seat(1).finished && !seat(1).may_act, "{:?}", seat(1));
    assert!(!seat(0).finished && seat(0).may_act, "{:?}", seat(0));

    // The last human dies too: the game is over for everyone.
    assert!(disband_all(&mut h, 0) > 0);
    h.run();
    assert_eq!(alive(&mut h), vec![(0, false), (1, false)]);
    assert!(
        matches!(h.w.phase().1, TurnPhase::GameOver { .. }),
        "{:?}",
        h.w.phase()
    );
    for p in [0, 1] {
        assert!(
            matches!(status_of(&mut h, p), TurnStatus::GameOver { .. }),
            "seat {p}"
        );
    }
}
