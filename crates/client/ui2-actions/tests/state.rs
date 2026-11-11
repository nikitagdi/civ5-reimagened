//! The local state of the UI: writes by path, the state of skins and mods across remounts, settings and their
//! limits, and reconciling with a rebuilt model.

use std::sync::Arc;

use ui2_actions::{MemorySettings, RecordingSink, SettingsNs, StateError, StateNs, UiActions};
use ui2_model::generated::{
    CityBanner, ItemKind, Map, Opt, OptGroup, ProdTab, Production, Ui, UnitFlag,
};
use ui2_model::wire::{Origin, StateOp, WireIntent};
use ui2_model::{Fragment, Key, KeyedList, ModelStore, Namespace, Op, Owner, Schema, Value};

const SKIN_A: &str = "[root]\ncount = \"i32\"\nname = \"str\"\n";
const SKIN_B: &str = "[root]\nx = \"i32\"\n";
const SKIN_A2: &str = "[root]\ncount = \"str\"\nname = \"str\"\nextra = \"bool\"\n";

fn frag(src: &str) -> Fragment {
    Fragment::parse(src).unwrap()
}

fn op(path: &str, value: Value) -> StateOp {
    StateOp {
        path: Arc::from(path),
        value,
    }
}

fn s(text: &str) -> Value {
    Value::Str(Arc::from(text))
}

fn id(text: &str) -> Arc<str> {
    Arc::from(text)
}

struct Rig {
    store: ModelStore,
    actions: UiActions,
    device: MemorySettings,
}

impl Rig {
    fn new() -> Rig {
        let schema = Schema::v1();
        let device = MemorySettings::default();
        let mut store = ModelStore::new(schema);
        let mut actions = UiActions::new(schema, Box::new(device.clone()));
        actions.publish(&mut store);
        store.take_frame();
        Rig {
            store,
            actions,
            device,
        }
    }

    fn ops(&mut self, ns: StateNs, ops: &[StateOp]) -> Vec<StateError> {
        self.actions.state_ops(ns, ops, &mut self.store)
    }

    fn frame(&mut self) -> Vec<Op> {
        self.store.take_frame().map(|f| f.ops).unwrap_or_default()
    }

    /// The value of a path as the frame shows it: takes the pending frame first.
    fn read(&mut self, path: &str) -> Option<Value> {
        self.store.take_frame();
        let f = self.store.resolve(path).ok()?;
        self.store.value(f.id, &[]).cloned()
    }

    fn ui(&self) -> &Ui {
        self.store.get::<Ui>().unwrap()
    }
}

#[test]
fn ui_accepts_only_js_fields() {
    let mut rig = Rig::new();
    let errs = rig.ops(
        StateNs::Skin,
        &[
            op("ui.production.tab", s("units")),
            op("ui.production.focus", s("0:12")),
            op("ui.chat.to", Value::Enum(1)),
            op("ui.windows.menu.open", Value::Bool(true)),
            op("ui.selection.unit", Value::None),
            op("top.gold", Value::I32(1)),
            op("ui.nope", Value::I32(1)),
            op("ui.production.tab", Value::Bool(true)),
            op("ui.production.tab", s("attic")),
        ],
    );
    assert_eq!(errs.len(), 6, "{errs:?}");
    assert!(matches!(&errs[0], StateError::NotWritable(p) if p == "ui.windows.menu.open"));
    assert!(matches!(&errs[1], StateError::NotWritable(p) if p == "ui.selection.unit"));
    assert!(matches!(&errs[2], StateError::NotWritable(p) if p == "top.gold"));
    assert!(matches!(&errs[3], StateError::UnknownPath { .. }));
    assert!(matches!(&errs[4], StateError::BadValue { .. }));
    assert!(matches!(&errs[5], StateError::BadValue { .. }));

    // The three good ones landed, in the typed root and the module's copy.
    assert_eq!(rig.ui().production.tab, ProdTab::Units);
    assert_eq!(rig.ui().production.focus, Some(Key::new(0, 12)));
    assert_eq!(rig.actions.ui(), rig.ui());
    assert!(!rig.ui().windows.menu.open);
    assert_eq!(rig.frame().len(), 3);
    // A later intent does not undo them.
    let mut intents = vec![WireIntent {
        name: id("window.toggle"),
        args: vec![(id("window"), s("menu"))],
        origin: Origin::Script,
    }];
    rig.actions
        .apply(&mut intents, &mut rig.store, &mut RecordingSink::new(), 0);
    assert_eq!(rig.ui().production.tab, ProdTab::Units);
    assert!(rig.ui().windows.menu.open);
}

