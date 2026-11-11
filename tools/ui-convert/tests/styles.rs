//! Styles from stored extractions (no browser): `tests/fixtures/top-strip.ir.json` is the top
//! strip of `skins/engraved` (scanned with its includes written out, see
//! in `tests/fixtures/README.md`), `layout.ir.json` the layout fixture
//! (`tests/fixtures/layout/`).

use std::path::{Path, PathBuf};

use ui2_convert::ir::{AnonKind, Dom, Element};
use ui2_convert::styles::{BoxNode, Compiled};
use ui2_convert::{Input, Run, check_skin};
use ui2_package::style::{ColorV, Paint};
use ui2_package::{Cond, CondKind, PropId, TokenId};

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn engraved() -> PathBuf {
    crate_dir()
        .join("..")
        .join("..")
        .join("skins")
        .join("engraved")
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(crate_dir().join("tests").join("fixtures").join(name)).expect("fixture")
}

fn top() -> Run {
    check_skin(&engraved(), Input::Dump(&fixture("top-strip.ir.json")))
}

fn styles(run: &Run) -> &Compiled {
    run.styles.as_ref().expect("styles compiled")
}

/// The first element with all these classes (and the part, when given).
fn find<'a>(dom: &Dom<'a>, classes: &[&str], part: Option<&str>) -> &'a Element {
    dom.elems
        .iter()
        .copied()
        .find(|e| {
            let c = e.classes();
            classes.iter().all(|k| c.contains(k))
                && part.is_none_or(|p| e.attr("data-part") == Some(p))
        })
        .unwrap_or_else(|| panic!("no element {classes:?} {part:?}"))
}

fn token(s: &Compiled, name: &str) -> ColorV {
    let i = s
        .tokens
        .roots
        .iter()
        .position(|(n, _)| s.strs.get(*n) == name)
        .unwrap_or_else(|| panic!("no token {name}"));
    ColorV::Tok(TokenId(i as u32))
}

#[test]
fn styles_fixture_matches_the_plan() {
    let run = top();
    assert!(
        !run.errors.iter().any(|e| e.code == "E9-003"),
        "{:?}",
        run.errors
    );
    let s = styles(&run);
    assert_eq!(s.tokens.roots.len(), 22);
    assert_eq!(s.tokens.aliases.len(), 17);
    assert!(s.media_w.is_empty() && s.media_h.is_empty());
}

#[test]
fn styles_mbtn_hover_and_focus_give_four_variants() {
    let run = top();
    let s = styles(&run);
    let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
    let b = find(&dom, &["mbtn"], Some("top.menu.tech"));
    let set = s.set_of(b.id).expect("set");
    // :hover changes the colors, :focus-visible the outline: two conditions, four variants.
    assert_eq!(
        set.conds,
        [
            Cond {
                up: 0,
                kind: CondKind::Hover
            },
            Cond {
                up: 0,
                kind: CondKind::FocusVisible
            }
        ]
    );
    assert_eq!(s.variant_count(b.id), 4);
    let base = s.record_of(b.id, 0, 0).unwrap();
    let hover = s.record_of(b.id, 0, 1).unwrap();
    let focus = s.record_of(b.id, 0, 2).unwrap();
    assert_eq!(focus.outline_style, ui2_package::style::OutlineStyle::Solid);
    assert_eq!(focus.outline_color, token(s, "--brass"));
    assert_eq!(focus.outline_offset, ui2_package::style::Len::px(-2.0));
    assert_eq!(base.outline_style, ui2_package::style::OutlineStyle::None);
    assert_eq!(base.color, token(s, "--bone-3"));
    assert_eq!(hover.color, token(s, "--bone"));
    assert_eq!(base.background_color, ColorV::Lit([0, 0, 0, 0]));
    assert_eq!(hover.background_color, token(s, "--lacquer-2"));
    // The transitions are baked from the base pass and the same in both variants.
    assert_eq!(base.transitions.len(), 2);
    assert_eq!(base.transitions, hover.transitions);
    assert_eq!(base.transitions[0].prop, PropId::BackgroundColor);
    assert_eq!(base.transitions[0].dur_ms, 150.0);
    assert_eq!(base.transitions[0].ease, [0.22, 1.0, 0.36, 1.0]);
    // All eight menu buttons share the set.
    let other = find(&dom, &["mbtn"], Some("top.menu.victory"));
    assert_eq!(s.node_set[&b.id], s.node_set[&other.id]);
}

