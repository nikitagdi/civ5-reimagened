//! The options window: the classic client's options (`civ5.options.*`), drafts until accepted, the hotkeys and the
//! multiplayer auto end turn.

mod common;

use std::sync::Arc;

use common::{Rig, n, s};
use protocol::Command;
use ui2_actions::{MemorySettings, Remap, SettingsStore, UiActions};
use ui2_model::generated::{
    GameMenu, GameMenuHotkeys, GameMenuHotkeysActions, Selection, Turn, TurnUi, UiOption,
};
use ui2_model::{Key, KeyedList, Schema, Text};

/// A rig whose options live in `device`, with three skin hotkeys.
fn rig_with(device: &MemorySettings) -> Rig {
    let mut rig = Rig::new();
    rig.actions.set_client_store(Box::new(device.clone()), 0.8);
    rig.actions.set_skin_hotkeys(vec![
        ("F6".into(), Some("top.menu.tech".into())),
        ("Ctrl+KeyM".into(), Some("top.main-menu.button".into())),
        ("Enter".into(), Some("helm.dial".into())),
        ("F7".into(), Some("notes.log".into())),
        ("F7".into(), Some("top.menu.notification-log".into())),
    ]);
    rig.send(vec![]);
    rig
}

fn row(rig: &Rig, key: &str) -> UiOption {
    rig.ui()
        .options
        .iter()
        .find(|o| &*o.key == key)
        .cloned()
        .unwrap_or_else(|| panic!("no option {key}"))
}

fn stored(device: &MemorySettings, key: &str) -> Option<String> {
    device
        .contents()
        .get(&format!("civ5.options.{key}"))
        .cloned()
}

fn set(rig: &mut Rig, key: &str, value: i32) {
    rig.one("options.set", &[("key", s(key)), ("value", n(value))]);
}

#[test]
fn the_rows_read_the_classic_store_and_its_defaults() {
    let mut device = MemorySettings::default();
    device.set("civ5.options.MultiplayerAutoEndTurn", "1");
    device.set("civ5.options.Volume.MUSIC", "0.25");
    let rig = rig_with(&device);
    assert_eq!(row(&rig, "MultiplayerAutoEndTurn").value, 1);
    assert!(row(&rig, "MultiplayerAutoEndTurn").toggle);
    assert_eq!(row(&rig, "Volume.MUSIC").value, 25);
    // A knob never saved is the page's default; a switch never saved is the classic default.
    assert_eq!(row(&rig, "Volume.SFX").value, 80);
    assert_eq!(row(&rig, "AutoUnitCycle").value, 1);
    assert_eq!(row(&rig, "NumAutosavesKept").value, 10);
    let name = row(&rig, "MultiplayerAutoEndTurn").name;
    assert_eq!(name, Text::key("options.MultiplayerAutoEndTurn", &[]));
}

#[test]
fn a_draft_is_stored_only_when_accepted_in_the_classic_form() {
    let device = MemorySettings::default();
    let mut rig = rig_with(&device);
    rig.one("options.edit", &[]);
    set(&mut rig, "MultiplayerAutoEndTurn", 1);
    set(&mut rig, "Volume.MUSIC", 50);
    assert_eq!(
        row(&rig, "MultiplayerAutoEndTurn").value,
        1,
        "the draft shows"
    );
    assert_eq!(
        stored(&device, "MultiplayerAutoEndTurn"),
        None,
        "not stored yet"
    );
    assert_eq!(rig.actions.client_option("MultiplayerAutoEndTurn"), Some(0));
    rig.one("options.accept", &[]);
    assert_eq!(
        stored(&device, "MultiplayerAutoEndTurn").as_deref(),
        Some("1")
    );
    assert_eq!(stored(&device, "Volume.MUSIC").as_deref(), Some("0.5"));
    // An option left at its default is not written.
    assert_eq!(stored(&device, "Volume.SFX"), None);
    assert_eq!(rig.actions.client_option("MultiplayerAutoEndTurn"), Some(1));
    // A new session of the client reads them back.
    let mut again = UiActions::new(Schema::v1(), Box::new(MemorySettings::default()));
    again.set_client_store(Box::new(device.clone()), 0.8);
    assert_eq!(again.client_option("MultiplayerAutoEndTurn"), Some(1));
    assert_eq!(again.client_option("Volume.MUSIC"), Some(50));
}

