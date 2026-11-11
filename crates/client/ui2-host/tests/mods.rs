//! The mods' panels in UI v2: a panel goes to its point in load order, else to `mods` (or nowhere with
//! `fallback = none`); a mod for another UI API shows one placeholder line in `mods`; a panel that replaces an
//! element of the skin hides it and gives it back when the mod fails. The index of the fragments and its checks.

use std::sync::Arc;

use ui2_host::mods::{ModIndexEntry, ModPanel, ModUi, parse_mods_index};
use ui2_host::{V2Frame, V2FrameReport, V2Init, V2Ui};
use ui2_package::style::{Display, Len, LenAuto, StyleRecord};
use ui2_package::{
    Kind, Layer, Manifest, Node, NodeKind, Package, PackageBuilder, StyleSet, Tag, Template,
};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_script::testing::MockScriptHost;

/// A block 20 px high.
fn row(kit: &mut Kit) -> ui2_package::StyleSetId {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.height = LenAuto::Len(Len::px(20.0));
    let r = kit.record(r);
    kit.plain(r)
}

/// A HUD with the points `hud.right` (with a child of its own), `hud.minimap` (an element panels replace) and
/// `mods`.
fn skin() -> Arc<Package> {
    let mut kit = Kit::new();
    let block = {
        let mut r = StyleRecord::initial();
        r.display = Display::Block;
        let r = kit.record(r);
        kit.plain(r)
    };
    let set = row(&mut kit);
    let (right, own, map, mods) = (
        kit.str("right"),
        kit.str("own"),
        kit.str("minimap"),
        kit.str("mods-col"),
    );
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, block).children([
                TNode::el(block)
                    .part(right)
                    .slot("hud.right")
                    .child(TNode::el(set).part(own)),
                TNode::el(set).part(map).slot("hud.minimap"),
                TNode::el(block).part(mods).slot("mods"),
            ]),
        ],
    );
    kit.finish().expect("skin")
}

/// A fragment of mod `id` with one 20 px panel per name, the script API `api` and the given script files.
fn fragment(id: &str, panels: &[&str], api: (u16, u16), scripts: &[(&str, &str)]) -> Arc<Package> {
    let mut b = PackageBuilder::new();
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.height = LenAuto::Len(Len::px(20.0));
    let record = b.style_record(r);
    let style = b.style_set(StyleSet::plain(vec![record]));
    let nodes = panels
        .iter()
        .map(|p| Node {
            kind: NodeKind::Box { tag: Tag::Div },
            subtree: 1,
            style,
            part: Some(b.str(p)),
            flags: 0,
        })
        .collect();
    let name = b.str("doc");
    b.template(Template { name, nodes });
    for (path, code) in scripts {
        b.script(path, code.as_bytes());
    }
    let mut m: Manifest = Kit::manifest();
    m.kind = Kind::Fragment {
        slots: vec!["mods".into()],
    };
    m.id = id.into();
    m.api = api;
    let bytes = b.finish(m).expect("fragment");
    Arc::new(Package::read(Arc::from(bytes)).expect("readable"))
}

fn panel(id: &str, point: &str, replace: bool, fallback: bool) -> ModPanel {
    ModPanel {
        id: id.into(),
        point: point.into(),
        replace,
        fallback,
    }
}

fn mod_ui(id: &str, pkg: Arc<Package>, api: &str, panels: Vec<ModPanel>) -> ModUi {
    ModUi {
        id: Arc::from(id),
        pkg,
        api: api.into(),
        panels,
    }
}

fn frame(ui: &mut V2Ui, mods: Option<Vec<Arc<str>>>) -> V2FrameReport {
    ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods,
        now_ms: 16.0,
    })
}

/// The top of each element of a part, in CSS px; `None` when it has no box.
fn top(ui: &V2Ui, part: &str) -> Option<f32> {
    ui.runtime()
        .dump_rects(Some(part))
        .first()
        .filter(|r| r.h > 0.0)
        .map(|r| r.y)
}

