//! Item to instance encoding.
//!
//! One box makes one instance (fill and border together), one mask, picture or glyph one instance. Boxes
//! with nothing to show, rectangles without area and pictures of a page the scene does not hold make none. Sprite
//! rectangles of package pages are mapped from page to array coordinates ([`Pages`]). Transforms and clips are interned in the
//! [`Tables`]; the instance carries their indices and the chunk's anchor.

use glam::Affine2;

use crate::glyphs::{GlyphStore, Lookup, glyph_key};
use crate::instance::{
    FILL_SOLID, FLAG_BORDER, FLAG_FILL, KIND_BOX, KIND_IMAGE, KIND_MASK, Ui2Instance,
};
use crate::pages::Pages;
use crate::tables::{Clip, Tables};
use crate::{BoxItem, ChunkItems, Fill, Glyphs, ImageItem, Item, MaskItem};

/// What the encoder works on besides the items.
pub struct EncodeCtx<'a> {
    /// Transform and clip tables (entries are interned here).
    pub tables: &'a mut Tables,
    /// Glyph atlas and queue.
    pub glyphs: &'a mut GlyphStore,
    /// Incremented for every transform or clip that did not fit in its table (the item is drawn without it).
    pub table_overflow: &'a mut u32,
    /// The package's picture pages.
    pub pages: &'a Pages,
    /// Side of the layers of the coverage array (mask pages and glyph pages), px.
    pub mask_side: u32,
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
    /// Glyphs that were queued for rasterization (their instances are placeholders until then).
    pub unresolved: u32,
}

/// Encodes a whole chunk.
pub fn encode_chunk(chunk: &ChunkItems, ctx: &mut EncodeCtx<'_>) -> Encoded {
    let mut out = Encoded::default();
    let anchor = chunk.anchor.map_or(0, |a| a.0);
    for item in &chunk.items {
        match item {
            Item::Box(b) => encode_box(b, anchor, ctx, &mut out),
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

fn encode_box(b: &BoxItem, anchor: u16, ctx: &mut EncodeCtx<'_>, out: &mut Encoded) {
    if !b.rect.is_drawable() {
        return;
    }
    let fill = match b.fill {
        Fill::Solid(c) if !c.is_transparent() => Some(c),
        _ => None,
    };
    let border = (!b.border.is_none()).then_some(b.border);
    if fill.is_none() && border.is_none() {
        return;
    }
    let (xform, clip) = place(&b.transform, &b.clip, ctx, out);
    let mut i = Ui2Instance::of_kind(KIND_BOX);
    i.rect = [b.rect.x, b.rect.y, b.rect.w, b.rect.h];
    let r = b.radii.fitted(b.rect.w, b.rect.h);
    i.set_radii([r.tl, r.tr, r.br, r.bl]);
    i.flags = FILL_SOLID;
    if let Some(c) = fill {
        i.colors[0] = c.0;
        i.flags |= FLAG_FILL;
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

/// A colour picture: the sprite of an `Rgba8` page times the tint, cut to the rounded rectangle. A picture whose
/// page the scene does not hold makes no instance.
fn encode_image(m: &ImageItem, anchor: u16, ctx: &mut EncodeCtx<'_>, out: &mut Encoded) {
    if !m.rect.is_drawable() || m.tint.is_transparent() {
        return;
    }
    let Some(uv) = ctx.pages.image_uv(m.image.layer, m.image.uv) else {
        return;
    };
    let (xform, clip) = place(&m.transform, &m.clip, ctx, out);
    let mut i = Ui2Instance::of_kind(KIND_IMAGE);
    i.rect = [m.rect.x, m.rect.y, m.rect.w, m.rect.h];
    let r = m.radii.fitted(m.rect.w, m.rect.h);
    i.set_radii([r.tl, r.tr, r.br, r.bl]);
    i.uv = uv;
    i.colors[0] = m.tint.0;
    i.atlas_layer = m.image.layer;
    i.xform = xform;
    i.clip = clip;
    i.anchor = anchor;
    out.instances.push(i);
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
    use crate::{AnchorSlot, Border, ChunkBuilder, Color, ImageRef, Radii, Rect};

    struct Rig {
        tables: Tables,
        glyphs: GlyphStore,
        overflow: u32,
        pages: Pages,
    }

    impl Rig {
        fn new() -> Self {
            Self {
                tables: Tables::new(),
                glyphs: GlyphStore::new(AtlasConfig::default()),
                overflow: 0,
                pages: Pages::default(),
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