#[test]
fn styles_color_tokens_become_tok() {
    let run = top();
    let s = styles(&run);
    let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
    let strip = find(&dom, &["top"], Some("top"));
    let r = s.record_of(strip.id, 0, 0).unwrap();
    assert_eq!(r.background_color, token(s, "--lacquer"));
    assert_eq!(r.box_shadow.len(), 1);
    assert!(r.box_shadow[0].inset);
    assert_eq!(r.box_shadow[0].color, token(s, "--engrave"));
    // Synonyms point at their root token.
    let (_, t) = s
        .tokens
        .aliases
        .iter()
        .find(|(n, _)| s.strs.get(*n) == "--ui-text")
        .expect("--ui-text");
    assert_eq!(ColorV::Tok(*t), token(s, "--bone"));
    // A literal stays a literal: the default theme value of a token is stored once, in TOKN.
    let bone = s
        .tokens
        .roots
        .iter()
        .find(|(n, _)| s.strs.get(*n) == "--bone")
        .unwrap();
    assert_eq!(bone.1, [0xeb, 0xe2, 0xd2, 255]);
}

#[test]
fn styles_inherited_color_conditions_reach_the_icon() {
    let run = top();
    let s = styles(&run);
    let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
    let b = find(&dom, &["mbtn"], Some("top.menu.tech"));
    let icon = b.elements().next().expect("svg");
    assert_eq!(icon.tag, "svg");
    let set = s.set_of(icon.id).expect("set");
    assert_eq!(
        set.conds,
        [Cond {
            up: 1,
            kind: CondKind::Hover
        }]
    );
    // stroke: currentColor follows the button's token color.
    let base = s.record_of(icon.id, 0, 0).unwrap();
    let hover = s.record_of(icon.id, 0, 1).unwrap();
    assert_eq!(base.stroke, Paint::Color(token(s, "--bone-3")));
    assert_eq!(hover.stroke, Paint::Color(token(s, "--bone")));
    assert_eq!(base.fill, Paint::None);
}

#[test]
fn styles_toggled_classes_are_conditions() {
    let run = top();
    let s = styles(&run);
    let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
    let gold = dom
        .elems
        .iter()
        .find(|e| e.attr("data-bind-class-toggle") == Some("neg:{{top.gold_rate < 0}}"))
        .expect("gold rate");
    let set = s.set_of(gold.id).unwrap();
    assert_eq!(set.conds.len(), 1);
    assert!(matches!(set.conds[0].kind, CondKind::Class(c) if s.strs.get(c) == "neg"));
    assert_eq!(
        s.record_of(gold.id, 0, 0).unwrap().color,
        token(s, "--bone-2")
    );
    assert_eq!(s.record_of(gold.id, 0, 1).unwrap().color, token(s, "--neg"));
    // `.res .off .v`: the class is toggled on the loop item, one level above the value.
    let res = find(&dom, &["yield", "sec"], None);
    let v = res
        .elements()
        .find(|e| e.classes().contains(&"v"))
        .expect(".v");
    let set = s.set_of(v.id).unwrap();
    assert!(
        set.conds
            .iter()
            .any(|c| c.up == 1 && matches!(c.kind, CondKind::Class(k) if s.strs.get(k) == "off"))
    );
    assert_eq!(s.record_of(v.id, 0, 1).unwrap().font_weight, 400);
    assert_eq!(s.record_of(v.id, 0, 0).unwrap().font_weight, 500);
}

#[test]
fn styles_are_deterministic_and_the_dump_round_trips() {
    let a = top();
    let b = top();
    assert_eq!(styles(&a), styles(&b));
    assert_eq!(a.dump().unwrap(), fixture("top-strip.ir.json"));
}

