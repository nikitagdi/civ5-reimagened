//! Writing the package: the compiled styles, the box tree, the bindings, the texts, the fonts and the pictures
//! of a run go through `PackageBuilder`.
//!
//! Templates. The box tree is the page of the preview's extract mode, where every `if` is drawn and every `for`
//! has one instance, so it has the shape of the templates. Template 0 is the document: the four layer roots
//! and what they hold. An element with `data-bind-for` or `data-bind-if` becomes a service node (`For`, `If`)
//! in its parent's template and is node 0 of a template of its own; templates are numbered in the order their
//! roots appear in the document, as the preview numbers them (`numbering.rs`), so the runtime's node labels
//! are the marks of the page. Text leaves are template nodes too (styled by their element); an element with
//! `data-bind-value` or `data-l10n` has exactly one leaf, bound. Anonymous boxes are not nodes: their text and
//! elements are children of the container (the runtime groups text runs itself).
//!
//! Loop variables. Expressions are written as the markup has them: a path that starts with the item variable
//! of an enclosing loop (`r.available` in a loop over `top.resources`) stays a path of that variable. The
//! runtime keeps the loop variables of every instance in scope and resolves such a path to the model path of
//! the item with the instance's keys, so a binding inside a loop body depends on exactly the fields it reads.
//!
//! Strings, records and sets are re-interned in the builder in the order the compiled styles have them (first
//! use); everything else follows document order. The package bytes are a pure function of the run.

use std::collections::BTreeMap;

use ui2_package::profile::attrs::NodeAttrs;
use ui2_package::profile::expr::{Expr, TplSeg};
use ui2_package::sections::blobs::{BlobRef, BlobSection};
use ui2_package::style::BgImage;
use ui2_package::{
    BindKind, Binding, CondKind, Face, FaceFamily, FontRole, FontRoles, ImageSrc, Kind, Lit,
    Manifest, Node, NodeKind, NodeRef, PackageBuilder, Seg, SrcLoc, StyleSet, StyleSetId, Tag,
    Template, TemplateId, TextSrc, Theme, Tokens, Tool, node_flags,
};

use crate::bindings;
use crate::errors::{ConvError, Place, codes};
use crate::fonts::Subset;
use crate::ir::{Dom, Element, Node as IrNode};
use crate::numbering;
use crate::skin::{Skin, StateValue};
use crate::styles::{BoxNode, Compiled, scrolls};

/// zstd level of font and picture blobs (deterministic for a fixed library version).
pub const ZSTD_LEVEL: i32 = 19;

/// A blob as a zstd frame of level [`ZSTD_LEVEL`]. The frame is a pure function of the bytes, so the frames
/// of this process are kept by the blake3 of their input: watch mode rebuilds a package with the same fonts
/// and pictures many times, and level 19 is slow.
fn compress(raw: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::sync::{Mutex, OnceLock};
    static FRAMES: OnceLock<Mutex<BTreeMap<[u8; 32], Vec<u8>>>> = OnceLock::new();
    let key = *blake3::hash(raw).as_bytes();
    let frames = FRAMES.get_or_init(|| Mutex::new(BTreeMap::new()));
    if let Some(z) = frames.lock().ok().and_then(|m| m.get(&key).cloned()) {
        return Ok(z);
    }
    let z = zstd::bulk::compress(raw, ZSTD_LEVEL)?;
    if let Ok(mut m) = frames.lock() {
        m.insert(key, z.clone());
    }
    Ok(z)
}

/// Tool versions for the manifest.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    /// The Chromium major version.
    pub chrome: String,
    /// blake3 of the pinned Chromium flags, hex.
    pub chrome_flags: String,
    /// The fontTools version.
    pub fonttools: String,
}

/// One picture of the package.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    /// The name (the file in the skin, or `icons/<symbol>@<w>x<h>` for an icon).
    pub path: String,
    /// The size in CSS px.
    pub size: [f32; 2],
    /// A mask (alpha only; painted with the node's color).
    pub mask: bool,
    /// The sprite at scale 1.
    pub x1: ui2_package::Sprite,
    /// The sprite at scale 2.
    pub x2: Option<ui2_package::Sprite>,
}