#[test]
fn a_js_write_that_changes_nothing_gives_no_frame() {
    let mut rig = Rig::new();
    assert!(
        rig.ops(StateNs::Skin, &[op("ui.production.tab", s("all"))])
            .is_empty()
    );
    assert!(rig.frame().is_empty());
}

#[test]
fn skin_state_is_the_skins_and_mods_get_their_own() {
    let mut rig = Rig::new();
    // Nothing mounted yet.
    let errs = rig.ops(StateNs::Skin, &[op("state.count", Value::I32(1))]);
    assert!(matches!(&errs[..], [StateError::NotMounted(_)]), "{errs:?}");

    assert!(
        rig.actions
            .set_skin(Some((id("a"), &frag(SKIN_A))), &mut rig.store)
            .is_empty()
    );
    rig.actions
        .set_mod_state("hello", Some(&frag(SKIN_B)), &mut rig.store);
    rig.actions
        .set_mod_state("other", Some(&frag(SKIN_B)), &mut rig.store);
    rig.frame();

    let errs = rig.ops(
        StateNs::Skin,
        &[
            op("state.count", Value::I32(5)),
            op("state.name", s("Rex")),
            op("state.count", s("no")),
            op("state.nope", Value::I32(1)),
            op("modstate.hello.x", Value::I32(1)),
            op("mods.hello.x", Value::I32(1)),
        ],
    );
    assert_eq!(errs.len(), 4, "{errs:?}");
    assert!(matches!(&errs[0], StateError::BadValue { .. }));
    assert!(matches!(&errs[1], StateError::UnknownPath { .. }));
    assert!(matches!(&errs[2], StateError::OutsideNamespace { .. }));
    assert!(matches!(
        &errs[3],
        StateError::NotWritable(_) | StateError::OutsideNamespace { .. }
    ));
    assert_eq!(rig.read("state.count"), Some(Value::I32(5)));
    assert_eq!(rig.read("state.name"), Some(s("Rex")));

    // A mod's `state.x` is its own `modstate.<id>.x`; it reaches nobody else's.
    let m = StateNs::Mod(id("hello"));
    let errs = rig.ops(
        m.clone(),
        &[
            op("state.x", Value::I32(7)),
            op("modstate.hello.x", Value::I32(8)),
            op("modstate.other.x", Value::I32(9)),
            op("state.nope", Value::I32(1)),
        ],
    );
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(matches!(&errs[0], StateError::OutsideNamespace { .. }));
    assert_eq!(rig.read("modstate.hello.x"), Some(Value::I32(8)));
    assert_eq!(rig.read("modstate.other.x"), Some(Value::I32(0)));
    // The mod cannot write the skin's state: `state.x` is its own and the skin has no `x`.
    assert_eq!(rig.read("state.count"), Some(Value::I32(5)));
    // The Rust half of a mod owns mods.<id>, not these.
    assert!(
        rig.store
            .writer(Owner::Mod(id("hello")))
            .set_value("modstate.hello.x", Value::I32(1))
            .is_err()
    );
}