#[test]
fn cancel_drops_the_drafts_and_reset_makes_them_the_defaults() {
    let mut device = MemorySettings::default();
    device.set("civ5.options.MultiplayerAutoEndTurn", "1");
    let mut rig = rig_with(&device);
    rig.one("options.edit", &[]);
    set(&mut rig, "MultiplayerAutoEndTurn", 0);
    rig.one("options.cancel", &[]);
    assert_eq!(row(&rig, "MultiplayerAutoEndTurn").value, 1);
    rig.one("options.edit", &[]);
    rig.one("options.reset", &[]);
    assert_eq!(
        row(&rig, "MultiplayerAutoEndTurn").value,
        0,
        "the classic default"
    );
    assert_eq!(row(&rig, "Volume.MUSIC").value, 80, "the page's volume");
    rig.one("options.accept", &[]);
    assert_eq!(
        stored(&device, "MultiplayerAutoEndTurn").as_deref(),
        Some("0")
    );
}

#[test]
fn an_option_that_does_not_apply_says_why_and_cannot_change() {
    let device = MemorySettings::default();
    let mut rig = rig_with(&device);
    let r = row(&rig, "AutoWorkersDontReplace");
    assert!(!r.available);
    assert_eq!(r.why, Some(Text::key("options.why.server", &[])));
    rig.one("options.edit", &[]);
    set(&mut rig, "AutoWorkersDontReplace", 1);
    assert_eq!(rig.why(), vec!["option_unavailable"]);
    assert_eq!(row(&rig, "AutoWorkersDontReplace").value, 0);
    rig.one("options.accept", &[]);
    assert_eq!(stored(&device, "AutoWorkersDontReplace"), None);
    set(&mut rig, "NoSuchOption", 1);
    assert!(rig.why().contains(&"invalid"));
}

#[test]
fn a_volume_is_heard_at_once_and_a_cancel_brings_the_stored_one_back() {
    let mut device = MemorySettings::default();
    device.set("civ5.options.Volume.SFX", "0.4");
    let mut rig = rig_with(&device);
    rig.one("options.edit", &[]);
    set(&mut rig, "Volume.SFX", 130);
    assert_eq!(row(&rig, "Volume.SFX").value, 100, "clamped");
    assert_eq!(rig.actions.take_volume_changes(), vec![("SFX", 1.0)]);
    rig.one("options.cancel", &[]);
    assert_eq!(rig.actions.take_volume_changes(), vec![("SFX", 0.4)]);
    assert!(rig.actions.take_volume_changes().is_empty());
}

#[test]
fn a_hotkey_is_rebound_stored_and_remapped() {
    let device = MemorySettings::default();
    let mut rig = rig_with(&device);
    rig.one("options.edit", &[]);
    rig.one("hotkey.capture", &[("id", s("F6"))]);
    assert_eq!(rig.ui().hotkey_capture.as_deref(), Some("F6"));
    assert!(rig.actions.capturing_key());
    // A modifier alone waits for the key.
    assert!(rig.actions.capture_key("ShiftLeft"));
    assert!(rig.actions.capture_key("Shift+KeyY"));
    rig.send(vec![]);
    let f6 = rig.ui().hotkeys.get(Key::from_text("F6")).cloned().unwrap();
    assert_eq!(&*f6.key, "Shift+KeyY");
    assert_eq!(&*f6.label, "Shift+Y");
    assert_eq!(f6.name, Text::key("hotkeys.top.menu.tech", &[]));
    assert_eq!(rig.ui().hotkey_capture, None);
    // Not in force before the window's accept.
    assert_eq!(rig.actions.remap_key("Shift+KeyY"), Remap::Same);
    rig.one("options.accept", &[]);
    assert_eq!(
        stored(&device, "UI2Hotkeys").as_deref(),
        Some("F6=Shift+KeyY")
    );
    assert_eq!(rig.actions.remap_key("Shift+KeyY"), Remap::To("F6".into()));
    assert_eq!(
        rig.actions.remap_key("F6"),
        Remap::Drop,
        "the old key is free"
    );
    assert_eq!(rig.actions.remap_key("F2"), Remap::Same);
    // Another session reads the binding back.
    let mut again = UiActions::new(Schema::v1(), Box::new(MemorySettings::default()));
    again.set_client_store(Box::new(device.clone()), 1.0);
    assert_eq!(again.remap_key("Shift+KeyY"), Remap::To("F6".into()));
}

