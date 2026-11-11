//! Transitions: a hover changes color, opacity and transform over 150 ms; nothing is laid out, a finished
//! transition costs nothing, and `animating` is true only while one runs.

use ui2_draw::{Color, Fill, Item, Scene};
use ui2_model::{ModelStore, Schema};
use ui2_package::style::{ColorV, Display, Len, LenAuto, StyleRecord, TFn, Transition};
use ui2_package::{Cond, CondKind, Layer, PropId};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, FrameOutput, NodeId, Runtime, Services};
use ui2_text::TextEngine;

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    scene: Scene,
    store: ModelStore,
    button: NodeId,
}

impl Rig {
    fn new() -> Rig {
        let mut kit = Kit::new();
        let linear = [0.0, 0.0, 1.0, 1.0];
        let tr = |prop| Transition {
            prop,
            dur_ms: 150.0,
            delay_ms: 0.0,
            ease: linear,
        };
        let mut base = StyleRecord::initial();
        base.display = Display::Block;
        let mut button = base.clone();
        button.width = LenAuto::Len(Len::px(100.0));
        button.height = LenAuto::Len(Len::px(20.0));
        button.background_color = ColorV::Lit([0, 0, 0, 255]);
        button.transitions = vec![
            tr(PropId::BackgroundColor),
            tr(PropId::Opacity),
            tr(PropId::Transform),
        ];
        let mut hover = button.clone();
        hover.background_color = ColorV::Lit([200, 100, 0, 255]);
        hover.opacity = 0.5;
        hover.transform = vec![TFn::Scale(2.0, 2.0)];
        let (base, button, hover) = (kit.record(base), kit.record(button), kit.record(hover));
        let layer = kit.plain(base);
        let set = kit.set(
            vec![Cond {
                up: 0,
                kind: CondKind::Hover,
            }],
            vec![button, hover],
        );
        let part = kit.str("button");
        kit.template(
            "doc",
            vec![TNode::layer(Layer::Hud, layer).child(TNode::el(set).part(part))],
        );
        let pkg = kit.finish().expect("package");
        let rt = Runtime::new(pkg, Env::default(), Services::default()).expect("runtime");
        let button = rt.find_part("button").expect("button");
        let mut rig = Rig {
            rt,
            engine: TextEngine::new(),
            scene: Scene::new(1.0),
            store: ModelStore::new(Schema::v1()),
            button,
        };
        let first = rig.frame(0.0);
        assert!(!first.animating, "nothing transitions on the first records");
        rig
    }

    fn frame(&mut self, now: f64) -> FrameOutput {
        self.rt
            .frame(now, &self.store, &mut self.engine, &mut self.scene)
    }

    /// The fill, its alpha folded in, and the horizontal scale of the button's box.
    fn shown(&mut self) -> (Color, f32) {
        let chunk = self.rt.node(self.button).expect("button").chunk();
        let (_, items) = self.rt.chunk_items(chunk).expect("chunk");
        items
            .items
            .iter()
            .find_map(|i| match i {
                Item::Box(b) => match b.fill {
                    Fill::Solid(c) => Some((c, b.transform.map_or(1.0, |t| t.matrix2.x_axis.x))),
                    _ => None,
                },
                _ => None,
            })
            .expect("the button's box")
    }
}

/// A button whose class binding holds from the start (a unit action that is not available, say) takes its
/// class record in its first frame without a transition: an element's first style is not a change. Before,
/// the record selected at creation counted as the old one, the transition ran from it, and with a clock that
/// stands still (a frozen or paused frame clock) the node was painted again on every idle frame.
#[test]
fn a_class_bound_in_the_first_frame_starts_no_transition() {
    use ui2_package::BindKind;
    use ui2_package::profile::expr::parse;

    let mut kit = Kit::new();
    let mut base = StyleRecord::initial();
    base.display = Display::Block;
    let mut button = base.clone();
    button.width = LenAuto::Len(Len::px(100.0));
    button.height = LenAuto::Len(Len::px(20.0));
    button.background_color = ColorV::Lit([20, 18, 16, 255]);
    button.transitions = vec![Transition {
        prop: PropId::BackgroundColor,
        dur_ms: 150.0,
        delay_ms: 0.0,
        ease: [0.0, 0.0, 1.0, 1.0],
    }];
    let mut off = button.clone();
    off.background_color = ColorV::Lit([0, 0, 0, 0]);
    let (base, button, off) = (kit.record(base), kit.record(button), kit.record(off));
    let layer = kit.plain(base);
    let class = kit.str("off");
    let set = kit.set(
        vec![Cond {
            up: 0,
            kind: CondKind::Class(class),
        }],
        vec![button, off],
    );
    let cond = kit.expr(parse("1 > 0").expect("cond"));
    let part = kit.str("button");
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, layer).child(
                TNode::el(set)
                    .part(part)
                    .bind(BindKind::ClassToggle { class, cond }),
            ),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut rt = Runtime::new(pkg, Env::default(), Services::default()).expect("runtime");
    let (mut engine, mut scene) = (TextEngine::new(), Scene::new(1.0));
    let store = ModelStore::new(Schema::v1());
    let first = rt.frame(0.0, &store, &mut engine, &mut scene);
    assert!(
        first.stats.restyled > 0,
        "the class is applied in the first frame"
    );
    assert!(!first.animating, "a first record does not transition");
    let button = rt.find_part("button").expect("button");
    let chunk = rt.node(button).expect("button").chunk();
    let (_, items) = rt.chunk_items(chunk).expect("chunk");
    assert!(
        !items.items.iter().any(
            |i| matches!(i, Item::Box(b) if matches!(b.fill, Fill::Solid(Color([20, 18, 16, 255]))))
        ),
        "the button shows its class record at once"
    );
    // The clock stands still: idle frames do nothing.
    for _ in 0..10 {
        let out = rt.frame(0.0, &store, &mut engine, &mut scene);
        let s = out.stats;
        assert!(!out.animating);
        assert_eq!(
            (s.restyled, s.layout_runs, s.chunks_rebuilt, s.visited),
            (0, 0, 0, 0)
        );
    }
}

