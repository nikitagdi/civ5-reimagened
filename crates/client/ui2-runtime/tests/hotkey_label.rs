//! Hotkey labels (`data-hotkey-label`): a node inside a node with `data-hotkey` shows the name of that key.
//! The name is the skin's text `key.<code>` when it has one, otherwise the letter of `KeyX`, the digit of
//! `DigitN` or the code itself; modifiers come first (`key.Ctrl`, `key.Alt`, `key.Shift` or their names).

use ui2_draw::Scene;
use ui2_model::{ModelStore, Schema};
use ui2_package::style::{Display, Len, StyleRecord};
use ui2_package::{BindKind, Layer, NodeKind, Tag, hotkey_mods};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, ErrorCode, Runtime, Services, load_faces};
use ui2_text::TextEngine;

/// A runtime over buttons with hotkeys and labels: `fortify` (KeyF, label straight inside), `found`
/// (Ctrl+Shift+KeyB, the label one element further down, the skin names KeyB), `disband` (Delete), `seven`
/// (Digit7) and `orphan` (a label without a hotkey around it).
fn runtime() -> (Runtime, TextEngine, Scene) {
    let fira = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf"),
    )
    .expect("font");
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &fira);
    let mut rec = StyleRecord::initial();
    rec.display = Display::Block;
    rec.font_family = vec![kit.str("Fira Sans")];
    rec.font_size = Len::px(14.0);
    let rec = kit.record(rec);
    let set = kit.plain(rec);
    kit.l10n("en_US", "key.KeyB", "Б");
    kit.l10n("en_US", "key.Ctrl", "Ctl");
    let empty = kit.str("");
    let label = |part: &str, kit: &mut Kit| {
        let part = kit.str(part);
        TNode::new(NodeKind::Box { tag: Tag::Span }, set)
            .part(part)
            .bind(BindKind::HotkeyLabel)
            .child(TNode::text(empty, set))
    };
    let button = || TNode::new(NodeKind::Box { tag: Tag::Button }, set);
    let fortify = label("fortify", &mut kit);
    let found = label("found", &mut kit);
    let disband = label("disband", &mut kit);
    let seven = label("seven", &mut kit);
    let orphan = label("orphan", &mut kit);
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, set).children([
                button().hotkey("KeyF", 0).child(fortify),
                button()
                    .hotkey("KeyB", hotkey_mods::CTRL | hotkey_mods::SHIFT)
                    .child(TNode::el(set).child(found)),
                button().hotkey("Delete", 0).child(disband),
                button().hotkey("Digit7", 0).child(seven),
                TNode::el(set).child(orphan),
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
    let rt = Runtime::new(pkg, env, Services::default()).expect("runtime");
    (rt, engine, Scene::new(1.0))
}

fn label_of(rt: &Runtime, part: &str) -> String {
    let id = rt.find_part(part).expect("part");
    let leaf = rt.node(id).expect("node").children()[0];
    rt.node(leaf)
        .and_then(|n| n.text())
        .unwrap_or("")
        .to_string()
}

#[test]
fn hotkey_labels_name_the_key_of_the_node_around_them() {
    let (mut rt, mut engine, mut scene) = runtime();
    let store = ModelStore::new(Schema::v1());
    let out = rt.frame(0.0, &store, &mut engine, &mut scene);
    assert_eq!(label_of(&rt, "fortify"), "F");
    assert_eq!(label_of(&rt, "found"), "Ctl+Shift+Б");
    assert_eq!(label_of(&rt, "disband"), "Delete");
    assert_eq!(label_of(&rt, "seven"), "7");
    // A label without a hotkey stays empty and is reported once.
    assert_eq!(label_of(&rt, "orphan"), "");
    let errors: Vec<_> = out
        .errors
        .iter()
        .map(|e| (e.code, e.path.clone()))
        .collect();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, ErrorCode::Bind);
    // The label's text is shaped and laid out like any text.
    let r = rt.dump_rects(Some("fortify"));
    assert!(r.iter().any(|r| r.node.contains('#') && r.w > 0.0), "{r:?}");
    // Labels read nothing of the model: idle frames do no work.
    for _ in 0..3 {
        let out = rt.frame(0.0, &store, &mut engine, &mut scene);
        assert_eq!(
            (out.stats.binds_evaluated, out.stats.visited),
            (0, 0),
            "an idle frame"
        );
    }
}

#[test]
fn hotkey_labels_follow_a_new_language() {
    let (mut rt, mut engine, mut scene) = runtime();
    let store = ModelStore::new(Schema::v1());
    rt.frame(0.0, &store, &mut engine, &mut scene);
    let game = loc::Localizer::new(loc::LanguageInfo::en_us(), Box::new(loc::MapTable::new()));
    rt.set_localizer(std::rc::Rc::new(game));
    let s = rt.frame(0.0, &store, &mut engine, &mut scene).stats;
    // The five labels depend on the language and are evaluated again; their texts did not change.
    assert_eq!(s.binds_evaluated, 5);
    assert_eq!(label_of(&rt, "found"), "Ctl+Shift+Б");
}
