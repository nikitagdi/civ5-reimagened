//! Chat, notifications and the intents of mods.

mod common;

use std::sync::Arc;

use common::{Rig, city_at, intent, key, s};
use ui2_actions::{CameraReq, IntentError, MAX_MOD_INTENTS, Refused, Sent, StateNs};
use ui2_model::generated::{Note, NoteCat, PlayerRow, Plot, RefuseWhy};
use ui2_model::wire::{ModIdx, Origin, StateOp, WireIntent};
use ui2_model::{Key, KeyedList, Value};

fn say(rig: &mut Rig, text: &str, to: &str) {
    rig.one("chat.send", &[("text", s(text)), ("to", s(to))]);
}

#[test]
fn a_line_goes_to_everybody() {
    let mut rig = Rig::new();
    say(&mut rig, "hello", "all");
    assert_eq!(
        rig.sent(),
        &[Sent::Chat {
            to: protocol::ChatTo::All,
            text: "hello".into()
        }]
    );
    assert!(rig.toasts().is_empty());
}

#[test]
fn a_line_goes_to_one_player() {
    let mut rig = Rig::new();
    rig.one(
        "chat.send",
        &[
            ("text", s("psst")),
            ("to", s("player")),
            ("player", s(&Key::from_i32(3).to_js())),
        ],
    );
    assert_eq!(
        rig.sent(),
        &[Sent::Chat {
            to: protocol::ChatTo::Player(3),
            text: "psst".into()
        }]
    );
}

#[test]
fn the_player_may_come_from_the_chat_state() {
    let mut rig = Rig::new();
    let errors = rig.actions.state_ops(
        StateNs::Skin,
        &[StateOp {
            path: Arc::from("ui.chat.to_player"),
            value: s(&Key::from_i32(2).to_js()),
        }],
        &mut rig.store,
    );
    assert!(errors.is_empty(), "{errors:?}");
    say(&mut rig, "hi", "player");
    assert_eq!(
        rig.sent(),
        &[Sent::Chat {
            to: protocol::ChatTo::Player(2),
            text: "hi".into()
        }]
    );
}

#[test]
fn a_whisper_needs_a_player_the_game_has() {
    let mut rig = Rig::new();
    say(&mut rig, "hi", "player");
    rig.assert_refused("other");

    let mut rig = Rig::new();
    rig.put(
        KeyedList::from_vec(vec![PlayerRow {
            id: Key::from_i32(1),
            ..PlayerRow::default()
        }])
        .unwrap(),
    );
    rig.one(
        "chat.send",
        &[
            ("text", s("hi")),
            ("to", s("player")),
            ("player", s(&Key::from_i32(5).to_js())),
        ],
    );
    rig.assert_refused("other");
}

#[test]
fn an_empty_line_is_refused() {
    for text in ["", "   ", "\n\t"] {
        let mut rig = Rig::new();
        say(&mut rig, text, "all");
        rig.assert_refused("chat_empty");
    }
}

#[test]
fn a_line_is_at_most_500_characters() {
    let mut rig = Rig::new();
    say(&mut rig, &"x".repeat(500), "all");
    assert_eq!(rig.sent().len(), 1);

    let mut rig = Rig::new();
    say(&mut rig, &"x".repeat(501), "all");
    rig.assert_refused("chat_too_long");

    // Characters, not bytes.
    let mut rig = Rig::new();
    say(&mut rig, &"я".repeat(500), "all");
    assert_eq!(rig.sent().len(), 1);
}

#[test]
fn a_second_line_within_half_a_second_is_refused() {
    let mut rig = Rig::new();
    say(&mut rig, "one", "all");
    rig.now += 100;
    say(&mut rig, "two", "all");
    assert_eq!(rig.sent().len(), 1, "nothing more went");
    assert_eq!(rig.why(), vec!["chat_rate"]);

    // The refused line did not move the clock: 500 ms after the first one is enough.
    rig.now += 400;
    say(&mut rig, "three", "all");
    assert_eq!(rig.sent().len(), 2);
}

#[test]
fn the_sink_refusing_a_line_is_a_toast_and_does_not_start_the_gap() {
    let mut rig = Rig::new();
    rig.sink.refuse = Some(Refused::NoConnection);
    say(&mut rig, "one", "all");
    assert_eq!(rig.why(), vec!["no_connection"]);
    rig.sink.refuse = None;
    rig.now += 10;
    say(&mut rig, "two", "all");
    assert_eq!(rig.sent().len(), 1);
}