/// A frozen scene gives the transitions their time: a transition half way when the scene is frozen holds
/// there whatever the frame clock says, costs nothing after the frame that froze it and does not ask for
/// frames; thawed, it runs on the frame clock again. A transition that starts in a frozen scene starts at
/// the frozen time.
#[test]
fn transitions_take_the_time_of_a_frozen_scene() {
    let mut rig = Rig::new();
    let button = rig.button;
    rig.rt.set_hover(Some(button));
    rig.frame(1000.0);
    rig.scene.freeze(Some(1075.0));
    let out = rig.frame(1010.0);
    assert!(!out.animating, "nothing moves in a frozen scene");
    let mid = (Color([100, 50, 0, 191]), 1.5);
    let (c, s) = rig.shown();
    assert_eq!(c, mid.0);
    assert!((s - mid.1).abs() < 1e-4, "{s}");
    for t in [2000.0, 9000.0, 1075.0] {
        let out = rig.frame(t);
        let st = out.stats;
        assert!(!out.animating);
        assert_eq!((st.chunks_rebuilt, st.visited), (0, 0), "at {t}");
        assert_eq!(rig.shown().0, mid.0);
    }
    // Thawed: the frame clock again (the transition ended long ago).
    rig.scene.freeze(None);
    let out = rig.frame(9000.0);
    assert!(!out.animating);
    assert_eq!(rig.shown(), (Color([200, 100, 0, 128]), 2.0));

    // Leaving while frozen: the way back starts at the frozen time and holds its first values.
    rig.scene.freeze(Some(20_000.0));
    rig.rt.set_hover(None);
    let out = rig.frame(9100.0);
    assert!(!out.animating);
    assert_eq!(rig.shown(), (Color([200, 100, 0, 128]), 2.0));
    rig.scene.freeze(Some(20_075.0));
    rig.frame(9200.0);
    let (c, s) = rig.shown();
    assert_eq!(c, mid.0, "half way back at the new frozen time");
    assert!((s - 1.5).abs() < 1e-4, "{s}");
}

#[test]
fn transitions_run_150_ms_without_layout_and_then_cost_nothing() {
    let mut rig = Rig::new();
    assert_eq!(rig.shown(), (Color([0, 0, 0, 255]), 1.0));
    let button = rig.button;
    rig.rt.set_hover(Some(button));

    let mut timeline = Vec::new();
    for t in [1000.0, 1050.0, 1075.0, 1100.0, 1150.0] {
        let out = rig.frame(t);
        assert_eq!(out.stats.layout_runs, 0, "a transition never lays out");
        assert_eq!(out.stats.shaped_runs, 0);
        assert_eq!(out.stats.chunks_rebuilt, 1);
        timeline.push((t, out.animating, rig.shown()));
    }
    // Start: the old values; half way: half way (opacity folds into the alpha); end: the new record.
    assert_eq!(timeline[0].2, (Color([0, 0, 0, 255]), 1.0));
    assert!(timeline[0].1);
    let (mid_color, mid_scale) = timeline[2].2;
    assert_eq!(mid_color, Color([100, 50, 0, 191]));
    assert!((mid_scale - 1.5).abs() < 1e-4, "{mid_scale}");
    assert!(timeline[3].1, "still running at 100 ms");
    assert!(!timeline[4].1, "done at 150 ms");
    assert_eq!(timeline[4].2, (Color([200, 100, 0, 128]), 2.0));

    // Finished: frames do nothing at all.
    for t in [1200.0, 1300.0, 5000.0] {
        let out = rig.frame(t);
        let s = out.stats;
        assert!(!out.animating);
        assert_eq!(
            (s.restyled, s.layout_runs, s.chunks_rebuilt, s.visited),
            (0, 0, 0, 0)
        );
    }

    // Leaving half way back, then hovering again: the new transition starts where the shown value is.
    rig.rt.set_hover(None);
    rig.frame(6000.0);
    rig.frame(6075.0);
    let back_mid = rig.shown();
    rig.rt.set_hover(Some(button));
    let out = rig.frame(6075.0);
    assert!(out.animating);
    assert_eq!(
        rig.shown(),
        back_mid,
        "no jump when the transition turns around"
    );
    let out = rig.frame(6225.0);
    assert!(!out.animating);
    assert_eq!(rig.shown(), (Color([200, 100, 0, 128]), 2.0));
}
