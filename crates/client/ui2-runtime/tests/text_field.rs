//! Text fields (test 8 of the runtime spec, the text field part): focus asks the host's text bridge for the
//! field; typing, paste, selection and composition come back as the bridge's state and are written to the
//! model path and drawn (text, selection, underline, caret); Enter, Escape and Tab; a model change of the
//! focused field's text; a press that moves the caret; the caret blinks with a chunk rebuild and nothing
//! else, and holds still in a frozen scene; idle frames do nothing.

use std::path::Path;
use std::sync::Arc;

use platform::ui2::{TextInputRequest, TextInputState, TextSel};
use platform::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId,
    PointerKind, PointerPhase,
};
use ui2_draw::{Fill, Item, Scene};
use ui2_model::wire::StateOp;
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Schema, Value as MValue};
use ui2_package::profile::action::{Action, Arg, Event};
use ui2_package::style::{ColorV, Len, LenAuto, Position, StyleRecord};
use ui2_package::{BindKind, Layer, NodeKind, Package, node_flags};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{
    CARET_BLINK_MS, Env, FrameOutput, InputOutcome, NodeId, Runtime, Services, StateBits,
    load_faces,
};
use ui2_text::TextEngine;

const WHITE: [u8; 4] = [250, 250, 250, 255];

fn font() -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

fn at(x: f32, y: f32, w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::px(x));
    r.top = LenAuto::Len(Len::px(y));
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    r
}

fn call(name: &str) -> Action {
    Action::Call {
        name: name.into(),
        args: Vec::<Arg>::new(),
    }
}

/// A HUD with `field` (a text field on `state.text`, 10, 10, 200 x 24 content box and 4 px padding, Fira Sans 16 px,
/// placeholder `chat.hint`; `input`, `change` and `submit` call scripts), `note` (a field without a model,
/// 10, 50) and `btn` (a focusable box, 300, 10).
fn package() -> Arc<Package> {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &font());
    kit.l10n("en_US", "chat.hint", "Say something");
    let family = kit.str("Fira Sans");
    let mut field = at(10.0, 10.0, 200.0, 24.0);
    field.padding_left = Len::px(4.0);
    field.font_family = vec![family];
    field.color = ColorV::Lit(WHITE);
    let mut note = field.clone();
    note.top = LenAuto::Len(Len::px(50.0));
    let mut sets = Vec::new();
    for r in [
        at(0.0, 0.0, 1000.0, 800.0),
        field,
        note,
        at(300.0, 10.0, 40.0, 20.0),
    ] {
        let id = kit.record(r);
        sets.push(kit.plain(id));
    }
    let hint = kit.str("chat.hint");
    let model = kit.str("text");
    let names = ["field", "note", "btn"].map(|n| kit.str(n));
    let input = |set| {
        TNode::new(
            NodeKind::Input {
                placeholder: Some(hint),
            },
            set,
        )
    };
    let field = input(sets[1])
        .part(names[0])
        .flags(node_flags::FOCUSABLE)
        .bind(BindKind::Model(model))
        .bind(BindKind::Event {
            ev: Event::Input,
            actions: vec![call("typed")],
        })
        .bind(BindKind::Event {
            ev: Event::Change,
            actions: vec![call("changed")],
        })
        .bind(BindKind::Event {
            ev: Event::Submit,
            actions: vec![call("send")],
        });
    let note = input(sets[2]).part(names[1]).flags(node_flags::FOCUSABLE);
    let btn = TNode::el(sets[3])
        .part(names[2])
        .flags(node_flags::FOCUSABLE);
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, sets[0]).children([field, note, btn])],
    );
    kit.finish().expect("package")
}

struct Host {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    now: f64,
}

