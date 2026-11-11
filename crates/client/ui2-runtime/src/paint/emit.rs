//! Building the draw list of one chunk.
//!
//! Positions come from layout in CSS px relative to each node's containing block; they are made absolute
//! (page px) and scaled by `k` into device pixels. Box edges that no transform touches snap to device pixels
//! like Chrome's, and border widths are at least one device pixel, floored; glyphs keep the positions of
//! shaping. Colors resolve their theme tokens here and take the opacity of the node and its ancestors.

use std::collections::HashMap;

use glam::{Affine2, Vec2};
use ui2_draw::{
    Border, BoxItem, ChunkBuilder, ChunkItems, Clip, Color, Fill, GlyphPos, Glyphs, ImageRef, Item,
    LayerId, MaskItem, Radii, Rect,
};
use ui2_package::style::{
    BorderStyle, ColorV, Len, Overflow, Position, StyleRecord, TFn, Visibility,
};
use ui2_package::{ImageSrc, Layer, NodeKind, Package, PageFormat};

use crate::arena::{Arena, NodeClass, NodeId};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;
use crate::paint::stacking;
use crate::style::{StyleEnv, StyleIndex, node_record, tokens};

/// What painting reads besides the arena.
pub(crate) struct PaintCtx<'a> {
    pub pkg: &'a Package,
    pub index: &'a StyleIndex,
    pub env: &'a StyleEnv,
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
        }
    }

    fn record(&self, id: NodeId) -> Option<&'a StyleRecord> {
        let n = self.arena.get(id)?;
        Some(node_record(self.cx.index, self.cx.pkg.styles(), n))
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

    /// The border box of a node in device px (unsnapped).
    fn border_box(&mut self, id: NodeId) -> Rect {
        let [x, y] = self.abs(id);
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

    fn radii(&mut self, id: NodeId, r: &StyleRecord, b: Rect) -> Radii {
        let k = self.k;
        let (w, h) = (b.w / k, b.h / k);
        let corners = [
            r.border_top_left_radius,
            r.border_top_right_radius,
            r.border_bottom_right_radius,
            r.border_bottom_left_radius,
        ];
        if corners.iter().any(|c| resolve(c.h, w) != resolve(c.v, h)) {
            self.unsupported(id, "an elliptical corner");
        }
        let [tl, tr, br, bl] = corners.map(|c| resolve(c.h, w) * k);
        Radii { tl, tr, br, bl }
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

    /// The background, inset shadows and border of an element.
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
        let radii = self.radii(id, r, rect);
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
        if r.box_shadow.iter().any(|sh| !sh.inset || sh.blur != 0.0) {
            self.unsupported(id, "an outer or blurred box shadow");
        }
        if !r.text_shadow.is_empty() {
            self.unsupported(id, "a text shadow");
        }
        let clip = Self::clip_of(s);
        let transform = Self::xf_of(xf);
        if insets.is_empty() && fill.is_transparent() && border.is_none() {
            return;
        }
        if insets.is_empty() {
            self.push(Item::Box(BoxItem {
                rect,
                fill: Fill::Solid(fill),
                border,
                radii,
                transform,
                clip,
            }));
            return;
        }
        // Inset shadows without blur are bands inside the padding box, between the fill and the border.
        if !fill.is_transparent() {
            self.push(Item::Box(BoxItem {
                rect,
                fill: Fill::Solid(fill),
                border: Border::NONE,
                radii,
                transform,
                clip,
            }));
        }
        let pad = Rect::new(
            rect.x + border.left,
            rect.y + border.top,
            rect.w - border.left - border.right,
            rect.h - border.top - border.bottom,
        );
        let inner = Radii {
            tl: (radii.tl - border.left.max(border.top)).max(0.0),
            tr: (radii.tr - border.right.max(border.top)).max(0.0),
            br: (radii.br - border.right.max(border.bottom)).max(0.0),
            bl: (radii.bl - border.left.max(border.bottom)).max(0.0),
        };
        for sh in insets.iter().rev() {
            let k = self.k;
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
            self.push(Item::Box(BoxItem {
                rect: pad,
                fill: Fill::None,
                border: bands,
                radii: inner,
                transform,
                clip,
            }));
        }
        if !border.is_none() {
            self.push(Item::Box(BoxItem {
                rect,
                fill: Fill::None,
                border,
                radii,
                transform,
                clip,
            }));
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
            ImageSrc::Icon { mask: false, .. } | ImageSrc::Pkg(_) => {
                return self.unsupported(id, "a picture of the package");
            }
            ImageSrc::Asset(_) | ImageSrc::Bound(_) => {
                return self.unsupported(id, "a game asset picture");
            }
        };
        let Some(mask) = image_ref(self.cx.pkg, img, self.k) else {
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
        let record = |n: NodeId| {
            arena
                .get(n)
                .map(|node| node_record(cx.index, cx.pkg.styles(), node))
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

    /// The items of the chunk of `root`.
    pub fn chunk(mut self, root: NodeId) -> (ChunkItems, Emitted) {
        self.paint_context(root, &Inherit::ROOT);
        (self.out.build(), self.stats)
    }
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

/// Where a mask icon lies in the mask texture array: the package's `A8` pages come first, in order; the
/// sprite's 1 px margin is left out. The 2x sprite is used above a device scale of 1 when there is one.
pub(crate) fn image_ref(pkg: &Package, img: ui2_package::ImgId, k: f32) -> Option<ImageRef> {
    let images = pkg.images();
    let img = images.imgs.get(img.index())?;
    let sprite = match img.x2 {
        Some(x2) if k > 1.0 => x2,
        _ => img.x1,
    };
    let page = images.pages.get(usize::from(sprite.page))?;
    if page.format != PageFormat::A8 {
        return None;
    }
    let layer = images.pages[..usize::from(sprite.page)]
        .iter()
        .filter(|p| p.format == PageFormat::A8)
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
