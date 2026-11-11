//! Rectangle dumps: the boxes as the runtime laid them out, in the converter's rectangle dump format, so they can
//! be compared with Chrome's.
//!
//! One entry per element (its border box) and per text leaf (the content area of its text, like a DOM `Range`
//! rectangle); inline elements give the content area of their own font around their text. Positions are CSS px
//! in the page, before transforms (the top strip has none; transforms are applied with painting). Hidden nodes
//! (`display: none`, service nodes and what is below them) are not listed.

use ui2_package::style::{Display, Position};

use crate::arena::{Arena, NodeClass, NodeId};
use crate::instantiate::{self, Source};
use crate::style::StyleIndex;

/// One rectangle of a dump.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementRect {
    /// `<template>:<index>`, with `#k` for the k-th text leaf of an element.
    pub node: String,
    /// The list keys of the instances above the node, `/`-separated.
    pub keys: String,
    /// Left, CSS px.
    pub x: f32,
    /// Top, CSS px.
    pub y: f32,
    /// Width, CSS px.
    pub w: f32,
    /// Height, CSS px.
    pub h: f32,
}

fn round3(v: f32) -> f32 {
    (v * 1000.0).round() / 1000.0
}

struct Ctx<'a, S: Source + ?Sized> {
    arena: &'a Arena,
    src: &'a S,
    index: &'a StyleIndex,
    out: Vec<(NodeId, [f32; 4])>,
}

impl<S: Source + ?Sized> Ctx<'_, S> {
    fn record(&self, id: NodeId) -> Option<&ui2_package::style::StyleRecord> {
        let n = self.arena.get(id)?;
        Some(crate::style::node_record(self.index, self.src.styles(), n))
    }

    fn out_of_flow(&self, id: NodeId) -> bool {
        self.arena.get(id).is_some_and(|n| n.class().is_element())
            && self
                .record(id)
                .is_some_and(|r| r.position == Position::Absolute && r.display != Display::None)
    }

    fn visit(&mut self, id: NodeId, origin: [f32; 2]) {
        let Some(node) = self.arena.get(id) else {
            return;
        };
        if node.class() == NodeClass::Service
            || (node.class() != NodeClass::Text
                && self.record(id).is_some_and(|r| r.display == Display::None))
        {
            return;
        }
        let l = node.layout().unrounded;
        let abs = [origin[0] + l.location.x, origin[1] + l.location.y];
        if node.class() != NodeClass::Text {
            self.out
                .push((id, [abs[0], abs[1], l.size.width, l.size.height]));
        }
        if let Some(ifc) = node.ifc() {
            let cx = abs[0] + l.border.left + l.padding.left;
            let cy = abs[1] + l.border.top + l.padding.top;
            let cw =
                l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right;
            for r in ifc.rects(cw) {
                self.out.push((r.node, [cx + r.x, cy + r.y, r.w, r.h]));
            }
            return;
        }
        for &c in node.children() {
            if !self.out_of_flow(c) {
                self.visit(c, abs);
            }
        }
        for h in &node.layout().hoisted {
            if let Some(hid) = self.arena.id_at(usize::from(*h))
                && hid != id
            {
                self.visit(hid, abs);
            }
        }
    }
}

/// The dump of the whole document, or of the subtree of one node. Sorted by node name, then keys.
pub(crate) fn dump<S: Source + ?Sized>(
    arena: &Arena,
    src: &S,
    index: &StyleIndex,
    under: Option<NodeId>,
) -> Vec<ElementRect> {
    let mut ctx = Ctx {
        arena,
        src,
        index,
        out: Vec::new(),
    };
    for &root in arena.roots() {
        ctx.visit(root, [0.0, 0.0]);
    }
    let inside = |mut id: NodeId| {
        let Some(top) = under else {
            return true;
        };
        loop {
            if id == top {
                return true;
            }
            match arena.get(id).and_then(|n| n.parent()) {
                Some(p) => id = p,
                None => return false,
            }
        }
    };
    let mut rects: Vec<ElementRect> = ctx
        .out
        .into_iter()
        .filter(|(id, _)| inside(*id))
        .map(|(id, [x, y, w, h])| {
            let path = instantiate::node_path(arena, src, id);
            ElementRect {
                node: path.label,
                keys: path.keys,
                x: round3(x),
                y: round3(y),
                w: round3(w),
                h: round3(h),
            }
        })
        .collect();
    rects.sort_by(|a, b| (&a.node, &a.keys).cmp(&(&b.node, &b.keys)));
    rects
}
