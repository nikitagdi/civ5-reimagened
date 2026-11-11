//! `TMPL`: node templates. Template 0 is the document; the others are the bodies of `for` and
//! `if` nodes and of tooltips.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::{ARefId, ExprId, ImgId, StrId, StyleSetId, TemplateId, TplId};
use crate::profile::Tag;

/// `Node::flags` bits.
pub mod node_flags {
    /// The node can take keyboard focus.
    pub const FOCUSABLE: u16 = 1;
    /// The node scrolls its content.
    pub const SCROLL: u16 = 2;
    /// The node carries a `data-control` behaviour.
    pub const CONTROL: u16 = 4;
    /// The node is a map anchor (`data-anchor`).
    pub const ANCHOR: u16 = 8;
    /// Every bit this version defines.
    pub const ALL: u16 = FOCUSABLE | SCROLL | CONTROL | ANCHOR;
}

/// The four layers of the document, bottom to top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layer {
    /// Over the game map.
    Map,
    /// The HUD.
    Hud,
    /// Windows and modal screens.
    Window,
    /// Tooltips.
    Tooltip,
}

/// Where a text leaf gets its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextSrc {
    /// A fixed string.
    Static(StrId),
    /// A localized string of the skin's `L10N` table.
    L10n {
        /// The `L10N` key.
        key: StrId,
        /// Argument expressions.
        args: Vec<ExprId>,
    },
    /// A text template with `{{ }}` inserts.
    Bound(TplId),
}

/// Where a picture comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageSrc {
    /// A picture of the package.
    Pkg(ImgId),
    /// An icon; a mask icon is painted with the node's `color`.
    Icon {
        /// The picture.
        img: ImgId,
        /// A mask (alpha only).
        mask: bool,
    },
    /// A game asset reference.
    Asset(ARefId),
    /// A game asset chosen by an expression.
    Bound(ExprId),
}

/// What a node is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// An element box.
    Box {
        /// The element.
        tag: Tag,
    },
    /// The root of a layer.
    Layer(Layer),
    /// An anonymous text leaf, styled like its parent.
    Text(TextSrc),
    /// A picture leaf.
    Image(ImageSrc),
    /// A text input.
    Input {
        /// The placeholder's `L10N` key.
        placeholder: Option<StrId>,
    },
    /// A list: the body template is instantiated per item. No box of its own.
    For {
        /// The body, a template after the current one.
        tpl: TemplateId,
        /// The item variable.
        item: StrId,
        /// The index variable.
        index: Option<StrId>,
        /// The list expression.
        list: ExprId,
        /// The key expression of an item.
        key: ExprId,
        /// A virtual list.
        virt: bool,
        /// The sort key.
        sort: Option<ExprId>,
        /// Whether the sort is descending.
        sort_desc: Option<ExprId>,
        /// The filter.
        filter: Option<ExprId>,
    },
    /// A conditional subtree. No box of its own.
    If {
        /// The body, a template after the current one.
        tpl: TemplateId,
        /// The condition.
        cond: ExprId,
    },
}

/// One node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// What the node is.
    pub kind: NodeKind,
    /// The number of nodes in its subtree, itself included: the next sibling is at `i + subtree`.
    pub subtree: u32,
    /// The styles of the node, one record per cell and condition mask.
    pub style: StyleSetId,
    /// The stable part name (`data-part`).
    pub part: Option<StrId>,
    /// `node_flags` bits.
    pub flags: u16,
}

/// A template: nodes in document (preorder) order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// The template's name, for diagnostics.
    pub name: StrId,
    /// The nodes, preorder.
    pub nodes: Vec<Node>,
}

/// The section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Templates {
    /// Template 0 is the document.
    pub items: Vec<Template>,
}

/// The sizes of the tables a template refers to.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub strs: usize,
    pub style_sets: usize,
    pub exprs: usize,
    pub tpls: usize,
    pub imgs: usize,
    pub arefs: usize,
}

