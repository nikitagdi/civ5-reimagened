//! The window intents, the shape of intents, and the `ui` root they write.

use std::sync::Arc;

use ui2_actions::{IntentError, MemorySettings, RecordingSink, UiActions};
use ui2_model::generated::{CityBanner, Map, RefuseWhy, ToastKind, Ui};
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{Key, ModelStore, Op, Owner, Schema, Value};

fn intent(name: &str, args: &[(&str, Value)]) -> WireIntent {
    WireIntent {
        name: Arc::from(name),
        args: args
            .iter()
            .map(|(k, v)| (Arc::from(*k), v.clone()))
            .collect(),
        origin: Origin::Markup { node: 7 },
    }
}

fn window(name: &str) -> Value {
    Value::Str(Arc::from(name))
}

struct Rig {
    store: ModelStore,
    actions: UiActions,
    sink: RecordingSink,
}

impl Rig {
    fn new() -> Rig {
        let schema = Schema::v1();
        let mut store = ModelStore::new(schema);
        let mut actions = UiActions::new(schema, Box::new(MemorySettings::default()));
        actions.publish(&mut store);
        store
            .take_frame()
            .expect("the ui root reaches the first frame");
        Rig {
            store,
            actions,
            sink: RecordingSink::new(),
        }
    }

    fn send(&mut self, intents: Vec<WireIntent>, now_ms: u64) -> Option<Vec<Op>> {
        let mut intents = intents;
        self.actions
            .apply(&mut intents, &mut self.store, &mut self.sink, now_ms);
        assert!(intents.is_empty(), "apply drains the intents");
        self.store.take_frame().map(|f| f.ops)
    }

    fn ui(&self) -> &Ui {
        self.store.get::<Ui>().unwrap()
    }
}

fn own_city(rig: &mut Rig, id: u32, mine: bool) -> Key {
    let key = Key::new(0, id);
    let mut map = rig.store.get::<Map>().cloned().unwrap_or_default();
    map.cities
        .try_push(CityBanner {
            key,
            mine,
            ..CityBanner::default()
        })
        .unwrap();
    rig.store.writer(Owner::Source).put(map).unwrap();
    rig.store.take_frame();
    key
}

#[test]
fn toggle_menu_is_one_delta() {
    let mut rig = Rig::new();
    let ops = rig
        .send(
            vec![intent("window.toggle", &[("window", window("menu"))])],
            0,
        )
        .expect("a frame");
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert!(matches!(
        &ops[0],
        Op::Set {
            value: Value::Bool(true),
            ..
        }
    ));
    assert!(rig.ui().windows.menu.open);
    assert_eq!(rig.actions.ui(), rig.ui());

    let ops = rig
        .send(
            vec![intent("window.toggle", &[("window", window("menu"))])],
            1,
        )
        .expect("a frame");
    assert_eq!(ops.len(), 1);
    assert!(!rig.ui().windows.menu.open);
    assert!(rig.sink.sent.is_empty(), "windows never leave the module");
}

#[test]
fn open_and_close_are_idempotent_and_quiet() {
    let mut rig = Rig::new();
    for name in ["research", "chat", "notes_log"] {
        let ops = rig
            .send(vec![intent("window.open", &[("window", window(name))])], 0)
            .unwrap();
        assert_eq!(ops.len(), 1, "{name}");
        // The second one changes nothing: no frame.
        assert!(
            rig.send(vec![intent("window.open", &[("window", window(name))])], 0)
                .is_none()
        );
    }
    assert!(rig.ui().windows.research.open && rig.ui().windows.chat.open);
    let ops = rig
        .send(
            vec![intent("window.close", &[("window", window("chat"))])],
            0,
        )
        .unwrap();
    assert_eq!(ops.len(), 1);
    assert!(
        rig.send(
            vec![intent("window.close", &[("window", window("chat"))])],
            0
        )
        .is_none()
    );
}

#[test]
fn several_intents_in_a_frame_merge_into_one_delta() {
    let mut rig = Rig::new();
    let ops = rig
        .send(
            vec![
                intent("window.open", &[("window", window("menu"))]),
                intent("window.close", &[("window", window("menu"))]),
                intent("window.open", &[("window", window("research"))]),
            ],
            0,
        )
        .unwrap();
    // The menu went and came back to closed: only the research window changed.
    assert_eq!(ops.len(), 1, "{ops:?}");
}

#[test]
fn the_production_window_needs_an_own_city() {
    let mut rig = Rig::new();
    let mine = own_city(&mut rig, 3, true);
    let theirs = own_city(&mut rig, 4, false);
    let key_arg = |k: Key| Value::Str(Arc::from(k.to_js().as_str()));

    // No city, nothing selected: refused with a toast.
    rig.send(
        vec![intent("window.open", &[("window", window("production"))])],
        100,
    );
    assert!(!rig.ui().windows.production.open);
    assert_eq!(rig.ui().errors.len(), 1);
    assert_eq!(rig.ui().errors[0].kind, ToastKind::Refused);
    assert_eq!(rig.ui().errors[0].why, RefuseWhy::NotYourCity);

    // A city that is not ours.
    rig.send(
        vec![intent(
            "window.open",
            &[("window", window("production")), ("city", key_arg(theirs))],
        )],
        100,
    );
    assert!(!rig.ui().windows.production.open);
    assert_eq!(rig.ui().errors.len(), 2);

    // Ours, as a JS string key and as a key value.
    rig.send(
        vec![intent(
            "window.open",
            &[("window", window("production")), ("city", key_arg(mine))],
        )],
        100,
    );
    assert!(rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, Some(mine));
    rig.send(
        vec![intent("window.close", &[("window", window("production"))])],
        100,
    );
    assert!(!rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, None);
    // Toggling has no city argument: with no selected city it is refused like an open without one.
    let before = rig.ui().errors.len();
    rig.send(
        vec![intent("window.toggle", &[("window", Value::Enum(0))])],
        100,
    );
    assert!(!rig.ui().windows.production.open);
    assert_eq!(rig.ui().errors.len(), before + 1);
}

