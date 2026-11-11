//! In-memory skin packages for tests (feature `testkit`).
//!
//! [`Kit`] wraps the package builder with defaults (manifest, one viewport cell) and builds templates from
//! nested [`TNode`]s, computing the preorder list and subtree sizes. String ids returned while building are
//! provisional: the finished package sorts its string table, so look strings up again with
//! `Package::str_id` after [`Kit::finish`].

use std::sync::Arc;

use ui2_package::profile::TypeSig;
use ui2_package::profile::expr::Expr;
use ui2_package::{
    BindKind, Binding, BlobRef, BlobSection, Bound, Cond, ExprId, Face, FaceFamily, FontRoles,
    ImageSrc, Img, ImgId, Kind, Layer, Lit, Manifest, Node, NodeKind, Package, PackageBuilder,
    PackageError, Page, PageFormat, Seg, Sprite, StrId, StyleId, StyleRecord, StyleSet, StyleSetId,
    Tag, Template, TemplateId, TextSrc, Tokens, Tool, TplId,
};

/// A template node with its children, for [`Kit::template`].
#[derive(Debug, Clone)]
pub struct TNode {
    kind: NodeKind,
    style: StyleSetId,
    part: Option<StrId>,
    flags: u16,
    binds: Vec<BindKind>,
    children: Vec<TNode>,
}

impl TNode {
    /// A node of any kind.
    pub fn new(kind: NodeKind, style: StyleSetId) -> Self {
        Self {
            kind,
            style,
            part: None,
            flags: 0,
            binds: Vec::new(),
            children: Vec::new(),
        }
    }

    /// A text leaf whose text is a template with inserts (`data-bind-value`).
    pub fn bound_text(tpl: TplId, style: StyleSetId) -> Self {
        Self::new(NodeKind::Text(TextSrc::Bound(tpl)), style)
    }

    /// A text leaf with a skin text and its arguments (`data-l10n`, `data-l10n-args`).
    pub fn l10n_text(key: StrId, args: Vec<ExprId>, style: StyleSetId) -> Self {
        Self::new(NodeKind::Text(TextSrc::L10n { key, args }), style)
    }

    /// A mask icon (an `svg` of the skin's icon sprite), painted with the node's `color`.
    pub fn icon(img: ImgId, style: StyleSetId) -> Self {
        Self::new(NodeKind::Image(ImageSrc::Icon { img, mask: true }), style)
    }

    /// Adds a binding of this node (`data-bind-*`).
    pub fn bind(mut self, kind: BindKind) -> Self {
        self.binds.push(kind);
        self
    }

    /// A `div`.
    pub fn el(style: StyleSetId) -> Self {
        Self::new(NodeKind::Box { tag: Tag::Div }, style)
    }

    /// The root of a layer.
    pub fn layer(layer: Layer, style: StyleSetId) -> Self {
        Self::new(NodeKind::Layer(layer), style)
    }

    /// A static text leaf (styled like its parent: pass the parent's set).
    pub fn text(s: StrId, style: StyleSetId) -> Self {
        Self::new(NodeKind::Text(TextSrc::Static(s)), style)
    }

    /// Sets the part name (`data-part`).
    pub fn part(mut self, part: StrId) -> Self {
        self.part = Some(part);
        self
    }

    /// Sets `node_flags` bits.
    pub fn flags(mut self, flags: u16) -> Self {
        self.flags = flags;
        self
    }

    /// Adds a child.
    pub fn child(mut self, child: TNode) -> Self {
        self.children.push(child);
        self
    }

    /// Adds children.
    pub fn children(mut self, children: impl IntoIterator<Item = TNode>) -> Self {
        self.children.extend(children);
        self
    }

    fn flatten(self, out: &mut Vec<Node>, binds: &mut Vec<(u32, BindKind)>) {
        let at = out.len();
        out.push(Node {
            kind: self.kind,
            subtree: 1,
            style: self.style,
            part: self.part,
            flags: self.flags,
        });
        binds.extend(self.binds.into_iter().map(|b| (at as u32, b)));
        for c in self.children {
            c.flatten(out, binds);
        }
        let size = (out.len() - at) as u32;
        out[at].subtree = size;
    }
}

/// A piece of a text template for [`Kit::tpl_of`].
#[derive(Debug, Clone)]
pub enum TplPiece {
    /// Text copied as it is.
    Lit(String),
    /// An insert.
    Expr(Expr),
}

/// A path expression (`top.gold` as `path("top.gold")`).
pub fn path(p: &str) -> Expr {
    Expr::Path(p.split('.').map(str::to_string).collect())
}

/// Builds a package in memory.
#[derive(Debug, Default)]
pub struct Kit {
    b: PackageBuilder,
    cells: usize,
}

impl Kit {
    /// A builder with one viewport cell.
    pub fn new() -> Self {
        Self {
            b: PackageBuilder::new(),
            cells: 1,
        }
    }

    /// The manifest the kit writes.
    pub fn manifest() -> Manifest {
        Manifest {
            kind: Kind::Skin,
            id: "kit".into(),
            version: "1.0.0".into(),
            title_key: "kit.title".into(),
            profile: (1, 0),
            model: (1, 0),
            api: (1, 0),
            default: false,
            tool: Tool {
                converter: "testkit".into(),
                chrome: "0".into(),
                chrome_flags: "00".into(),
                fonttools: "0".into(),
            },
            themes: vec!["dark".into(), "light".into()],
            languages: vec!["en_US".into()],
            font_roles: FontRoles {
                sans: FaceFamily("Sans".into()),
                display: FaceFamily("Display".into()),
            },
            schema_hash: 0,
        }
    }

