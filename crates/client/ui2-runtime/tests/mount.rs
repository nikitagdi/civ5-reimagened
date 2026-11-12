//! Mod fragments in the skin's extension points (and the mount rules): panels
//! append in order, a broken binding is reported once in 100 frames, a failed panel shows a placeholder while
//! its neighbours live on, `Replace` hides the skin's node, a mod's colors and fonts are the skin's, its
//! `state.*` is its own, and idle frames do no work with panels mounted.

use std::sync::Arc;

use serde_json::json;
use ui2_draw::Scene;
use ui2_model::wire::{IslandCmd, IslandOp, ScriptCtxId};
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Sample, Schema, Value};
use ui2_package::profile::expr::parse;
use ui2_package::style::{ColorV, Display, FlexDirection, Len, StyleRecord};
use ui2_package::{BindKind, Cond, CondKind, Layer, Package, TokenId, Tokens};
use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};
use ui2_runtime::{
    Env, ErrorCode, FrameOutput, MountMode, NodeClass, NodeId, PanelId, Runtime, Services,
    load_faces,
};
use ui2_text::TextEngine;

fn fira() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font")
}

const BONE: [u8; 4] = [235, 226, 210, 255];

/// The skin: a HUD layer with the panel `right` (the point `mods`, a column with one text of the skin), the
/// minimap (the replaceable point `hud.minimap`) and the panel `top` (the gold). The sans role is the skin's
/// only face; `--ui-text` is a synonym of `--bone`.
fn skin() -> Arc<Package> {
    let mut kit = Kit::new();
    kit.font("Sans", 400, false, &fira());
    let bone = kit.str("--bone");
    let ui_text = kit.str("--ui-text");
    kit.tokens(Tokens {
        roots: vec![(bone, BONE)],
        aliases: vec![(ui_text, TokenId(0))],
        themes: Vec::new(),
    });
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("Sans")];
    rec.font_size = Len::px(14.0);
    let rec = kit.record(rec);
    let set = kit.plain(rec);
    let mut col = StyleRecord::initial();
    col.display = Display::Flex;
    col.flex_direction = FlexDirection::Column;
    col.font_family = vec![kit.str("Sans")];
    col.font_size = Len::px(14.0);
    let col = kit.record(col);
    let col = kit.plain(col);
    let mut mini = StyleRecord::initial();
    mini.display = Display::Block;
    mini.height = ui2_package::style::LenAuto::Len(Len::px(100.0));
    let mini = kit.record(mini);
    let mini = kit.plain(mini);
    let [right, minimap, top, own] = ["right", "minimap", "top", "own"].map(|p| kit.str(p));
    let skin_text = kit.str("skin");
    let gold = kit.tpl_of(vec![TplPiece::Expr(path("top.gold"))]);
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, set).children([
                TNode::el(set).part(right).child(
                    TNode::el(col)
                        .slot("mods")
                        .child(TNode::el(set).part(own).child(TNode::text(skin_text, set))),
                ),
                TNode::el(mini).part(minimap).slot("hud.minimap"),
                TNode::el(set).part(top).child(TNode::bound_text(gold, set)),
            ]),
        ],
    );
    kit.finish().expect("skin")
}

