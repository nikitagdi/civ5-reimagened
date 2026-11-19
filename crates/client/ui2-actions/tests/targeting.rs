//! The map's targeting mode (`map.target`, `map.cancel`, `map.click` while it is on): the state it writes to `ui`,
//! the order a click on a listed plot sends, and every way out.

mod common;

use common::{Rig, city_at, key, kstr, n, plot, s, unit_at};
use protocol::Command;
use ui2_model::KeyedList;
use ui2_model::generated::{
    EndTurnBlock, MapMode, TargetKind, Targets, TargetsLook, TargetsPlot, Turn, TurnBlocker,
};

fn new_rig() -> Rig {
    let mut rig = Rig::new();
    rig.units(vec![
        unit_at(1, 3, 4, true, true),
        unit_at(3, 5, 5, true, true),
        unit_at(9, 7, 7, false, true),
    ]);
    rig.cities(vec![city_at(20, 10, 10, true), city_at(21, 11, 11, false)]);
    rig
}

fn cell(x: i16, y: i16) -> i32 {
    i32::from(x) * 65_536 + i32::from(y)
}

/// The server's answer for `id` and `action`: these plots, or any plot.
fn answer(rig: &mut Rig, id: i32, action: i32, plots: &[(i16, i16)], any: bool) {
    let list = |p: &[(i16, i16)]| {
        KeyedList::from_vec(
            p.iter()
                .map(|&(x, y)| TargetsPlot {
                    cell: cell(x, y),
                    x,
                    y,
                })
                .collect(),
        )
        .unwrap()
    };
    rig.put(Some(Targets {
        id,
        action,
        look: TargetsLook::Fire,
        plots: list(plots),
        area: list(&[]),
        any,
        ..Targets::default()
    }));
}

fn click(rig: &mut Rig, x: i32, y: i32, button: &str) {
    rig.one("map.click", &[("plot", plot(x, y)), ("button", s(button))]);
}

fn aim_unit(rig: &mut Rig, unit: u32, action: i32) {
    rig.one(
        "map.target",
        &[("kind", s("unit")), ("unit", kstr(unit)), ("a", n(action))],
    )
    .unwrap();
}

fn go(kind: TargetKind, id: i64, action: i64, x: i16, y: i16) -> Command {
    Command::ScreenAction {
        screen: "targets".into(),
        action: "go".into(),
        args: vec![
            ("a".into(), kind as i64),
            ("b".into(), id),
            ("c".into(), action),
            ("d".into(), i64::from(cell(x, y))),
        ],
        text: None,
    }
}

#[test]
fn a_units_order_selects_it_and_waits_for_the_servers_plots() {
    let mut rig = new_rig();
    aim_unit(&mut rig, 3, 41);
    let ui = rig.ui();
    assert_eq!(ui.map_mode, MapMode::Target);
    assert_eq!(
        ui.selection.unit,
        Some(key(3)),
        "the unit that acts is selected"
    );
    let t = ui.target.clone().unwrap();
    assert_eq!(
        (t.kind, t.unit, t.city, t.a),
        (TargetKind::Unit, Some(key(3)), None, 41)
    );
    // Before the answer, and with an answer for another order, a click does nothing.
    click(&mut rig, 6, 6, "left");
    answer(&mut rig, 3, 40, &[(6, 6)], false);
    click(&mut rig, 6, 6, "left");
    assert!(rig.sent().is_empty());
    assert_eq!(rig.ui().map_mode, MapMode::Target);
    assert!(rig.toasts().is_empty());
}

#[test]
fn a_click_on_a_listed_plot_sends_the_order_and_ends_the_mode() {
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 41);
    answer(&mut rig, 1, 41, &[(4, 4), (5, 4)], false);
    // A plot the server did not list: nothing, the mode stays.
    click(&mut rig, 9, 9, "left");
    assert!(rig.sent().is_empty());
    assert_eq!(rig.ui().map_mode, MapMode::Target);
    click(&mut rig, 5, 4, "left");
    assert_eq!(rig.commands(), vec![go(TargetKind::Unit, 1, 41, 5, 4)]);
    let ui = rig.ui();
    assert_eq!(ui.map_mode, MapMode::Select);
    assert!(ui.target.is_none());
    assert_eq!(ui.selection.unit, Some(key(1)), "the unit stays selected");
    let row = ui.pending.iter().next().expect("a waiting order").clone();
    assert_eq!((&*row.intent, &*row.target), ("map.target", "unit/0:1"));
    // The next click is an ordinary one again.
    click(&mut rig, 5, 4, "left");
    assert_eq!(rig.commands().len(), 1);
}

#[test]
fn a_right_click_or_map_cancel_leaves_the_mode_without_an_order() {
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 41);
    answer(&mut rig, 1, 41, &[(4, 4)], false);
    click(&mut rig, 4, 4, "right");
    assert_eq!(rig.ui().map_mode, MapMode::Select);
    assert!(rig.ui().target.is_none());
    assert!(
        rig.sent().is_empty(),
        "a right click in the mode does not move the unit"
    );
    aim_unit(&mut rig, 1, 41);
    rig.one("map.cancel", &[]).unwrap();
    assert_eq!(rig.ui().map_mode, MapMode::Select);
    assert!(rig.ui().target.is_none());
    assert_eq!(rig.ui().selection.unit, Some(key(1)));
    assert!(rig.sent().is_empty());
}

