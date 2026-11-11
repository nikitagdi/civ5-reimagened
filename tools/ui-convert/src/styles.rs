//! Styles: Chrome's answers to the plan, turned into the package's typed records.
//!
//! For every node, cell and condition mask the computed strings are parsed by the profile
//! (`parse_value`; a value outside the profile is an E2 error with the node and the property),
//! token beacons turn colors into token references, the transition longhands become
//! `Transition`s, and the records go into a `StyleSet` (`table[cell * 2^n + mask]`). A condition
//! that changes nothing in any cell is dropped from its set. Off-profile properties whose computed
//! value differs from the initial one are errors (E2-010), as are the element table's required
//! values (E1-006), the page background (E2-013) and the layer roots' box (E7-007).
//!
//! Layout follows the layout spike: text runs that form anonymous boxes (scan `anon`) become
//! anonymous nodes, an element with `display: contents` is replaced by its children, and the
//! children of a flex or grid container are stable-sorted by `order`. Both must hold in every
//! cell and state (E3-008, E3-009).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use ui2_package::profile::{BgLayer, Profile, PropId, Tag, Value, parse_value};
use ui2_package::style::{ColorV, Display, StyleRecord};
use ui2_package::{Bound, Cond, CondKind, StrId, StyleId, StyleSet, TokenId, Tokens};

use crate::errors::{ConvError, Place, codes};
use crate::ir::{AnonKind, Dom, Element, Ix, Node};
use crate::plan::{CondK, Plan, PlanNode, token_capable};
use crate::select::Pseudo;

/// One overlay of values: `(property index, value)`.
pub type Set = Vec<(usize, String)>;

/// One variant of a node in the extraction answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VarOut {
    /// The condition mask.
    pub mask: u32,
    /// The values that differ from the base.
    pub set: Set,
}

/// The beacon pass of one node.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BeaconOut {
    /// The token-capable values that changed under the beacons, base state.
    pub base: Set,
    /// The same per variant.
    pub vars: Vec<VarOut>,
}

/// One node of one cell.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NodeOut {
    /// The IR id.
    pub id: u32,
    /// Every profile property, `PropId` order.
    pub base: Vec<String>,
    /// The plan's extra values.
    pub extra: Vec<String>,
    /// Off-profile properties that differ from the initial values: `[name, value, initial]`.
    pub off: Vec<(String, String, String)>,
    /// The variants.
    pub vars: Vec<VarOut>,
    /// The beacon pass.
    pub beacon: BeaconOut,
    /// The border box in CSS px (base state), for a node with a `clip-path` only.
    #[serde(default)]
    pub size: Option<[f32; 2]>,
}

/// One cell.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CellOut {
    /// The viewport read.
    pub viewport: [u32; 2],
    /// The nodes, in plan order.
    pub nodes: Vec<NodeOut>,
}

/// A custom property of `:root` as Chrome computed it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TokenOut {
    /// `--name`.
    pub name: String,
    /// The computed text.
    pub value: String,
    /// The color it resolves to, when it is a color.
    pub color: Option<String>,
}

/// The answer to the plan.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Extracted {
    /// The red channel of the beacon colors.
    pub beacon: u8,
    /// The token candidates.
    pub tokens: Vec<TokenOut>,
    /// The cells.
    pub cells: Vec<CellOut>,
}

/// A string table for the ids inside records and sets (`T31` re-interns them in the builder).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Strs {
    /// The strings, in order of first use.
    pub list: Vec<String>,
    map: BTreeMap<String, StrId>,
}

impl Strs {
    /// The id of a string.
    pub fn id(&mut self, s: &str) -> StrId {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = StrId(self.list.len() as u32);
        self.list.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }

    /// The string of an id.
    pub fn get(&self, id: StrId) -> &str {
        self.list.get(id.index()).map_or("", String::as_str)
    }
}

/// A node of the box tree the templates are built from.
#[derive(Debug, Clone, PartialEq)]
pub enum BoxNode {
    /// An element with its style set.
    Elem {
        /// The IR id.
        id: u32,
        /// The index into [`Compiled::sets`].
        set: usize,
        /// The children, in layout order.
        children: Vec<BoxNode>,
    },
    /// An anonymous box around text runs.
    Anon {
        /// What kind.
        kind: AnonKind,
        /// The text and inline elements inside.
        children: Vec<BoxNode>,
    },
    /// A text leaf (styled by its parent).
    Text {
        /// The IR id.
        id: u32,
    },
}