/// The fragment of mod `hello`: `echo` (the count, a class `on` while `state.open`, a click that sends
/// `mods.hello.say`, an island `say` with key `line`), `broken` (reads a path the model lacks) and `map`
/// (stands in for the minimap). Its text color is `--ui-text`, its font the role `ui-sans`.
fn hello() -> Arc<Package> {
    let mut kit = Kit::new();
    let ui_text = kit.str("--ui-text");
    kit.tokens(Tokens {
        roots: vec![(ui_text, [1, 2, 3, 255])],
        aliases: Vec::new(),
        themes: Vec::new(),
    });
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("ui-sans")];
    rec.font_size = Len::px(12.0);
    rec.color = ColorV::Tok(TokenId(0));
    let mut on = rec.clone();
    on.color = ColorV::Lit([0, 200, 0, 255]);
    let (rec, on) = (kit.record(rec), kit.record(on));
    let set = kit.plain(rec);
    let class = kit.str("m-hello-on");
    let toggled = kit.set(
        vec![Cond {
            up: 0,
            kind: CondKind::Class(class),
        }],
        vec![rec, on],
    );
    let [echo, broken, map, line] = ["echo", "broken", "map", "line"].map(|p| kit.str(p));
    let count = kit.tpl_of(vec![
        TplPiece::Lit("Echoes ".into()),
        TplPiece::Expr(path("mods.hello.count")),
    ]);
    let nope = kit.tpl_of(vec![TplPiece::Expr(path("mods.hello.nope"))]);
    let open = kit.expr(parse("state.open").expect("expr"));
    let dash = kit.str("-");
    let say = BindKind::Event {
        ev: ui2_package::profile::action::Event::Click,
        actions: vec![ui2_package::profile::action::Action::Intent {
            name: "mods.hello.say".into(),
            args: Vec::new(),
        }],
    };
    kit.template(
        "doc",
        vec![
            TNode::el(toggled)
                .part(echo)
                .bind(BindKind::ClassToggle { class, cond: open })
                .bind(say)
                .children([
                    TNode::bound_text(count, toggled),
                    TNode::el(set).island("say").child(
                        TNode::el(set)
                            .part(line)
                            .key("line")
                            .child(TNode::text(dash, set)),
                    ),
                ]),
            TNode::el(set)
                .part(broken)
                .child(TNode::bound_text(nope, set)),
            TNode::el(set).part(map).child(TNode::text(dash, set)),
        ],
    );
    kit.finish_fragment("hello", &["mods", "hud.minimap"])
        .expect("fragment")
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    scene: Scene,
    store: ModelStore,
    now: f64,
}

impl Rig {
    fn new() -> Rig {
        let pkg = skin();
        let mut engine = TextEngine::new();
        load_faces(&pkg, &mut engine).expect("fonts");
        let env = Env {
            viewport_px: (800, 600),
            ..Env::default()
        };
        let mut store = ModelStore::new(Schema::v1());
        let mods = Fragment::parse("[root]\ncount = \"i32\"\n").expect("mods");
        store
            .mount(Namespace::Mod(Arc::from("hello")), &mods)
            .expect("mount mods");
        let state = Fragment::parse("[root]\nopen = \"bool\"\n").expect("modstate");
        store
            .mount(Namespace::ModState(Arc::from("hello")), &state)
            .expect("mount modstate");
        let mut r = Rig {
            rt: Runtime::new(pkg, env, Services::default()).expect("runtime"),
            engine,
            scene: Scene::new(1.0),
            store,
            now: 0.0,
        };
        let s = Sample::parse(
            Schema::v1(),
            &json!({ "model": { "top": { "gold": 5 } } }).to_string(),
        )
        .expect("sample");
        s.apply(&mut r.store).expect("apply");
        r.count(1);
        r
    }

    fn count(&mut self, n: i32) {
        self.store
            .writer(Owner::Mod(Arc::from("hello")))
            .set_value("mods.hello.count", Value::I32(n))
            .expect("count");
    }

    fn frame(&mut self) -> FrameOutput {
        self.now += 16.0;
        if let Some(d) = self.store.take_frame() {
            self.rt.apply_delta(&d, &self.store);
        }
        self.rt
            .frame(self.now, &self.store, &mut self.engine, &mut self.scene)
    }

    fn assert_idle(&mut self, frames: usize) {
        for _ in 0..frames {
            let out = self.frame();
            let s = out.stats;
            assert_eq!(
                (
                    s.layout_runs,
                    s.chunks_rebuilt,
                    s.binds_evaluated,
                    s.shaped_runs,
                    s.visited
                ),
                (0, 0, 0, 0, 0)
            );
            assert!(out.errors.is_empty(), "{:?}", out.errors);
        }
    }

    fn mount(&mut self, panel: &str, slot: &str, mode: MountMode, order: u32) -> PanelId {
        self.rt
            .mount_fragment(hello(), panel, slot, mode, order)
            .unwrap_or_else(|e| panic!("{panel}: {e}"))
    }

    fn root(&self, id: PanelId) -> NodeId {
        self.rt.panel_root(id).expect("panel root").0
    }

    fn text_of(&self, id: NodeId) -> String {
        self.rt
            .node(id)
            .expect("node")
            .children()
            .iter()
            .filter_map(|&c| self.rt.node(c))
            .find(|c| c.class() == NodeClass::Text)
            .and_then(|c| c.text().map(str::to_string))
            .unwrap_or_default()
    }
}