impl Host {
    fn new() -> Self {
        let mut store = ModelStore::new(Schema::v1());
        let state = Fragment::parse("[root]\ntext = \"str\"\n").expect("fragment");
        store.mount(Namespace::State, &state).expect("mount");
        let pkg = package();
        let mut engine = TextEngine::new();
        load_faces(&pkg, &mut engine).expect("faces");
        let mut h = Self {
            rt: Runtime::new(pkg, Env::default(), Services::default()).expect("runtime"),
            engine,
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
        self.frame_at(self.now)
    }

    fn frame_at(&mut self, now: f64) -> FrameOutput {
        self.now = now;
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        let out = self
            .rt
            .frame(now, &self.store, &mut self.engine, &mut self.scene);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        out
    }

    /// What the actions module does with the state writes; then a frame.
    fn apply(&mut self) -> (FrameOutput, Vec<String>) {
        let out = self.rt.drain_output();
        for op in &out.state_writes {
            self.store
                .writer(Owner::Skin)
                .set_value(&op.path, op.value.clone())
                .unwrap_or_else(|err| panic!("{op:?}: {err}"));
        }
        let events = out
            .script_events
            .iter()
            .map(|e| e.name.to_string())
            .collect();
        (self.frame(), events)
    }

    fn part(&self, name: &str) -> NodeId {
        self.rt.find_part(name).expect("part")
    }

    fn items(&mut self, part: &str) -> Vec<Item> {
        let chunk = self.rt.node(self.part(part)).expect("node").chunk();
        self.rt.chunk_items(chunk).expect("chunk").1.items
    }

    /// The items the field itself paints (its chunk is the whole HUD layer's child: only the field's).
    fn field_items(&mut self) -> FieldItems {
        let items = self.items("field");
        let mut f = FieldItems::default();
        for i in items {
            match i {
                Item::Glyphs(g) => {
                    f.glyphs += g.glyphs.len();
                    f.alpha = g.color.0[3];
                    f.first_x = g.glyphs.first().map(|p| p.x);
                }
                Item::Box(b) => match b.fill {
                    Fill::Solid(_) if b.rect.w <= 1.0 => f.caret = Some(b.rect),
                    Fill::Solid(_) if b.rect.h <= 1.0 => f.underline = Some(b.rect.w),
                    Fill::Solid(c) if c.0[3] < 255 => f.selection = Some(b.rect.w),
                    _ => {}
                },
                _ => {}
            }
        }
        f
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

    fn typed(&mut self, value: &str, sel: TextSel) {
        self.rt.set_text_input(TextInputState {
            value: value.into(),
            sel,
            ..TextInputState::default()
        });
    }
}

#[derive(Debug, Default)]
struct FieldItems {
    glyphs: usize,
    alpha: u8,
    first_x: Option<f32>,
    caret: Option<ui2_draw::Rect>,
    selection: Option<f32>,
    underline: Option<f32>,
}

fn focus_of(out: &FrameOutput) -> Option<(String, TextSel, [f32; 4])> {
    match &out.text_input {
        Some(TextInputRequest::Focus { value, sel, rect }) => Some((value.clone(), *sel, *rect)),
        _ => None,
    }
}

#[test]
fn text_field_focus_typing_paste_selection_composition_and_submit() {
    let mut h = Host::new();
    // Unfocused and empty: the placeholder, in the field's color at half opacity.
    let f = h.field_items();
    assert_eq!(f.glyphs, "Say something".len());
    assert_eq!(f.alpha, WHITE[3] / 2);
    assert_eq!(
        f.first_x,
        Some(14.0),
        "the content box starts after 4 px padding"
    );
    assert!(f.caret.is_none());

    // A click focuses the field: the bridge is asked for it, with the empty value and the field's box.
    h.click(50.0, 20.0);
    assert!(
        h.rt.node(h.part("field"))
            .expect("field")
            .state()
            .contains(StateBits::FOCUS)
    );
    let out = h.frame();
    assert_eq!(
        focus_of(&out),
        Some((String::new(), TextSel::caret(0), [10.0, 10.0, 204.0, 24.0]))
    );
    assert!(h.field_items().caret.is_some(), "the caret shows at once");
    // Keys are the field's while it has the focus (the bridge edits).
    let key = InputEvent::Key(KeyEvent {
        key: Key::Character("a".into()),
        code: Some("KeyA".into()),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    });
    assert_eq!(h.send(key), InputOutcome::Consumed);
    assert_eq!(h.send(InputEvent::Text("a".into())), InputOutcome::Consumed);

    // Typing: the value goes to the model path, `input` runs, the text is shaped (no layout).
    h.typed("hi", TextSel::caret(2));
    let writes = h.rt.drain_output();
    assert_eq!(
        writes.state_writes,
        [StateOp {
            path: "state.text".into(),
            value: MValue::Str("hi".into()),
        }]
    );
    assert_eq!(writes.script_events[0].name.as_ref(), "typed");
    // The actions module applies the write: the model has the field's own value, no new request.
    h.store
        .writer(Owner::Skin)
        .set_value("state.text", MValue::Str("hi".into()))
        .expect("write");
    let out = h.frame();
    assert_eq!(out.text_input, None);
    assert_eq!((out.stats.layout_runs, out.stats.chunks_rebuilt), (0, 1));
    let f = h.field_items();
    assert_eq!((f.glyphs, f.alpha), (2, 255));
    let caret = f.caret.expect("caret");
    assert!(caret.x > 14.0 && caret.h > 10.0, "{caret:?}");

    // Paste: one change of several characters.
    h.typed("hi there", TextSel::caret(8));
    let (_, events) = h.apply();
    assert_eq!(events, ["typed"]);
    assert_eq!(h.field_items().glyphs, 8);

    // A selection: no write, a repaint; the caret hides while something is selected.
    h.typed(
        "hi there",
        TextSel {
            anchor: 0,
            caret: 2,
        },
    );
    let out = h.rt.drain_output();
    assert!(out.is_empty(), "{out:?}");
    let st = h.frame().stats;
    assert_eq!(
        (st.layout_runs, st.shaped_runs, st.chunks_rebuilt),
        (0, 0, 1)
    );
    let f = h.field_items();
    assert!(f.selection.is_some_and(|w| w > 5.0), "{f:?}");
    assert!(f.caret.is_none());

    // A composition: the composed text is underlined.
    h.rt.set_text_input(TextInputState {
        value: "hi thereに".into(),
        sel: TextSel::caret(11),
        composing: Some((8, 11)),
        ..TextInputState::default()
    });
    h.apply();
    let f = h.field_items();
    assert!(f.underline.is_some_and(|w| w > 3.0), "{f:?}");

    // Enter: `submit`.
    h.rt.set_text_input(TextInputState {
        value: "hi thereに".into(),
        sel: TextSel::caret(11),
        submitted: true,
        ..TextInputState::default()
    });
    let (_, events) = h.apply();
    assert_eq!(events, ["send"]);

    // The skin clears the field (its own `set`): the bridge is asked to edit the new value.
    h.store
        .writer(Owner::Skin)
        .set_value("state.text", MValue::Str("".into()))
        .expect("write");
    let out = h.frame();
    assert_eq!(
        focus_of(&out).map(|f| (f.0, f.1)),
        Some((String::new(), TextSel::caret(0)))
    );

    // Escape: the field loses the focus, `change` runs (its text changed), the bridge lets go.
    h.typed("bye", TextSel::caret(3));
    h.apply();
    h.rt.set_text_input(TextInputState {
        value: "bye".into(),
        sel: TextSel::caret(3),
        cancelled: true,
        ..TextInputState::default()
    });
    let (out, events) = h.apply();
    assert_eq!(out.text_input, Some(TextInputRequest::Blur));
    assert_eq!(events, ["changed"]);
    assert_eq!(h.rt.focused(), None);
    // Unfocused: the model value, no caret; the bridge's late states are ignored.
    let f = h.field_items();
    assert_eq!((f.glyphs, f.caret), (3, None));
    h.typed("ignored", TextSel::caret(7));
    assert!(h.rt.drain_output().is_empty());
}

#[test]
fn text_field_tab_a_field_without_a_model_and_a_press_that_moves_the_caret() {
    let mut h = Host::new();
    h.click(50.0, 60.0);
    let out = h.frame();
    assert!(focus_of(&out).is_some());
    assert_eq!(h.rt.focused(), Some(h.part("note")));
    // A field without a model keeps its own text: nothing is written.
    h.typed("abcdef", TextSel::caret(6));
    let (_, events) = h.apply();
    assert!(events.is_empty());
    // A press near the start of the text moves the caret there.
    h.click(15.0, 60.0);
    let out = h.frame();
    assert_eq!(focus_of(&out).map(|f| f.1), Some(TextSel::caret(0)));
    // Tab from the bridge moves the focus on (to the button): the bridge lets go of the field.
    h.rt.set_text_input(TextInputState {
        value: "abcdef".into(),
        tab: Some(false),
        ..TextInputState::default()
    });
    let out = h.frame();
    assert_eq!(out.text_input, Some(TextInputRequest::Blur));
    assert_eq!(h.rt.focused(), Some(h.part("btn")));
    // The field shows its own text.
    let items = h.items("note");
    let glyphs: usize = items
        .iter()
        .map(|i| match i {
            Item::Glyphs(g) => g.glyphs.len(),
            _ => 0,
        })
        .sum();
    assert_eq!(glyphs, 6);
}

#[test]
fn text_field_caret_blinks_with_one_chunk_and_holds_still_when_frozen() {
    let mut h = Host::new();
    h.click(50.0, 20.0);
    let t0 = h.now + 16.0;
    h.frame_at(t0);
    let mut rebuilds = Vec::new();
    let mut t = t0;
    while t < t0 + 3.0 * CARET_BLINK_MS {
        t += 10.0;
        let st = h.frame_at(t).stats;
        assert_eq!(
            (st.layout_runs, st.shaped_runs, st.binds_evaluated),
            (0, 0, 0)
        );
        if st.chunks_rebuilt > 0 {
            assert_eq!(st.chunks_rebuilt, 1);
            rebuilds.push(t - t0);
        }
    }
    // Hidden at 530, shown at 1060, hidden at 1590 (within the 10 ms frame step).
    assert_eq!(rebuilds.len(), 3, "{rebuilds:?}");
    for (r, want) in rebuilds.iter().zip([1.0, 2.0, 3.0]) {
        assert!((r - want * CARET_BLINK_MS).abs() < 10.0, "{rebuilds:?}");
    }
    // A frozen scene: the caret holds still whatever the frame clock says.
    h.scene.freeze(Some(5000.0));
    h.frame_at(t + 10.0);
    for i in 0..200 {
        let st = h.frame_at(t + 20.0 + f64::from(i) * 10.0).stats;
        assert_eq!(st.chunks_rebuilt, 0);
    }
}

#[test]
fn text_fields_idle_frames_do_nothing() {
    let mut h = Host::new();
    for _ in 0..100 {
        let out = h.frame();
        let st = out.stats;
        assert_eq!(
            (
                st.visited,
                st.chunks_rebuilt,
                st.layout_runs,
                st.shaped_runs
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(out.text_input, None);
    }
}
