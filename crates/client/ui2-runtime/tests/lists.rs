//! List and condition bindings: `for` by key (insert, remove, move, reset), `if` that sleeps, sorting and
//! filtering by item fields and `state.*`, virtual lists.

mod support;

use glam::Vec2;
use serde_json::json;
use ui2_draw::Scene;
use ui2_model::{Fragment, FrameDelta, Key, ModelStore, Namespace, Owner, Sample, Schema, Value};
use ui2_package::profile::expr::{Expr, parse};
use ui2_package::style::{Display, Len, LenAuto, Overflow, StyleRecord};
use ui2_package::{ExprId, Layer, Lit, NodeKind, TemplateId};
use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};
use ui2_runtime::{Env, FrameStats, NodeClass, NodeId, Runtime, Services, load_faces};
use ui2_text::TextEngine;

fn env() -> Env {
    Env {
        viewport_px: (800, 600),
        dpr: 1.0,
        ui_scale: 1.0,
        theme: 0,
    }
}

fn fira() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

struct ForSpec {
    item: &'static str,
    list: &'static str,
    virt: bool,
    sort: Option<&'static str>,
    sort_desc: Option<&'static str>,
    filter: Option<&'static str>,
}

fn for_node(kit: &mut Kit, body: u32, f: &ForSpec) -> TNode {
    let mut e =
        |src: &str| -> ExprId { kit.expr(parse(src).unwrap_or_else(|e| panic!("{src}: {e}"))) };
    let list = e(f.list);
    let key = e(&format!("{}.id", f.item));
    let sort = f.sort.map(&mut e);
    let sort_desc = f.sort_desc.map(&mut e);
    let filter = f.filter.map(&mut e);
    let item = kit.str(f.item);
    TNode::new(
        NodeKind::For {
            tpl: TemplateId(body),
            item,
            index: None,
            list,
            key,
            virt: f.virt,
            sort,
            sort_desc,
            filter,
        },
        ui2_package::StyleSetId(0),
    )
}

/// The skin: a plain list, a condition, a sorted and filtered list (template 0); the row body (1) and the
/// badge (2).
fn small_runtime() -> (Runtime, TextEngine, Scene) {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("Fira Sans")];
    rec.font_size = Len::px(14.0);
    let rec = kit.record(rec);
    let set = kit.plain(rec);
    kit.state_default("min", Lit::Int(0));
    kit.state_default("desc", Lit::Bool(false));
    let parts = ["plain", "cond", "sorted"].map(|p| kit.str(p));
    let plain = for_node(
        &mut kit,
        1,
        &ForSpec {
            item: "r",
            list: "top.resources",
            virt: false,
            sort: None,
            sort_desc: None,
            filter: None,
        },
    );
    let sorted = for_node(
        &mut kit,
        1,
        &ForSpec {
            item: "r",
            list: "top.resources",
            virt: false,
            sort: Some("r.total"),
            sort_desc: Some("state.desc"),
            filter: Some("r.available >= state.min"),
        },
    );
    let cond = kit.expr(parse("top.gold > 100").expect("cond"));
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, set).children([
            TNode::el(set).part(parts[0]).child(plain),
            TNode::el(set).part(parts[1]).child(TNode::new(
                NodeKind::If {
                    tpl: TemplateId(2),
                    cond,
                },
                set,
            )),
            TNode::el(set).part(parts[2]).child(sorted),
        ])],
    );
    let name = kit.tpl_of(vec![TplPiece::Expr(path("r.name"))]);
    kit.template(
        "row",
        vec![TNode::el(set).child(TNode::bound_text(name, set))],
    );
    let happy = kit.tpl_of(vec![
        TplPiece::Lit("+".into()),
        TplPiece::Expr(path("top.happiness")),
    ]);
    kit.template(
        "badge",
        vec![TNode::el(set).child(TNode::bound_text(happy, set))],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let rt = Runtime::new(pkg, env(), Services::default()).expect("runtime");
    (rt, engine, Scene::new(1.0))
}

/// A store with the skin's state mounted (`state.min`, `state.desc`).
fn store() -> ModelStore {
    let mut s = ModelStore::new(Schema::v1());
    let f = Fragment::parse("[root]\nmin = \"i32\"\ndesc = \"bool\"\n").expect("fragment");
    s.mount(Namespace::State, &f).expect("mount");
    s
}

