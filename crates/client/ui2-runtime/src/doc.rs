//! The document's tables as the passes read them: the skin's package, and the mod fragments linked into it.
//!
//! [`Doc`] answers the same questions as a [`Package`] (templates, styles, strings, expressions, bindings,
//! extension tables) under the same names, so the passes are written against it once. A skin alone is read
//! straight from its package. Mounting a mod fragment *links* it ([`Doc::link`]): the fragment's templates,
//! style sets and records, strings, expressions, text templates and bindings are appended after the skin's,
//! with every id renumbered into the document's id space, so a node of a mod panel is a node like any other
//! for every pass. Linking copies the skin's templates and styles once (the first mount); the other tables
//! of the skin are never copied. A fragment linked once stays linked (its ids do not move) until the skin is
//! loaded again: mounting it again reuses it.
//!
//! What linking translates:
//! - colors given by tokens become the skin's token of the same name (a mod reads the skin's palette; a
//!   name the skin lacks keeps the fragment's own value);
//! - the font roles `ui-sans` and `ui-display` become the skin's faces of those roles;
//! - `state.*` in the fragment's expressions, actions and controls becomes `modstate.<id>.*` (the mod's own
//!   state, which the skin does not see);
//! - the fragment's viewport cells: a fragment without breakpoints uses its one cell everywhere.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use ui2_model::wire::ScriptCtxId;
use ui2_package::profile::action::Action;
use ui2_package::profile::expr::Expr;
use ui2_package::style::{BgImage, ColorV, Paint, StyleRecord};
use ui2_package::{
    ARefId, BindKind, Binding, CondKind, ExprId, Hotkey, ImageSrc, Images, ImgId, Kind, Lit,
    NodeKind, NodeRef, Package, PackageHash, Seg, StrId, StyleId, StyleSet, StyleSetId, Styles,
    Template, TemplateId, TextSrc, TokenId, Tokens, TplId,
};

use crate::island::IslandDef;

/// A fragment linked into the document.
#[derive(Debug, Clone)]
pub(crate) struct Linked {
    /// The fragment's package (its texts are looked up there).
    pub pkg: Arc<Package>,
    /// The mod id (the fragment's manifest id).
    pub id: Arc<str>,
    /// The document templates of the fragment.
    pub tpls: Range<u32>,
    /// The fragment's islands, by name.
    pub islands: Vec<(String, IslandDef)>,
}

/// The tables added by linked fragments.
#[derive(Debug, Clone)]
struct Ext {
    /// The skin's templates, then the fragments'.
    templates: Vec<Template>,
    /// The skin's styles, then the fragments' sets and records.
    styles: Styles,
    /// Strings after the skin's (id = skin count + index).
    strs: Vec<String>,
    str_index: HashMap<String, StrId>,
    exprs: Vec<Expr>,
    tpls: Vec<Vec<Seg>>,
    /// Bindings of the fragments' templates, ascending by `(tpl, node)`.
    bindings: Vec<Binding>,
    arefs: Vec<StrId>,
    tooltips: Vec<(NodeRef, TemplateId)>,
    hotkeys: Vec<Hotkey>,
    frags: Vec<Linked>,
}

/// The tables of the document.
#[derive(Debug, Clone)]
pub struct Doc {
    skin: Arc<Package>,
    ext: Option<Box<Ext>>,
    /// Sizes of the skin's tables (the first ids of the linked ones).
    n_strs: u32,
    n_exprs: u32,
    n_tpls: u32,
    n_arefs: u32,
    n_templates: u32,
}

/// What linking a fragment gave.
#[derive(Debug, Clone)]
pub(crate) struct LinkOut {
    /// The index of the fragment among the linked ones.
    pub frag: usize,
    /// Parts of the fragment this skin cannot use as they are (each becomes an error of the mount).
    pub warnings: Vec<String>,
}

/// The `state` path of a fragment as the mod's own state.
fn mod_state_path(id: &str, segs: &mut Vec<String>) {
    if segs.first().map(String::as_str) == Some("state") {
        segs.splice(0..1, ["modstate".to_string(), id.to_string()]);
    }
}

fn mod_state_text(id: &str, path: &mut String) {
    if let Some(rest) = path.strip_prefix("state.") {
        *path = format!("modstate.{id}.{rest}");
    } else if path == "state" {
        *path = format!("modstate.{id}");
    }
}

