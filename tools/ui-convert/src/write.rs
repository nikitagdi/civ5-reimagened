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
//! Loop variables. A path that starts with the item variable of an enclosing loop over a model list
//! (`r.available` in a loop over `top.resources`) is written as the model path of the item
//! (`top.resources[].available`): the runtime reads it with the keys of the instance, so a binding inside a
//! loop body depends on exactly the fields it reads. Index variables stay as they are.
//!
//! Strings, records and sets are re-interned in the builder in the order the compiled styles have them (first
//! use); everything else follows document order. The package bytes are a pure function of the run.

use std::collections::BTreeMap;

use ui2_package::profile::action::{Action, Arg};
use ui2_package::profile::attrs::NodeAttrs;
use ui2_package::profile::expr::{Expr, TplSeg};
use ui2_package::sections::blobs::{BlobRef, BlobSection};
use ui2_package::style::{Overflow, StyleRecord};
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
use crate::styles::{BoxNode, Compiled};

/// zstd level of font and picture blobs (deterministic for a fixed library version).
pub const ZSTD_LEVEL: i32 = 19;

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

/// A loop variable's model path, as segments (`top`, `resources[]`).
type Vars = Vec<(String, Vec<String>)>;

fn rewrite(e: &Expr, vars: &Vars) -> Expr {
    match e {
        Expr::Path(segs) => {
            let Some(head) = segs.first() else {
                return e.clone();
            };
            match vars.iter().rev().find(|(v, _)| v == head) {
                Some((_, origin)) => {
                    let mut out = origin.clone();
                    out.extend(segs[1..].iter().cloned());
                    Expr::Path(out)
                }
                None => e.clone(),
            }
        }
        Expr::Unary(op, a) => Expr::Unary(*op, Box::new(rewrite(a, vars))),
        Expr::Binary(op, a, b) => {
            Expr::Binary(*op, Box::new(rewrite(a, vars)), Box::new(rewrite(b, vars)))
        }
        Expr::Cond(a, b, c) => Expr::Cond(
            Box::new(rewrite(a, vars)),
            Box::new(rewrite(b, vars)),
            Box::new(rewrite(c, vars)),
        ),
        Expr::Call(f, args) => Expr::Call(*f, args.iter().map(|a| rewrite(a, vars)).collect()),
        other => other.clone(),
    }
}

fn rewrite_action(a: &Action, vars: &Vars) -> Action {
    let args = |list: &[Arg]| {
        list.iter()
            .map(|x| Arg {
                name: x.name.clone(),
                value: rewrite(&x.value, vars),
            })
            .collect()
    };
    match a {
        Action::Intent { name, args: a } => Action::Intent {
            name: name.clone(),
            args: args(a),
        },
        Action::Call { name, args: a } => Action::Call {
            name: name.clone(),
            args: args(a),
        },
        Action::Set { path, value } => Action::Set {
            path: path.clone(),
            value: rewrite(value, vars),
        },
        other => other.clone(),
    }
}

struct Tb<'a> {
    inp: &'a Inputs<'a>,
    b: PackageBuilder,
    sets: Vec<StyleSetId>,
    numbers: BTreeMap<u32, (u32, u32)>,
    templates: BTreeMap<u32, (String, Vec<Node>, Vec<SrcLoc>)>,
    next_tpl: u32,
    vars: Vars,
    files: Vec<String>,
    errs: Vec<ConvError>,
}

impl Tb<'_> {
    fn expr(&mut self, e: &Expr) -> ui2_package::ExprId {
        let e = rewrite(e, &self.vars);
        self.b.expr(e)
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
                if numbering::is_template_root(e) {
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
        let mut pushed = false;
        let kind = if let Some(f) = &attrs.for_ {
            let list = self.expr(&f.list);
            let key = match &f.key {
                Some(k) => {
                    // The key is read per item: inside the item's scope.
                    let origin = match rewrite(&f.list, &self.vars) {
                        Expr::Path(mut segs) => {
                            if let Some(last) = segs.last_mut() {
                                last.push_str("[]");
                            }
                            Some(segs)
                        }
                        _ => None,
                    };
                    if let Some(o) = origin {
                        self.vars.push((f.item.clone(), o));
                        pushed = true;
                    }
                    self.expr(k)
                }
                None => {
                    self.err(e, "a loop without a key".into());
                    self.b.expr(Expr::Null)
                }
            };
            if !pushed && let Expr::Path(mut segs) = rewrite(&f.list, &self.vars) {
                if let Some(last) = segs.last_mut() {
                    last.push_str("[]");
                }
                self.vars.push((f.item.clone(), segs));
                pushed = true;
            }
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
        if pushed {
            self.vars.pop();
        }
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
        let part = attrs.part.as_ref().map(|p| self.b.str(p));
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
        if let Some(p) = &attrs.part {
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
            let actions = ev
                .actions
                .iter()
                .map(|a| rewrite_action(a, &self.vars))
                .collect();
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

fn scrolls(r: &StyleRecord) -> bool {
    matches!(r.overflow_x, Overflow::Auto | Overflow::Scroll)
        || matches!(r.overflow_y, Overflow::Auto | Overflow::Scroll)
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
        let z = match zstd::bulk::compress(raw, ZSTD_LEVEL) {
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
    let mut tb = Tb {
        inp,
        b,
        sets,
        numbers,
        templates: BTreeMap::new(),
        next_tpl: 1,
        vars: Vec::new(),
        files: Vec::new(),
        errs: Vec::new(),
    };
    let mut doc = Vec::new();
    let mut locs = Vec::new();
    for root in &c.tree {
        tb.child(0, &mut doc, &mut locs, root, StyleSetId(0));
    }
    tb.templates.insert(0, ("document".to_string(), doc, locs));
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

    // Bindings: what the skin reads and sends, its state.
    for (p, t) in &inp.bound.paths {
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
        let z = match zstd::bulk::compress(&f.bytes, ZSTD_LEVEL) {
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
    let manifest = Manifest {
        kind: Kind::Skin,
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
    Ok(Written {
        file_name: format!("{}-{}.ui2", s.id, hash.short()),
        hash: hex,
        sections: section_sizes(&bytes),
        bytes,
    })
}

/// The N4 budget of a package: (what, bytes, limit).
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

/// E8 errors for a budget over its limits.
pub fn check_budget(sections: &[(String, u32)]) -> Vec<ConvError> {
    budget(sections)
        .into_iter()
        .filter(|(_, n, max)| n > max)
        .map(|(what, n, max)| {
            ConvError::new(
                codes::BUDGET,
                Place::default(),
                format!("{what}: {n} bytes, at most {max}"),
            )
        })
        .collect()
}
