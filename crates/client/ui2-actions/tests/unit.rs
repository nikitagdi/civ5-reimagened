//! Units, clicks on the map and the camera: what each intent sends, what it writes to `ui`, and every refusal.

mod common;

use common::{Rig, city_at, key, kstr, n, plot, s, selected, unit_at};
use protocol::Command;
use ui2_actions::{CameraReq, Refused, Sent};
use ui2_model::generated::{MapMode, UnitActionKind};
use ui2_model::{Key, Value};

fn rig_with_units() -> Rig {
    let mut rig = Rig::new();
    rig.units(vec![
        unit_at(1, 3, 4, true, true),
        unit_at(2, 3, 4, true, false),
        unit_at(3, 5, 5, true, true),
        unit_at(9, 7, 7, false, true),
    ]);
    rig.cities(vec![city_at(20, 10, 10, true), city_at(21, 11, 11, false)]);
    rig
}

#[test]
fn select_writes_the_selection_and_sends_nothing() {
    let mut rig = rig_with_units();
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("move"))]);
    assert_eq!(rig.ui().map_mode, MapMode::Move);
    rig.one("unit.select", &[("unit", kstr(3))]).unwrap();
    let ui = rig.ui();
    assert_eq!(ui.selection.unit, Some(key(3)));
    assert_eq!(ui.selection.plot.as_ref().map(|p| (p.x, p.y)), Some((5, 5)));
    assert_eq!(ui.map_mode, MapMode::Select);
    assert!(rig.sent().is_empty());
}

#[test]
fn select_refuses_a_foreign_or_unknown_unit() {
    let mut rig = rig_with_units();
    rig.one("unit.select", &[("unit", kstr(9))]).unwrap();
    rig.assert_refused("no_unit");
    assert_eq!(rig.ui().selection.unit, None);

    let mut rig = rig_with_units();
    rig.one("unit.select", &[("unit", kstr(77))]).unwrap();
    rig.assert_refused("no_unit");
}

#[test]
fn select_next_walks_the_waiting_units_and_moves_the_camera() {
    let mut rig = rig_with_units();
    rig.one("unit.select_next", &[]).unwrap();
    assert_eq!(rig.ui().selection.unit, Some(key(1)));
    rig.one("unit.select_next", &[]).unwrap();
    assert_eq!(rig.ui().selection.unit, Some(key(3)));
    rig.one("unit.select_next", &[]).unwrap();
    assert_eq!(rig.ui().selection.unit, Some(key(1)), "wraps round");
    assert_eq!(
        rig.sent(),
        &[
            Sent::Camera(CameraReq::Plot(3, 4)),
            Sent::Camera(CameraReq::Plot(5, 5)),
            Sent::Camera(CameraReq::Plot(3, 4)),
        ]
    );
}

#[test]
fn select_next_without_a_waiting_unit_is_refused() {
    let mut rig = Rig::new();
    rig.units(vec![
        unit_at(1, 0, 0, true, false),
        unit_at(9, 1, 1, false, true),
    ]);
    rig.one("unit.select_next", &[]).unwrap();
    rig.assert_refused("no_unit");
}

#[test]
fn deselect_clears_unit_and_mode() {
    let mut rig = rig_with_units();
    rig.one("unit.select", &[("unit", kstr(1))]).unwrap();
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("move"))])
        .unwrap();
    assert_eq!(rig.ui().map_mode, MapMode::Move);
    rig.one("unit.deselect", &[]).unwrap();
    assert_eq!(rig.ui().selection.unit, None);
    assert_eq!(rig.ui().map_mode, MapMode::Select);
}

