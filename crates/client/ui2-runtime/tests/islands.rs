//! Script islands and failed panels: island commands change only their node and its chunk, commands the
//! runtime refuses are reported once, a failed panel becomes a placeholder while its neighbours keep working.

use std::sync::Arc;

use serde_json::json;
use ui2_draw::Scene;
use ui2_model::wire::{IslandCmd, IslandOp, ScriptCtxId};
use ui2_model::{ModelStore, Sample, Schema, Value};
use ui2_package::profile::expr::parse;
use ui2_package::style::{ColorV, Display, Len, StyleRecord};
use ui2_package::{BindKind, Cond, CondKind, Layer};
use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};
use ui2_runtime::{
    Env, ErrorCode, FrameOutput, FrameStats, NodeClass, NodeId, Runtime, Services, load_faces,
};
use ui2_text::TextEngine;

fn fira() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    scene: Scene,
    store: ModelStore,
    now: f64,
}

/// The HUD layer with two panels: `status` holds the island `clock` (a static text `time`, an empty `empty`,
/// a box `box` whose style tests the class `lit`, a node `bound` that markup binds); `other` shows the gold.
fn rig() -> Rig {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("Fira Sans")];
    rec.font_size = Len::px(14.0);
    let mut lit = rec.clone();
    lit.color = ColorV::Lit([250, 200, 40, 255]);
    let (rec, lit) = (kit.record(rec), kit.record(lit));
    let set = kit.plain(rec);
    let class = kit.str("lit");
    let cond = Cond {
        up: 0,
        kind: CondKind::Class(class),
    };
    let toggled = kit.set(vec![cond], vec![rec, lit]);
    let part = |kit: &mut Kit, s: &str| kit.str(s);
    let (status, other, time, empty, boxed, bound) = (
        part(&mut kit, "status"),
        part(&mut kit, "other"),
        part(&mut kit, "time"),
        part(&mut kit, "empty"),
        part(&mut kit, "box"),
        part(&mut kit, "bound"),
    );
    let zero = kit.str("00:00");
    let on = kit.expr(parse("top.gold > 0").expect("expr"));
    let gold = kit.tpl_of(vec![TplPiece::Expr(path("top.gold"))]);
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, set).children([
                TNode::el(set).part(status).child(
                    TNode::el(set).island("clock").children([
                        TNode::el(set)
                            .part(time)
                            .key("time")
                            .child(TNode::text(zero, set)),
                        TNode::el(set).part(empty).key("empty"),
                        TNode::el(toggled)
                            .part(boxed)
                            .key("box")
                            .child(TNode::text(zero, toggled)),
                        TNode::el(toggled)
                            .part(bound)
                            .key("bound")
                            .bind(BindKind::ClassToggle { class, cond: on }),
                    ]),
                ),
                TNode::el(set)
                    .part(other)
                    .child(TNode::bound_text(gold, set)),
            ]),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let env = Env {
        viewport_px: (800, 600),
        ..Env::default()
    };
    let mut r = Rig {
        rt: Runtime::new(pkg, env, Services::default()).expect("runtime"),
        engine,
        scene: Scene::new(1.0),
        store: ModelStore::new(Schema::v1()),
        now: 0.0,
    };
    r.gold(5);
    r.settle();
    r
}

impl Rig {
    fn gold(&mut self, g: i32) {
        let s = Sample::parse(
            Schema::v1(),
            &json!({ "model": { "top": { "gold": g } } }).to_string(),
        )
        .expect("sample");
        s.apply(&mut self.store).expect("apply");
    }