/// The pictures of a skin, rasterized and packed (made by `assets.rs` and `atlas.rs`).
#[derive(Debug, Clone, Default)]
pub struct Pictures {
    /// The pages, in package order, with their raw pixels.
    pub pages: Vec<(ui2_package::Page, Vec<u8>)>,
    /// The pictures; an `ImgId` of a node is the index here.
    pub imgs: Vec<Picture>,
    /// The picture of an element (by IR id): index into `imgs` and whether it is a mask icon.
    pub of_element: BTreeMap<u32, (usize, bool)>,
}

/// Everything the writer reads.
pub struct Inputs<'a> {
    /// The manifest.
    pub skin: &'a Skin,
    /// The page.
    pub dom: &'a Dom<'a>,
    /// The styles.
    pub styles: &'a Compiled,
    /// The bindings.
    pub bound: &'a bindings::Bound,
    /// The texts.
    pub texts: &'a crate::l10n::Texts,
    /// The font subsets.
    pub fonts: &'a [Subset],
    /// The rasterized pictures.
    pub pictures: &'a Pictures,
    /// The skin's scripts: path and bytes (LF line ends).
    pub scripts: &'a [(String, Vec<u8>)],
    /// Tool versions.
    pub tools: &'a Tools,
}

/// The written package.
#[derive(Debug, Clone)]
pub struct Written {
    /// The bytes.
    pub bytes: Vec<u8>,
    /// `<id>-<hash16>.ui2`.
    pub file_name: String,
    /// The package hash, hex.
    pub hash: String,
    /// Section tag and size in bytes, in file order.
    pub sections: Vec<(String, u32)>,
}

fn version_pair(s: &str) -> (u16, u16) {
    let mut it = s.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0))
}

/// The section table of package bytes (tag, length).
pub fn section_sizes(bytes: &[u8]) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let Some(n) = bytes
        .get(12..16)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    else {
        return out;
    };
    for i in 0..n as usize {
        let at = 16 + 16 * i;
        let Some(e) = bytes.get(at..at + 16) else {
            break;
        };
        let tag = String::from_utf8_lossy(&e[0..4]).to_string();
        let len = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
        out.push((tag, len));
    }
    out
}

struct Tb<'a> {
    inp: &'a Inputs<'a>,
    b: PackageBuilder,
    sets: Vec<StyleSetId>,
    numbers: BTreeMap<u32, (u32, u32)>,
    templates: BTreeMap<u32, (String, Vec<Node>, Vec<SrcLoc>)>,
    next_tpl: u32,
    files: Vec<String>,
    errs: Vec<ConvError>,
    /// The part names the converter gives (a mod panel's root is named by its panel id), by IR id.
    parts: BTreeMap<u32, String>,
    /// The tooltip templates by id.
    tooltip_tpls: BTreeMap<String, u32>,
    /// The nodes with `data-tooltip` and the id they name.
    tooltip_refs: Vec<(NodeRef, String, u32)>,
}