fn frame_of(store: &mut ModelStore, sample: &serde_json::Value) -> FrameDelta {
    let s = Sample::parse(Schema::v1(), &sample.to_string()).expect("sample");
    s.apply(store).expect("apply");
    store.take_frame().expect("a change")
}

fn resource(id: u32, name: &str, available: i32, total: i32) -> serde_json::Value {
    json!({ "id": format!("0:{id}"), "name": name, "icon": "icon:R:1", "available": available, "used": 0, "total": total })
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    scene: Scene,
    store: ModelStore,
    sample: serde_json::Value,
}

impl Rig {
    fn new() -> Rig {
        let (rt, engine, scene) = small_runtime();
        let mut rig = Rig {
            rt,
            engine,
            scene,
            store: store(),
            sample: support::sample_json(),
        };
        let first = rig.push();
        assert!(first.binds_evaluated > 0);
        rig
    }

    /// Takes the sample (and state writes) into the store and runs one frame.
    fn push(&mut self) -> FrameStats {
        let d = frame_of(&mut self.store, &self.sample);
        self.rt.apply_delta(&d, &self.store);
        self.tick()
    }

    fn set_state(&mut self, path: &str, v: Value) -> FrameStats {
        self.store
            .writer(Owner::Skin)
            .set_value(path, v)
            .expect("state write");
        let d = self.store.take_frame().expect("a change");
        self.rt.apply_delta(&d, &self.store);
        self.tick()
    }

    fn tick(&mut self) -> FrameStats {
        let out = self
            .rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        out.stats
    }

    fn idle(&mut self) {
        for _ in 0..20 {
            let s = self.tick();
            assert_eq!(
                (
                    s.binds_evaluated,
                    s.restyled,
                    s.shaped_runs,
                    s.layout_runs,
                    s.chunks_rebuilt,
                    s.visited
                ),
                (0, 0, 0, 0, 0, 0)
            );
        }
    }

    /// The element children of a part (the instances of its list or condition) with their texts.
    fn rows(&self, part: &str) -> Vec<(NodeId, String)> {
        let rt = &self.rt;
        let p = rt.find_part(part).expect("part");
        rt.node(p)
            .expect("part node")
            .children()
            .iter()
            .copied()
            .filter(|&c| rt.node(c).is_some_and(|n| n.class() == NodeClass::Element))
            .map(|c| {
                let leaf = rt.node(c).expect("row").children()[0];
                (
                    c,
                    rt.node(leaf)
                        .and_then(|n| n.text())
                        .unwrap_or("")
                        .to_string(),
                )
            })
            .collect()
    }

    fn names(&self, part: &str) -> Vec<String> {
        self.rows(part).into_iter().map(|(_, s)| s).collect()
    }

    fn resources(&mut self) -> &mut Vec<serde_json::Value> {
        self.sample["model"]["top"]["resources"]
            .as_array_mut()
            .expect("resources")
    }
}