impl Templates {
    /// Checks every index and the subtree structure.
    pub(crate) fn validate(&self, at: SectionTag, lim: &Limits) -> Result<(), PackageError> {
        let bad = |what: String| Err(PackageError::invalid(at, what));
        if u32::try_from(self.items.len()).is_err() {
            return bad("too many templates".into());
        }
        for (ti, tpl) in self.items.iter().enumerate() {
            if tpl.name.index() >= lim.strs {
                return bad(format!("template {ti}: name is not in the string table"));
            }
            if u32::try_from(tpl.nodes.len()).is_err() {
                return bad(format!("template {ti}: too many nodes"));
            }
            check_tiling(&tpl.nodes).map_err(|n| {
                PackageError::invalid(
                    at,
                    format!("template {ti} node {n}: subtree is inconsistent"),
                )
            })?;
            for (ni, node) in tpl.nodes.iter().enumerate() {
                let where_ = |what: &str| format!("template {ti} node {ni}: {what}");
                if node.flags & !node_flags::ALL != 0 {
                    return bad(where_("unknown flag bits"));
                }
                if node.part.is_some_and(|p| p.index() >= lim.strs) {
                    return bad(where_("part is not in the string table"));
                }
                let leaf = !matches!(node.kind, NodeKind::Box { .. } | NodeKind::Layer(_));
                if leaf && node.subtree != 1 {
                    return bad(where_("a leaf node has children"));
                }
                let service = matches!(node.kind, NodeKind::For { .. } | NodeKind::If { .. });
                if !service && node.style.index() >= lim.style_sets {
                    return bad(where_("style set is out of range"));
                }
                let str_ok = |s: StrId| s.index() < lim.strs;
                let expr_ok = |e: ExprId| e.index() < lim.exprs;
                let ok = match &node.kind {
                    NodeKind::Box { .. } | NodeKind::Layer(_) => true,
                    NodeKind::Text(TextSrc::Static(s)) => str_ok(*s),
                    NodeKind::Text(TextSrc::L10n { key, args }) => {
                        str_ok(*key) && args.iter().all(|e| expr_ok(*e))
                    }
                    NodeKind::Text(TextSrc::Bound(t)) => t.index() < lim.tpls,
                    NodeKind::Image(ImageSrc::Pkg(i))
                    | NodeKind::Image(ImageSrc::Icon { img: i, .. }) => i.index() < lim.imgs,
                    NodeKind::Image(ImageSrc::Asset(a)) => a.index() < lim.arefs,
                    NodeKind::Image(ImageSrc::Bound(e)) => expr_ok(*e),
                    NodeKind::Input { placeholder } => placeholder.is_none_or(str_ok),
                    NodeKind::For {
                        tpl,
                        item,
                        index,
                        list,
                        key,
                        sort,
                        sort_desc,
                        filter,
                        ..
                    } => {
                        if tpl.index() <= ti || tpl.index() >= self.items.len() {
                            return bad(where_("a body template must come after the current one"));
                        }
                        str_ok(*item)
                            && index.is_none_or(str_ok)
                            && expr_ok(*list)
                            && expr_ok(*key)
                            && sort.is_none_or(expr_ok)
                            && sort_desc.is_none_or(expr_ok)
                            && filter.is_none_or(expr_ok)
                    }
                    NodeKind::If { tpl, cond } => {
                        if tpl.index() <= ti || tpl.index() >= self.items.len() {
                            return bad(where_("a body template must come after the current one"));
                        }
                        expr_ok(*cond)
                    }
                };
                if !ok {
                    return bad(where_("an index is out of range"));
                }
            }
        }
        Ok(())
    }
}

/// The subtree sizes must tile the node list: every node's subtree ends inside its parent's, and
/// the roots cover the list exactly. Returns the first bad node.
fn check_tiling(nodes: &[Node]) -> Result<(), usize> {
    // Ends of the open subtrees, innermost last.
    let mut open: Vec<usize> = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        while open.last() == Some(&i) {
            open.pop();
        }
        let size = node.subtree as usize;
        if size == 0 {
            return Err(i);
        }
        let end = i.checked_add(size).ok_or(i)?;
        if end > nodes.len() || open.last().is_some_and(|&parent_end| end > parent_end) {
            return Err(i);
        }
        if size > 1 {
            open.push(end);
        }
    }
    Ok(())
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Template {
    fn remap(&mut self, r: &crate::remap::Remap) {
        r.str(&mut self.name);
        for node in &mut self.nodes {
            r.opt_str(&mut node.part);
            match &mut node.kind {
                NodeKind::Box { .. } | NodeKind::Layer(_) => {}
                NodeKind::Text(TextSrc::Static(s)) => r.str(s),
                NodeKind::Text(TextSrc::L10n { key, .. }) => r.str(key),
                NodeKind::Text(TextSrc::Bound(_)) => {}
                NodeKind::Image(ImageSrc::Pkg(i))
                | NodeKind::Image(ImageSrc::Icon { img: i, .. }) => r.img(i),
                NodeKind::Image(ImageSrc::Asset(a)) => r.aref(a),
                NodeKind::Image(ImageSrc::Bound(_)) => {}
                NodeKind::Input { placeholder } => r.opt_str(placeholder),
                NodeKind::For { item, index, .. } => {
                    r.str(item);
                    r.opt_str(index);
                }
                NodeKind::If { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(kind: NodeKind) -> Node {
        Node {
            kind,
            subtree: 1,
            style: StyleSetId(0),
            part: None,
            flags: 0,
        }
    }

    fn boxed(subtree: u32) -> Node {
        Node {
            subtree,
            ..leaf(NodeKind::Box { tag: Tag::Div })
        }
    }

    #[test]
    fn tiling_accepts_a_forest_and_rejects_overruns() {
        let nodes = [boxed(3), boxed(2), boxed(1), boxed(1)];
        assert_eq!(check_tiling(&nodes), Ok(()));
        // The child claims more nodes than its parent has.
        let nodes = [boxed(2), boxed(3), boxed(1)];
        assert_eq!(check_tiling(&nodes), Err(1));
        let nodes = [boxed(0)];
        assert_eq!(check_tiling(&nodes), Err(0));
        let nodes = [boxed(5)];
        assert_eq!(check_tiling(&nodes), Err(0));
        assert_eq!(check_tiling(&[]), Ok(()));
    }
}
