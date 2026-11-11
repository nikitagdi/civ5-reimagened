//! Input (test 8 of the runtime spec): hit-testing through layers, stacking, transforms, clips and
//! `pointer-events`; hover, active and the cursor; pointer capture; the wheel over scroll containers; modal
//! screens of the `window` layer; the minimap. Then markup actions (`intent` with its arguments, `set`,
//! `toggle`, `call`, `stop`, bubbling), focus and Tab, Enter and Space, Esc, `data-hotkey` by layer, and
//! tooltips.

use std::path::PathBuf;
use std::sync::Arc;

use platform::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, NamedKey, PhysicalPosition, PointerEvent,
    PointerId, PointerKind, PointerPhase, WheelDelta, WheelEvent,
};
use ui2_draw::Scene;
use ui2_model::wire::{Origin, ScriptCtxId, StateOp, WireIntent};
use ui2_model::{Key as MKey, ModelStore, Sample, Schema, Value as MValue};
use ui2_package::profile::action::{Action, Arg, Event};
use ui2_package::profile::expr::{Expr, parse};
use ui2_package::style::{
    ColorV, Cursor, Display, IntAuto, Len, LenAuto, Overflow, PointerEvents, Position, StyleRecord,
    TFn,
};
use ui2_package::{
    BindKind, Cond, CondKind, Layer, Lit, NodeKind, Package, StyleSetId, TemplateId, hotkey_mods,
    node_flags,
};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{
    Env, FrameOutput, InputOutcome, NodeId, PointerCapture, Runtime, Services, StateBits,
};
use ui2_text::TextEngine;

const PASS: InputOutcome = InputOutcome::PassToWorld;
const TAKEN: InputOutcome = InputOutcome::Consumed;

fn at(x: f32, y: f32, w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::px(x));
    r.top = LenAuto::Len(Len::px(y));
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    r
}

fn with(mut r: StyleRecord, f: impl FnOnce(&mut StyleRecord)) -> StyleRecord {
    f(&mut r);
    r
}

/// A package under construction, with helpers for named boxes and layer roots.
struct Fx {
    kit: Kit,
}

impl Fx {
    fn new() -> Self {
        Self { kit: Kit::new() }
    }

    fn set(&mut self, r: StyleRecord) -> StyleSetId {
        let id = self.kit.record(r);
        self.kit.plain(id)
    }

    fn el(&mut self, name: &str, r: StyleRecord) -> TNode {
        let set = self.set(r);
        let p = self.kit.str(name);
        TNode::el(set).part(p)
    }

    fn layer(&mut self, layer: Layer, name: &str) -> TNode {
        let set = self.set(at(0.0, 0.0, 1000.0, 800.0));
        let p = self.kit.str(name);
        TNode::layer(layer, set).part(p)
    }
}

/// The scene of most tests (CSS px; layers 1000 x 800):
///
/// - map layer: `map-panel` (0, 700, 100 x 50);
/// - HUD layer: `panel` (10, 10, 200 x 100, cursor pointer) with `btn` (20, 20, 50 x 30, hover turns it red);
///   `ghost` (300, 10, 100 x 100, `pointer-events: none`) with `ghost-child` (10, 10, 20 x 20); `scroll`
///   (500, 10, 100 x 100, `overflow-y: auto`) with `tall` (400 px high) and `mark` (its first 20 px);
///   `minimap` (600, 200, 200 x 100, `render:minimap`); `rot` (10, 300, 50 x 50, `translate(100px, 0)`);
///   `clip` (300, 300, 50 x 50, `overflow: hidden`) with `spill` (200 x 200); `zbox` (10, 400) with `zlow`
///   (z-index 1) before `zhigh` in the tree but drawn above it (both at its corner, 50 x 50);
/// - window layer: `screen` (700, 400, 200 x 200), a screen when `modal` is set.
fn package(modal: bool) -> Arc<Package> {
    let mut f = Fx::new();
    let minimap = f.kit.asset_ref("render:minimap");
    let map = f.layer(Layer::Map, "map");
    let map_panel = f.el("map-panel", at(0.0, 700.0, 100.0, 50.0));
    let map = map.child(map_panel);

    let hud = f.layer(Layer::Hud, "hud");
    let panel = f.el(
        "panel",
        with(at(10.0, 10.0, 200.0, 100.0), |r| r.cursor = Cursor::Pointer),
    );
    let plain = with(at(20.0, 20.0, 50.0, 30.0), |r| r.cursor = Cursor::Pointer);
    let red = with(plain.clone(), |r| {
        r.background_color = ColorV::Lit([255, 0, 0, 255]);
    });
    let (plain, red) = (f.kit.record(plain), f.kit.record(red));
    let btn_set = f.kit.set(
        vec![Cond {
            up: 0,
            kind: CondKind::Hover,
        }],
        vec![plain, red],
    );
    let btn_name = f.kit.str("btn");
    let btn = TNode::el(btn_set).part(btn_name);
    let panel = panel.child(btn);

    let ghost = f.el(
        "ghost",
        with(at(300.0, 10.0, 100.0, 100.0), |r| {
            r.pointer_events = PointerEvents::None;
        }),
    );
    let ghost_child = f.el("ghost-child", at(10.0, 10.0, 20.0, 20.0));
    let ghost = ghost.child(ghost_child);

    let scroll = f.el(
        "scroll",
        with(at(500.0, 10.0, 100.0, 100.0), |r| {
            r.overflow_y = Overflow::Auto;
            r.overflow_x = Overflow::Hidden;
        }),
    );
    let mut tall = StyleRecord::initial();
    tall.height = LenAuto::Len(Len::px(400.0));
    tall.position = Position::Relative;
    let tall = f.el("tall", tall);
    let mark = f.el("mark", at(0.0, 0.0, 100.0, 20.0));
    let scroll = scroll.child(tall.child(mark));

    let mm_set = f.set(at(600.0, 200.0, 200.0, 100.0));
    let mm_name = f.kit.str("minimap");
    let minimap = TNode::asset(minimap, mm_set).part(mm_name);

    let rot = f.el(
        "rot",
        with(at(10.0, 300.0, 50.0, 50.0), |r| {
            r.transform = vec![TFn::Translate(Len::px(100.0), Len::px(0.0))];
        }),
    );
    let clip = f.el(
        "clip",
        with(at(300.0, 300.0, 50.0, 50.0), |r| {
            r.overflow_x = Overflow::Hidden;
            r.overflow_y = Overflow::Hidden;
        }),
    );
    let spill = f.el("spill", at(0.0, 0.0, 200.0, 200.0));
    let clip = clip.child(spill);
    // Stacking inside one chunk (chunks themselves are drawn in document order).
    let zbox = f.el("zbox", at(10.0, 400.0, 100.0, 100.0));
    let zlow = f.el(
        "zlow",
        with(at(0.0, 0.0, 50.0, 50.0), |r| r.z_index = IntAuto::Int(1)),
    );
    let zhigh = f.el("zhigh", at(0.0, 0.0, 50.0, 50.0));
    let zbox = zbox.children([zlow, zhigh]);
    let hud = hud.children([panel, ghost, scroll, minimap, rot, clip, zbox]);

    let window = f.layer(Layer::Window, "window");
    let mut screen = f.el("screen", at(700.0, 400.0, 200.0, 200.0));
    if modal {
        screen = screen.screen("production");
    }
    let window = window.child(screen);
    f.kit.template("doc", vec![map, hud, window]);
    f.kit.finish().expect("package")
}