#[test]
fn mounted_panels_bind_follow_order_and_stay_idle() {
    let mut r = Rig::new();
    let echo = r.mount("echo", "mods", MountMode::Append, 2);
    let broken = r.mount("broken", "mods", MountMode::Append, 1);
    // In the point: the skin's own child, then the panels by order.
    let point =
        r.rt.node(r.root(echo))
            .and_then(|n| n.parent())
            .expect("point");
    let own = r.rt.find_part("own").expect("own");
    assert_eq!(
        r.rt.node(point).expect("point").children(),
        &[own, r.root(broken), r.root(echo)]
    );
    // Each panel is a chunk of its own.
    let echo_root = r.root(echo);
    assert_eq!(
        r.rt.node(echo_root).map(|n| n.chunk().0),
        Some(echo_root.index())
    );

    // Test 9: the broken binding is reported once in 100 frames; the neighbour shows its data.
    let mut bind_errors = 0;
    for _ in 0..100 {
        let out = r.frame();
        bind_errors += out
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::Bind)
            .count();
    }
    assert_eq!(bind_errors, 1);
    assert_eq!(r.text_of(echo_root), "Echoes 1");
    // The skin's palette and font roles: `--ui-text` is the skin's `--bone`, `ui-sans` its sans face.
    let rec = r.rt.style(echo_root).expect("record").clone();
    assert_eq!(r.rt.color(rec.color), BONE);
    let sans = r.rt.doc().str_id("Sans").expect("sans");
    assert_eq!(rec.font_family, vec![sans]);
    let laid =
        r.rt.node(echo_root)
            .map(|n| n.layout().unrounded.size.height);
    assert!(laid.is_some_and(|h| h > 10.0), "{laid:?}");

    // Idle with panels mounted.
    r.assert_idle(100);
    // A mod field: one binding, one shaped run, the panel's chunk only.
    r.count(2);
    let s = r.frame().stats;
    assert_eq!(
        (s.binds_evaluated, s.shaped_runs, s.chunks_rebuilt),
        (1, 1, 1)
    );
    assert_eq!(r.text_of(echo_root), "Echoes 2");
    r.assert_idle(3);

    // The fragment's `state.open` is the mod's own state.
    r.store
        .writer(Owner::ModScript(Arc::from("hello")))
        .set_value("modstate.hello.open", Value::Bool(true))
        .expect("open");
    r.frame();
    assert_eq!(
        r.rt.style(echo_root).map(|rec| rec.color),
        Some(ColorV::Lit([0, 200, 0, 255]))
    );
    // Its markup belongs to the mod's script context.
    let tmpl = r.rt.node(echo_root).expect("echo").tmpl();
    assert_eq!(
        r.rt.doc().ctx_of(tmpl.0),
        ScriptCtxId::Mod(Arc::from("hello"))
    );

    // The panels as the console sees them.
    let panels = r.rt.panels();
    let mods: Vec<(&str, Option<&str>)> = panels
        .iter()
        .filter(|p| p.owner.is_some())
        .map(|p| (p.name.as_str(), p.slot.as_deref()))
        .collect();
    assert_eq!(mods, [("echo", Some("mods")), ("broken", Some("mods"))]);
}

#[test]
fn a_failed_mod_panel_is_a_placeholder_and_its_neighbours_live_on() {
    let mut r = Rig::new();
    let echo = r.mount("echo", "mods", MountMode::Append, 1);
    let broken = r.mount("broken", "mods", MountMode::Append, 2);
    for _ in 0..3 {
        r.frame();
    }
    r.rt.set_panel_failed(broken, "scripts of hello are switched off");
    let out = r.frame();
    assert_eq!(
        out.errors
            .iter()
            .filter(|e| e.code == ErrorCode::PanelFailed)
            .count(),
        1
    );
    let ph = r.root(broken);
    assert_eq!(
        r.rt.node(ph).map(|n| n.class()),
        Some(NodeClass::Placeholder)
    );
    assert_eq!(
        r.rt.node(ph).map(|n| n.chunk().0),
        Some(ph.index()),
        "the placeholder keeps the panel's own chunk"
    );
    r.count(9);
    let s = r.frame().stats;
    assert_eq!((s.binds_evaluated, s.chunks_rebuilt), (1, 1));
    assert_eq!(r.text_of(r.root(echo)), "Echoes 9");
    r.assert_idle(100);
}

