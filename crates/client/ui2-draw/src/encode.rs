//! Item to instance encoding.
//!
//! One box, shadow, mask, picture or glyph makes one instance (a box's fill and border together); a 9-slice
//! picture makes nine. Boxes with nothing to show, rectangles without area and pictures of a page the scene does
//! not hold make none. Sprite rectangles of package pages are mapped from page to array coordinates ([`Pages`]).
//! Transforms and clips are interned in the [`Tables`], external textures in the [`ExtSlots`]; the instance
//! carries their indices and the chunk's anchor.

use glam::Affine2;

use crate::glyphs::{GlyphStore, Lookup, glyph_key};
use crate::instance::{
    FILL_CONIC, FILL_LINEAR, FILL_REPEAT, FILL_SOLID, FLAG_BORDER, FLAG_ELLIPSE, FLAG_EXT,
    FLAG_FILL, FLAG_INSET, FLAG_TILE, KIND_BOX, KIND_IMAGE, KIND_MASK, KIND_SHADOW, Ui2Instance,
    f16_bits,
};
use crate::pages::{ExtSlots, Pages};
use crate::tables::{Clip, Tables};
use crate::{
    BoxItem, ChunkItems, Fill, Glyphs, Gradient, GradientShape, ImageItem, Item, MaskItem,
    NineSlice, Rect, ShadowItem,
};

/// What the encoder works on besides the items.
pub struct EncodeCtx<'a> {
    /// Transform and clip tables (entries are interned here).
    pub tables: &'a mut Tables,
    /// Glyph atlas and queue.
    pub glyphs: &'a mut GlyphStore,
    /// Incremented for every transform, clip or external texture that did not fit in its table (the item is
    /// drawn without the transform or clip; a picture without its texture is not drawn).
    pub table_overflow: &'a mut u32,
    /// The package's picture pages.
    pub pages: &'a Pages,
    /// Side of the layers of the coverage array (mask pages and glyph pages), px.
    pub mask_side: u32,
    /// Slots of the external textures.
    pub exts: &'a mut ExtSlots,
}

/// The result of encoding one chunk.
#[derive(Debug, Default)]
pub struct Encoded {
    /// The instances, in drawing order.
    pub instances: Vec<Ui2Instance>,
    /// Element transform records acquired (one reference each; release them when the chunk goes away).
    pub xforms: Vec<u16>,
    /// Clips acquired (one reference each).
    pub clips: Vec<u16>,
    /// External texture slots acquired (one reference each).
    pub exts: Vec<u16>,
    /// Glyphs that were queued for rasterization (their instances are placeholders until then).
    pub unresolved: u32,
    /// The glyph pages the instances sample (bit `i`: page `i` of the glyph store).
    pub glyph_pages: u8,
}

/// Encodes a whole chunk.
pub fn encode_chunk(chunk: &ChunkItems, ctx: &mut EncodeCtx<'_>) -> Encoded {
    let mut out = Encoded::default();
    let anchor = chunk.anchor.map_or(0, |a| a.0);
    for item in &chunk.items {
        match item {
            Item::Box(b) => encode_box(b, 1.0, anchor, ctx, &mut out),
            Item::EllipticBox(e) => encode_box(&e.base, e.ratio, anchor, ctx, &mut out),
            Item::Shadow(s) => encode_shadow(s, false, anchor, ctx, &mut out),
            Item::InsetShadow(s) => encode_shadow(s, true, anchor, ctx, &mut out),
            Item::Mask(m) => encode_mask(m, anchor, ctx, &mut out),
            Item::Image(m) => encode_image(m, anchor, ctx, &mut out),
            Item::Glyphs(g) => encode_glyphs(g, anchor, ctx, &mut out),
        }
    }
    out
}

/// Interns the item's transform and clip; `(xform, clip)` indices, 0 where there is none.
fn place(
    transform: &Option<Affine2>,
    clip: &Option<Clip>,
    ctx: &mut EncodeCtx<'_>,
    out: &mut Encoded,
) -> (u16, u16) {
    let mut xf = 0;
    if let Some(m) = transform
        && *m != Affine2::IDENTITY
    {
        match ctx.tables.intern_xform(m) {
            Some(i) => {
                out.xforms.push(i);
                xf = i;
            }
            None => *ctx.table_overflow += 1,
        }
    }
    let mut cl = 0;
    if let Some(c) = clip
        && !c.is_empty()
    {
        match ctx.tables.intern_clip(c) {
            Some(i) => {
                out.clips.push(i);
                cl = i;
            }
            None => *ctx.table_overflow += 1,
        }
    }
    (xf, cl)
}