#[test]
fn state_goes_away_with_the_skin_and_comes_back() {
    let mut rig = Rig::new();
    let (a, b) = (frag(SKIN_A), frag(SKIN_B));
    rig.actions.set_skin(Some((id("a"), &a)), &mut rig.store);
    rig.ops(
        StateNs::Skin,
        &[op("state.count", Value::I32(5)), op("state.name", s("Rex"))],
    );
    rig.frame();

    // Another skin: its own state, and `state.count` is gone.
    assert!(
        rig.actions
            .set_skin(Some((id("b"), &b)), &mut rig.store)
            .is_empty()
    );
    let ops = rig.frame();
    assert!(
        ops.iter().any(|o| matches!(o, Op::Reset { .. })),
        "the consumers see the state go: {ops:?}"
    );
    assert!(rig.store.resolve("state.count").is_err());
    assert_eq!(rig.read("state.x"), Some(Value::I32(0)));
    rig.ops(StateNs::Skin, &[op("state.x", Value::I32(3))]);

    // Back to the first: as it left.
    rig.actions.set_skin(Some((id("a"), &a)), &mut rig.store);
    assert_eq!(rig.read("state.count"), Some(Value::I32(5)));
    assert_eq!(rig.read("state.name"), Some(s("Rex")));
    // And the second still has its own.
    rig.actions.set_skin(Some((id("b"), &b)), &mut rig.store);
    assert_eq!(rig.read("state.x"), Some(Value::I32(3)));

    // No skin: the state is empty and not writable.
    rig.actions.set_skin(None, &mut rig.store);
    assert!(rig.store.resolve("state.x").is_err());
    let errs = rig.ops(StateNs::Skin, &[op("state.x", Value::I32(1))]);
    assert!(matches!(&errs[..], [StateError::NotMounted(_)]));
    rig.actions.set_skin(Some((id("b"), &b)), &mut rig.store);
    assert_eq!(rig.read("state.x"), Some(Value::I32(3)));
}

#[test]
fn a_reloaded_skin_keeps_what_still_fits() {
    let mut rig = Rig::new();
    rig.actions
        .set_skin(Some((id("a"), &frag(SKIN_A))), &mut rig.store);
    rig.ops(
        StateNs::Skin,
        &[op("state.count", Value::I32(5)), op("state.name", s("Rex"))],
    );
    // The new build changed the type of `count`, kept `name`, added `extra`.
    let errs = rig
        .actions
        .set_skin(Some((id("a"), &frag(SKIN_A2))), &mut rig.store);
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(rig.read("state.count"), Some(s("")));
    assert_eq!(rig.read("state.name"), Some(s("Rex")));
    assert_eq!(rig.read("state.extra"), Some(Value::Bool(false)));
}

#[test]
fn a_skin_state_starts_from_its_defaults_and_set_aside_values_win() {
    let mut rig = Rig::new();
    let defaults = vec![
        ("state.count".to_string(), Value::I32(3)),
        ("state.name".to_string(), s("Ann")),
    ];
    let errs = rig.actions.set_skin_with_defaults(
        Some((id("a"), &frag(SKIN_A))),
        &defaults,
        &mut rig.store,
    );
    assert!(errs.is_empty(), "{errs:?}");
    assert_eq!(rig.read("state.count"), Some(Value::I32(3)));
    assert_eq!(rig.read("state.name"), Some(s("Ann")));
    rig.ops(StateNs::Skin, &[op("state.name", s("Rex"))]);
    // Reloaded: the player's value comes back over the default, the untouched one keeps the default.
    rig.actions
        .set_skin_with_defaults(Some((id("a"), &frag(SKIN_A))), &defaults, &mut rig.store);
    assert_eq!(rig.read("state.name"), Some(s("Rex")));
    assert_eq!(rig.read("state.count"), Some(Value::I32(3)));
    // A default that does not fit its path is reported; the rest are written.
    let bad = vec![
        ("state.count".to_string(), s("three")),
        ("state.name".to_string(), s("Ann")),
    ];
    let errs =
        rig.actions
            .set_skin_with_defaults(Some((id("b2"), &frag(SKIN_A))), &bad, &mut rig.store);
    assert!(
        matches!(&errs[..], [StateError::Store { path, .. }] if path == "state.count"),
        "{errs:?}"
    );
    assert_eq!(rig.read("state.name"), Some(s("Ann")));
}