/// The compiled styles of a skin.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compiled {
    /// Width breakpoints.
    pub media_w: Vec<Bound>,
    /// Height breakpoints.
    pub media_h: Vec<Bound>,
    /// The strings the records, sets and tokens refer to.
    pub strs: Strs,
    /// Root color tokens, synonyms (themes come later).
    pub tokens: Tokens,
    /// Distinct records, first use.
    pub records: Vec<StyleRecord>,
    /// Distinct sets, first use.
    pub sets: Vec<StyleSet>,
    /// The set of every element of the box tree, by IR id.
    pub node_set: BTreeMap<u32, usize>,
    /// Elements with `display: contents` (replaced by their children).
    pub contents: BTreeSet<u32>,
    /// The box tree: one entry per layer root, in document order.
    pub tree: Vec<BoxNode>,
}

impl Compiled {
    /// The set of an element.
    pub fn set_of(&self, id: u32) -> Option<&StyleSet> {
        self.node_set.get(&id).and_then(|&i| self.sets.get(i))
    }

    /// The record of an element in a cell and mask.
    pub fn record_of(&self, id: u32, cell: u8, mask: u8) -> Option<&StyleRecord> {
        let set = self.set_of(id)?;
        self.records.get(set.get(cell, mask).index())
    }

    /// The number of records over all sets (variants, counted per set entry).
    pub fn variant_count(&self, id: u32) -> usize {
        self.set_of(id).map_or(0, |s| {
            let mut v: Vec<StyleId> = s.table.clone();
            v.sort();
            v.dedup();
            v.len()
        })
    }
}

fn err_node(dom: &Dom, ix: Ix, code: &str, prop: &str, why: String) -> ConvError {
    let e = dom.elems[ix];
    ConvError::new(code, e.place(), why)
        .element(e.describe())
        .prop(prop)
}

/// The values of one (cell, mask) read: base with the variant's overlay.
fn values(n: &NodeOut, mask: u32) -> Vec<String> {
    let mut v = n.base.clone();
    if mask > 0
        && let Some(var) = n.vars.iter().find(|x| x.mask == mask)
    {
        for (i, s) in &var.set {
            if let Some(slot) = v.get_mut(*i) {
                *slot = s.clone();
            }
        }
    }
    v
}

fn beacon_values(n: &NodeOut, mask: u32) -> BTreeMap<usize, String> {
    let set = if mask == 0 {
        Some(&n.beacon.base)
    } else {
        n.beacon
            .vars
            .iter()
            .find(|x| x.mask == mask)
            .map(|v| &v.set)
    };
    set.map(|s| s.iter().cloned().collect()).unwrap_or_default()
}

struct Beacon {
    r: u8,
    count: usize,
}

impl Beacon {
    fn token(&self, c: &ColorV) -> Option<TokenId> {
        match c {
            ColorV::Lit([r, g, b, 255]) if *r == self.r => {
                let i = (usize::from(*g) << 8) | usize::from(*b);
                (i < self.count).then_some(TokenId(i as u32))
            }
            _ => None,
        }
    }

    /// The value with the colors that came from tokens replaced by references.
    fn apply(&self, v: Value, under: &Value) -> Value {
        match (v, under) {
            (Value::Color(c), Value::Color(b)) => match self.token(b) {
                Some(t) => Value::Color(ColorV::Tok(t)),
                None => Value::Color(c),
            },
            (Value::Shadows(mut list), Value::Shadows(blist)) if list.len() == blist.len() => {
                for (s, b) in list.iter_mut().zip(blist) {
                    if let Some(t) = self.token(&b.color) {
                        s.color = ColorV::Tok(t);
                    }
                }
                Value::Shadows(list)
            }
            (Value::Images(mut list), Value::Images(blist)) if list.len() == blist.len() => {
                for (img, b) in list.iter_mut().zip(blist) {
                    let (BgLayer::Gradient(g), BgLayer::Gradient(bg)) = (img, b) else {
                        continue;
                    };
                    if g.stops.len() != bg.stops.len() {
                        continue;
                    }
                    for (s, bs) in g.stops.iter_mut().zip(&bg.stops) {
                        if let Some(t) = self.token(&bs.color) {
                            s.color = ColorV::Tok(t);
                        }
                    }
                }
                Value::Images(list)
            }
            (v, _) => v,
        }
    }
}