#[test]
fn every_unit_action_is_one_command_stamped_with_the_turn() {
    type Make = fn(i32) -> Command;
    let table: [(&str, Make); 10] = [
        ("fortify", |unit| Command::UnitFortify { unit }),
        ("sleep", |unit| Command::UnitSleep { unit }),
        ("skip", |unit| Command::UnitSkip { unit }),
        ("heal", |unit| Command::UnitHeal { unit }),
        ("found_city", |unit| Command::UnitFoundCity { unit }),
        ("automate_build", |unit| Command::UnitAutomateBuild { unit }),
        ("automate_explore", |unit| Command::UnitAutomateExplore {
            unit,
        }),
        ("cancel", |unit| Command::UnitCancel { unit }),
        ("disband", |unit| Command::UnitDisband { unit }),
        ("wake", |unit| Command::UnitWake { unit }),
    ];
    for (name, expect) in table {
        let mut rig = rig_with_units();
        rig.one("unit.action", &[("unit", kstr(1)), ("action", s(name))]);
        assert_eq!(rig.commands(), vec![expect(1)], "{name}");
        assert!(
            matches!(rig.sent()[0], Sent::Command { turn: 42, .. }),
            "{name}: stamped with game.turn"
        );
        let row = &rig
            .ui()
            .pending
            .iter()
            .next()
            .expect("a pending row")
            .clone();
        assert_eq!(&*row.intent, "unit.action");
        assert_eq!(
            &*row.target, "unit/0:1",
            "the source looks up `unit_target`"
        );
        assert!(rig.toasts().is_empty(), "{name}");
    }
}

#[test]
fn the_unit_id_is_the_low_half_of_its_key() {
    let mut rig = Rig::new();
    let k = Key::new(3, 4711);
    let mut u = unit_at(0, 0, 0, true, true);
    u.key = k;
    rig.units(vec![u]);
    rig.one(
        "unit.action",
        &[("unit", s(&k.to_js())), ("action", s("sleep"))],
    );
    assert_eq!(rig.commands(), vec![Command::UnitSleep { unit: 4711 }]);
}

#[test]
fn alert_pillage_and_upgrade_are_not_ordered() {
    for name in ["alert", "pillage", "upgrade"] {
        let mut rig = rig_with_units();
        rig.one("unit.action", &[("unit", kstr(1)), ("action", s(name))])
            .unwrap();
        rig.assert_refused("unsupported");
    }
}

#[test]
fn a_foreign_or_unknown_unit_gets_no_order() {
    for id in [9, 77] {
        let mut rig = rig_with_units();
        rig.one(
            "unit.action",
            &[("unit", kstr(id)), ("action", s("fortify"))],
        )
        .unwrap();
        rig.assert_refused("no_unit");
        let mut rig = rig_with_units();
        rig.one("unit.move", &[("unit", kstr(id)), ("plot", plot(1, 1))])
            .unwrap();
        rig.assert_refused("no_unit");
    }
}

#[test]
fn an_action_the_model_says_is_unavailable_is_refused() {
    let rows = [
        (UnitActionKind::Fortify, false, true),
        (UnitActionKind::Sleep, true, true),
        (UnitActionKind::Pillage, true, false),
    ];
    let mut rig = rig_with_units();
    rig.put(selected(1, &rows));
    rig.one(
        "unit.action",
        &[("unit", kstr(1)), ("action", s("fortify"))],
    )
    .unwrap();
    rig.assert_refused("action_unavailable");

    // No row at all: the core does not offer it.
    let mut rig = rig_with_units();
    rig.put(selected(1, &rows));
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("heal"))])
        .unwrap();
    rig.assert_refused("action_unavailable");

    // An enabled one goes.
    let mut rig = rig_with_units();
    rig.put(selected(1, &rows));
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("sleep"))]);
    assert_eq!(rig.commands(), vec![Command::UnitSleep { unit: 1 }]);
}

#[test]
fn the_model_of_another_unit_does_not_judge() {
    let mut rig = rig_with_units();
    rig.put(selected(3, &[(UnitActionKind::Fortify, false, true)]));
    rig.one(
        "unit.action",
        &[("unit", kstr(1)), ("action", s("fortify"))],
    );
    assert_eq!(rig.commands(), vec![Command::UnitFortify { unit: 1 }]);
}

