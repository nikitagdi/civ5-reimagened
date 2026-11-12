//! The order of a frame: the roots always on screen (turn, game, top, the selected unit) are fresh in the frame of
//! a change whatever the budget; the others share what is left, the longest waiting first, and none waits more
//! than `STALE_FRAMES` frames; a frame of the reference game never spends more than the default budget.

mod common;

use common::*;
use replica::UnitAction;
use ui2_model::r#gen::{Game, Map, Research, Top, Turn, Unit};
use ui2_source::build::entity_key;
use ui2_source::profile::ROOT_COUNTERS;
use ui2_source::trigger::Roots;
use ui2_source::{DEFAULT_FRAME_BUDGET, STALE_FRAMES};

/// The reference game with the Knight selected and its orders.
fn selected() -> (replica::testing::FakeRead, Rig) {
    let mut f = game();
    f.actions.insert(
        BOGATYR,
        vec![UnitAction {
            action: 4,
            enabled: true,
            reason: None,
            cost: None,
        }],
    );
    let mut rig = Rig::new(session());
    rig.ui.selection.unit = Some(entity_key(CATHERINE, BOGATYR));
    rig.start(&mut f);
    (f, rig)
}

#[test]
fn the_roots_on_screen_are_fresh_in_the_frame_of_the_change_whatever_the_budget() {
    let (mut f, mut rig) = selected();
    // A budget smaller than any read.
    rig.src.set_read_budget(1);
    f.info.turn = 88;
    f.top.gold = 999;
    f.units[0].moves_left = 0;
    rig.batch(&mut f);
    let p = rig.src.profile().clone();
    for root in Roots::PRIORITY {
        assert!(p.wrote(root), "{root:?} in {:?}", p.roots);
    }
    assert_eq!(rig.store.get::<Game>().map(|g| g.turn), Some(88));
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(999));
    assert_eq!(
        rig.store
            .get::<Option<Unit>>()
            .and_then(|u| u.as_ref().map(|u| u.moves)),
        Some(0.0)
    );
    assert!(rig.store.get::<Turn>().is_some());
    // The others wait for the next frames.
    assert!(p.pending.contains(Roots::RESEARCH) || p.pending.contains(Roots::MAP));
}

#[test]
fn the_dial_shows_the_turn_the_journal_reached_before_its_snapshot() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);
    rig.start(&mut f);
    assert_eq!(rig.store.get::<Game>().map(|g| g.turn), Some(87));
    // The batch that ends the turn: the replica's journal is at turn 88, its game still at 87 until the snapshot.
    rig.journal_turn = Some(88);
    rig.batch(&mut f);
    assert_eq!(rig.store.get::<Game>().map(|g| g.turn), Some(88));
    // The server's turn state names the next turn first: shown in the frame it comes.
    rig.session.turn.as_mut().expect("turn state").turn = 89;
    rig.frame(&mut f);
    assert_eq!(rig.store.get::<Game>().map(|g| g.turn), Some(89));
    rig.session.turn.as_mut().expect("turn state").turn = 87;
    // The snapshot comes: the record says 88 too, nothing goes back.
    f.info.turn = 88;
    rig.generation += 1;
    rig.frame(&mut f);
    assert_eq!(rig.store.get::<Game>().map(|g| g.turn), Some(88));
    // Only the turn moved (no batch): the turn roots are built from it alone.
    rig.journal_turn = Some(89);
    f.calls.clear();
    rig.frame(&mut f);
    assert_eq!(rig.store.get::<Game>().map(|g| g.turn), Some(89));
    let p = rig.src.profile();
    assert!(
        p.wrote(Roots::GAME) && p.wrote(Roots::TURN),
        "{:?}",
        p.roots
    );
    assert!(!p.wrote(Roots::MAP));
}

#[test]
fn no_frame_of_a_stream_of_batches_spends_more_than_the_budget() {
    let (mut f, mut rig) = selected();
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);
    let mut worst = 0;
    for i in 0..30 {
        f.info.turn = 87 + i / 10;
        rig.journal_turn = Some(f.info.turn);
        let s = rig.batch(&mut f);
        worst = worst.max(s.spent_units);
        assert_eq!(s.deferred, 0, "the reference game fits one frame");
    }
    assert!(worst <= DEFAULT_FRAME_BUDGET, "{worst}");
}

#[test]
fn under_a_batch_every_frame_no_root_waits_more_than_stale_frames() {
    let (mut f, mut rig) = selected();
    // Room for the roots on screen and little else: without the age order the last roots would never be built.
    rig.src.set_read_budget(300);
    let mut last_written = [0u32; 16];
    for frame in 1..=40u32 {
        f.top.gold = i32::try_from(frame).unwrap_or(0);
        rig.batch(&mut f);
        let p = rig.src.profile().clone();
        for (root, _) in ROOT_COUNTERS {
            if root == Roots::PRODUCTION {
                // No window open: nothing to build, never pending long.
                continue;
            }
            if p.wrote(root) {
                last_written[root.index()] = frame;
            }
            let age = frame - last_written[root.index()];
            assert!(
                age <= u32::from(STALE_FRAMES),
                "frame {frame}: {root:?} last written {age} frames ago"
            );
        }
        assert!(
            rig.store.get::<Research>().is_some() && rig.store.get::<Map>().is_some(),
            "frame {frame}"
        );
    }
}

#[test]
fn the_warm_up_after_a_load_reads_what_the_roots_on_screen_read_and_keeps_nothing() {
    let (mut f, mut rig) = selected();
    f.calls.clear();
    let s = rig.src.warm_turn(&mut f, &rig.ui);
    assert_eq!(
        f.calls,
        [
            "game_info",
            "top_bar",
            "player_resources",
            "policy_branches",
            "units",
            "unit_actions",
            "unit_promotions"
        ]
    );
    assert_eq!((s.roots_built, s.getter_calls), (0, 7));
    assert!(rig.delta().is_none(), "nothing written");
    // The next frame reads the game again (the warm-up kept nothing of it) and is within the budget.
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);
    f.calls.clear();
    let s = rig.batch(&mut f);
    assert!(f.calls.contains(&"top_bar"));
    assert!(s.spent_units <= DEFAULT_FRAME_BUDGET);
}