#[test]
fn panels_go_to_their_points_in_load_order_else_to_mods_or_nowhere() {
    let mut ui = V2Ui::new(skin(), V2Init::default()).expect("ui");
    let alpha = fragment("alpha", &["a1", "a2", "a3"], (1, 0), &[]);
    let beta = fragment("beta", &["b1"], (1, 0), &[]);
    let report = ui.mount_mods(vec![
        mod_ui(
            "alpha",
            alpha,
            "^1",
            vec![
                panel("a1", "hud.right", false, true),
                panel("a2", "hud.nowhere", false, true),
                panel("a3", "hud.nowhere", false, false),
            ],
        ),
        mod_ui(
            "beta",
            beta,
            "^1",
            vec![panel("b1", "hud.right", false, true)],
        ),
    ]);
    assert_eq!(
        report[0].panels,
        [
            ("a1".to_string(), "hud.right".to_string()),
            ("a2".to_string(), "mods".to_string())
        ]
    );
    assert_eq!(report[0].skipped.len(), 1, "{report:?}");
    assert!(report[0].skipped[0].contains("a3"));
    assert_eq!(
        report[1].panels,
        [("b1".to_string(), "hud.right".to_string())]
    );
    frame(&mut ui, Some(vec![Arc::from("alpha"), Arc::from("beta")]));
    // The skin's own child first, then the panels by load order.
    let (own, a1, b1) = (top(&ui, "own"), top(&ui, "a1"), top(&ui, "b1"));
    assert!(own < a1 && a1 < b1, "{own:?} {a1:?} {b1:?}");
    let (a2, mods) = (top(&ui, "a2"), top(&ui, "mods-col"));
    assert!(
        a2.is_some() && a2 == mods,
        "{a2:?} in the mods column at {mods:?}"
    );
    assert_eq!(top(&ui, "a3"), None, "fallback none: no panel");
    let panels = ui.runtime().panels();
    assert_eq!(panels.iter().filter(|p| p.owner.is_some()).count(), 3);

    // A skin built anew (here: the same UI after a remount) keeps them; mounting again replaces them.
    let again = ui.mount_mods(vec![]);
    assert!(again.is_empty());
    assert!(ui.runtime().panels().iter().all(|p| p.owner.is_none()));
}

#[test]
fn a_mod_for_another_ui_api_shows_one_placeholder_line_in_mods() {
    let mut ui = V2Ui::new(skin(), V2Init::default()).expect("ui");
    let gamma = fragment("gamma", &["g1"], (2, 0), &[]);
    let report = ui.mount_mods(vec![mod_ui(
        "gamma",
        gamma,
        "^2",
        vec![panel("g1", "hud.right", false, true)],
    )]);
    assert_eq!(
        report[0].refused.as_deref(),
        Some("gamma: needs interface API ^2")
    );
    assert!(report[0].panels.is_empty());
    let out = frame(&mut ui, Some(vec![Arc::from("gamma")]));
    let panels = ui.runtime().panels();
    let mine: Vec<_> = panels.iter().filter(|p| p.owner.is_some()).collect();
    assert_eq!(mine.len(), 1, "{panels:?}");
    assert_eq!(mine[0].slot.as_deref(), Some("mods"));
    assert_eq!(
        mine[0].failed.as_deref(),
        Some("gamma: needs interface API ^2")
    );
    // The line is the reason: the placeholder shows it.
    assert_eq!(mine[0].name, "gamma: needs interface API ^2");
    assert!(
        ui.texts()
            .iter()
            .any(|t| t == "gamma: needs interface API ^2")
            || ui.package().fonts().is_empty()
    );
    assert!(
        out.errors
            .iter()
            .any(|e| e.msg.contains("needs interface API")),
        "{:?}",
        out.errors
    );
}

