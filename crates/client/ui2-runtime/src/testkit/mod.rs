//! In-memory skin packages for tests (feature `testkit`).
//!
//! [`Kit`] wraps the package builder with defaults (manifest, one viewport cell) and builds templates from
//! nested [`TNode`]s, computing the preorder list and subtree sizes. String ids returned while building are
//! provisional: the finished package sorts its string table, so look strings up again with
//! `Package::str_id` after [`Kit::finish`].

use std::sync::Arc;

use ui2_package::{
    Bound, Cond, FaceFamily, FontRoles, Kind, Layer, Manifest, Node, NodeKind, Package,
    PackageBuilder, PackageError, StrId, StyleId, StyleRecord, StyleSet, StyleSetId, Tag, Template,
    TemplateId, TextSrc, Tokens, Tool,
};

/// A template node with its children, for [`Kit::template`].
#[derive(Debug, Clone)]
pub struct TNode {
    kind: NodeKind,
    style: StyleSetId,
    part: Option<StrId>,
    flags: u16,
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
            children: Vec::new(),
        }
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

    fn flatten(self, out: &mut Vec<Node>) {
        let at = out.len();
        out.push(Node {
            kind: self.kind,
            subtree: 1,
            style: self.style,
            part: self.part,
            flags: self.flags,
        });
        for c in self.children {
            c.flatten(out);
        }
        let size = (out.len() - at) as u32;
        out[at].subtree = size;
    }
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

    /// Adds a template made of the given top-level nodes; the first template is the document.
    pub fn template(&mut self, name: &str, roots: Vec<TNode>) -> TemplateId {
        let name = self.b.str(name);
        let mut nodes = Vec::new();
        for r in roots {
            r.flatten(&mut nodes);
        }
        self.b.template(Template { name, nodes })
    }

    /// Writes and reads the package.
    pub fn finish(self) -> Result<Arc<Package>, PackageError> {
        let bytes = self.b.finish(Self::manifest())?;
        Package::read(Arc::from(bytes)).map(Arc::new)
    }
}