fn note(id: u32, cat: NoteCat, at: Option<(i16, i16)>) -> Note {
    Note {
        id: Key::from_u32(id),
        category: cat,
        plot: at.map(|(x, y)| Plot { x, y }),
        ..Note::default()
    }
}

fn nk(id: u32) -> Value {
    s(&Key::from_u32(id).to_js())
}

#[test]
fn dismissing_a_note_is_local_and_remembered() {
    let mut rig = Rig::new();
    rig.put(
        KeyedList::from_vec(vec![
            note(1, NoteCat::Growth, None),
            note(2, NoteCat::Growth, None),
        ])
        .unwrap(),
    );
    rig.one("note.dismiss", &[("note", nk(2))]);
    assert_eq!(
        rig.actions.dismissed().iter().copied().collect::<Vec<_>>(),
        vec![Key::from_u32(2)]
    );
    assert!(rig.sent().is_empty());
    assert!(rig.toasts().is_empty());
}

#[test]
fn a_dismissed_note_stays_dismissed_until_the_core_drops_it() {
    let mut rig = Rig::new();
    rig.put(
        KeyedList::from_vec(vec![
            note(1, NoteCat::Growth, None),
            note(2, NoteCat::Growth, None),
        ])
        .unwrap(),
    );
    rig.one("note.dismiss", &[("note", nk(2))]);
    // The source leaves the dismissed note out of `notes`, while the core still has it: frame after frame the
    // dismissal holds and the note does not come back.
    for _ in 0..3 {
        rig.put(KeyedList::from_vec(vec![note(1, NoteCat::Growth, None)]).unwrap());
        rig.reconcile_notes(&[1, 2]);
        assert_eq!(
            rig.actions.dismissed().iter().copied().collect::<Vec<_>>(),
            vec![Key::from_u32(2)]
        );
        rig.now += 16;
    }
    // A frame without the core's ids judges nothing.
    rig.reconcile();
    assert_eq!(rig.actions.dismissed().len(), 1);
    // The core drops the note: so does the module.
    rig.reconcile_notes(&[1]);
    assert!(rig.actions.dismissed().is_empty());
}

#[test]
fn dismissing_a_note_the_model_does_not_have_does_nothing() {
    let mut rig = Rig::new();
    rig.put(KeyedList::from_vec(vec![note(1, NoteCat::Growth, None)]).unwrap());
    rig.one("note.dismiss", &[("note", nk(9))]);
    assert!(rig.actions.dismissed().is_empty());
    assert!(rig.toasts().is_empty());
}

#[test]
fn activating_a_note_moves_the_camera() {
    let mut rig = Rig::new();
    rig.put(
        KeyedList::from_vec(vec![
            note(1, NoteCat::Military, Some((4, 5))),
            note(2, NoteCat::Military, None),
        ])
        .unwrap(),
    );
    rig.one("note.activate", &[("note", nk(1))]);
    assert_eq!(rig.sent(), &[Sent::Camera(CameraReq::Plot(4, 5))]);
    rig.one("note.activate", &[("note", nk(2))]);
    assert_eq!(
        rig.sent().len(),
        1,
        "a note without a plot has nowhere to go"
    );
}

#[test]
fn a_production_note_of_an_own_city_opens_its_window() {
    let mut rig = Rig::new();
    rig.cities(vec![city_at(20, 6, 7, true), city_at(21, 8, 8, false)]);
    rig.put(
        KeyedList::from_vec(vec![
            note(1, NoteCat::Production, Some((6, 7))),
            note(2, NoteCat::Production, Some((8, 8))),
            note(3, NoteCat::Production, Some((0, 0))),
        ])
        .unwrap(),
    );
    rig.one("note.activate", &[("note", nk(1))]);
    assert!(rig.ui().windows.production.open);
    assert_eq!(rig.ui().windows.production.city, Some(key(20)));

    let mut rig2 = Rig::new();
    rig2.cities(vec![city_at(21, 8, 8, false)]);
    rig2.put(KeyedList::from_vec(vec![note(2, NoteCat::Production, Some((8, 8)))]).unwrap());
    rig2.one("note.activate", &[("note", nk(2))]);
    assert!(!rig2.ui().windows.production.open);
    assert!(rig2.toasts().is_empty());
}