struct Host {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    now: f64,
}

impl Host {
    fn with(pkg: Arc<Package>, env: Env) -> Self {
        let mut h = Self {
            rt: Runtime::new(pkg, env, Services::default()).expect("runtime"),
            engine: TextEngine::new(),
            store: ModelStore::new(Schema::v1()),
            scene: Scene::new(1.0),
            now: 0.0,
        };
        let out = h.frame();
        // Game asset pictures (the minimap) are not painted by the runtime yet; nothing else may fail.
        let other: Vec<_> = out
            .errors
            .iter()
            .filter(|e| !e.msg.contains("game asset"))
            .collect();
        assert!(other.is_empty(), "{other:?}");
        h
    }

    fn new(modal: bool) -> Self {
        Self::with(package(modal), Env::default())
    }

    fn frame(&mut self) -> FrameOutput {
        self.now += 16.0;
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        self.rt
            .frame(self.now, &self.store, &mut self.engine, &mut self.scene)
    }

    fn part(&self, name: &str) -> NodeId {
        self.rt
            .find_part(name)
            .unwrap_or_else(|| panic!("part {name}"))
    }

    fn send(&mut self, ev: InputEvent) -> InputOutcome {
        self.now += 1.0;
        self.rt.handle_input(&ev, self.now)
    }

    fn ptr(&mut self, phase: PointerPhase, x: f64, y: f64) -> InputOutcome {
        let button =
            matches!(phase, PointerPhase::Down | PointerPhase::Up).then_some(MouseButton::Left);
        self.send(pointer(phase, x, y, button))
    }

    fn click(&mut self, x: f64, y: f64) -> (InputOutcome, InputOutcome) {
        self.ptr(PointerPhase::Move, x, y);
        (
            self.ptr(PointerPhase::Down, x, y),
            self.ptr(PointerPhase::Up, x, y),
        )
    }

    fn wheel(&mut self, x: f64, y: f64, lines: f32) -> InputOutcome {
        self.send(InputEvent::Wheel(WheelEvent {
            delta: WheelDelta::Lines { x: 0.0, y: lines },
            position: PhysicalPosition { x, y },
            modifiers: Modifiers::default(),
        }))
    }

