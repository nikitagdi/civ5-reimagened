//! Node tree from a test-kit package: creation, paths, removal, slot reuse, idle frames, errors.

use std::sync::Arc;

use ui2_package::{Layer, Package, StyleRecord, TemplateId};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Dirty, Env, ErrorCode, NodeClass, Runtime, Services};

/// A HUD layer with a panel three elements deep, text leaves at two levels, and a spare template.
fn package() -> Arc<Package> {
    let mut kit = Kit::new();
    let rec = kit.record(StyleRecord::initial());
    let s = kit.plain(rec);
    let hud = kit.str("hud");
    let top = kit.str("top");
    let gold = kit.str("gold");
    let label = kit.str("Gold");
    let value = kit.str("120");
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, s).part(hud).child(
                TNode::el(s).part(top).children([
                    TNode::el(s)
                        .part(gold)
                        .child(TNode::text(label, s))
                        .child(TNode::el(s).child(TNode::text(value, s))),
                    TNode::el(s),
                ]),
            ),
        ],
    );
    kit.template(
        "extra",
        vec![
            TNode::el(s)
                .child(TNode::text(label, s))
                .child(TNode::el(s)),
        ],
    );
    kit.finish().expect("package")
}

fn runtime() -> Runtime {
    Runtime::new(package(), Env::default(), Services::default()).expect("runtime")
}

#[test]
fn the_document_is_built_with_paths() {
    let rt = runtime();
    assert_eq!(rt.arena().len(), 7);
    let gold = rt.find_part("gold").expect("gold");
    let path = rt.path(gold);
    assert_eq!(path.label, "0:2");
    assert_eq!(path.part.as_deref(), Some("gold"));
    assert_eq!(path.panel.as_deref(), Some("hud"));
    let children = rt.node(gold).expect("gold").children().to_vec();
    assert_eq!(rt.path(children[0]).label, "0:2#0");
    let inner = rt.node(children[1]).expect("inner").children()[0];
    assert_eq!(rt.path(inner).label, "0:3#0");
    assert_eq!(
        rt.node(inner).map(|n| n.class()),
        Some(NodeClass::Text),
        "a text leaf three elements below the layer"
    );
    // Chunks: the layer's child is a chunk root; everything under it paints into it.
    let top = rt.find_part("top").expect("top");
    assert_eq!(
        rt.node(inner).map(|n| n.chunk()),
        rt.node(top).map(|n| n.chunk())
    );
    assert_eq!(rt.node(top).map(|n| n.chunk().0), Some(top.index()));
}

#[test]
fn instances_are_added_removed_and_slots_reused() {
    let mut rt = runtime();
    let top = rt.find_part("top").expect("top");
    let rev = rt.arena().revision();
    let extra = rt.instantiate(TemplateId(1), Some(top));
    assert_eq!(extra.len(), 1);
    assert_eq!(rt.arena().len(), 10);
    assert!(rt.arena().revision() > rev);
    assert_eq!(
        rt.node(top).map(|n| n.children().last().copied()),
        Some(extra.last().copied())
    );
    assert_eq!(rt.path(extra[0]).label, "1:0");
    assert_eq!(rt.path(extra[0]).part.as_deref(), Some("top"));

    assert_eq!(rt.remove(extra[0]), 3);
    assert_eq!(rt.arena().len(), 7);
    assert!(rt.node(extra[0]).is_none());
    let again = rt.instantiate(TemplateId(1), Some(top));
    assert_eq!(rt.arena().capacity(), 10, "freed slots are reused");
    assert!(rt.node(extra[0]).is_none(), "the old handle stays dead");
    assert!(rt.node(again[0]).is_some());

    // Removing the panel removes all of it; the chunk root's removal is reported for the scene.
    rt.take_dirty(Dirty::PAINT);
    assert_eq!(rt.remove(top), 9);
    assert_eq!(rt.arena().len(), 1);
    assert!(
        rt.take_dirty(Dirty::PAINT)
            .iter()
            .all(|&n| rt.node(n).is_some()),
        "only live nodes come out of a queue"
    );
}

#[test]
fn new_nodes_are_queued_for_layout_paint_text_and_bindings() {
    let mut rt = runtime();
    assert_eq!(rt.arena().pending(Dirty::LAYOUT), 7);
    assert_eq!(rt.arena().pending(Dirty::TEXT), 2);
    assert_eq!(rt.arena().pending(Dirty::BIND), 7);
    assert_eq!(rt.take_dirty(Dirty::PAINT).len(), 7);
    let top = rt.find_part("top").expect("top");
    rt.instantiate(TemplateId(1), Some(top));
    assert_eq!(rt.take_dirty(Dirty::PAINT).len(), 3);
}

#[test]
fn idle_frames_touch_nothing() {
    let mut rt = runtime();
    tick(&mut rt);
    let rev = rt.arena().revision();
    let pending: Vec<usize> = [
        Dirty::STYLE,
        Dirty::LAYOUT,
        Dirty::TEXT,
        Dirty::PAINT,
        Dirty::BIND,
    ]
    .iter()
    .map(|&d| rt.arena().pending(d))
    .collect();
    for _ in 0..100 {
        let stats = tick(&mut rt);
        assert_eq!(stats.visited, 0);
        assert_eq!(stats.restyled, 0);
        assert_eq!(stats.layout_runs, 0);
        assert_eq!(stats.chunks_rebuilt, 0);
        assert_eq!(stats.binds_evaluated, 0);
        assert_eq!(stats.shaped_runs, 0);
        assert_eq!(stats.nodes, 7);
    }
    assert_eq!(rt.arena().revision(), rev);
    let after: Vec<usize> = [
        Dirty::STYLE,
        Dirty::LAYOUT,
        Dirty::TEXT,
        Dirty::PAINT,
        Dirty::BIND,
    ]
    .iter()
    .map(|&d| rt.arena().pending(d))
    .collect();
    assert_eq!(after, pending);
}

#[test]
fn a_bad_template_gives_a_placeholder_and_one_error() {
    let mut rt = runtime();
    let top = rt.find_part("top").expect("top");
    for _ in 0..100 {
        let ph = rt.instantiate(TemplateId(42), Some(top));
        assert_eq!(
            rt.node(ph[0]).map(|n| n.class()),
            Some(NodeClass::Placeholder)
        );
        rt.remove(ph[0]);
    }
    let errors: Vec<_> = rt
        .take_errors()
        .into_iter()
        .filter(|e| e.code == ErrorCode::MissingTemplate)
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].code, ErrorCode::MissingTemplate);
    assert_eq!(errors[0].panel.as_deref(), Some("hud"));
    assert!(errors[0].path.contains("in part top"), "{}", errors[0].path);
    // The rest of the tree is untouched.
    assert_eq!(rt.arena().len(), 7);
}

#[test]
fn a_package_without_a_document_is_fatal() {
    let kit = Kit::new();
    let pkg = kit.finish().expect("empty package");
    assert!(Runtime::new(pkg, Env::default(), Services::default()).is_err());
}

fn tick(rt: &mut Runtime) -> ui2_runtime::FrameStats {
    let store = ui2_model::ModelStore::new(ui2_model::Schema::v1());
    rt.update(&store, &mut ui2_text::TextEngine::new())
}
