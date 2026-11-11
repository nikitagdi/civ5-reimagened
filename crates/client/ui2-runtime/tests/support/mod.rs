//! TEMPORARY test support: the top strip package built with the test kit from Chrome's computed styles.
//!
//! Replaced by the converter's top strip package and rectangle dump once they exist; delete this module,
//! `top_strip_chrome.mjs` and `fixtures/top-strip-chrome.json` then. The fixture is made by
//! `top_strip_chrome.mjs` (see there); this module turns its node list (tags, parts, texts, computed styles)
//! into a package with one plain style set per element, as the converter would bake it for one viewport cell.

// Each test file uses part of this module.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use ui2_package::style::{
    BoxSizing, ContentAlign, Display, FlexDirection, FlexWrap, FontStyle, ItemsAlign, Len, LenAuto,
    LenNone, LenNormal, LineHeight, NumericFeatures, Overflow, Position, SelfAlign, StyleRecord,
    TextAlign, TextOverflow, TextWrapMode, WhiteSpaceCollapse,
};
use ui2_package::{BindKind, Layer, NodeKind, Package, PropId, Tag};
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

/// How the texts of the strip are made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Texts {
    /// The sample texts Chrome laid out, as static text.
    Static,
    /// The bindings of `parts/top.html` (scalar ones: `if` and `for` are left static), with the skin's
    /// texts; the research fill's width comes from its `style-width` binding (its record has none).
    Bound,
}