fn modded(name: &str, origin: Origin) -> WireIntent {
    let mut w = intent(name, &[("n", Value::I32(3)), ("who", s("x"))]);
    w.origin = origin;
    w
}

fn rig_with_mods() -> Rig {
    let mut rig = Rig::new();
    rig.actions
        .set_mods(vec![Arc::from("hello"), Arc::from("echo-2")]);
    rig
}

#[test]
fn a_mod_intent_from_its_own_mod_goes_to_the_dispatcher() {
    let mut rig = rig_with_mods();
    rig.send(vec![
        modded("mods.hello.say", Origin::Mod(ModIdx(0))),
        modded("mods.echo-2.ping_back", Origin::Mod(ModIdx(1))),
    ]);
    let got = rig.actions.take_mod_intents();
    assert_eq!(got.len(), 2);
    assert_eq!((&*got[0].mod_id, &*got[0].name), ("hello", "say"));
    assert_eq!((&*got[1].mod_id, &*got[1].name), ("echo-2", "ping_back"));
    assert_eq!(
        got[0].args,
        vec![(Arc::from("n"), Value::I32(3)), (Arc::from("who"), s("x"))],
        "the arguments pass unchanged"
    );
    assert!(rig.actions.take_mod_intents().is_empty(), "taken once");
    assert!(rig.toasts().is_empty());
    assert!(rig.sent().is_empty());
    assert!(rig.actions.take_form_errors().is_empty());
}

#[test]
fn a_mod_intent_from_another_origin_is_a_form_error_logged_once() {
    let mut rig = rig_with_mods();
    for origin in [
        Origin::Mod(ModIdx(1)),
        Origin::Mod(ModIdx(9)),
        Origin::Markup { node: 4 },
        Origin::Script,
        Origin::Hotkey,
        Origin::Map,
    ] {
        rig.send(vec![modded("mods.hello.say", origin)]);
        rig.send(vec![modded("mods.hello.say", origin)]);
    }
    assert!(rig.actions.take_mod_intents().is_empty());
    assert!(
        rig.toasts().is_empty(),
        "a mistake of the skin is not shown to the player"
    );
    let errors = rig.actions.take_form_errors();
    assert_eq!(errors.len(), 6, "once for each origin");
    assert!(matches!(errors[0].error, IntentError::ModIntent { .. }));
}

#[test]
fn a_malformed_mod_intent_name_is_a_form_error() {
    let mut rig = rig_with_mods();
    for name in [
        "mods.hello",
        "mods.hello.",
        "mods..say",
        "mods.Hello.say",
        "mods.hello.Say",
        "mods.hello.a.b",
    ] {
        rig.send(vec![modded(name, Origin::Mod(ModIdx(0)))]);
    }
    assert!(rig.actions.take_mod_intents().is_empty());
    assert_eq!(rig.actions.take_form_errors().len(), 6);
}

#[test]
fn a_mod_may_not_send_the_intents_of_the_catalogue() {
    let mut rig = rig_with_mods();
    let mut w = intent("window.toggle", &[("window", s("menu"))]);
    w.origin = Origin::Mod(ModIdx(0));
    rig.send(vec![w]);
    assert!(!rig.ui().windows.menu.open);
    let errors = rig.actions.take_form_errors();
    assert!(matches!(errors[0].error, IntentError::ModCatalog { .. }));
}

#[test]
fn too_many_mod_intents_are_dropped() {
    let mut rig = rig_with_mods();
    let many: Vec<WireIntent> = (0..MAX_MOD_INTENTS + 10)
        .map(|_| modded("mods.hello.say", Origin::Mod(ModIdx(0))))
        .collect();
    rig.send(many);
    assert_eq!(rig.actions.take_mod_intents().len(), MAX_MOD_INTENTS);
}

#[test]
fn every_intent_of_the_catalogue_has_a_route() {
    // With default arguments every intent reaches its route; the fallback of the module says `other`.
    let schema = ui2_model::Schema::v1();
    for def in schema.intents() {
        let mut rig = Rig::new();
        let args: Vec<(Arc<str>, Value)> = def
            .args
            .iter()
            .map(|a| (Arc::from(&*a.name), ui2_model::default_value(schema, &a.ty)))
            .collect();
        rig.send(vec![WireIntent {
            name: Arc::from(&*def.name),
            args,
            origin: Origin::Script,
        }]);
        assert!(
            !rig.why().contains(&RefuseWhy::Other.name()),
            "`{}` has no route",
            def.name
        );
    }
}