#[test]
fn replace_hides_the_skins_node_until_unmounted() {
    let mut r = Rig::new();
    r.frame();
    let minimap = r.rt.find_part("minimap").expect("minimap");
    let map = r.mount("map", "hud.minimap", MountMode::Replace, 0);
    r.frame();
    assert_eq!(r.rt.style(minimap).map(|s| s.display), Some(Display::None));
    let root = r.root(map);
    let layer = r.rt.arena().roots()[0];
    let children = r.rt.node(layer).expect("layer").children().to_vec();
    let at = children
        .iter()
        .position(|&c| c == minimap)
        .expect("minimap");
    assert_eq!(children[at + 1], root, "the panel takes the point's place");
    assert_eq!(r.text_of(root), "-");
    r.assert_idle(5);

    assert!(r.rt.unmount(map));
    assert!(!r.rt.unmount(map), "once");
    let out = r.frame();
    assert!(out.stats.chunks_rebuilt >= 1);
    assert!(r.rt.node(root).is_none());
    assert_eq!(r.rt.style(minimap).map(|s| s.display), Some(Display::Block));
    assert!(r.rt.panels().iter().all(|p| p.name != "map"));
    // Mounting the same fragment again reuses its linked tables.
    let templates = r.rt.doc().templates().len();
    r.mount("map", "hud.minimap", MountMode::Replace, 0);
    assert_eq!(r.rt.doc().templates().len(), templates);
    r.frame();
    assert_eq!(r.rt.style(minimap).map(|s| s.display), Some(Display::None));
}

#[test]
fn mod_islands_and_mount_errors() {
    let mut r = Rig::new();
    let echo = r.mount("echo", "mods", MountMode::Append, 0);
    r.frame();
    let cmd = |key: &str| IslandCmd {
        island: Arc::from("say"),
        key: Arc::from(key),
        op: IslandOp::Text(Value::Str(Arc::from("hi"))),
    };
    // The mod reaches its own island, the skin does not.
    assert!(
        r.rt.apply_island(&ScriptCtxId::Mod(Arc::from("hello")), &[cmd("line")])
            .is_empty()
    );
    assert_eq!(
        r.rt.apply_island(&ScriptCtxId::Skin, &[cmd("line")]).len(),
        1
    );
    let s = r.frame().stats;
    assert_eq!((s.shaped_runs, s.chunks_rebuilt), (1, 1));
    let line = r.rt.find_part("line").expect("line");
    assert_eq!(r.text_of(line), "hi");
    assert!(r.rt.panels().iter().any(|p| p.id == echo));

    // Mounts that cannot be done change nothing.
    let nodes = r.rt.arena().len();
    assert!(
        r.rt.mount_fragment(hello(), "echo", "nowhere", MountMode::Append, 0)
            .is_err()
    );
    assert!(
        r.rt.mount_fragment(hello(), "nope", "mods", MountMode::Append, 0)
            .is_err()
    );
    assert!(
        r.rt.mount_fragment(skin(), "echo", "mods", MountMode::Append, 0)
            .is_err()
    );
    assert_eq!(r.rt.arena().len(), nodes);
    r.assert_idle(3);
}

#[test]
fn a_mounted_panel_takes_the_pointer_over_the_skins_node() {
    use platform::{
        InputEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
        PointerPhase,
    };
    let mut r = Rig::new();
    let echo = r.mount("echo", "mods", MountMode::Append, 0);
    r.frame();
    let rect = r.rt.dump_rects(Some("echo"))[0].clone();
    let (x, y) = (f64::from(rect.x + 4.0), f64::from(rect.y + 4.0));
    // The panel is a chunk nested in the skin's chunk of `right`: the hit finds it, not the point under it.
    let hit = r.rt.node_at(x, y).expect("a node");
    let root = r.root(echo);
    let mut at = Some(hit);
    while at.is_some_and(|a| a != root) {
        at = at.and_then(|a| r.rt.node(a)).and_then(|n| n.parent());
    }
    assert_eq!(at, Some(root), "the hit is in the panel");
    for (phase, button) in [
        (PointerPhase::Move, None),
        (PointerPhase::Down, Some(MouseButton::Left)),
        (PointerPhase::Up, Some(MouseButton::Left)),
    ] {
        r.rt.handle_input(
            &InputEvent::Pointer(PointerEvent {
                id: PointerId::MOUSE,
                kind: PointerKind::Mouse,
                phase,
                position: PhysicalPosition { x, y },
                button,
                modifiers: Modifiers::default(),
            }),
            r.now,
        );
    }
    let out = r.rt.drain_output();
    let names: Vec<&str> = out.intents.iter().map(|i| &*i.name).collect();
    assert_eq!(names, ["mods.hello.say"]);
}