/// A box: fill (solid or gradient) and border in one instance. `ratio` is the ry / rx ratio of elliptic corners
/// (1: circular).
fn encode_box(b: &BoxItem, ratio: f32, anchor: u16, ctx: &mut EncodeCtx<'_>, out: &mut Encoded) {
    if !b.rect.is_drawable() {
        return;
    }
    let fill = match b.fill {
        Fill::Solid(c) if c.is_transparent() => Fill::None,
        Fill::Gradient(g) if g.is_transparent() => Fill::None,
        f => f,
    };
    let border = (!b.border.is_none()).then_some(b.border);
    if fill == Fill::None && border.is_none() {
        return;
    }
    let (xform, clip) = place(&b.transform, &b.clip, ctx, out);
    let mut i = Ui2Instance::of_kind(KIND_BOX);
    i.rect = [b.rect.x, b.rect.y, b.rect.w, b.rect.h];
    let ratio = if ratio.is_finite() && ratio > 0.0 {
        ratio
    } else {
        1.0
    };
    // Elliptic radii are fitted with the vertical ones against the height: ry = rx * ratio <= h.
    let r = b.radii.fitted(b.rect.w, b.rect.h / ratio);
    i.set_radii([r.tl, r.tr, r.br, r.bl]);
    if ratio != 1.0 {
        i.flags |= FLAG_ELLIPSE;
        i.extra[2] = f16_bits(ratio);
    }
    match fill {
        Fill::Solid(c) => {
            i.colors[0] = c.0;
            i.flags |= FLAG_FILL | FILL_SOLID;
        }
        Fill::Gradient(g) => {
            encode_gradient(&g, &mut i);
            i.flags |= FLAG_FILL;
        }
        _ => {}
    }
    if let Some(bd) = border {
        let w = |v: f32| v.max(0.0);
        i.set_border([w(bd.top), w(bd.right), w(bd.bottom), w(bd.left)]);
        i.border_color = bd.color.0;
        i.flags |= FLAG_BORDER;
    }
    i.xform = xform;
    i.clip = clip;
    i.anchor = anchor;
    out.instances.push(i);
}

/// Writes a gradient into the fill fields of a box instance (mode bits, `uv`, colours, stops, tile).
///
/// The stops are normalized so that the first lies at position 0 and the last at 1, which keeps them exact as
/// half floats and makes the repeat period of a repeating gradient the whole 0..1 range: a linear gradient's
/// line is moved onto the first and the last stop, a repeating conic gradient's start angle and span likewise.
/// A plain conic gradient keeps its positions (the angle outside the stops takes the end colours).
fn encode_gradient(g: &Gradient, i: &mut Ui2Instance) {
    let stops = g.stops();
    let (first, last) = (stops[0].pos, stops[stops.len() - 1].pos);
    // All stops at one position: a hard switch there. Keep a tiny span so the order still decides.
    let last = if last - first > 1e-6 {
        last
    } else {
        first + 1e-6
    };
    let span = last - first;
    let normalize = !matches!(g.shape, GradientShape::Conic { .. }) || g.repeating;
    for (k, s) in stops.iter().enumerate() {
        i.colors[k] = s.color.0;
        let pos = if normalize {
            (s.pos - first) / span
        } else {
            s.pos.clamp(0.0, 1.0)
        };
        i.stops[k] = f16_bits(pos);
    }
    i.n_stops = stops.len() as u8;
    match g.shape {
        GradientShape::Linear { from, to } => {
            let d = to - from;
            let (a, b) = (from + d * first, from + d * last);
            i.uv = [a.x, a.y, b.x, b.y];
            i.flags |= if g.repeating {
                FILL_REPEAT
            } else {
                FILL_LINEAR
            };
        }
        GradientShape::Conic { center, start } => {
            i.uv = if g.repeating {
                [center.x, center.y, (start + first).rem_euclid(1.0), span]
            } else {
                [center.x, center.y, start.rem_euclid(1.0), 1.0]
            };
            i.flags |= FILL_CONIC;
        }
    }
    if let Some(t) = g.tile {
        i.flags |= FLAG_TILE;
        i.extra[0] = f16_bits(t.x);
        i.extra[1] = f16_bits(t.y);
    }
}

/// A shadow: one instance. The shader grows the quad of an outer shadow to the blurred shadow; an inset
/// shadow's quad is its rectangle (the padding box).
fn encode_shadow(
    s: &ShadowItem,
    inset: bool,
    anchor: u16,
    ctx: &mut EncodeCtx<'_>,
    out: &mut Encoded,
) {
    let finite = s.offset.is_finite() && s.blur.is_finite() && s.spread.is_finite();
    if !s.rect.is_drawable() || s.color.is_transparent() || !finite {
        return;
    }
    // A spread that swallows the box leaves nothing of an outer shadow (an inset one then fills its box).
    if !inset && (s.rect.w + 2.0 * s.spread <= 0.0 || s.rect.h + 2.0 * s.spread <= 0.0) {
        return;
    }
    let (xform, clip) = place(&s.transform, &s.clip, ctx, out);
    let mut i = Ui2Instance::of_kind(KIND_SHADOW);
    if inset {
        i.flags |= FLAG_INSET;
    }
    i.rect = [s.rect.x, s.rect.y, s.rect.w, s.rect.h];
    let r = s.radii.fitted(s.rect.w, s.rect.h);
    i.set_radii([r.tl, r.tr, r.br, r.bl]);
    i.uv = [s.offset.x, s.offset.y, 0.0, 0.0];
    i.extra[0] = f16_bits(s.blur.max(0.0) * 0.5);
    i.extra[1] = f16_bits(s.spread);
    i.colors[0] = s.color.0;
    i.xform = xform;
    i.clip = clip;
    i.anchor = anchor;
    out.instances.push(i);
}