    fn state(&self, name: &str) -> StateBits {
        self.rt.node(self.part(name)).expect("node").state()
    }
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

#[test]
fn input_hit_test_follows_layers_stacking_transforms_and_clips() {
    let h = Host::new(false);
    let at = |x, y| h.rt.node_at(x, y);
    assert_eq!(
        at(45.0, 35.0),
        Some(h.part("btn")),
        "the child above its parent"
    );
    assert_eq!(at(15.0, 15.0), Some(h.part("panel")));
    assert_eq!(
        at(950.0, 50.0),
        None,
        "an empty part of a layer is the world"
    );
    assert_eq!(at(50.0, 720.0), Some(h.part("map-panel")), "the map layer");
    // `pointer-events: none` lets the point through the box but not through its children.
    assert_eq!(at(305.0, 15.0), None);
    assert_eq!(at(315.0, 25.0), Some(h.part("ghost-child")));
    // A transform moves the box that is hit.
    assert_eq!(at(120.0, 310.0), Some(h.part("rot")));
    assert_eq!(at(20.0, 310.0), None);
    // Overflow clips the children.
    assert_eq!(at(320.0, 320.0), Some(h.part("spill")));
    assert_eq!(at(400.0, 320.0), None, "outside the clip of its parent");
    // Painting order, not tree order: the z-index 1 box is drawn and hit above the later one.
    assert_eq!(at(20.0, 410.0), Some(h.part("zlow")));
    // The window layer is above the HUD.
    assert_eq!(at(750.0, 450.0), Some(h.part("screen")));
}

#[test]
fn input_click_outside_panels_passes_to_world() {
    let mut h = Host::new(false);
    assert_eq!(h.click(950.0, 50.0), (PASS, PASS));
    assert_eq!(h.rt.pointer_capture(), PointerCapture::None);
    assert_eq!(h.ptr(PointerPhase::Move, 950.0, 60.0), PASS);
    assert_eq!(h.rt.hovered(), None);
    // A click on a panel is the UI's.
    assert_eq!(h.click(15.0, 15.0), (TAKEN, TAKEN));
}

#[test]
fn input_press_on_a_panel_captures_until_release() {
    let mut h = Host::new(false);
    assert_eq!(h.ptr(PointerPhase::Move, 45.0, 35.0), TAKEN);
    assert_eq!(h.ptr(PointerPhase::Down, 45.0, 35.0), TAKEN);
    let btn = h.part("btn");
    assert_eq!(h.rt.pointer_capture(), PointerCapture::Ui(Some(btn)));
    for name in ["btn", "panel"] {
        assert!(h.state(name).contains(StateBits::ACTIVE), "{name}");
    }
    // Dragged over the world, the press still belongs to the panel.
    assert_eq!(h.ptr(PointerPhase::Move, 950.0, 50.0), TAKEN);
    assert_eq!(h.rt.hovered(), None, "hover follows the pointer");
    // A second button while the first is held stays with the holder.
    assert_eq!(
        h.send(pointer(
            PointerPhase::Down,
            950.0,
            50.0,
            Some(MouseButton::Right)
        )),
        TAKEN
    );
    assert_eq!(
        h.send(pointer(
            PointerPhase::Up,
            950.0,
            50.0,
            Some(MouseButton::Right)
        )),
        TAKEN
    );
    assert_eq!(h.ptr(PointerPhase::Up, 950.0, 50.0), TAKEN);
    assert_eq!(h.rt.pointer_capture(), PointerCapture::None);
    assert!(!h.state("btn").contains(StateBits::ACTIVE));
    assert!(!h.state("panel").contains(StateBits::ACTIVE));

    // A press in the world keeps the world: the UI only follows the hover.
    assert_eq!(h.ptr(PointerPhase::Down, 950.0, 50.0), PASS);
    assert_eq!(h.rt.pointer_capture(), PointerCapture::World);
    assert_eq!(h.ptr(PointerPhase::Move, 45.0, 35.0), PASS);
    assert_eq!(h.rt.hovered(), Some(btn));
    assert!(!h.state("btn").contains(StateBits::ACTIVE));
    assert_eq!(h.ptr(PointerPhase::Up, 45.0, 35.0), PASS);
    assert_eq!(h.rt.pointer_capture(), PointerCapture::None);

    // A cancelled press ends the capture too; losing the window focus as well.
    h.ptr(PointerPhase::Down, 45.0, 35.0);
    assert_eq!(h.ptr(PointerPhase::Cancel, 45.0, 35.0), TAKEN);
    assert_eq!(h.rt.pointer_capture(), PointerCapture::None);
    h.ptr(PointerPhase::Down, 45.0, 35.0);
    assert_eq!(h.send(InputEvent::Focused(false)), PASS);
    assert_eq!(h.rt.pointer_capture(), PointerCapture::None);
    assert!(!h.state("btn").contains(StateBits::ACTIVE));
}

#[test]
fn input_hover_restyles_and_sets_the_cursor() {
    let mut h = Host::new(false);
    h.ptr(PointerPhase::Move, 45.0, 35.0);
    let out = h.frame();
    assert_eq!(out.cursor, Cursor::Pointer);
    assert_eq!(out.stats.restyled, 1, "only the button tests hover");
    assert_eq!(out.stats.layout_runs, 0);
    let btn = h.part("btn");
    let red = h.rt.color(h.rt.style(btn).expect("style").background_color);
    assert_eq!(red, [255, 0, 0, 255]);
    assert!(h.state("panel").contains(StateBits::HOVER), "the ancestors");
    assert!(h.state("hud").contains(StateBits::HOVER));

    // Off the UI: no hover, the world's cursor.
    assert_eq!(h.ptr(PointerPhase::Move, 950.0, 50.0), PASS);
    let out = h.frame();
    assert_eq!(out.cursor, Cursor::Auto);
    assert!(!h.state("panel").contains(StateBits::HOVER));
    // The pointer leaving the window clears the hover as well.
    h.ptr(PointerPhase::Move, 45.0, 35.0);
    h.send(pointer(PointerPhase::Left, 0.0, 0.0, None));
    assert_eq!(h.rt.hovered(), None);
    assert!(!h.state("btn").contains(StateBits::HOVER));
}

#[test]
fn input_wheel_scrolls_the_container_under_the_pointer() {
    let mut h = Host::new(false);
    let scroll = h.part("scroll");
    let mark = h.part("mark");
    assert_eq!(h.rt.node_at(550.0, 15.0), Some(mark));
    // One line down: 40 px.
    assert_eq!(h.wheel(550.0, 50.0, -1.0), TAKEN);
    assert_eq!(h.rt.node(scroll).expect("scroll").scroll(), [0.0, 40.0]);
    let s = h.frame().stats;
    assert_eq!(
        (s.layout_runs, s.chunks_rebuilt),
        (0, 1),
        "scrolling repaints its chunk, nothing is laid out"
    );
    // The children moved up with the content: the mark is gone from the top of the box.
    assert_eq!(h.rt.node_at(550.0, 15.0), Some(h.part("tall")));
    // The offset stops at the end of the content (400 - 100) and at the start.
    for _ in 0..20 {
        assert_eq!(h.wheel(550.0, 50.0, -1.0), TAKEN);
    }
    assert_eq!(h.rt.node(scroll).expect("scroll").scroll(), [0.0, 300.0]);
    for _ in 0..20 {
        h.wheel(550.0, 50.0, 1.0);
    }
    assert_eq!(h.rt.node(scroll).expect("scroll").scroll(), [0.0, 0.0]);
    // At the end of its range the wheel is still the container's; a still wheel repaints nothing.
    assert_eq!(h.wheel(550.0, 50.0, 1.0), TAKEN);
    h.frame();
    assert_eq!(h.frame().stats.chunks_rebuilt, 0);
    // Over a panel that does not scroll, and over the world, the wheel zooms the map.
    assert_eq!(h.wheel(15.0, 15.0, 1.0), PASS);
    assert_eq!(h.wheel(950.0, 50.0, 1.0), PASS);
}

#[test]
fn input_modal_screen_blocks_the_layers_below() {
    let mut h = Host::new(true);
    assert_eq!(h.rt.visible_screens(), vec![h.part("screen")]);
    // The HUD under the pointer is blocked: taken, but no hover and no press state.
    assert_eq!(h.ptr(PointerPhase::Move, 45.0, 35.0), TAKEN);
    assert_eq!(h.rt.hovered(), None);
    assert_eq!(h.ptr(PointerPhase::Down, 45.0, 35.0), TAKEN);
    assert!(!h.state("btn").contains(StateBits::ACTIVE));
    assert_eq!(h.rt.pointer_capture(), PointerCapture::Ui(None));
    assert_eq!(h.ptr(PointerPhase::Up, 45.0, 35.0), TAKEN);
    // So is the world, and the wheel.
    assert_eq!(h.click(950.0, 50.0), (TAKEN, TAKEN));
    assert_eq!(h.wheel(950.0, 50.0, 1.0), TAKEN);
    // The screen itself works.
    assert_eq!(h.ptr(PointerPhase::Move, 750.0, 450.0), TAKEN);
    assert_eq!(h.rt.hovered(), Some(h.part("screen")));

    // A hidden screen is not modal: here it is removed.
    let screen = h.part("screen");
    h.rt.remove(screen);
    h.frame();
    assert!(h.rt.visible_screens().is_empty());
    assert_eq!(h.click(950.0, 50.0), (PASS, PASS));
    assert_eq!(h.ptr(PointerPhase::Move, 45.0, 35.0), TAKEN);
    assert_eq!(h.rt.hovered(), Some(h.part("btn")));
}

#[test]
fn input_screen_hidden_by_display_none_is_not_modal() {
    let mut f = Fx::new();
    let hud = f.layer(Layer::Hud, "hud");
    let panel = f.el("panel", at(10.0, 10.0, 100.0, 100.0));
    let hud = hud.child(panel);
    let window = f.layer(Layer::Window, "window");
    let hidden = f.el(
        "screen",
        with(at(0.0, 0.0, 500.0, 500.0), |r| r.display = Display::None),
    );
    let window = window.child(hidden.screen("research"));
    f.kit.template("doc", vec![hud, window]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    assert!(h.rt.visible_screens().is_empty());
    assert_eq!(h.ptr(PointerPhase::Move, 20.0, 20.0), TAKEN);
    assert_eq!(h.rt.hovered(), Some(h.part("panel")));
    assert_eq!(h.click(900.0, 700.0), (PASS, PASS));
}

#[test]
fn input_minimap_press_and_drag_report_the_point() {
    let mut h = Host::new(false);
    h.ptr(PointerPhase::Move, 700.0, 250.0);
    assert_eq!(
        h.ptr(PointerPhase::Down, 700.0, 250.0),
        InputOutcome::MinimapClick { u: 0.5, v: 0.5 }
    );
    assert_eq!(
        h.ptr(PointerPhase::Move, 650.0, 225.0),
        InputOutcome::MinimapClick { u: 0.25, v: 0.25 }
    );
    // Dragged off the picture: clamped to its edge.
    assert_eq!(
        h.ptr(PointerPhase::Move, 950.0, 100.0),
        InputOutcome::MinimapClick { u: 1.0, v: 0.0 }
    );
    assert_eq!(h.ptr(PointerPhase::Up, 950.0, 100.0), TAKEN);
    assert_eq!(h.ptr(PointerPhase::Move, 950.0, 100.0), PASS);
    // A right press on the minimap is only taken.
    h.ptr(PointerPhase::Move, 700.0, 250.0);
    assert_eq!(
        h.send(pointer(
            PointerPhase::Down,
            700.0,
            250.0,
            Some(MouseButton::Right)
        )),
        TAKEN
    );
}

#[test]
fn input_disabled_node_is_hovered_but_never_active() {
    let mut h = Host::new(false);
    let panel = h.part("panel");
    h.rt.set_state(panel, StateBits::DISABLED, true);
    h.ptr(PointerPhase::Move, 45.0, 35.0);
    assert_eq!(h.ptr(PointerPhase::Down, 45.0, 35.0), TAKEN);
    assert!(h.state("btn").contains(StateBits::HOVER));
    assert!(
        !h.state("btn").contains(StateBits::ACTIVE),
        "inside a disabled node"
    );
    h.ptr(PointerPhase::Up, 45.0, 35.0);
}

#[test]
fn input_coordinates_are_divided_by_the_device_scale() {
    let env = Env {
        viewport_px: (2000, 1600),
        dpr: 2.0,
        ..Env::default()
    };
    let mut h = Host::with(package(false), env);
    // (45, 35) CSS px is (90, 70) physical.
    assert_eq!(h.rt.node_at(90.0, 70.0), Some(h.part("btn")));
    assert_eq!(h.rt.node_at(45.0, 35.0), Some(h.part("panel")));
    // A wheel given in pixels scrolls by CSS px.
    h.send(InputEvent::Wheel(WheelEvent {
        delta: WheelDelta::Pixels { x: 0.0, y: -60.0 },
        position: PhysicalPosition {
            x: 1100.0,
            y: 100.0,
        },
        modifiers: Modifiers::default(),
    }));
    assert_eq!(
        h.rt.node(h.part("scroll")).expect("scroll").scroll(),
        [0.0, 30.0]
    );
}

#[test]
fn input_idle_frames_stay_free_after_pointer_events() {
    let mut h = Host::new(false);
    h.ptr(PointerPhase::Move, 45.0, 35.0);
    h.click(45.0, 35.0);
    h.wheel(550.0, 50.0, -1.0);
    h.frame();
    // Moving inside the same node changes nothing.
    h.ptr(PointerPhase::Move, 46.0, 36.0);
    for _ in 0..100 {
        let s = h.frame().stats;
        assert_eq!(
            (
                s.restyled,
                s.layout_runs,
                s.chunks_rebuilt,
                s.binds_evaluated,
                s.visited
            ),
            (0, 0, 0, 0, 0)
        );
    }
    assert!(h.rt.drain_output().is_empty());
}

// Markup actions, focus, keys and tooltips.

fn e(src: &str) -> Expr {
    parse(src).unwrap_or_else(|err| panic!("{src}: {err}"))
}

fn intent(name: &str, args: &[(&str, &str)]) -> Action {
    Action::Intent {
        name: name.into(),
        args: args
            .iter()
            .map(|(n, v)| Arg {
                name: Some((*n).into()),
                value: e(v),
            })
            .collect(),
    }
}

fn call(name: &str, args: &[&str]) -> Action {
    Action::Call {
        name: name.into(),
        args: args
            .iter()
            .map(|v| Arg {
                name: None,
                value: e(v),
            })
            .collect(),
    }
}

fn on(ev: Event, actions: Vec<Action>) -> BindKind {
    BindKind::Event { ev, actions }
}

/// The scene of the keyboard and action tests (layers 1000 x 800; the viewport is 1920 x 1080):
///
/// - HUD: `bar` (0, 0, 400 x 50; click: `intent turn.end()`) with the focusable buttons `b1` (click: an
///   intent with an enum argument, `set`, `toggle`, `call` with two arguments; hotkey B), `b2` (click: an
///   intent with a key argument, then `stop`; double click: an intent with a key and a computed argument;
///   `focus`/`blur` handlers), `b3` (click: `intent turn.primary()`; hotkey D), `mod` (a mod intent with a
///   positional argument), the text field `field`, and `tip` (a tooltip, `pointerenter`/`pointerleave`
///   handlers); `edge` (a tooltip owner in the bottom right corner of the viewport); `hk-hud` (hotkey W) and
///   `ctrl-k` (hotkey Ctrl+K);
/// - window: `screen` (500, 100, 300 x 300; a screen when `modal`, hidden otherwise) with the focusable `sb`
///   and `sb2` and `hk-win` (hotkey W);
/// - tooltip layer `tips`; template 1 is the tooltip, `tip-box` (80 x 30).
fn kb_package(modal: bool) -> Arc<Package> {
    let mut f = Fx::new();
    f.kit.state_default("open", Lit::Bool(false));
    f.kit.state_default("tab", Lit::Str("cities".into()));
    let focusable = |t: TNode| t.flags(node_flags::FOCUSABLE);
    let hud = f.layer(Layer::Hud, "hud");
    let bar = f
        .el("bar", at(0.0, 0.0, 400.0, 50.0))
        .bind(on(Event::Click, vec![intent("turn.end", &[])]));
    let b1 = focusable(f.el("b1", at(0.0, 0.0, 50.0, 50.0)))
        .hotkey("KeyB", 0)
        .bind(on(
            Event::Click,
            vec![
                intent("window.open", &[("window", "'production'")]),
                Action::Set {
                    path: "state.tab".into(),
                    value: e("'units'"),
                },
                Action::Toggle {
                    path: "state.open".into(),
                },
                call("pick", &["top.gold", "'x'"]),
            ],
        ));
    let b2 = focusable(f.el("b2", at(60.0, 0.0, 50.0, 50.0)))
        .bind(on(
            Event::Click,
            vec![
                intent("camera.to_city", &[("city", "game.me")]),
                Action::Stop,
            ],
        ))
        .bind(on(
            Event::Dblclick,
            vec![intent(
                "research.choose",
                &[("tech", "game.me"), ("queue", "top.gold > 10")],
            )],
        ))
        .bind(on(Event::Focus, vec![call("f2", &[])]))
        .bind(on(Event::Blur, vec![call("b2", &[])]));
    let b3 = focusable(f.el("b3", at(120.0, 0.0, 50.0, 50.0)))
        .hotkey("KeyD", 0)
        .bind(on(Event::Click, vec![intent("turn.primary", &[])]));
    let md = f.el("mod", at(180.0, 0.0, 50.0, 50.0)).bind(on(
        Event::Click,
        vec![Action::Intent {
            name: "mods.demo.ping".into(),
            args: vec![Arg {
                name: None,
                value: e("top.gold + 1"),
            }],
        }],
    ));
    let field_set = f.set(at(240.0, 0.0, 50.0, 20.0));
    let field_name = f.kit.str("field");
    let field = TNode::new(NodeKind::Input { placeholder: None }, field_set)
        .part(field_name)
        .flags(node_flags::FOCUSABLE);
    let tip = f
        .el("tip", at(300.0, 0.0, 50.0, 20.0))
        .tooltip(TemplateId(1))
        .bind(on(Event::Pointerenter, vec![call("enter", &[])]))
        .bind(on(Event::Pointerleave, vec![call("leave", &[])]));
    let bar = bar.children([b1, b2, b3, md, field, tip]);
    let edge = f
        .el("edge", at(1900.0, 1060.0, 20.0, 20.0))
        .tooltip(TemplateId(1));
    let hk_hud = f
        .el("hk-hud", at(0.0, 100.0, 50.0, 50.0))
        .hotkey("KeyW", 0)
        .bind(on(Event::Click, vec![intent("turn.primary", &[])]));
    let ctrl = f
        .el("ctrl-k", at(0.0, 200.0, 50.0, 50.0))
        .hotkey("KeyK", hotkey_mods::CTRL)
        .bind(on(Event::Click, vec![intent("turn.cancel", &[])]));
    let hud = hud.children([bar, edge, hk_hud, ctrl]);

    let window = f.layer(Layer::Window, "window");
    let screen_rec = with(at(500.0, 100.0, 300.0, 300.0), |r| {
        if !modal {
            r.display = Display::None;
        }
    });
    let mut screen = f.el("screen", screen_rec);
    if modal {
        screen = screen.screen("production");
    }
    let sb = focusable(f.el("sb", at(10.0, 10.0, 50.0, 50.0)))
        .bind(on(Event::Click, vec![intent("turn.cancel", &[])]));
    let sb2 = focusable(f.el("sb2", at(70.0, 10.0, 50.0, 50.0)));
    let hk_win = f
        .el("hk-win", at(130.0, 10.0, 50.0, 50.0))
        .hotkey("KeyW", 0)
        .bind(on(
            Event::Click,
            vec![intent("window.toggle", &[("window", "'research'")])],
        ));
    let window = window.child(screen.children([sb, sb2, hk_win]));
    let tips = f.layer(Layer::Tooltip, "tips");
    f.kit.template("doc", vec![hud, window, tips]);
    let tip_box = f.el("tip-box", at(0.0, 0.0, 80.0, 30.0));
    f.kit.template("tip", vec![tip_box]);
    f.kit.finish().expect("package")
}

/// A host of the keyboard scene with the HUD sample written (`top.gold` 312, `game.me` the key 3:7).
fn kb_host(modal: bool) -> Host {
    let mut h = Host::with(kb_package(modal), Env::default());
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../skins/engraved/sample/hud.json");
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    let mut json: serde_json::Value = serde_json::from_str(&src).expect("sample is JSON");
    json["model"]["game"]["me"] = serde_json::json!("3:7");
    json["model"]["top"]["gold"] = serde_json::json!(312);
    let sample = Sample::parse(Schema::v1(), &json.to_string()).expect("sample");
    sample.apply(&mut h.store).expect("apply");
    let out = h.frame();
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    h
}

fn key_event(key: Key, code: &str, pressed: bool, modifiers: Modifiers) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key,
        code: Some(code.into()),
        pressed,
        repeat: false,
        modifiers,
    })
}

