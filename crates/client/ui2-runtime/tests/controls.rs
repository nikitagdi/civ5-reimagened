//! Controls (test 8 of the runtime spec, `data-control`): a click and the keyboard on each of button,
//! checkbox, toggle, radio, slider and select give `StateOp`s for the model path (and `input` / `change`
//! events); the model's value marks checked controls; idle frames stay free.

use std::path::PathBuf;
use std::sync::Arc;

use platform::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, NamedKey, PhysicalPosition, PointerEvent,
    PointerId, PointerKind, PointerPhase,
};
use ui2_draw::Scene;
use ui2_model::wire::{Origin, StateOp};
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Sample, Schema, Value as MValue};
use ui2_package::profile::action::{Action, Arg, ControlKind, Event};
use ui2_package::profile::expr::{Expr, parse};
use ui2_package::style::{Display, Len, LenAuto, Position, StyleRecord};
use ui2_package::{BindKind, Layer, NodeKind, Package, StyleSetId, TemplateId, node_flags};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, FrameOutput, InputOutcome, NodeId, Runtime, Services, StateBits};
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

fn block(h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.height = LenAuto::Len(Len::px(h));
    r
}

fn e(src: &str) -> Expr {
    parse(src).unwrap_or_else(|err| panic!("{src}: {err}"))
}

fn call(name: &str) -> Action {
    Action::Call {
        name: name.into(),
        args: Vec::<Arg>::new(),
    }
}

struct Fx {
    kit: Kit,
}

impl Fx {
    fn set(&mut self, r: StyleRecord) -> StyleSetId {
        let id = self.kit.record(r);
        self.kit.plain(id)
    }

    fn el(&mut self, name: &str, r: StyleRecord) -> TNode {
        let set = self.set(r);
        let p = self.kit.str(name);
        TNode::el(set).part(p)
    }

    /// A control: its model path (below `state.`) and behaviour.
    fn control(
        &mut self,
        name: &str,
        r: StyleRecord,
        model: &str,
        kind: ControlKind,
        value: Option<&str>,
        range: Option<[f32; 3]>,
    ) -> TNode {
        let model = self.kit.str(model);
        let value = value.map(|v| self.kit.str(v));
        let options = (kind == ControlKind::Select).then(|| self.kit.expr(e("top.resources")));
        self.el(name, r)
            .flags(node_flags::CONTROL)
            .bind(BindKind::Model(model))
            .bind(BindKind::Control {
                kind,
                value,
                range,
                options,
            })
    }
}