#[test]
fn selecting_another_unit_or_deselecting_ends_the_mode() {
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 41);
    rig.one("unit.select", &[("unit", kstr(3))]).unwrap();
    assert_eq!(rig.ui().map_mode, MapMode::Select);
    assert!(rig.ui().target.is_none());
    aim_unit(&mut rig, 3, 41);
    rig.one("unit.deselect", &[]).unwrap();
    assert!(rig.ui().target.is_none());
    assert_eq!(rig.ui().map_mode, MapMode::Select);
}

#[test]
fn a_foreign_unit_or_city_is_refused() {
    let mut rig = new_rig();
    aim_unit(&mut rig, 9, 41);
    rig.assert_refused("no_unit");
    assert_eq!(rig.ui().map_mode, MapMode::Select);
    let mut rig = new_rig();
    rig.one("map.target", &[("kind", s("city")), ("city", kstr(21))]);
    rig.assert_refused("not_your_city");
    assert!(rig.ui().target.is_none());
    // A unit's order without its row is a malformed one.
    let mut rig = new_rig();
    rig.one("map.target", &[("kind", s("unit")), ("unit", kstr(1))]);
    rig.assert_refused("invalid");
}

#[test]
fn a_city_strike_selects_the_city_and_cancelling_lets_it_go() {
    let mut rig = new_rig();
    rig.one("map.target", &[("kind", s("city")), ("city", kstr(20))])
        .unwrap();
    assert_eq!(rig.ui().selection.city, Some(key(20)));
    assert_eq!(rig.ui().map_mode, MapMode::Target);
    answer(&mut rig, 20, 0, &[(11, 10)], false);
    click(&mut rig, 11, 10, "left");
    assert_eq!(rig.commands(), vec![go(TargetKind::City, 20, 0, 11, 10)]);
    assert_eq!(
        &*rig.ui().pending.iter().next().unwrap().target,
        "city/0:20"
    );
    // Esc (map.cancel) in a city strike also lets the city go, as the original's.
    rig.one("map.target", &[("kind", s("city")), ("city", kstr(20))]);
    rig.one("map.cancel", &[]);
    assert_eq!(rig.ui().selection.city, None);
}

#[test]
fn a_gift_to_a_city_state_names_it_and_any_plot_mode_takes_every_click() {
    let mut rig = new_rig();
    rig.one("map.target", &[("kind", s("gift_unit")), ("a", n(23))])
        .unwrap();
    answer(&mut rig, 23, 0, &[(3, 4)], false);
    click(&mut rig, 3, 4, "left");
    assert_eq!(rig.commands(), vec![go(TargetKind::GiftUnit, 23, 0, 3, 4)]);
    // A route to a plot: the server lists none, the core judges the clicked one.
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 12);
    answer(&mut rig, 1, 12, &[], true);
    click(&mut rig, 30, 2, "left");
    assert_eq!(rig.commands(), vec![go(TargetKind::Unit, 1, 12, 30, 2)]);
}

#[test]
fn the_mode_ends_when_its_unit_is_gone() {
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 41);
    rig.units(vec![unit_at(3, 5, 5, true, true)]);
    rig.reconcile();
    assert_eq!(rig.ui().map_mode, MapMode::Select);
    assert!(rig.ui().target.is_none());
    // A gift does not depend on the selected unit.
    let mut rig = new_rig();
    rig.one("unit.select", &[("unit", kstr(1))]);
    rig.one("map.target", &[("kind", s("gift_unit")), ("a", n(23))]);
    rig.units(vec![unit_at(3, 5, 5, true, true)]);
    rig.reconcile();
    assert_eq!(rig.ui().map_mode, MapMode::Target);
    assert_eq!(rig.ui().selection.unit, None);
}

#[test]
fn the_city_strike_blocker_starts_the_citys_targeting() {
    let mut rig = new_rig();
    let blocker = TurnBlocker {
        seq: 0,
        kind: EndTurnBlock::CityRangeAttack,
        city: Some(key(20)),
        ..TurnBlocker::default()
    };
    rig.turn(Turn {
        blockers: KeyedList::from_vec(vec![blocker]).unwrap(),
        ..Turn::default()
    });
    rig.one("turn.blocker", &[("seq", n(0))]).unwrap();
    assert_eq!(rig.ui().map_mode, MapMode::Target);
    let t = rig.ui().target.clone().unwrap();
    assert_eq!((t.kind, t.city), (TargetKind::City, Some(key(20))));
}

#[test]
fn a_units_flag_in_the_mode_stands_for_its_plot() {
    // Another player's unit (its flag): the click on its plot.
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 41);
    answer(&mut rig, 1, 41, &[(7, 7)], false);
    rig.one("unit.select", &[("unit", kstr(9))]).unwrap();
    assert_eq!(rig.commands(), vec![go(TargetKind::Unit, 1, 41, 7, 7)]);
    assert_eq!(
        rig.ui().selection.unit,
        Some(key(1)),
        "the foreign unit is not selected"
    );
    // One of the player's units ends the mode and is selected, unless a unit is to be chosen (a gift).
    let mut rig = new_rig();
    aim_unit(&mut rig, 1, 41);
    rig.one("unit.select", &[("unit", kstr(3))]).unwrap();
    assert_eq!(
        (rig.ui().map_mode, rig.ui().selection.unit),
        (MapMode::Select, Some(key(3)))
    );
    let mut rig = new_rig();
    rig.one("map.target", &[("kind", s("gift_unit")), ("a", n(23))]);
    answer(&mut rig, 23, 0, &[(5, 5)], false);
    rig.one("unit.select", &[("unit", kstr(3))]).unwrap();
    assert_eq!(rig.commands(), vec![go(TargetKind::GiftUnit, 23, 0, 5, 5)]);
}
