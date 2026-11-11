//! TEMPORARY test support: the top strip package built with the test kit from Chrome's computed styles.
//!
//! Replaced by the converter's top strip package and rectangle dump once they exist; delete this module,
//! `top_strip_chrome.mjs` and `fixtures/top-strip-chrome.json` then. The fixture is made by
//! `top_strip_chrome.mjs` (see there); this module turns its node list (tags, parts, texts, computed styles)
//! into a package with one plain style set per element, as the converter would bake it for one viewport cell.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use ui2_package::style::{
    BoxSizing, ContentAlign, Display, FlexDirection, FlexWrap, FontStyle, ItemsAlign, Len, LenAuto,
    LenNone, LenNormal, LineHeight, NumericFeatures, Overflow, Position, SelfAlign, StyleRecord,
    TextAlign, TextOverflow, TextWrapMode, WhiteSpaceCollapse,
};
use ui2_package::{Layer, NodeKind, Package, Tag};
use ui2_runtime::testkit::{Kit, TNode};

/// One node of the fixture.
pub struct FNode {
    pub node: String,
    pub parent: Option<String>,
    pub tag: String,
    pub part: String,
    pub text: Option<String>,
    pub style: Option<serde_json::Map<String, Value>>,
}

/// One rectangle: node, keys, x, y, w, h.
pub type Rect = (String, String, [f64; 4]);

/// The fixture: nodes and, per viewport, Chrome's rectangles.
pub struct Fixture {
    pub nodes: Vec<FNode>,
    pub viewports: Vec<([u32; 2], Vec<Rect>)>,
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

/// Reads the fixture.
pub fn load() -> Fixture {
    let json: Value = serde_json::from_str(include_str!("../fixtures/top-strip-chrome.json"))
        .expect("fixture is JSON");
    let nodes = json["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .map(|n| FNode {
            node: str_of(&n["node"]),
            parent: n["parent"].as_str().map(str::to_string),
            tag: str_of(&n["tag"]),
            part: str_of(&n["part"]),
            text: n["text"].as_str().map(str::to_string),
            style: n["style"].as_object().cloned(),
        })
        .collect();
    let viewports = json["viewports"]
        .as_array()
        .expect("viewports")
        .iter()
        .map(|v| {
            let vp = [
                v["viewport"][0].as_u64().unwrap_or(0) as u32,
                v["viewport"][1].as_u64().unwrap_or(0) as u32,
            ];
            let rects = v["rects"]
                .as_array()
                .expect("rects")
                .iter()
                .map(|r| {
                    let f = |i: usize| r[i].as_f64().expect("number");
                    (str_of(&r[0]), str_of(&r[1]), [f(2), f(3), f(4), f(5)])
                })
                .collect();
            (vp, rects)
        })
        .collect();
    Fixture { nodes, viewports }
}

fn len(v: &str) -> Len {
    if let Some(px) = v.strip_suffix("px") {
        Len::px(px.parse().unwrap_or_else(|_| panic!("length {v}")))
    } else if let Some(pct) = v.strip_suffix('%') {
        Len::pct(pct.parse().unwrap_or_else(|_| panic!("percentage {v}")))
    } else {
        panic!("the profile has no value {v:?} for a length")
    }
}

fn len_auto(v: &str) -> LenAuto {
    if v == "auto" {
        LenAuto::Auto
    } else {
        LenAuto::Len(len(v))
    }
}

fn len_none(v: &str) -> LenNone {
    if v == "none" {
        LenNone::None
    } else {
        LenNone::Len(len(v))
    }
}

fn len_normal(v: &str) -> LenNormal {
    if v == "normal" {
        LenNormal::Normal
    } else {
        LenNormal::Len(len(v))
    }
}

fn kw<T>(parse: fn(&str) -> Option<T>, v: &str) -> T {
    parse(v).unwrap_or_else(|| panic!("keyword {v:?}"))
}