#[test]
fn mod_state_is_set_aside_when_the_mod_is_switched_off() {
    let mut rig = Rig::new();
    let f = frag(SKIN_B);
    rig.actions.set_mod_state("hello", Some(&f), &mut rig.store);
    rig.ops(StateNs::Mod(id("hello")), &[op("state.x", Value::I32(4))]);
    rig.actions.set_mod_state("hello", None, &mut rig.store);
    assert!(!rig.store.is_mounted(&Namespace::ModState(id("hello"))));
    let errs = rig.ops(StateNs::Mod(id("hello")), &[op("state.x", Value::I32(4))]);
    assert!(matches!(&errs[..], [StateError::NotMounted(_)]));
    rig.actions.set_mod_state("hello", Some(&f), &mut rig.store);
    assert_eq!(rig.read("modstate.hello.x"), Some(Value::I32(4)));
}

// ------------------------------------------------------------------------------------------ settings

fn skin_ns() -> SettingsNs {
    SettingsNs::Skin(id("engraved"))
}

#[test]
fn settings_live_under_the_ui2_prefix() {
    let mut rig = Rig::new();
    rig.actions.tick(0);
    rig.actions
        .settings_set(&skin_ns(), "scale", Some("1.25"))
        .unwrap();
    rig.actions.tick(2000);
    rig.actions
        .settings_set(&SettingsNs::Mod(id("hello")), "open", Some("true"))
        .unwrap();
    let all = rig.device.contents();
    assert_eq!(all["civ5.ui2.skin.engraved.scale"], "1.25");
    assert_eq!(all["civ5.ui2.mod.hello.open"], "true");
    assert!(all.keys().all(|k| k.starts_with("civ5.ui2.")), "{all:?}");
    assert!(all.keys().all(|k| !k.starts_with("civ5.options.")));
    assert_eq!(
        rig.actions.settings_snapshot(&skin_ns()),
        vec![("scale".to_string(), "1.25".to_string())]
    );
    // Another session finds them.
    let mut again = UiActions::new(Schema::v1(), Box::new(rig.device.clone()));
    assert_eq!(
        again.settings_snapshot(&skin_ns()),
        vec![("scale".to_string(), "1.25".to_string())]
    );
    assert!(
        again
            .settings_snapshot(&SettingsNs::Skin(id("other")))
            .is_empty()
    );
    // A removal reaches the store too.
    rig.actions.tick(5000);
    rig.actions.settings_set(&skin_ns(), "scale", None).unwrap();
    assert!(
        !rig.device
            .contents()
            .contains_key("civ5.ui2.skin.engraved.scale")
    );
    assert!(rig.actions.settings_snapshot(&skin_ns()).is_empty());
}

#[test]
fn settings_are_written_at_most_once_a_second() {
    let mut rig = Rig::new();
    rig.actions.tick(10_000);
    rig.actions
        .settings_set(&skin_ns(), "a", Some("1"))
        .unwrap();
    // The first write goes at once: the value and the index of keys.
    assert_eq!(rig.device.writes(), 2);

    // Changes inside the second are kept back, and visible at once.
    rig.actions.tick(10_300);
    rig.actions
        .settings_set(&skin_ns(), "a", Some("2"))
        .unwrap();
    rig.actions.tick(10_600);
    rig.actions
        .settings_set(&skin_ns(), "b", Some("3"))
        .unwrap();
    assert_eq!(rig.device.writes(), 2);
    assert_eq!(rig.device.contents()["civ5.ui2.skin.engraved.a"], "1");
    assert_eq!(rig.actions.settings_snapshot(&skin_ns()).len(), 2);
    rig.actions.tick(10_999);
    assert_eq!(rig.device.writes(), 2);

    // Then everything pending goes together.
    rig.actions.tick(11_000);
    assert_eq!(rig.device.writes(), 5, "a, b and the index");
    assert_eq!(rig.device.contents()["civ5.ui2.skin.engraved.a"], "2");
    // Nothing pending: no writes, however often the host ticks.
    for t in 12_000..12_050 {
        rig.actions.tick(t);
    }
    assert_eq!(rig.device.writes(), 5);
    // The frame functions tick too.
    rig.actions.tick(20_000);
    rig.actions
        .settings_set(&skin_ns(), "a", Some("9"))
        .unwrap();
    assert_eq!(rig.device.writes(), 6);
    rig.actions
        .settings_set(&skin_ns(), "a", Some("10"))
        .unwrap();
    assert_eq!(rig.device.writes(), 6);
    rig.actions.reconcile(&mut rig.store, None, 21_000);
    assert_eq!(rig.device.writes(), 7);
    // A write of the same value is not a change.
    rig.actions.reconcile(&mut rig.store, None, 40_000);
    rig.actions
        .settings_set(&skin_ns(), "a", Some("10"))
        .unwrap();
    assert_eq!(rig.device.writes(), 7);
}