fn tree_shape(dom: &Dom, n: &BoxNode) -> String {
    match n {
        BoxNode::Elem { id, children, .. } => {
            let e = dom.elems[dom.by_id[id]];
            let name = e.classes().first().map_or(e.tag.clone(), |c| c.to_string());
            if children.is_empty() {
                name
            } else {
                let kids: Vec<String> = children.iter().map(|k| tree_shape(dom, k)).collect();
                format!("{name}[{}]", kids.join(" "))
            }
        }
        BoxNode::Anon { kind, children } => {
            let kids: Vec<String> = children.iter().map(|k| tree_shape(dom, k)).collect();
            let k = match kind {
                AnonKind::Flex => "flex",
                AnonKind::Grid => "grid",
                AnonKind::Block => "block",
            };
            format!("({k} {})", kids.join(" "))
        }
        BoxNode::Text { id } => {
            fn text(n: &ui2_convert::ir::Node, id: u32) -> Option<String> {
                match n {
                    ui2_convert::ir::Node::Text(t) if t.id == id => Some(t.text.clone()),
                    ui2_convert::ir::Node::Element(e) => {
                        e.children.iter().find_map(|c| text(c, id))
                    }
                    _ => None,
                }
            }
            let root = ui2_convert::ir::Node::Element(dom.elems[0].clone());
            format!("'{}'", text(&root, *id).unwrap_or_default())
        }
    }
}

#[test]
fn styles_layout_anonymous_boxes_contents_and_order() {
    let dir = crate_dir().join("tests").join("fixtures").join("layout");
    let run = check_skin(&dir, Input::Dump(&fixture("layout.ir.json")));
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let s = styles(&run);
    let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
    let shapes: Vec<String> = s.tree.iter().map(|n| tree_shape(&dom, n)).collect();
    assert_eq!(
        shapes,
        [
            "layer",
            "layer[row[first (flex '12') b (flex '34') c] blk[(block '56') d (block '78')]]",
            "layer",
            "layer"
        ]
    );
    let wrap = find(&dom, &["wrap"], None);
    assert!(s.contents.contains(&wrap.id));
}

#[test]
fn styles_values_outside_the_profile_are_errors_with_the_node() {
    let text = fixture("top-strip.ir.json");
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let ring_id = {
        let run = top();
        let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
        find(&dom, &["ring"], None).id
    };
    let nodes = v["extract"]["result"]["cells"][0]["nodes"]
        .as_array_mut()
        .unwrap();
    let ring = nodes
        .iter_mut()
        .find(|n| n["id"] == ring_id)
        .expect("ring read");
    ring["base"][PropId::Display.index()] = "table".into();
    ring["off"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!(["float", "right", "none"]));
    let run = check_skin(&engraved(), Input::Dump(&v.to_string()));
    let ring_errs: Vec<(String, String)> = run
        .errors
        .iter()
        .filter(|e| e.element.contains("class=\"ring\""))
        .map(|e| (e.code.clone(), e.prop.clone()))
        .collect();
    assert_eq!(
        ring_errs,
        [
            ("E2-002".to_string(), "display".to_string()),
            ("E2-010".to_string(), "float".to_string())
        ]
    );
    let e = run
        .errors
        .iter()
        .find(|e| e.code == "E2-002" && e.element.contains("ring"))
        .unwrap();
    assert!(e.why.contains("computed by Chrome"), "{e}");
    assert!(e.place.line > 0);
}

#[test]
fn styles_research_ticks_are_a_repeating_gradient_with_a_token_stop() {
    use ui2_package::style::{BgImage, BgSize, GradDir, Len, LenAuto, Repeat, Side};
    let run = top();
    let s = styles(&run);
    let dom = Dom::new(&run.scanned.as_ref().unwrap().ir.dom);
    let ticks = find(&dom, &["ticks"], None);
    let r = s.record_of(ticks.id, 0, 0).unwrap();
    assert_eq!(r.background_image.len(), 1);
    let BgImage::Gradient(g) = &r.background_image[0] else {
        panic!("a gradient layer");
    };
    assert!(!g.conic && !g.repeating);
    assert_eq!(g.dir, GradDir::To(Side::Right));
    assert_eq!(g.stops.len(), 2);
    assert_eq!(g.stops[0].color, ColorV::Lit([0, 0, 0, 0]));
    assert_eq!(g.stops[1].color, token(s, "--lacquer"));
    assert_eq!(
        g.stops[1].pos,
        Some(Len {
            px: -1.0,
            pct: 100.0
        })
    );
    assert_eq!(r.background_repeat[0].x, Repeat::Repeat);
    assert_eq!(r.background_repeat[0].y, Repeat::NoRepeat);
    // The markup's inline size (the binding replaces it at run time).
    assert_eq!(
        r.background_size,
        [BgSize::Size(
            LenAuto::Len(Len::pct(12.6667)),
            LenAuto::Len(Len::pct(100.0))
        )]
    );
}