/// The record of one element from its computed styles.
fn record(kit: &mut Kit, s: &serde_json::Map<String, Value>) -> StyleRecord {
    let g = |k: &str| -> String { str_of(&s[k]) };
    let num = |k: &str| -> f32 { g(k).parse().unwrap_or_else(|_| panic!("number {k}")) };
    let mut r = StyleRecord::initial();
    r.display = kw(Display::from_keyword, &g("display"));
    r.position = kw(Position::from_keyword, &g("position"));
    r.box_sizing = kw(BoxSizing::from_keyword, &g("box-sizing"));
    r.top = len_auto(&g("top"));
    r.right = len_auto(&g("right"));
    r.bottom = len_auto(&g("bottom"));
    r.left = len_auto(&g("left"));
    r.width = len_auto(&g("width"));
    r.height = len_auto(&g("height"));
    r.min_width = len_auto(&g("min-width"));
    r.min_height = len_auto(&g("min-height"));
    r.max_width = len_none(&g("max-width"));
    r.max_height = len_none(&g("max-height"));
    r.margin_top = len_auto(&g("margin-top"));
    r.margin_right = len_auto(&g("margin-right"));
    r.margin_bottom = len_auto(&g("margin-bottom"));
    r.margin_left = len_auto(&g("margin-left"));
    r.padding_top = len(&g("padding-top"));
    r.padding_right = len(&g("padding-right"));
    r.padding_bottom = len(&g("padding-bottom"));
    r.padding_left = len(&g("padding-left"));
    r.border_top_width = len(&g("border-top-width"));
    r.border_right_width = len(&g("border-right-width"));
    r.border_bottom_width = len(&g("border-bottom-width"));
    r.border_left_width = len(&g("border-left-width"));
    r.overflow_x = kw(Overflow::from_keyword, &g("overflow-x"));
    r.overflow_y = kw(Overflow::from_keyword, &g("overflow-y"));
    r.flex_direction = kw(FlexDirection::from_keyword, &g("flex-direction"));
    r.flex_wrap = kw(FlexWrap::from_keyword, &g("flex-wrap"));
    r.flex_grow = num("flex-grow");
    r.flex_shrink = num("flex-shrink");
    r.flex_basis = len_auto(&g("flex-basis"));
    r.order = num("order") as i32;
    r.justify_content = kw(ContentAlign::from_keyword, &g("justify-content"));
    r.align_items = kw(ItemsAlign::from_keyword, &g("align-items"));
    r.align_self = kw(SelfAlign::from_keyword, &g("align-self"));
    r.align_content = kw(ContentAlign::from_keyword, &g("align-content"));
    r.row_gap = len_normal(&g("row-gap"));
    r.column_gap = len_normal(&g("column-gap"));
    r.font_family = g("font-family")
        .split(',')
        .map(|f| kit.str(f.trim().trim_matches('"')))
        .collect();
    r.font_size = len(&g("font-size"));
    r.font_weight = num("font-weight") as u16;
    r.font_style = kw(FontStyle::from_keyword, &g("font-style"));
    let mut numeric = NumericFeatures::default();
    for word in g("font-variant-numeric").split_whitespace() {
        match word {
            "tabular-nums" => numeric.tabular = true,
            "proportional-nums" => numeric.proportional = true,
            "lining-nums" => numeric.lining = true,
            "oldstyle-nums" => numeric.oldstyle = true,
            _ => {}
        }
    }
    r.font_variant_numeric = numeric;
    let lh = g("line-height");
    r.line_height = if lh == "normal" {
        LineHeight::Normal
    } else if let Some(px) = lh.strip_suffix("px") {
        LineHeight::Px(px.parse().expect("line height"))
    } else {
        LineHeight::Factor(lh.parse().expect("line height"))
    };
    r.letter_spacing = len_normal(&g("letter-spacing"));
    r.word_spacing = len_normal(&g("word-spacing"));
    r.text_align = kw(TextAlign::from_keyword, &g("text-align"));
    r.white_space_collapse = kw(WhiteSpaceCollapse::from_keyword, &g("white-space-collapse"));
    r.text_wrap_mode = kw(TextWrapMode::from_keyword, &g("text-wrap-mode"));
    r.text_overflow = kw(TextOverflow::from_keyword, &g("text-overflow"));
    r
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// The top strip package: the fixture's nodes as template 0, Fira Sans 400 and 500.
pub fn package(fx: &Fixture) -> Arc<Package> {
    let mut kit = Kit::new();
    for (weight, file) in [(400, "FiraSans-Regular.ttf"), (500, "FiraSans-Medium.ttf")] {
        let path = repo_root().join("third_party/fonts/fira-sans").join(file);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        kit.font("Fira Sans", weight, false, &bytes);
    }
    let mut sets = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, n) in fx.nodes.iter().enumerate() {
        if let Some(s) = &n.style {
            let rec = record(&mut kit, s);
            let rec = kit.record(rec);
            sets.insert(n.node.clone(), kit.plain(rec));
        }
        if let Some(p) = &n.parent {
            children.entry(p.clone()).or_default().push(i);
        }
    }
    fn build(
        kit: &mut Kit,
        fx: &Fixture,
        sets: &BTreeMap<String, ui2_package::StyleSetId>,
        children: &BTreeMap<String, Vec<usize>>,
        i: usize,
    ) -> TNode {
        let n = &fx.nodes[i];
        if let Some(text) = &n.text {
            let parent = n.parent.as_deref().unwrap_or_default();
            let s = kit.str(text);
            return TNode::text(s, sets[parent]);
        }
        let set = sets[&n.node];
        let tag = match n.tag.as_str() {
            "div" => Tag::Div,
            "header" => Tag::Header,
            "span" => Tag::Span,
            "button" => Tag::Button,
            "svg" => Tag::Svg,
            "img" => Tag::Img,
            other => panic!("tag {other}"),
        };
        let mut t = if n.parent.is_none() {
            TNode::new(NodeKind::Layer(Layer::Hud), set)
        } else {
            TNode::new(NodeKind::Box { tag }, set)
        };
        if !n.part.is_empty() {
            let p = kit.str(&n.part);
            t = t.part(p);
        }
        for &c in children.get(&n.node).map_or(&[][..], |v| v.as_slice()) {
            let child = build(kit, fx, sets, children, c);
            t = t.child(child);
        }
        t
    }
    let root = build(&mut kit, fx, &sets, &children, 0);
    kit.template("doc", vec![root]);
    kit.finish().expect("package")
}