#[test]
fn settings_limits_drop_the_write() {
    let mut rig = Rig::new();
    rig.actions.tick(0);
    let ns = skin_ns();
    for key in ["", "a b", "a/b", "ключ", &"k".repeat(65)] {
        assert!(
            matches!(
                rig.actions.settings_set(&ns, key, Some("1")),
                Err(StateError::BadKey(_))
            ),
            "{key:?}"
        );
    }
    assert!(
        rig.actions
            .settings_set(&ns, &"k".repeat(64), Some("1"))
            .is_ok()
    );
    assert!(rig.actions.settings_set(&ns, "A.b_c-9", Some("1")).is_ok());

    let big = format!("\"{}\"", "x".repeat(4094));
    assert_eq!(big.len(), 4096);
    assert!(rig.actions.settings_set(&ns, "big", Some(&big)).is_ok());
    let too_big = format!("\"{}\"", "x".repeat(4095));
    assert!(matches!(
        rig.actions.settings_set(&ns, "big2", Some(&too_big)),
        Err(StateError::ValueTooBig { len: 4097, .. })
    ));
    assert!(matches!(
        rig.actions.settings_set(&ns, "j", Some("{oops")),
        Err(StateError::BadJson { .. })
    ));
    assert!(matches!(
        rig.actions
            .settings_set(&SettingsNs::Mod(id("a.b")), "k", Some("1")),
        Err(StateError::BadNamespace(_))
    ));

    // 64 keys at most: three are in, fill up to 64, the 65th is refused, an existing one can change.
    for i in 0..61 {
        rig.actions
            .settings_set(&ns, &format!("n{i}"), Some("0"))
            .unwrap();
    }
    assert_eq!(rig.actions.settings_snapshot(&ns).len(), 64);
    assert!(matches!(
        rig.actions.settings_set(&ns, "one-too-many", Some("0")),
        Err(StateError::TooManyKeys(_))
    ));
    assert!(rig.actions.settings_set(&ns, "n0", Some("1")).is_ok());
    // A removal makes room.
    rig.actions.settings_set(&ns, "n1", None).unwrap();
    assert!(rig.actions.settings_set(&ns, "one-more", Some("0")).is_ok());
    // The skins are counted apart.
    assert!(
        rig.actions
            .settings_set(&SettingsNs::Skin(id("other")), "x", Some("0"))
            .is_ok()
    );
    assert!(
        !rig.device
            .contents()
            .contains_key("civ5.ui2.skin.engraved.a b")
    );
}

// ------------------------------------------------------------------------------------------ reconcile

fn banner(id: u32, mine: bool) -> CityBanner {
    CityBanner {
        key: Key::new(0, id),
        mine,
        ..CityBanner::default()
    }
}

fn flag(id: u32) -> UnitFlag {
    UnitFlag {
        key: Key::new(0, id),
        mine: true,
        ..UnitFlag::default()
    }
}

fn put_map(rig: &mut Rig, cities: Vec<CityBanner>, units: Vec<UnitFlag>) {
    let map = Map {
        cities: KeyedList::from_vec(cities).unwrap(),
        units: KeyedList::from_vec(units).unwrap(),
    };
    rig.store.writer(Owner::Source).put(map).unwrap();
}

