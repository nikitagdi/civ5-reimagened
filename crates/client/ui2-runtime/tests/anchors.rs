//! Map anchors: banners of a `for` in the map layer, each a chunk placed by an
//! anchor slot; placing them moves nothing but the scene's anchor table.

mod support;

use glam::Vec2;
use serde_json::json;
use ui2_draw::{LayerId, Scene};
use ui2_model::{FrameDelta, Key, ModelStore, Sample, Schema, Value};
use ui2_package::profile::action::AnchorKind;
use ui2_package::profile::expr::parse;
use ui2_package::style::{ColorV, Display, Len, LenAuto, Position, StyleRecord, TFn};
use ui2_package::{BindKind, Layer, NodeKind, StyleSetId, TemplateId, node_flags};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{AnchorChange, AnchorId, AnchorScreen, Env, FrameStats, Runtime, Services};
use ui2_text::TextEngine;

const COUNT: u32 = 200;

fn frame_of(store: &mut ModelStore, sample: &serde_json::Value) -> FrameDelta {
    let s = Sample::parse(Schema::v1(), &sample.to_string()).expect("sample");
    s.apply(store).expect("apply");
    store.take_frame().expect("a change")
}

fn resource(id: u32) -> serde_json::Value {
    json!({ "id": format!("0:{id}"), "name": format!("r{id}"), "icon": "icon:R:1", "available": 1, "used": 0, "total": 1 })
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    sample: serde_json::Value,
}

impl Rig {
    fn push(&mut self) -> (FrameStats, Vec<AnchorChange>) {
        let d = frame_of(&mut self.store, &self.sample);
        self.rt.apply_delta(&d, &self.store);
        self.tick()
    }

    fn tick(&mut self) -> (FrameStats, Vec<AnchorChange>) {
        let out = self
            .rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        (out.stats, out.anchor_changes)
    }
}

/// A map layer with one banner per resource (20 x 10 px, centered above its anchor point, with a colored
/// child), and a HUD panel.
fn rig(count: u32) -> (Rig, Vec<AnchorChange>) {
    let mut kit = Kit::new();
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let mut banner = StyleRecord::initial();
    banner.display = Display::Block;
    banner.position = Position::Absolute;
    banner.left = LenAuto::Len(Len::ZERO);
    banner.top = LenAuto::Len(Len::ZERO);
    banner.width = LenAuto::Len(Len::px(20.0));
    banner.height = LenAuto::Len(Len::px(10.0));
    banner.transform = vec![TFn::Translate(Len::pct(-50.0), Len::pct(-100.0))];
    let mut fill = StyleRecord::initial();
    fill.display = Display::Block;
    fill.height = LenAuto::Len(Len::pct(100.0));
    fill.background_color = ColorV::Lit([200, 0, 0, 255]);
    let mut panel = fill.clone();
    panel.height = LenAuto::Len(Len::px(30.0));
    let sets: Vec<StyleSetId> = [layer, banner, fill, panel]
        .into_iter()
        .map(|r| {
            let id = kit.record(r);
            kit.plain(id)
        })
        .collect();
    let list = kit.expr(parse("top.resources").expect("list"));
    let key = kit.expr(parse("r.id").expect("key"));
    let item = kit.str("r");
    let hud = kit.str("hud");
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Map, sets[0]).child(TNode::new(
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
                sets[0],
            )),
            TNode::layer(Layer::Hud, sets[0]).child(TNode::el(sets[3]).part(hud)),
        ],
    );
    let id = kit.expr(parse("r.id").expect("anchor key"));
    kit.template(
        "banner",
        vec![
            TNode::el(sets[1])
                .flags(node_flags::ANCHOR)
                .bind(BindKind::Anchor {
                    kind: AnchorKind::City,
                    id,
                })
                .child(TNode::el(sets[2])),
        ],
    );
    let pkg = kit.finish().expect("package");
    let rt = Runtime::new(pkg, Env::default(), Services::default()).expect("runtime");
    let mut rig = Rig {
        rt,
        engine: TextEngine::new(),
        store: ModelStore::new(Schema::v1()),
        scene: Scene::new(1.0),
        sample: support::sample_json(),
    };
    rig.sample["model"]["top"]["resources"] = (0..count).map(resource).collect();
    let (_, changes) = rig.push();
    (rig, changes)
}