/// What reading one node produced in one (cell, mask).
struct Read {
    record: StyleRecord,
    contents: bool,
}

struct Cx<'a> {
    dom: &'a Dom<'a>,
    beacon: Beacon,
    strs: Strs,
    errs: Vec<ConvError>,
    static_props: &'a BTreeSet<String>,
}

const TRANSITIONS: [PropId; 4] = [
    PropId::TransitionProperty,
    PropId::TransitionDuration,
    PropId::TransitionDelay,
    PropId::TransitionTimingFunction,
];

const ICON_PAINT: [PropId; 5] = [
    PropId::Fill,
    PropId::Stroke,
    PropId::StrokeWidth,
    PropId::StrokeLinecap,
    PropId::StrokeLinejoin,
];

impl Cx<'_> {
    fn read(&mut self, ix: Ix, vals: &[String], beacon: &BTreeMap<usize, String>) -> Read {
        let e = self.dom.elems[ix];
        let mut rec = StyleRecord::initial();
        let contents = vals
            .get(PropId::Display.index())
            .is_some_and(|d| d == "contents");
        let mut trans: [Option<Value>; 4] = [None, None, None, None];
        for (i, &p) in PropId::ALL.iter().enumerate() {
            let Some(s) = vals.get(i) else { continue };
            if contents && p == PropId::Display {
                continue;
            }
            if self.static_props.contains(p.name()) {
                // Already reported where it is written.
                continue;
            }
            if ICON_PAINT.contains(&p) && !e.is_svg() {
                if s.trim() != Profile::v1().def(p).initial {
                    self.errs.push(err_node(
                        self.dom,
                        ix,
                        codes::PAINT,
                        p.name(),
                        format!("`{}: {s}` computed by Chrome", p.name()),
                    ));
                }
                continue;
            }
            let v = match parse_value(p, s) {
                Ok(v) => v,
                Err(ve) => {
                    self.errs.push(err_node(
                        self.dom,
                        ix,
                        ve.code,
                        p.name(),
                        format!("`{}: {s}` computed by Chrome: {}", p.name(), ve.message),
                    ));
                    continue;
                }
            };
            let v = match beacon.get(&i).filter(|_| token_capable(p)) {
                Some(bs) => match parse_value(p, bs) {
                    Ok(bv) => self.beacon.apply(v, &bv),
                    Err(_) => v,
                },
                None => v,
            };
            let v = match skin_pictures(v) {
                Ok(v) => v,
                Err(url) => {
                    self.errs.push(err_node(
                        self.dom,
                        ix,
                        codes::RESOURCE,
                        p.name(),
                        format!(
                            "`{}: {s}` computed by Chrome: `{url}` is not a file of the skin",
                            p.name()
                        ),
                    ));
                    continue;
                }
            };
            if let Some(k) = TRANSITIONS.iter().position(|&t| t == p) {
                trans[k] = Some(v);
                continue;
            }
            let strs = &mut self.strs;
            if rec.set_value(p, &v, &mut |s| strs.id(s)).is_err() {
                self.errs.push(err_node(
                    self.dom,
                    ix,
                    codes::STORE,
                    p.name(),
                    format!("`{}: {s}`", p.name()),
                ));
            }
        }
        if let [
            Some(Value::Properties(ps)),
            Some(Value::Times(d)),
            Some(Value::Times(dl)),
            Some(Value::Easings(es)),
        ] = &trans
        {
            rec.transitions =
                ui2_package::profile::apply::compose_transitions(Profile::v1(), ps, d, dl, es);
        }
        Read {
            record: rec,
            contents,
        }
    }
}

/// A value with every picture URL (Chrome's absolute URL on the local server) replaced by the path of the
/// picture in the skin, the name the package's `IMGS` table gives it. `Err` carries a URL outside the skin.
pub fn skin_pictures(v: Value) -> Result<Value, String> {
    let local = |u: String| crate::skin::server_path(&u).ok_or(u);
    Ok(match v {
        Value::Url(u) => Value::Url(local(u)?),
        Value::Images(list) => Value::Images(
            list.into_iter()
                .map(|l| match l {
                    BgLayer::Url(u) => local(u).map(BgLayer::Url),
                    g => Ok(g),
                })
                .collect::<Result<_, _>>()?,
        ),
        v => v,
    })
}

