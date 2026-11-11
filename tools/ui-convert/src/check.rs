//! Static checks on the scanned page: document and elements (E1), attributes (the profile's
//! grammars: E1, E4, E5, E6, E7), untranslated text (E6), layers, slots, parts, hotkeys, tokens
//! (E7), and the stylesheets as written (E2 for units and properties, E3 for selectors and
//! at-rules). Nothing here needs Chrome beyond the scan.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ui2_package::profile::attrs::{self, AttrCtx, NodeAttrs, RawAttrs};
use ui2_package::profile::media::{self, Cells, MediaCond};
use ui2_package::profile::select::{self, SelInfo, SelectError};
use ui2_package::profile::values::codes as value_codes;
use ui2_package::profile::{Profile, PropId, Tag, ValueKind};
use ui2_package::sections::templates::Layer;

use crate::errors::{ConvError, Place, codes};
use crate::ir::{CssItem, Dom, Element, Ir, Ix, Node};
use crate::skin::{Skin, is_skin_path, resolve_in_skin, url_args};

/// One style rule of the skin's stylesheets, with its selectors checked against the profile.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The file and position of the rule.
    pub place: Place,
    /// The selector list as written.
    pub text: String,
    /// The selectors of the profile, split into their static part and conditions (the others were
    /// reported).
    pub selectors: Vec<SelInfo>,
    /// The declared properties (as written, lower case).
    pub props: Vec<String>,
    /// The media conditions the rule sits in.
    pub media: Vec<MediaCond>,
}

/// A profile error of a selector, a media query or a node's conditions as a converter error.
fn select_error(e: SelectError, place: Place) -> ConvError {
    ConvError::new(e.code, place, e.message)
}

/// A custom property declared on `:root`.
#[derive(Debug, Clone, PartialEq)]
pub struct RootVar {
    /// `--name`.
    pub name: String,
    /// The value as written.
    pub value: String,
    /// Where.
    pub place: Place,
}

/// What the static pass learned about the page.
#[derive(Debug, Clone, Default)]
pub struct Statics {
    /// The parsed attributes of every element of the skin tree that parsed.
    pub attrs: BTreeMap<Ix, NodeAttrs>,
    /// The elements of the skin tree (body descendants that are not development-only and not
    /// inside an svg), document order.
    pub ui: Vec<Ix>,
    /// Classes that bindings or scripts switch, with the elements that switch them.
    pub dyn_classes: BTreeMap<String, BTreeSet<Ix>>,
    /// The style rules, in cascade-independent document order.
    pub rules: Vec<Rule>,
    /// The viewport cells the media conditions of every rule make.
    pub cells: Cells,
    /// Custom properties of `:root` rules, in order.
    pub root_vars: Vec<RootVar>,
}

/// Properties a stylesheet may name although the style record has no field for them: they
/// expand to profile properties (Chrome bakes the shorthand) or are required values of elements.
pub const SHORTHANDS: &[(&str, &[&str])] = &[
    ("margin", &["margin-top"]),
    ("padding", &["padding-top"]),
    ("inset", &["top"]),
    ("flex", &["flex-grow"]),
    ("flex-flow", &["flex-direction"]),
    ("gap", &["row-gap"]),
    ("grid-gap", &["row-gap"]),
    ("place-items", &["align-items"]),
    ("place-content", &["align-content"]),
    ("place-self", &["align-self"]),
    ("border", &["border-top-width"]),
    ("border-top", &["border-top-width"]),
    ("border-right", &["border-right-width"]),
    ("border-bottom", &["border-bottom-width"]),
    ("border-left", &["border-left-width"]),
    ("border-width", &["border-top-width"]),
    ("border-style", &["border-top-style"]),
    ("border-color", &["border-top-color"]),
    ("border-radius", &["border-top-left-radius"]),
    ("background", &["background-color"]),
    ("background-position", &["background-position-x"]),
    ("border-image", &["border-image-source"]),
    ("grid-template", &["grid-template-rows"]),
    ("grid-row", &["grid-row-start"]),
    ("grid-column", &["grid-column-start"]),
    ("grid-area", &["grid-row-start"]),
    ("outline", &["outline-style"]),
    ("font", &["font-size"]),
    ("font-variant", &["font-variant-numeric"]),
    ("overflow", &["overflow-x"]),
    ("transition", &["transition-property"]),
    ("white-space", &["white-space-collapse"]),
    ("text-wrap", &["text-wrap-mode"]),
    ("text-decoration", &["text-decoration-line"]),
    ("word-wrap", &["overflow-wrap"]),
];