/// The scene (HUD layer 1000 x 800):
///
/// - `chk` checkbox on `state.flag` (10, 10, 40 x 20; `input` and `change` call scripts), `tgl` toggle on
///   `state.flag2` (60, 10), `dis` a disabled checkbox on `state.flag` (110, 10);
/// - radios `r1` `r2` `r3` on `state.tab` (values `cities`, `units`, `3`) at (10 | 60 | 110, 50, 40 x 20);
/// - `vol` slider on `state.vol` (i32, 0 to 100 step 10) at (10, 100, 200 x 20), `zoom` slider on
///   `state.zoom` (f32, 0 to 1 step 0.25) at (10, 130, 100 x 20);
/// - `sel` select on `state.res` over `top.resources` (300, 10, 120 x 80): `head` (20 px), then one 20 px
///   option row per resource (template 1, `opt`);
/// - `btn` a button (500, 10; click: `intent turn.end()`).
fn package() -> Arc<Package> {
    let mut f = Fx { kit: Kit::new() };
    let hud_set = f.set(at(0.0, 0.0, 1000.0, 800.0));
    let hud = TNode::layer(Layer::Hud, hud_set);
    let chk = f
        .control(
            "chk",
            at(10.0, 10.0, 40.0, 20.0),
            "flag",
            ControlKind::Checkbox,
            None,
            None,
        )
        .bind(BindKind::Event {
            ev: Event::Input,
            actions: vec![call("chk-input")],
        })
        .bind(BindKind::Event {
            ev: Event::Change,
            actions: vec![call("chk-change")],
        });
    let tgl = f.control(
        "tgl",
        at(60.0, 10.0, 40.0, 20.0),
        "flag2",
        ControlKind::Toggle,
        None,
        None,
    );
    let yes = f.kit.expr(e("true"));
    let dis = f
        .control(
            "dis",
            at(110.0, 10.0, 40.0, 20.0),
            "flag",
            ControlKind::Checkbox,
            None,
            None,
        )
        .bind(BindKind::Disabled(yes));
    let radio = |f: &mut Fx, name: &str, x: f32, v: &str| {
        f.control(
            name,
            at(x, 50.0, 40.0, 20.0),
            "tab",
            ControlKind::Radio,
            Some(v),
            None,
        )
    };
    let r1 = radio(&mut f, "r1", 10.0, "cities");
    let r2 = radio(&mut f, "r2", 60.0, "units");
    let r3 = radio(&mut f, "r3", 110.0, "3");
    let vol = f
        .control(
            "vol",
            at(10.0, 100.0, 200.0, 20.0),
            "vol",
            ControlKind::Slider,
            None,
            Some([0.0, 100.0, 10.0]),
        )
        .bind(BindKind::Event {
            ev: Event::Input,
            actions: vec![call("vol-input")],
        })
        .bind(BindKind::Event {
            ev: Event::Change,
            actions: vec![call("vol-change")],
        });
    let zoom = f.control(
        "zoom",
        at(10.0, 130.0, 100.0, 20.0),
        "zoom",
        ControlKind::Slider,
        None,
        Some([0.0, 1.0, 0.25]),
    );
    let head = f.el("head", block(20.0));
    let list = f.kit.expr(e("top.resources"));
    let key = f.kit.expr(e("o.id"));
    let item = f.kit.str("o");
    let rows = TNode::new(
        NodeKind::For {
            tpl: TemplateId(1),
            item,
            index: None,
            list,
            key,
            virt: false,
            sort: None,
            sort_desc: None,
            filter: None,
        },
        StyleSetId(0),
    );
    let sel = f
        .control(
            "sel",
            at(300.0, 10.0, 120.0, 80.0),
            "res",
            ControlKind::Select,
            None,
            None,
        )
        .children([head, rows]);
    let btn = f
        .el("btn", at(500.0, 10.0, 40.0, 20.0))
        .flags(node_flags::FOCUSABLE)
        .bind(BindKind::Event {
            ev: Event::Click,
            actions: vec![Action::Intent {
                name: "turn.end".into(),
                args: Vec::new(),
            }],
        });
    let hud = hud.children([chk, tgl, dis, r1, r2, r3, vol, zoom, sel, btn]);
    f.kit.template("doc", vec![hud]);
    let opt = f.el("opt", block(20.0));
    f.kit.template("opt", vec![opt]);
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
    /// The scene with the skin state mounted and the HUD sample written (three resources `0:1`..`0:3`).
    fn new() -> Self {
        let mut store = ModelStore::new(Schema::v1());
        let state = Fragment::parse(
            "[root]\nflag = \"bool\"\nflag2 = \"bool\"\ntab = \"str\"\nvol = \"i32\"\nzoom = \"f32\"\nres = \"str\"\n",
        )
        .expect("fragment");
        store.mount(Namespace::State, &state).expect("mount");
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../skins/engraved/sample/hud.json");
        let src = std::fs::read_to_string(&path).expect("sample");
        Sample::parse(Schema::v1(), &src)
            .expect("sample")
            .apply(&mut store)
            .expect("apply");
        let mut h = Self {
            rt: Runtime::new(package(), Env::default(), Services::default()).expect("runtime"),
            engine: TextEngine::new(),
            store,
            scene: Scene::new(1.0),
            now: 0.0,
        };
        let out = h.frame();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        h
    }

    fn frame(&mut self) -> FrameOutput {
        self.now += 16.0;
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        self.rt
            .frame(self.now, &self.store, &mut self.engine, &mut self.scene)
    }

    /// What the actions module does with the writes: the store takes them; then a frame.
    fn apply(&mut self, ops: &[StateOp]) {
        for op in ops {
            self.store
                .writer(Owner::Skin)
                .set_value(&op.path, op.value.clone())
                .unwrap_or_else(|err| panic!("{op:?}: {err}"));
        }
        let out = self.frame();
        assert!(out.errors.is_empty(), "{:?}", out.errors);
    }

    fn part(&self, name: &str) -> NodeId {
        self.rt
            .find_part(name)
            .unwrap_or_else(|| panic!("part {name}"))
    }

    fn state(&self, name: &str) -> StateBits {
        self.rt.node(self.part(name)).expect("node").state()
    }

    fn send(&mut self, ev: InputEvent) -> InputOutcome {
        self.now += 1.0;
        self.rt.handle_input(&ev, self.now)
    }

    fn ptr(&mut self, phase: PointerPhase, x: f64, y: f64) -> InputOutcome {
        let button =
            matches!(phase, PointerPhase::Down | PointerPhase::Up).then_some(MouseButton::Left);
        self.send(InputEvent::Pointer(PointerEvent {
            id: PointerId::MOUSE,
            kind: PointerKind::Mouse,
            phase,
            position: PhysicalPosition { x, y },
            button,
            modifiers: Modifiers::default(),
        }))
    }

    fn click(&mut self, x: f64, y: f64) {
        self.ptr(PointerPhase::Move, x, y);
        self.ptr(PointerPhase::Down, x, y);
        self.ptr(PointerPhase::Up, x, y);
    }

    fn key(&mut self, k: NamedKey) -> InputOutcome {
        let key = match k {
            NamedKey::Space => Key::Character(" ".into()),
            k => Key::Named(k),
        };
        self.send(InputEvent::Key(KeyEvent {
            key,
            code: Some(format!("{k:?}")),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        }))
    }

    /// The state writes since the last call, as (path, value).
    fn writes(&mut self) -> Vec<(String, MValue)> {
        let out = self.rt.drain_output();
        assert!(out.intents.is_empty(), "{:?}", out.intents);
        out.state_writes
            .into_iter()
            .map(|w| (w.path.to_string(), w.value))
            .collect()
    }

    /// The writes and the names of the script events since the last call.
    fn writes_and_events(&mut self) -> (Vec<(String, MValue)>, Vec<String>) {
        let out = self.rt.drain_output();
        let events = out
            .script_events
            .iter()
            .map(|s| s.name.to_string())
            .collect();
        let writes = out
            .state_writes
            .into_iter()
            .map(|w| (w.path.to_string(), w.value))
            .collect();
        (writes, events)
    }
}

