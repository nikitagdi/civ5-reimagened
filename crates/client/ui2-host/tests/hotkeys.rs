//! The player's hotkeys in front of the skin's runtime: the options window's rows of the skin's `data-hotkey` keys,
//! the capture of a new key, and a bound key that reaches the runtime as the key the skin binds.

use std::sync::Arc;

use platform::{InputEvent, Key, KeyEvent, Modifiers};
use ui2_actions::{MemorySettings, SettingsStore};
use ui2_host::{Env, InputOutcome, V2Frame, V2Init, V2Ui};
use ui2_model::r#gen::Ui;
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{Key as ModelKey, Value};
use ui2_package::profile::action::{Action, Arg, Event};
use ui2_package::profile::expr::parse;
use ui2_package::style::{Len, LenAuto, Position, StyleRecord};
use ui2_package::{BindKind, Layer, Package};
use ui2_runtime::testkit::{Kit, TNode};

/// The HUD with one button, part `menu`, F6 toggling the menu window.
fn package() -> Arc<Package> {
    let mut kit = Kit::new();
    let at = |x: f32, y: f32, w: f32, h: f32| {
        let mut r = StyleRecord::initial();
        r.position = Position::Absolute;
        r.left = LenAuto::Len(Len::px(x));
        r.top = LenAuto::Len(Len::px(y));
        r.width = LenAuto::Len(Len::px(w));
        r.height = LenAuto::Len(Len::px(h));
        r
    };
    let layer = kit.record(at(0.0, 0.0, 1280.0, 720.0));
    let layer = kit.plain(layer);
    let button = kit.record(at(10.0, 10.0, 100.0, 40.0));
    let button = kit.plain(button);
    let toggle = Action::Intent {
        name: "window.toggle".into(),
        args: vec![Arg {
            name: Some("window".into()),
            value: parse("'menu'").expect("expression"),
        }],
    };
    let menu = kit.str("menu");
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, layer).children([TNode::el(button)
                .part(menu)
                .hotkey("F6", 0)
                .bind(BindKind::Event {
                    ev: Event::Click,
                    actions: vec![toggle],
                })]),
        ],
    );
    kit.finish().expect("package")
}

fn key(code: &str, pressed: bool) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Unidentified,
        code: Some(code.to_string()),
        pressed,
        repeat: false,
        modifiers: Modifiers::default(),
    })
}

struct Rig {
    ui: V2Ui,
    now: f64,
}

impl Rig {
    fn new(device: &MemorySettings) -> Rig {
        let mut ui = V2Ui::new(
            package(),
            V2Init {
                env: Env {
                    viewport_px: (1280, 720),
                    ..Env::default()
                },
                ..V2Init::default()
            },
        )
        .expect("the skin loads");
        ui.set_client_store(Box::new(device.clone()), 1.0);
        let mut rig = Rig { ui, now: 0.0 };
        rig.frame();
        rig
    }

    fn frame(&mut self) {
        self.now += 16.0;
        self.ui.frame(V2Frame {
            world: None,
            net: None,
            events: &[],
            mods: None,
            now_ms: self.now,
        });
    }

    /// A press and a release: the outcome of the press.
    fn press(&mut self, code: &str) -> InputOutcome {
        self.now += 1.0;
        let out = self.ui.handle_input(&key(code, true), self.now);
        self.ui.handle_input(&key(code, false), self.now);
        out
    }

    fn model_ui(&self) -> &Ui {
        self.ui.store().get::<Ui>().expect("the ui root")
    }
}

#[test]
fn the_skins_hotkeys_are_rows_of_the_options_window() {
    let rig = Rig::new(&MemorySettings::default());
    let row = rig
        .model_ui()
        .hotkeys
        .get(ModelKey::from_text("F6"))
        .cloned()
        .expect("the F6 row");
    assert_eq!(&*row.group, "skin");
    assert_eq!(row.name, ui2_model::Text::key("hotkeys.menu", &[]));
}

#[test]
fn a_bound_key_clicks_what_the_default_key_did_and_the_default_key_goes_to_the_world() {
    let mut device = MemorySettings::default();
    device.set("civ5.options.UI2Hotkeys", "F6=KeyY");
    let mut rig = Rig::new(&device);
    assert_eq!(rig.press("F6"), InputOutcome::PassToWorld);
    rig.frame();
    assert!(!rig.model_ui().windows.menu.open, "F6 moved away");
    assert_eq!(rig.press("KeyY"), InputOutcome::Consumed);
    rig.frame();
    assert!(rig.model_ui().windows.menu.open, "Y is the menu's key now");
}

#[test]
fn a_waiting_row_takes_the_next_key_press() {
    let device = MemorySettings::default();
    let mut rig = Rig::new(&device);
    rig.ui.push_intent(WireIntent {
        name: Arc::from("options.edit"),
        args: vec![],
        origin: Origin::Script,
    });
    rig.ui.push_intent(WireIntent {
        name: Arc::from("hotkey.capture"),
        args: vec![(Arc::from("id"), Value::Str(Arc::from("F6")))],
        origin: Origin::Script,
    });
    rig.frame();
    assert_eq!(rig.model_ui().hotkey_capture.as_deref(), Some("F6"));
    // The press is the new key, not a click of anything.
    assert_eq!(rig.press("KeyU"), InputOutcome::Consumed);
    rig.frame();
    let row = rig
        .model_ui()
        .hotkeys
        .get(ModelKey::from_text("F6"))
        .cloned()
        .unwrap();
    assert_eq!(&*row.key, "KeyU");
    assert!(!rig.model_ui().windows.menu.open);
    rig.ui.push_intent(WireIntent {
        name: Arc::from("options.accept"),
        args: vec![],
        origin: Origin::Script,
    });
    rig.frame();
    assert_eq!(
        device
            .contents()
            .get("civ5.options.UI2Hotkeys")
            .map(String::as_str),
        Some("F6=KeyU")
    );
    assert_eq!(rig.press("KeyU"), InputOutcome::Consumed);
    rig.frame();
    assert!(rig.model_ui().windows.menu.open);
}
