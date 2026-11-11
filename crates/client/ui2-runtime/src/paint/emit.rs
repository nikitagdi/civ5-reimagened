//! Building the draw list of one chunk.
//!
//! Positions come from layout in CSS px relative to each node's containing block; they are made absolute
//! (page px) and scaled by `k` into device pixels. Box edges that no transform touches snap to device pixels
//! like Chrome's, and border widths are at least one device pixel, floored; glyphs keep the positions of
//! shaping. Colors resolve their theme tokens here and take the opacity of the node and its ancestors.
//!
//! An element paints, in this order: its outer box shadows (the last listed first), its background color
//! (inside the `background-clip` box of the bottom layer), its `background-image` layers (the bottom layer
//! first: a box per gradient layer, a picture per tile of a `url()` layer), its inset shadows (the last
//! listed first; without blur as bands inside the padding box, with blur as inset shadow items of the padding
//! box), its border or, when it has one, its `border-image` (a 9-slice of a package picture over the border
//! box), and its outline (a border-only box grown by the outline offset and width). Text paints each piece's
//! text shadows (the last listed first: the glyphs again, moved, in the shadow's color, blurred when the
//! shadow has a blur) right under the piece.
//!
//! Pictures of `url()` layers and `border-image-source` are found by their path in the skin among the
//! package's pictures (`IMGS`), as sprites of its `Rgba8` pages.
//!
//! Clips: the overflow clips above an item are intersected into one rectangle; a `clip-path` polygon cuts its
//! node (box, shadows, outline) and the whole subtree, in the same clip as the rectangle.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Affine2, Vec2};
use ui2_draw::{
    Border, BoxItem, ChunkBuilder, ChunkItems, Clip, Color, EllipticBox, ExtTexture, Fill,
    GlyphPos, GlyphShadow, Glyphs, Gradient, GradientStop, ImageItem, ImageRef, Item, LayerId,
    MaskItem, NineSlice, Polygon, Radii, Rect, ShadowItem, css_corner_angle, css_linear_line,
};
use ui2_package::style::{
    BgBox, BgImage, BgRepeat, BgSize, BorderStyle, ColorV, GradDir, ImageRepeat, ImageWidth, Len,
    LenAuto, OutlineStyle, Overflow, Position, Repeat, Side, Stop, StyleRecord, TFn, Visibility,
};
use ui2_package::{ImageSrc, Layer, NodeKind, PageFormat, StrId};

use crate::arena::{Arena, NodeClass, NodeId};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;
use crate::paint::clip_path::{self, ClipPath};
use crate::paint::stacking;
use crate::style::{StyleEnv, StyleIndex, node_record, tokens};
use crate::{AssetResolver, ExtImage};

/// What painting reads besides the arena.
pub(crate) struct PaintCtx<'a> {
    pub pkg: &'a crate::Pkg,
    pub index: &'a StyleIndex,
    pub env: &'a StyleEnv,
    /// Running transitions: an animated node is painted with its current values.
    pub anim: &'a crate::anim::Animator,
    /// The asset references bound pictures show now (`data-bind-src`).
    pub srcs: &'a HashMap<NodeId, Arc<str>>,
    /// Where game asset pictures are found (the host's texture ids).
    pub assets: Option<&'a AssetResolver>,
    /// Map anchors: the slots their chunks are placed with.
    pub anchors: &'a crate::anchor::Anchors,
    /// Text fields: what they show and the focused one's selection and caret.
    pub fields: &'a crate::input::text_edit::Fields,
}

impl<'a> PaintCtx<'a> {
    /// The record a node is painted with: the animated one while a transition runs.
    pub fn record(&self, id: NodeId, n: &'a crate::arena::Node) -> &'a StyleRecord {
        self.anim
            .record(id)
            .unwrap_or_else(|| node_record(self.index, self.pkg.styles(), n))
    }
}

/// A clip in chunk space: a rectangle with a corner radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ClipRect {
    rect: Rect,
    radius: f32,
}

/// What a node passes down to its children.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Inherit {
    /// Device-pixel item space to chunk space.
    xf: Affine2,
    /// The intersection of the overflow clips above.
    clip: Option<ClipRect>,
    /// The `clip-path` polygon above, in chunk space.
    poly: Option<Polygon>,
    /// The product of the opacities above.
    alpha: f32,
}

impl Inherit {
    const ROOT: Inherit = Inherit {
        xf: Affine2::IDENTITY,
        clip: None,
        poly: None,
        alpha: 1.0,
    };
}

/// What one chunk build produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Emitted {
    pub items: u32,
    pub glyphs: u32,
}

/// The layer a chunk root paints into: the layer of its layer root (the HUD for a root outside layers).
pub(crate) fn layer_of(arena: &Arena, pkg: &crate::Pkg, root: NodeId) -> LayerId {
    let mut at = Some(root);
    while let Some(id) = at {
        let Some(n) = arena.get(id) else {
            break;
        };
        if let Some(NodeKind::Layer(l)) = instantiate::template_node(pkg, n.tmpl()).map(|t| &t.kind)
        {
            return match l {
                Layer::Map => LayerId::Map,
                Layer::Hud => LayerId::Hud,
                Layer::Window => LayerId::Window,
                Layer::Tooltip => LayerId::Tooltip,
            };
        }
        at = n.parent();
    }
    LayerId::Hud
}

fn with_alpha(c: [u8; 4], alpha: f32) -> Color {
    // SIMPLIFIED(SIMP-226): opacity multiplies into the alpha of each item of the node and its descendants; there is no group opacity (overlapping children of a translucent node show through each other)
    let a = (f32::from(c[3]) * alpha.clamp(0.0, 1.0)).round() as u8;
    Color([c[0], c[1], c[2], a])
}

fn resolve(l: Len, reference: f32) -> f32 {
    l.px + l.pct * reference / 100.0
}

/// Builds the items of a chunk.
pub(crate) struct Emitter<'a> {
    arena: &'a Arena,
    cx: &'a PaintCtx<'a>,
    errors: &'a mut ErrorSink,
    k: f32,
    out: ChunkBuilder,
    stats: Emitted,
    abs: HashMap<u32, [f32; 2]>,
    /// The page point (CSS px) that is the chunk's origin: zero, or for a map anchor the origin of its
    /// root's containing block.
    origin: [f32; 2],
}

impl<'a> Emitter<'a> {
    pub fn new(arena: &'a Arena, cx: &'a PaintCtx<'a>, errors: &'a mut ErrorSink) -> Self {
        Self {
            arena,
            cx,
            errors,
            k: cx.env.scale.k,
            out: ChunkBuilder::new(),
            stats: Emitted::default(),
            abs: HashMap::new(),
            origin: [0.0, 0.0],
        }
    }

    /// Whether a node is marked as a map anchor in its template.
    fn is_anchor_node(&self, id: NodeId) -> bool {
        self.arena
            .get(id)
            .and_then(|n| instantiate::template_node(self.cx.pkg, n.tmpl()))
            .is_some_and(|t| t.flags & ui2_package::node_flags::ANCHOR != 0)
    }