fn expr_paths(e: &mut Expr, f: &mut dyn FnMut(&mut Vec<String>)) {
    match e {
        Expr::Path(p) => f(p),
        Expr::Unary(_, a) => expr_paths(a, f),
        Expr::Binary(_, a, b) => {
            expr_paths(a, f);
            expr_paths(b, f);
        }
        Expr::Cond(a, b, c) => {
            expr_paths(a, f);
            expr_paths(b, f);
            expr_paths(c, f);
        }
        Expr::Call(_, args) => args.iter_mut().for_each(|a| expr_paths(a, f)),
        _ => {}
    }
}

/// Every color of a record.
fn record_colors(r: &mut StyleRecord, f: &mut dyn FnMut(&mut ColorV)) {
    for c in [
        &mut r.color,
        &mut r.background_color,
        &mut r.border_top_color,
        &mut r.border_right_color,
        &mut r.border_bottom_color,
        &mut r.border_left_color,
        &mut r.outline_color,
    ] {
        f(c);
    }
    for s in r.box_shadow.iter_mut().chain(r.text_shadow.iter_mut()) {
        f(&mut s.color);
    }
    for p in [&mut r.fill, &mut r.stroke] {
        if let Paint::Color(c) = p {
            f(c);
        }
    }
    for layer in &mut r.background_image {
        if let BgImage::Gradient(g) = layer {
            for stop in &mut g.stops {
                f(&mut stop.color);
            }
        }
    }
}

impl Doc {
    /// The document of a skin.
    pub fn new(skin: Arc<Package>) -> Self {
        let n_templates = skin.templates().len() as u32;
        Self {
            n_strs: skin.str_count() as u32,
            n_exprs: skin.bindings().exprs.len() as u32,
            n_tpls: skin.bindings().tpls.len() as u32,
            n_arefs: skin.asset_refs().count() as u32,
            n_templates,
            skin,
            ext: None,
        }
    }

    /// The skin's package.
    pub fn skin(&self) -> &Arc<Package> {
        &self.skin
    }

    /// The templates; template 0 is the skin's document, the fragments' templates follow the skin's.
    pub fn templates(&self) -> &[Template] {
        match &self.ext {
            Some(e) => &e.templates,
            None => self.skin.templates(),
        }
    }

    /// The styles (the cells are the skin's).
    pub fn styles(&self) -> &Styles {
        match &self.ext {
            Some(e) => &e.styles,
            None => self.skin.styles(),
        }
    }

    /// The color tokens of the skin.
    pub fn tokens(&self) -> &Tokens {
        self.skin.tokens()
    }

    /// A string.
    pub fn str(&self, id: StrId) -> Option<&str> {
        if id.0 < self.n_strs {
            return self.skin.str(id);
        }
        let e = self.ext.as_ref()?;
        e.strs
            .get((id.0 - self.n_strs) as usize)
            .map(String::as_str)
    }

    /// The id of a string.
    pub fn str_id(&self, s: &str) -> Option<StrId> {
        self.skin
            .str_id(s)
            .or_else(|| self.ext.as_ref()?.str_index.get(s).copied())
    }

    /// An expression.
    pub fn expr(&self, id: ExprId) -> Option<&Expr> {
        if id.0 < self.n_exprs {
            return self.skin.expr(id);
        }
        self.ext.as_ref()?.exprs.get((id.0 - self.n_exprs) as usize)
    }

    /// A text template.
    pub fn tpl(&self, id: TplId) -> Option<&[Seg]> {
        if id.0 < self.n_tpls {
            return self.skin.tpl(id);
        }
        self.ext
            .as_ref()?
            .tpls
            .get((id.0 - self.n_tpls) as usize)
            .map(Vec::as_slice)
    }

    /// The bindings of a template node.
    pub fn bindings_of(&self, tpl: TemplateId, node: u32) -> &[Binding] {
        if tpl.0 < self.n_templates {
            return self.skin.bindings_of(tpl, node);
        }
        let Some(e) = &self.ext else {
            return &[];
        };
        let items = &e.bindings;
        let key = |b: &Binding| (b.tpl.0, b.node);
        let from = items.partition_point(|b| key(b) < (tpl.0, node));
        let to = items.partition_point(|b| key(b) <= (tpl.0, node));
        &items[from..to]
    }

    /// The skin's pictures (a fragment's pictures are not drawn).
    pub fn images(&self) -> &Images {
        self.skin.images()
    }

    /// A game asset reference.
    pub fn asset_ref(&self, id: ARefId) -> Option<&str> {
        if id.0 < self.n_arefs {
            return self.skin.asset_ref(id);
        }
        let e = self.ext.as_ref()?;
        let s = *e.arefs.get((id.0 - self.n_arefs) as usize)?;
        self.str(s)
    }