#[test]
fn toasts_expire_after_six_seconds_in_one_delta() {
    let mut rig = Rig::new();
    for t in 0..3 {
        rig.send(
            vec![intent("window.open", &[("window", window("production"))])],
            t * 1000,
        );
    }
    assert_eq!(rig.ui().errors.len(), 3);
    // At 6000 ms the first one is due; at 8000 the other two.
    assert!(rig.send(vec![], 5999).is_none());
    let ops = rig.send(vec![], 6000).unwrap();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert_eq!(rig.ui().errors.len(), 2);
    let ops = rig.send(vec![], 9000).unwrap();
    assert_eq!(ops.len(), 2, "two removals in one frame");
    assert!(rig.ui().errors.is_empty());
}

#[test]
fn at_most_five_toasts_at_once() {
    let mut rig = Rig::new();
    for _ in 0..8 {
        rig.send(
            vec![intent("window.open", &[("window", window("production"))])],
            0,
        );
    }
    assert_eq!(rig.ui().errors.len(), 5);
    // The oldest went: the sequence numbers kept counting.
    assert_eq!(rig.ui().errors[0].seq, 3);
}

#[test]
fn a_bad_form_is_logged_once_and_does_nothing() {
    let mut rig = Rig::new();
    let bad = [
        intent("window.nope", &[]),
        intent("window.toggle", &[]),
        intent("window.toggle", &[("window", window("attic"))]),
        intent("window.toggle", &[("window", Value::I32(1))]),
        intent(
            "window.toggle",
            &[("window", window("menu")), ("extra", Value::Bool(true))],
        ),
        intent(
            "window.toggle",
            &[("window", window("menu")), ("window", window("menu"))],
        ),
        intent(
            "window.open",
            &[
                ("window", window("production")),
                ("city", Value::Str(Arc::from("x"))),
            ],
        ),
        intent("mods.hello.poke", &[]),
    ];
    // Each one from its own node: the log-once rule is per node and name.
    let bad: Vec<WireIntent> = bad
        .into_iter()
        .enumerate()
        .map(|(i, mut w)| {
            w.origin = Origin::Markup { node: i as u32 };
            w
        })
        .collect();
    for round in 0..3 {
        let sent: Vec<WireIntent> = bad.to_vec();
        assert!(rig.send(sent, 0).is_none(), "round {round}: no delta");
        let errs = rig.actions.take_form_errors();
        if round == 0 {
            assert_eq!(errs.len(), 8, "{errs:?}");
            assert!(matches!(errs[3].error, IntentError::BadArg { .. }));
            assert!(matches!(errs[4].error, IntentError::Unexpected { .. }));
            assert!(matches!(errs[5].error, IntentError::Duplicate { .. }));
            assert!(matches!(errs[0].error, IntentError::Unknown(_)));
            assert!(matches!(errs[1].error, IntentError::Missing { .. }));
            assert!(matches!(errs[2].error, IntentError::BadArg { .. }));
            assert!(errs[2].error.to_string().contains("attic"));
        } else {
            assert!(errs.is_empty(), "logged once, not again: {errs:?}");
        }
    }
    assert!(rig.ui().errors.is_empty(), "a form error is no toast");
    assert!(rig.sink.sent.is_empty());
    // The same name from another node is a new place in the skin: logged again.
    let mut other = intent("window.nope", &[]);
    other.origin = Origin::Markup { node: 8 };
    rig.send(vec![other], 0);
    assert_eq!(rig.actions.take_form_errors().len(), 1);
}

#[test]
fn text_forms_of_enums_and_keys_are_accepted() {
    let mut rig = Rig::new();
    let ops = rig
        .send(
            vec![intent("window.toggle", &[("window", Value::Enum(2))])],
            0,
        )
        .unwrap();
    assert_eq!(ops.len(), 1);
    assert!(rig.ui().windows.menu.open);
}

#[test]
fn the_recording_sink_numbers_orders_and_can_refuse() {
    use protocol::Command;
    use ui2_actions::{CameraReq, OrderSink, Refused, Sent};
    use ui2_model::generated::ScreenId;

    let mut s = RecordingSink::new();
    assert_eq!(s.command(Command::UnitFortify { unit: 1 }, 5), Ok(1));
    assert_eq!(s.command(Command::UnitSleep { unit: 1 }, 5), Ok(2));
    s.camera(CameraReq::Plot(3, 4));
    assert!(s.open_screen(ScreenId::TechTree));
    s.refuse = Some(Refused::NoConnection);
    assert_eq!(
        s.command(Command::UnitSkip { unit: 1 }, 5),
        Err(Refused::NoConnection)
    );
    assert_eq!(
        s.chat(protocol::ChatTo::All, "hi"),
        Err(Refused::NoConnection)
    );
    assert_eq!(s.commands().len(), 2);
    assert_eq!(s.sent.len(), 4);
    assert!(matches!(s.sent[2], Sent::Camera(CameraReq::Plot(3, 4))));
    assert_eq!(Refused::TurnChanged.why(), RefuseWhy::TurnChanged);
    s.has_screens = false;
    assert!(!s.open_screen(ScreenId::Policies));
}