    fn record(&self, id: NodeId) -> Option<&'a StyleRecord> {
        let n = self.arena.get(id)?;
        Some(self.cx.record(id, n))
    }

    fn color(&self, c: ColorV, alpha: f32) -> Color {
        with_alpha(
            tokens::resolve(self.cx.pkg.tokens(), self.cx.env.theme, c),
            alpha,
        )
    }

    fn unsupported(&mut self, id: NodeId, what: &str) {
        let path = instantiate::node_path(self.arena, self.cx.pkg, id);
        self.errors.report(UiError::at(
            &path,
            ErrorCode::Paint,
            format!("{what} is not drawn yet"),
        ));
    }

    /// Reports a paint problem of a node (once per node and kind).
    fn problem(&mut self, id: NodeId, msg: String) {
        let path = instantiate::node_path(self.arena, self.cx.pkg, id);
        self.errors
            .report(UiError::at(&path, ErrorCode::Paint, msg));
    }

    /// The page position of a node's border box, CSS px.
    fn abs(&mut self, id: NodeId) -> [f32; 2] {
        if let Some(p) = self.abs.get(&id.index()) {
            return *p;
        }
        let Some(n) = self.arena.get(id) else {
            return [0.0, 0.0];
        };
        let loc = n.layout().unrounded.location;
        let out_of_flow = n.class().is_element()
            && self
                .record(id)
                .is_some_and(|r| r.position == Position::Absolute);
        // An absolutely positioned box is placed in its containing block: the ancestor that hoisted it.
        let mut base = n.parent();
        if out_of_flow {
            let mut at = n.parent();
            while let Some(a) = at.and_then(|a| self.arena.get(a).map(|an| (a, an))) {
                if a.1.layout().hoisted.contains(&id.taffy()) {
                    break;
                }
                at = a.1.parent();
            }
            if at.is_some() {
                base = at;
            }
        }
        // A scroll container shifts the boxes it is the base of by its scroll offset.
        let origin = match base {
            Some(b) => {
                let o = self.abs(b);
                let s = self.arena.get(b).map_or([0.0, 0.0], |bn| bn.scroll());
                [o[0] - s[0], o[1] - s[1]]
            }
            None => [0.0, 0.0],
        };
        let p = [origin[0] + loc.x, origin[1] + loc.y];
        self.abs.insert(id.index(), p);
        p
    }

    /// The border box of a node in device px of the chunk (unsnapped).
    fn border_box(&mut self, id: NodeId) -> Rect {
        let [ax, ay] = self.abs(id);
        let (x, y) = (ax - self.origin[0], ay - self.origin[1]);
        let size = self
            .arena
            .get(id)
            .map_or(taffy::Size::ZERO, |n| n.layout().unrounded.size);
        let k = self.k;
        Rect::new(x * k, y * k, size.width * k, size.height * k)
    }

    /// Snaps a rectangle's edges to device pixels when no transform moves it.
    fn snap(rect: Rect, xf: &Affine2) -> Rect {
        if *xf != Affine2::IDENTITY {
            return rect;
        }
        let x0 = rect.x.round();
        let y0 = rect.y.round();
        let x1 = (rect.x + rect.w).round();
        let y1 = (rect.y + rect.h).round();
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    fn width(&self, w: Len) -> f32 {
        if w.px <= 0.0 {
            0.0
        } else {
            (w.px * self.k).floor().max(1.0)
        }
    }

    /// The transform of a node around its origin, in device px (identity without `transform`).
    fn own_transform(&mut self, id: NodeId, r: &StyleRecord) -> Affine2 {
        if r.transform.is_empty() {
            return Affine2::IDENTITY;
        }
        let b = self.border_box(id);
        let k = self.k;
        let origin = Vec2::new(
            b.x + resolve(r.transform_origin.x, b.w / k) * k,
            b.y + resolve(r.transform_origin.y, b.h / k) * k,
        );
        let mut m = Affine2::from_translation(origin);
        for f in &r.transform {
            m *= match *f {
                TFn::Translate(x, y) => Affine2::from_translation(Vec2::new(
                    resolve(x, b.w / k) * k,
                    resolve(y, b.h / k) * k,
                )),
                TFn::Rotate(deg) => Affine2::from_angle(deg.to_radians()),
                TFn::Scale(x, y) => Affine2::from_scale(Vec2::new(x, y)),
            };
        }
        m * Affine2::from_translation(-origin)
    }

    /// The `clip-path` of a node in chunk space, `xf` being the node's item space to chunk space.
    fn clip_path(&mut self, id: NodeId, r: &StyleRecord, xf: &Affine2) -> ClipPath {
        if r.clip_path.is_empty() {
            return ClipPath::None;
        }
        let b = self.border_box(id);
        let k = self.k;
        let map = |p: [f32; 2]| xf.transform_point2(Vec2::new(b.x + p[0] * k, b.y + p[1] * k));
        match clip_path::resolve(&r.clip_path, [b.w / k, b.h / k], map) {
            Ok(c) => c,
            Err(what) => {
                self.unsupported(id, what);
                ClipPath::None
            }
        }
    }

    /// The polygon that cuts a node: the one above it, else its own.
    fn merge_poly(&mut self, id: NodeId, above: Option<Polygon>, own: ClipPath) -> Option<Polygon> {
        match (above, own) {
            (Some(a), ClipPath::Poly(_)) => {
                // SIMPLIFIED(SIMP-227): one polygon clip per item: under a clip-path a nested clip-path does not cut (the outer polygon does)
                self.unsupported(id, "a clip-path inside another clip-path");
                Some(a)
            }
            (Some(a), _) => Some(a),
            (None, ClipPath::Poly(p)) => Some(p),
            (None, _) => None,
        }
    }

    /// The state a node passes to its children: its transform, its opacity, its overflow clip and its
    /// `clip-path`.
    fn child_state(&mut self, id: NodeId, s: &Inherit) -> Inherit {
        let Some(r) = self.record(id) else {
            return *s;
        };
        let xf = s.xf * self.own_transform(id, r);
        let alpha = s.alpha * r.opacity;
        let own = self.clip_path(id, r, &xf);
        let poly = self.merge_poly(id, s.poly, own);
        let mut clip = s.clip;
        if r.overflow_x != Overflow::Visible || r.overflow_y != Overflow::Visible {
            let pad = self.padding_box(id);
            // SIMPLIFIED(SIMP-227): overflow clips are rectangles (rounded only for the innermost one when it lies inside the clips above); nested clips are intersected here, and an element gets at most one polygon clip (clip-path), which the profile allows only once per element
            match rect_through(&xf, pad) {
                Some(rect) => {
                    let radius = self.inner_radius(id, r);
                    clip = Some(match s.clip {
                        None => ClipRect { rect, radius },
                        Some(outer) => {
                            let inside = contains(outer.rect, rect);
                            ClipRect {
                                rect: intersect(outer.rect, rect),
                                radius: if inside { radius } else { outer.radius },
                            }
                        }
                    });
                }
                None => self.unsupported(id, "an overflow clip under a rotation"),
            }
        }
        Inherit {
            xf,
            clip,
            poly,
            alpha,
        }
    }

    fn padding_box(&mut self, id: NodeId) -> Rect {
        let b = self.border_box(id);
        let Some(l) = self.arena.get(id).map(|n| n.layout().unrounded) else {
            return b;
        };
        let k = self.k;
        Rect::new(
            b.x + l.border.left * k,
            b.y + l.border.top * k,
            (b.w - (l.border.left + l.border.right) * k).max(0.0),
            (b.h - (l.border.top + l.border.bottom) * k).max(0.0),
        )
    }

    fn content_box(&mut self, id: NodeId) -> Rect {
        let b = self.border_box(id);
        let Some(l) = self.arena.get(id).map(|n| n.layout().unrounded) else {
            return b;
        };
        let k = self.k;
        let (left, top) = (l.border.left + l.padding.left, l.border.top + l.padding.top);
        let (right, bottom) = (
            l.border.right + l.padding.right,
            l.border.bottom + l.padding.bottom,
        );
        Rect::new(
            b.x + left * k,
            b.y + top * k,
            (b.w - (left + right) * k).max(0.0),
            (b.h - (top + bottom) * k).max(0.0),
        )
    }

    /// The corner radii of a box (the horizontal ones, device px) and, when the corners are elliptic with
    /// one shared vertical to horizontal ratio, that ratio.
    fn corners(&mut self, id: NodeId, r: &StyleRecord, b: Rect) -> (Radii, Option<f32>) {
        let k = self.k;
        let (w, h) = (b.w / k, b.h / k);
        let corners = [
            r.border_top_left_radius,
            r.border_top_right_radius,
            r.border_bottom_right_radius,
            r.border_bottom_left_radius,
        ];
        // A corner with a zero radius on either axis is square.
        let hv = corners.map(|c| {
            let (x, y) = (resolve(c.h, w) * k, resolve(c.v, h) * k);
            if x > 0.0 && y > 0.0 {
                (x, y)
            } else {
                (0.0, 0.0)
            }
        });
        let mut ratio: Option<f32> = None;
        let mut shared = true;
        for (x, y) in hv.iter().filter(|(x, _)| *x > 0.0) {
            let q = y / x;
            match ratio {
                None => ratio = Some(q),
                Some(r0) if (q - r0).abs() <= 1e-3 * r0.max(q) => {}
                Some(_) => shared = false,
            }
        }
        let ratio = ratio.filter(|q| (q - 1.0).abs() > 1e-3);
        if ratio.is_some() && !shared {
            self.unsupported(id, "elliptical corners of different shapes");
        }
        let [tl, tr, br, bl] = hv.map(|(x, _)| x);
        (Radii { tl, tr, br, bl }, ratio.filter(|_| shared))
    }

    fn radii(&mut self, id: NodeId, r: &StyleRecord, b: Rect) -> Radii {
        self.corners(id, r, b).0
    }

    fn inner_radius(&mut self, id: NodeId, r: &StyleRecord) -> f32 {
        let b = self.border_box(id);
        let radii = self.radii(id, r, b);
        let bw = self
            .width(r.border_top_width)
            .max(self.width(r.border_left_width));
        (radii.tl - bw).max(0.0)
    }

    fn push(&mut self, item: Item) {
        self.stats.items += 1;
        match &item {
            Item::Glyphs(g) | Item::GlyphShadow(GlyphShadow { glyphs: g, .. }) => {
                self.stats.glyphs += g.glyphs.len() as u32;
            }
            _ => {}
        }
        self.out.push(item);
    }

    /// The clip of an item: the overflow rectangle above and the `clip-path` polygon above, in one clip.
    fn clip_of(s: &Inherit) -> Option<Clip> {
        if s.clip.is_none() && s.poly.is_none() {
            return None;
        }
        Some(Clip {
            rect: s.clip.map(|c| c.rect),
            radius: s.clip.map_or(0.0, |c| c.radius.max(0.0)),
            polygon: s.poly,
        })
    }

    fn xf_of(xf: Affine2) -> Option<Affine2> {
        (xf != Affine2::IDENTITY).then_some(xf)
    }

    /// A box item, elliptic when the corners have a shared ratio other than 1.
    fn push_box(&mut self, b: BoxItem, ratio: Option<f32>) {
        match ratio {
            Some(ratio) => self.push(Item::EllipticBox(EllipticBox { base: b, ratio })),
            None => self.push(Item::Box(b)),
        }
    }

    /// The border box, padding box and content box of an element around its snapped border box `rect`,
    /// with the radii of each.
    fn boxes(&self, id: NodeId, rect: Rect, border: &Border, radii: Radii) -> Boxes {
        let k = self.k;
        let pad = self
            .arena
            .get(id)
            .map_or(taffy::Rect::zero(), |n| n.layout().unrounded.padding);
        let bw = [border.top, border.right, border.bottom, border.left];
        let padding = inset(rect, bw);
        let pw = [pad.top * k, pad.right * k, pad.bottom * k, pad.left * k];
        let content = inset(padding, pw);
        let cw = [0, 1, 2, 3].map(|i| bw[i] + pw[i]);
        Boxes {
            border: (rect, radii),
            padding: (padding, shrink(radii, bw)),
            content: (content, shrink(radii, cw)),
        }
    }

    /// The clip of an item that is also cut to `own` (item space): `own` mapped to chunk space and
    /// intersected with the clips above.
    fn clip_with(&mut self, id: NodeId, s: &Inherit, xf: &Affine2, own: ClipRect) -> Option<Clip> {
        let Some(rect) = rect_through(xf, own.rect) else {
            self.unsupported(id, "a background tile clip under a rotation");
            return Self::clip_of(s);
        };
        let c = match s.clip {
            None => ClipRect {
                rect,
                radius: own.radius,
            },
            Some(outer) => ClipRect {
                rect: intersect(outer.rect, rect),
                radius: if contains(outer.rect, rect) {
                    own.radius
                } else {
                    outer.radius
                },
            },
        };
        Self::clip_of(&Inherit {
            clip: Some(c),
            ..*s
        })
    }

    /// A length of a background property in device px (`reference` is in device px too).
    fn bg_len(&self, l: Len, reference: f32) -> f32 {
        l.px * self.k + l.pct * reference / 100.0
    }

    /// The stops of a gradient as fractions of its line (`line` device px; 0 for a conic gradient, whose
    /// positions are fractions of a turn): missing positions are spread as CSS does.
    fn stops(&self, stops: &[Stop], line: f32, alpha: f32) -> Vec<GradientStop> {
        let raw: Vec<Option<f32>> = stops
            .iter()
            .map(|s| {
                s.pos.map(|p| {
                    let px = if line > 0.0 {
                        p.px * self.k / line
                    } else {
                        0.0
                    };
                    px + p.pct / 100.0
                })
            })
            .collect();
        stop_positions(&raw)
            .into_iter()
            .zip(stops)
            .map(|(pos, s)| GradientStop::new(self.color(s.color, alpha), pos))
            .collect()
    }

    /// One `background-image` gradient layer: a box over the layer's clip box, tiled when its size or
    /// position asks for it.
    #[allow(clippy::too_many_arguments)]
    fn paint_gradient(
        &mut self,
        id: NodeId,
        r: &StyleRecord,
        layer: usize,
        g: &ui2_package::style::Gradient,
        boxes: &Boxes,
        ratio: Option<f32>,
        alpha: f32,
        xf: Affine2,
        s: &Inherit,
    ) {
        let at = |len: usize| if len == 0 { 0 } else { layer % len };
        let clip_kind = r
            .background_clip
            .get(at(r.background_clip.len()))
            .copied()
            .unwrap_or(BgBox::BorderBox);
        let origin_kind = r
            .background_origin
            .get(at(r.background_origin.len()))
            .copied()
            .unwrap_or(BgBox::PaddingBox);
        let (clip, clip_radii) = boxes.of(clip_kind);
        let (area, _) = boxes.of(origin_kind);
        // Gradients have no size of their own: `auto`, `cover` and `contain` fill the positioning area.
        let size = r
            .background_size
            .get(at(r.background_size.len()))
            .copied()
            .unwrap_or(BgSize::Size(LenAuto::Auto, LenAuto::Auto));
        let (tw, th) = match size {
            BgSize::Cover | BgSize::Contain => (area.w, area.h),
            BgSize::Size(w, h) => {
                let dim = |v: LenAuto, full: f32| match v {
                    LenAuto::Auto => full,
                    LenAuto::Len(l) => self.bg_len(l, full),
                };
                (dim(w, area.w), dim(h, area.h))
            }
        };
        if !(tw > 0.0 && th > 0.0 && clip.is_drawable()) {
            return;
        }
        let px = r
            .background_position_x
            .get(at(r.background_position_x.len()))
            .copied()
            .unwrap_or(Len::ZERO);
        let py = r
            .background_position_y
            .get(at(r.background_position_y.len()))
            .copied()
            .unwrap_or(Len::ZERO);
        let tile = Vec2::new(
            area.x + self.bg_len(px, area.w - tw),
            area.y + self.bg_len(py, area.h - th),
        );
        let repeat = r
            .background_repeat
            .get(at(r.background_repeat.len()))
            .copied()
            .unwrap_or(BgRepeat::REPEAT);
        // A repeating axis starts at the tile edge at or before the clip box (tiles start at the item's
        // corner) and runs to the clip box's end; a single tile is where the position puts it.
        let span = |rep: Repeat, t0: f32, size: f32, lo: f32, hi: f32| match rep {
            Repeat::Repeat => (t0 - ((t0 - lo) / size).ceil() * size, hi),
            Repeat::NoRepeat => (t0, t0 + size),
        };
        let (x0, x1) = span(repeat.x, tile.x, tw, clip.x, clip.x + clip.w);
        let (y0, y1) = span(repeat.y, tile.y, th, clip.y, clip.y + clip.h);
        let dest = Rect::new(x0, y0, x1 - x0, y1 - y0);
        if !dest.is_drawable() {
            return;
        }
        let eps = 0.01;
        let tiled = dest.w > tw + eps || dest.h > th + eps;
        let fits = (dest.x - clip.x).abs() < eps
            && (dest.y - clip.y).abs() < eps
            && (dest.w - clip.w).abs() < eps
            && (dest.h - clip.h).abs() < eps;
        let size = Vec2::new(tw, th);
        let grad = if g.conic {
            let start = match g.dir {
                GradDir::Angle(a) => a / 360.0,
                GradDir::To(_) => 0.0,
            };
            Gradient::conic(size * 0.5, start, &self.stops(&g.stops, 0.0, alpha))
        } else {
            let angle = match g.dir {
                GradDir::Angle(a) => a,
                GradDir::To(side) => match side {
                    Side::Top => 0.0,
                    Side::Right => 90.0,
                    Side::Bottom => 180.0,
                    Side::Left => 270.0,
                    Side::TopLeft => css_corner_angle(false, false, size),
                    Side::TopRight => css_corner_angle(true, false, size),
                    Side::BottomLeft => css_corner_angle(false, true, size),
                    Side::BottomRight => css_corner_angle(true, true, size),
                },
            };
            let (from, to) = css_linear_line(angle, size);
            let line = (to - from).length();
            Gradient::linear(from, to, &self.stops(&g.stops, line, alpha))
        };
        let mut grad = match grad {
            Ok(g) => g,
            Err(e) => return self.unsupported(id, &format!("a gradient ({e})")),
        };
        if g.repeating {
            grad = grad.repeating();
        }
        if tiled {
            grad = grad.tiled(size);
        }
        if grad.is_transparent() {
            return;
        }
        // The item is the clip box itself, or tiles that the clip box cuts.
        let inside = contains(clip, dest) && clip_radii == Radii::ZERO;
        let (radii, ratio, item_clip) = if fits {
            (clip_radii, ratio, Self::clip_of(s))
        } else if inside {
            (Radii::ZERO, None, Self::clip_of(s))
        } else {
            let own = ClipRect {
                rect: clip,
                radius: clip_radii.tl,
            };
            (Radii::ZERO, None, self.clip_with(id, s, &xf, own))
        };
        self.push_box(
            BoxItem {
                rect: if fits { clip } else { dest },
                fill: Fill::Gradient(grad),
                border: Border::NONE,
                radii,
                transform: Self::xf_of(xf),
                clip: item_clip,
            },
            ratio,
        );
    }

    /// A color picture of the package by its path in the skin, as a sprite of the `Rgba8` pages; reported
    /// (once per node) and `None` when the package has no such picture or packed it as a mask.
    fn picture(&mut self, id: NodeId, path: StrId, what: &str) -> Option<(ImageRef, [f32; 2])> {
        let pkg = self.cx.pkg;
        let Some((img, entry)) = find_picture(pkg, path) else {
            let name = pkg.str(path).unwrap_or("?").to_string();
            self.problem(id, format!("the package has no picture {name} for {what}"));
            return None;
        };
        if entry.mask {
            self.problem(
                id,
                format!("{what} is a mask icon in the package, not a color picture"),
            );
            return None;
        }
        match image_ref(pkg, img, self.k, PageFormat::Rgba8) {
            Some(r) => Some((r, entry.size)),
            None => {
                self.problem(id, format!("{what} has no color page in the package"));
                None
            }
        }
    }

    /// One `background-image: url()` layer: the picture at its size (`auto` is its own size, keeping the
    /// ratio when one side is given), placed by the layer's position in its origin box, repeated over the
    /// clip box when the layer repeats (one item per tile), cut to the clip box.
    #[allow(clippy::too_many_arguments)]
    fn paint_bg_picture(
        &mut self,
        id: NodeId,
        r: &StyleRecord,
        layer: usize,
        path: StrId,
        boxes: &Boxes,
        alpha: f32,
        xf: Affine2,
        s: &Inherit,
    ) {
        let Some((image, natural)) = self.picture(id, path, "a background picture") else {
            return;
        };
        let k = self.k;
        let (nw, nh) = (natural[0] * k, natural[1] * k);
        if !(nw > 0.0 && nh > 0.0) {
            return;
        }
        let clip_kind = layer_at(&r.background_clip, layer, BgBox::BorderBox);
        let origin_kind = layer_at(&r.background_origin, layer, BgBox::PaddingBox);
        let (clip, clip_radii) = boxes.of(clip_kind);
        let (area, _) = boxes.of(origin_kind);
        let size = layer_at(
            &r.background_size,
            layer,
            BgSize::Size(LenAuto::Auto, LenAuto::Auto),
        );
        let (tw, th) = match size {
            BgSize::Cover => {
                let f = (area.w / nw).max(area.h / nh);
                (nw * f, nh * f)
            }
            BgSize::Contain => {
                let f = (area.w / nw).min(area.h / nh);
                (nw * f, nh * f)
            }
            BgSize::Size(w, h) => {
                let len = |v: LenAuto, full: f32| match v {
                    LenAuto::Auto => None,
                    LenAuto::Len(l) => Some(self.bg_len(l, full)),
                };
                match (len(w, area.w), len(h, area.h)) {
                    (None, None) => (nw, nh),
                    (Some(w), None) => (w, w * nh / nw),
                    (None, Some(h)) => (h * nw / nh, h),
                    (Some(w), Some(h)) => (w, h),
                }
            }
        };
        if !(tw > 0.0 && th > 0.0 && clip.is_drawable()) {
            return;
        }
        let px = layer_at(&r.background_position_x, layer, Len::ZERO);
        let py = layer_at(&r.background_position_y, layer, Len::ZERO);
        let tile = Vec2::new(
            area.x + self.bg_len(px, area.w - tw),
            area.y + self.bg_len(py, area.h - th),
        );
        let repeat = layer_at(&r.background_repeat, layer, BgRepeat::REPEAT);
        let span = |rep: Repeat, t0: f32, size: f32, lo: f32, hi: f32| match rep {
            Repeat::Repeat => (t0 - ((t0 - lo) / size).ceil() * size, hi),
            Repeat::NoRepeat => (t0, t0 + size),
        };
        let (x0, x1) = span(repeat.x, tile.x, tw, clip.x, clip.x + clip.w);
        let (y0, y1) = span(repeat.y, tile.y, th, clip.y, clip.y + clip.h);
        let nx = ((x1 - x0) / tw - 0.01).ceil().max(1.0);
        let ny = ((y1 - y0) / th - 0.01).ceil().max(1.0);
        if nx * ny > MAX_TILES as f32 {
            self.unsupported(
                id,
                &format!("a background picture repeated more than {MAX_TILES} times"),
            );
            return;
        }
        let tint = with_alpha([255, 255, 255, 255], alpha);
        if tint.is_transparent() {
            return;
        }
        let whole = Rect::new(x0, y0, nx * tw, ny * th);
        let item_clip = if contains(clip, whole) && clip_radii == Radii::ZERO {
            Self::clip_of(s)
        } else {
            let own = ClipRect {
                rect: clip,
                radius: clip_radii.tl,
            };
            self.clip_with(id, s, &xf, own)
        };
        for j in 0..ny as u32 {
            for i in 0..nx as u32 {
                let rect = Rect::new(x0 + i as f32 * tw, y0 + j as f32 * th, tw, th);
                self.push(Item::Image(ImageItem {
                    rect,
                    image,
                    tint,
                    radii: Radii::ZERO,
                    transform: Self::xf_of(xf),
                    clip: item_clip,
                    nine: None,
                    external: None,
                }));
            }
        }
    }

    /// The 9-slice of a node's `border-image` over its border box: the sprite and its cut, or `None` when the
    /// node has no border image (or its picture is missing: the border is painted then, as in CSS).
    fn border_image(
        &mut self,
        id: NodeId,
        r: &StyleRecord,
        boxes: &Boxes,
    ) -> Option<(ImageRef, NineSlice)> {
        let path = r.border_image_source?;
        let (image, natural) = self.picture(id, path, "a border image")?;
        let k = self.k;
        let sl = &r.border_image_slice;
        let cut = |l: Len, full: f32| {
            let v = if full > 0.0 {
                l.px / full + l.pct / 100.0
            } else {
                l.pct / 100.0
            };
            v.clamp(0.0, 1.0)
        };
        let src = [
            cut(sl.edges[0], natural[1]),
            cut(sl.edges[1], natural[0]),
            cut(sl.edges[2], natural[1]),
            cut(sl.edges[3], natural[0]),
        ];
        let sides = [
            r.border_top_width,
            r.border_right_width,
            r.border_bottom_width,
            r.border_left_width,
        ];
        let dst = [0, 1, 2, 3].map(|i| {
            let w = match r.border_image_width[i] {
                ImageWidth::Px(v) => v,
                ImageWidth::Factor(f) => f * sides[i].px,
            };
            (w * k).max(0.0)
        });
        if r.border_image_repeat.x != ImageRepeat::Stretch
            || r.border_image_repeat.y != ImageRepeat::Stretch
        {
            self.problem(
                id,
                "border-image-repeat other than stretch is drawn stretched".into(),
            );
        }
        if !boxes.border.0.is_drawable() {
            return None;
        }
        Some((
            image,
            NineSlice {
                src,
                dst,
                fill: sl.fill,
            },
        ))
    }

    /// The shadows, background, border and outline of an element.
    fn paint_box(&mut self, id: NodeId, s: &Inherit) {
        let Some(n) = self.arena.get(id) else {
            return;
        };
        if !n.class().is_element() {
            return;
        }
        let Some(r) = self.record(id) else {
            return;
        };
        if r.visibility == Visibility::Hidden {
            return;
        }
        let xf = s.xf * self.own_transform(id, r);
        let alpha = s.alpha * r.opacity;
        // The node's own clip-path cuts its box too.
        let own = self.clip_path(id, r, &xf);
        if own == ClipPath::Empty {
            return;
        }
        let poly = self.merge_poly(id, s.poly, own);
        let s = &Inherit { poly, ..*s };
        let rect = Self::snap(self.border_box(id), &xf);
        if !rect.is_drawable() {
            return;
        }
        let k = self.k;
        let (radii, ratio) = self.corners(id, r, rect);
        let fill = self.color(r.background_color, alpha);
        let mut border = Border::NONE;
        let sides = [
            (r.border_top_style, r.border_top_width, r.border_top_color),
            (
                r.border_right_style,
                r.border_right_width,
                r.border_right_color,
            ),
            (
                r.border_bottom_style,
                r.border_bottom_width,
                r.border_bottom_color,
            ),
            (
                r.border_left_style,
                r.border_left_width,
                r.border_left_color,
            ),
        ];
        let widths = sides.map(|(style, w, _)| {
            if style == BorderStyle::Solid {
                self.width(w)
            } else {
                0.0
            }
        });
        [border.top, border.right, border.bottom, border.left] = widths;
        let colors: Vec<ColorV> = sides
            .iter()
            .zip(widths)
            .filter(|(_, w)| *w > 0.0)
            .map(|((_, _, c), _)| *c)
            .collect();
        if let Some(first) = colors.first() {
            border.color = self.color(*first, alpha);
            if colors.iter().any(|c| c != first) {
                self.unsupported(id, "a border with sides of different colors");
            }
        }
        let insets: Vec<_> = r.box_shadow.iter().filter(|sh| sh.inset).copied().collect();
        let clip = Self::clip_of(s);
        let transform = Self::xf_of(xf);

        // Outer shadows under everything else, the first listed on top.
        for sh in r.box_shadow.iter().rev().filter(|sh| !sh.inset) {
            let color = self.color(sh.color, alpha);
            if color.is_transparent() {
                continue;
            }
            self.push(Item::Shadow(ShadowItem {
                rect,
                radii,
                offset: Vec2::new(sh.x * k, sh.y * k),
                blur: sh.blur * k,
                spread: sh.spread * k,
                color,
                transform,
                clip,
            }));
        }

        let boxes = self.boxes(id, rect, &border, radii);
        // A border image is painted instead of the border (the border widths still make the boxes).
        let nine = self.border_image(id, r, &boxes);
        let edge = if nine.is_some() { Border::NONE } else { border };
        // The color is painted inside the clip box of the bottom layer (the last listed). Every list of the
        // background properties is indexed by the layer's place in `background-image`, gradients and pictures
        // alike, repeating as CSS does when a list is shorter.
        let layers = r.background_image.len().max(1);
        let color_clip = r
            .background_clip
            .get((layers - 1) % r.background_clip.len().max(1))
            .copied()
            .unwrap_or(BgBox::BorderBox);
        let simple = r.background_image.is_empty()
            && insets.is_empty()
            && color_clip == BgBox::BorderBox
            && nine.is_none();
        if simple {
            if !(fill.is_transparent() && border.is_none()) {
                self.push_box(
                    BoxItem {
                        rect,
                        fill: Fill::Solid(fill),
                        border,
                        radii,
                        transform,
                        clip,
                    },
                    ratio,
                );
            }
        } else {
            if !fill.is_transparent() {
                let (frect, fradii) = boxes.of(color_clip);
                if frect.is_drawable() {
                    self.push_box(
                        BoxItem {
                            rect: frect,
                            fill: Fill::Solid(fill),
                            border: Border::NONE,
                            radii: fradii,
                            transform,
                            clip,
                        },
                        ratio,
                    );
                }
            }
            // The bottom layer is the last listed.
            for (i, layer) in r.background_image.iter().enumerate().rev() {
                match layer {
                    BgImage::Gradient(g) => {
                        self.paint_gradient(id, r, i, g, &boxes, ratio, alpha, xf, s)
                    }
                    BgImage::Url(path) => {
                        self.paint_bg_picture(id, r, i, *path, &boxes, alpha, xf, s)
                    }
                }
            }
            // Inset shadows lie inside the padding box, between the background and the border, the first listed
            // on top. Without blur they are bands (a border-only box); with blur, inset shadow items.
            let (pad, inner) = boxes.padding;
            for sh in insets.iter().rev() {
                if sh.blur != 0.0 {
                    let color = self.color(sh.color, alpha);
                    if !color.is_transparent() && pad.is_drawable() {
                        self.push(Item::InsetShadow(ShadowItem {
                            rect: pad,
                            radii: inner,
                            offset: Vec2::new(sh.x * k, sh.y * k),
                            blur: sh.blur * k,
                            spread: sh.spread * k,
                            color,
                            transform,
                            clip,
                        }));
                    }
                    continue;
                }
                let band = |v: f32| {
                    if v > 0.0 {
                        (v * k).round().max(1.0)
                    } else {
                        0.0
                    }
                };
                let spread = sh.spread.max(0.0);
                let bands = Border {
                    top: band(sh.y.max(0.0) + spread),
                    right: band((-sh.x).max(0.0) + spread),
                    bottom: band((-sh.y).max(0.0) + spread),
                    left: band(sh.x.max(0.0) + spread),
                    color: self.color(sh.color, alpha),
                };
                self.push_box(
                    BoxItem {
                        rect: pad,
                        fill: Fill::None,
                        border: bands,
                        radii: inner,
                        transform,
                        clip,
                    },
                    ratio,
                );
            }
            if !edge.is_none() {
                self.push_box(
                    BoxItem {
                        rect,
                        fill: Fill::None,
                        border: edge,
                        radii,
                        transform,
                        clip,
                    },
                    ratio,
                );
            }
            if let Some((image, nine)) = nine {
                let tint = with_alpha([255, 255, 255, 255], alpha);
                if !tint.is_transparent() {
                    self.push(Item::Image(ImageItem {
                        rect,
                        image,
                        tint,
                        radii: Radii::ZERO,
                        transform,
                        clip,
                        nine: Some(nine),
                        external: None,
                    }));
                }
            }
        }

        // The outline: a border outside the border box, at the outline offset.
        if r.outline_style == OutlineStyle::Solid {
            let w = self.width(r.outline_width);
            let color = self.color(r.outline_color, alpha);
            if w > 0.0 && !color.is_transparent() {
                // Painted right after the element's border, under its children (CSS paints outlines in the
                // last phase of the stacking context).
                let grow = (r.outline_offset.px * k).round() + w;
                let orect = Rect::new(
                    rect.x - grow,
                    rect.y - grow,
                    rect.w + 2.0 * grow,
                    rect.h + 2.0 * grow,
                );
                let g = |v: f32| if v > 0.0 { (v + grow).max(0.0) } else { 0.0 };
                if orect.is_drawable() {
                    self.push_box(
                        BoxItem {
                            rect: orect,
                            fill: Fill::None,
                            border: Border::uniform(w, color),
                            radii: Radii {
                                tl: g(radii.tl),
                                tr: g(radii.tr),
                                br: g(radii.br),
                                bl: g(radii.bl),
                            },
                            transform,
                            clip,
                        },
                        ratio,
                    );
                }
            }
        }
    }

    /// The content of a node: the glyphs of an inline formatting root, the picture of an image.
    fn paint_content(&mut self, id: NodeId, s: &Inherit) {
        let Some(n) = self.arena.get(id) else {
            return;
        };
        let inner = self.child_state(id, s);
        if n.ifc().is_some() {
            self.paint_text(id, &inner);
            return;
        }
        let Some(t) = instantiate::template_node(self.cx.pkg, n.tmpl()) else {
            return;
        };
        if matches!(t.kind, NodeKind::Input { .. }) {
            self.paint_field(id, &inner);
            return;
        }
        if let NodeKind::Image(src) = &t.kind {
            // A picture fills its own box: its overflow does not clip it, the clips above do.
            let src = src.clone();
            let own = Inherit {
                clip: s.clip,
                ..inner
            };
            self.paint_image(id, &src, &own);
        }
    }

    fn paint_image(&mut self, id: NodeId, src: &ImageSrc, s: &Inherit) {
        let Some(r) = self.record(id) else {
            return;
        };
        if r.visibility == Visibility::Hidden {
            return;
        }
        let img = match src {
            ImageSrc::Icon { img, mask: true } => *img,
            ImageSrc::Icon { img, mask: false } | ImageSrc::Pkg(img) => {
                let Some(image) = image_ref(self.cx.pkg, *img, self.k, PageFormat::Rgba8) else {
                    return self.unsupported(id, "a color picture without a color page");
                };
                return self.paint_picture(id, r, image, None, s);
            }
            ImageSrc::Asset(a) => {
                let Some(name) = self.cx.pkg.asset_ref(*a) else {
                    return;
                };
                return self.paint_asset(id, r, name, s);
            }
            ImageSrc::Bound(_) => {
                // Nothing until the binding gave a reference; an empty one shows nothing.
                let Some(name) = self.cx.srcs.get(&id).cloned() else {
                    return;
                };
                if name.is_empty() {
                    return;
                }
                return self.paint_asset(id, r, &name, s);
            }
        };
        let Some(mask) = image_ref(self.cx.pkg, img, self.k, PageFormat::A8) else {
            return self.unsupported(id, "a mask icon without a mask page");
        };
        let rect = Self::snap(self.content_box(id), &s.xf);
        let color = self.color(r.color, s.alpha);
        self.push(Item::Mask(MaskItem {
            rect,
            mask,
            color,
            transform: Self::xf_of(s.xf),
            clip: Self::clip_of(s),
        }));
    }

    /// A game asset picture: the minimap, or a texture the host's resolver names.
    fn paint_asset(&mut self, id: NodeId, r: &StyleRecord, name: &str, s: &Inherit) {
        let ext = if name == crate::input::MINIMAP_ASSET {
            Some(ExtImage {
                texture: ExtTexture::MINIMAP,
                uv: [0.0, 0.0, 1.0, 1.0],
            })
        } else {
            match self.cx.assets {
                Some(resolve) => resolve(name),
                None => {
                    return self.unsupported(id, "a game asset picture without an asset resolver");
                }
            }
        };
        let Some(ext) = ext else {
            let path = instantiate::node_path(self.arena, self.cx.pkg, id);
            self.errors.report(UiError::at(
                &path,
                ErrorCode::Paint,
                format!("the game has no picture {name}"),
            ));
            return;
        };
        let image = ImageRef {
            layer: 0,
            uv: ext.uv,
        };
        self.paint_picture(id, r, image, Some(ext.texture), s);
    }

    /// A color picture in the content box, cut to the node's corners and tinted by its opacity.
    fn paint_picture(
        &mut self,
        id: NodeId,
        r: &StyleRecord,
        image: ImageRef,
        external: Option<ExtTexture>,
        s: &Inherit,
    ) {
        let border = self.border_box(id);
        let rect = Self::snap(self.content_box(id), &s.xf);
        if !rect.is_drawable() {
            return;
        }
        let (radii, _) = self.corners(id, r, border);
        let inner = match self.arena.get(id).map(|n| n.layout().unrounded) {
            Some(l) => {
                let k = self.k;
                shrink(
                    radii,
                    [
                        (l.border.top + l.padding.top) * k,
                        (l.border.right + l.padding.right) * k,
                        (l.border.bottom + l.padding.bottom) * k,
                        (l.border.left + l.padding.left) * k,
                    ],
                )
            }
            None => radii,
        };
        let tint = with_alpha([255, 255, 255, 255], s.alpha);
        if tint.is_transparent() {
            return;
        }
        self.push(Item::Image(ImageItem {
            rect,
            image,
            tint,
            radii: inner,
            transform: Self::xf_of(s.xf),
            clip: Self::clip_of(s),
            nine: None,
            external,
        }));
    }

    /// A solid rectangle of a text field (selection, composition underline, caret).
    fn field_rect(&mut self, rect: Rect, color: Color, s: &Inherit, clip: Option<Clip>) {
        if !rect.is_drawable() || color.is_transparent() {
            return;
        }
        self.push(Item::Box(BoxItem {
            rect,
            fill: Fill::Solid(color),
            border: Border::NONE,
            radii: Radii::ZERO,
            transform: Self::xf_of(s.xf),
            clip,
        }));
    }

    /// The content of a text field: its text on one line in the content box (centered vertically, scrolled
    /// so that the caret is inside), the selection under it, the composition underlined, the caret.
    fn paint_field(&mut self, id: NodeId, s: &Inherit) {
        use crate::input::text_edit::{offset_x, scroll_for};
        let fields = self.cx.fields;
        let Some(view) = fields.view(id) else {
            return;
        };
        let Some(r) = self.record(id) else {
            return;
        };
        if r.visibility == Visibility::Hidden {
            return;
        }
        let edit = fields.edit_of(id);
        let k = self.k;
        let content = self.content_box(id);
        let pad = self.padding_box(id);
        // The field's text never leaves its padding box.
        let clip = self.clip_with(
            id,
            s,
            &s.xf,
            ClipRect {
                rect: pad,
                radius: 0.0,
            },
        );
        let run = view.run.as_ref();
        let text_run = if view.placeholder { None } else { run };
        let (cw, ch) = (content.w / k, content.h / k);
        let caret = edit.map_or(0, |e| e.sel.caret);
        let caret_x = offset_x(text_run, caret);
        let width = run.map_or(0.0, |r| r.width);
        let scroll = if edit.is_some() {
            scroll_for(caret_x, cw)
        } else {
            0.0
        };
        let free = (cw - width).max(0.0);
        let align = match r.text_align {
            ui2_package::style::TextAlign::Center => free / 2.0,
            ui2_package::style::TextAlign::Right | ui2_package::style::TextAlign::End => free,
            _ => 0.0,
        };
        let x0 = content.x + (align - scroll) * k;
        let m = view.vm.metrics;
        let base = content.y + ((ch - view.vm.line_height) / 2.0 + view.vm.above()) * k;
        let (top, height) = (base - m.ascent * k, (m.ascent + m.descent) * k);
        let color = self.color(r.color, s.alpha);
        let x_of = |at: u32| x0 + offset_x(text_run, at) * k;
        // The selection under the text.
        if let Some(e) = edit
            && !e.sel.is_empty()
            && text_run.is_some()
        {
            let (a, b) = e.sel.range();
            let sel = Color([color.0[0], color.0[1], color.0[2], color.0[3] / 3]);
            let (xa, xb) = (x_of(a), x_of(b));
            self.field_rect(Rect::new(xa, top, xb - xa, height), sel, s, clip);
        }
        if let Some(run) = run {
            // The placeholder is drawn in the field's color at half opacity: `::placeholder` rules are not
            // baked into the package.
            let shown = if view.placeholder {
                Color([color.0[0], color.0[1], color.0[2], color.0[3] / 2])
            } else {
                color
            };
            let glyphs: Vec<GlyphPos> = run
                .glyphs
                .iter()
                .map(|g| GlyphPos {
                    glyph: g.glyph_id,
                    x: x0 + g.x * k,
                    y: base + g.y * k,
                })
                .collect();
            if !glyphs.is_empty() && !shown.is_transparent() {
                let style = run.style;
                self.push(Item::Glyphs(Glyphs {
                    face: style.face,
                    size_px: style.size * k,
                    wght: style.wght,
                    color: shown,
                    glyphs,
                    transform: Self::xf_of(s.xf),
                    clip,
                }));
            }
        }
        let Some(e) = edit else {
            return;
        };
        let line = k.floor().max(1.0);
        // The text an input method composes is underlined.
        if let Some((a, b)) = e.composing {
            let (xa, xb) = (x_of(a), x_of(b));
            let y = (base + m.descent.min(2.0) * k).round();
            self.field_rect(Rect::new(xa, y, xb - xa, line), color, s, clip);
        }
        // The caret (`caret-color: auto`), while nothing is selected and the blink shows it.
        if e.caret_on && e.sel.is_empty() {
            let x = x_of(e.sel.caret).round();
            self.field_rect(Rect::new(x, top, line, height), color, s, clip);
        }
    }

    fn paint_text(&mut self, id: NodeId, s: &Inherit) {
        let arena = self.arena;
        let Some(ifc) = arena.get(id).and_then(|n| n.ifc()) else {
            return;
        };
        let content = self.content_box(id);
        let k = self.k;
        let cw = content.w / k;
        let mut items = Vec::new();
        let shown = ifc.shown();
        for line in &shown.lines.lines {
            let lx = ifc.line_x(line, cw);
            let base = ifc.lines_y() + line.top + line.baseline;
            for seg in &line.segments {
                let Some(run) = shown.run(seg.run) else {
                    continue;
                };
                // A text leaf is styled like its parent element (with that element's bound properties).
                let owner = arena
                    .get(run.node)
                    .and_then(|n| {
                        if n.class() == NodeClass::Text {
                            n.parent()
                        } else {
                            Some(run.node)
                        }
                    })
                    .unwrap_or(run.node);
                let Some(r) = self.record(owner) else {
                    continue;
                };
                if r.visibility == Visibility::Hidden {
                    continue;
                }
                // A piece colored by markup (`[COLOR_X]`) has its token's color.
                let color = self.color(run.color.unwrap_or(r.color), s.alpha);
                let range = seg.start as u32..seg.end as u32;
                let glyphs: Vec<GlyphPos> = run
                    .shaped
                    .glyphs
                    .iter()
                    .filter(|g| range.contains(&g.cluster))
                    .map(|g| GlyphPos {
                        glyph: g.glyph_id,
                        x: content.x + (lx + seg.x + g.x - seg.run_x) * k,
                        y: content.y + (base + g.y) * k,
                    })
                    .collect();
                if glyphs.is_empty() {
                    continue;
                }
                let style = run.shaped.style;
                let glyphs_of = |color: Color, glyphs: Vec<GlyphPos>| Glyphs {
                    face: style.face,
                    size_px: style.size * k,
                    wght: style.wght,
                    color,
                    glyphs,
                    transform: Self::xf_of(s.xf),
                    clip: Self::clip_of(s),
                };
                // Text shadows: the glyphs again, moved and in the shadow's color, blurred when the shadow
                // has a blur, under the text (the first listed on top).
                for sh in r.text_shadow.iter().rev() {
                    let color = self.color(sh.color, s.alpha);
                    if color.is_transparent() {
                        continue;
                    }
                    let (dx, dy) = (sh.x * k, sh.y * k);
                    let moved = glyphs
                        .iter()
                        .map(|g| GlyphPos {
                            x: g.x + dx,
                            y: g.y + dy,
                            ..*g
                        })
                        .collect();
                    let run = glyphs_of(color, moved);
                    items.push(if sh.blur > 0.0 {
                        Item::GlyphShadow(GlyphShadow {
                            glyphs: run,
                            blur: sh.blur * k,
                        })
                    } else {
                        Item::Glyphs(run)
                    });
                }
                items.push(Item::Glyphs(glyphs_of(color, glyphs)));
            }
        }
        for item in items {
            self.push(item);
        }
    }

    /// Paints a stacking context: its box, then its descendants in phase order.
    fn paint_context(&mut self, id: NodeId, s: &Inherit) {
        let hidden = self
            .arena
            .get(id)
            .is_none_or(|n| n.class() == NodeClass::Service)
            || self
                .record(id)
                .is_none_or(|r| r.display == ui2_package::style::Display::None);
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
        self.paint_box(id, s);
        let inner = self.child_state(id, s);
        let arena = self.arena;
        let cx = self.cx;
        let record = |n: NodeId| arena.get(n).map(|node| cx.record(n, node));
        let content_root = |n: NodeId| arena.get(n).is_some_and(|node| node.ifc().is_some());
        let phases = stacking::phases(
            arena,
            id,
            inner,
            &record,
            &mut |n, st| self.child_state(n, st),
            &content_root,
        );
        for (n, st) in &phases.negative {
            self.paint_context(*n, st);
        }
        for (n, st) in &phases.flow {
            self.paint_box(*n, st);
        }
        self.paint_content(id, s);
        for (n, st) in &phases.flow {
            self.paint_content(*n, st);
        }
        for (n, st) in phases.zero.iter().chain(&phases.positive) {
            self.paint_context(*n, st);
        }
    }

    /// The items of the chunk of `root`. A map anchor's chunk is drawn in its own space (the origin of the
    /// root's containing block is the anchor point) and placed by its anchor slot; while it has no slot it
    /// draws nothing.
    pub fn chunk(mut self, root: NodeId) -> (ChunkItems, Emitted) {
        if let Some(slot) = self.cx.anchors.slot(root) {
            let Some(slot) = slot else {
                return (ChunkItems::default(), self.stats);
            };
            self.out = ChunkBuilder::new().anchor(slot);
            let [x, y] = self.abs(root);
            let loc = self
                .arena
                .get(root)
                .map_or(taffy::Point::ZERO, |n| n.layout().unrounded.location);
            self.origin = [x - loc.x, y - loc.y];
        } else if self.is_anchor_node(root) {
            // An anchor that has no object yet is not drawn at the layout position.
            return (ChunkItems::default(), self.stats);
        }
        self.paint_context(root, &Inherit::ROOT);
        (self.out.build(), self.stats)
    }
}

