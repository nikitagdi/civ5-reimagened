//! The client-side fog audit on synthetic views: every violation is found, a clean view has none, and the
//! map basis is on the white list.

use net_test::bot::audit::Audit;
use protocol::{
    BasisState, CityFlags, CityView, ContextMsg, JournalEntry, JournalKind, MapBasis, MapView,
    PlotBasisView, PlotFlags, PlotView, TargetKind, TargetView, TopBar, TurnStatus, UnitContext,
    UnitView, WorldView, basis_hash_of,
};

const REVEALED: u16 = PlotFlags::REVEALED;
const VISIBLE: u16 = PlotFlags::REVEALED | PlotFlags::VISIBLE;

/// A 4x1 map: plot 0 visible, plot 1 revealed only, plot 2 unrevealed, plot 3 visible.
fn world() -> WorldView {
    let plot = |flags: u16| PlotView {
        flags: PlotFlags(flags),
        ..PlotView::default()
    };
    WorldView {
        turn: 5,
        year: 0,
        human: 0,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker: None,
        map: MapView {
            width: 4,
            height: 1,
            wrap_x: false,
            wrap_y: false,
            plots: vec![plot(VISIBLE), plot(REVEALED), plot(0), plot(VISIBLE)],
            terrain_seed: 1,
        },
        units: vec![],
        cities: vec![],
        players: vec![],
        top_bar: TopBar::default(),
        notifications: vec![],
        peace_offers: vec![],
        trade_routes: vec![],
    }
}

fn basis() -> BasisState {
    let mut b = MapBasis {
        game: 1,
        basis_hash: 0,
        width: 4,
        height: 1,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 1,
        plots: vec![PlotBasisView::default(); 4],
    };
    b.basis_hash = basis_hash_of(&b);
    BasisState::new(b).unwrap()
}

fn unit(id: i32, owner: i8, x: i16) -> UnitView {
    UnitView {
        id,
        owner,
        x,
        y: 0,
        ..UnitView::default()
    }
}

fn city(id: i32, owner: i8, x: i16, population: i16) -> CityView {
    CityView {
        id,
        owner,
        x,
        y: 0,
        population,
        flags: CityFlags(0),
        ..CityView::default()
    }
}

