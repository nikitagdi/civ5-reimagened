//! The text bridge in the frame: a click on a text field makes the frame ask the platform's text input to focus
//! it, and what the player types there comes back to the field in the frame that polls it; the bridge goes back
//! to the host before the UI is dropped.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use platform::ui2::{TextInputBridge, TextInputRequest, TextInputState, TextSel};
use platform::{
    InputEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
    PointerPhase,
};
use ui2_draw::Item;
use ui2_host::{Env, V2Frame, V2Init, V2Ui};
use ui2_package::style::{Len, LenAuto, Position, StyleRecord};
use ui2_package::{Layer, NodeKind, node_flags};
use ui2_runtime::testkit::{Kit, TNode};

/// What the fake bridge was asked, and what it reports at the next poll.
#[derive(Default)]
struct Seen {
    requests: Vec<TextInputRequest>,
    next: Option<TextInputState>,
    polls: u32,
}

struct FakeBridge(Rc<RefCell<Seen>>);

impl TextInputBridge for FakeBridge {
    fn request(&mut self, req: &TextInputRequest) {
        self.0.borrow_mut().requests.push(req.clone());
    }

    fn poll(&mut self) -> Option<TextInputState> {
        let mut s = self.0.borrow_mut();
        s.polls += 1;
        s.next.take()
    }
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

/// A HUD with the text field `field` (10, 10, 200 x 24) without a model path.
fn package() -> std::sync::Arc<ui2_package::Package> {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &support::fira());
    let family = kit.str("Fira Sans");
    let mut field = at(10.0, 10.0, 200.0, 24.0);
    field.font_family = vec![family];
    let layer = kit.record(at(0.0, 0.0, 1000.0, 800.0));
    let field = kit.record(field);
    let (layer, field) = (kit.plain(layer), kit.plain(field));
    let name = kit.str("field");
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, layer).child(
                TNode::new(NodeKind::Input { placeholder: None }, field)
                    .part(name)
                    .flags(node_flags::FOCUSABLE),
            ),
        ],
    );
    kit.finish().expect("package")
}

fn frame(ui: &mut V2Ui, now: f64) {
    let r = ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: now,
    });
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

fn click(ui: &mut V2Ui, x: f64, y: f64, now: f64) {
    for phase in [PointerPhase::Move, PointerPhase::Down, PointerPhase::Up] {
        let button = (phase != PointerPhase::Move).then_some(MouseButton::Left);
        ui.handle_input(
            &InputEvent::Pointer(PointerEvent {
                id: PointerId::MOUSE,
                kind: PointerKind::Mouse,
                phase,
                position: PhysicalPosition { x, y },
                button,
                modifiers: Modifiers::default(),
            }),
            now,
        );
    }
}

/// The glyphs the field paints.
fn field_glyphs(ui: &mut V2Ui) -> usize {
    let rt = ui.runtime_mut();
    let node = rt.find_part("field").expect("the field");
    let chunk = rt.node(node).expect("node").chunk();
    rt.chunk_items(chunk)
        .expect("chunk")
        .1
        .items
        .iter()
        .map(|i| match i {
            Item::Glyphs(g) => g.glyphs.len(),
            _ => 0,
        })
        .sum()
}

#[test]
fn the_focused_field_is_asked_of_the_bridge_and_typing_comes_back() {
    let mut ui = V2Ui::new(
        package(),
        V2Init {
            env: Env {
                viewport_px: (1000, 800),
                ..Env::default()
            },
            ..V2Init::default()
        },
    )
    .expect("the skin loads");
    let seen = Rc::new(RefCell::new(Seen::default()));
    assert!(
        ui.set_text_bridge(Box::new(FakeBridge(Rc::clone(&seen))))
            .is_none()
    );
    frame(&mut ui, 16.0);
    assert!(seen.borrow().requests.is_empty(), "no field has the focus");
    assert_eq!(seen.borrow().polls, 1, "polled once per frame");

    click(&mut ui, 50.0, 20.0, 20.0);
    frame(&mut ui, 32.0);
    let focus = seen.borrow().requests.last().cloned();
    assert!(
        matches!(&focus, Some(TextInputRequest::Focus { value, .. }) if value.is_empty()),
        "{focus:?}"
    );
    assert_eq!(field_glyphs(&mut ui), 0);

    // The player types: the next poll reports it, the field shows it in the frame after.
    seen.borrow_mut().next = Some(TextInputState {
        value: "hello".into(),
        sel: TextSel::caret(5),
        ..TextInputState::default()
    });
    frame(&mut ui, 48.0);
    frame(&mut ui, 64.0);
    assert!(field_glyphs(&mut ui) >= 5, "the typed text is drawn");

    // Leaving the screen lets the field go; the bridge goes back to the host.
    ui.release_text_input();
    assert_eq!(seen.borrow().requests.last(), Some(&TextInputRequest::Blur));
    assert!(ui.take_text_bridge().is_some());
    frame(&mut ui, 80.0);
    assert_eq!(seen.borrow().polls, 4, "no bridge, no poll");
}