    fn frame(&mut self) -> FrameOutput {
        self.now += 16.0;
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        self.rt
            .frame(self.now, &self.store, &mut self.engine, &mut self.scene)
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.frame();
        }
    }

    fn assert_idle(&mut self, frames: usize) {
        for _ in 0..frames {
            let out = self.frame();
            let s: FrameStats = out.stats;
            assert_eq!(
                (
                    s.layout_runs,
                    s.chunks_rebuilt,
                    s.binds_evaluated,
                    s.shaped_runs,
                    s.restyled,
                    s.visited
                ),
                (0, 0, 0, 0, 0, 0)
            );
            assert!(out.errors.is_empty(), "{:?}", out.errors);
        }
    }

    fn part(&self, name: &str) -> NodeId {
        self.rt
            .find_part(name)
            .unwrap_or_else(|| panic!("part {name}"))
    }

    fn text_of(&self, id: NodeId) -> String {
        let n = self.rt.node(id).expect("node");
        n.children()
            .iter()
            .filter_map(|&c| self.rt.node(c))
            .find(|c| c.class() == NodeClass::Text)
            .and_then(|c| c.text().map(str::to_string))
            .unwrap_or_default()
    }

    fn cmd(&mut self, key: &str, op: IslandOp) -> Vec<ui2_runtime::UiError> {
        self.rt.apply_island(
            &ScriptCtxId::Skin,
            &[IslandCmd {
                island: Arc::from("clock"),
                key: Arc::from(key),
                op,
            }],
        )
    }
}

#[test]
fn island_text_class_style_and_show_touch_one_node() {
    let mut r = rig();
    r.assert_idle(5);

    // Text: one leaf shaped, one chunk.
    let errs = r.cmd("time", IslandOp::Text(Value::Str(Arc::from("12:34"))));
    assert!(errs.is_empty(), "{errs:?}");
    let s = r.frame().stats;
    assert_eq!(
        (s.shaped_runs, s.chunks_rebuilt, s.binds_evaluated),
        (1, 1, 0)
    );
    assert_eq!(r.text_of(r.part("time")), "12:34");
    r.assert_idle(3);

    // Text into an element that had none: it gets a leaf of its own.
    assert!(r.cmd("empty", IslandOp::Text(Value::I32(42))).is_empty());
    let s = r.frame().stats;
    assert_eq!(s.shaped_runs, 1);
    assert_eq!(r.text_of(r.part("empty")), "42");
    r.assert_idle(3);

    // A class: the box and its text leaf (styled by the box's conditions) restyle, nothing is laid out.
    assert!(
        r.cmd("box", IslandOp::Cls(Arc::from("lit"), true))
            .is_empty()
    );
    let s = r.frame().stats;
    assert_eq!((s.restyled, s.layout_runs, s.chunks_rebuilt), (2, 0, 1));
    let boxed = r.part("box");
    assert_eq!(
        r.rt.style(boxed).map(|rec| rec.color),
        Some(ColorV::Lit([250, 200, 40, 255]))
    );
    // A class no condition tests changes nothing.
    assert!(
        r.cmd("box", IslandOp::Cls(Arc::from("nowhere"), true))
            .is_empty()
    );
    r.assert_idle(2);

    // A style property: one layout run.
    assert!(
        r.cmd(
            "box",
            IslandOp::Style(Arc::from("width"), Value::Str(Arc::from("120px")))
        )
        .is_empty()
    );
    let s = r.frame().stats;
    assert_eq!(s.layout_runs, 1);
    let w = r.rt.node(boxed).map(|n| n.layout().unrounded.size.width);
    assert_eq!(w, Some(120.0));
    // camelCase names are taken too; an empty value gives the record's own value back.
    assert!(
        r.cmd(
            "box",
            IslandOp::Style(
                Arc::from("backgroundColor"),
                Value::Str(Arc::from("#ff0000"))
            )
        )
        .is_empty()
    );
    r.frame();
    assert_eq!(
        r.rt.style(boxed).map(|rec| rec.background_color),
        Some(ColorV::Lit([255, 0, 0, 255]))
    );
    assert!(
        r.cmd("box", IslandOp::Style(Arc::from("width"), Value::None))
            .is_empty()
    );
    r.frame();
    assert_eq!(
        r.rt.node(boxed).map(|n| n.layout().unrounded.size.width),
        Some(800.0)
    );

    // Hide and show.
    assert!(r.cmd("box", IslandOp::Show(false)).is_empty());
    r.frame();
    assert_eq!(
        r.rt.style(boxed).map(|rec| rec.display),
        Some(Display::None)
    );
    assert!(r.cmd("box", IslandOp::Show(true)).is_empty());
    r.frame();
    assert_eq!(
        r.rt.style(boxed).map(|rec| rec.display),
        Some(Display::Block)
    );
    r.assert_idle(5);
}