fn put_production(rig: &mut Rig, items: &[u32]) {
    let options = items
        .iter()
        .map(|i| Opt {
            item: Key::new(0, *i),
            ..Opt::default()
        })
        .collect();
    let p = Production {
        groups: KeyedList::from_vec(vec![OptGroup {
            kind: ItemKind::Unit,
            count: items.len() as i32,
            options: KeyedList::from_vec(options).unwrap(),
        }])
        .unwrap(),
        ..Production::default()
    };
    rig.store.writer(Owner::Source).put(Some(p)).unwrap();
}

fn open_production(rig: &mut Rig, city: Key) {
    let mut intents = vec![WireIntent {
        name: id("window.open"),
        args: vec![
            (id("window"), s("production")),
            (id("city"), Value::Key(city)),
        ],
        origin: Origin::Script,
    }];
    rig.actions
        .apply(&mut intents, &mut rig.store, &mut RecordingSink::new(), 0);
}

#[test]
fn the_production_window_closes_when_the_city_is_lost() {
    let mut rig = Rig::new();
    put_map(&mut rig, vec![banner(3, true), banner(4, true)], vec![]);
    rig.actions.reconcile(&mut rig.store, None, 0);
    open_production(&mut rig, Key::new(0, 3));
    rig.frame();
    assert!(rig.ui().windows.production.open);

    // A new snapshot with the same cities: the window stays.
    put_map(&mut rig, vec![banner(4, true), banner(3, true)], vec![]);
    rig.actions.reconcile(&mut rig.store, None, 1);
    assert!(rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, Some(Key::new(0, 3)));

    // The city is taken: one change, the window closes.
    put_map(&mut rig, vec![banner(3, false), banner(4, true)], vec![]);
    rig.actions.reconcile(&mut rig.store, None, 2);
    assert!(!rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, None);
    rig.frame();
    rig.actions.reconcile(&mut rig.store, None, 3);
    assert!(rig.frame().is_empty(), "nothing more to reconcile");
}

#[test]
fn a_focus_on_a_vanished_option_is_dropped() {
    let mut rig = Rig::new();
    put_production(&mut rig, &[5, 6]);
    rig.actions.reconcile(&mut rig.store, None, 0);
    rig.ops(
        StateNs::Skin,
        &[
            op("ui.production.focus", s("0:5")),
            op("ui.production.hover", s("0:6")),
        ],
    );
    rig.actions.reconcile(&mut rig.store, None, 1);
    assert_eq!(rig.ui().production.focus, Some(Key::new(0, 5)));

    put_production(&mut rig, &[6, 7]);
    rig.actions.reconcile(&mut rig.store, None, 2);
    assert_eq!(rig.ui().production.focus, None);
    assert_eq!(rig.ui().production.hover, Some(Key::new(0, 6)));

    // The window closes: the source writes `None`, everything focused is dropped.
    rig.store
        .writer(Owner::Source)
        .put(None::<Production>)
        .unwrap();
    rig.actions.reconcile(&mut rig.store, None, 3);
    assert_eq!(rig.ui().production.hover, None);
}

#[test]
fn roots_the_source_has_not_written_are_not_judged() {
    let mut rig = Rig::new();
    rig.ops(StateNs::Skin, &[op("ui.production.focus", s("0:5"))]);
    rig.actions.reconcile(&mut rig.store, None, 0);
    assert_eq!(rig.ui().production.focus, Some(Key::new(0, 5)));
}

#[test]
fn the_ui_state_survives_a_new_snapshot() {
    let mut rig = Rig::new();
    put_map(&mut rig, vec![banner(3, true)], vec![flag(1)]);
    rig.actions.reconcile(&mut rig.store, None, 0);
    rig.ops(StateNs::Skin, &[op("ui.production.tab", s("wonders"))]);
    open_production(&mut rig, Key::new(0, 3));
    for t in 1..4 {
        // The source rebuilds the same model over and over (a resync, a turn change).
        put_map(&mut rig, vec![banner(3, true)], vec![flag(1)]);
        rig.actions.reconcile(&mut rig.store, None, t);
    }
    assert_eq!(rig.ui().production.tab, ProdTab::Wonders);
    assert!(rig.ui().windows.production.open);
}