fn w(path: &str, v: MValue) -> (String, MValue) {
    (path.to_string(), v)
}

fn s(v: &str) -> MValue {
    MValue::Str(v.into())
}

#[test]
fn controls_checkbox_and_toggle_flip_their_path_by_click_and_keys() {
    let mut h = Host::new();
    assert!(!h.state("chk").contains(StateBits::CHECKED));
    h.click(20.0, 20.0);
    let (writes, events) = h.writes_and_events();
    assert_eq!(writes, vec![w("state.flag", MValue::Bool(true))]);
    assert_eq!(events, vec!["chk-input", "chk-change"]);
    // A control takes the focus; the model's value marks it checked once applied.
    assert_eq!(h.rt.focused(), Some(h.part("chk")));
    h.apply(&[StateOp {
        path: "state.flag".into(),
        value: MValue::Bool(true),
    }]);
    assert!(h.state("chk").contains(StateBits::CHECKED));
    assert!(
        h.state("dis").contains(StateBits::CHECKED),
        "a disabled control still shows its path's value"
    );
    // Space and Enter click the focused checkbox; before a frame the control reads its own last write.
    assert_eq!(h.key(NamedKey::Space), TAKEN);
    assert_eq!(h.key(NamedKey::Enter), TAKEN);
    let (writes, _) = h.writes_and_events();
    assert_eq!(
        writes,
        vec![
            w("state.flag", MValue::Bool(false)),
            w("state.flag", MValue::Bool(true))
        ]
    );
    h.frame();
    // The toggle: click, then Enter.
    h.click(70.0, 20.0);
    assert_eq!(h.writes(), vec![w("state.flag2", MValue::Bool(true))]);
    h.key(NamedKey::Enter);
    assert_eq!(h.writes(), vec![w("state.flag2", MValue::Bool(false))]);
    // A disabled control gets no click and no focus.
    h.click(120.0, 20.0);
    assert!(h.rt.drain_output().is_empty());
    assert_eq!(h.rt.focused(), None);
}

