//! The node numbering of the top strip: the preview's marks (stored in the dump) follow the converter's rule.

use ui2_convert::ir::Dom;
use ui2_convert::numbering;

fn dump() -> String {
    let p =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/top-strip.ir.json");
    std::fs::read_to_string(p).expect("fixture")
}

#[test]
fn numbering_matches_the_preview_marks() {
    let d = ui2_convert::read_dump(&dump()).expect("dump");
    assert!(
        d.scanned.preview,
        "the fixture was made through the preview script"
    );
    let dom = Dom::new(&d.scanned.ir.dom);
    let (numbers, errs) = numbering::check(&dom);
    assert!(errs.is_empty(), "{errs:?}");
    let label = |pred: &dyn Fn(&ui2_convert::ir::Element) -> bool| {
        let e = dom.elems.iter().find(|e| pred(e)).expect("element");
        numbering::label(numbers[&e.id])
    };
    // The layer roots are the first document nodes.
    assert_eq!(label(&|e| e.attr("data-layer") == Some("map")), "0:0");
    assert_eq!(label(&|e| e.attr("data-layer") == Some("hud")), "0:1");
    // Each `if` and `for` root starts a template of its own, in document order.
    let roots: Vec<String> = dom
        .elems
        .iter()
        .filter(|e| numbering::is_template_root(e))
        .map(|e| numbering::label(numbers[&e.id]))
        .collect();
    assert_eq!(roots, ["1:0", "2:0", "3:0", "4:0"]);
    // An element inside a loop body is numbered in the body's template.
    assert_eq!(label(&|e| e.attr("class") == Some("rico")), "4:1");
}

#[test]
fn numbering_skips_svg_content_and_non_nodes() {
    let d = ui2_convert::read_dump(&dump()).expect("dump");
    let dom = Dom::new(&d.scanned.ir.dom);
    let numbers = numbering::number(&dom);
    for e in &dom.elems {
        let inside_svg = dom
            .ancestors(dom.by_id[&e.id])
            .any(|a| dom.elems[a].tag == "svg");
        if inside_svg || numbering::NOT_NODES.contains(&e.tag.as_str()) {
            assert!(!numbers.contains_key(&e.id), "{}", e.describe());
        }
    }
}