#[test]
fn refused_island_commands_are_reported_once() {
    let mut r = rig();
    let errs = r.cmd("bound", IslandOp::Cls(Arc::from("lit"), false));
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].code, ErrorCode::Island);
    assert!(errs[0].msg.contains("bound"), "{}", errs[0].msg);
    for _ in 0..100 {
        assert!(
            r.cmd("bound", IslandOp::Cls(Arc::from("lit"), false))
                .is_empty()
        );
    }
    // The bound node keeps what markup gave it.
    let bound = r.part("bound");
    assert!(r.rt.node(bound).is_some_and(|n| !n.classes().is_empty()));

    let errs = r.cmd("nokey", IslandOp::Show(false));
    assert_eq!(errs.len(), 1);
    assert!(errs[0].msg.contains("no key"));
    let errs = r.cmd(
        "box",
        IslandOp::Style(Arc::from("display"), Value::Str(Arc::from("none"))),
    );
    assert_eq!(errs.len(), 1);
    let errs = r.cmd(
        "box",
        IslandOp::Style(Arc::from("width"), Value::Str(Arc::from("wide"))),
    );
    assert_eq!(errs.len(), 1);
    assert!(errs[0].msg.contains("not a value"), "{}", errs[0].msg);
    let errs = r.rt.apply_island(
        &ScriptCtxId::Mod(Arc::from("hello")),
        &[IslandCmd {
            island: Arc::from("clock"),
            key: Arc::from("time"),
            op: IslandOp::Show(false),
        }],
    );
    assert_eq!(errs.len(), 1, "a mod does not reach the skin's islands");
    // Refused commands changed nothing and are not handed over again by the frame.
    r.assert_idle(5);
}

#[test]
fn a_failed_panel_shows_a_placeholder_and_its_neighbours_live_on() {
    let mut r = rig();
    let panels = r.rt.panels();
    let names: Vec<&str> = panels.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["status", "other"]);
    assert!(
        panels
            .iter()
            .all(|p| p.owner.is_none() && p.failed.is_none())
    );
    let status = panels[0].clone();
    let height =
        r.rt.node(r.part("status"))
            .map(|n| n.layout().unrounded.size.height)
            .expect("status");

    r.rt.set_panel_failed(status.id, "scripts switched off");
    let out = r.frame();
    assert_eq!(out.errors.len(), 1);
    assert_eq!(out.errors[0].code, ErrorCode::PanelFailed);
    assert_eq!(out.errors[0].panel.as_deref(), Some("status"));
    assert!(
        r.rt.find_part("time").is_none(),
        "the panel's nodes are gone"
    );
    let now = r.rt.panels();
    assert_eq!(now[0].failed.as_deref(), Some("scripts switched off"));
    assert_eq!(now[0].nodes, 1);
    let layer = r.rt.arena().roots()[0];
    let ph = r.rt.node(layer).expect("layer").children()[0];
    let n = r.rt.node(ph).expect("placeholder");
    assert_eq!(n.class(), NodeClass::Placeholder);
    assert_eq!(n.layout().unrounded.size.height, height);
    // Islands of a failed panel are gone: their commands change nothing.
    r.cmd("time", IslandOp::Text(Value::Str(Arc::from("x"))));
    r.assert_idle(3);

    // The neighbour still follows the model.
    r.gold(77);
    let s = r.frame().stats;
    assert_eq!((s.binds_evaluated, s.chunks_rebuilt), (1, 1));
    assert_eq!(r.text_of(r.part("other")), "77");
    // Failing again reports nothing new.
    r.rt.set_panel_failed(status.id, "again");
    r.assert_idle(100);
}