#[test]
fn a_replacing_panel_hides_the_element_and_gives_it_back_when_the_mod_fails() {
    let host = MockScriptHost::new();
    let mut ui = V2Ui::new(skin(), V2Init::default()).expect("ui");
    ui.set_mod_script_host(Box::new(host.clone()));
    let delta = fragment(
        "delta",
        &["map"],
        (1, 0),
        &[("ui/delta.js", "ui.log.info('up');")],
    );
    let report = ui.mount_mods(vec![mod_ui(
        "delta",
        delta,
        "^1",
        vec![panel("map", "hud.minimap", true, true)],
    )]);
    assert_eq!(report[0].refused, None);
    assert_eq!(
        report[0].panels,
        [("map".to_string(), "hud.minimap".to_string())]
    );
    let table = Some(vec![Arc::from("delta")]);
    frame(&mut ui, table);
    assert_eq!(
        top(&ui, "minimap"),
        None,
        "hidden while the mod's panel is there"
    );
    assert!(top(&ui, "map").is_some());
    // The mod's script keeps failing until the supervisor switches it off: its panel goes and the skin's element
    // is back.
    for _ in 0..=ui2_script::MAX_FAILURES {
        let Some(&w) = host.live().last() else { break };
        host.reply(
            w,
            r#"{"t":"fail","ctx":"mod:delta","error":{"message":"Error: boom","file":"ui/delta.js","line":1}}"#,
        );
        frame(&mut ui, None);
    }
    frame(&mut ui, None);
    assert!(top(&ui, "minimap").is_some(), "the element is back");
    assert_eq!(top(&ui, "map"), None);
    assert!(ui.runtime().panels().iter().all(|p| p.owner.is_none()));
}

#[test]
fn the_index_lists_the_fragments_and_a_package_must_match_its_entry() {
    let pkg = fragment("hello", &["echo"], (1, 0), &[]);
    let hash = ui2_host::hash_hex(&pkg.hash());
    let text = format!(
        "[\n  {{\"id\": \"hello\", \"version\": \"0.2.0\", \"file\": \"mods/hello-0.2.0-x.ui2\", \"hash\": \"{hash}\", \
         \"api\": \"^1\", \"panels\": [{{\"id\": \"echo\", \"point\": \"hud.right\", \"mode\": \"append\", \"fallback\": \
         \"none\"}}]}}\n]\n"
    );
    let index = parse_mods_index(&text).expect("index");
    assert_eq!(index.len(), 1);
    let e: &ModIndexEntry = &index[0];
    assert_eq!(e.path(), "ui2/mods/hello-0.2.0-x.ui2");
    let ui = e.open(Arc::clone(pkg.bytes())).expect("opens");
    assert_eq!(&*ui.id, "hello");
    assert_eq!(ui.panels, [panel("echo", "hud.right", false, false)]);
    // Another package under the entry, or another mod's: refused.
    let other = fragment("other", &["echo"], (1, 0), &[]);
    assert!(
        e.open(Arc::clone(other.bytes()))
            .unwrap_err()
            .contains("hash")
    );
    let mut wrong = e.clone();
    wrong.hash = ui2_host::hash_hex(&other.hash());
    assert!(
        wrong
            .open(Arc::clone(other.bytes()))
            .unwrap_err()
            .contains("hello")
    );
    assert!(parse_mods_index("{}").is_err());
}

