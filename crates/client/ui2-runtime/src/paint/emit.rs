//! Building the draw list of one chunk.
//!
//! Positions come from layout in CSS px relative to each node's containing block; they are made absolute
//! (page px) and scaled by `k` into device pixels. Box edges that no transform touches snap to device pixels
//! like Chrome's, and border widths are at least one device pixel, floored; glyphs keep the positions of
//! shaping. Colors resolve their theme tokens here and take the opacity of the node and its ancestors.
//!
//! An element paints, in this order: its outer box shadows (the last listed first), its background color
//! (inside the `background-clip` box of the bottom layer), one box per `background-image` gradient layer
//! (the bottom layer first), its inset shadows without blur (bands inside the padding box), its border, and
//! its outline (a border-only box grown by the outline offset and width).

use std::collections::HashMap;
use std::sync::Arc;

use glam::{Affine2, Vec2};
use ui2_draw::{
    Border, BoxItem, ChunkBuilder, ChunkItems, Clip, Color, EllipticBox, ExtTexture, Fill,
    GlyphPos, Glyphs, Gradient, GradientStop, ImageItem, ImageRef, Item, LayerId, MaskItem, Radii,
    Rect, ShadowItem, css_corner_angle, css_linear_line,
};
use ui2_package::style::{
    BgBox, BgImage, BgRepeat, BgSize, BorderStyle, ColorV, GradDir, Len, LenAuto, OutlineStyle,
    Overflow, Position, Repeat, Side, Stop, StyleRecord, TFn, Visibility,
};
use ui2_package::{ImageSrc, Layer, NodeKind, Package, PageFormat};

use crate::arena::{Arena, NodeClass, NodeId};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;
use crate::paint::stacking;
use crate::style::{StyleEnv, StyleIndex, node_record, tokens};
use crate::{AssetResolver, ExtImage};

/// What painting reads besides the arena.
pub(crate) struct PaintCtx<'a> {
    pub pkg: &'a Package,
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
    /// The product of the opacities above.
    alpha: f32,
}

impl Inherit {
    const ROOT: Inherit = Inherit {
        xf: Affine2::IDENTITY,
        clip: None,
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
pub(crate) fn layer_of(arena: &Arena, pkg: &Package, root: NodeId) -> LayerId {
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

    /// The state a node passes to its children: its transform, its opacity and its overflow clip.
    fn child_state(&mut self, id: NodeId, s: &Inherit) -> Inherit {
        let Some(r) = self.record(id) else {
            return *s;
        };
        let xf = s.xf * self.own_transform(id, r);
        let alpha = s.alpha * r.opacity;
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
        Inherit { xf, clip, alpha }
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
        if let Item::Glyphs(g) = &item {
            self.stats.glyphs += g.glyphs.len() as u32;
        }
        self.out.push(item);
    }

    fn clip_of(s: &Inherit) -> Option<Clip> {
        s.clip.map(|c| {
            if c.radius > 0.0 {
                Clip::rounded(c.rect, c.radius)
            } else {
                Clip::rect(c.rect)
            }
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
        let insets: Vec<_> = r
            .box_shadow
            .iter()
            .filter(|sh| sh.inset && sh.blur == 0.0)
            .copied()
            .collect();
        if r.box_shadow.iter().any(|sh| sh.inset && sh.blur != 0.0) {
            self.unsupported(id, "a blurred inset box shadow");
        }
        if !r.text_shadow.is_empty() {
            self.unsupported(id, "a text shadow");
        }
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
        let gradients: Vec<&ui2_package::style::Gradient> = r
            .background_image
            .iter()
            // Picture layers (`url()`) are not painted yet: only gradient layers are kept, so in a list that
            // mixes both the per-layer size, position and clip of the later layers are read one layer early.
            .filter_map(|b| match b {
                BgImage::Gradient(g) => Some(g),
                BgImage::Url(_) => None,
            })
            .collect();
        // The color is painted inside the clip box of the bottom layer.
        let layers = gradients.len().max(1);
        let color_clip = r
            .background_clip
            .get((layers - 1) % r.background_clip.len().max(1))
            .copied()
            .unwrap_or(BgBox::BorderBox);
        let simple = gradients.is_empty() && insets.is_empty() && color_clip == BgBox::BorderBox;
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
            for (i, g) in gradients.iter().enumerate().rev() {
                self.paint_gradient(id, r, i, g, &boxes, ratio, alpha, xf, s);
            }
            // Inset shadows without blur are bands inside the padding box, between the background and the
            // border.
            let (pad, inner) = boxes.padding;
            for sh in insets.iter().rev() {
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
            if !border.is_none() {
                self.push_box(
                    BoxItem {
                        rect,
                        fill: Fill::None,
                        border,
                        radii,
                        transform,
                        clip,
                    },
                    ratio,
                );
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
            let base = line.top + line.baseline;
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
                items.push(Item::Glyphs(Glyphs {
                    face: style.face,
                    size_px: style.size * k,
                    wght: style.wght,
                    color,
                    glyphs,
                    transform: Self::xf_of(s.xf),
                    clip: Self::clip_of(s),
                }));
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
    pkg: &Package,
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