impl Host {
    fn press(&mut self, key: Key, code: &str) -> InputOutcome {
        self.send(key_event(key, code, true, Modifiers::default()))
    }

    fn named(&mut self, k: NamedKey, code: &str) -> InputOutcome {
        self.press(Key::Named(k), code)
    }

    fn letter(&mut self, code: &str) -> InputOutcome {
        let c = code.trim_start_matches("Key").to_lowercase();
        self.press(Key::Character(c), code)
    }

    fn tab(&mut self, back: bool) -> InputOutcome {
        let modifiers = Modifiers {
            shift: back,
            ..Modifiers::default()
        };
        self.send(key_event(Key::Named(NamedKey::Tab), "Tab", true, modifiers))
    }

    /// The template node index of a part (what `Origin::Markup` carries).
    fn tnode(&self, name: &str) -> u32 {
        self.rt.node(self.part(name)).expect("node").tmpl().1
    }

    fn intents(&mut self) -> Vec<WireIntent> {
        let out = self.rt.drain_output();
        assert!(
            out.state_writes.is_empty() && out.script_events.is_empty(),
            "{out:?}"
        );
        out.intents
    }

    fn script_names(&mut self) -> Vec<String> {
        let out = self.rt.drain_output();
        out.script_events
            .iter()
            .map(|s| s.name.to_string())
            .collect()
    }
}