fn describe_cond(c: &crate::plan::CondSpec) -> String {
    match &c.kind {
        CondK::Pseudo(p) => format!(":{}", p.name()),
        CondK::Class(k) => format!(".{k}"),
    }
}

/// Compiles the answer. `static_props` are properties already reported as written in a
/// stylesheet (their computed values are not reported again).
pub fn compile(
    dom: &Dom,
    plan: &Plan,
    out: &Extracted,
    static_props: &BTreeSet<String>,
) -> (Compiled, Vec<ConvError>) {
    let color_tokens: Vec<&TokenOut> = out.tokens.iter().filter(|t| t.color.is_some()).collect();
    let mut cx = Cx {
        dom,
        beacon: Beacon {
            r: out.beacon,
            count: color_tokens.len(),
        },
        strs: Strs::default(),
        errs: Vec::new(),
        static_props,
    };
    let mut c = Compiled {
        media_w: plan.media_w.clone(),
        media_h: plan.media_h.clone(),
        ..Compiled::default()
    };

    // Tokens: the color roots, then the synonyms that point at one of them.
    for t in &color_tokens {
        let rgba = match t.color.as_deref().map(|s| parse_value(PropId::Color, s)) {
            Some(Ok(Value::Color(ColorV::Lit(rgba)))) => rgba,
            _ => [0, 0, 0, 0],
        };
        let name = cx.strs.id(&t.name);
        c.tokens.roots.push((name, rgba));
    }
    for (alias, target) in &plan.aliases {
        if let Some(i) = color_tokens.iter().position(|t| &t.name == target) {
            let name = cx.strs.id(alias);
            c.tokens.aliases.push((name, TokenId(i as u32)));
        }
    }
    for (name, def) in Profile::v1().tokens() {
        if def.required
            && plan.aliases.iter().any(|(a, _)| a == name)
            && !plan
                .aliases
                .iter()
                .any(|(a, t)| a == name && color_tokens.iter().any(|ct| &ct.name == t))
        {
            c_err_token(&mut cx.errs, name);
        }
    }

    let ncells = plan.cells.len();
    let mut record_ids: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    let mut set_ids: BTreeMap<Vec<u8>, usize> = BTreeMap::new();
    for (ni, pn) in plan.nodes.iter().enumerate() {
        let reads: Vec<&NodeOut> = out
            .cells
            .iter()
            .filter_map(|cell| cell.nodes.get(ni).filter(|n| n.id == pn.id))
            .collect();
        if reads.len() != ncells {
            cx.errs.push(ConvError::new(
                codes::PROTOCOL,
                Place::default(),
                format!("the extraction has no answer for node {}", pn.id),
            ));
            continue;
        }
        if pn.page {
            page_checks(&mut cx, pn, &reads);
            continue;
        }
        element_checks(&mut cx, plan, pn, &reads);
        let n = pn.conds.len();
        // table[cell][mask]
        let mut table: Vec<Vec<Read>> = Vec::new();
        for r in &reads {
            let mut row = Vec::new();
            for mask in 0..(1u32 << n) {
                let vals = values(r, mask);
                let b = beacon_values(r, mask);
                row.push(cx.read(pn.ix, &vals, &b));
            }
            table.push(row);
        }
        consistency(&mut cx, pn, &table);
        clip_at_size(&mut cx, pn, &plan.cells, &reads, &table);
        // Drop the conditions that change no record.
        let mut keep: Vec<usize> = Vec::new();
        for bit in 0..n {
            let matters = table.iter().any(|row| {
                (0..(1usize << n))
                    .filter(|m| m & (1 << bit) != 0)
                    .any(|m| row[m].record != row[m & !(1 << bit)].record)
            });
            if matters {
                keep.push(bit);
            }
        }
        let mut conds = Vec::new();
        for &bit in &keep {
            let cs = &pn.conds[bit];
            let kind = match &cs.kind {
                CondK::Pseudo(Pseudo::Hover) => CondKind::Hover,
                CondK::Pseudo(Pseudo::Active) => CondKind::Active,
                CondK::Pseudo(Pseudo::Focus) => CondKind::Focus,
                CondK::Pseudo(Pseudo::FocusVisible) => CondKind::FocusVisible,
                CondK::Pseudo(Pseudo::Disabled) => CondKind::Disabled,
                CondK::Class(k) => CondKind::Class(cx.strs.id(k)),
            };
            conds.push(Cond { up: cs.up, kind });
        }
        let mut ids = Vec::new();
        for row in &table {
            for m in 0..(1usize << keep.len()) {
                let mut full = 0usize;
                for (j, &bit) in keep.iter().enumerate() {
                    if m & (1 << j) != 0 {
                        full |= 1 << bit;
                    }
                }
                let rec = &row[full].record;
                let key = postcard::to_allocvec(rec).unwrap_or_default();
                let id = *record_ids.entry(key).or_insert_with(|| {
                    c.records.push(rec.clone());
                    c.records.len() - 1
                });
                ids.push(StyleId(id as u32));
            }
        }
        let set = StyleSet { conds, table: ids };
        let key = postcard::to_allocvec(&set).unwrap_or_default();
        let si = *set_ids.entry(key).or_insert_with(|| {
            c.sets.push(set.clone());
            c.sets.len() - 1
        });
        c.node_set.insert(pn.id, si);
        if table.iter().all(|row| row[0].contents) {
            c.contents.insert(pn.id);
        }
    }
    c.tree = box_tree(dom, &c, plan);
    c.strs = cx.strs;
    (c, cx.errs)
}