/// A fragment of mod `echo` with one 40 px panel `box` that toggles `state.open` on a click, the state default
/// `open = true` and a script.
fn stateful() -> Arc<Package> {
    use ui2_package::profile::action::{Action, Event};
    use ui2_package::{BindKind, Binding, Lit};
    let mut b = PackageBuilder::new();
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.height = LenAuto::Len(Len::px(40.0));
    let record = b.style_record(r);
    let style = b.style_set(StyleSet::plain(vec![record]));
    let part = b.str("box");
    let name = b.str("doc");
    let tpl = b.template(Template {
        name,
        nodes: vec![Node {
            kind: NodeKind::Box { tag: Tag::Div },
            subtree: 1,
            style,
            part: Some(part),
            flags: 0,
        }],
    });
    b.binding(Binding {
        tpl,
        node: 0,
        kind: BindKind::Event {
            ev: Event::Click,
            actions: vec![Action::Toggle {
                path: "state.open".into(),
            }],
        },
    });
    b.state_default("open", Lit::Bool(true));
    b.state_default("view.tab", Lit::Str("all".into()));
    b.script("ui/echo.js", b"ui.log.info('up');");
    let mut m: Manifest = Kit::manifest();
    m.kind = Kind::Fragment {
        slots: vec!["mods".into()],
    };
    m.id = "echo".into();
    let bytes = b.finish(m).expect("fragment");
    Arc::new(Package::read(Arc::from(bytes)).expect("readable"))
}

/// `modstate.echo.open`.
fn open(ui: &V2Ui) -> Option<bool> {
    match ui.mod_state_value("echo")? {
        ui2_model::Value::Struct(f) => f.iter().find_map(|v| match v {
            ui2_model::Value::Bool(b) => Some(*b),
            _ => None,
        }),
        _ => None,
    }
}

fn click(ui: &mut V2Ui, x: f64, y: f64) {
    use platform::{
        Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
        PointerPhase,
    };
    for (phase, button) in [
        (PointerPhase::Move, None),
        (PointerPhase::Down, Some(MouseButton::Left)),
        (PointerPhase::Up, Some(MouseButton::Left)),
    ] {
        ui.handle_input(
            &platform::InputEvent::Pointer(PointerEvent {
                id: PointerId::MOUSE,
                kind: PointerKind::Mouse,
                phase,
                position: PhysicalPosition { x, y },
                button,
                modifiers: Modifiers::default(),
            }),
            16.0,
        );
    }
}

#[test]
fn a_mods_state_is_written_by_its_markup_and_its_script_and_not_by_the_skin() {
    let host = MockScriptHost::new();
    let mut ui = V2Ui::new(skin(), V2Init::default()).expect("ui");
    ui.set_mod_script_host(Box::new(host.clone()));
    let report = ui.mount_mods(vec![mod_ui(
        "echo",
        stateful(),
        "^1",
        vec![panel("box", "hud.right", false, true)],
    )]);
    assert_eq!(report[0].refused, None, "{report:?}");
    frame(&mut ui, Some(vec![Arc::from("echo")]));
    assert_eq!(open(&ui), Some(true), "the default of [ui] state");
    // The panel's `toggle state.open` writes modstate.echo.open.
    let r = ui.runtime().dump_rects(Some("box"))[0].clone();
    click(
        &mut ui,
        f64::from(r.x + r.w / 2.0),
        f64::from(r.y + r.h / 2.0),
    );
    let out = frame(&mut ui, None);
    assert_eq!(out.refused, 0, "{:?}", out.errors);
    assert_eq!(open(&ui), Some(false));
    // The mod's script writes it as its `state.open`.
    let w = host.live()[0];
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"state":[{"p":"state.open","v":true}]}"#,
    );
    let out = frame(&mut ui, None);
    assert_eq!(out.refused, 0);
    assert_eq!(open(&ui), Some(true));
    // The skin's state is another namespace: the mod's is not `state.*`, and a mount again keeps the value.
    assert!(!ui.store().is_mounted(&ui2_model::Namespace::State));
    host.reply(
        w,
        r#"{"t":"out","n":2,"ack":null,"state":[{"p":"state.open","v":false}]}"#,
    );
    frame(&mut ui, None);
    ui.mount_mods(vec![mod_ui(
        "echo",
        stateful(),
        "^1",
        vec![panel("box", "hud.right", false, true)],
    )]);
    frame(&mut ui, None);
    assert_eq!(open(&ui), Some(false), "kept over a mount again");
}
