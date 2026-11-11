//! Hit-testing: which node is under a point, in CSS px of the page.
//!
//! The layers are tested top-down, `window`, `hud`, `map` (the `tooltip` layer never takes input). Inside a
//! layer the chunks (children of the layer root, in document order) are tested last first, and inside a chunk
//! the boxes in reverse painting order: the same stacking walk the paint pass uses, so the box drawn on top is
//! the one hit. A box is hit when the point lies in its border box after its transforms and the scroll offsets
//! above it, inside every overflow clip above it and inside the `clip-path` polygon at or above it (one: the
//! outermost, as painting draws it). `pointer-events: none` and `visibility: hidden` skip the
//! box but not its children; layer roots are never targets (a click on an empty part of a layer reaches the
//! world). A map anchor's chunk is tested where the host placed it ([`crate::Runtime::set_anchor_screen`]),
//! and skipped while it is not placed or not visible.
//!
//! The geometry repeats what painting computes, in CSS px instead of device pixels: positions relative to the
//! containing block made absolute, scroll offsets of scroll containers, transforms around their origins.

use std::collections::HashMap;

use glam::{Affine2, Vec2};
use ui2_package::style::{
    Display, Len, Overflow, PointerEvents, Position, StyleRecord, TFn, Visibility,
};
use ui2_package::{Layer, NodeKind, NodeRef, node_flags};

use crate::anchor::Anchors;
use crate::arena::{Arena, NodeClass, NodeId};
use crate::instantiate;
use crate::paint::clip_path::{self, ClipPath};
use crate::paint::stacking;
use crate::style::{StyleIndex, node_record};
use ui2_draw::Polygon;

/// An axis-aligned rectangle in page space, by its edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Edges {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Edges {
    fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x0 && p.x < self.x1 && p.y >= self.y0 && p.y < self.y1
    }

    fn intersect(&self, o: &Edges) -> Edges {
        Edges {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }
}

/// What a node passes down to its children: the transform from layout space to page space, the
/// intersection of the overflow clips above and the `clip-path` polygon above (page space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HitState {
    xf: Affine2,
    clip: Option<Edges>,
    poly: Option<Polygon>,
}

impl HitState {
    const ROOT: HitState = HitState {
        xf: Affine2::IDENTITY,
        clip: None,
        poly: None,
    };
}

/// The result of a hit-test.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Hit {
    /// The node under the point.
    pub node: Option<NodeId>,
    /// The point in the node's border box (layout space, from its top left corner), CSS px.
    pub local: Vec2,
    /// The size of the node's border box, CSS px.
    pub size: Vec2,
    /// A visible modal screen of the `window` layer stopped the point before the lower layers.
    pub blocked: bool,
}

/// The layer of a template node kind, when it is a layer root.
pub(crate) fn layer_kind(pkg: &crate::Pkg, arena: &Arena, id: NodeId) -> Option<Layer> {
    let n = arena.get(id)?;
    match instantiate::template_node(pkg, n.tmpl())?.kind {
        NodeKind::Layer(l) => Some(l),
        _ => None,
    }
}

/// The layer roots of the document, in document order: top-level layer roots and layer roots that are
/// children of a top-level node. A top-level node that is no layer and has no layer child paints into the HUD
/// and is listed as a HUD root of itself (`(Hud, id, false)`); real layer roots are `(layer, id, true)`.
pub(crate) fn layer_roots(pkg: &crate::Pkg, arena: &Arena) -> Vec<(Layer, NodeId, bool)> {
    let mut out = Vec::new();
    for &r in arena.roots() {
        if let Some(l) = layer_kind(pkg, arena, r) {
            out.push((l, r, true));
            continue;
        }
        let before = out.len();
        if let Some(n) = arena.get(r) {
            for &c in n.children() {
                if let Some(l) = layer_kind(pkg, arena, c) {
                    out.push((l, c, true));
                }
            }
        }
        if out.len() == before {
            out.push((Layer::Hud, r, false));
        }
    }
    out
}

