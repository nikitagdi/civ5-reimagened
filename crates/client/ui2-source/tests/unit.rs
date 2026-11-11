//! The root `unit`: the reference HUD's selected Knight "Bogatyr" (strength 20, moves 2 of 4, Shock I, ready to
//! promote) with its orders, the extra alert/pillage/upgrade rows that have no order yet, no reads without a
//! selection, a unit that is gone, and the pending mark of an order waiting for its answer.

mod common;

use common::*;
use replica::testing::catalog_entry;
use replica::{CatalogTable, ReadError, UnitAction};
use ui2_model::r#gen::{
    Act, Activity, Domain, Pending, Promo, Unit, UnitActionKind, UnitActionRow,
};
use ui2_model::{AssetRef, Key, KeyedList, Text};
use ui2_source::pending::unit_target;

const SHOCK_I: i16 = 5;
const DRILL_I: i16 = 8;

fn action(action: i16, enabled: bool, reason: Option<&str>) -> UnitAction {
    UnitAction {
        action,
        enabled,
        reason: reason.map(Into::into),
        cost: None,
    }
}

/// The Knight's orders: move (melee attacks are moves), fortify, sleep, skip, disband; alert and pillage
/// possible, upgrade not.
fn knight_actions() -> Vec<UnitAction> {
    let mut upgrade = action(103, false, Some("Not enough gold"));
    upgrade.cost = Some(210);
    vec![
        action(1, true, None),
        action(4, true, None),
        action(5, true, None),
        action(6, true, None),
        action(11, true, None),
        action(101, true, None),
        action(102, true, None),
        upgrade,
        // A command this source does not know is skipped.
        action(77, true, None),
    ]
}

fn reference() -> replica::testing::FakeRead {
    let mut f = game();
    f.actions.insert(BOGATYR, knight_actions());
    f.actions
        .insert(2, vec![action(1, true, None), action(4, true, None)]);
    f.promotions.insert(BOGATYR, vec![SHOCK_I]);
    f.catalogs.push((
        CatalogTable::Promotions,
        vec![
            catalog_entry(
                SHOCK_I,
                "PROMOTION_SHOCK_1",
                "Shock I",
                "PROMOTION_ATLAS",
                5,
            ),
            catalog_entry(
                DRILL_I,
                "PROMOTION_DRILL_1",
                "Drill I",
                "PROMOTION_ATLAS",
                8,
            ),
        ],
    ));
    f
}

fn unit(rig: &Rig) -> Option<Unit> {
    rig.store
        .get::<Option<Unit>>()
        .cloned()
        .expect("unit is written")
}

fn select(rig: &mut Rig, owner: i8, id: i32) {
    rig.ui.selection.unit = Some(Key::new(
        u32::from(owner.cast_unsigned()),
        id.cast_unsigned(),
    ));
}

fn ui2_calls(f: &replica::testing::FakeRead) -> Vec<&'static str> {
    f.calls
        .iter()
        .copied()
        .filter(|c| *c == "unit_actions" || *c == "unit_promotions")
        .collect()
}

#[test]
fn the_reference_knight() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    select(&mut rig, CATHERINE, BOGATYR);
    rig.frame(&mut f);
    assert_eq!(ui2_calls(&f), ["unit_actions", "unit_promotions"]);
    let u = unit(&rig).expect("the knight is selected");

    let row = |kind, enabled, reason: Option<&str>, supported| UnitActionRow {
        kind,
        enabled,
        reason: reason.map(Into::into),
        supported,
    };
    let on = Act {
        shown: true,
        enabled: true,
        reason: None,
    };
    let mut acts = ui2_model::r#gen::UnitActs {
        r#move: on.clone(),
        fortify: on.clone(),
        sleep: on.clone(),
        skip: on.clone(),
        disband: on.clone(),
        alert: on.clone(),
        pillage: on.clone(),
        ..Default::default()
    };
    acts.upgrade = Act {
        shown: true,
        enabled: false,
        reason: Some("Not enough gold".into()),
    };
    assert_eq!(
        u,
        Unit {
            key: Key::new(0, 1),
            name: "Bogatyr".into(),
            type_name: Text::raw("Knight"),
            portrait: AssetRef::new("icon:UNIT_ATLAS_1:25"),
            level: 2,
            xp: 15,
            strength: 20,
            ranged: 0,
            range: 0,
            moves: 2.0,
            max_moves: 4,
            hp: 100,
            max_hp: 100,
            hp_fraction: 1.0,
            domain: Domain::Land,
            activity: Activity::Awake,
            promotions: KeyedList::from_vec(vec![Promo {
                id: SHOCK_I,
                name: Text::raw("Shock I"),
                icon: AssetRef::new("icon:PROMOTION_ATLAS:5"),
            }])
            .unwrap(),
            promotion_ready: true,
            actions: KeyedList::from_vec(vec![
                row(UnitActionKind::Move, true, None, true),
                row(UnitActionKind::Fortify, true, None, true),
                row(UnitActionKind::Sleep, true, None, true),
                row(UnitActionKind::Skip, true, None, true),
                row(UnitActionKind::Disband, true, None, true),
                row(UnitActionKind::Alert, true, None, false),
                row(UnitActionKind::Pillage, true, None, false),
                row(
                    UnitActionKind::Upgrade,
                    false,
                    Some("Not enough gold"),
                    false
                ),
            ])
            .unwrap(),
            act: acts,
            pending: false,
        }
    );
    assert!(!u.act.found_city.shown, "a knight founds no city");
}