fn kinds(a: &Audit) -> Vec<String> {
    a.examples
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_clean_view_has_no_violation() {
    let mut v = world();
    v.units = vec![unit(1, 0, 1), unit(2, 0, 2), unit(3, 1, 0), unit(4, 1, 3)];
    v.cities = vec![city(1, 1, 1, 3)];
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    // Own units may stand anywhere; the foreign ones stand on visible plots; the foreign city is on a
    // revealed plot (its first sighting is the baseline).
    assert_eq!(a.violations, 0, "{:?}", a.examples);
    assert_eq!(a.checked, 1);
}

#[test]
fn a_foreign_unit_on_a_plot_that_is_not_visible_is_found() {
    let mut v = world();
    v.units = vec![unit(3, 1, 1), unit(4, 1, 2)];
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(a.violations, 2);
    assert_eq!(kinds(&a), ["unit_in_fog", "unit_in_fog"]);
}

#[test]
fn a_foreign_city_on_an_unrevealed_plot_is_found() {
    let mut v = world();
    v.cities = vec![city(1, 1, 2, 3)];
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(kinds(&a), ["city_on_unrevealed_plot"]);
}

#[test]
fn the_live_value_of_a_city_in_the_fog_is_found() {
    let b = basis();
    let mut a = Audit::default();
    // Seen while visible (plot 3), population 3.
    let mut v = world();
    v.cities = vec![city(1, 1, 3, 3)];
    a.check_keyframe(&v, &b, 5);
    assert_eq!(a.violations, 0);
    // The plot goes into the fog and the city shows the same numbers: fine.
    v.map.plots[3].flags = PlotFlags(REVEALED);
    a.check_keyframe(&v, &b, 6);
    assert_eq!(a.violations, 0, "{:?}", a.examples);
    // Now it shows a population nobody saw: the live value of a city in the fog.
    v.cities[0].population = 4;
    a.check_keyframe(&v, &b, 7);
    assert_eq!(kinds(&a), ["city_live_in_fog"]);
    // The same for the buildings and the hit points.
    v.cities[0].population = 3;
    v.cities[0].buildings = vec![9];
    a.check_keyframe(&v, &b, 8);
    assert_eq!(a.violations, 2);
    v.cities[0].buildings = vec![];
    v.cities[0].hp = 7;
    a.check_keyframe(&v, &b, 9);
    assert_eq!(a.violations, 3);
    // When the plot is visible again the new value is what was seen.
    v.map.plots[3].flags = PlotFlags(VISIBLE);
    a.check_keyframe(&v, &b, 10);
    assert_eq!(a.violations, 3);
}

#[test]
fn an_unrevealed_plot_carries_the_world_but_nothing_of_sight_unless_strict_is_off() {
    // The whole world comes to the client (NP-D19, 2026-10-22): a forest and a resource under the clouds
    // are fine.
    let mut v = world();
    v.map.plots[2] = PlotView::unrevealed_of(&PlotView {
        flags: PlotFlags(VISIBLE | PlotFlags::WORKED),
        feature: 5,
        resource: 3,
        improvement: 1,
        owner: 1,
        yields: [2, 1, 0, 0, 0, 0],
        ..PlotView::default()
    });
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(a.violations, 0, "{:?}", a.examples);
    // An owner (or yields, or sight) on it is a leak.
    v.map.plots[2].owner = 1;
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(kinds(&a), ["unrevealed_plot_has_sight_data"]);
    // Its relief must be the basis's.
    v.map.plots[2].owner = -1;
    v.map.plots[2].terrain = 4;
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(kinds(&a), ["plot_differs_from_basis"]);
    v.map.plots[2].owner = 1;
    let mut lax = Audit::with_strict(false);
    lax.check_keyframe(&v, &basis(), 5);
    assert_eq!(lax.violations, 0);
}

#[test]
fn the_relief_of_a_revealed_plot_must_equal_the_basis() {
    let mut v = world();
    v.map.plots[1].terrain = 2;
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(kinds(&a), ["plot_differs_from_basis"]);
    // The basis itself is not a violation: it names the relief of the unrevealed plots too (NP-D19), and a
    // wonder under the fog is there.
    let mut b = MapBasis {
        game: 1,
        basis_hash: 0,
        width: 4,
        height: 1,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 1,
        plots: vec![PlotBasisView::default(); 4],
    };
    b.plots[2].terrain = 7;
    b.plots[2].flags = PlotFlags(PlotFlags::NATURAL_WONDER);
    b.basis_hash = basis_hash_of(&b);
    let v = world();
    let mut a = Audit::default();
    a.check_keyframe(&v, &BasisState::new(b).unwrap(), 5);
    assert_eq!(a.violations, 0, "{:?}", a.examples);
}

#[test]
fn the_targets_of_a_context_must_be_visible() {
    let v = world();
    let target = |x| TargetView {
        x,
        y: 0,
        kind: TargetKind::Melee,
        flags: 0,
        preview: None,
    };
    let ctx = ContextMsg::Unit(UnitContext {
        unit: 1,
        view_seq: 1,
        actions: vec![],
        reach: vec![],
        targets: vec![target(0), target(1)],
    });
    let mut a = Audit::default();
    a.check_context(&v, &ctx, 5);
    assert_eq!(kinds(&a), ["context_target_in_fog"]);
}

#[test]
fn a_journal_line_names_a_visible_or_an_own_plot() {
    let mut v = world();
    // Nobody owns plot 1; plot 2 is the bot's own.
    v.map.plots[1].owner = -1;
    v.map.plots[2].owner = 0;
    let line = |kind, x| JournalEntry {
        turn: 5,
        kind,
        text: format!("at {x}"),
        at: Some((x, 0)),
        simplification: None,
    };
    let fight = |x| line(JournalKind::Combat, x);
    let mut a = Audit::default();
    a.check_journal(&v, &[fight(0), fight(2), fight(1)], 5);
    assert_eq!(kinds(&a), ["journal_plot_in_fog"]);
    assert!(a.examples[0]["detail"].as_str().unwrap().contains("at 1"));
}

#[test]
fn a_notification_may_name_a_revealed_plot_but_not_a_dark_one() {
    // The original's "barbarian encampment found" names a plot the team revealed out of sight.
    let mut v = world();
    v.map.plots[1].owner = -1;
    v.map.plots[2].owner = -1;
    let line = |x| JournalEntry {
        turn: 5,
        kind: JournalKind::Notification,
        text: format!("camp at {x}"),
        at: Some((x, 0)),
        simplification: None,
    };
    let mut a = Audit::default();
    a.check_journal(&v, &[line(1), line(2)], 5);
    assert_eq!(kinds(&a), ["journal_plot_in_fog"]);
    assert!(
        a.examples[0]["detail"]
            .as_str()
            .unwrap()
            .contains("camp at 2")
    );
}

#[test]
fn examples_are_capped_but_the_count_is_not() {
    let mut v = world();
    v.units = (0..30).map(|i| unit(i, 1, 2)).collect();
    let mut a = Audit::default();
    a.check_keyframe(&v, &basis(), 5);
    assert_eq!(a.violations, 30);
    assert_eq!(a.examples.len(), 20);
}