impl Tb<'_> {
    fn expr(&mut self, e: &Expr) -> ui2_package::ExprId {
        self.b.expr(e.clone())
    }

    fn tpl(&mut self, t: &ui2_package::profile::expr::Template) -> ui2_package::TplId {
        let mut segs = Vec::new();
        for s in &t.segs {
            segs.push(match s {
                TplSeg::Lit(l) => Seg::Lit(self.b.str(l)),
                TplSeg::Expr(e) => Seg::Expr(self.expr(e)),
            });
        }
        self.b.tpl(segs)
    }

    fn loc(&mut self, e: &Element) -> SrcLoc {
        let mut p = e.place();
        if p.file.is_empty() {
            // An element the page made without a source tag: the entry file, no position.
            p = Place::file(&self.inp.skin.skin.entry);
        }
        let file = match self.files.iter().position(|f| *f == p.file) {
            Some(i) => i,
            None => {
                self.files.push(p.file.clone());
                self.files.len() - 1
            }
        };
        SrcLoc {
            file: file as u16,
            line: p.line,
            col: p.col.min(u32::from(u16::MAX)) as u16,
        }
    }

    fn attrs(&self, id: u32) -> Option<&NodeAttrs> {
        self.inp.bound.nodes.get(&id).map(|n| &n.attrs)
    }

    fn err(&mut self, e: &Element, why: String) {
        self.errs
            .push(ConvError::new(codes::WRITE, e.place(), why).element(e.describe()));
    }

    /// A child box of a template: a template root becomes a service node and a template of its own.
    fn child(
        &mut self,
        tpl: u32,
        nodes: &mut Vec<Node>,
        locs: &mut Vec<SrcLoc>,
        n: &BoxNode,
        parent_set: StyleSetId,
    ) {
        match n {
            BoxNode::Elem { id, set, children } => {
                let Some(&ix) = self.inp.dom.by_id.get(id) else {
                    return;
                };
                let e = self.inp.dom.elems[ix];
                if let Some(t) = &e.tooltip {
                    self.tooltip_template(t, e, *set, children);
                } else if numbering::is_template_root(e) {
                    self.service(tpl, nodes, locs, e, *set, children, parent_set);
                } else {
                    self.element(tpl, nodes, locs, e, *set, children, false);
                }
            }
            BoxNode::Anon { children, .. } => {
                for c in children {
                    self.child(tpl, nodes, locs, c, parent_set);
                }
            }
            BoxNode::Text { id } => {
                let text = self.text_of(*id);
                let s = self.b.str(&text);
                nodes.push(Node {
                    kind: NodeKind::Text(TextSrc::Static(s)),
                    subtree: 1,
                    style: parent_set,
                    part: None,
                    flags: 0,
                });
                let last = locs.last().copied().unwrap_or(SrcLoc {
                    file: 0,
                    line: 0,
                    col: 0,
                });
                locs.push(last);
            }
        }
    }

    fn text_of(&self, id: u32) -> String {
        fn find(e: &Element, id: u32) -> Option<String> {
            for c in &e.children {
                match c {
                    IrNode::Text(t) if t.id == id => return Some(t.text.clone()),
                    IrNode::Element(k) => {
                        if let Some(s) = find(k, id) {
                            return Some(s);
                        }
                    }
                    IrNode::Text(_) => {}
                }
            }
            None
        }
        find(self.inp.dom.elems[0], id).unwrap_or_default()
    }

    /// The root of a tooltip template: a template of its own and no node of the parent's (the runtime instantiates
    /// it in the tooltip layer while a node that names it is hovered).
    fn tooltip_template(&mut self, id: &str, e: &Element, set: usize, children: &[BoxNode]) {
        let body = self.next_tpl;
        self.next_tpl += 1;
        if self.numbers.get(&e.id) != Some(&(body, 0)) {
            self.err(
                e,
                format!(
                    "the tooltip root is template {body}, the numbering says {:?}",
                    self.numbers.get(&e.id)
                ),
            );
        }
        if self.tooltip_tpls.insert(id.to_string(), body).is_some() {
            self.err(e, format!("two tooltip templates have the id {id}"));
        }
        let mut bn = Vec::new();
        let mut bl = Vec::new();
        self.element(body, &mut bn, &mut bl, e, set, children, true);
        self.templates
            .insert(body, (format!("tooltip:{id}"), bn, bl));
    }

    #[allow(clippy::too_many_arguments)]
    fn service(
        &mut self,
        tpl: u32,
        nodes: &mut Vec<Node>,
        locs: &mut Vec<SrcLoc>,
        e: &Element,
        set: usize,
        children: &[BoxNode],
        parent_set: StyleSetId,
    ) {
        let body = self.next_tpl;
        self.next_tpl += 1;
        if self.numbers.get(&e.id) != Some(&(body, 0)) {
            self.err(
                e,
                format!(
                    "the template root is template {body}, the numbering says {:?}",
                    self.numbers.get(&e.id)
                ),
            );
        }
        let attrs = self.attrs(e.id).cloned().unwrap_or_default();
        let loc = self.loc(e);
        let kind = if let Some(f) = &attrs.for_ {
            let list = self.expr(&f.list);
            let key = match &f.key {
                Some(k) => self.expr(k),
                None => {
                    self.err(e, "a loop without a key".into());
                    self.b.expr(Expr::Null)
                }
            };
            let item = self.b.str(&f.item);
            let index = f.index.as_ref().map(|i| self.b.str(i));
            let sort = f.sort.as_ref().map(|x| self.expr(x));
            let sort_desc = f.sort_desc.as_ref().map(|x| self.expr(x));
            let filter = f.filter.as_ref().map(|x| self.expr(x));
            NodeKind::For {
                tpl: TemplateId(body),
                item,
                index,
                list,
                key,
                virt: f.virtual_list,
                sort,
                sort_desc,
                filter,
            }
        } else {
            let cond = match &attrs.cond {
                Some(c) => self.expr(c),
                None => self.b.expr(Expr::Bool(true)),
            };
            NodeKind::If {
                tpl: TemplateId(body),
                cond,
            }
        };
        nodes.push(Node {
            kind,
            subtree: 1,
            style: parent_set,
            part: None,
            flags: 0,
        });
        locs.push(loc);
        let _ = tpl;
        let mut bn = Vec::new();
        let mut bl = Vec::new();
        self.element(body, &mut bn, &mut bl, e, set, children, true);
        let name = attrs
            .part
            .clone()
            .unwrap_or_else(|| format!("{}:{}", e.tag, body));
        self.templates.insert(body, (name, bn, bl));
    }

    #[allow(clippy::too_many_arguments)]
    fn element(
        &mut self,
        tpl: u32,
        nodes: &mut Vec<Node>,
        locs: &mut Vec<SrcLoc>,
        e: &Element,
        set: usize,
        children: &[BoxNode],
        body_root: bool,
    ) {
        let style = self.sets.get(set).copied().unwrap_or(StyleSetId(0));
        let attrs = self.attrs(e.id).cloned().unwrap_or_default();
        let idx = nodes.len() as u32;
        let element_index = nodes
            .iter()
            .filter(|n| {
                matches!(
                    n.kind,
                    NodeKind::Box { .. }
                        | NodeKind::Layer(_)
                        | NodeKind::Image(_)
                        | NodeKind::Input { .. }
                )
            })
            .count() as u32;
        if self.numbers.get(&e.id) != Some(&(tpl, element_index)) {
            self.err(
                e,
                format!(
                    "the template node is {tpl}:{element_index}, the numbering says {:?}",
                    self.numbers.get(&e.id).map(|n| numbering::label(*n))
                ),
            );
        }
        let _ = body_root;
        let at = NodeRef(TemplateId(tpl), idx);
        let tag = Tag::from_name(&e.tag);
        let pic = self.inp.pictures.of_element.get(&e.id).copied();
        let kind = if let (Some(layer), 0) = (attrs.layer, tpl) {
            NodeKind::Layer(layer)
        } else if let Some((img, mask)) = pic {
            let id = ui2_package::ImgId(img as u32);
            if mask {
                NodeKind::Image(ImageSrc::Icon {
                    img: id,
                    mask: true,
                })
            } else {
                NodeKind::Image(ImageSrc::Pkg(id))
            }
        } else if let Some(src) = &attrs.src {
            NodeKind::Image(ImageSrc::Bound(self.expr(src)))
        } else if let Some(a) = &attrs.asset {
            NodeKind::Image(ImageSrc::Asset(self.b.asset_ref(a)))
        } else if e.tag == "input" {
            let placeholder = attrs.l10n_placeholder.as_ref().map(|k| self.b.str(k));
            NodeKind::Input { placeholder }
        } else {
            match tag {
                Some(t) => NodeKind::Box { tag: t },
                None => {
                    self.err(e, format!("<{}> has no tag of the profile", e.tag));
                    NodeKind::Box { tag: Tag::Div }
                }
            }
        };
        let part_name = attrs
            .part
            .clone()
            .or_else(|| self.parts.get(&e.id).cloned());
        let part = part_name.as_ref().map(|p| self.b.str(p));
        let mut flags = 0u16;
        if matches!(tag, Some(Tag::Button | Tag::Input)) || e.attr("tabindex").is_some() {
            flags |= node_flags::FOCUSABLE;
        }
        if attrs.control.is_some() {
            flags |= node_flags::CONTROL;
        }
        if attrs.anchor.is_some() {
            flags |= node_flags::ANCHOR;
        }
        if self.inp.styles.record_of(e.id, 0, 0).is_some_and(scrolls) {
            flags |= node_flags::SCROLL;
        }
        nodes.push(Node {
            kind,
            subtree: 1,
            style,
            part,
            flags,
        });
        let loc = self.loc(e);
        locs.push(loc);

        // Extension points, parts, hotkeys.
        if let Some(s) = &attrs.slot {
            self.b.slot(s, at);
        }
        if let Some(p) = &part_name {
            self.b.part(p, at);
        }
        if let Some(s) = &attrs.screen {
            self.b.screen(s, at);
        }
        if let Some(h) = &attrs.hotkey {
            self.b.hotkey(&h.code, h.mods(), at);
        }
        if let Some(isl) = &attrs.island {
            self.b.island(&isl.id, at, &[]);
        }
        if let Some(t) = &attrs.tooltip {
            self.tooltip_refs.push((at, t.clone(), e.id));
        }

        // Bindings, in a fixed order.
        let tid = TemplateId(tpl);
        let bind = |tb: &mut Self, kind: BindKind| {
            tb.b.binding(Binding {
                tpl: tid,
                node: idx,
                kind,
            });
        };
        for t in &attrs.class_toggle {
            let class = self.b.str(&t.class);
            let cond = self.expr(&t.cond);
            bind(self, BindKind::ClassToggle { class, cond });
        }
        for s in &attrs.style {
            let tpl = self.tpl(&s.tpl);
            bind(self, BindKind::Style { prop: s.prop, tpl });
        }
        if let Some(d) = &attrs.disabled {
            let d = self.expr(d);
            bind(self, BindKind::Disabled(d));
        }
        if let Some(m) = &attrs.model {
            let m = self.b.str(m);
            bind(self, BindKind::Model(m));
        }
        for ev in &attrs.events {
            let actions = ev.actions.clone();
            bind(self, BindKind::Event { ev: ev.ev, actions });
        }
        if let Some((kind, id)) = &attrs.anchor {
            let id = self.expr(id);
            bind(self, BindKind::Anchor { kind: *kind, id });
        }
        if let Some(c) = &attrs.control {
            let value = c.value.as_ref().map(|v| self.b.str(v));
            let options = c.options.as_ref().map(|o| self.expr(o));
            let range = c.range.map(|(a, b, s)| [a as f32, b as f32, s as f32]);
            bind(
                self,
                BindKind::Control {
                    kind: c.kind,
                    value,
                    range,
                    options,
                },
            );
        }
        if let Some(min) = attrs.fit {
            bind(self, BindKind::Fit { min });
        }
        if attrs.hotkey_label {
            bind(self, BindKind::HotkeyLabel);
        }

        // Content: one bound leaf, or the children.
        let text = if let Some(v) = &attrs.value {
            Some(TextSrc::Bound(self.tpl(v)))
        } else if let Some(k) = &attrs.l10n {
            let key = self.b.str(k);
            let args = attrs.l10n_args.iter().map(|a| self.expr(a)).collect();
            Some(TextSrc::L10n { key, args })
        } else if attrs.hotkey_label {
            // The runtime writes the key's name into this leaf (the markup's text only stands in for it).
            Some(TextSrc::Static(self.b.str("")))
        } else {
            None
        };
        if let Some(t) = text {
            nodes.push(Node {
                kind: NodeKind::Text(t),
                subtree: 1,
                style,
                part: None,
                flags: 0,
            });
            locs.push(loc);
        } else if !matches!(nodes[idx as usize].kind, NodeKind::Image(_)) {
            for c in children {
                self.child(tpl, nodes, locs, c, style);
            }
        }
        nodes[idx as usize].subtree = nodes.len() as u32 - idx;
    }
}

