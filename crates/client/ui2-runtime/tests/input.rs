//! Input: hit-testing through layers, stacking, transforms, clips and
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
fn input_screen_whose_root_takes_no_pointer_blocks_only_its_panels() {
    let mut f = Fx::new();
    let hud = f.layer(Layer::Hud, "hud");
    let panel = f.el("panel", at(10.0, 10.0, 100.0, 100.0));
    let hud = hud.child(panel);
    let window = f.layer(Layer::Window, "window");
    let screen = f.el(
        "screen",
        with(at(0.0, 0.0, 1000.0, 800.0), |r| {
            r.pointer_events = PointerEvents::None;
        }),
    );
    let side = f.el("side", at(800.0, 0.0, 200.0, 800.0));
    let window = window.child(screen.screen("city").child(side));
    f.kit.template("doc", vec![hud, window]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    assert_eq!(h.rt.visible_screens(), vec![h.part("screen")]);
    // The screen's panel takes the pointer.
    assert_eq!(h.ptr(PointerPhase::Move, 900.0, 300.0), TAKEN);
    assert_eq!(h.rt.hovered(), Some(h.part("side")));
    // Elsewhere the layers below do: the HUD's panel and the world.
    assert_eq!(h.ptr(PointerPhase::Move, 20.0, 20.0), TAKEN);
    assert_eq!(h.rt.hovered(), Some(h.part("panel")));
    assert_eq!(h.click(500.0, 500.0), (PASS, PASS));
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
fn input_tooltip_closes_when_its_owner_becomes_hidden_without_pointer_motion() {
    let mut f = Fx::new();
    let visible = at(0.0, 0.0, 50.0, 20.0);
    let hidden = with(visible.clone(), |r| r.display = Display::None);
    let records = vec![f.kit.record(visible), f.kit.record(hidden)];
    let set = f.kit.set(
        vec![Cond {
            up: 0,
            kind: CondKind::Active,
        }],
        records,
    );
    let owner = TNode::el(set).tooltip(TemplateId(1));
    let hud = f.layer(Layer::Hud, "hud").child(owner);
    let tips = f.layer(Layer::Tooltip, "tips");
    f.kit.template("doc", vec![hud, tips]);
    let tip = f.el("tip-box", at(0.0, 0.0, 80.0, 30.0));
    f.kit.template("tip", vec![tip]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    h.ptr(PointerPhase::Move, 10.0, 10.0);
    h.frame();
    assert!(h.rt.tooltip().is_some());
    h.ptr(PointerPhase::Down, 10.0, 10.0);
    h.frame();
    assert!(h.rt.tooltip().is_none());
    assert!(h.rt.dump_rects(Some("tip-box")).is_empty());
}

#[test]
fn input_tooltip_is_placed_again_when_its_size_changes() {
    let mut f = Fx::new();
    // A node at the bottom of the viewport (1920 x 1080): its tooltip goes above it.
    let owner = f
        .el("owner", at(0.0, 1040.0, 50.0, 20.0))
        .tooltip(TemplateId(1));
    let hud = f.layer(Layer::Hud, "hud").child(owner);
    let tips = f.layer(Layer::Tooltip, "tips");
    f.kit.template("doc", vec![hud, tips]);
    // The tooltip grows (as when a core reply fills it after the hover); `:disabled` stands for that here.
    let (small, tall) = (
        f.kit.record(at(0.0, 0.0, 80.0, 30.0)),
        f.kit.record(at(0.0, 0.0, 80.0, 90.0)),
    );
    let set = f.kit.set(
        vec![Cond {
            up: 0,
            kind: CondKind::Disabled,
        }],
        vec![small, tall],
    );
    let name = f.kit.str("tip-box");
    f.kit.template("tip", vec![TNode::el(set).part(name)]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    h.ptr(PointerPhase::Move, 10.0, 1050.0);
    h.frame();
    let rect = |h: &Host| {
        let r = h.rt.dump_rects(Some("tip-box"));
        assert_eq!(r.len(), 1, "{r:?}");
        (r[0].x, r[0].y, r[0].w, r[0].h)
    };
    assert_eq!(rect(&h), (0.0, 1006.0, 80.0, 30.0));
    let tip = h.rt.tooltip().expect("tooltip");
    h.rt.set_state(tip, StateBits::DISABLED, true);
    h.frame();
    // Its bottom stays 4 px above the node: it is not left hanging over the node.
    assert_eq!(rect(&h), (0.0, 946.0, 80.0, 90.0));
    // Then idle frames stay free.
    for _ in 0..5 {
        let s = h.frame().stats;
        assert_eq!((s.layout_runs, s.chunks_rebuilt, s.visited), (0, 0, 0));
    }
}

#[test]
fn input_hover_follows_a_window_that_opens_and_closes_under_a_still_pointer() {
    // The HUD's `tip` (a tooltip, enter/leave handlers) and, in the window layer above it, `cover`: hidden until it
    // is disabled (`:disabled` stands for a window being opened by the model here).
    let mut f = Fx::new();
    let tip = f
        .el("tip", at(300.0, 0.0, 50.0, 20.0))
        .tooltip(TemplateId(1))
        .bind(on(Event::Pointerenter, vec![call("enter", &[])]))
        .bind(on(Event::Pointerleave, vec![call("leave", &[])]));
    let hud = f.layer(Layer::Hud, "hud").child(tip);
    let shown = at(250.0, 0.0, 200.0, 100.0);
    let hidden = with(shown.clone(), |r| r.display = Display::None);
    let records = vec![f.kit.record(hidden), f.kit.record(shown)];
    let set = f.kit.set(
        vec![Cond {
            up: 0,
            kind: CondKind::Disabled,
        }],
        records,
    );
    let cover_name = f.kit.str("cover");
    let cover = TNode::el(set)
        .part(cover_name)
        .bind(on(Event::Pointerenter, vec![call("cover-enter", &[])]))
        .bind(on(Event::Pointerleave, vec![call("cover-leave", &[])]));
    let window = f.layer(Layer::Window, "window").child(cover);
    let tips = f.layer(Layer::Tooltip, "tips");
    f.kit.template("doc", vec![hud, window, tips]);
    let tip_box = f.el("tip-box", at(0.0, 0.0, 80.0, 30.0));
    f.kit.template("tip", vec![tip_box]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());

    h.ptr(PointerPhase::Move, 310.0, 10.0);
    h.frame();
    assert_eq!(h.rt.hovered(), Some(h.part("tip")));
    assert!(h.rt.tooltip().is_some());
    assert_eq!(h.script_names(), vec!["enter"]);
    // Idle frames with the pointer still: nothing is tested again, nothing is dispatched.
    for _ in 0..5 {
        let s = h.frame().stats;
        assert_eq!((s.layout_runs, s.chunks_rebuilt, s.visited), (0, 0, 0));
    }
    assert!(h.rt.drain_output().is_empty());

    // The window opens under the pointer, which does not move: the frame that lays it out moves the hover to it,
    // the HUD node gets its leave and its tooltip goes.
    let cover = h.part("cover");
    h.rt.set_state(cover, StateBits::DISABLED, true);
    h.frame();
    assert_eq!(h.rt.hovered(), Some(cover));
    assert_eq!(h.rt.tooltip(), None);
    assert_eq!(h.script_names(), vec!["leave", "cover-enter"]);
    h.frame();
    assert!(h.rt.dump_rects(Some("tip-box")).is_empty());
    for _ in 0..5 {
        let s = h.frame().stats;
        assert_eq!((s.layout_runs, s.chunks_rebuilt, s.visited), (0, 0, 0));
    }
    assert!(h.rt.drain_output().is_empty());

    // It closes: the HUD node under the pointer is hovered again and its tooltip comes back.
    h.rt.set_state(cover, StateBits::DISABLED, false);
    h.frame();
    assert_eq!(h.rt.hovered(), Some(h.part("tip")));
    assert!(h.rt.tooltip().is_some());
    assert_eq!(h.script_names(), vec!["cover-leave", "enter"]);

    // A pointer that left the window is not tested again when the geometry changes.
    h.send(pointer(PointerPhase::Left, 310.0, 10.0, None));
    assert_eq!(h.rt.hovered(), None);
    assert_eq!(h.script_names(), vec!["leave"]);
    h.rt.set_state(cover, StateBits::DISABLED, true);
    h.frame();
    h.frame();
    assert_eq!(h.rt.hovered(), None);
    assert!(h.rt.drain_output().is_empty());
}

#[test]
fn input_disabled_node_still_gets_pointer_enter_and_leave_and_its_tooltip() {
    // As Chrome does for a disabled control: the pointer's arrival and departure reach it (a researched technology
    // still names itself for its tooltip); a click does not.
    let mut h = kb_host(false);
    let tip = h.part("tip");
    h.rt.set_state(tip, StateBits::DISABLED, true);
    h.frame();
    h.ptr(PointerPhase::Move, 310.0, 10.0);
    assert!(h.rt.tooltip().is_some());
    assert_eq!(h.script_names(), vec!["enter"]);
    h.click(310.0, 10.0);
    assert!(
        h.rt.drain_output().is_empty(),
        "no click on a disabled node"
    );
    h.ptr(PointerPhase::Move, 950.0, 700.0);
    assert_eq!(h.rt.tooltip(), None);
    assert_eq!(h.script_names(), vec!["leave"]);
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

    // Near the corner of the viewport (1920 x 1080) it does not fit under its node (top 1060): it goes 4 px above
    // the node instead of over it, and is pushed back inside horizontally.
    h.ptr(PointerPhase::Move, 1910.0, 1070.0);
    h.frame();
    assert_eq!(rect(&h), (1840.0, 1026.0, 80.0, 30.0));
}

#[test]
fn click_and_script_events_read_current_modifiers_without_another_frame() {
    let mut f = Fx::new();
    f.kit.state_default("amount", Lit::Int(7));
    let button = f
        .el("button", at(0.0, 0.0, 100.0, 100.0))
        .bind(on(
            Event::Click,
            vec![call(
                "pick",
                &["event.shift ? state.amount : 0", "event.ctrl", "event.alt"],
            )],
        ))
        .bind(on(
            Event::Contextmenu,
            vec![call("right", &["event.shift"])],
        ));
    f.kit.template("doc", vec![button]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    for shift in [false, true, false] {
        for phase in [PointerPhase::Down, PointerPhase::Up] {
            let mut input = pointer(phase, 5.0, 5.0, Some(MouseButton::Left));
            if let InputEvent::Pointer(p) = &mut input {
                p.modifiers = Modifiers {
                    shift,
                    ctrl: true,
                    alt: true,
                    ..Modifiers::default()
                };
            }
            h.send(input);
        }
        let out = h.rt.drain_output();
        let ev = out
            .script_events
            .iter()
            .find(|e| e.name.as_ref() == "pick")
            .expect("script event");
        assert_eq!(
            ev.arg,
            MValue::Struct(vec![
                MValue::I32(if shift { 7 } else { 0 }),
                MValue::Bool(true),
                MValue::Bool(true)
            ])
        );
        assert_eq!(
            (ev.input.shift, ev.input.ctrl, ev.input.alt),
            (shift, true, true)
        );
    }
    for phase in [PointerPhase::Down, PointerPhase::Up] {
        let mut input = pointer(phase, 5.0, 5.0, Some(MouseButton::Right));
        if let InputEvent::Pointer(p) = &mut input {
            p.modifiers.shift = true;
        }
        h.send(input);
    }
    assert_eq!(h.rt.drain_output().script_events[0].arg, MValue::Bool(true));
}

#[test]
fn escape_policy_consumes_without_closing_or_dispatches_its_intent() {
    for actions in [vec![Action::Stop], vec![intent("turn.end", &[])]] {
        let mut f = Fx::new();
        let screen = f
            .el("screen", at(0.0, 0.0, 200.0, 100.0))
            .screen("production")
            .bind(on(Event::Escape, actions.clone()));
        let window = f.layer(Layer::Window, "window").child(screen);
        f.kit.template("doc", vec![window]);
        let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
        assert_eq!(h.named(NamedKey::Escape, "Escape"), TAKEN);
        let out = h.rt.drain_output();
        if actions == vec![Action::Stop] {
            assert!(out.intents.is_empty());
        } else {
            assert_eq!(out.intents[0].name.as_ref(), "turn.end");
        }
    }
}

#[derive(Clone)]
struct DynamicRoot<const UI: bool>(MValue);
impl<const UI: bool> Default for DynamicRoot<UI> {
    fn default() -> Self {
        Self(MValue::Struct(Vec::new()))
    }
}
impl<const UI: bool> ui2_model::Model for DynamicRoot<UI> {
    fn diff(&self, old: &Self, at: &mut ui2_model::Cursor, out: &mut ui2_model::Ops) {
        if self.0 != old.0 {
            out.push(ui2_model::Op::Set {
                field: at.field,
                keys: at.keys,
                value: self.0.clone(),
            });
        }
    }
    fn to_value(&self) -> MValue {
        self.0.clone()
    }
    fn from_value(v: &MValue) -> Result<Self, ui2_model::ValueError> {
        Ok(Self(v.clone()))
    }
}
impl<const UI: bool> ui2_model::Root for DynamicRoot<UI> {
    const NAME: &'static str = if UI { "ui" } else { "top" };
}

#[test]
fn model_hotkeys_rebuild_after_list_reordering_and_report_first_wins() {
    let mut f = Fx::new();
    let key = f.kit.expr(e("r.hotkey"));
    let row = f
        .el("row", at(0.0, 0.0, 30.0, 30.0))
        .bind(BindKind::Hotkey(key))
        .bind(on(Event::Click, vec![call("pick", &["r.id"])]));
    let list = f.kit.expr(e("top.rows"));
    let id = f.kit.expr(e("r.id"));
    let item = f.kit.str("r");
    let list = TNode::new(
        NodeKind::For {
            tpl: TemplateId(1),
            item,
            index: None,
            list,
            key: id,
            virt: false,
            sort: None,
            sort_desc: None,
            filter: None,
        },
        StyleSetId(0),
    );
    let hud = f.layer(Layer::Hud, "hud").child(list);
    f.kit.template("doc", vec![hud]);
    f.kit.template("row", vec![row]);
    let schema = Box::leak(Box::new(Schema::parse("version = \"1.0\"\n[type.Row]\nid = \"i32\"\nhotkey = \"str\"\n[model.top]\nrows = { list = \"Row\", key = \"id\" }\n").expect("schema")));
    let pkg = f.kit.finish().expect("package");
    let mut h = Host {
        rt: Runtime::new(pkg, Env::default(), Services::default()).expect("runtime"),
        engine: TextEngine::new(),
        store: ModelStore::new(schema),
        scene: Scene::new(1.0),
        now: 0.0,
    };
    for (rows, winner, conflict) in [
        (
            r#"[{"id":1,"hotkey":"KeyQ"},{"id":2,"hotkey":"KeyQ"}]"#,
            1,
            true,
        ),
        (
            r#"[{"id":2,"hotkey":"KeyQ"},{"id":1,"hotkey":""}]"#,
            2,
            false,
        ),
        (r#"[{"id":3,"hotkey":"KeyQ"}]"#, 3, false),
    ] {
        let sample = Sample::parse(
            schema,
            &format!("{{\"model\":{{\"top\":{{\"rows\":{rows}}}}}}}"),
        )
        .expect("sample");
        h.store
            .writer(ui2_model::Owner::Source)
            .put(DynamicRoot::<false>(sample.roots()[0].1.clone()))
            .expect("apply");
        let frame = h.frame();
        assert_eq!(
            frame
                .errors
                .iter()
                .any(|e| e.msg.contains("hotkey conflict")),
            conflict,
            "{:?}",
            frame.errors
        );
        assert_eq!(h.letter("KeyQ"), TAKEN);
        assert_eq!(
            h.rt.drain_output().script_events[0].arg,
            MValue::I32(winner)
        );
    }
}

#[test]
fn static_keys_use_optional_action_map_overrides() {
    let mut f = Fx::new();
    let button = f
        .el("button", at(0.0, 0.0, 30.0, 30.0))
        .hotkey("KeyQ", 0)
        .bind(on(Event::Click, vec![call("pick", &[])]));
    f.kit.template("doc", vec![button]);
    let schema = Box::leak(Box::new(
        Schema::parse(
            "version = \"1.0\"\n[type.Keys]\npick = \"str\"\n[model.ui]\nkeys = \"Keys\"\n",
        )
        .expect("schema"),
    ));
    let mut h = Host {
        rt: Runtime::new(
            f.kit.finish().expect("package"),
            Env::default(),
            Services::default(),
        )
        .expect("runtime"),
        engine: TextEngine::new(),
        store: ModelStore::new(schema),
        scene: Scene::new(1.0),
        now: 0.0,
    };
    let sample = Sample::parse(schema, r#"{"ui":{"keys":{"pick":"Ctrl+KeyB"}}}"#).expect("sample");
    h.store
        .writer(ui2_model::Owner::Actions)
        .put(DynamicRoot::<true>(sample.ui().expect("ui").1.clone()))
        .expect("apply");
    h.frame();
    assert_eq!(h.letter("KeyQ"), PASS);
    assert_eq!(
        h.send(key_event(
            Key::Character("b".into()),
            "KeyB",
            true,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            }
        )),
        TAKEN
    );
    assert_eq!(h.rt.drain_output().script_events[0].name.as_ref(), "pick");
}

#[test]
fn wheel_handler_and_scroll_binding_share_state() {
    let mut f = Fx::new();
    f.kit.state_default("techs.x", Lit::Float(40.0));
    f.kit.state_default("techs.y", Lit::Int(0));
    let path = f.kit.str("techs.x");
    let scroller = f
        .el(
            "scroll",
            with(at(0.0, 0.0, 100.0, 100.0), |r| {
                r.overflow_x = Overflow::Auto;
                r.overflow_y = Overflow::Hidden;
            }),
        )
        .bind(BindKind::Scroll { axis: 0, path })
        .bind(on(
            Event::Wheel,
            vec![
                Action::Set {
                    path: "state.techs.x".into(),
                    value: e("state.techs.x + event.dy"),
                },
                call("wheel", &["event.dx", "event.dy"]),
            ],
        ));
    let wide = f.el("wide", at(0.0, 0.0, 500.0, 80.0));
    f.kit.template("doc", vec![scroller.child(wide)]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    let id = h.part("scroll");
    assert_eq!(h.rt.node(id).expect("scroll").scroll(), [40.0, 0.0]);
    h.wheel(50.0, 50.0, -1.0);
    let out = h.rt.drain_output();
    assert_eq!(out.state_writes[0].value, MValue::F32(80.0));
    assert_eq!(
        out.script_events[0].arg,
        MValue::Struct(vec![MValue::F32(0.0), MValue::F32(40.0)])
    );
    h.rt.scroll_by(id, glam::Vec2::new(20.0, 0.0));
    let out = h.rt.drain_output();
    assert_eq!(
        out.state_writes[0],
        StateOp {
            path: "state.techs.x".into(),
            value: MValue::F32(60.0)
        }
    );
}

#[test]
fn unknown_game_picture_draws_the_placeholder_and_reports_once() {
    let mut f = Fx::new();
    let asset = f.kit.asset_ref("unit:unknown");
    let set = f.set(at(0.0, 0.0, 40.0, 40.0));
    let part = f.kit.str("missing");
    f.kit
        .template("doc", vec![TNode::asset(asset, set).part(part)]);
    let mut rt = Runtime::new(
        f.kit.finish().expect("package"),
        Env::default(),
        Services::default(),
    )
    .expect("runtime");
    rt.set_asset_resolver(std::rc::Rc::new(|_| None));
    let mut scene = Scene::new(1.0);
    let mut engine = TextEngine::new();
    let store = ModelStore::new(Schema::v1());
    let out = rt.frame(0.0, &store, &mut engine, &mut scene);
    assert_eq!(out.errors.len(), 1);
    assert!(out.errors[0].msg.contains("unit:unknown"));
    let chunk = rt
        .node(rt.find_part("missing").expect("part"))
        .expect("node")
        .chunk();
    let items = rt.chunk_items(chunk).expect("chunk").1.items;
    assert!(items.iter().any(|item| matches!(item, ui2_draw::Item::Box(b) if b.fill == ui2_draw::Fill::Solid(ui2_draw::Color([170, 60, 50, 72])))));
    assert!(
        rt.frame(1.0, &store, &mut engine, &mut scene)
            .errors
            .is_empty()
    );
}

#[test]
fn explicit_enter_confirmation_precedes_previous_focus_and_ignores_repeats() {
    let mut f = Fx::new();
    let previous = f
        .el("previous", at(0.0, 0.0, 50.0, 40.0))
        .flags(node_flags::FOCUSABLE)
        .bind(on(Event::Click, vec![intent("turn.cancel", &[])]));
    let confirm = f
        .el("confirm", at(60.0, 0.0, 50.0, 40.0))
        .hotkey("Enter", 0)
        .bind(on(Event::Click, vec![intent("turn.end", &[])]));
    let screen = f
        .el("screen", at(0.0, 0.0, 200.0, 100.0))
        .screen("production")
        .children([previous, confirm]);
    let window = f.layer(Layer::Window, "window").child(screen);
    f.kit.template("doc", vec![window]);
    let mut h = Host::with(f.kit.finish().expect("package"), Env::default());
    h.click(20.0, 20.0);
    h.rt.drain_output();
    assert_eq!(h.rt.focused(), Some(h.part("previous")));
    assert_eq!(h.named(NamedKey::Enter, "Enter"), TAKEN);
    assert_eq!(h.intents(), vec![wire("turn.end", vec![], Origin::Hotkey)]);
    let InputEvent::Key(mut repeated) = key_event(
        Key::Named(NamedKey::Enter),
        "Enter",
        true,
        Modifiers::default(),
    ) else {
        unreachable!()
    };
    repeated.repeat = true;
    assert_eq!(h.send(InputEvent::Key(repeated)), TAKEN);
    assert!(h.intents().is_empty());
    h.rt.set_state(h.part("confirm"), StateBits::DISABLED, true);
    h.frame();
    assert_eq!(h.named(NamedKey::Enter, "Enter"), TAKEN);
    assert_eq!(h.intents()[0].name.as_ref(), "turn.cancel");
}

#[test]
fn input_key_maps_are_built_only_when_what_they_read_changed() {
    let mut h = kb_host(false);
    // Idle frames build no key map (the zero-work rule holds for the keys too) and the keys still act.
    for _ in 0..3 {
        let s = h.frame().stats;
        assert!(!s.keys_rebuilt && s.keys_ms == 0.0, "{s:?}");
    }
    assert_eq!(h.letter("KeyW"), TAKEN);
    h.intents();
    // A node that becomes disabled loses its key in the next frame, and gets it back once enabled.
    let w = h.part("hk-hud");
    h.rt.set_state(w, StateBits::DISABLED, true);
    assert!(h.frame().stats.keys_rebuilt);
    assert_eq!(h.letter("KeyW"), PASS);
    assert!(!h.frame().stats.keys_rebuilt, "nothing changed since");
    h.rt.set_state(w, StateBits::DISABLED, false);
    assert!(h.frame().stats.keys_rebuilt);
    assert_eq!(h.letter("KeyW"), TAKEN);
    h.intents();
    // A model change may carry a key override: the maps are built again, once.
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../skins/engraved/sample/hud.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("sample")).expect("json");
    json["model"]["game"]["me"] = serde_json::json!("3:7");
    json["model"]["top"]["gold"] = serde_json::json!(999);
    let sample = Sample::parse(Schema::v1(), &json.to_string()).expect("sample");
    sample.apply(&mut h.store).expect("apply");
    assert!(h.frame().stats.keys_rebuilt);
    assert!(!h.frame().stats.keys_rebuilt);
}

#[test]
fn input_hit_test_skips_what_a_clip_leaves_out() {
    // The rows of a scroll container are hit inside its clip and never outside it; scrolled, the hit follows.
    let mut h = Host::new(false);
    assert_eq!(h.rt.node_at(520.0, 15.0), Some(h.part("mark")));
    assert_eq!(
        h.rt.node_at(520.0, 200.0),
        None,
        "below the clip: the tall content is cut"
    );
    assert_eq!(h.wheel(550.0, 50.0, -3.0), TAKEN);
    h.frame();
    assert_eq!(
        h.rt.node_at(520.0, 15.0),
        Some(h.part("tall")),
        "scrolled: the mark left the view"
    );
    assert_eq!(h.rt.node_at(320.0, 320.0), Some(h.part("spill")));
    assert_eq!(h.rt.node_at(400.0, 320.0), None);
}

#[test]
fn input_events_are_timed_into_the_next_frame() {
    let mut h = Host::new(false);
    let t = std::rc::Rc::new(std::cell::Cell::new(0.0f64));
    let tick = std::rc::Rc::clone(&t);
    h.rt.set_clock(Box::new(move || {
        tick.set(tick.get() + 0.5);
        tick.get()
    }));
    h.ptr(PointerPhase::Move, 45.0, 35.0);
    h.ptr(PointerPhase::Move, 46.0, 35.0);
    let s = h.frame().stats;
    assert_eq!((s.input_events, s.input_ms), (2, 1.0));
    let s = h.frame().stats;
    assert_eq!((s.input_events, s.input_ms), (0, 0.0), "counted once");
}