/// The three boxes of an element (device px) with their corner radii.
struct Boxes {
    border: (Rect, Radii),
    padding: (Rect, Radii),
    content: (Rect, Radii),
}

impl Boxes {
    fn of(&self, b: BgBox) -> (Rect, Radii) {
        match b {
            BgBox::BorderBox => self.border,
            BgBox::PaddingBox => self.padding,
            BgBox::ContentBox => self.content,
        }
    }
}

/// Most tiles one repeated background picture is drawn with.
const MAX_TILES: u32 = 1024;

/// The value of a per-layer background list for a layer: lists shorter than the layers repeat (CSS).
fn layer_at<T: Copy>(list: &[T], layer: usize, default: T) -> T {
    if list.is_empty() {
        default
    } else {
        list[layer % list.len()]
    }
}

/// A picture of the package by its path inside the skin (`IMGS` entries are looked up by the string-table
/// id, then by the text of the path).
pub(crate) fn find_picture(
    pkg: &crate::Pkg,
    path: StrId,
) -> Option<(ui2_package::ImgId, &ui2_package::Img)> {
    let imgs = &pkg.images().imgs;
    let at = imgs.iter().position(|i| i.path == path).or_else(|| {
        let want = pkg.str(path)?;
        imgs.iter().position(|i| pkg.str(i.path) == Some(want))
    })?;
    Some((ui2_package::ImgId(at as u32), &imgs[at]))
}