fn wire(name: &str, args: Vec<(&str, MValue)>, origin: Origin) -> WireIntent {
    WireIntent {
        name: name.into(),
        args: args.into_iter().map(|(n, v)| (n.into(), v)).collect(),
        origin,
    }
}

#[test]
fn input_markup_actions_give_intents_state_writes_and_script_events() {
    let mut h = kb_host(false);
    assert!(h.rt.drain_output().is_empty());
    h.click(25.0, 25.0);
    let out = h.rt.drain_output();
    let b1 = Origin::Markup {
        node: h.tnode("b1"),
    };
    let bar = Origin::Markup {
        node: h.tnode("bar"),
    };
    assert_eq!(
        out.intents,
        vec![
            wire(
                "window.open",
                vec![("window", MValue::Str("production".into()))],
                b1
            ),
            // No `stop`: the click bubbled to the bar.
            wire("turn.end", vec![], bar),
        ]
    );
    assert_eq!(
        out.state_writes,
        vec![
            StateOp {
                path: "state.tab".into(),
                value: MValue::Str("units".into()),
            },
            StateOp {
                path: "state.open".into(),
                value: MValue::Bool(true),
            },
        ]
    );
    assert_eq!(out.script_events.len(), 1);
    let ev = &out.script_events[0];
    assert_eq!(ev.ctx, ScriptCtxId::Skin);
    assert_eq!(&*ev.name, "pick");
    assert_eq!(
        ev.arg,
        MValue::Struct(vec![MValue::I32(312), MValue::Str("x".into())])
    );
    assert!(ev.node_path.contains("in part b1"), "{}", ev.node_path);
    assert!(h.rt.drain_output().is_empty(), "drained once");

    // A positional argument of a mod intent is named by its index; a computed one is evaluated.
    h.click(205.0, 25.0);
    let mod_origin = Origin::Markup {
        node: h.tnode("mod"),
    };
    assert_eq!(
        h.intents(),
        vec![
            wire("mods.demo.ping", vec![("0", MValue::I32(313))], mod_origin),
            wire("turn.end", vec![], bar),
        ]
    );
}

