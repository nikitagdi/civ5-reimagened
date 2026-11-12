//! Rectangle dumps: the boxes as the runtime laid them out, in the converter's rectangle dump format, so they can
//! be compared with Chrome's.
//!
//! One entry per element (its border box) and per text leaf (the content area of its text, like a DOM `Range`
//! rectangle); inline elements give the content area of their own font around their text. Positions are CSS px
//! in the page. A box under a `transform` (its own or an ancestor's, around each one's `transform-origin`)
//! is listed as the bounding box of its four corners through those transforms, as Chrome's
//! `getBoundingClientRect`; text leaves follow their element. Boxes without transforms are listed exactly as
//! laid out. Hidden nodes (`display: none`, service nodes and what is below them) are not listed.

use glam::{Affine2, Vec2};
use ui2_package::style::{Display, Position};

use crate::anchor::Anchors;
use crate::arena::{Arena, NodeClass, NodeId};
use crate::input::hit::transform_about;
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
    /// The map anchors and the device scale: a placed anchor is listed where the screen shows it.
    anchors: Option<(&'a Anchors, f32)>,
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

    /// Lists a node and its subtree. `origin`: the page position of the parent's border box (of the
    /// containing block for an out-of-flow box); `xf`: the transforms above, page px to page px.
    fn visit(&mut self, id: NodeId, origin: [f32; 2], xf: Affine2) {
        let Some(node) = self.arena.get(id) else {
            return;
        };
        if node.class() == NodeClass::Service
            || (node.class() != NodeClass::Text
                && self.record(id).is_some_and(|r| r.display == Display::None))
        {
            return;
        }
        // A map anchor the host placed: off the screen it is not listed; on it, its chunk's layout space goes
        // where the host put it (an anchor not placed yet is listed as laid out).
        let xf = match self.anchors {
            Some((a, k)) => match a.shown(id) {
                Some(false) => return,
                Some(true) => match a.placement(id, Vec2::from(origin), k) {
                    Some(p) => xf * p,
                    None => return,
                },
                None => xf,
            },
            None => xf,
        };
        let l = node.layout().unrounded;
        let abs = [origin[0] + l.location.x, origin[1] + l.location.y];
        // The node's own transform moves its box and everything inside it.
        let xf = match self.record(id) {
            Some(r) if node.class() != NodeClass::Text && !r.transform.is_empty() => {
                xf * transform_about(r, Vec2::from(abs), Vec2::new(l.size.width, l.size.height))
            }
            _ => xf,
        };
        if node.class() != NodeClass::Text {
            self.out.push((
                id,
                bounds(&xf, [abs[0], abs[1], l.size.width, l.size.height]),
            ));
        }
        if let Some(ifc) = node.ifc() {
            let cx = abs[0] + l.border.left + l.padding.left;
            let cy = abs[1] + l.border.top + l.padding.top;
            let cw =
                l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right;
            for r in ifc.rects(cw) {
                self.out
                    .push((r.node, bounds(&xf, [cx + r.x, cy + r.y, r.w, r.h])));
            }
            return;
        }
        for &c in node.children() {
            if !self.out_of_flow(c) {
                self.visit(c, abs, xf);
            }
        }
        for h in &node.layout().hoisted {
            if let Some(hid) = self.arena.id_at(usize::from(*h))
                && hid != id
            {
                self.visit(hid, abs, xf);
            }
        }
    }
}

/// The bounding box of a rectangle `[x, y, w, h]` through a transform, as `getBoundingClientRect` gives
/// it; the rectangle itself, untouched, without one.
fn bounds(xf: &Affine2, r: [f32; 4]) -> [f32; 4] {
    if *xf == Affine2::IDENTITY {
        return r;
    }
    let [x, y, w, h] = r;
    let corners = [
        Vec2::new(x, y),
        Vec2::new(x + w, y),
        Vec2::new(x, y + h),
        Vec2::new(x + w, y + h),
    ]
    .map(|c| xf.transform_point2(c));
    let lo = corners
        .iter()
        .fold(Vec2::splat(f32::INFINITY), |a, c| a.min(*c));
    let hi = corners
        .iter()
        .fold(Vec2::splat(f32::NEG_INFINITY), |a, c| a.max(*c));
    [lo.x, lo.y, hi.x - lo.x, hi.y - lo.y]
}

/// The dump of the whole document, or of the subtree of one node. Sorted by node name, then keys. With
/// `anchors` (and the device scale), placed map anchors are listed where the screen shows them.
pub(crate) fn dump<S: Source + ?Sized>(
    arena: &Arena,
    src: &S,
    index: &StyleIndex,
    under: Option<NodeId>,
    anchors: Option<(&Anchors, f32)>,
) -> Vec<ElementRect> {
    let mut ctx = Ctx {
        arena,
        src,
        index,
        anchors,
        out: Vec::new(),
    };
    for &root in arena.roots() {
        ctx.visit(root, [0.0, 0.0], Affine2::IDENTITY);
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