/// A rectangle shrunk by `[top, right, bottom, left]`.
fn inset(r: Rect, w: [f32; 4]) -> Rect {
    Rect::new(
        r.x + w[3],
        r.y + w[0],
        (r.w - w[1] - w[3]).max(0.0),
        (r.h - w[0] - w[2]).max(0.0),
    )
}

/// The radii of a box shrunk by `[top, right, bottom, left]` (each corner by the larger of its sides).
fn shrink(r: Radii, w: [f32; 4]) -> Radii {
    let [t, rt, b, l] = w;
    Radii {
        tl: (r.tl - l.max(t)).max(0.0),
        tr: (r.tr - rt.max(t)).max(0.0),
        br: (r.br - rt.max(b)).max(0.0),
        bl: (r.bl - l.max(b)).max(0.0),
    }
}

/// CSS stop positions: the first defaults to 0 and the last to 1, a position before an earlier one is
/// raised to it, and runs of missing positions are spread evenly between their neighbours.
pub(crate) fn stop_positions(raw: &[Option<f32>]) -> Vec<f32> {
    let n = raw.len();
    let mut p: Vec<Option<f32>> = raw.to_vec();
    if let Some(first) = p.first_mut() {
        first.get_or_insert(0.0);
    }
    if let Some(last) = p.last_mut() {
        last.get_or_insert(1.0);
    }
    let mut max = f32::NEG_INFINITY;
    for v in p.iter_mut().flatten() {
        *v = v.max(max);
        max = *v;
    }
    let mut i = 0;
    while i < n {
        if p[i].is_some() {
            i += 1;
            continue;
        }
        // `p[0]` and `p[n - 1]` are set, so a gap has a start before it and an end after it.
        let a = i - 1;
        let mut b = i;
        while b < n && p[b].is_none() {
            b += 1;
        }
        let (pa, pb) = (
            p[a].unwrap_or(0.0),
            p.get(b).copied().flatten().unwrap_or(1.0),
        );
        for (j, v) in p.iter_mut().enumerate().take(b).skip(i) {
            *v = Some(pa + (pb - pa) * (j - a) as f32 / (b - a) as f32);
        }
        i = b;
    }
    p.into_iter().map(|v| v.unwrap_or(0.0)).collect()
}

