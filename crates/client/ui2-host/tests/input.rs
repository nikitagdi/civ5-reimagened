//! Input of a running UI v2 between frames, as the application routes it:
//!
//! - a click on a panel is the skin's (`Consumed`), and the intent of its markup reaches the actions module in the
//!   next frame;
//! - a click off the panels is the world's (`PassToWorld`) and asks nothing of the skin;
//! - a click on the map that the application's picker turned into a plot (`map.click`) selects a unit and orders
//!   the selected one through the actions module;
//! - a press on the minimap is a `MinimapClick` at the point of the picture.

mod support;

use std::sync::Arc;

use platform::{
    InputEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
    PointerPhase,
};
use protocol::Command;
use replica::testing::FakeRead;
use replica::{ReplicaStep, State};
use support::{FakeNet, game};
use ui2_actions::Sent;
use ui2_host::{
    Cursor, Env, InputOutcome, MAP_CLICK, MapButton, ReplicaView, V2Frame, V2FrameReport, V2Init,
    V2Ui, map_click,
};
use ui2_model::r#gen::Ui;
use ui2_model::wire::Origin;
use ui2_model::{Key, Value};
use ui2_package::profile::action::{Action, Arg, Event};
use ui2_package::profile::expr::parse;
use ui2_package::style::{Len, LenAuto, Position, StyleRecord};
use ui2_package::{BindKind, Layer, Package};
use ui2_runtime::testkit::{Kit, TNode};

const TAKEN: InputOutcome = InputOutcome::Consumed;
const WORLD: InputOutcome = InputOutcome::PassToWorld;

fn at(x: f32, y: f32, w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::px(x));
    r.top = LenAuto::Len(Len::px(y));
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    r
}

/// The HUD layer (1280 x 720) with the `menu` button (10, 10, 100 x 40; cursor pointer; a click toggles the menu
/// window) and the minimap picture (1000, 500, 200 x 100).
fn package() -> Arc<Package> {
    let mut kit = Kit::new();
    let set = |kit: &mut Kit, r: StyleRecord| {
        let id = kit.record(r);
        kit.plain(id)
    };
    let layer = set(&mut kit, at(0.0, 0.0, 1280.0, 720.0));
    let mut button = at(10.0, 10.0, 100.0, 40.0);
    button.cursor = Cursor::Pointer;
    let button = set(&mut kit, button);
    let minimap = set(&mut kit, at(1000.0, 500.0, 200.0, 100.0));
    let toggle = Action::Intent {
        name: "window.toggle".into(),
        args: vec![Arg {
            name: Some("window".into()),
            value: parse("'menu'").expect("expression"),
        }],
    };
    let menu = kit.str("menu");
    let mm = kit.str("minimap");
    let aref = kit.asset_ref("render:minimap");
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, layer).children([
            TNode::el(button).part(menu).bind(BindKind::Event {
                ev: Event::Click,
                actions: vec![toggle],
            }),
            TNode::asset(aref, minimap).part(mm),
        ])],
    );
    kit.finish().expect("package")
}

fn pointer(phase: PointerPhase, x: f64, y: f64, button: Option<MouseButton>) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        id: PointerId::MOUSE,
        kind: PointerKind::Mouse,
        phase,
        position: PhysicalPosition { x, y },
        button,
        modifiers: Modifiers::default(),
    })
}

struct Rig {
    ui: V2Ui,
    game: FakeRead,
    net: FakeNet,
    now: f64,
}

impl Rig {
    fn new() -> Rig {
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
        // These tests look at one frame's model: every root is built in the first frame.
        ui.set_read_budget(u32::MAX);
        let mut net = FakeNet::default();
        net.session.connected = true;
        net.session.me = Some(0);
        let mut rig = Rig {
            ui,
            game: game(),
            net,
            now: 0.0,
        };
        rig.run(&[ReplicaStep::Loaded { turn: 87 }]);
        rig
    }

    fn run(&mut self, steps: &[ReplicaStep]) -> V2FrameReport {
        self.now += 16.0;
        let view = ReplicaView {
            state: State::Live,
            catching_up: false,
            generation: 1,
            applied_jseq: 0,
            read: Some(&mut self.game),
        };
        let r = self.ui.frame(V2Frame {
            replica: Some(view),
            steps,
            net: Some(&mut self.net),
            events: &[],
            mods: None,
            now_ms: self.now,
        });
        // The runtime does not paint game asset pictures (the minimap) itself; nothing else may fail.
        let other: Vec<_> = r
            .errors
            .iter()
            .filter(|e| !e.msg.contains("game asset"))
            .collect();
        assert!(other.is_empty(), "{other:?}");
        r
    }

    fn frame(&mut self) -> V2FrameReport {
        self.run(&[])
    }

    fn send(&mut self, ev: InputEvent) -> InputOutcome {
        self.now += 1.0;
        self.ui.handle_input(&ev, self.now)
    }