#[test]
fn input_stop_ends_bubbling_and_double_click_follows_two_clicks() {
    let mut h = kb_host(false);
    let b2 = Origin::Markup {
        node: h.tnode("b2"),
    };
    let to_city = wire(
        "camera.to_city",
        vec![("city", MValue::Key(MKey::new(3, 7)))],
        b2,
    );
    h.click(85.0, 25.0);
    assert_eq!(h.rt.drain_output().intents, vec![to_city.clone()]);
    h.click(85.0, 25.0);
    let research = wire(
        "research.choose",
        vec![
            ("tech", MValue::Key(MKey::new(3, 7))),
            ("queue", MValue::Bool(true)),
        ],
        b2,
    );
    let out = h.rt.drain_output();
    assert_eq!(out.intents, vec![to_city.clone(), research]);
    // A release over another node clicks their common ancestor: here the bar, outside b2's handlers.
    h.ptr(PointerPhase::Down, 85.0, 25.0);
    h.ptr(PointerPhase::Up, 145.0, 25.0);
    let bar = Origin::Markup {
        node: h.tnode("bar"),
    };
    assert_eq!(
        h.rt.drain_output().intents,
        vec![wire("turn.end", vec![], bar)]
    );
    // A release over the world clicks nothing.
    h.ptr(PointerPhase::Down, 85.0, 25.0);
    h.ptr(PointerPhase::Up, 950.0, 25.0);
    assert!(h.rt.drain_output().intents.is_empty());
}