fn lit_of(v: &StateValue) -> Lit {
    match v {
        StateValue::Bool(b) => Lit::Bool(*b),
        StateValue::Int(i) => Lit::Int(*i),
        StateValue::Float(f) => Lit::Float(*f),
        StateValue::Str(s) => Lit::Str(s.clone()),
    }
}

/// Writes the package.
pub fn write(inp: &Inputs) -> Result<Written, Vec<ConvError>> {
    let c = inp.styles;
    let mut b = PackageBuilder::new();
    let mut errs = Vec::new();

    // Styles: records and sets in the compiled order, strings re-interned.
    let mut record_ids = Vec::new();
    for r in &c.records {
        let mut r = r.clone();
        r.font_family = r
            .font_family
            .iter()
            .map(|s| b.str(c.strs.get(*s)))
            .collect();
        // Pictures by path: the path of an `IMGS` entry, in the builder's string table.
        for layer in &mut r.background_image {
            if let BgImage::Url(s) = layer {
                *s = b.str(c.strs.get(*s));
            }
        }
        if let Some(s) = r.border_image_source.as_mut() {
            *s = b.str(c.strs.get(*s));
        }
        record_ids.push(b.style_record(r));
    }
    let mut sets = Vec::new();
    for s in &c.sets {
        let conds = s
            .conds
            .iter()
            .map(|cd| ui2_package::Cond {
                up: cd.up,
                kind: match cd.kind {
                    CondKind::Class(id) => CondKind::Class(b.str(c.strs.get(id))),
                    k => k,
                },
            })
            .collect();
        let table = s
            .table
            .iter()
            .map(|id| record_ids.get(id.index()).copied().unwrap_or(*id))
            .collect();
        sets.push(b.style_set(StyleSet { conds, table }));
    }
    b.media(c.media_w.clone(), c.media_h.clone());
    let mut tokens = Tokens::default();
    for (name, rgba) in &c.tokens.roots {
        tokens.roots.push((b.str(c.strs.get(*name)), *rgba));
    }
    for (name, t) in &c.tokens.aliases {
        tokens.aliases.push((b.str(c.strs.get(*name)), *t));
    }
    let default_theme = inp
        .skin
        .themes
        .default
        .clone()
        .unwrap_or_else(|| "default".to_string());
    // SIMPLIFIED(SIMP-301): only the default theme is compiled; a skin that lists other theme stylesheets is refused.
    if !inp.skin.themes.css.is_empty() {
        errs.push(ConvError::new(
            codes::SKIN_TOML,
            Place::file("skin.toml"),
            "theme stylesheets other than the default theme are not compiled yet",
        ));
    }
    let theme_name = b.str(&default_theme);
    tokens.themes.push(Theme {
        name: theme_name,
        set: Vec::new(),
    });
    b.tokens(tokens);

    // Pictures (their pages keep the order the packer made).
    for (page, raw) in &inp.pictures.pages {
        let z = match compress(raw) {
            Ok(z) => z,
            Err(e) => {
                errs.push(ConvError::new(
                    codes::WRITE,
                    Place::default(),
                    format!("zstd: {e}"),
                ));
                continue;
            }
        };
        let mut p = page.clone();
        p.blake3 = *blake3::hash(raw).as_bytes();
        p.blob = BlobRef {
            section: BlobSection::Imgs,
            offset: 0,
            len: 0,
            raw_len: raw.len() as u32,
            zstd: true,
        };
        b.image_page(p, &z, raw.len() as u32);
    }
    for p in &inp.pictures.imgs {
        let path = b.str(&p.path);
        b.image(ui2_package::Img {
            path,
            size: p.size,
            mask: p.mask,
            x1: p.x1,
            x2: p.x2,
        });
    }

    // Templates.
    let numbers = numbering::number(inp.dom);
    let parts = match &inp.skin.fragment {
        Some(f) => crate::fragment::panel_roots(inp.dom, f)
            .into_iter()
            .map(|(ix, panel)| (inp.dom.elems[ix].id, panel))
            .collect(),
        None => BTreeMap::new(),
    };
    let mut tb = Tb {
        inp,
        b,
        sets,
        numbers,
        templates: BTreeMap::new(),
        next_tpl: 1,
        files: Vec::new(),
        errs: Vec::new(),
        parts,
        tooltip_tpls: BTreeMap::new(),
        tooltip_refs: Vec::new(),
    };
    let mut doc = Vec::new();
    let mut locs = Vec::new();
    for root in &c.tree {
        tb.child(0, &mut doc, &mut locs, root, StyleSetId(0));
    }
    tb.templates.insert(0, ("document".to_string(), doc, locs));
    // Tooltips: the templates are known once the whole tree is written (the tooltip layer comes last).
    for (at, id, el) in std::mem::take(&mut tb.tooltip_refs) {
        match tb.tooltip_tpls.get(&id) {
            Some(&t) => tb.b.tooltip(at, TemplateId(t)),
            None => {
                if let Some(&ix) = inp.dom.by_id.get(&el) {
                    tb.err(inp.dom.elems[ix], format!("no tooltip template {id}"));
                }
            }
        }
    }
    let Tb {
        mut b,
        templates,
        files,
        errs: terrs,
        ..
    } = tb;
    errs.extend(terrs);
    let mut places = Vec::new();
    for (i, (id, (name, nodes, locs))) in templates.into_iter().enumerate() {
        if id as usize != i {
            errs.push(ConvError::new(
                codes::WRITE,
                Place::default(),
                format!("template {i} is missing"),
            ));
        }
        let name = b.str(&name);
        b.template(Template { name, nodes });
        places.push(locs);
    }
    let file_refs: Vec<&str> = files.iter().map(String::as_str).collect();
    b.source_map(&file_refs, places);

    // Bindings: what the skin reads and sends, its state. A mod's own fields (`mods.<id>.*`) are not paths of the
    // shared model: the client checks them when it mounts the mod's model fragment.
    let own = inp
        .skin
        .fragment
        .as_ref()
        .map(|f| format!("mods.{}.", f.mod_id));
    for (p, t) in &inp.bound.paths {
        if own.as_deref().is_some_and(|o| p.starts_with(o)) {
            continue;
        }
        b.used_path(p, t.clone());
    }
    for (i, a) in &inp.bound.intents {
        b.intent(i, a.clone());
    }
    for h in &inp.bound.handlers {
        b.handler(h);
    }
    for (path, v) in &inp.skin.state {
        b.state_default(path, lit_of(v));
    }

    // Texts.
    let mut languages: Vec<String> = inp.texts.langs.iter().map(|l| l.tag.clone()).collect();
    languages.sort_by_key(|l| (l != "en_US", l.clone()));
    for lang in &inp.texts.langs {
        for (k, v) in &lang.entries {
            b.l10n(&lang.tag, k, v);
        }
    }

    // Scripts.
    for (path, bytes) in inp.scripts {
        b.script(path, bytes);
    }
    let reads: Vec<&str> = inp.skin.scripts.reads.iter().map(String::as_str).collect();
    if !reads.is_empty() {
        b.script_reads(&reads);
    }

    // Fonts.
    let role = |family: &str| {
        if inp.skin.fonts.sans.as_deref() == Some(family) {
            Some(FontRole::Sans)
        } else if inp.skin.fonts.display.as_deref() == Some(family) {
            Some(FontRole::Display)
        } else {
            None
        }
    };
    let ranges = inp.texts.glyphs.codepoints();
    for f in inp.fonts {
        let z = match compress(&f.bytes) {
            Ok(z) => z,
            Err(e) => {
                errs.push(ConvError::new(
                    codes::WRITE,
                    f.decl.place.clone(),
                    format!("zstd: {e}"),
                ));
                continue;
            }
        };
        let family = b.str(&f.decl.family);
        let face = Face {
            family,
            weight: f.decl.weight,
            italic: f.decl.italic,
            role: role(&f.decl.family),
            ranges: ranges.clone(),
            blob: BlobRef {
                section: BlobSection::Font,
                offset: 0,
                len: 0,
                raw_len: f.bytes.len() as u32,
                zstd: true,
            },
            blake3: *blake3::hash(&f.bytes).as_bytes(),
        };
        b.font(face, &z, f.bytes.len() as u32);
    }

    if !errs.is_empty() {
        return Err(errs);
    }
    let s = &inp.skin.skin;
    // A fragment has no texts of its own; the reader still wants a language.
    if languages.is_empty() {
        languages.push("en_US".to_string());
    }
    let kind = match &inp.skin.fragment {
        Some(f) => Kind::Fragment { slots: f.points() },
        None => Kind::Skin,
    };
    let manifest = Manifest {
        kind,
        id: s.id.clone(),
        version: s.version.clone(),
        title_key: s.title.clone(),
        profile: ui2_package::profile::Profile::v1().version(),
        model: version_pair(&s.model),
        api: version_pair(&s.api),
        default: s.default,
        tool: Tool {
            converter: env!("CARGO_PKG_VERSION").to_string(),
            chrome: inp.tools.chrome.clone(),
            chrome_flags: inp.tools.chrome_flags.clone(),
            fonttools: inp.tools.fonttools.clone(),
        },
        themes: vec![default_theme],
        languages,
        font_roles: FontRoles {
            sans: FaceFamily(inp.skin.fonts.sans.clone().unwrap_or_default()),
            display: FaceFamily(inp.skin.fonts.display.clone().unwrap_or_default()),
        },
        schema_hash: ui2_model::Schema::v1().hash(),
    };
    let bytes = b.finish(manifest).map_err(|e| {
        vec![ConvError::new(
            codes::WRITE,
            Place::default(),
            format!("the package builder refused the data: {e}"),
        )]
    })?;
    let pkg = ui2_package::Package::read(std::sync::Arc::from(bytes.as_slice())).map_err(|e| {
        vec![ConvError::new(
            codes::WRITE,
            Place::default(),
            format!("the written package does not read back: {e}"),
        )]
    })?;
    let hash = pkg.hash();
    let hex: String = hash.0.iter().map(|b| format!("{b:02x}")).collect();
    let file_name = match &inp.skin.fragment {
        Some(_) => format!("{}-{}-{}.ui2", s.id, s.version, hash.short()),
        None => format!("{}-{}.ui2", s.id, hash.short()),
    };
    Ok(Written {
        file_name,
        hash: hex,
        sections: section_sizes(&bytes),
        bytes,
    })
}