/// Where the test puts anchor `i`: a grid of 20 x 20 px cells.
fn spot(i: usize) -> Vec2 {
    Vec2::new(
        100.0 + (i % 20) as f32 * 30.0,
        100.0 + (i / 20) as f32 * 30.0,
    )
}

fn place(rig: &mut Rig, ids: &[AnchorId], visible: bool) {
    let upd: Vec<AnchorScreen> = ids
        .iter()
        .enumerate()
        .map(|(i, &id)| AnchorScreen {
            id,
            pos: spot(i),
            scale: 1.0,
            visible,
        })
        .collect();
    rig.rt.set_anchor_screen(&upd, &mut rig.scene);
}

#[test]
fn anchor_placing_200_banners_needs_no_layout_and_no_chunk() {
    let (mut rig, changes) = rig(COUNT);
    // One change per new anchor, with its kind and key, in the order of the list.
    assert_eq!(changes.len(), COUNT as usize);
    assert_eq!(rig.rt.anchor_count(), COUNT as usize);
    let ids: Vec<AnchorId> = changes.iter().map(|c| c.0).collect();
    for (i, (_, kind, key)) in changes.iter().enumerate() {
        assert_eq!(*kind, AnchorKind::City);
        assert_eq!(
            *key,
            Value::Key(Key::parse_js(&format!("0:{i}")).expect("key"))
        );
    }
    // Each banner is its own chunk on the map layer, placed by an anchor slot, drawn around its origin.
    let banners: Vec<_> = rig
        .rt
        .arena()
        .ids()
        .filter(|&n| {
            rig.rt
                .node(n)
                .is_some_and(|node| node.chunk().0 == n.index() && node.key().is_some())
        })
        .collect();
    assert_eq!(banners.len(), COUNT as usize);
    let chunk = rig.rt.node(banners[0]).expect("banner").chunk();
    let (layer, items) = rig.rt.chunk_items(chunk).expect("chunk");
    assert_eq!(layer, LayerId::Map);
    assert!(items.anchor.is_some());
    match items.items.as_slice() {
        [ui2_draw::Item::Box(b)] => {
            assert_eq!(b.rect, ui2_draw::Rect::new(0.0, 0.0, 20.0, 10.0));
            let xf = b.transform.expect("the banner's transform");
            assert_eq!(xf.translation, Vec2::new(-10.0, -10.0));
        }
        other => panic!("{other:?}"),
    }
    // Not placed yet: nothing to hit.
    assert_eq!(rig.rt.node_at(100.0, 95.0), None);

    rig.scene.take_table_uploads();
    rig.scene.reset_frame_stats();
    for round in 0..3 {
        // The camera moves every anchor; the frame after does no work.
        let upd: Vec<AnchorScreen> = ids
            .iter()
            .enumerate()
            .map(|(i, &id)| AnchorScreen {
                id,
                pos: spot(i) + Vec2::splat(round as f32),
                scale: 1.0,
                visible: true,
            })
            .collect();
        rig.rt.set_anchor_screen(&upd, &mut rig.scene);
        let (st, changes) = rig.tick();
        assert_eq!(
            (
                st.layout_runs,
                st.chunks_rebuilt,
                st.restyled,
                st.binds_evaluated
            ),
            (0, 0, 0, 0)
        );
        assert!(changes.is_empty());
    }
    let stats = rig.scene.stats();
    assert_eq!(stats.chunks_encoded, 0, "no chunk was encoded again");
    assert_eq!(stats.anchor_bytes, 3 * COUNT * 32, "one record per anchor");
    assert!(!rig.scene.take_table_uploads().is_empty());
}

