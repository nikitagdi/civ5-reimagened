//! Automated units on the real core (b12-account, SIMP-exe-1085): the units a human put on automation (explore,
//! automated workers) do not move at the start of the turn nor while the player plays it, they do not hold the
//! end of the turn, a manual order given to one during the turn takes effect at once, and they move after the
//! player ended the turn (the auto-moves phase of `CvGame::updateMoves`).
//!
//! Returns early on the stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` set (`cargo xtask test -p game-host
//! --gamecore-lib <dir> --test real_automate`) and the game data in `CIV5_GAMEDATA`.

use protocol::{Command, CommandOutcome, UnitFlags, UnitView};

#[path = "common/real.rs"]
mod common;
use common::{H, init, real_core, setup};

use game_host::Worker;
use protocol::Start;

fn unit(h: &mut H, player: i8, id: i32) -> UnitView {
    h.w.fresh_view(player)
        .unwrap()
        .units
        .into_iter()
        .find(|u| u.owner == player && u.id == id)
        .unwrap_or_else(|| panic!("seat {player} has no unit {id}"))
}

#[test]
fn automated_units_move_after_the_end_of_turn_not_before() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (w, outs) = Worker::new(real_core(), init(Start::New(setup()), "saves"), 0);
    let mut h = H::with(w, outs);
    let humans = [0, 1];
    for p in humans {
        h.found_city(p);
    }

    // The warrior of seat 0 goes on exploring.
    let view = h.w.fresh_view(0).unwrap();
    let scout = view
        .units
        .iter()
        .find(|u| u.owner == 0 && u.max_moves > 0 && u.strength > 0)
        .expect("a military unit")
        .id;
    let before = unit(&mut h, 0, scout);
    assert_eq!(
        h.cmd(0, Command::UnitAutomateExplore { unit: scout }),
        CommandOutcome::Applied
    );
    // The turn goes on for a while (the core is stepped): the unit stands where it was, automated, with all its
    // moves, and it does not ask for orders.
    h.run();
    let during = unit(&mut h, 0, scout);
    assert!(during.flags.contains(UnitFlags::AUTOMATED), "{during:?}");
    assert!(
        !during.flags.contains(UnitFlags::NEEDS_ORDERS),
        "{during:?}"
    );
    assert_eq!(
        (during.x, during.y, during.moves_left),
        (before.x, before.y, before.moves_left),
        "an automated unit moved while its owner plays the turn"
    );

    // The end of the turn is not held by it, and after the end it has moved.
    let turn = h.w.phase().0;
    h.next_turn(&humans);
    assert_eq!(h.w.phase().0, turn + 1);
    let after = unit(&mut h, 0, scout);
    assert!(after.flags.contains(UnitFlags::AUTOMATED), "{after:?}");
    assert_ne!(
        (after.x, after.y),
        (before.x, before.y),
        "the automated unit did not move after the end of the turn"
    );

    // The new turn: it stands where the auto-moves left it, with all its moves ...
    h.run();
    let start = unit(&mut h, 0, scout);
    assert_eq!(
        (start.x, start.y),
        (after.x, after.y),
        "moved at turn start"
    );
    assert_eq!(start.moves_left, start.max_moves, "{start:?}");

    // ... and a manual order during the turn takes effect: it goes to a neighbour plot and is off automation.
    let mut moved = false;
    'search: for dx in -1i16..=1 {
        for dy in -1i16..=1 {
            if (dx, dy) == (0, 0) {
                continue;
            }
            let (x, y) = (start.x + dx, start.y + dy);
            if h.cmd(0, Command::UnitMoveTo { unit: scout, x, y }) != CommandOutcome::Applied {
                continue;
            }
            h.run();
            let u = unit(&mut h, 0, scout);
            if (u.x, u.y) == (x, y) {
                assert!(
                    !u.flags.contains(UnitFlags::AUTOMATED),
                    "a manual move leaves the unit on automation: {u:?}"
                );
                moved = true;
                break 'search;
            }
        }
    }
    assert!(moved, "no manual order took effect on the automated unit");
}
