//! The frame budget in cost units: what a frame reads stays within the budget (the first root of a frame always
//! gets its reads), the stats count the cost of exactly the reads made, roots are written whole, and frames with
//! nothing to do read and write nothing.

mod common;

use common::*;
use replica::testing::FakeRead;
use ui2_model::r#gen::{Map, Research, Top};
use ui2_source::cost::ReadKind;
use ui2_source::{DEFAULT_FRAME_BUDGET, SourceStats};

/// The cost of one recorded call of the fake (catalog reads are not looked at here).
fn cost(call: &str) -> u32 {
    let kind = match call {
        "game_info" => ReadKind::GameInfo,
        "top_bar" => ReadKind::TopBar,
        "players" => ReadKind::Players,
        "notifications" => ReadKind::Notifications,
        "techs" => ReadKind::Techs,
        "player_resources" => ReadKind::PlayerResources,
        "policy_branches" => ReadKind::PolicyBranches,
        "units" => ReadKind::Units,
        "cities" => ReadKind::Cities,
        "unit_actions" => ReadKind::UnitActions,
        "unit_promotions" => ReadKind::UnitPromotions,
        "city_queue" => ReadKind::CityQueue,
        "city_builds" => ReadKind::CityBuilds,
        other => panic!("no cost for {other}"),
    };
    kind.cost()
}

/// One frame, checked: the stats count the reads the fake saw and their cost.
fn frame(rig: &mut Rig, f: &mut FakeRead, batch: bool) -> SourceStats {
    f.calls.clear();
    let stats = if batch { rig.batch(f) } else { rig.frame(f) };
    assert_eq!(stats.getter_calls as usize, f.calls.len(), "{:?}", f.calls);
    let spent: u32 = f.calls.iter().map(|c| cost(c)).sum();
    assert_eq!(stats.spent_units, spent, "{:?}", f.calls);
    stats
}

#[test]
fn the_default_budget_is_one_and_a_half_ms() {
    assert_eq!(DEFAULT_FRAME_BUDGET, 1500);
}

#[test]
fn a_batch_fits_the_budget_and_is_cut_between_whole_roots() {
    let mut f = game();
    let mut whole = Rig::new(session());
    whole.start(&mut f);
    whole.batch(&mut f);

    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    // A budget below the cost of the whole rebuild (590 units): the rebuild takes two frames.
    rig.src.set_read_budget(450);
    let first = frame(&mut rig, &mut f, true);
    assert!(first.spent_units <= 450, "{}", first.spent_units);
    assert!(first.deferred > 0);
    assert!(rig.store.get::<Top>().is_some() && rig.store.get::<Research>().is_some());
    let mut frames = 1;
    let mut deferred = first.deferred;
    while deferred > 0 {
        let s = frame(&mut rig, &mut f, false);
        assert!(s.spent_units <= 450, "{}", s.spent_units);
        assert!(s.deferred < deferred, "progress every frame");
        deferred = s.deferred;
        frames += 1;
    }
    assert_eq!(frames, 2);
    // The same model as one unlimited frame.
    assert_eq!(rig.store.get::<Map>(), whole.store.get::<Map>());
    assert_eq!(rig.store.get::<Top>(), whole.store.get::<Top>());

    // Nothing left: no read, no write.
    rig.delta();
    for _ in 0..10 {
        assert_eq!(frame(&mut rig, &mut f, false), SourceStats::default());
    }
    assert!(rig.delta().is_none());
}

#[test]
fn the_default_budget_rebuilds_the_reference_game_in_one_frame() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);
    let s = frame(&mut rig, &mut f, true);
    assert_eq!(s.deferred, 0);
    assert!(s.spent_units <= DEFAULT_FRAME_BUDGET);
    assert_eq!(s.spent_units, 590, "{:?}", f.calls);
}

#[test]
fn the_catalog_is_read_over_frames_within_the_budget() {
    let mut f = game();
    let mut whole = Rig::new(session());
    whole.frame(&mut f);

    // From the first frame on at the default budget: the whole catalog (1860 units) does not fit in one frame.
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.src.set_read_budget(DEFAULT_FRAME_BUDGET);
    let mut tables = 0;
    let mut frames = 0;
    loop {
        f.calls.clear();
        let s = rig.frame(&mut f);
        frames += 1;
        assert!(
            s.spent_units <= DEFAULT_FRAME_BUDGET,
            "frame {frames}: {}",
            s.spent_units
        );
        tables += f.calls.iter().filter(|c| **c == "catalog").count();
        if s.deferred == 0 {
            break;
        }
        assert!(frames < 10);
    }
    assert_eq!(frames, 2, "the catalog and the roots in two frames");
    assert_eq!(tables, 8, "each table once");
    // The roots show the names of the tables read on the second frame (units, promotions) all the same.
    assert_eq!(rig.store.get::<Map>(), whole.store.get::<Map>());
    assert_eq!(rig.store.get::<Top>(), whole.store.get::<Top>());
    assert_eq!(rig.store.get::<Research>(), whole.store.get::<Research>());

    // Read once per core: a batch reads no table.
    f.calls.clear();
    rig.batch(&mut f);
    assert!(!f.calls.contains(&"catalog"));
}

#[test]
fn a_failed_table_is_asked_again_after_the_game_changes() {
    let mut f = game();
    let mut rig = Rig::new(session());
    f.error = Some(replica::ReadError::Core("down".into()));
    rig.frame(&mut f);
    assert_eq!(
        f.calls.iter().filter(|c| **c == "catalog").count(),
        8,
        "every table tried once"
    );
    // No new trigger: nothing is asked again.
    f.calls.clear();
    for _ in 0..3 {
        rig.frame(&mut f);
    }
    assert!(f.calls.is_empty(), "{:?}", f.calls);
    // The core answers again and the game changes: the tables and the roots are read.
    f.error = None;
    rig.batch(&mut f);
    assert_eq!(f.calls.iter().filter(|c| **c == "catalog").count(), 8);
    assert!(rig.store.get::<Map>().is_some());
}

#[test]
fn the_first_root_of_a_frame_gets_all_its_reads() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    // Less than any read: each frame builds its first root, plus the roots whose reads are kept.
    rig.src.set_read_budget(1);
    let mut s = frame(&mut rig, &mut f, true);
    let mut frames = 1;
    while s.deferred > 0 {
        assert!(s.roots_built >= 1, "a root a frame");
        s = frame(&mut rig, &mut f, false);
        frames += 1;
    }
    assert!((4..=9).contains(&frames), "{frames}");
    assert_eq!(rig.store.get::<Map>().map(|m| m.cities.len()), Some(5));
}