fn c_err_token(errs: &mut Vec<ConvError>, name: &str) {
    errs.push(
        ConvError::new(
            codes::TOKEN_MISSING,
            Place::default(),
            format!("`{name}` does not name a color token of :root"),
        )
        .prop(name.to_string()),
    );
}

/// `html` and `body`: transparent.
fn page_checks(cx: &mut Cx, pn: &PlanNode, reads: &[&NodeOut]) {
    let bg = PropId::BackgroundColor.index();
    for r in reads {
        let color = r.base.get(bg).map(String::as_str).unwrap_or("");
        let image = r
            .off
            .iter()
            .find(|o| o.0 == "background-image")
            .map(|o| o.1.as_str());
        if color != "rgba(0, 0, 0, 0)" || image.is_some_and(|i| i != "none") {
            let what = image.map_or(format!("background-color: {color}"), |i| {
                format!("background-color: {color}; background-image: {i}")
            });
            cx.errs.push(err_node(
                cx.dom,
                pn.ix,
                codes::PAGE_BG,
                "background",
                format!("`{what}` computed by Chrome"),
            ));
        }
    }
}

/// Off-profile values, required values and layer roots, from the base reads.
fn element_checks(cx: &mut Cx, plan: &Plan, pn: &PlanNode, reads: &[&NodeOut]) {
    let e: &Element = cx.dom.elems[pn.ix];
    let tag = Tag::from_name(&e.tag);
    for r in reads {
        let color = r
            .base
            .get(PropId::Color.index())
            .cloned()
            .unwrap_or_default();
        for (name, value, initial) in &r.off {
            // Logical longhands mirror the physical ones of the profile.
            let logical = name.contains("block")
                || name.contains("inline")
                || name.starts_with("border-start-")
                || name.starts_with("border-end-");
            let required = plan.extra.contains(name)
                || name.starts_with("list-style")
                || name == "appearance"
                || name == "-webkit-appearance";
            let current = *value == color;
            // Chrome also lists a few shorthands and used values: the longhands are checked
            // themselves (`-webkit-border-image` mirrors the `border-image-*` longhands), and the
            // perspective origin matters only with a perspective.
            let skipped = matches!(
                name.as_str(),
                "-webkit-locale" | "perspective-origin" | "-webkit-border-image"
            ) || crate::check::SHORTHANDS.iter().any(|(s, _)| s == name);
            if logical
                || required
                || current
                || skipped
                || Profile::v1().ignored(name).is_some()
                || cx.static_props.contains(name)
            {
                continue;
            }
            cx.errs.push(err_node(
                cx.dom,
                pn.ix,
                codes::PROP,
                name,
                format!("`{name}: {value}` computed by Chrome (initial `{initial}`)"),
            ));
        }
        if let Some(def) = tag.and_then(|t| Profile::v1().element(t)) {
            for (k, want) in &def.require {
                let name = if k == "list-style" {
                    "list-style-type"
                } else {
                    k.as_str()
                };
                let Some(i) = plan.extra.iter().position(|x| x == name) else {
                    continue;
                };
                let got = r.extra.get(i).map(String::as_str).unwrap_or("");
                if got != want {
                    cx.errs.push(err_node(
                        cx.dom,
                        pn.ix,
                        codes::REQUIRE,
                        k,
                        format!("`{name}: {got}`; the profile requires `{k}: {want}`"),
                    ));
                }
            }
        }
        if e.attr("data-layer").is_some() && e.tag != "template" {
            let v = |p: PropId| r.base.get(p.index()).map(String::as_str).unwrap_or("");
            let ok = v(PropId::Position) == "absolute"
                && [PropId::Top, PropId::Right, PropId::Bottom, PropId::Left]
                    .iter()
                    .all(|&p| v(p) == "0px")
                && v(PropId::PointerEvents) == "none";
            if !ok {
                cx.errs.push(err_node(
                    cx.dom,
                    pn.ix,
                    codes::LAYER_ROOT,
                    "",
                    format!(
                        "position: {}; top/right/bottom/left: {} {} {} {}; pointer-events: {}",
                        v(PropId::Position),
                        v(PropId::Top),
                        v(PropId::Right),
                        v(PropId::Bottom),
                        v(PropId::Left),
                        v(PropId::PointerEvents)
                    ),
                ));
            }
        }
    }
}