    /// The tooltips: node and template, ascending by node.
    pub fn tooltips(&self) -> &[(NodeRef, TemplateId)] {
        match &self.ext {
            Some(e) => &e.tooltips,
            None => self.skin.tooltips(),
        }
    }

    /// The hotkeys of the skin and the fragments.
    pub fn hotkeys(&self) -> &[Hotkey] {
        match &self.ext {
            Some(e) => &e.hotkeys,
            None => self.skin.hotkeys(),
        }
    }

    /// The skin's languages.
    pub fn languages(&self) -> &[String] {
        self.skin.languages()
    }

    /// The text template of a key in a language: the skin's, else a fragment's (in that language, else in
    /// the fragment's first one).
    pub fn l10n(&self, lang: &str, key: &str) -> Option<&str> {
        self.skin.l10n(lang, key).or_else(|| {
            self.ext.as_ref()?.frags.iter().find_map(|f| {
                f.pkg.l10n(lang, key).or_else(|| {
                    let first = f.pkg.languages().first()?;
                    f.pkg.l10n(first, key)
                })
            })
        })
    }

    /// The skin's state defaults.
    pub fn state_defaults(&self) -> impl Iterator<Item = (&str, &Lit)> + '_ {
        self.skin.state_defaults()
    }

    /// The fragment a document template belongs to.
    pub(crate) fn frag_of(&self, tpl: TemplateId) -> Option<&Linked> {
        if tpl.0 < self.n_templates {
            return None;
        }
        self.ext
            .as_ref()?
            .frags
            .iter()
            .find(|f| f.tpls.contains(&tpl.0))
    }

    /// A linked fragment.
    pub(crate) fn frag(&self, i: usize) -> Option<&Linked> {
        self.ext.as_ref()?.frags.get(i)
    }

    /// The script context the markup of a template belongs to: the skin's, or the mod's of a fragment.
    pub fn ctx_of(&self, tpl: TemplateId) -> ScriptCtxId {
        match self.frag_of(tpl) {
            Some(f) => ScriptCtxId::Mod(Arc::clone(&f.id)),
            None => ScriptCtxId::Skin,
        }
    }

    /// The model path of a control's state path (`data-bind-model` keeps the path below the state root):
    /// `state.<path>` for the skin, `modstate.<id>.<path>` for a mod.
    pub fn state_path(&self, tpl: TemplateId, path: &str) -> String {
        match self.frag_of(tpl) {
            Some(f) => format!("modstate.{}.{path}", f.id),
            None => format!("state.{path}"),
        }
    }

    fn skin_island(&self, name: &str) -> Option<IslandDef> {
        let island = self
            .skin
            .islands()
            .iter()
            .find(|i| self.skin.str(i.id) == Some(name))?;
        Some(IslandDef {
            keys: island
                .keys
                .iter()
                .filter_map(|(k, n)| {
                    Some((self.skin.str(*k)?.to_string(), NodeRef(island.node.0, *n)))
                })
                .collect(),
        })
    }

    /// An island of a script context by name: the skin's for the skin, those of the mod's fragments for a
    /// mod.
    pub(crate) fn island(&self, ctx: &ScriptCtxId, name: &str) -> Option<IslandDef> {
        match ctx {
            ScriptCtxId::Skin => self.skin_island(name),
            ScriptCtxId::Mod(id) => self.ext.as_ref()?.frags.iter().find_map(|f| {
                (f.id == *id)
                    .then(|| f.islands.iter().find(|(n, _)| n == name))
                    .flatten()
                    .map(|(_, d)| d.clone())
            }),
        }
    }

    /// Every keyed node of every island.
    pub(crate) fn island_nodes(&self) -> Vec<NodeRef> {
        let mut out: Vec<NodeRef> = self
            .skin
            .islands()
            .iter()
            .flat_map(|i| i.keys.iter().map(|(_, n)| NodeRef(i.node.0, *n)))
            .collect();
        if let Some(e) = &self.ext {
            for f in &e.frags {
                for (_, d) in &f.islands {
                    out.extend(d.keys.iter().map(|(_, r)| *r));
                }
            }
        }
        out
    }

    fn ext_mut(&mut self) -> &mut Ext {
        let skin = &self.skin;
        self.ext.get_or_insert_with(|| {
            Box::new(Ext {
                templates: skin.templates().to_vec(),
                styles: skin.styles().clone(),
                strs: Vec::new(),
                str_index: HashMap::new(),
                exprs: Vec::new(),
                tpls: Vec::new(),
                bindings: Vec::new(),
                arefs: Vec::new(),
                tooltips: skin.tooltips().to_vec(),
                hotkeys: skin.hotkeys().to_vec(),
                frags: Vec::new(),
            })
        })
    }

    /// The document id of a string, added when the document does not have it.
    fn intern(&mut self, s: &str) -> StrId {
        if let Some(id) = self.str_id(s) {
            return id;
        }
        let n = self.n_strs;
        let e = self.ext_mut();
        let id = StrId(n + e.strs.len() as u32);
        e.strs.push(s.to_string());
        e.str_index.insert(s.to_string(), id);
        id
    }

    /// The skin's token of a fragment token, by name; `None` when the skin has no token of that name.
    fn skin_token(&self, name: &str) -> Option<TokenId> {
        let id = self.skin.str_id(name)?;
        let t = self.skin.tokens();
        t.alias(id).or_else(|| {
            t.roots
                .iter()
                .position(|(n, _)| *n == id)
                .map(|i| TokenId(i as u32))
        })
    }

    /// Links a mod fragment (see the [module docs](self)); a fragment with the same id and hash that is
    /// already linked is reused. `Err` for a package that is not a fragment.
    pub(crate) fn link(&mut self, frag: &Arc<Package>) -> Result<LinkOut, String> {
        if !matches!(frag.manifest().kind, Kind::Fragment { .. }) {
            return Err(format!(
                "{} is a skin package, not a mod fragment",
                frag.manifest().id
            ));
        }
        let id: Arc<str> = Arc::from(frag.manifest().id.as_str());
        let hash: PackageHash = frag.hash();
        if let Some(i) = self.ext.as_ref().and_then(|e| {
            e.frags
                .iter()
                .position(|f| f.id == id && f.pkg.hash() == hash)
        }) {
            return Ok(LinkOut {
                frag: i,
                warnings: Vec::new(),
            });
        }
        let mut warnings = Vec::new();

        // Strings, with the font roles as the skin's faces.
        let roles = &self.skin.manifest().font_roles;
        let (sans, display) = (roles.sans.0.clone(), roles.display.0.clone());
        let strs: Vec<StrId> = (0..frag.str_count() as u32)
            .map(|i| {
                let s = frag.str(StrId(i)).unwrap_or("").to_string();
                self.intern(&s)
            })
            .collect();
        let (sans, display) = (self.intern(&sans), self.intern(&display));
        let fam: Vec<StrId> = (0..frag.str_count() as u32)
            .map(|i| match frag.str(StrId(i)) {
                Some("ui-sans") => sans,
                Some("ui-display") => display,
                _ => strs[i as usize],
            })
            .collect();
        let s = |id: StrId| strs.get(id.index()).copied().unwrap_or(id);

        // Tokens: the skin's of the same name, else the fragment's own color.
        let ftok = frag.tokens();
        let toks: Vec<ColorV> = ftok
            .roots
            .iter()
            .map(|(name, rgba)| {
                let n = frag.str(*name).unwrap_or("");
                match self.skin_token(n) {
                    Some(t) => ColorV::Tok(t),
                    None => {
                        warnings.push(format!(
                            "the skin has no color {n}; the fragment's own value is used"
                        ));
                        ColorV::Lit(*rgba)
                    }
                }
            })
            .collect();

        let n_templates_now = self.templates().len() as u32;
        let tpl_base = n_templates_now;
        let n_exprs_now = self.n_exprs + self.ext.as_ref().map_or(0, |e| e.exprs.len() as u32);
        let n_tpls_now = self.n_tpls + self.ext.as_ref().map_or(0, |e| e.tpls.len() as u32);
        let n_arefs_now = self.n_arefs + self.ext.as_ref().map_or(0, |e| e.arefs.len() as u32);
        let set_base = self.styles().sets.len() as u32;
        let rec_base = self.styles().records.len() as u32;
        let t = |x: TemplateId| TemplateId(x.0 + tpl_base);
        let ex = |x: ExprId| ExprId(x.0 + n_exprs_now);
        let tp = |x: TplId| TplId(x.0 + n_tpls_now);
        let ar = |x: ARefId| ARefId(x.0 + n_arefs_now);
        let st = |x: StyleSetId| StyleSetId(x.0 + set_base);

        // Styles. The fragment's cells: one cell repeats over the skin's; several are used as they are when
        // the breakpoints are the skin's, else the first cell is used everywhere.
        let fstyles = frag.styles();
        let skin_cells = self.skin.styles().cells();
        let same_media = fstyles.media_w == self.skin.styles().media_w
            && fstyles.media_h == self.skin.styles().media_h;
        let frag_cells = fstyles.cells();
        if frag_cells > 1 && !same_media {
            // Limitation: a mod panel whose breakpoints differ from the skin's uses its first viewport cell at every size
            warnings.push(
                "the fragment's breakpoints differ from the skin's; its first viewport cell is used".into(),
            );
        }
        let mut records: Vec<StyleRecord> = Vec::with_capacity(fstyles.records.len());
        for r in &fstyles.records {
            let mut r = r.clone();
            for f in &mut r.font_family {
                *f = fam.get(f.index()).copied().unwrap_or(*f);
            }
            for layer in &mut r.background_image {
                if let BgImage::Url(p) = layer {
                    *p = s(*p);
                }
            }
            if let Some(p) = &mut r.border_image_source {
                *p = s(*p);
            }
            record_colors(&mut r, &mut |c| {
                if let ColorV::Tok(t) = c {
                    *c = toks.get(t.index()).copied().unwrap_or(ColorV::Lit([0; 4]));
                }
            });
            records.push(r);
        }
        let sets: Vec<StyleSet> = fstyles
            .sets
            .iter()
            .map(|set| {
                let per_cell = 1usize << set.conds.len();
                let conds = set
                    .conds
                    .iter()
                    .map(|c| {
                        let mut c = *c;
                        if let CondKind::Class(k) = &mut c.kind {
                            *k = s(*k);
                        }
                        c
                    })
                    .collect();
                let rec = |x: &StyleId| StyleId(x.0 + rec_base);
                let table: Vec<StyleId> = if frag_cells > 1 && same_media {
                    set.table.iter().map(rec).collect()
                } else {
                    let first = &set.table[..per_cell.min(set.table.len())];
                    (0..skin_cells)
                        .flat_map(|_| first.iter().map(rec))
                        .collect()
                };
                StyleSet { conds, table }
            })
            .collect();

        // Templates.
        let mut pictures = false;
        let templates: Vec<Template> = frag
            .templates()
            .iter()
            .map(|tpl| {
                let mut tpl = tpl.clone();
                tpl.name = s(tpl.name);
                for n in &mut tpl.nodes {
                    n.part = n.part.map(s);
                    n.style = st(n.style);
                    match &mut n.kind {
                        NodeKind::Box { .. } | NodeKind::Layer(_) => {}
                        NodeKind::Text(TextSrc::Static(x)) => *x = s(*x),
                        NodeKind::Text(TextSrc::L10n { key, args }) => {
                            *key = s(*key);
                            args.iter_mut().for_each(|a| *a = ex(*a));
                        }
                        NodeKind::Text(TextSrc::Bound(x)) => *x = tp(*x),
                        NodeKind::Image(ImageSrc::Pkg(i) | ImageSrc::Icon { img: i, .. }) => {
                            // Limitation: pictures inside a mod fragment are not drawn; the runtime has no texture pages of fragments
                            pictures = true;
                            *i = ImgId(u32::MAX);
                        }
                        NodeKind::Image(ImageSrc::Asset(a)) => *a = ar(*a),
                        NodeKind::Image(ImageSrc::Bound(e)) => *e = ex(*e),
                        NodeKind::Input { placeholder } => *placeholder = placeholder.map(s),
                        NodeKind::For {
                            tpl: body,
                            item,
                            index,
                            list,
                            key,
                            sort,
                            sort_desc,
                            filter,
                            ..
                        } => {
                            *body = t(*body);
                            *item = s(*item);
                            *index = index.map(s);
                            *list = ex(*list);
                            *key = ex(*key);
                            *sort = sort.map(ex);
                            *sort_desc = sort_desc.map(ex);
                            *filter = filter.map(ex);
                        }
                        NodeKind::If { tpl: body, cond } => {
                            *body = t(*body);
                            *cond = ex(*cond);
                        }
                    }
                }
                tpl
            })
            .collect();
        if pictures {
            warnings.push("the fragment's pictures are not drawn".into());
        }

        // Expressions, text templates and bindings: `state.*` is the mod's state.
        let fb = frag.bindings();
        let exprs: Vec<Expr> = fb
            .exprs
            .iter()
            .map(|e| {
                let mut e = e.clone();
                expr_paths(&mut e, &mut |p| mod_state_path(&id, p));
                e
            })
            .collect();
        let tpls: Vec<Vec<Seg>> = fb
            .tpls
            .iter()
            .map(|segs| {
                segs.iter()
                    .map(|seg| match *seg {
                        Seg::Lit(x) => Seg::Lit(s(x)),
                        Seg::Expr(e) => Seg::Expr(ex(e)),
                    })
                    .collect()
            })
            .collect();
        let bindings: Vec<Binding> = fb
            .items
            .iter()
            .map(|b| {
                let kind = match b.kind.clone() {
                    BindKind::Text(x) => BindKind::Text(tp(x)),
                    BindKind::ClassToggle { class, cond } => BindKind::ClassToggle {
                        class: s(class),
                        cond: ex(cond),
                    },
                    BindKind::Style { prop, tpl } => BindKind::Style { prop, tpl: tp(tpl) },
                    BindKind::Src(e) => BindKind::Src(ex(e)),
                    BindKind::Disabled(e) => BindKind::Disabled(ex(e)),
                    BindKind::Model(p) => BindKind::Model(s(p)),
                    BindKind::Event { ev, mut actions } => {
                        for a in &mut actions {
                            match a {
                                Action::Intent { args, .. } | Action::Call { args, .. } => {
                                    for arg in args {
                                        expr_paths(&mut arg.value, &mut |p| mod_state_path(&id, p));
                                    }
                                }
                                Action::Set { path, value } => {
                                    mod_state_text(&id, path);
                                    expr_paths(value, &mut |p| mod_state_path(&id, p));
                                }
                                Action::Toggle { path } => mod_state_text(&id, path),
                                Action::Stop => {}
                            }
                        }
                        BindKind::Event { ev, actions }
                    }
                    BindKind::Anchor { kind, id: e } => BindKind::Anchor { kind, id: ex(e) },
                    BindKind::Control {
                        kind,
                        value,
                        range,
                        options,
                    } => BindKind::Control {
                        kind,
                        value: value.map(s),
                        range,
                        options: options.map(ex),
                    },
                    other @ (BindKind::Fit { .. } | BindKind::HotkeyLabel) => other,
                };
                Binding {
                    tpl: t(b.tpl),
                    node: b.node,
                    kind,
                }
            })
            .collect();
        let arefs: Vec<StrId> = frag
            .asset_refs()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .iter()
            .map(|a| self.intern(a))
            .collect();
        let islands: Vec<(String, IslandDef)> = frag
            .islands()
            .iter()
            .filter_map(|i| {
                let name = frag.str(i.id)?.to_string();
                let keys = i
                    .keys
                    .iter()
                    .filter_map(|(k, n)| {
                        Some((frag.str(*k)?.to_string(), NodeRef(t(i.node.0), *n)))
                    })
                    .collect();
                Some((name, IslandDef { keys }))
            })
            .collect();
        let tooltips: Vec<(NodeRef, TemplateId)> = frag
            .tooltips()
            .iter()
            .map(|(n, tt)| (NodeRef(t(n.0), n.1), t(*tt)))
            .collect();
        let hotkeys: Vec<Hotkey> = frag
            .hotkeys()
            .iter()
            .map(|h| Hotkey {
                code: s(h.code),
                mods: h.mods,
                node: NodeRef(t(h.node.0), h.node.1),
            })
            .collect();

        let e = self.ext_mut();
        let tpl_end = tpl_base + templates.len() as u32;
        e.templates.extend(templates);
        e.styles.records.extend(records);
        e.styles.sets.extend(sets);
        e.exprs.extend(exprs);
        e.tpls.extend(tpls);
        e.bindings.extend(bindings);
        e.arefs.extend(arefs);
        e.tooltips.extend(tooltips);
        e.tooltips.sort_by_key(|(n, _)| *n);
        e.hotkeys.extend(hotkeys);
        e.frags.push(Linked {
            pkg: Arc::clone(frag),
            id,
            tpls: tpl_base..tpl_end,
            islands,
        });
        Ok(LinkOut {
            frag: e.frags.len() - 1,
            warnings,
        })
    }
}

impl crate::instantiate::Source for Doc {
    fn templates(&self) -> &[Template] {
        Doc::templates(self)
    }

    fn styles(&self) -> &Styles {
        Doc::styles(self)
    }

    fn str(&self, id: StrId) -> Option<&str> {
        Doc::str(self, id)
    }

    fn str_id(&self, s: &str) -> Option<StrId> {
        Doc::str_id(self, s)
    }
}