/// A text leaf's binding in `parts/top.html`: a template, or a skin text key with argument expressions.
enum TextBind {
    Tpl(&'static str),
    L10n(&'static str, &'static [&'static str]),
}

/// The bound text leaves of the strip, by node name.
const TEXT_BINDS: &[(&str, TextBind)] = &[
    ("0:8#0", TextBind::Tpl("{{research.current.name}}")),
    (
        "0:9#0",
        TextBind::L10n("top.turns", &["research.current.turns"]),
    ),
    (
        "0:10#0",
        TextBind::L10n("top.next", &["research.next.name"]),
    ),
    ("0:35#0", TextBind::Tpl("{{fmt.signed(top.science_rate)}}")),
    ("0:38#0", TextBind::Tpl("{{top.gold}}")),
    ("0:39#0", TextBind::Tpl("{{fmt.signed(top.gold_rate)}}")),
    ("0:42#0", TextBind::Tpl("{{top.happiness}}")),
    (
        "0:43#0",
        TextBind::Tpl("{{top.golden_age.progress}}/{{top.golden_age.threshold}}"),
    ),
    ("0:46#0", TextBind::Tpl("{{fmt.signed(top.culture_rate)}}")),
    ("0:47#0", TextBind::L10n("top.turns", &["top.policy_turns"])),
    ("0:50#0", TextBind::Tpl("{{top.faith}}")),
    ("0:51#0", TextBind::Tpl("{{fmt.signed(top.faith_rate)}}")),
    ("0:54#0", TextBind::Tpl("{{top.tourism}}")),
];

/// Elements with a `neg` class toggle: node and condition.
const NEG_TOGGLES: &[(&str, &str)] = &[
    ("0:39", "top.gold_rate < 0"),
    ("0:51", "top.faith_rate < 0"),
];

/// The research fill and its width binding.
const FILL: (&str, &str) = ("0:12", "{{research.current.fraction * 100}}%");

/// The color of `.neg` (a record variant the toggles select).
pub const NEG_COLOR: [u8; 4] = [214, 96, 77, 255];

fn tpl(kit: &mut Kit, src: &str) -> ui2_package::TplId {
    use ui2_package::profile::expr::{TplSeg, parse_template};
    use ui2_runtime::testkit::TplPiece;
    let t = parse_template(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    let pieces = t
        .segs
        .into_iter()
        .map(|s| match s {
            TplSeg::Lit(l) => TplPiece::Lit(l),
            TplSeg::Expr(e) => TplPiece::Expr(e),
        })
        .collect();
    kit.tpl_of(pieces)
}

fn expr(kit: &mut Kit, src: &str) -> ui2_package::ExprId {
    let e = ui2_package::profile::expr::parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    kit.expr(e)
}

/// The skin's texts (`l10n/en_US.toml`: `"key" = "template"` lines).
fn skin_texts(kit: &mut Kit) {
    let path = repo_root().join("skins/engraved/l10n/en_US.toml");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    for line in src.lines() {
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some((key, value)) = rest.split_once("\" = \"") else {
            continue;
        };
        kit.l10n("en_US", key, value.trim_end().trim_end_matches('"'));
    }
}

/// The sample state of the strip (`sample/hud.json`) as JSON, for edits.
pub fn sample_json() -> serde_json::Value {
    let path = repo_root().join("skins/engraved/sample/hud.json");
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&src).expect("sample is JSON")
}

/// The top strip package: the fixture's nodes as template 0, Fira Sans 400 and 500.
pub fn package(fx: &Fixture) -> Arc<Package> {
    package_with(fx, Texts::Static)
}

/// The top strip package with static or bound texts.
pub fn package_with(fx: &Fixture, texts: Texts) -> Arc<Package> {
    let mut kit = Kit::new();
    let bound = texts == Texts::Bound;
    if bound {
        skin_texts(&mut kit);
    }
    for (weight, file) in [(400, "FiraSans-Regular.ttf"), (500, "FiraSans-Medium.ttf")] {
        let path = repo_root().join("third_party/fonts/fira-sans").join(file);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        kit.font("Fira Sans", weight, false, &bytes);
    }
    let mut sets = BTreeMap::new();
    let mut children: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, n) in fx.nodes.iter().enumerate() {
        if let Some(s) = &n.style {
            let mut rec = record(&mut kit, s);
            if bound && n.node == FILL.0 {
                // The width comes from the binding only.
                rec.width = LenAuto::Len(Len::ZERO);
            }
            let set = if bound && NEG_TOGGLES.iter().any(|(node, _)| *node == n.node) {
                let mut neg = rec.clone();
                neg.color = ui2_package::style::ColorV::Lit(NEG_COLOR);
                let (rec, neg) = (kit.record(rec), kit.record(neg));
                let class = kit.str("neg");
                let cond = ui2_package::Cond {
                    up: 0,
                    kind: ui2_package::CondKind::Class(class),
                };
                let mut table = Vec::new();
                for _ in 0..kit.cells() {
                    table.extend([rec, neg]);
                }
                kit.set(vec![cond], table)
            } else {
                let rec = kit.record(rec);
                kit.plain(rec)
            };
            sets.insert(n.node.clone(), set);
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
        bound: bool,
        i: usize,
    ) -> TNode {
        let n = &fx.nodes[i];
        if let Some(text) = &n.text {
            let parent = n.parent.as_deref().unwrap_or_default();
            let style = sets[parent];
            let bind = TEXT_BINDS.iter().find(|(node, _)| *node == n.node);
            return match (bound, bind) {
                (true, Some((_, TextBind::Tpl(src)))) => {
                    let t = tpl(kit, src);
                    TNode::bound_text(t, style)
                }
                (true, Some((_, TextBind::L10n(key, args)))) => {
                    let key = kit.str(key);
                    let args = args.iter().map(|a| expr(kit, a)).collect();
                    TNode::l10n_text(key, args, style)
                }
                _ => {
                    let s = kit.str(text);
                    TNode::text(s, style)
                }
            };
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
        if bound {
            if let Some((_, cond)) = NEG_TOGGLES.iter().find(|(node, _)| *node == n.node) {
                let class = kit.str("neg");
                let cond = expr(kit, cond);
                t = t.bind(BindKind::ClassToggle { class, cond });
            }
            if n.node == FILL.0 {
                let tpl = tpl(kit, FILL.1);
                t = t.bind(BindKind::Style {
                    prop: PropId::Width,
                    tpl,
                });
            }
        }
        for &c in children.get(&n.node).map_or(&[][..], |v| v.as_slice()) {
            let child = build(kit, fx, sets, children, bound, c);
            t = t.child(child);
        }
        t
    }
    let root = build(&mut kit, fx, &sets, &children, bound, 0);
    kit.template("doc", vec![root]);
    kit.finish().expect("package")
}