/// A rectangle mapped by a transform without rotation or skew (`None` otherwise).
fn rect_through(xf: &Affine2, r: Rect) -> Option<Rect> {
    let m = xf.matrix2;
    if m.x_axis.y != 0.0 || m.y_axis.x != 0.0 {
        return None;
    }
    let a = xf.transform_point2(Vec2::new(r.x, r.y));
    let b = xf.transform_point2(Vec2::new(r.x + r.w, r.y + r.h));
    let (lo, hi) = (a.min(b), a.max(b));
    Some(Rect::new(lo.x, lo.y, hi.x - lo.x, hi.y - lo.y))
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.w).min(b.x + b.w);
    let y1 = (a.y + a.h).min(b.y + b.h);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

fn contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.w <= outer.x + outer.w
        && inner.y + inner.h <= outer.y + outer.h
}

/// Where a picture of the package lies in the texture array of its page format: the package's pages of that
/// format, in order, are the layers (`A8` pages for mask icons, `Rgba8` pages for color pictures); the
/// sprite's 1 px margin is left out. The 2x sprite is used above a device scale of 1 when there is one.
pub(crate) fn image_ref(
    pkg: &crate::Pkg,
    img: ui2_package::ImgId,
    k: f32,
    format: PageFormat,
) -> Option<ImageRef> {
    let images = pkg.images();
    let img = images.imgs.get(img.index())?;
    let sprite = match img.x2 {
        Some(x2) if k > 1.0 => x2,
        _ => img.x1,
    };
    let page = images.pages.get(usize::from(sprite.page))?;
    if page.format != format {
        return None;
    }
    let layer = images.pages[..usize::from(sprite.page)]
        .iter()
        .filter(|p| p.format == format)
        .count();
    let [x, y, w, h] = sprite.rect.map(f32::from);
    let (pw, ph) = (f32::from(page.size[0]), f32::from(page.size[1]));
    Some(ImageRef {
        layer: u8::try_from(layer).ok()?,
        uv: [
            (x + 1.0) / pw,
            (y + 1.0) / ph,
            (x + w - 1.0) / pw,
            (y + h - 1.0) / ph,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::stop_positions;

    #[test]
    fn stop_positions_follow_css() {
        assert_eq!(stop_positions(&[None, None]), [0.0, 1.0]);
        assert_eq!(
            stop_positions(&[None, None, None, None]),
            [0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]
        );
        // A position before an earlier one is raised to it; a gap spreads between its neighbours.
        assert_eq!(
            stop_positions(&[Some(0.5), Some(0.2), None, Some(0.9)]),
            [0.5, 0.5, 0.7, 0.9]
        );
        assert_eq!(stop_positions(&[Some(0.2), None, None]), [0.2, 0.6, 1.0]);
    }
}