#[test]
fn controls_radio_writes_its_value_and_arrows_move_through_the_group() {
    let mut h = Host::new();
    h.click(70.0, 60.0);
    assert_eq!(h.writes(), vec![w("state.tab", s("units"))]);
    h.apply(&[StateOp {
        path: "state.tab".into(),
        value: s("units"),
    }]);
    assert!(h.state("r2").contains(StateBits::CHECKED));
    assert!(!h.state("r1").contains(StateBits::CHECKED));
    // Clicking the selected radio writes nothing.
    h.click(70.0, 60.0);
    assert!(h.writes().is_empty());
    // Arrows: the next radio of the group takes the focus and its value; wrapping at the ends.
    assert_eq!(h.key(NamedKey::ArrowRight), TAKEN);
    assert_eq!(h.rt.focused(), Some(h.part("r3")));
    assert_eq!(h.writes(), vec![w("state.tab", s("3"))]);
    h.key(NamedKey::ArrowDown);
    assert_eq!(h.rt.focused(), Some(h.part("r1")));
    h.key(NamedKey::ArrowLeft);
    assert_eq!(h.rt.focused(), Some(h.part("r3")));
    assert_eq!(
        h.writes(),
        vec![w("state.tab", s("cities")), w("state.tab", s("3"))]
    );
    // Space on the focused radio: already selected (its own last write).
    h.key(NamedKey::Space);
    assert!(h.writes().is_empty());
    h.apply(&[StateOp {
        path: "state.tab".into(),
        value: s("3"),
    }]);
    assert!(h.state("r3").contains(StateBits::CHECKED));
    assert!(!h.state("r2").contains(StateBits::CHECKED));
}

#[test]
fn controls_slider_follows_press_drag_and_keys() {
    let mut h = Host::new();
    // A press at 37 % of the box: 37, snapped to the step.
    h.ptr(PointerPhase::Move, 84.0, 110.0);
    assert_eq!(h.ptr(PointerPhase::Down, 84.0, 110.0), TAKEN);
    // Dragged on: 97.5 % rounds to 100; past the end it stays at the maximum (no new write).
    h.ptr(PointerPhase::Move, 205.0, 110.0);
    h.ptr(PointerPhase::Move, 600.0, 300.0);
    h.ptr(PointerPhase::Up, 600.0, 300.0);
    let (writes, events) = h.writes_and_events();
    assert_eq!(
        writes,
        vec![
            w("state.vol", MValue::I32(40)),
            w("state.vol", MValue::I32(100))
        ]
    );
    assert_eq!(events, vec!["vol-input", "vol-input", "vol-change"]);
    h.apply(&[StateOp {
        path: "state.vol".into(),
        value: MValue::I32(100),
    }]);
    // The press focused it: the keys step it.
    assert_eq!(h.rt.focused(), Some(h.part("vol")));
    assert_eq!(h.key(NamedKey::ArrowLeft), TAKEN);
    assert_eq!(h.key(NamedKey::PageDown), TAKEN);
    assert_eq!(h.key(NamedKey::Home), TAKEN, "already at the minimum");
    assert_eq!(h.key(NamedKey::End), TAKEN);
    assert_eq!(h.key(NamedKey::ArrowUp), TAKEN, "already at the maximum");
    assert_eq!(h.key(NamedKey::Enter), TAKEN);
    let (writes, events) = h.writes_and_events();
    assert_eq!(
        writes,
        vec![
            w("state.vol", MValue::I32(90)),
            w("state.vol", MValue::I32(0)),
            w("state.vol", MValue::I32(100)),
        ]
    );
    assert_eq!(events.len(), 6, "input and change per change: {events:?}");
    // A float slider steps by its step.
    h.click(20.0, 140.0);
    h.writes();
    h.frame();
    assert_eq!(h.rt.focused(), Some(h.part("zoom")));
    h.key(NamedKey::End);
    h.key(NamedKey::ArrowLeft);
    assert_eq!(
        h.writes(),
        vec![
            w("state.zoom", MValue::F32(1.0)),
            w("state.zoom", MValue::F32(0.75))
        ]
    );
}