#[test]
fn a_key_another_binding_has_or_a_fixed_key_is_refused_as_a_clash() {
    let device = MemorySettings::default();
    let mut rig = rig_with(&device);
    rig.one("options.edit", &[]);
    rig.one("hotkey.capture", &[("id", s("F6"))]);
    rig.actions.capture_key("Ctrl+KeyM");
    rig.send(vec![]);
    assert_eq!(rig.ui().hotkey_clash.as_deref(), Some("Ctrl+KeyM"));
    let f6 = rig.ui().hotkeys.get(Key::from_text("F6")).cloned().unwrap();
    assert_eq!(&*f6.key, "F6", "unchanged");
    rig.one("hotkey.capture", &[("id", s("F6"))]);
    rig.actions.capture_key("Enter");
    rig.send(vec![]);
    assert_eq!(rig.ui().hotkey_clash.as_deref(), Some("Enter"));
    // A fixed row does not wait for a key; Escape stops waiting.
    rig.one("hotkey.capture", &[("id", s("Enter"))]);
    assert!(!rig.actions.capturing_key());
    assert!(rig.ui().hotkeys.get(Key::from_text("Enter")).unwrap().fixed);
    rig.one("hotkey.capture", &[("id", s("F6"))]);
    assert!(rig.actions.capture_key("Escape"));
    assert!(!rig.actions.capturing_key());
    assert!(!rig.actions.capture_key("KeyQ"), "nobody waits");
    // Two parts on one key are one row.
    let f7 = rig.ui().hotkeys.get(Key::from_text("F7")).cloned().unwrap();
    assert_eq!(f7.more, 1);
}

#[test]
fn moving_a_key_onto_the_free_default_of_a_moved_one_and_reset() {
    let device = MemorySettings::default();
    let mut rig = rig_with(&device);
    rig.one("options.edit", &[]);
    rig.one("hotkey.capture", &[("id", s("F6"))]);
    rig.actions.capture_key("KeyT");
    rig.one("hotkey.capture", &[("id", s("Ctrl+KeyM"))]);
    rig.actions.capture_key("F6");
    rig.send(vec![]);
    assert_eq!(rig.ui().hotkey_clash, None);
    // F6 cannot come back while the menu has it.
    rig.one("hotkey.clear", &[("id", s("F6"))]);
    assert_eq!(rig.ui().hotkey_clash.as_deref(), Some("F6"));
    rig.one("options.accept", &[]);
    assert_eq!(rig.actions.remap_key("F6"), Remap::To("Ctrl+KeyM".into()));
    assert_eq!(rig.actions.remap_key("KeyT"), Remap::To("F6".into()));
    assert_eq!(rig.actions.remap_key("Ctrl+KeyM"), Remap::Drop);
    rig.one("options.edit", &[]);
    rig.one("options.reset", &[]);
    rig.one("options.accept", &[]);
    assert_eq!(
        stored(&device, "UI2Hotkeys"),
        None,
        "the defaults are not stored"
    );
    assert_eq!(rig.actions.remap_key("KeyT"), Remap::Same);
}

#[test]
fn the_game_command_keys_of_the_game_menu_are_rows_too() {
    let device = MemorySettings::default();
    let mut rig = rig_with(&device);
    let action = |key: &str, name: &str, default: Option<&str>| GameMenuHotkeysActions {
        key: Arc::from(key),
        name: Text::raw(name),
        default: default.map(Arc::from),
        ..GameMenuHotkeysActions::default()
    };
    let group = GameMenuHotkeys {
        key: Arc::from("commands"),
        actions: KeyedList::from_vec(vec![
            action("MISSION_FORTIFY", "Fortify", Some("KeyF")),
            action("MISSION_ALERT", "Alert", Some("KeyF")),
            action("MISSION_EMBARK", "Embark", None),
        ])
        .unwrap(),
        ..GameMenuHotkeys::default()
    };
    rig.put(Some(GameMenu {
        hotkeys: KeyedList::from_vec(vec![group]).unwrap(),
        ..GameMenu::default()
    }));
    rig.one("options.edit", &[]);
    let f = rig
        .ui()
        .hotkeys
        .get(Key::from_text("KeyF"))
        .cloned()
        .unwrap();
    assert_eq!((&*f.group, f.more), ("game", 1));
    assert_eq!(f.name, Text::raw("Fortify"));
    assert_eq!(
        rig.ui().hotkeys.iter().count(),
        5,
        "4 skin keys, 1 game key"
    );
    rig.one("hotkey.capture", &[("id", s("KeyF"))]);
    rig.actions.capture_key("KeyG");
    rig.one("options.accept", &[]);
    assert_eq!(rig.actions.remap_key("KeyG"), Remap::To("KeyF".into()));
}