#[test]
fn input_disabled_nodes_get_no_events_and_no_keys() {
    let mut h = kb_host(false);
    let b3 = h.part("b3");
    h.rt.set_state(b3, StateBits::DISABLED, true);
    h.click(145.0, 25.0);
    assert!(h.rt.drain_output().is_empty());
    assert_eq!(h.letter("KeyD"), PASS, "its hotkey goes to the world");
    assert!(!h.rt.focusable(b3));
    h.rt.set_state(b3, StateBits::DISABLED, false);
    assert_eq!(h.letter("KeyD"), TAKEN);
    assert_eq!(
        h.intents(),
        vec![
            wire("turn.primary", vec![], Origin::Hotkey),
            wire("turn.end", vec![], Origin::Hotkey),
        ]
    );
}

#[test]
fn input_focus_moves_with_presses_tab_and_keys() {
    let mut h = kb_host(false);
    let (b1, b2, b3) = (h.part("b1"), h.part("b2"), h.part("b3"));
    // A press focuses without the keyboard mark.
    h.click(25.0, 25.0);
    h.rt.drain_output();
    assert_eq!(h.rt.focused(), Some(b1));
    assert!(h.state("b1").contains(StateBits::FOCUS));
    assert!(!h.state("b1").contains(StateBits::FOCUS_VISIBLE));
    // Tab: the next focusable node in document order, with the keyboard mark and its `focus` handler.
    assert_eq!(h.tab(false), TAKEN);
    assert_eq!(h.rt.focused(), Some(b2));
    assert!(h.state("b2").contains(StateBits::FOCUS_VISIBLE));
    assert!(!h.state("b1").contains(StateBits::FOCUS));
    assert_eq!(h.script_names(), vec!["f2"]);
    h.tab(false);
    assert_eq!(h.rt.focused(), Some(b3));
    assert_eq!(h.script_names(), vec!["b2"], "blur");
    h.tab(true);
    assert_eq!(h.rt.focused(), Some(b2));
    h.rt.drain_output();
    // Wrapping: b2, b3, the text field, back to b1.
    for _ in 0..3 {
        h.tab(false);
    }
    assert_eq!(h.rt.focused(), Some(b1));
    h.tab(true);
    assert_eq!(h.rt.focused(), Some(h.part("field")));
    h.tab(false);
    h.tab(false);
    h.rt.drain_output();
    // Enter and Space click the focused node.
    assert_eq!(h.rt.focused(), Some(b2));
    assert_eq!(h.named(NamedKey::Enter, "Enter"), TAKEN);
    assert_eq!(h.press(Key::Character(" ".into()), "Space"), TAKEN);
    let names: Vec<String> = h.intents().iter().map(|i| i.name.to_string()).collect();
    assert_eq!(names, vec!["camera.to_city", "camera.to_city"]);
    // A press in the world takes the focus from the UI.
    h.click(950.0, 700.0);
    assert_eq!(h.rt.focused(), None);
    assert_eq!(h.script_names(), vec!["b2"]);
    assert_eq!(h.named(NamedKey::Enter, "Enter"), PASS);
    // A press on a node that cannot take the focus takes it from the UI too.
    h.click(25.0, 25.0);
    h.click(205.0, 25.0);
    assert_eq!(h.rt.focused(), None);
}

#[test]
fn input_text_field_takes_the_keyboard_but_esc_and_tab() {
    let mut h = kb_host(false);
    let field = h.part("field");
    assert_eq!(h.send(InputEvent::Text("a".into())), PASS);
    h.click(250.0, 10.0);
    h.rt.drain_output();
    assert_eq!(h.rt.focused(), Some(field));
    assert_eq!(h.letter("KeyB"), TAKEN, "typed, not a hotkey");
    assert_eq!(
        h.send(key_event(
            Key::Character("b".into()),
            "KeyB",
            false,
            Modifiers::default()
        )),
        TAKEN
    );
    assert_eq!(h.send(InputEvent::Text("b".into())), TAKEN);
    assert_eq!(h.named(NamedKey::Enter, "Enter"), TAKEN);
    assert!(h.rt.drain_output().is_empty());
    // Esc is not the field's (no screen is open: the world's); Tab moves on.
    assert_eq!(h.named(NamedKey::Escape, "Escape"), PASS);
    assert_eq!(h.tab(false), TAKEN);
    assert_eq!(h.rt.focused(), Some(h.part("b1")));
}