    /// Sets the viewport breakpoints in CSS px (a value at a breakpoint is in the upper cell). Call before
    /// adding sets: [`plain`](Self::plain) repeats its record over the cells.
    pub fn media(&mut self, width: &[f32], height: &[f32]) {
        let b = |v: &[f32]| {
            v.iter()
                .map(|&px| Bound {
                    px,
                    upper_inclusive: false,
                })
                .collect()
        };
        self.cells = (width.len() + 1) * (height.len() + 1);
        self.b.media(b(width), b(height));
    }

    /// The number of viewport cells.
    pub fn cells(&self) -> usize {
        self.cells
    }

    /// Adds a string; the id is provisional.
    pub fn str(&mut self, s: &str) -> StrId {
        self.b.str(s)
    }

    /// Adds a style record.
    pub fn record(&mut self, record: StyleRecord) -> StyleId {
        self.b.style_record(record)
    }

    /// Adds a style set; `table` has `cells * 2^conds` records, cell-major.
    pub fn set(&mut self, conds: Vec<Cond>, table: Vec<StyleId>) -> StyleSetId {
        self.b.style_set(StyleSet { conds, table })
    }

    /// A set without conditions that uses one record in every cell.
    pub fn plain(&mut self, record: StyleId) -> StyleSetId {
        self.b.style_set(StyleSet::plain(vec![record; self.cells]))
    }

    /// Sets the color tokens and themes.
    pub fn tokens(&mut self, tokens: Tokens) {
        self.b.tokens(tokens);
    }

    /// Adds a font face (the font file stored as it is, not compressed).
    pub fn font(&mut self, family: &str, weight: u16, italic: bool, file: &[u8]) {
        let family = self.b.str(family);
        let face = Face {
            family,
            weight,
            italic,
            role: None,
            ranges: vec![(0, 0x10_ffff)],
            blob: BlobRef::unplaced(BlobSection::Font, false),
            blake3: *blake3::hash(file).as_bytes(),
        };
        self.b.font(face, file, file.len() as u32);
    }

    /// Adds a template made of the given top-level nodes, with their bindings; the first template is the
    /// document.
    pub fn template(&mut self, name: &str, roots: Vec<TNode>) -> TemplateId {
        let name = self.b.str(name);
        let mut nodes = Vec::new();
        let mut binds = Vec::new();
        for r in roots {
            r.flatten(&mut nodes, &mut binds);
        }
        let tpl = self.b.template(Template { name, nodes });
        for (node, kind) in binds {
            self.b.binding(Binding { tpl, node, kind });
        }
        tpl
    }

    /// Adds an expression tree (`for`/`if` nodes and bindings refer to it).
    pub fn expr(&mut self, e: Expr) -> ExprId {
        self.b.expr(e)
    }

    /// Adds a text template.
    pub fn tpl(&mut self, segs: Vec<Seg>) -> TplId {
        self.b.tpl(segs)
    }

    /// A text template of literal pieces and expressions.
    pub fn tpl_of(&mut self, pieces: Vec<TplPiece>) -> TplId {
        let segs = pieces
            .into_iter()
            .map(|p| match p {
                TplPiece::Lit(s) => Seg::Lit(self.b.str(&s)),
                TplPiece::Expr(e) => Seg::Expr(self.b.expr(e)),
            })
            .collect();
        self.b.tpl(segs)
    }

    /// Declares a model path the skin reads and the type it expects.
    pub fn used_path(&mut self, path: &str, ty: TypeSig) {
        self.b.used_path(path, ty);
    }

    /// Declares a skin state path (without `state.`) and its default.
    pub fn state_default(&mut self, path: &str, value: Lit) {
        self.b.state_default(path, value);
    }

    /// Adds a skin text (`lang` must be a language of [`Kit::manifest`]: `en_US`).
    pub fn l10n(&mut self, lang: &str, key: &str, template: &str) {
        self.b.l10n(lang, key, template);
    }

    /// Adds mask icons of the given CSS sizes, packed side by side on one new 1x `A8` page (fully covered, a
    /// 1 px margin around each), and returns their ids in order. The ids are provisional like string ids:
    /// the package sorts pictures by path.
    pub fn mask_icons(&mut self, icons: &[(&str, [u16; 2])]) -> Vec<ImgId> {
        let width: u16 = icons.iter().map(|(_, [w, _])| w + 2).sum();
        let height: u16 = icons.iter().map(|(_, [_, h])| h + 2).max().unwrap_or(0);
        let mut pixels = vec![0u8; usize::from(width) * usize::from(height)];
        let mut x = 0u16;
        let mut rects = Vec::new();
        for (_, [w, h]) in icons {
            for row in 1..=*h {
                let at = usize::from(row) * usize::from(width) + usize::from(x) + 1;
                pixels[at..at + usize::from(*w)].fill(255);
            }
            rects.push([x, 0, w + 2, h + 2]);
            x += w + 2;
        }
        let page = Page {
            format: PageFormat::A8,
            scale: 1,
            size: [width, height],
            blob: BlobRef::unplaced(BlobSection::Imgs, false),
            blake3: *blake3::hash(&pixels).as_bytes(),
        };
        let page = self.b.image_page(page, &pixels, pixels.len() as u32);
        icons
            .iter()
            .zip(rects)
            .map(|((path, [w, h]), rect)| {
                let path = self.b.str(path);
                self.b.image(Img {
                    path,
                    size: [f32::from(*w), f32::from(*h)],
                    mask: true,
                    x1: Sprite { page, rect },
                    x2: None,
                })
            })
            .collect()
    }

    /// Writes and reads the package.
    pub fn finish(self) -> Result<Arc<Package>, PackageError> {
        let bytes = self.b.finish(Self::manifest())?;
        Package::read(Arc::from(bytes)).map(Arc::new)
    }
}