#[test]
fn lists_follow_inserts_removes_moves_and_resets_by_key() {
    let mut rig = Rig::new();
    assert_eq!(rig.names("plain"), ["Horses", "Iron", "Coal"]);
    rig.idle();
    let before = rig.rows("plain");

    // Insert: one instance and its one binding; the others stay as they are.
    rig.resources().insert(1, resource(4, "Salt", 1, 1));
    let s = rig.push();
    let now = rig.rows("plain");
    assert_eq!(rig.names("plain"), ["Horses", "Salt", "Iron", "Coal"]);
    assert_eq!(
        (now[0].0, now[2].0, now[3].0),
        (before[0].0, before[1].0, before[2].0)
    );
    // Two list rows (both lists read the list), two new rows' texts, one new sort/filter item.
    assert_eq!(s.binds_evaluated, 2 + 2);
    assert_eq!(s.shaped_runs, 2);
    rig.idle();

    // Remove: the instance and its nodes go.
    let nodes = rig.rt.arena().len();
    rig.resources().remove(2);
    let s = rig.push();
    assert_eq!(rig.names("plain"), ["Horses", "Salt", "Coal"]);
    assert_eq!(rig.rt.arena().len(), nodes - 4, "two rows of two nodes");
    assert_eq!(s.binds_evaluated, 2);
    assert_eq!(s.shaped_runs, 0);

    // Move: instances are reordered, nothing is evaluated but the lists, nothing shaped.
    let ids: Vec<NodeId> = rig.rows("plain").into_iter().map(|(id, _)| id).collect();
    let coal = rig.resources().remove(2);
    rig.resources().insert(0, coal);
    let s = rig.push();
    assert_eq!(rig.names("plain"), ["Coal", "Horses", "Salt"]);
    let moved: Vec<NodeId> = rig.rows("plain").into_iter().map(|(id, _)| id).collect();
    assert_eq!(moved, vec![ids[2], ids[0], ids[1]]);
    assert_eq!((s.binds_evaluated, s.shaped_runs, s.layout_runs), (2, 0, 1));

    // A field of one item: only that item's bindings (one per list showing it).
    rig.resources()[0]["name"] = json!("Charcoal");
    let s = rig.push();
    assert_eq!(s.binds_evaluated, 2);
    assert_eq!(rig.names("plain"), ["Charcoal", "Horses", "Salt"]);

    // Reset (a new skin, a restarted source): reconciled by key, every instance kept.
    let snapshot = rig.store.snapshot();
    rig.rt.apply_delta(&snapshot, &rig.store);
    let s = rig.tick();
    assert_eq!(
        rig.rows("plain")
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        moved
    );
    assert_eq!((s.shaped_runs, s.layout_runs), (0, 0));
    rig.idle();

    // The list emptied, then filled again.
    let all = std::mem::take(rig.resources());
    rig.push();
    assert!(rig.names("plain").is_empty());
    *rig.resources() = all;
    rig.push();
    assert_eq!(rig.names("plain"), ["Charcoal", "Horses", "Salt"]);
    rig.idle();
}

#[test]
fn lists_if_sleeps_while_false_and_wakes_its_stale_rows_only() {
    let mut rig = Rig::new();
    let badge = rig.rows("cond");
    assert_eq!(badge.len(), 1);
    assert_eq!(badge[0].1, "+7");
    rig.idle();

    // False: the body leaves the tree; its binding sleeps.
    rig.sample["model"]["top"]["gold"] = json!(50);
    let s = rig.push();
    assert!(rig.rows("cond").is_empty());
    assert_eq!(s.binds_evaluated, 1, "the condition");
    rig.sample["model"]["top"]["happiness"] = json!(9);
    let s = rig.push();
    assert_eq!((s.binds_evaluated, s.shaped_runs, s.layout_runs), (0, 0, 0));
    rig.idle();

    // True again: the same nodes come back, only the stale binding is evaluated.
    rig.sample["model"]["top"]["gold"] = json!(200);
    let s = rig.push();
    let back = rig.rows("cond");
    assert_eq!(back.len(), 1);
    assert_eq!(
        back[0].0, badge[0].0,
        "the body is put back, not made again"
    );
    assert_eq!(back[0].1, "+9");
    assert_eq!(s.binds_evaluated, 2, "the condition and the stale text");
    rig.idle();
}

#[test]
fn lists_sort_and_filter_by_item_fields_and_state() {
    let mut rig = Rig::new();
    // Ascending by total: Horses 2, Coal 3, Iron 4.
    assert_eq!(rig.names("sorted"), ["Horses", "Coal", "Iron"]);
    let ids: Vec<NodeId> = rig.rows("sorted").into_iter().map(|(id, _)| id).collect();
    rig.idle();

    // One item's key: one item row, one instance moves (binary search), nothing else is made.
    rig.resources()[0]["total"] = json!(5);
    let s = rig.push();
    assert_eq!(rig.names("sorted"), ["Coal", "Iron", "Horses"]);
    let now: Vec<NodeId> = rig.rows("sorted").into_iter().map(|(id, _)| id).collect();
    assert_eq!(now, vec![ids[1], ids[2], ids[0]]);
    assert_eq!(s.binds_evaluated, 1);
    assert_eq!((s.shaped_runs, s.layout_runs), (0, 1));
    // The plain list keeps model order.
    assert_eq!(rig.names("plain"), ["Horses", "Iron", "Coal"]);
    rig.idle();

    // The filter reads `state.min`: Coal (none available) leaves.
    let s = rig.set_state("state.min", Value::I32(1));
    assert_eq!(rig.names("sorted"), ["Iron", "Horses"]);
    assert_eq!(s.binds_evaluated, 3, "the item rows");
    rig.idle();

    // Descending by `state.desc`.
    let s = rig.set_state("state.desc", Value::Bool(true));
    assert_eq!(rig.names("sorted"), ["Horses", "Iron"]);
    assert_eq!(s.binds_evaluated, 1, "the list row");
    let s = rig.set_state("state.min", Value::I32(0));
    assert_eq!(rig.names("sorted"), ["Horses", "Iron", "Coal"]);
    assert_eq!(s.shaped_runs, 1, "Coal's row is made again");
    rig.idle();
}