#[test]
fn move_and_range_attack_actions_only_set_the_mode() {
    let mut rig = rig_with_units();
    rig.one("unit.action", &[("unit", kstr(3)), ("action", s("move"))])
        .unwrap();
    assert_eq!(rig.ui().selection.unit, Some(key(3)), "it selects the unit");
    assert_eq!(rig.ui().map_mode, MapMode::Move);
    rig.one(
        "unit.action",
        &[("unit", kstr(3)), ("action", s("range_attack"))],
    )
    .unwrap();
    assert_eq!(rig.ui().map_mode, MapMode::RangeAttack);
    assert!(rig.sent().is_empty());
}

#[test]
fn unit_move_and_range_attack_send_one_command_each() {
    let mut rig = rig_with_units();
    rig.one("unit.move", &[("unit", kstr(1)), ("plot", plot(8, 9))]);
    rig.one(
        "unit.range_attack",
        &[("unit", kstr(1)), ("plot", plot(2, 3))],
    );
    assert_eq!(
        rig.commands(),
        vec![
            Command::UnitMoveTo {
                unit: 1,
                x: 8,
                y: 9
            },
            Command::UnitRangeAttack {
                unit: 1,
                x: 2,
                y: 3
            },
        ]
    );
    assert_eq!(rig.ui().pending.len(), 2);
}

#[test]
fn a_ranged_attack_the_model_does_not_allow_is_refused() {
    let mut rig = rig_with_units();
    rig.put(selected(1, &[(UnitActionKind::RangeAttack, false, true)]));
    rig.one(
        "unit.range_attack",
        &[("unit", kstr(1)), ("plot", plot(2, 3))],
    )
    .unwrap();
    rig.assert_refused("action_unavailable");
}

#[test]
fn the_sink_refusing_shows_a_toast_and_no_pending_row() {
    for (refused, why) in [
        (Refused::NoConnection, "no_connection"),
        (Refused::TooMany, "too_many"),
        (Refused::TurnChanged, "turn_changed"),
        (Refused::NoGame, "no_game"),
        (Refused::TooBig, "too_big"),
    ] {
        let mut rig = rig_with_units();
        rig.sink.refuse = Some(refused);
        rig.one("unit.action", &[("unit", kstr(1)), ("action", s("sleep"))])
            .unwrap();
        assert_eq!(rig.why(), vec![why]);
        assert!(rig.ui().pending.is_empty());
    }
}

#[test]
fn without_a_game_nothing_is_ordered() {
    let mut rig = Rig::new();
    // A rig whose model has no `game` root yet.
    let schema = ui2_model::Schema::v1();
    rig.store = ui2_model::ModelStore::new(schema);
    rig.actions.publish(&mut rig.store);
    rig.store.take_frame();
    rig.units(vec![unit_at(1, 0, 0, true, true)]);
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("sleep"))])
        .unwrap();
    rig.assert_refused("no_game");
}

#[test]
fn a_click_in_move_mode_orders_the_selected_unit_and_resets_the_mode() {
    let mut rig = rig_with_units();
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("move"))])
        .unwrap();
    rig.one("map.click", &[("plot", plot(8, 8)), ("button", s("left"))])
        .unwrap();
    assert_eq!(
        rig.commands(),
        vec![Command::UnitMoveTo {
            unit: 1,
            x: 8,
            y: 8
        }]
    );
    assert_eq!(rig.ui().map_mode, MapMode::Select);

    rig.one(
        "unit.action",
        &[("unit", kstr(1)), ("action", s("range_attack"))],
    )
    .unwrap();
    rig.one("map.click", &[("plot", plot(9, 9)), ("button", s("left"))])
        .unwrap();
    assert_eq!(
        rig.commands()[1],
        Command::UnitRangeAttack {
            unit: 1,
            x: 9,
            y: 9
        }
    );
    assert_eq!(rig.ui().map_mode, MapMode::Select);
}