/// Transitions, `display: contents` and `order` hold in every variant (E3-007..009).
fn consistency(cx: &mut Cx, pn: &PlanNode, table: &[Vec<Read>]) {
    let first = &table[0][0];
    let conds: Vec<String> = pn.conds.iter().map(describe_cond).collect();
    let mut trans_bad = false;
    let mut contents_bad = false;
    let mut order_bad = false;
    for row in table {
        for r in row {
            contents_bad |= r.contents != first.contents;
            order_bad |= r.record.order != first.record.order;
        }
        for r in row.iter().skip(1) {
            trans_bad |= r.record.transitions != row[0].record.transitions;
        }
    }
    let list = if conds.is_empty() {
        "breakpoints".to_string()
    } else {
        format!("{} or breakpoints", conds.join(", "))
    };
    for (bad, code, prop) in [
        (trans_bad, codes::TRANSITIONS, "transition"),
        (contents_bad, codes::CONTENTS, "display"),
        (order_bad, codes::ORDER, "order"),
    ] {
        if bad {
            cx.errs.push(err_node(
                cx.dom,
                pn.ix,
                code,
                prop,
                format!("changes with {list}"),
            ));
        }
    }
}

/// A clip polygon passed the value parser's convexity check in its reference boxes; one that mixes px and %
/// can still turn concave at the size the node really has. Checked in every cell at the node's border box
/// (E2-022), for every variant.
fn clip_at_size(
    cx: &mut Cx,
    pn: &PlanNode,
    cells: &[[u32; 2]],
    reads: &[&NodeOut],
    table: &[Vec<Read>],
) {
    for ((r, row), view) in reads.iter().zip(table).zip(cells) {
        let Some(size) = r.size else { continue };
        let bad = row.iter().find(|x| {
            !x.record.clip_path.is_empty()
                && !ui2_package::style::convex_at(&x.record.clip_path, size)
        });
        if bad.is_some() {
            cx.errs.push(err_node(
                cx.dom,
                pn.ix,
                ui2_package::profile::values::codes::CONCAVE,
                PropId::ClipPath.name(),
                format!(
                    "the clip polygon is not convex at the element's size {} x {} px (viewport {} x {})",
                    size[0],
                    size[1],
                    view[0],
                    view[1]
                ),
            ));
            return;
        }
    }
}

fn flex_or_grid(c: &Compiled, id: u32) -> bool {
    c.record_of(id, 0, 0)
        .is_some_and(|r| matches!(r.display, Display::Flex | Display::Grid))
}

fn order_of(c: &Compiled, n: &BoxNode) -> i32 {
    match n {
        BoxNode::Elem { id, .. } => c.record_of(*id, 0, 0).map_or(0, |r| r.order),
        _ => 0,
    }
}