fn encode_mask(m: &MaskItem, anchor: u16, ctx: &mut EncodeCtx<'_>, out: &mut Encoded) {
    if !m.rect.is_drawable() || m.color.is_transparent() {
        return;
    }
    let (xform, clip) = place(&m.transform, &m.clip, ctx, out);
    let mut i = Ui2Instance::of_kind(KIND_MASK);
    i.rect = [m.rect.x, m.rect.y, m.rect.w, m.rect.h];
    i.uv = ctx.pages.mask_uv(m.mask.layer, m.mask.uv, ctx.mask_side);
    i.colors[0] = m.color.0;
    i.atlas_layer = m.mask.layer;
    i.xform = xform;
    i.clip = clip;
    i.anchor = anchor;
    out.instances.push(i);
}

/// A colour picture: the sprite of an `Rgba8` page or a rectangle of an external texture, times the tint, cut
/// to the rounded rectangle, or drawn as a 9-slice. A picture whose page the scene does not hold makes no
/// instance.
fn encode_image(m: &ImageItem, anchor: u16, ctx: &mut EncodeCtx<'_>, out: &mut Encoded) {
    if !m.rect.is_drawable() || m.tint.is_transparent() {
        return;
    }
    let (uv, layer) = match m.external {
        Some(_) => (m.image.uv, 0),
        None => match ctx.pages.image_uv(m.image.layer, m.image.uv) {
            Some(uv) => (uv, m.image.layer),
            None => return,
        },
    };
    let mut ext = 0;
    if let Some(t) = m.external {
        let Some(slot) = ctx.exts.acquire(t) else {
            *ctx.table_overflow += 1;
            return;
        };
        out.exts.push(slot);
        ext = slot;
    }
    let (xform, clip) = place(&m.transform, &m.clip, ctx, out);
    let mut base = Ui2Instance::of_kind(KIND_IMAGE);
    base.colors[0] = m.tint.0;
    base.atlas_layer = layer;
    base.ext = ext;
    if ext != 0 {
        base.flags |= FLAG_EXT;
    }
    base.xform = xform;
    base.clip = clip;
    base.anchor = anchor;
    match m.nine {
        None => {
            let mut i = base;
            i.rect = [m.rect.x, m.rect.y, m.rect.w, m.rect.h];
            let r = m.radii.fitted(m.rect.w, m.rect.h);
            i.set_radii([r.tl, r.tr, r.br, r.bl]);
            i.uv = uv;
            out.instances.push(i);
        }
        Some(n) => {
            for (rect, uv) in nine_pieces(m.rect, uv, &n) {
                let mut i = base;
                i.rect = [rect.x, rect.y, rect.w, rect.h];
                i.uv = uv;
                out.instances.push(i);
            }
        }
    }
}

/// The pieces of a 9-slice, row by row: destination rectangle and sprite rectangle of each corner, edge and
/// (with `fill`) the middle. Pieces without area in the destination or in the sprite are left out.
fn nine_pieces(rect: Rect, uv: [f32; 4], n: &NineSlice) -> Vec<(Rect, [f32; 4])> {
    let w = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let f = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    let [t, r, b, l] = n.dst.map(w);
    // CSS: widths that do not fit are scaled down together.
    let mut k = 1.0f32;
    if l + r > rect.w {
        k = k.min(rect.w / (l + r));
    }
    if t + b > rect.h {
        k = k.min(rect.h / (t + b));
    }
    let (t, r, b, l) = (t * k, r * k, b * k, l * k);
    let xs = [rect.x, rect.x + l, rect.x + rect.w - r, rect.x + rect.w];
    let ys = [rect.y, rect.y + t, rect.y + rect.h - b, rect.y + rect.h];
    let [st, sr, sb, sl] = n.src.map(f);
    let (du, dv) = (uv[2] - uv[0], uv[3] - uv[1]);
    // Slices that overlap in the sprite meet in the middle.
    let (sl, sr) = if sl + sr > 1.0 {
        (sl / (sl + sr), sr / (sl + sr))
    } else {
        (sl, sr)
    };
    let (st, sb) = if st + sb > 1.0 {
        (st / (st + sb), sb / (st + sb))
    } else {
        (st, sb)
    };
    let us = [uv[0], uv[0] + sl * du, uv[2] - sr * du, uv[2]];
    let vs = [uv[1], uv[1] + st * dv, uv[3] - sb * dv, uv[3]];
    let mut out = Vec::with_capacity(9);
    for row in 0..3 {
        for col in 0..3 {
            if row == 1 && col == 1 && !n.fill {
                continue;
            }
            let dst = Rect::new(
                xs[col],
                ys[row],
                xs[col + 1] - xs[col],
                ys[row + 1] - ys[row],
            );
            let src = [us[col], vs[row], us[col + 1], vs[row + 1]];
            if dst.is_drawable() && src[2] != src[0] && src[3] != src[1] {
                out.push((dst, src));
            }
        }
    }
    out
}