#[test]
fn a_left_click_selects_and_a_repeat_goes_to_the_next_unit_on_the_plot() {
    let mut rig = rig_with_units();
    let click = |rig: &mut Rig| {
        rig.one("map.click", &[("plot", plot(3, 4)), ("button", s("left"))]);
    };
    click(&mut rig);
    assert_eq!(rig.ui().selection.unit, Some(key(1)));
    click(&mut rig);
    assert_eq!(rig.ui().selection.unit, Some(key(2)));
    click(&mut rig);
    assert_eq!(rig.ui().selection.unit, Some(key(1)));
    assert!(rig.sent().is_empty());
}

#[test]
fn a_foreign_unit_is_not_selected_by_a_click() {
    let mut rig = rig_with_units();
    rig.one("map.click", &[("plot", plot(7, 7)), ("button", s("left"))]);
    assert_eq!(rig.ui().selection.unit, None);
    assert!(rig.toasts().is_empty());
}

#[test]
fn a_click_on_an_own_city_without_units_opens_its_production() {
    let mut rig = rig_with_units();
    rig.one(
        "map.click",
        &[("plot", plot(10, 10)), ("button", s("left"))],
    )
    .unwrap();
    assert!(rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, Some(key(20)));
    assert_eq!(rig.ui().selection.city, Some(key(20)));

    // A foreign city does nothing.
    let mut rig = rig_with_units();
    rig.one(
        "map.click",
        &[("plot", plot(11, 11)), ("button", s("left"))],
    );
    assert!(!rig.ui().windows.production.open);
    assert!(rig.toasts().is_empty());
}

#[test]
fn a_right_click_moves_the_selected_unit_and_does_nothing_without_one() {
    let mut rig = rig_with_units();
    rig.one("map.click", &[("plot", plot(8, 8)), ("button", s("right"))]);
    assert!(rig.sent().is_empty());

    rig.one("unit.select", &[("unit", kstr(3))]);
    rig.one("map.click", &[("plot", plot(8, 8)), ("button", s("right"))]);
    assert_eq!(
        rig.commands(),
        vec![Command::UnitMoveTo {
            unit: 3,
            x: 8,
            y: 8
        }]
    );
}

#[test]
fn a_click_for_a_unit_that_is_gone_is_refused_and_the_mode_resets() {
    let mut rig = rig_with_units();
    rig.one("unit.action", &[("unit", kstr(1)), ("action", s("move"))]);
    // The unit is gone from the map before the module has reconciled.
    rig.units(vec![unit_at(2, 3, 4, true, true)]);
    rig.one("map.click", &[("plot", plot(5, 5)), ("button", s("left"))]);
    rig.assert_refused("no_unit");
    assert_eq!(rig.ui().map_mode, MapMode::Select);
}

#[test]
fn the_camera_goes_to_a_plot_a_city_or_a_unit() {
    let mut rig = rig_with_units();
    rig.one("camera.to_plot", &[("plot", plot(6, 7))]);
    rig.one("camera.to_city", &[("city", kstr(21))]);
    rig.one("camera.to_unit", &[("unit", kstr(9))]);
    assert_eq!(
        rig.sent(),
        &[
            Sent::Camera(CameraReq::Plot(6, 7)),
            Sent::Camera(CameraReq::Plot(11, 11)),
            Sent::Camera(CameraReq::Plot(7, 7)),
        ]
    );
    assert!(rig.toasts().is_empty());
}

#[test]
fn the_camera_cannot_go_to_what_the_map_does_not_have() {
    let mut rig = rig_with_units();
    rig.one("camera.to_city", &[("city", kstr(99))]).unwrap();
    rig.assert_refused("not_your_city");
    let mut rig = rig_with_units();
    rig.one("camera.to_unit", &[("unit", kstr(99))]).unwrap();
    rig.assert_refused("no_unit");
}

#[test]
fn plots_outside_the_i16_range_are_a_form_error() {
    let mut rig = rig_with_units();
    rig.one(
        "camera.to_plot",
        &[("plot", Value::Struct(vec![n(70000), n(1)]))],
    );
    assert!(rig.sent().is_empty());
    assert!(rig.toasts().is_empty());
    assert_eq!(rig.actions.take_form_errors().len(), 1);
}