/// The rig at turn 42 with the turn button on "next turn" and the auto end on (or off).
fn auto_rig(on: bool) -> Rig {
    let mut device = MemorySettings::default();
    if on {
        device.set("civ5.options.MultiplayerAutoEndTurn", "1");
    }
    let mut rig = rig_with(&device);
    rig.turn(Turn {
        state: TurnUi::NextTurn,
        ..Turn::default()
    });
    rig
}

fn frame(rig: &mut Rig) -> bool {
    let sent = rig
        .actions
        .auto_end_turn(&mut rig.store, &mut rig.sink, rig.now);
    rig.store.take_frame();
    sent
}

#[test]
fn the_auto_end_ends_a_free_turn_only_when_the_option_is_on() {
    let mut off = auto_rig(false);
    assert!(!frame(&mut off));
    assert!(off.commands().is_empty());
    let mut on = auto_rig(true);
    assert!(frame(&mut on));
    assert_eq!(on.commands(), vec![Command::EndTurn { force: false }]);
    // The order waits: nothing more is sent.
    assert!(!frame(&mut on));
    assert_eq!(on.commands().len(), 1);
}

#[test]
fn a_blocker_a_window_or_a_selection_holds_the_auto_end() {
    let mut rig = auto_rig(true);
    rig.turn(Turn {
        state: TurnUi::UnitNeedsOrders,
        ..Turn::default()
    });
    assert!(!frame(&mut rig));
    rig.turn(Turn {
        state: TurnUi::NextTurn,
        ..Turn::default()
    });
    rig.one("window.open", &[("window", s("research"))]);
    assert!(!frame(&mut rig));
    rig.one("window.close", &[("window", s("research"))]);
    // A selected unit holds Part A.
    rig.put(ui2_model::generated::Map::default());
    let mut ui = rig.ui().clone();
    ui.selection = Selection {
        unit: Some(common::key(5)),
        ..Selection::default()
    };
    rig.actions
        .adopt_ui(ui2_model::Model::to_value(&ui), &mut rig.store)
        .unwrap();
    rig.store.take_frame();
    for _ in 0..10 {
        assert!(!frame(&mut rig));
    }
    assert!(rig.commands().is_empty());
}

#[test]
fn after_an_order_the_turn_ends_three_frames_later_even_with_a_selection() {
    let mut rig = auto_rig(true);
    let mut ui = rig.ui().clone();
    ui.selection = Selection {
        unit: Some(common::key(5)),
        ..Selection::default()
    };
    // An order of the seat waits for its answer.
    ui.pending = KeyedList::from_vec(vec![ui2_model::generated::Pending {
        seq: 1,
        intent: Arc::from("unit.action"),
        target: Arc::from("unit:0:5"),
        since_ms: 0,
    }])
    .unwrap();
    rig.actions
        .adopt_ui(ui2_model::Model::to_value(&ui), &mut rig.store)
        .unwrap();
    assert!(!frame(&mut rig), "held while the order waits");
    ui.pending = KeyedList::new();
    rig.actions
        .adopt_ui(ui2_model::Model::to_value(&ui), &mut rig.store)
        .unwrap();
    rig.now += 16;
    for _ in 0..3 {
        assert!(!frame(&mut rig));
    }
    assert!(frame(&mut rig), "Part B");
}

#[test]
fn a_take_back_latches_the_auto_end_off_until_the_next_turn() {
    let mut rig = auto_rig(true);
    rig.turn(Turn {
        state: TurnUi::Ended,
        finished: true,
        ..Turn::default()
    });
    assert!(!frame(&mut rig));
    // The player took the end back.
    rig.turn(Turn {
        state: TurnUi::NextTurn,
        ..Turn::default()
    });
    for _ in 0..5 {
        assert!(!frame(&mut rig));
    }
    assert!(rig.commands().is_empty());
    // A new turn lifts the latch.
    rig.put(ui2_model::generated::Game {
        turn: 43,
        phase: ui2_model::generated::Phase::Humans,
        ..Default::default()
    });
    assert!(frame(&mut rig));
}