#[test]
fn input_hotkeys_follow_layers_and_modifiers() {
    let mut h = kb_host(false);
    assert_eq!(h.letter("KeyW"), TAKEN);
    assert_eq!(
        h.intents(),
        vec![wire("turn.primary", vec![], Origin::Hotkey)]
    );
    // Modifiers must match exactly.
    assert_eq!(h.letter("KeyK"), PASS);
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    assert_eq!(
        h.send(key_event(Key::Character("k".into()), "KeyK", true, ctrl)),
        TAKEN
    );
    assert_eq!(
        h.intents(),
        vec![wire("turn.cancel", vec![], Origin::Hotkey)]
    );
    // A hotkey clicks its node: the click bubbles, every intent carries the hotkey origin.
    assert_eq!(h.letter("KeyB"), TAKEN);
    let out = h.rt.drain_output();
    assert_eq!(
        out.intents
            .iter()
            .map(|i| (i.name.to_string(), i.origin))
            .collect::<Vec<_>>(),
        vec![
            ("window.open".to_string(), Origin::Hotkey),
            ("turn.end".to_string(), Origin::Hotkey)
        ]
    );
    // A held key repeats without acting; its release is the world's; unknown keys are the world's.
    let mut repeat = KeyEvent {
        key: Key::Character("w".into()),
        code: Some("KeyW".into()),
        pressed: true,
        repeat: true,
        modifiers: Modifiers::default(),
    };
    assert_eq!(h.send(InputEvent::Key(repeat.clone())), TAKEN);
    assert!(h.rt.drain_output().is_empty());
    repeat.pressed = false;
    repeat.repeat = false;
    assert_eq!(h.send(InputEvent::Key(repeat)), PASS);
    assert_eq!(h.letter("KeyQ"), PASS);
}

#[test]
fn input_modal_screen_takes_hotkeys_tab_and_esc() {
    let mut h = kb_host(true);
    // W is bound in the HUD and in the screen: the window layer wins.
    assert_eq!(h.letter("KeyW"), TAKEN);
    assert_eq!(
        h.intents(),
        vec![wire(
            "window.toggle",
            vec![("window", MValue::Str("research".into()))],
            Origin::Hotkey
        )]
    );
    // Hotkeys of the HUD are blocked by the screen.
    assert_eq!(h.letter("KeyB"), PASS);
    assert_eq!(h.letter("KeyD"), PASS);
    // Tab stays inside the screen.
    let (sb, sb2) = (h.part("sb"), h.part("sb2"));
    h.tab(false);
    assert_eq!(h.rt.focused(), Some(sb));
    h.tab(false);
    assert_eq!(h.rt.focused(), Some(sb2));
    h.tab(false);
    assert_eq!(h.rt.focused(), Some(sb));
    // Esc closes the top screen.
    assert_eq!(h.named(NamedKey::Escape, "Escape"), TAKEN);
    assert_eq!(
        h.intents(),
        vec![wire(
            "window.close",
            vec![("window", MValue::Str("production".into()))],
            Origin::Hotkey
        )]
    );
    // Without a screen, Esc is the world's.
    let mut plain = kb_host(false);
    assert_eq!(plain.named(NamedKey::Escape, "Escape"), PASS);
}

#[test]
fn input_tooltip_shows_under_its_node_while_hovered() {
    let mut h = kb_host(false);
    assert_eq!(h.rt.tooltip(), None);
    h.ptr(PointerPhase::Move, 310.0, 10.0);
    assert!(h.rt.tooltip().is_some());
    assert_eq!(h.script_names(), vec!["enter"]);
    h.frame();
    let rect = |h: &Host| {
        let r = h.rt.dump_rects(Some("tip-box"));
        assert_eq!(r.len(), 1, "{r:?}");
        (r[0].x, r[0].y, r[0].w, r[0].h)
    };
    // Under the node, left edges aligned, 4 px apart (the simplified placement).
    assert_eq!(rect(&h), (300.0, 24.0, 80.0, 30.0));
    let tip_root = h.rt.tooltip().expect("tooltip");
    // Moving inside the node keeps the tooltip and runs no handler; idle frames stay free.
    h.ptr(PointerPhase::Move, 320.0, 15.0);
    assert_eq!(h.rt.tooltip(), Some(tip_root));
    assert!(h.rt.drain_output().is_empty());
    for _ in 0..10 {
        let s = h.frame().stats;
        assert_eq!((s.layout_runs, s.chunks_rebuilt, s.visited), (0, 0, 0));
    }
    // The tooltip layer takes no input: the point under it is the world's.
    assert_eq!(h.rt.node_at(310.0, 52.0), None);
    // Leaving the node removes it.
    h.ptr(PointerPhase::Move, 950.0, 700.0);
    assert_eq!(h.rt.tooltip(), None);
    assert!(!h.rt.arena().contains(tip_root));
    assert_eq!(h.script_names(), vec!["leave"]);
    h.frame();
    assert!(h.rt.dump_rects(Some("tip-box")).is_empty());

    // Near the corner of the viewport it is pushed back inside (1920 x 1080).
    h.ptr(PointerPhase::Move, 1910.0, 1070.0);
    h.frame();
    assert_eq!(rect(&h), (1840.0, 1050.0, 80.0, 30.0));
}