/// The size budget of a package: (what, bytes, limit).
pub fn budget(sections: &[(String, u32)]) -> Vec<(&'static str, u64, u64)> {
    let of = |t: &str| {
        sections
            .iter()
            .filter(|(s, _)| s == t)
            .map(|(_, n)| u64::from(*n))
            .sum::<u64>()
    };
    let total: u64 = sections.iter().map(|(_, n)| u64::from(*n)).sum();
    let fonts = of("FONT");
    let images = of("IMGS");
    let scripts = of("SCRP");
    vec![
        ("total", total, 3_000_000),
        ("fonts", fonts, 1_200_000),
        ("pictures", images, 800_000),
        ("scripts", scripts, 750_000),
        ("rest", total - fonts - images - scripts, 500_000),
    ]
}

/// The E8 error of a package over its budget: one error that names every part over its limit.
pub fn check_budget(sections: &[(String, u32)]) -> Vec<ConvError> {
    let over: Vec<String> = budget(sections)
        .into_iter()
        .filter(|(_, n, max)| n > max)
        .map(|(what, n, max)| format!("{what}: {n} bytes, at most {max}"))
        .collect();
    if over.is_empty() {
        return Vec::new();
    }
    vec![ConvError::new(
        codes::BUDGET,
        Place::default(),
        over.join("; "),
    )]
}