    /// A move, a press and a release of the left button at one point: the outcomes of the press and the release.
    fn click(&mut self, x: f64, y: f64) -> (InputOutcome, InputOutcome) {
        self.send(pointer(PointerPhase::Move, x, y, None));
        let down = self.send(pointer(PointerPhase::Down, x, y, Some(MouseButton::Left)));
        let up = self.send(pointer(PointerPhase::Up, x, y, Some(MouseButton::Left)));
        (down, up)
    }

    fn model_ui(&self) -> &Ui {
        self.ui.store().get::<Ui>().expect("the ui root")
    }
}

#[test]
fn a_click_on_a_panel_is_the_skins_and_its_intent_runs_in_the_next_frame() {
    let mut rig = Rig::new();
    assert!(!rig.model_ui().windows.menu.open);
    assert_eq!(rig.click(50.0, 30.0), (TAKEN, TAKEN));
    // Between frames nothing is applied yet.
    assert!(!rig.model_ui().windows.menu.open);
    assert_eq!(rig.ui.cursor(), Cursor::Pointer);
    let r = rig.frame();
    assert_eq!(r.intents, 1);
    assert_eq!(r.refused, 0);
    assert!(rig.model_ui().windows.menu.open);
    assert_eq!(r.cursor, Cursor::Pointer, "the frame reports the cursor");
    // The intent was taken once.
    assert_eq!(rig.frame().intents, 0);
    assert!(rig.net.sink.sent.is_empty(), "windows are local");
}

#[test]
fn a_click_off_the_panels_is_the_worlds() {
    let mut rig = Rig::new();
    assert_eq!(rig.click(640.0, 400.0), (WORLD, WORLD));
    assert_eq!(
        rig.ui.cursor(),
        Cursor::Auto,
        "the world decides the cursor"
    );
    let r = rig.frame();
    assert_eq!(r.intents, 0);
    assert!(!rig.model_ui().windows.menu.open);
    assert_eq!(r.cursor, Cursor::Auto);
    // A press that started in the world stays the world's over a panel, up to its release.
    assert_eq!(
        rig.send(pointer(
            PointerPhase::Down,
            640.0,
            400.0,
            Some(MouseButton::Left)
        )),
        WORLD
    );
    assert_eq!(
        rig.send(pointer(PointerPhase::Move, 50.0, 30.0, None)),
        WORLD
    );
    assert_eq!(
        rig.send(pointer(
            PointerPhase::Up,
            50.0,
            30.0,
            Some(MouseButton::Left)
        )),
        WORLD
    );
    assert_eq!(
        rig.frame().intents,
        0,
        "the release over the button is no click"
    );
}

#[test]
fn the_map_click_intent_has_the_form_of_the_catalog() {
    let i = map_click(12, -3, MapButton::Right);
    assert_eq!(&*i.name, MAP_CLICK);
    assert_eq!(i.origin, Origin::Map);
    assert_eq!(
        i.args,
        [
            (
                Arc::from("plot"),
                Value::Struct(vec![Value::I32(12), Value::I32(-3)])
            ),
            (Arc::from("button"), Value::Enum(1)),
        ]
    );
}

#[test]
fn a_map_click_selects_a_unit_and_a_right_click_orders_it() {
    let mut rig = Rig::new();
    assert_eq!(rig.model_ui().selection.unit, None);
    // The picker found the plot of unit 1.
    rig.ui.click_map(10, 4, MapButton::Left);
    let r = rig.frame();
    assert_eq!((r.intents, r.refused), (1, 0));
    assert_eq!(rig.model_ui().selection.unit, Some(Key::new(0, 1)));
    // A left click on an empty plot keeps the selection; a right click sends the selected unit there.
    rig.ui.click_map(12, 5, MapButton::Right);
    rig.frame();
    assert_eq!(
        rig.net.sink.sent,
        [Sent::Command {
            id: 1,
            cmd: Command::UnitMoveTo {
                unit: 1,
                x: 12,
                y: 5
            },
            turn: 87,
        }]
    );
}

#[test]
fn a_press_on_the_minimap_is_a_minimap_click() {
    let mut rig = Rig::new();
    let out = rig.send(pointer(
        PointerPhase::Down,
        1100.0,
        525.0,
        Some(MouseButton::Left),
    ));
    let InputOutcome::MinimapClick { u, v } = out else {
        panic!("{out:?}");
    };
    assert!((u - 0.5).abs() < 1e-3 && (v - 0.25).abs() < 1e-3, "{u} {v}");
    // Dragged on: still the minimap, clamped to its edge.
    let out = rig.send(pointer(PointerPhase::Move, 1250.0, 525.0, None));
    assert!(
        matches!(out, InputOutcome::MinimapClick { u, .. } if (u - 1.0).abs() < 1e-3),
        "{out:?}"
    );
    assert_eq!(
        rig.send(pointer(
            PointerPhase::Up,
            1250.0,
            525.0,
            Some(MouseButton::Left)
        )),
        TAKEN
    );
    assert_eq!(rig.frame().intents, 0);
}