/// The children of an element as boxes: anonymous runs grouped, `display: contents` flattened,
/// whitespace in flex and grid containers dropped, flex and grid children sorted by `order`.
fn children(dom: &Dom, c: &Compiled, e: &Element, seen: &mut BTreeSet<String>) -> Vec<BoxNode> {
    let container = flex_or_grid(c, e.id);
    let mut out: Vec<BoxNode> = Vec::new();
    let mut run: Option<(crate::ir::Anon, Vec<BoxNode>)> = None;
    let flush = |out: &mut Vec<BoxNode>, run: &mut Option<(crate::ir::Anon, Vec<BoxNode>)>| {
        if let Some((a, kids)) = run.take() {
            out.push(BoxNode::Anon {
                kind: a.kind,
                children: kids,
            });
        }
    };
    for k in &e.children {
        let (anon, nodes): (Option<crate::ir::Anon>, Vec<BoxNode>) = match k {
            Node::Text(t) => {
                if t.ws && (container || t.anon.is_none()) && !paragraph(e) {
                    continue;
                }
                (t.anon, vec![BoxNode::Text { id: t.id }])
            }
            Node::Element(k) => {
                let Some(&kx) = dom.by_id.get(&k.id) else {
                    continue;
                };
                if dom.is_dev(kx) {
                    continue;
                }
                if let Some(m) = &k.mark
                    && !seen.insert(m.clone())
                {
                    continue;
                }
                let Some(&set) = c.node_set.get(&k.id) else {
                    continue;
                };
                if c.contents.contains(&k.id) {
                    (k.anon, children(dom, c, k, seen))
                } else {
                    let kids = if k.is_svg() {
                        Vec::new()
                    } else {
                        children(dom, c, k, seen)
                    };
                    (
                        k.anon,
                        vec![BoxNode::Elem {
                            id: k.id,
                            set,
                            children: kids,
                        }],
                    )
                }
            }
        };
        match (anon, &mut run) {
            (Some(a), Some((cur, kids))) if *cur == a => kids.extend(nodes),
            (Some(a), _) => {
                flush(&mut out, &mut run);
                run = Some((a, nodes));
            }
            (None, _) => {
                flush(&mut out, &mut run);
                out.extend(nodes);
            }
        }
    }
    flush(&mut out, &mut run);
    if container {
        out.sort_by_key(|n| order_of(c, n));
    }
    out
}

/// A block whose children include text: spaces between inline parts matter there.
fn paragraph(e: &Element) -> bool {
    e.children
        .iter()
        .any(|k| matches!(k, Node::Text(t) if !t.ws))
}

fn box_tree(dom: &Dom, c: &Compiled, _plan: &Plan) -> Vec<BoxNode> {
    let Some(body) = dom.body() else {
        return Vec::new();
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for k in dom.elems[body].elements() {
        if k.attr("data-layer").is_none() || k.attr("data-ui2-dev").is_some() {
            continue;
        }
        if let Some(&set) = c.node_set.get(&k.id) {
            out.push(BoxNode::Elem {
                id: k.id,
                set,
                children: children(dom, c, k, &mut seen),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_picture_urls_become_skin_paths() {
        let v = parse_value(
            PropId::BackgroundImage,
            "url(\"http://127.0.0.1:61234/images/frame.png\"), linear-gradient(rgb(0, 0, 0), rgb(255, 255, 255))",
        )
        .expect("parses");
        let Ok(Value::Images(list)) = skin_pictures(v) else {
            panic!("images")
        };
        assert!(matches!(&list[0], BgLayer::Url(p) if p == "images/frame.png"));
        assert!(matches!(&list[1], BgLayer::Gradient(_)));
        let v = parse_value(
            PropId::BorderImageSource,
            "url(\"http://127.0.0.1:61234/images/nine.png\")",
        )
        .expect("parses");
        assert!(matches!(skin_pictures(v), Ok(Value::Url(p)) if p == "images/nine.png"));
        let v = parse_value(
            PropId::BorderImageSource,
            "url(\"http://example.com/nine.png\")",
        )
        .expect("parses");
        assert_eq!(
            skin_pictures(v).err().as_deref(),
            Some("http://example.com/nine.png")
        );
    }
}