#[test]
fn controls_select_is_its_own_list_of_options() {
    let mut h = Host::new();
    let sel = h.part("sel");
    // A click on the head opens the list: the select takes the focus.
    h.click(350.0, 20.0);
    assert_eq!(h.rt.focused(), Some(sel));
    assert!(h.writes().is_empty());
    // A click on the second option row writes its key and closes the list.
    h.click(350.0, 60.0);
    assert_eq!(h.writes(), vec![w("state.res", s("0:2"))]);
    assert_eq!(h.rt.focused(), None);
    h.apply(&[StateOp {
        path: "state.res".into(),
        value: s("0:2"),
    }]);
    // A second click on the head of the open list closes it.
    h.click(350.0, 20.0);
    assert_eq!(h.rt.focused(), Some(sel));
    h.click(350.0, 20.0);
    assert_eq!(h.rt.focused(), None);
    // The keys: next (no wrapping), first, last; Esc closes.
    h.click(350.0, 20.0);
    assert_eq!(h.key(NamedKey::ArrowDown), TAKEN);
    assert_eq!(h.key(NamedKey::ArrowDown), TAKEN);
    assert_eq!(h.key(NamedKey::Home), TAKEN);
    assert_eq!(h.key(NamedKey::End), TAKEN);
    assert_eq!(h.key(NamedKey::ArrowUp), TAKEN);
    assert_eq!(
        h.writes(),
        vec![
            w("state.res", s("0:3")),
            w("state.res", s("0:1")),
            w("state.res", s("0:3")),
            w("state.res", s("0:2")),
        ]
    );
    assert_eq!(h.key(NamedKey::Escape), TAKEN);
    assert_eq!(h.rt.focused(), None);
    assert_eq!(h.key(NamedKey::Escape), PASS, "no screen, no select");
}

#[test]
fn controls_button_clicks_by_pointer_enter_and_space() {
    let mut h = Host::new();
    h.click(510.0, 20.0);
    let btn = h.part("btn");
    assert_eq!(h.rt.focused(), Some(btn));
    h.key(NamedKey::Enter);
    h.key(NamedKey::Space);
    let out = h.rt.drain_output();
    assert!(out.state_writes.is_empty());
    let node = h.rt.node(btn).expect("node").tmpl().1;
    assert_eq!(
        out.intents
            .iter()
            .map(|i| (i.name.to_string(), i.origin))
            .collect::<Vec<_>>(),
        vec![("turn.end".to_string(), Origin::Markup { node }); 3]
    );
}

#[test]
fn controls_tab_reaches_controls_and_idle_frames_stay_free() {
    let mut h = Host::new();
    // Controls take the focus from Tab in document order (the disabled one is skipped).
    h.key(NamedKey::Tab);
    assert_eq!(h.rt.focused(), Some(h.part("chk")));
    h.key(NamedKey::Tab);
    assert_eq!(h.rt.focused(), Some(h.part("tgl")));
    h.key(NamedKey::Tab);
    assert_eq!(h.rt.focused(), Some(h.part("r1")));
    // Some changes, applied; then nothing happens.
    h.click(20.0, 20.0);
    h.click(84.0, 110.0);
    h.click(350.0, 20.0);
    h.click(350.0, 80.0);
    let ops = h.rt.drain_output().state_writes;
    assert_eq!(ops.len(), 3, "{ops:?}");
    h.apply(&ops);
    let s = h.frame().stats;
    assert_eq!(
        (s.restyled, s.layout_runs),
        (0, 0),
        "checked restyles nothing"
    );
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
