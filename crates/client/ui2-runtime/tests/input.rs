//! Input (test 8 of the runtime spec): hit-testing through layers, stacking, transforms, clips and
//! `pointer-events`; hover, active and the cursor; pointer capture; the wheel over scroll containers; modal
//! screens of the `window` layer; the minimap.

use std::sync::Arc;

use platform::{
    InputEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
    PointerPhase, WheelDelta, WheelEvent,
};
use ui2_draw::Scene;
use ui2_model::{ModelStore, Schema};
use ui2_package::style::{
    ColorV, Cursor, Display, IntAuto, Len, LenAuto, Overflow, PointerEvents, Position, StyleRecord,
    TFn,
};
use ui2_package::{Cond, CondKind, Layer, Package, StyleSetId};
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