#[test]
fn anchor_hit_test_follows_the_placement() {
    let (mut rig, changes) = rig(4);
    let ids: Vec<AnchorId> = changes.iter().map(|c| c.0).collect();
    place(&mut rig, &ids, true);
    // Banner 1 spans 120..140 x 90..100 around its point (130, 100).
    let hit = rig.rt.node_at(131.0, 95.0).expect("a banner");
    let banner = rig.rt.node(hit).and_then(|n| n.parent()).expect("parent");
    assert_eq!(
        rig.rt.node(banner).and_then(|n| n.key()),
        Key::parse_js("0:1")
    );
    assert_eq!(rig.rt.node_at(131.0, 101.0), None, "below the banner");
    // A scaled anchor is hit at its scaled size.
    let upd = [AnchorScreen {
        id: ids[0],
        pos: Vec2::new(300.0, 300.0),
        scale: 2.0,
        visible: true,
    }];
    rig.rt.set_anchor_screen(&upd, &mut rig.scene);
    assert!(rig.rt.node_at(282.0, 285.0).is_some());
    assert_eq!(rig.rt.node_at(278.0, 285.0), None);
    // Hidden anchors are not hit.
    place(&mut rig, &ids, false);
    assert_eq!(rig.rt.node_at(131.0, 95.0), None);
}

#[test]
fn anchor_rect_dump_follows_the_placement() {
    let (mut rig, changes) = rig(2);
    let ids: Vec<AnchorId> = changes.iter().map(|c| c.0).collect();
    let boxes = |rig: &Rig| {
        rig.rt
            .dump_rects(None)
            .into_iter()
            .filter(|r| r.node.starts_with("1:"))
            .map(|r| (r.keys, [r.x, r.y, r.w, r.h]))
            .collect::<Vec<_>>()
    };
    // Not placed yet: as laid out around the layer's origin.
    assert_eq!(boxes(&rig)[0].1, [-10.0, -10.0, 20.0, 10.0]);
    // Placed: where the screen shows them (banner 1 around its point (130, 100)).
    place(&mut rig, &ids, true);
    let placed = boxes(&rig);
    assert!(
        placed.contains(&("0:1".to_string(), [120.0, 90.0, 20.0, 10.0])),
        "{placed:?}"
    );
    // Off the screen: not listed.
    place(&mut rig, &ids[..1], false);
    let left = boxes(&rig);
    assert!(left.iter().all(|(k, _)| k == "0:1"), "{left:?}");
    assert!(!left.is_empty());
}

#[test]
fn anchor_removed_items_free_their_anchors() {
    let (mut rig, changes) = rig(3);
    let ids: Vec<AnchorId> = changes.iter().map(|c| c.0).collect();
    place(&mut rig, &ids, true);
    // The middle item goes away: its anchor is reported removed, the others stay.
    let list = rig.sample["model"]["top"]["resources"]
        .as_array()
        .cloned()
        .expect("list");
    rig.sample["model"]["top"]["resources"] = json!([list[0], list[2]]);
    let (_, changes) = rig.push();
    assert_eq!(changes, vec![(ids[1], AnchorKind::City, Value::None)]);
    assert_eq!(rig.rt.anchor_count(), 2);
    // A placement for the removed anchor is ignored; a new item gets a new id.
    place(&mut rig, &ids, true);
    rig.sample["model"]["top"]["resources"] = json!([list[0], list[2], resource(7)]);
    let (_, changes) = rig.push();
    assert_eq!(changes.len(), 1);
    assert!(!ids.contains(&changes[0].0));
    assert_eq!(changes[0].2, Value::Key(Key::parse_js("0:7").expect("key")));
    // Idle frames afterwards report nothing.
    for _ in 0..10 {
        let (st, changes) = rig.tick();
        assert!(changes.is_empty());
        assert_eq!((st.chunks_rebuilt, st.layout_runs), (0, 0));
    }
}