#[test]
fn no_selection_no_unit_actions() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    rig.frame(&mut f);
    for _ in 0..3 {
        rig.batch(&mut f);
    }
    assert_eq!(unit(&rig), None);
    assert!(ui2_calls(&f).is_empty(), "{:?}", f.calls);
}

#[test]
fn a_new_selection_reads_only_that_units_orders() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    select(&mut rig, CATHERINE, BOGATYR);
    rig.start(&mut f);

    select(&mut rig, CATHERINE, 2);
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1, "unit only");
    assert_eq!(
        f.calls,
        ["unit_actions", "unit_promotions"],
        "the units were read already"
    );
    let u = unit(&rig).expect("selected");
    assert_eq!((&*u.name, u.promotion_ready), ("Longswordsman", false));
    assert!(u.promotions.is_empty());
    assert_eq!(u.actions.len(), 2);

    // Back to the knight: everything about it was read since the game last changed.
    f.calls.clear();
    select(&mut rig, CATHERINE, BOGATYR);
    rig.frame(&mut f);
    assert!(f.calls.is_empty());
    assert_eq!(unit(&rig).map(|u| u.strength), Some(20));

    // Deselect: no unit, no read.
    rig.ui.selection.unit = None;
    rig.frame(&mut f);
    assert_eq!(unit(&rig), None);
    assert!(f.calls.is_empty());
}

#[test]
fn a_unit_that_is_gone_is_no_unit() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    select(&mut rig, CATHERINE, BOGATYR);
    rig.start(&mut f);
    f.units
        .retain(|u| !(u.owner == CATHERINE && u.id == BOGATYR));
    rig.batch(&mut f);
    assert_eq!(unit(&rig), None);
    assert!(
        !f.calls.contains(&"unit_actions"),
        "no orders read for a unit that is not there"
    );
}

#[test]
fn a_batch_rereads_the_selected_units_orders() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    select(&mut rig, CATHERINE, BOGATYR);
    rig.start(&mut f);
    f.units[0].moves_left = 0;
    f.actions.insert(BOGATYR, vec![action(4, true, None)]);
    rig.batch(&mut f);
    let u = unit(&rig).expect("selected");
    assert_eq!(u.moves, 0.0);
    assert_eq!(u.actions.len(), 1);
    assert!(!u.act.r#move.shown && u.act.fortify.shown);
    assert_eq!(f.calls.iter().filter(|c| **c == "unit_actions").count(), 1);
}

#[test]
fn a_foreign_unit_shows_without_orders() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    select(&mut rig, MONTEZUMA, 1);
    rig.frame(&mut f);
    let u = unit(&rig).expect("the jaguar");
    assert_eq!(&*u.name, "Jaguar");
    assert!(u.actions.is_empty() && u.promotions.is_empty());
    assert!(ui2_calls(&f).is_empty());
}

#[test]
fn an_order_waiting_for_its_answer_marks_the_unit_without_reading() {
    let mut f = reference();
    let mut rig = Rig::new(session());
    select(&mut rig, CATHERINE, BOGATYR);
    rig.start(&mut f);
    rig.ui
        .pending
        .try_push(Pending {
            seq: 1,
            intent: "unit.action".into(),
            target: unit_target(Key::new(0, 1)).into(),
            since_ms: 0,
        })
        .unwrap();
    let stats = rig.frame(&mut f);
    assert_eq!(
        stats.roots_built, 2,
        "unit, and the closed production window"
    );
    assert!(f.calls.is_empty());
    assert!(unit(&rig).is_some_and(|u| u.pending));
    let d = rig.delta().expect("a delta");
    assert_eq!(d.ops.len(), 1);

    rig.ui.pending = KeyedList::new();
    rig.frame(&mut f);
    assert!(unit(&rig).is_some_and(|u| !u.pending));
}

#[test]
fn a_core_without_promotions_is_not_asked_again() {
    let mut f = reference();
    f.ui2_error = Some(ReadError::Unsupported);
    let mut rig = Rig::new(session());
    select(&mut rig, CATHERINE, BOGATYR);
    rig.frame(&mut f);
    let u = unit(&rig).expect("selected");
    assert!(u.promotions.is_empty());
    assert!(u.promotion_ready, "the unit record still says so");
    assert_eq!(u.actions.len(), 8);
    f.calls.clear();
    rig.batch(&mut f);
    assert!(!f.calls.contains(&"unit_promotions"));
    assert!(f.calls.contains(&"unit_actions"));
}