/// A scroll container 200 px high with a virtual list of 20 px rows (template 1).
fn virtual_runtime() -> (Runtime, TextEngine, Scene) {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira());
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("Fira Sans")];
    rec.font_size = Len::px(14.0);
    let mut scroller = rec.clone();
    scroller.height = LenAuto::Len(Len::px(200.0));
    scroller.overflow_y = Overflow::Scroll;
    let mut row = rec.clone();
    row.height = LenAuto::Len(Len::px(20.0));
    let (rec, scroller, row) = (kit.record(rec), kit.record(scroller), kit.record(row));
    let (set, scroller, row) = (kit.plain(rec), kit.plain(scroller), kit.plain(row));
    let part = kit.str("vlist");
    let list = for_node(
        &mut kit,
        1,
        &ForSpec {
            item: "v",
            list: "top.resources",
            virt: true,
            sort: None,
            sort_desc: None,
            filter: None,
        },
    );
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, set).child(TNode::el(scroller).part(part).child(list))],
    );
    let name = kit.tpl_of(vec![TplPiece::Expr(Expr::Path(vec![
        "v".into(),
        "name".into(),
    ]))]);
    kit.template(
        "row",
        vec![TNode::el(row).child(TNode::bound_text(name, row))],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let rt = Runtime::new(pkg, env(), Services::default()).expect("runtime");
    (rt, engine, Scene::new(1.0))
}

#[test]
fn lists_virtual_keeps_the_visible_rows_and_eight_more() {
    let (rt, engine, scene) = virtual_runtime();
    let mut sample = support::sample_json();
    sample["model"]["top"]["resources"] = (0..1000)
        .map(|i| resource(i + 1, &format!("R{i}"), 1, i as i32))
        .collect();
    let mut rig = Rig {
        rt,
        engine,
        scene,
        store: store(),
        sample,
    };
    rig.push();
    let visible = 10;
    let live = rig.rows("vlist");
    assert!(live.len() <= visible + 8, "{} rows live", live.len());
    assert_eq!(live[0].1, "R0");
    assert_eq!(live.len(), visible + 4, "nothing above the first row");
    // The content keeps the height of all rows.
    let list = rig.rt.find_part("vlist").expect("list");
    rig.idle();

    // Scroll to row 250: the instances are given to the rows in the window, no node is made.
    let nodes = rig.rt.arena().len();
    rig.rt.scroll_by(list, Vec2::new(0.0, 5000.0));
    let s = rig.tick();
    let live = rig.rows("vlist");
    assert_eq!(live.len(), visible + 8);
    assert_eq!(
        rig.rt.arena().len(),
        nodes + 4 * 2,
        "four more rows below, as many above"
    );
    assert_eq!(live[0].1, "R246");
    assert_eq!(live[4].1, "R250");
    let y = rig
        .rt
        .node(live[4].0)
        .expect("row")
        .layout()
        .unrounded
        .location
        .y;
    assert_eq!(y, 5000.0, "row 250 sits at its place in the full list");
    assert!(
        s.binds_evaluated as usize >= visible,
        "the reassigned rows are bound again"
    );
    for (id, _) in &live {
        let key = rig.rt.node(*id).and_then(|n| n.key()).expect("key");
        assert!(key >= Key::new(0, 247) && key <= Key::new(0, 264));
    }
    rig.idle();

    // One row further: one instance goes from the top to the bottom, with its one binding.
    rig.rt.scroll_by(list, Vec2::new(0.0, 20.0));
    let s = rig.tick();
    assert_eq!(rig.rt.arena().len(), nodes + 8);
    assert_eq!(s.binds_evaluated, 1);
    assert_eq!(rig.names("vlist")[0], "R247");
    rig.idle();
}