/// The properties of `require` rows of the element table and their longhands.
fn required_props() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for &tag in Tag::ALL {
        if let Some(def) = Profile::v1().element(tag) {
            for k in def.require.keys() {
                out.insert(k.clone());
                if k == "list-style" {
                    for l in ["list-style-type", "list-style-position", "list-style-image"] {
                        out.insert(l.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Whether a declaration's property may appear in a stylesheet of the skin.
pub fn known_property(name: &str) -> bool {
    name.starts_with("--")
        || PropId::from_name(name).is_some()
        || Profile::v1().ignored(name).is_some()
        || SHORTHANDS.iter().any(|(s, _)| *s == name)
        || required_props().contains(name)
}

const VIEWPORT_UNITS: &[&str] = &[
    "vw", "vh", "vmin", "vmax", "vi", "vb", "dvw", "dvh", "svw", "svh", "lvw", "lvh", "dvmin",
    "dvmax", "svmin", "svmax", "lvmin", "lvmax", "cqw", "cqh", "cqi", "cqb", "cqmin", "cqmax",
];

/// The first viewport or container unit in a value, if any.
pub fn viewport_unit(value: &str) -> Option<String> {
    let b = value.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() || b[i] == b'.' {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            let mut k = j;
            while k < b.len() && b[k].is_ascii_alphabetic() {
                k += 1;
            }
            let unit = value[j..k].to_ascii_lowercase();
            if VIEWPORT_UNITS.contains(&unit.as_str()) {
                return Some(format!("{}{}", &value[i..j], unit));
            }
            i = k.max(i + 1);
        } else if b[i].is_ascii_alphabetic() || b[i] == b'-' || b[i] == b'_' {
            // Skip identifiers (`h1`, `--x2vw`) whole.
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'_') {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    None
}

const NEUTRAL: &str = "0123456789+\u{2212}-\u{2013}\u{2014}\u{00b7}\u{00d7}%/:.,()";

/// Text that needs no translation: digits, spaces and a few signs.
pub fn is_neutral(text: &str) -> bool {
    text.chars()
        .all(|c| c.is_whitespace() || c == '\u{a0}' || NEUTRAL.contains(c))
}

/// SVG presentation and structure attributes the svg element of an icon or picture may carry;
/// they are not profile attributes and are dropped before the attribute grammar runs.
const SVG_ATTRS: &[&str] = &[
    "xmlns",
    "xmlns:xlink",
    "viewbox",
    "preserveaspectratio",
    "version",
    "focusable",
];

struct Ctx<'a> {
    skin_dir: &'a Path,
    dom: &'a Dom<'a>,
    errs: Vec<ConvError>,
    out: Statics,
}

impl Ctx<'_> {
    fn err_at(&mut self, ix: Ix, code: &str, prop: &str, why: impl Into<String>) {
        let e = self.dom.elems[ix];
        self.errs.push(
            ConvError::new(code, e.place(), why)
                .element(e.describe())
                .prop(prop),
        );
    }

    fn file_exists(&self, rel: &str) -> bool {
        is_skin_path(rel) && self.skin_dir.join(rel).is_file()
    }
}

/// Runs every static check. `skin_dir` is read for includes and scripts.
pub fn check(skin: &Skin, ir: &Ir, skin_dir: &Path) -> (Statics, Vec<ConvError>) {
    let dom = Dom::new(&ir.dom);
    let mut cx = Ctx {
        skin_dir,
        dom: &dom,
        errs: Vec::new(),
        out: Statics::default(),
    };
    for e in &ir.errors {
        let why = match &e.url {
            Some(url) => format!("{}: {url}", e.message),
            None => e.message.clone(),
        };
        cx.errs
            .push(ConvError::new(&e.code, Place::file(&ir.entry), why));
    }
    elements(&mut cx);
    layers(&mut cx);
    registry(&mut cx);
    texts(&mut cx);
    sheets(&mut cx, ir);
    scripts(&mut cx, skin);
    let Ctx { errs, out, .. } = cx;
    (out, errs)
}

fn element_tag(e: &Element) -> Option<Tag> {
    Tag::from_name(&e.tag)
}

/// The attribute context of an element: loop, layer, island and children.
fn attr_ctx(dom: &Dom, ix: Ix) -> AttrCtx {
    let e = dom.elems[ix];
    let mut ctx = AttrCtx {
        has_children: e.elements().next().is_some(),
        ..AttrCtx::default()
    };
    for a in dom.ancestors(ix) {
        let ae = dom.elems[a];
        if ae.attr("data-bind-for").is_some() {
            ctx.in_for = true;
        }
        if ae.attr("data-ui2-island").is_some() {
            ctx.in_island = true;
        }
    }
    for a in std::iter::once(ix).chain(dom.ancestors(ix)) {
        let layer = match dom.elems[a].attr("data-layer") {
            Some("map") => Some(Layer::Map),
            Some("hud") => Some(Layer::Hud),
            Some("window") => Some(Layer::Window),
            Some("tooltip") => Some(Layer::Tooltip),
            _ => None,
        };
        if layer.is_some() {
            ctx.layer = layer;
            break;
        }
    }
    ctx
}

fn elements(cx: &mut Ctx) {
    let dom = cx.dom;
    let body = dom.body();
    for ix in 0..dom.elems.len() {
        let e = dom.elems[ix];
        if dom.is_dev(ix) {
            continue;
        }
        // Inside an svg: only the references and the forbidden content matter.
        if let Some(svg_root) = dom
            .ancestors(ix)
            .find(|&a| dom.elems[a].is_svg() && dom.elems[a].tag == "svg")
        {
            let _ = svg_root;
            match e.tag.as_str() {
                "text" | "script" | "foreignObject" | "foreignobject" => {
                    cx.err_at(ix, codes::SVG, "", format!("`<{}>` inside an svg", e.tag))
                }
                "use" => {
                    let href = e
                        .attr("href")
                        .or_else(|| e.attr("xlink:href"))
                        .unwrap_or("");
                    if !href.starts_with('#') || href.len() < 2 {
                        cx.err_at(
                            ix,
                            codes::SVG_REF,
                            "href",
                            format!("`{href}` is not a symbol of the skin"),
                        );
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some(tag) = element_tag(e) else {
            cx.err_at(
                ix,
                codes::ELEMENT,
                "",
                format!("`<{}>` is not an element of the profile", e.tag),
            );
            continue;
        };
        let in_body = body.is_some_and(|b| dom.ancestors(ix).any(|a| a == b));
        if in_body {
            cx.out.ui.push(ix);
        }
        resources(cx, ix, tag);
        let attrs_in: Vec<(String, String)> = e
            .attrs
            .iter()
            .filter(|(n, _)| {
                !(tag == Tag::Svg && SVG_ATTRS.contains(&n.to_ascii_lowercase().as_str()))
            })
            .cloned()
            .collect();
        let raw = RawAttrs {
            tag,
            attrs: attrs_in,
        };
        let ctx = attr_ctx(dom, ix);
        match attrs::parse(&raw, &ctx) {
            Ok(parsed) => {
                for t in &parsed.class_toggle {
                    cx.out
                        .dyn_classes
                        .entry(t.class.clone())
                        .or_default()
                        .insert(ix);
                }
                if let Some(island) = &parsed.island {
                    for c in &island.classes {
                        cx.out.dyn_classes.entry(c.clone()).or_default().insert(ix);
                    }
                }
                if let Some(inc) = &parsed.include
                    && !cx.file_exists(inc)
                {
                    cx.err_at(
                        ix,
                        codes::INCLUDE_MISSING,
                        "data-include",
                        format!("`{inc}` is not a file of the skin"),
                    );
                }
                cx.out.attrs.insert(ix, parsed);
            }
            Err(list) => {
                for pe in list {
                    let why = match pe.at {
                        Some(at) => format!("{} (at byte {at} of the value)", pe.message),
                        None => pe.message.clone(),
                    };
                    cx.err_at(ix, pe.code, &pe.attr, why);
                }
            }
        }
    }
    // A class toggled by a binding must not be written statically on the same element.
    let mut dyn_static = Vec::new();
    for (class, owners) in &cx.out.dyn_classes {
        for &ix in owners {
            if let Err(e) = select::check_toggled(&dom.elems[ix].classes(), &[class.as_str()]) {
                dyn_static.push((ix, e));
            }
        }
    }
    for (ix, e) in dyn_static {
        cx.err_at(ix, e.code, "class", e.message);
    }
}

fn external(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("//")
        || lower.starts_with('/')
        || lower.split_once(':').is_some_and(|(scheme, _)| {
            scheme
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c))
        })
}

fn resources(cx: &mut Ctx, ix: Ix, tag: Tag) {
    let e = cx.dom.elems[ix];
    match tag {
        Tag::Script => {
            match e.attr("src") {
                None => cx.err_at(ix, codes::INLINE_SCRIPT, "", "an inline script"),
                Some(src) if external(src) => cx.err_at(
                    ix,
                    codes::RESOURCE,
                    "src",
                    format!("`{src}` is not a file of the skin"),
                ),
                Some(src) if !cx.file_exists(src) => cx.err_at(
                    ix,
                    codes::RESOURCE,
                    "src",
                    format!("`{src}` does not exist"),
                ),
                Some(_) => {}
            }
            if e.attr("type")
                .is_some_and(|t| t.eq_ignore_ascii_case("module"))
            {
                cx.err_at(ix, codes::INLINE_SCRIPT, "type", "a module script");
            }
        }
        Tag::Img => {
            if let Some(src) = e.attr("src")
                && (external(src) || !cx.file_exists(src.split(['?', '#']).next().unwrap_or("")))
            {
                cx.err_at(
                    ix,
                    codes::RESOURCE,
                    "src",
                    format!("`{src}` is not a file of the skin"),
                );
            }
        }
        _ => {}
    }
}

fn layers(cx: &mut Ctx) {
    let dom = cx.dom;
    let Some(body) = dom.body() else {
        cx.errs.push(ConvError::new(
            codes::LAYERS,
            Place::default(),
            "the page has no body",
        ));
        return;
    };
    let order = ["map", "hud", "window", "tooltip"];
    let mut seen = Vec::new();
    let b = dom.elems[body];
    for c in &b.children {
        match c {
            Node::Text(t) if !t.text.trim().is_empty() => cx.err_at(
                body,
                codes::LAYERS,
                "",
                format!("text `{}` directly in body", t.text.trim()),
            ),
            Node::Text(_) => {}
            Node::Element(k) => {
                let Some(&kx) = dom.by_id.get(&k.id) else {
                    continue;
                };
                if k.attr("data-ui2-dev").is_some()
                    || matches!(k.tag.as_str(), "script" | "template")
                {
                    continue;
                }
                match k.attr("data-layer") {
                    Some(l) if order.contains(&l) => seen.push((kx, l)),
                    _ => cx.err_at(
                        kx,
                        codes::LAYERS,
                        "",
                        "a child of body that is not a layer root",
                    ),
                }
            }
        }
    }
    let names: Vec<&str> = seen.iter().map(|(_, l)| *l).collect();
    if names != order {
        cx.err_at(
            body,
            codes::LAYERS,
            "",
            format!(
                "the layer roots are [{}]; expected map, hud, window, tooltip",
                names.join(", ")
            ),
        );
    }
    // A data-layer below the roots (other than tooltip templates) is misplaced.
    for &ix in &cx.out.ui.clone() {
        let e = dom.elems[ix];
        if e.attr("data-layer").is_some() && dom.parent[ix] != Some(body) && e.tag != "template" {
            cx.err_at(
                ix,
                codes::LAYERS,
                "data-layer",
                "a layer root is a child of body",
            );
        }
    }
}

/// The instance-independent identity of an element: its preview mark, else its id.
fn identity(e: &Element) -> String {
    e.mark.clone().unwrap_or_else(|| format!("#{}", e.id))
}

fn registry(cx: &mut Ctx) {
    let dom = cx.dom;
    let mut ids: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut parts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut hotkeys: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    let mut first: BTreeMap<String, Ix> = BTreeMap::new();
    let mut slots = BTreeSet::new();
    let mut tooltips = BTreeSet::new();
    for ix in 0..dom.elems.len() {
        if dom.is_dev(ix) {
            continue;
        }
        let e = dom.elems[ix];
        let ident = identity(e);
        if let Some(id) = e.attr("id") {
            ids.entry(id.to_string()).or_default().insert(ident.clone());
            first.entry(format!("id {id}")).or_insert(ix);
            if e.tag == "template" && e.attr("data-layer") == Some("tooltip") {
                tooltips.insert(id.to_string());
            }
        }
    }
    for ix in cx.out.ui.clone() {
        let e = dom.elems[ix];
        let ident = identity(e);
        let Some(a) = cx.out.attrs.get(&ix).cloned() else {
            continue;
        };
        if let Some(p) = &a.part {
            parts.entry(p.clone()).or_default().insert(ident.clone());
            first.entry(format!("part {p}")).or_insert(ix);
        }
        if let Some(s) = &a.slot {
            slots.insert(s.clone());
        }
        if let Some(h) = &a.hotkey {
            let layer = attr_ctx(dom, ix)
                .layer
                .map(|l| format!("{l:?}"))
                .unwrap_or_default();
            let key = format!(
                "{}{}{}{}",
                if h.ctrl { "Ctrl+" } else { "" },
                if h.alt { "Alt+" } else { "" },
                if h.shift { "Shift+" } else { "" },
                h.code
            );
            hotkeys
                .entry((layer, key.clone()))
                .or_default()
                .insert(ident.clone());
            first.entry(format!("hotkey {key}")).or_insert(ix);
        }
        if let Some(t) = &a.tooltip
            && !tooltips.contains(t)
        {
            cx.err_at(
                ix,
                codes::TOOLTIP_REF,
                "data-tooltip",
                format!("no <template data-layer=\"tooltip\" id=\"{t}\">"),
            );
        }
    }
    for (id, owners) in ids {
        if owners.len() > 1
            && let Some(&ix) = first.get(&format!("id {id}"))
        {
            cx.err_at(
                ix,
                codes::DUP_ID,
                "id",
                format!("`{id}` is used {} times", owners.len()),
            );
        }
    }
    for (p, owners) in parts {
        if owners.len() > 1
            && let Some(&ix) = first.get(&format!("part {p}"))
        {
            cx.err_at(
                ix,
                codes::PART_DUP,
                "data-part",
                format!("`{p}` names {} elements", owners.len()),
            );
        }
    }
    for ((layer, key), owners) in hotkeys {
        if owners.len() > 1
            && let Some(&ix) = first.get(&format!("hotkey {key}"))
        {
            cx.err_at(
                ix,
                codes::HOTKEY_DUP,
                "data-hotkey",
                format!(
                    "`{key}` is used {} times in the layer {layer}",
                    owners.len()
                ),
            );
        }
    }
    for (name, def) in Profile::v1().slots() {
        if def.required && !slots.contains(name) {
            cx.errs.push(
                ConvError::new(
                    codes::SLOT_MISSING,
                    Place::default(),
                    format!("no element has data-slot=\"{name}\""),
                )
                .prop("data-slot"),
            );
        }
    }
}

fn texts(cx: &mut Ctx) {
    let dom = cx.dom;
    for &ix in &cx.out.ui.clone() {
        let e = dom.elems[ix];
        if matches!(e.tag.as_str(), "style" | "script" | "title") {
            continue;
        }
        for c in &e.children {
            let Node::Text(t) = c else { continue };
            if is_neutral(&t.text) {
                continue;
            }
            let bound = std::iter::once(ix).chain(dom.ancestors(ix)).any(|a| {
                let x = dom.elems[a];
                x.attr("data-l10n").is_some() || x.attr("data-bind-value").is_some()
            });
            if !bound {
                let shown: String = t.text.trim().chars().take(40).collect();
                cx.err_at(
                    ix,
                    codes::UNTRANSLATED,
                    "",
                    format!("`{shown}` is written in the markup"),
                );
            }
        }
    }
}

fn sheets(cx: &mut Ctx, ir: &Ir) {
    let dom = cx.dom;
    for sheet in &ir.sheets {
        let Some(&ix) = dom.by_id.get(&sheet.node) else {
            continue;
        };
        if dom.is_dev(ix) {
            continue;
        }
        if sheet.external {
            cx.err_at(
                ix,
                codes::RESOURCE,
                "href",
                format!(
                    "`{}` is outside the skin",
                    sheet.href.clone().unwrap_or_default()
                ),
            );
            continue;
        }
        if sheet.missing {
            cx.err_at(
                ix,
                codes::RESOURCE,
                "href",
                format!(
                    "`{}` does not exist",
                    sheet.href.clone().unwrap_or_default()
                ),
            );
            continue;
        }
        let file = sheet.source_file();
        for pe in &sheet.parse_errors {
            cx.errs.push(ConvError::new(
                codes::CSS_SYNTAX,
                Place::at(&file, pe.line, pe.col),
                pe.message.clone(),
            ));
        }
        items(cx, &file, &sheet.rules, &[]);
    }
    // The viewport cells of every media condition (E3-004 for a breakpoint written with and without `=`,
    // E3-006 for too many). With an error the skin has one cell, so the later stages still run.
    let all: Vec<MediaCond> = cx
        .out
        .rules
        .iter()
        .flat_map(|r| r.media.iter().copied())
        .collect();
    match Cells::new(&all) {
        Ok(cells) => cx.out.cells = cells,
        Err(e) => cx.errs.push(select_error(e, Place::default())),
    }
    // The synonyms every skin defines for mods.
    for (name, def) in Profile::v1().tokens() {
        if def.required && !cx.out.root_vars.iter().any(|v| &v.name == name) {
            cx.errs.push(
                ConvError::new(
                    codes::TOKEN_MISSING,
                    Place::default(),
                    format!("no `:root {{ {name}: var(--...) }}`"),
                )
                .prop(name.clone()),
            );
        }
    }
}

fn items(cx: &mut Ctx, file: &str, list: &[CssItem], within: &[MediaCond]) {
    for item in list {
        match item {
            CssItem::Rule {
                line,
                col,
                selector,
                selectors,
                items: decls,
            } => {
                let place = Place::at(file, *line, *col);
                let mut parsed = Vec::new();
                for s in selectors {
                    match select::parse(s).and_then(|ast| select::check(&ast)) {
                        Ok(p) => parsed.push(p),
                        Err(e) => cx
                            .errs
                            .push(select_error(e, place.clone()).element(format!("`{s}`"))),
                    }
                }
                let is_root = parsed.len() == 1
                    && parsed[0].static_part.len() == 1
                    && parsed[0].subject().root
                    && parsed[0].conds.is_empty();
                let mut props = Vec::new();
                for d in decls {
                    if let CssItem::Decl {
                        prop,
                        value,
                        line,
                        col,
                        ..
                    } = d
                    {
                        let dplace = Place::at(file, *line, *col);
                        declaration(cx, &dplace, selector, prop, value);
                        if is_root && prop.starts_with("--") && within.is_empty() {
                            cx.out.root_vars.push(RootVar {
                                name: prop.clone(),
                                value: value.clone(),
                                place: dplace,
                            });
                        }
                        props.push(prop.to_ascii_lowercase());
                    } else if let CssItem::Rule { line, col, .. }
                    | CssItem::Atrule { line, col, .. } = d
                    {
                        cx.errs.push(
                            ConvError::new(
                                codes::AT_RULE,
                                Place::at(file, *line, *col),
                                "a nested rule",
                            )
                            .element(format!("`{selector}`")),
                        );
                    }
                }
                cx.out.rules.push(Rule {
                    place,
                    text: selector.clone(),
                    selectors: parsed,
                    props,
                    media: within.to_vec(),
                });
            }
            CssItem::Atrule {
                name,
                prelude,
                line,
                col,
                items: inner,
            } => {
                let place = Place::at(file, *line, *col);
                match name.to_ascii_lowercase().as_str() {
                    "font-face" => font_face(cx, &place, inner.as_deref().unwrap_or(&[])),
                    "media" => match media::parse(prelude) {
                        Ok(conds) => {
                            let mut all = within.to_vec();
                            all.extend(conds);
                            items(cx, file, inner.as_deref().unwrap_or(&[]), &all);
                        }
                        Err(e) => cx
                            .errs
                            .push(select_error(e, place).element(format!("`@media {prelude}`"))),
                    },
                    other => cx.errs.push(
                        ConvError::new(codes::AT_RULE, place, format!("`@{other}`"))
                            .element(format!("`@{other}`")),
                    ),
                }
            }
            CssItem::Decl { line, col, .. } => cx.errs.push(ConvError::new(
                codes::CSS_SYNTAX,
                Place::at(file, *line, *col),
                "a declaration outside a rule",
            )),
            CssItem::Other => {}
        }
    }
}

fn font_face(cx: &mut Ctx, place: &Place, decls: &[CssItem]) {
    for d in decls {
        if let CssItem::Decl {
            prop,
            value,
            line,
            col,
            ..
        } = d
            && prop == "src"
        {
            for part in value.split("url(").skip(1) {
                let url = part
                    .split(')')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_matches(['"', '\'']);
                if external(url) {
                    cx.errs.push(
                        ConvError::new(
                            codes::RESOURCE,
                            Place::at(&place.file, *line, *col),
                            format!("the font `{url}` is outside the skin"),
                        )
                        .element("`@font-face`")
                        .prop("src"),
                    );
                }
            }
        }
    }
}

/// The longhand under which a declaration that may hold a picture `url()` is reported:
/// `background-image` or `border-image-source` (their shorthands map to them).
pub fn picture_longhand(name: &str) -> Option<&'static str> {
    match name {
        "background" | "background-image" => Some("background-image"),
        "border-image" | "border-image-source" => Some("border-image-source"),
        _ => None,
    }
}

/// The `url()` pictures of a declaration as written: each must name a file of the skin, resolved against the
/// file the declaration is in (E1-003 at the declaration). `data:` URLs, queries and fragments are left to the
/// value parser (E2-023 on the computed value).
fn pictures_as_written(cx: &mut Ctx, place: &Place, subject: &str, name: &str, value: &str) {
    for url in url_args(value) {
        if url.to_ascii_lowercase().starts_with("data:") || url.contains(['?', '#']) {
            continue;
        }
        let why = match resolve_in_skin(&place.file, &url) {
            Ok(path) if cx.file_exists(&path) => continue,
            Ok(path) => format!("`url({url})` names `{path}`, which does not exist"),
            Err(why) => format!("`url({url})` {why}"),
        };
        cx.errs.push(
            ConvError::new(codes::RESOURCE, place.clone(), why)
                .element(subject.to_string())
                .prop(name.to_string()),
        );
    }
}

fn declaration(cx: &mut Ctx, place: &Place, selector: &str, prop: &str, value: &str) {
    let name = prop.to_ascii_lowercase();
    let subject = format!("`{selector}`");
    if !known_property(&name) {
        cx.errs.push(
            ConvError::new(codes::PROP, place.clone(), format!("`{name}: {value}`"))
                .element(subject)
                .prop(name),
        );
        return;
    }
    if picture_longhand(&name).is_some() {
        pictures_as_written(cx, place, &subject, &name, value);
    }
    if !name.starts_with("--")
        && let Some(unit) = viewport_unit(value)
    {
        cx.errs.push(
            ConvError::new(
                value_codes::UNIT,
                place.clone(),
                format!("`{name}: {value}` uses `{unit}`"),
            )
            .element(subject.clone())
            .prop(name.clone()),
        );
    }
    // A plain keyword of a keyword property can be checked as written. `display` values Chrome
    // may still change (blockification of flex and grid children) are left to the computed pass.
    if let Some(id) = PropId::from_name(&name) {
        let def = Profile::v1().def(id);
        let v = value.trim().to_ascii_lowercase();
        let plain = !v.is_empty() && v.bytes().all(|c| c.is_ascii_lowercase() || c == b'-');
        let wide = matches!(
            v.as_str(),
            "inherit" | "initial" | "unset" | "revert" | "revert-layer"
        );
        // `display: contents` is flattened by the converter (layout spike).
        let blockified = id == PropId::Display && (v.starts_with("inline-") || v == "contents");
        if def.kind == ValueKind::Keyword
            && plain
            && !wide
            && !blockified
            && !def.values.contains(&v)
        {
            cx.errs.push(
                ConvError::new(
                    value_codes::OUTSIDE,
                    place.clone(),
                    format!(
                        "`{name}: {value}`; the profile allows {}",
                        def.values.join(", ")
                    ),
                )
                .element(subject)
                .prop(name),
            );
        }
    }
}

fn scripts(cx: &mut Ctx, skin: &Skin) {
    let mut files: BTreeSet<String> = skin.scripts.files.iter().cloned().collect();
    for &ix in &cx.out.ui.clone() {
        let e = cx.dom.elems[ix];
        if e.tag == "script"
            && let Some(src) = e.attr("src")
            && !external(src)
        {
            files.insert(src.to_string());
        }
    }
    for f in files {
        let path = cx.skin_dir.join(&f);
        match std::fs::read_to_string(&path) {
            Ok(text) if is_skin_path(&f) => cx.errs.extend(crate::script::check(&f, &text)),
            _ => cx.errs.push(ConvError::new(
                codes::RESOURCE,
                Place::file("skin.toml"),
                format!("the script `{f}` is not a file of the skin"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(s: &str) -> Result<SelInfo, SelectError> {
        select::parse(s).and_then(|a| select::check(&a))
    }

    /// The selectors and media queries the converter's own parser used to take (and refuse) go the same
    /// way through the profile's.
    #[test]
    fn profile_selectors_parse() {
        use ui2_package::profile::select::{Comb, Pseudo};
        let s = sel(".top .grp").expect("descendant");
        assert_eq!(s.static_part.len(), 2);
        assert_eq!(s.static_part[1].0, Some(Comb::Descendant));
        let s = sel(".yields .yield>svg.i").expect("child");
        assert_eq!(s.static_part[2].0, Some(Comb::Child));
        assert_eq!(s.subject().tag, Some(Tag::Svg));
        assert_eq!(s.subject().classes, ["i"]);
        let s = sel(".mbtn:hover").expect("hover");
        assert_eq!(s.conds[0].pseudo, Pseudo::Hover);
        assert!(sel(":root").expect("root").subject().root);
        assert!(sel("*").expect("any").subject().tag.is_none());
        assert!(sel("input::placeholder").expect("placeholder").placeholder);
        let s = sel(".a > .b:focus-visible").expect("focus-visible");
        assert_eq!(s.static_part[1].0, Some(Comb::Child));
        assert_eq!(s.conds[0].pseudo, Pseudo::FocusVisible);
        assert_eq!(s.conds[0].part, 1);
    }

    #[test]
    fn selectors_outside_the_profile_are_refused() {
        for bad in [
            ".a+.b",
            ".a~.b",
            "li:nth-child(2)",
            ".x::after",
            "[data-x]",
            ".a:first-child",
            "svg|use",
            ".a:not(.b)",
            ".a:focus-within",
        ] {
            assert_eq!(
                sel(bad).map(|_| ()).map_err(|e| e.code),
                Err(codes::SELECTOR),
                "{bad}"
            );
        }
    }

    #[test]
    fn media_queries_become_cells() {
        let cells = |p: &[&str]| {
            let all: Vec<MediaCond> = p
                .iter()
                .flat_map(|q| media::parse(q).expect("parses"))
                .collect();
            Cells::new(&all).map(|c| c.viewports())
        };
        assert_eq!(
            cells(&["(min-width:1600px)"]),
            Ok(vec![[1599, 1080], [1600, 1080]])
        );
        assert_eq!(
            cells(&["screen and (max-height:800px)"]),
            Ok(vec![[1920, 800], [1920, 801]])
        );
        assert_eq!(
            cells(&["(1280px<=width<1600px)"]),
            Ok(vec![[1279, 1080], [1280, 1080], [1600, 1080]])
        );
        assert_eq!(
            cells(&["(min-width:1600px)", "(width<=1600px)"]).map_err(|e| e.code),
            Err(codes::MEDIA)
        );
    }

    #[test]
    fn media_outside_the_profile_is_refused() {
        for bad in [
            "(prefers-color-scheme:dark)",
            "(orientation:landscape)",
            "(min-width:40em)",
            "print",
            "not screen",
            "(min-width:100px),(max-width:50px)",
            "(min-width:100px) or (max-width:50px)",
            "(width:100px)",
        ] {
            assert_eq!(
                media::parse(bad).map(|_| ()).map_err(|e| e.code),
                Err(codes::MEDIA),
                "{bad}"
            );
        }
    }

    #[test]
    fn neutral_text() {
        assert!(is_neutral("+48"));
        assert!(is_neutral("412/650"));
        assert!(is_neutral("\u{a0}\u{a0}"));
        assert!(is_neutral("(3) 12:40"));
        assert!(!is_neutral("3 turns"));
        assert!(!is_neutral("Gold"));
    }

    #[test]
    fn viewport_units_are_found_in_values() {
        assert_eq!(viewport_unit("calc(100vw - 4px)").as_deref(), Some("100vw"));
        assert_eq!(viewport_unit("10.5dvh").as_deref(), Some("10.5dvh"));
        assert_eq!(viewport_unit("12px 1em"), None);
        assert_eq!(viewport_unit("var(--x2vw)"), None);
        assert_eq!(viewport_unit("h1vw"), None);
    }

    #[test]
    fn properties_known_to_the_profile() {
        assert!(known_property("margin"));
        assert!(known_property("--anything"));
        assert!(known_property("color"));
        assert!(known_property("user-select"));
        assert!(known_property("appearance"));
        assert!(known_property("list-style"));
        for s in [
            "border-image",
            "grid-template",
            "grid-row",
            "grid-column",
            "grid-area",
        ] {
            assert!(known_property(s), "{s}");
        }
        assert!(!known_property("grid-template-areas"));
        assert!(!known_property("float"));
        assert!(!known_property("animation"));
    }
}
