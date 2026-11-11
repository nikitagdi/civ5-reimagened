//! The package of the stored top strip run (no browser): it reads back, the runtime instantiates it, and the
//! runtime's node labels are the preview's marks of the page (one numbering rule for preview, templates and
//! runtime). The fonts are the full font files here (the subsets need Python; `tests/fonts.rs` shows they
//! measure the same).

use std::collections::BTreeSet;
use std::sync::Arc;

use ui2_convert::fonts::{self, Subset};
use ui2_convert::ir::Dom;
use ui2_convert::write::Pictures;
use ui2_convert::{Input, Run, check_skin};
use ui2_package::{NodeKind, Package, TextSrc};

fn engraved() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skins/engraved")
}

fn run() -> Run {
    let dump = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/top-strip.ir.json"),
    )
    .expect("fixture");
    let run = check_skin(&engraved(), Input::Dump(&dump));
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    run
}

fn full_fonts(run: &Run) -> Vec<Subset> {
    let src = run.source.as_ref().expect("source");
    let (decls, errs) = fonts::faces(&run.scanned.as_ref().expect("scan").ir, &src.skin);
    assert!(errs.is_empty(), "{errs:?}");
    decls
        .into_iter()
        .map(|d| Subset {
            bytes: std::fs::read(fonts::repo_root().join(&d.file)).expect("font"),
            decl: d,
        })
        .collect()
}

fn package(run: &Run) -> Arc<Package> {
    let w = ui2_convert::build::package_of(run, &full_fonts(run), &Pictures::default(), &[])
        .expect("package");
    Arc::new(Package::read(Arc::from(w.bytes.as_slice())).expect("read"))
}

#[test]
fn write_is_a_pure_function_of_the_run() {
    let r = run();
    let fonts = full_fonts(&r);
    let a = ui2_convert::build::package_of(&r, &fonts, &Pictures::default(), &[]).expect("a");
    let b = ui2_convert::build::package_of(&r, &fonts, &Pictures::default(), &[]).expect("b");
    assert_eq!(a.bytes, b.bytes);
    let tags: Vec<&str> = a.sections.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        tags,
        [
            "BIND", "EXTN", "FONT", "L10N", "MANI", "SRCM", "STRS", "STYL", "TMPL", "TOKN"
        ]
    );
}

#[test]
fn write_templates_follow_the_page() {
    let r = run();
    let pkg = package(&r);
    let t = pkg.templates();
    // The document and one template per `if` and `for` of the strip.
    assert_eq!(t.len(), 5);
    let services = t[0]
        .nodes
        .iter()
        .filter(|n| matches!(n.kind, NodeKind::For { .. } | NodeKind::If { .. }))
        .count();
    assert_eq!(services, 4);
    // Bound and localized texts are leaves of their elements.
    let texts = |f: fn(&TextSrc) -> bool| {
        t.iter()
            .flat_map(|x| &x.nodes)
            .filter(|n| matches!(&n.kind, NodeKind::Text(s) if f(s)))
            .count()
    };
    assert!(texts(|s| matches!(s, TextSrc::Bound(_))) >= 10);
    assert_eq!(texts(|s| matches!(s, TextSrc::L10n { .. })), 3);
    assert!(pkg.slots().any(|(s, _)| s == "mods"));
    assert!(pkg.parts().any(|(s, _)| s == "top.research"));
    // The loop body reads the item through its loop variable (the runtime resolves it per instance); no
    // expression names a list's elements directly.
    let paths: Vec<String> = (0..)
        .map_while(|i| pkg.expr(ui2_package::ExprId(i)))
        .map(|e| format!("{e:?}"))
        .collect();
    assert!(
        paths
            .iter()
            .any(|p| p.contains("Path([\"r\", \"available\"])")),
        "{paths:?}"
    );
    assert!(!paths.iter().any(|p| p.contains("[]")), "{paths:?}");
}

#[test]
fn write_runtime_labels_are_the_preview_marks() {
    use ui2_draw::Scene;
    use ui2_model::{ModelStore, Schema};
    use ui2_runtime::{Env, Runtime, Services, load_faces};
    use ui2_text::TextEngine;

    let r = run();
    let marks: BTreeSet<String> = Dom::new(&r.scanned.as_ref().expect("scan").ir.dom)
        .elems
        .iter()
        .filter_map(|e| e.mark.clone())
        .collect();
    let pkg = package(&r);
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let env = Env {
        viewport_px: (1920, 1080),
        dpr: 1.0,
        ui_scale: 1.0,
        theme: 0,
    };
    let mut rt = Runtime::new(pkg, env, Services::default()).expect("runtime");
    let store = ModelStore::new(Schema::v1());
    rt.frame(0.0, &store, &mut engine, &mut Scene::new(1.0));
    let rects = rt.dump_rects(None);
    assert!(rects.len() > 40);
    for rect in &rects {
        let label = rect.node.split('#').next().unwrap_or_default();
        assert!(marks.contains(label), "{label} is not a mark of the page");
    }
    // The strip is `0:3` (map layer, HUD layer, the icon sprite before it).
    let top = rt.dump_rects(Some("top"));
    assert!(
        top.iter()
            .any(|r| r.node == "0:3" && r.y == 0.0 && r.h == 48.0),
        "{top:?}"
    );
    assert!(!top.iter().any(|r| r.node == "0:1"));
}