/// The chunk roots of a layer root (its children), or the node itself for a HUD root without a layer.
pub(crate) fn chunk_roots(arena: &Arena, root: NodeId, is_layer: bool) -> Vec<NodeId> {
    if !is_layer {
        return vec![root];
    }
    arena
        .get(root)
        .map_or_else(Vec::new, |n| n.children().to_vec())
}

fn resolve(l: Len, reference: f32) -> f32 {
    l.px + l.pct * reference / 100.0
}

/// The `transform` of a record for a border box at `pos` of `size` (CSS px), around its
/// `transform-origin`: the matrix painting uses, in CSS px instead of device pixels.
pub(crate) fn transform_about(r: &StyleRecord, pos: Vec2, size: Vec2) -> Affine2 {
    let origin = pos
        + Vec2::new(
            resolve(r.transform_origin.x, size.x),
            resolve(r.transform_origin.y, size.y),
        );
    let mut m = Affine2::from_translation(origin);
    for f in &r.transform {
        m *= match *f {
            TFn::Translate(x, y) => {
                Affine2::from_translation(Vec2::new(resolve(x, size.x), resolve(y, size.y)))
            }
            TFn::Rotate(deg) => Affine2::from_angle(deg.to_radians()),
            TFn::Scale(x, y) => Affine2::from_scale(Vec2::new(x, y)),
        };
    }
    m * Affine2::from_translation(-origin)
}

/// Page geometry of the nodes, computed on demand and cached for one query.
pub(crate) struct Geo<'a> {
    pub arena: &'a Arena,
    pub pkg: &'a crate::Pkg,
    pub index: &'a StyleIndex,
    abs: HashMap<u32, Vec2>,
    /// Map anchors and the device scale, to place anchor chunks on the screen.
    anchors: Option<(&'a Anchors, f32)>,
}

impl<'a> Geo<'a> {
    pub fn new(arena: &'a Arena, pkg: &'a crate::Pkg, index: &'a StyleIndex) -> Self {
        Self {
            arena,
            pkg,
            index,
            abs: HashMap::new(),
            anchors: None,
        }
    }

    /// Hit-tests placed map anchors where the host put them (device scale `k`).
    pub fn with_anchors(mut self, anchors: &'a Anchors, k: f32) -> Self {
        self.anchors = Some((anchors, k));
        self
    }

    /// For an anchor's node: `Some` with its placement (layout space to page space) while it is placed and
    /// visible, `Some(None)` otherwise; `None` for other nodes.
    fn anchor_placement(&mut self, id: NodeId) -> Option<Option<Affine2>> {
        let n = self.arena.get(id)?;
        let flagged = instantiate::template_node(self.pkg, n.tmpl())
            .is_some_and(|t| t.flags & node_flags::ANCHOR != 0);
        if !flagged {
            return None;
        }
        let loc = n.layout().unrounded.location;
        let Some((anchors, k)) = self.anchors else {
            return Some(None);
        };
        let origin = self.abs(id) - Vec2::new(loc.x, loc.y);
        Some(anchors.placement(id, origin, k))
    }