fn encode_glyphs(g: &Glyphs, anchor: u16, ctx: &mut EncodeCtx<'_>, out: &mut Encoded) {
    if g.glyphs.is_empty() {
        return;
    }
    let (xform, clip) = place(&g.transform, &g.clip, ctx, out);
    let page = ctx.mask_side as f32;
    let layer_base = ctx.glyphs.layer_base();
    for pos in &g.glyphs {
        let (key, px, baseline) = glyph_key(g, pos);
        let mut i = Ui2Instance::of_kind(KIND_MASK);
        i.colors[0] = g.color.0;
        i.xform = xform;
        i.clip = clip;
        i.anchor = anchor;
        match ctx.glyphs.lookup(key) {
            Lookup::Ready(slot) if slot.w > 0 && slot.h > 0 => {
                i.rect = [
                    (px + slot.left) as f32,
                    (baseline - slot.top) as f32,
                    slot.w as f32,
                    slot.h as f32,
                ];
                i.uv = [
                    slot.x as f32 / page,
                    slot.y as f32 / page,
                    (slot.x + slot.w) as f32 / page,
                    (slot.y + slot.h) as f32 / page,
                ];
                i.atlas_layer = layer_base.saturating_add(slot.page as u8);
                if slot.page < 8 {
                    out.glyph_pages |= 1 << slot.page;
                }
            }
            Lookup::Pending => out.unresolved += 1,
            Lookup::Ready(_) | Lookup::Unavailable => {}
        }
        out.instances.push(i);
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;
    use ui2_text::AtlasConfig;

    use super::*;
    use crate::instance::f16_bits;
    use crate::{
        AnchorSlot, Border, ChunkBuilder, Color, EllipticBox, GradientStop, ImageRef, Radii,
    };

    struct Rig {
        tables: Tables,
        glyphs: GlyphStore,
        overflow: u32,
        pages: Pages,
        exts: ExtSlots,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                tables: Tables::new(),
                glyphs: GlyphStore::new(AtlasConfig::default()),
                overflow: 0,
                pages: Pages::default(),
                exts: ExtSlots::default(),
            }
        }
        fn encode(&mut self, c: &ChunkItems) -> Encoded {
            let mask_side = self.glyphs.page_size();
            let mut ctx = EncodeCtx {
                tables: &mut self.tables,
                glyphs: &mut self.glyphs,
                table_overflow: &mut self.overflow,
                pages: &self.pages,
                mask_side,
                exts: &mut self.exts,
            };
            encode_chunk(c, &mut ctx)
        }
    }

    fn one(item: Item) -> ChunkItems {
        let mut b = ChunkBuilder::new();
        b.push(item);
        b.build()
    }

    #[test]
    fn solid_box_bytes() {
        let mut rig = Rig::new();
        let e = rig.encode(&one(Item::Box(BoxItem::solid(
            Rect::new(10.0, 20.0, 100.0, 50.0),
            Color::rgba(200, 100, 50, 255),
        ))));
        assert_eq!(e.instances.len(), 1);
        let mut want = [0u8; 96];
        for (i, v) in [10.0f32, 20.0, 100.0, 50.0].iter().enumerate() {
            want[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        want[48..52].copy_from_slice(&[200, 100, 50, 255]);
        want[84] = KIND_BOX;
        want[85] = FLAG_FILL;
        assert_eq!(e.instances[0].as_bytes(), &want[..]);
    }

    #[test]
    fn bordered_rounded_box_bytes() {
        let mut rig = Rig::new();
        let b = BoxItem {
            rect: Rect::new(0.0, 0.0, 120.0, 48.0),
            fill: Fill::Solid(Color::rgba(10, 20, 30, 128)),
            border: Border {
                top: 1.0,
                right: 1.0,
                bottom: 2.0,
                left: 1.0,
                color: Color::rgba(255, 255, 255, 64),
            },
            radii: Radii {
                tl: 3.0,
                tr: 4.0,
                br: 5.0,
                bl: 6.0,
            },
            ..BoxItem::default()
        };
        let e = rig.encode(&one(Item::Box(b)));
        let i = e.instances[0];
        assert_eq!(i.radii, [3.0, 4.0, 5.0, 6.0].map(f16_bits));
        assert_eq!(i.border, [1.0, 1.0, 2.0, 1.0].map(f16_bits));
        assert_eq!(i.colors, [[10, 20, 30, 128], [0; 4], [0; 4], [0; 4]]);
        assert_eq!(i.border_color, [255, 255, 255, 64]);
        assert_eq!(i.flags, FLAG_FILL | FLAG_BORDER);
        assert_eq!((i.xform, i.clip, i.anchor, i.ext), (0, 0, 0, 0));
        // Raw bytes of the half-float and meta words.
        let b = i.as_bytes();
        assert_eq!(
            &b[16..24],
            &[0x00, 0x42, 0x00, 0x44, 0x00, 0x45, 0x00, 0x46]
        );
        assert_eq!(
            &b[24..32],
            &[0x00, 0x3c, 0x00, 0x3c, 0x00, 0x40, 0x00, 0x3c]
        );
        assert_eq!(&b[64..68], &[255, 255, 255, 64]);
    }

    #[test]
    fn border_only_box_has_no_fill_flag() {
        let mut rig = Rig::new();
        let b = BoxItem {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            border: Border::uniform(1.0, Color::rgba(1, 2, 3, 255)),
            ..BoxItem::default()
        };
        let i = rig.encode(&one(Item::Box(b))).instances[0];
        assert_eq!(i.flags, FLAG_BORDER);
        assert_eq!(i.colors[0], [0; 4]);
    }

    #[test]
    fn nothing_to_draw_makes_no_instance() {
        let mut rig = Rig::new();
        let c = Color::rgba(1, 2, 3, 255);
        let items = [
            BoxItem::solid(Rect::new(0.0, 0.0, 0.0, 10.0), c),
            BoxItem::solid(Rect::new(0.0, 0.0, 10.0, -1.0), c),
            BoxItem::solid(Rect::new(f32::NAN, 0.0, 10.0, 10.0), c),
            BoxItem::solid(Rect::new(0.0, 0.0, 10.0, 10.0), Color::TRANSPARENT),
            BoxItem {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                border: Border::uniform(0.0, c),
                ..BoxItem::default()
            },
        ];
        for b in items {
            assert!(rig.encode(&one(Item::Box(b))).instances.is_empty());
        }
    }

    #[test]
    fn radii_are_fitted_like_css() {
        let mut rig = Rig::new();
        let b = BoxItem {
            rect: Rect::new(0.0, 0.0, 20.0, 100.0),
            fill: Fill::Solid(Color::rgba(0, 0, 0, 255)),
            radii: Radii::uniform(20.0),
            ..BoxItem::default()
        };
        let i = rig.encode(&one(Item::Box(b))).instances[0];
        // The top side is 20 wide and carries tl + tr = 40: all radii scale by 1/2.
        assert_eq!(i.radii, [10.0; 4].map(f16_bits));
        let neg = Radii {
            tl: -3.0,
            tr: f32::NAN,
            br: 2.0,
            bl: 0.0,
        }
        .fitted(100.0, 100.0);
        assert_eq!(
            neg,
            Radii {
                tl: 0.0,
                tr: 0.0,
                br: 2.0,
                bl: 0.0
            }
        );
    }

    #[test]
    fn transform_clip_and_anchor_are_carried() {
        let mut rig = Rig::new();
        let m = Affine2::from_angle_translation(0.25, Vec2::new(5.0, 6.0));
        let clip = Clip::rounded(Rect::new(0.0, 0.0, 50.0, 50.0), 4.0);
        let mut b = ChunkBuilder::new().anchor(AnchorSlot(7));
        for x in [1.0, 2.0] {
            b.push(Item::Box(BoxItem {
                rect: Rect::new(x, 0.0, 10.0, 10.0),
                fill: Fill::Solid(Color::rgba(9, 9, 9, 255)),
                transform: Some(m),
                clip: Some(clip),
                ..BoxItem::default()
            }));
        }
        b.push(Item::Box(BoxItem {
            transform: Some(Affine2::IDENTITY),
            ..BoxItem::solid(Rect::new(0.0, 0.0, 1.0, 1.0), Color::rgba(1, 1, 1, 255))
        }));
        let e = rig.encode(&b.build());
        assert_eq!(e.instances.len(), 3);
        for k in 0..2 {
            let i = e.instances[k];
            assert_eq!((i.xform, i.clip, i.anchor), (2048, 1, 7));
        }
        // Identity transform and no clip: indices 0.
        let last = e.instances[2];
        assert_eq!((last.xform, last.clip, last.anchor), (0, 0, 7));
        // Two references each were taken for the shared entries.
        assert_eq!(e.xforms, vec![2048, 2048]);
        assert_eq!(e.clips, vec![1, 1]);
        assert_eq!(rig.tables.live(), (1, 1));
        assert_eq!(rig.overflow, 0);
    }

    #[test]
    fn mask_bytes() {
        let mut rig = Rig::new();
        let m = MaskItem {
            rect: Rect::new(4.0, 5.0, 16.0, 16.0),
            mask: ImageRef {
                layer: 2,
                uv: [0.25, 0.5, 0.375, 0.625],
            },
            color: Color::rgba(240, 220, 160, 255),
            ..MaskItem::default()
        };
        let e = rig.encode(&one(Item::Mask(m)));
        let i = e.instances[0];
        assert_eq!(i.kind, KIND_MASK);
        assert_eq!(i.rect, [4.0, 5.0, 16.0, 16.0]);
        assert_eq!(i.uv, [0.25, 0.5, 0.375, 0.625]);
        assert_eq!(i.colors[0], [240, 220, 160, 255]);
        assert_eq!(i.atlas_layer, 2);
        assert_eq!(i.flags, 0);
        let b = i.as_bytes();
        assert_eq!(&b[32..36], &0.25f32.to_le_bytes());
        assert_eq!(b[84], 3);
        assert_eq!(b[86], 2);
    }

    fn gstops(s: &[(Color, f32)]) -> Vec<GradientStop> {
        s.iter().map(|(c, p)| GradientStop::new(*c, *p)).collect()
    }

    const R: Color = Color::rgba(255, 0, 0, 255);
    const G: Color = Color::rgba(0, 255, 0, 255);

    fn gradient_instance(g: Gradient) -> Ui2Instance {
        let mut rig = Rig::new();
        let b = BoxItem {
            rect: Rect::new(0.0, 0.0, 40.0, 20.0),
            fill: Fill::Gradient(g),
            ..BoxItem::default()
        };
        rig.encode(&one(Item::Box(b))).instances[0]
    }

    #[test]
    fn linear_gradient_moves_its_line_onto_the_first_and_last_stop() {
        let g = Gradient::linear(
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            &gstops(&[(R, 0.25), (G, 0.5), (R, 0.75)]),
        )
        .unwrap();
        let i = gradient_instance(g);
        assert_eq!(i.flags, FLAG_FILL | FILL_LINEAR);
        assert_eq!(i.n_stops, 3);
        assert_eq!(i.uv, [25.0, 0.0, 75.0, 0.0]);
        assert_eq!(i.stops, [0.0, 0.5, 1.0, 0.0].map(f16_bits));
        assert_eq!(i.colors, [R.0, G.0, R.0, [0; 4]]);
        let r = gradient_instance(g.repeating().tiled(Vec2::new(10.0, 20.0)));
        assert_eq!(r.flags, FLAG_FILL | FILL_REPEAT | FLAG_TILE);
        assert_eq!(r.extra, [10.0, 20.0, 0.0, 0.0].map(f16_bits));
    }

    #[test]
    fn conic_gradient_keeps_plain_positions_and_normalizes_repeating_ones() {
        let s = gstops(&[(R, 0.0), (R, 0.125), (G, 0.125), (G, 0.25)]);
        let plain = gradient_instance(Gradient::conic(Vec2::new(5.0, 6.0), 1.25, &s).unwrap());
        assert_eq!(plain.flags, FLAG_FILL | FILL_CONIC);
        assert_eq!(plain.uv, [5.0, 6.0, 0.25, 1.0]);
        assert_eq!(plain.stops, [0.0, 0.125, 0.125, 0.25].map(f16_bits));
        let rep = gradient_instance(
            Gradient::conic(Vec2::new(5.0, 6.0), 0.5, &s)
                .unwrap()
                .repeating(),
        );
        assert_eq!(rep.uv, [5.0, 6.0, 0.5, 0.25]);
        assert_eq!(rep.stops, [0.0, 0.5, 0.5, 1.0].map(f16_bits));
    }

    #[test]
    fn gradients_validate_their_stops() {
        let v = Vec2::ZERO;
        assert!(Gradient::linear(v, v, &gstops(&[(R, 0.0)])).is_err());
        let five = gstops(&[(R, 0.0), (R, 0.1), (R, 0.2), (R, 0.3), (R, 0.4)]);
        assert!(Gradient::linear(v, v, &five).is_err());
        assert!(Gradient::linear(v, v, &gstops(&[(R, 0.0), (R, f32::NAN)])).is_err());
        // Backward positions are raised to the previous one.
        let g = Gradient::linear(v, v, &gstops(&[(R, 0.5), (G, 0.2), (R, 0.7)])).unwrap();
        let pos: Vec<f32> = g.stops().iter().map(|s| s.pos).collect();
        assert_eq!(pos, vec![0.5, 0.5, 0.7]);
        // A gradient of transparent stops fills nothing.
        let mut rig = Rig::new();
        let clear = Gradient::linear(
            v,
            Vec2::X,
            &gstops(&[(Color::TRANSPARENT, 0.0), (Color::TRANSPARENT, 1.0)]),
        )
        .unwrap();
        let b = BoxItem {
            rect: Rect::new(0.0, 0.0, 4.0, 4.0),
            fill: Fill::Gradient(clear),
            ..BoxItem::default()
        };
        assert!(rig.encode(&one(Item::Box(b))).instances.is_empty());
    }

    #[test]
    fn css_gradient_lines() {
        let size = Vec2::new(40.0, 20.0);
        // 90 degrees: left to right across the box.
        let (a, b) = crate::css_linear_line(90.0, size);
        assert!(
            (a - Vec2::new(0.0, 10.0)).length() < 1e-4
                && (b - Vec2::new(40.0, 10.0)).length() < 1e-4
        );
        // `to bottom right`: the 50 % line joins the top right and bottom left corners.
        let deg = crate::css_corner_angle(true, true, size);
        let (a, b) = crate::css_linear_line(deg, size);
        let d = b - a;
        let t = |p: Vec2| (p - a).dot(d) / d.length_squared();
        assert!((t(Vec2::new(40.0, 0.0)) - 0.5).abs() < 1e-5);
        assert!((t(Vec2::new(0.0, 20.0)) - 0.5).abs() < 1e-5);
        assert!(t(Vec2::ZERO).abs() < 1e-5 && (t(size) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn elliptic_box_carries_its_ratio_and_fits_radii_against_the_height() {
        let mut rig = Rig::new();
        let e = EllipticBox::ellipse(
            Rect::new(0.0, 0.0, 120.0, 48.0),
            Fill::None,
            Border::uniform(2.0, R),
        );
        let i = rig.encode(&one(Item::EllipticBox(e))).instances[0];
        assert_eq!(i.flags, FLAG_BORDER | FLAG_ELLIPSE);
        assert_eq!(i.radii, [60.0; 4].map(f16_bits));
        assert_eq!(i.extra[2], f16_bits(0.4));
        // Too large vertical radii are scaled down together: ry = 2 * rx would need 2 * 60 * 2 > 48.
        let tall = EllipticBox {
            base: BoxItem {
                radii: Radii::uniform(60.0),
                ..BoxItem::solid(Rect::new(0.0, 0.0, 120.0, 48.0), R)
            },
            ratio: 2.0,
        };
        let i = rig.encode(&one(Item::EllipticBox(tall))).instances[0];
        assert_eq!(i.radii, [12.0; 4].map(f16_bits));
        // Ratio 1 is a plain box.
        let round = EllipticBox::ellipse(
            Rect::new(0.0, 0.0, 10.0, 10.0),
            Fill::Solid(R),
            Border::NONE,
        );
        let i = rig.encode(&one(Item::EllipticBox(round))).instances[0];
        assert_eq!(i.flags, FLAG_FILL);
    }

    #[test]
    fn shadow_bytes() {
        let mut rig = Rig::new();
        let s = ShadowItem {
            rect: Rect::new(10.0, 20.0, 100.0, 50.0),
            radii: Radii::uniform(6.0),
            offset: Vec2::new(0.0, 8.0),
            blur: 24.0,
            spread: -2.0,
            color: Color::rgba(0, 0, 0, 89),
            ..ShadowItem::default()
        };
        let i = rig.encode(&one(Item::Shadow(s.clone()))).instances[0];
        assert_eq!(i.kind, KIND_SHADOW);
        assert_eq!(i.rect, [10.0, 20.0, 100.0, 50.0]);
        assert_eq!(i.uv, [0.0, 8.0, 0.0, 0.0]);
        assert_eq!(i.extra, [12.0, -2.0, 0.0, 0.0].map(f16_bits));
        assert_eq!(i.colors[0], [0, 0, 0, 89]);
        for none in [
            ShadowItem {
                color: Color::TRANSPARENT,
                ..s.clone()
            },
            ShadowItem {
                spread: -60.0,
                ..s.clone()
            },
            ShadowItem {
                blur: f32::NAN,
                ..s
            },
        ] {
            assert!(rig.encode(&one(Item::Shadow(none))).instances.is_empty());
        }
    }

    #[test]
    fn inset_shadow_bytes() {
        let mut rig = Rig::new();
        let s = ShadowItem {
            rect: Rect::new(11.0, 21.0, 98.0, 48.0),
            radii: Radii::uniform(5.0),
            offset: Vec2::new(2.0, -3.0),
            blur: 6.0,
            spread: 80.0,
            color: Color::rgba(0, 0, 0, 128),
            ..ShadowItem::default()
        };
        let i = rig.encode(&one(Item::InsetShadow(s.clone()))).instances[0];
        assert_eq!(i.kind, KIND_SHADOW);
        assert_eq!(i.flags, FLAG_INSET);
        assert_eq!(i.rect, [11.0, 21.0, 98.0, 48.0]);
        assert_eq!(i.radii, [5.0; 4].map(f16_bits));
        assert_eq!(i.uv, [2.0, -3.0, 0.0, 0.0]);
        // A spread larger than the box closes the hole: the whole box is shadow, still drawn.
        assert_eq!(i.extra, [3.0, 80.0, 0.0, 0.0].map(f16_bits));
        // The outer form of the same item carries no flag.
        let o = rig.encode(&one(Item::Shadow(s.clone()))).instances[0];
        assert_eq!(o.flags, 0);
        for none in [
            ShadowItem {
                color: Color::TRANSPARENT,
                ..s.clone()
            },
            ShadowItem {
                rect: Rect::new(0.0, 0.0, 0.0, 10.0),
                ..s
            },
        ] {
            assert!(
                rig.encode(&one(Item::InsetShadow(none)))
                    .instances
                    .is_empty()
            );
        }
    }

    fn ext_image(rect: Rect, tex: u64, nine: Option<NineSlice>) -> Item {
        Item::Image(ImageItem {
            rect,
            image: ImageRef {
                layer: 3,
                uv: [0.0, 0.0, 1.0, 0.5],
            },
            nine,
            external: Some(crate::ExtTexture(tex)),
            ..ImageItem::default()
        })
    }

    #[test]
    fn external_pictures_share_a_slot_and_carry_the_flag() {
        let mut rig = Rig::new();
        let mut b = ChunkBuilder::new();
        b.push(ext_image(Rect::new(0.0, 0.0, 8.0, 8.0), 77, None));
        b.push(ext_image(Rect::new(8.0, 0.0, 8.0, 8.0), 77, None));
        b.push(ext_image(Rect::new(16.0, 0.0, 8.0, 8.0), 78, None));
        let e = rig.encode(&b.build());
        let ext: Vec<u16> = e.instances.iter().map(|i| i.ext).collect();
        assert_eq!(ext, vec![1, 1, 2]);
        assert_eq!(e.exts, vec![1, 1, 2]);
        let i = e.instances[0];
        assert_eq!(i.kind, KIND_IMAGE);
        assert_eq!(i.flags, FLAG_EXT);
        // The uv is the texture's own, the page layer is not used.
        assert_eq!((i.uv, i.atlas_layer), ([0.0, 0.0, 1.0, 0.5], 0));
        assert_eq!(rig.exts.get(2), Some(crate::ExtTexture(78)));
        rig.exts.release(2);
        assert_eq!(rig.exts.get(2), None);
        assert_eq!(rig.exts.live(), 1);
    }

    #[test]
    fn nine_slice_makes_nine_pictures_with_the_css_geometry() {
        let mut rig = Rig::new();
        let n = NineSlice {
            src: [0.25, 0.25, 0.5, 0.25],
            dst: [4.0, 6.0, 8.0, 2.0],
            fill: true,
        };
        let e = rig.encode(&one(ext_image(
            Rect::new(10.0, 20.0, 40.0, 30.0),
            5,
            Some(n),
        )));
        assert_eq!(e.instances.len(), 9);
        // The middle: x 12..44, y 24..42; uv x 0.25..0.75, y (of 0..0.5) 0.125..0.25.
        let mid = e.instances[4];
        assert_eq!(mid.rect, [12.0, 24.0, 32.0, 18.0]);
        assert_eq!(mid.uv, [0.25, 0.125, 0.75, 0.25]);
        // The bottom right corner.
        let br = e.instances[8];
        assert_eq!(br.rect, [44.0, 42.0, 6.0, 8.0]);
        assert_eq!(br.uv, [0.75, 0.25, 1.0, 0.5]);
        assert!(e.instances.iter().all(|i| i.ext == 1 && i.radii == [0; 4]));
        // No fill: eight pieces at most. Widths too large for the box are scaled down together (here to 10 px,
        // which leaves the middle row without height: corners, top and bottom remain).
        let wide = NineSlice {
            src: [0.25; 4],
            dst: [20.0; 4],
            fill: false,
        };
        let e = rig.encode(&one(ext_image(
            Rect::new(0.0, 0.0, 40.0, 20.0),
            5,
            Some(wide),
        )));
        assert_eq!(e.instances.len(), 6);
        assert_eq!(e.instances[0].rect, [0.0, 0.0, 10.0, 10.0]);
        // Zero widths leave out the pieces they empty.
        let thin = NineSlice {
            src: [0.25; 4],
            dst: [0.0, 4.0, 0.0, 4.0],
            fill: true,
        };
        let e = rig.encode(&one(ext_image(
            Rect::new(0.0, 0.0, 40.0, 20.0),
            5,
            Some(thin),
        )));
        assert_eq!(e.instances.len(), 3);
    }

    #[test]
    fn unresolved_glyphs_make_placeholders_and_queue() {
        let mut rig = Rig::new();
        let g = Glyphs {
            face: ui2_text::FaceId(0),
            size_px: 14.0,
            wght: 400.0,
            color: Color::rgba(255, 255, 255, 255),
            glyphs: vec![
                (5, 10.0, 20.0).into(),
                (6, 17.3, 20.0).into(),
                (5, 24.0, 20.0).into(),
            ],
            transform: None,
            clip: None,
        };
        let e = rig.encode(&one(Item::Glyphs(g)));
        assert_eq!(e.instances.len(), 3);
        assert_eq!(e.unresolved, 3);
        for i in &e.instances {
            assert_eq!(i.kind, KIND_MASK);
            assert_eq!(i.rect, [0.0; 4]);
            assert_eq!(i.colors[0], [255, 255, 255, 255]);
        }
        // Glyph 5 at pen x 10.0 and 24.0 share a key (subpixel 0); glyph 6 at 17.3 is subpixel 1.
        assert_eq!(rig.glyphs.pending_len(), 2);
    }
}