    pub fn record(&self, id: NodeId) -> Option<&'a StyleRecord> {
        let n = self.arena.get(id)?;
        Some(node_record(self.index, self.pkg.styles(), n))
    }

    /// The page position of a node's border box (before transforms), CSS px.
    pub fn abs(&mut self, id: NodeId) -> Vec2 {
        if let Some(p) = self.abs.get(&id.index()) {
            return *p;
        }
        let Some(n) = self.arena.get(id) else {
            return Vec2::ZERO;
        };
        let loc = n.layout().unrounded.location;
        let out_of_flow = n.class().is_element()
            && self
                .record(id)
                .is_some_and(|r| r.position == Position::Absolute);
        let mut base = n.parent();
        if out_of_flow {
            let mut at = n.parent();
            while let Some(an) = at.and_then(|a| self.arena.get(a)) {
                if an.layout().hoisted.contains(&id.taffy()) {
                    break;
                }
                at = an.parent();
            }
            if at.is_some() {
                base = at;
            }
        }
        let origin = match base {
            Some(b) => {
                let s = self.arena.get(b).map_or([0.0, 0.0], |bn| bn.scroll());
                self.abs(b) - Vec2::new(s[0], s[1])
            }
            None => Vec2::ZERO,
        };
        let p = origin + Vec2::new(loc.x, loc.y);
        self.abs.insert(id.index(), p);
        p
    }

    /// The border box size of a node, CSS px.
    pub fn size(&self, id: NodeId) -> Vec2 {
        self.arena.get(id).map_or(Vec2::ZERO, |n| {
            let s = n.layout().unrounded.size;
            Vec2::new(s.width, s.height)
        })
    }

    /// The transform of a node around its origin (identity without `transform`).
    fn own_transform(&mut self, id: NodeId, r: &StyleRecord) -> Affine2 {
        if r.transform.is_empty() {
            return Affine2::IDENTITY;
        }
        let pos = self.abs(id);
        let size = self.size(id);
        transform_about(r, pos, size)
    }

    /// The padding box of a node in layout space.
    fn padding_box(&mut self, id: NodeId) -> Edges {
        let pos = self.abs(id);
        let size = self.size(id);
        let b = self
            .arena
            .get(id)
            .map(|n| n.layout().unrounded.border)
            .unwrap_or(taffy::Rect::zero());
        Edges {
            x0: pos.x + b.left,
            y0: pos.y + b.top,
            x1: pos.x + size.x - b.right,
            y1: pos.y + size.y - b.bottom,
        }
    }

    /// The `clip-path` of a node in page space (`xf`: the node's layout space to page space). A polygon the
    /// paint pass cannot draw does not cut hits either.
    fn clip_path(&mut self, id: NodeId, r: &StyleRecord, xf: &Affine2) -> ClipPath {
        if r.clip_path.is_empty() {
            return ClipPath::None;
        }
        let pos = self.abs(id);
        let size = self.size(id);
        let map = |p: [f32; 2]| xf.transform_point2(pos + Vec2::from(p));
        clip_path::resolve(&r.clip_path, size.into(), map).unwrap_or(ClipPath::None)
    }

    /// The state a node passes to its children: its transform, its overflow clip and its clip-path.
    fn child_state(&mut self, id: NodeId, s: &HitState) -> HitState {
        let Some(r) = self.record(id) else {
            return *s;
        };
        let xf = s.xf * self.own_transform(id, r);
        // The outer polygon wins, as in painting.
        let poly = s.poly.or(match self.clip_path(id, r, &xf) {
            ClipPath::Poly(p) => Some(p),
            _ => None,
        });
        let mut clip = s.clip;
        if r.overflow_x != Overflow::Visible || r.overflow_y != Overflow::Visible {
            let pad = self.padding_box(id);
            // The clip in page space: the bounding box of the padding box through the transform (exact
            // without rotation, which the paint pass does not clip under either).
            let corners = [
                Vec2::new(pad.x0, pad.y0),
                Vec2::new(pad.x1, pad.y0),
                Vec2::new(pad.x0, pad.y1),
                Vec2::new(pad.x1, pad.y1),
            ]
            .map(|c| xf.transform_point2(c));
            let lo = corners
                .iter()
                .fold(Vec2::splat(f32::INFINITY), |a, c| a.min(*c));
            let hi = corners
                .iter()
                .fold(Vec2::splat(f32::NEG_INFINITY), |a, c| a.max(*c));
            let own = Edges {
                x0: lo.x,
                y0: lo.y,
                x1: hi.x,
                y1: hi.y,
            };
            clip = Some(match s.clip {
                Some(outer) => outer.intersect(&own),
                None => own,
            });
        }
        HitState { xf, clip, poly }
    }

    /// The boxes of a stacking context in painting order (what is painted last is last), with the state their
    /// parents pass down. Mirrors the paint pass's walk.
    fn context(&mut self, id: NodeId, s: HitState, out: &mut Vec<(NodeId, HitState)>) {
        let hidden = self
            .arena
            .get(id)
            .is_none_or(|n| n.class() == NodeClass::Service)
            || self.record(id).is_none_or(|r| r.display == Display::None);
        if hidden {
            return;
        }
        // A clip-path that encloses nothing hides the node and its subtree.
        if let Some(r) = self.record(id)
            && !r.clip_path.is_empty()
        {
            let xf = s.xf * self.own_transform(id, r);
            if self.clip_path(id, r, &xf) == ClipPath::Empty {
                return;
            }
        }
        out.push((id, s));
        let inner = self.child_state(id, &s);
        let arena = self.arena;
        let (index, pkg) = (self.index, self.pkg);
        let record = |n: NodeId| {
            arena
                .get(n)
                .map(|node| node_record(index, pkg.styles(), node))
        };
        let content_root = |n: NodeId| arena.get(n).is_some_and(|node| node.ifc().is_some());
        let phases = stacking::phases(
            arena,
            id,
            inner,
            &record,
            &mut |n, st| self.child_state(n, st),
            &content_root,
        );
        for (n, st) in phases.negative {
            self.context(n, st, out);
        }
        out.extend(phases.flow);
        for (n, st) in phases.zero.into_iter().chain(phases.positive) {
            self.context(n, st, out);
        }
    }

    /// Whether a node can be the target of input at all (its children may be even when it cannot).
    fn hittable(&mut self, id: NodeId) -> bool {
        let Some(n) = self.arena.get(id) else {
            return false;
        };
        if !matches!(n.class(), NodeClass::Element | NodeClass::Placeholder) {
            return false;
        }
        if self.anchor_placement(id).is_some_and(|p| p.is_none()) {
            return false;
        }
        self.record(id).is_some_and(|r| {
            r.pointer_events != PointerEvents::None && r.visibility != Visibility::Hidden
        })
    }

    /// Whether a node, or one of its ancestors, is an anchor (anchors are skipped with their subtrees until the
    /// host places them).
    fn under_anchor(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(n) = at.and_then(|a| self.arena.get(a)) {
            if instantiate::template_node(self.pkg, n.tmpl())
                .is_some_and(|t| t.flags & node_flags::ANCHOR != 0)
            {
                return true;
            }
            at = n.parent();
        }
        false
    }

    /// The point in a node's border box when the point is on it (after its transform, inside the clips above).
    fn on(&mut self, id: NodeId, s: &HitState, p: Vec2) -> Option<Vec2> {
        if s.clip.is_some_and(|c| !c.contains(p)) {
            return None;
        }
        if s.poly.is_some_and(|q| !clip_path::contains(&q, p)) {
            return None;
        }
        let r = self.record(id)?;
        let m = s.xf * self.own_transform(id, r);
        if s.poly.is_none()
            && let ClipPath::Poly(q) = self.clip_path(id, r, &m)
            && !clip_path::contains(&q, p)
        {
            return None;
        }
        if m.matrix2.determinant() == 0.0 {
            return None;
        }
        let q = m.inverse().transform_point2(p);
        let pos = self.abs(id);
        let size = self.size(id);
        let local = q - pos;
        (local.x >= 0.0 && local.y >= 0.0 && local.x < size.x && local.y < size.y).then_some(local)
    }

    /// The topmost hittable node of a chunk under a point.
    fn hit_chunk(&mut self, root: NodeId, p: Vec2) -> Option<(NodeId, Vec2)> {
        // A placed anchor's chunk is where the host put it; an anchor deeper in a chunk is never placed.
        let start = match self.anchor_placement(root) {
            Some(Some(xf)) => HitState {
                xf,
                clip: None,
                poly: None,
            },
            Some(None) => return None,
            None if self.under_anchor(root) => return None,
            None => HitState::ROOT,
        };
        let mut order = Vec::new();
        self.context(root, start, &mut order);
        for (n, s) in order.into_iter().rev() {
            if !self.hittable(n) {
                continue;
            }
            if let Some(local) = self.on(n, &s, p) {
                return Some((n, local));
            }
        }
        None
    }

    /// The node under a page point. `modal` says that a screen of the `window` layer is visible: what the
    /// window layer does not take is then blocked.
    pub fn hit(&mut self, p: Vec2, modal: bool) -> Hit {
        let roots = layer_roots(self.pkg, self.arena);
        for layer in [Layer::Window, Layer::Hud, Layer::Map] {
            for &(l, root, is_layer) in roots.iter().rev() {
                if l != layer {
                    continue;
                }
                for chunk in chunk_roots(self.arena, root, is_layer).into_iter().rev() {
                    if let Some((node, local)) = self.hit_chunk(chunk, p) {
                        return Hit {
                            node: Some(node),
                            local,
                            size: self.size(node),
                            blocked: false,
                        };
                    }
                }
            }
            if layer == Layer::Window && modal {
                return Hit {
                    blocked: true,
                    ..Hit::default()
                };
            }
        }
        Hit::default()
    }

    /// Whether a node and all its ancestors are displayed and visible.
    pub fn shown(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        let mut any = false;
        while let Some(a) = at {
            let Some(n) = self.arena.get(a) else {
                return false;
            };
            if n.class() != NodeClass::Text {
                let Some(r) = self.record(a) else {
                    return false;
                };
                if r.display == Display::None || r.visibility == Visibility::Hidden {
                    return false;
                }
            }
            any = true;
            at = n.parent();
        }
        any
    }

    /// The visible screens (`data-screen`) of the `window` layer, in document order. `screens` is the
    /// package's list of screen nodes, sorted.
    pub fn visible_screens(&self, screens: &[(NodeRef, std::sync::Arc<str>)]) -> Vec<NodeId> {
        let mut out = Vec::new();
        if screens.is_empty() {
            return out;
        }
        for (l, root, _) in layer_roots(self.pkg, self.arena) {
            if l != Layer::Window {
                continue;
            }
            let mut stack = vec![root];
            while let Some(id) = stack.pop() {
                let Some(n) = self.arena.get(id) else {
                    continue;
                };
                let is_screen = screens.binary_search_by(|(r, _)| r.cmp(&n.tmpl())).is_ok();
                if is_screen && self.shown(id) {
                    out.push(id);
                    continue;
                }
                stack.extend(n.children().iter().rev().copied());
            }
        }
        out
    }

    /// The nearest scroll container (`overflow: auto | scroll`) at or above a node.
    pub fn scroller(&self, id: NodeId) -> Option<NodeId> {
        let mut at = Some(id);
        while let Some(a) = at {
            let n = self.arena.get(a)?;
            if n.class().is_element()
                && self.record(a).is_some_and(|r| {
                    matches!(r.overflow_x, Overflow::Auto | Overflow::Scroll)
                        || matches!(r.overflow_y, Overflow::Auto | Overflow::Scroll)
                })
            {
                return Some(a);
            }
            at = n.parent();
        }
        None
    }
}

/// How far a scroll container can scroll: its content beyond its padding box, CSS px.
pub(crate) fn scroll_range(arena: &Arena, id: NodeId) -> Vec2 {
    let Some(n) = arena.get(id) else {
        return Vec2::ZERO;
    };
    let l = &n.layout().unrounded;
    Vec2::new(l.scroll_width(), l.scroll_height())
}
