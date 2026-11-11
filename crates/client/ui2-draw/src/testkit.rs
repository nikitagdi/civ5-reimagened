//! Ready-made scenes for tests of the draw side and of the GPU pass (feature `testkit`).
//!
//! - [`icon_pages`]: a package page table with one `A8` page of four 16 x 16 icons ([`icon`]).
//! - [`plate`]: a bevelled plate (polygon clip, hard-stop gradient along the cut); [`dial`]: the end-turn dial
//!   (conic scale, rotated ticks, an arc of two clipped halves).
//! - [`hud_a`]: the heads-up display of the reference mockup as the runtime would hand it over: the top strip
//!   with its groups, separators, icons and labels and the research plate; the notes column and the unit panel
//!   with their shadows, the unit portrait ([`PORTRAIT`], an external texture) and action buttons; the minimap
//!   ([`ExtTexture::MINIMAP`]) in its frame and the end-turn dial. Sizes are in CSS px times the scale factor.
//! - [`map_banners`]: `n` city banners on the map layer, one chunk and one anchor each.
//!
//! The scenes only use public items, so what they exercise is what the runtime can emit.

use std::sync::Arc;

use glam::{Affine2, Vec2};
use ui2_package::{BlobRef, BlobSection, Images, Page, PageFormat};
use ui2_text::{FaceId, RunStyle, TextEngine};

use crate::{
    AnchorSlot, AnchorXform, Border, BoxItem, ChunkBuilder, ChunkId, ChunkItems, Clip, Color,
    ExtTexture, Fill, Glyphs, Gradient, GradientStop, ImageItem, ImageRef, Item, LayerId, MaskItem,
    Polygon, Radii, Rect, Scene, ShadowItem, css_corner_angle, css_linear_line,
};

/// Background lacquer of the mockup.
pub const LACQUER: Color = Color::rgba(27, 24, 21, 255);
/// A lighter lacquer (chips, keys).
pub const LACQUER_2: Color = Color::rgba(42, 36, 31, 255);
/// Engraved lines.
pub const ENGRAVE: Color = Color::rgba(77, 64, 50, 255);
/// Text colour.
pub const BONE: Color = Color::rgba(233, 223, 207, 255);
/// Accent.
pub const BRASS: Color = Color::rgba(201, 163, 106, 255);

/// The external texture id [`hud_a`] uses for the unit portrait (a game asset id in the renderer's space).
pub const PORTRAIT: ExtTexture = ExtTexture(0x7000_0001);

/// Side of the icon page.
pub const ICON_PAGE: u16 = 64;
/// Side of one icon on the page.
pub const ICON: u16 = 16;

/// Coverage of icon `n` (0: disc, 1: square, 2: triangle, 3: ring) at pixel `(x, y)` of its 16 x 16 cell.
fn icon_coverage(n: usize, x: u16, y: u16) -> u8 {
    let (fx, fy) = (f32::from(x) + 0.5 - 8.0, f32::from(y) + 0.5 - 8.0);
    let r = (fx * fx + fy * fy).sqrt();
    let on = match n {
        0 => r <= 6.0,
        1 => fx.abs() <= 5.0 && fy.abs() <= 5.0,
        2 => (-6.0..=6.0).contains(&fy) && fx.abs() <= (fy + 6.0) * 0.5,
        _ => (4.0..=6.5).contains(&r),
    };
    if on { 255 } else { 0 }
}

fn page(format: PageFormat, size: [u16; 2]) -> Page {
    Page {
        format,
        scale: 1,
        size,
        blob: BlobRef {
            section: BlobSection::Imgs,
            offset: 0,
            len: 0,
            raw_len: 0,
            zstd: false,
        },
        blake3: [0; 32],
    }
}

/// A page table with one `A8` page of [`ICON_PAGE`] px: four icons in its top row (see [`icon`]), the rest empty.
pub fn icon_pages() -> (Images, Vec<Arc<[u8]>>) {
    let side = ICON_PAGE;
    let mut px = vec![0u8; usize::from(side) * usize::from(side)];
    for n in 0..4 {
        for y in 0..ICON {
            for x in 0..ICON {
                let at =
                    usize::from(y) * usize::from(side) + n * usize::from(ICON) + usize::from(x);
                px[at] = icon_coverage(n, x, y);
            }
        }
    }
    (
        Images {
            pages: vec![page(PageFormat::A8, [side, side])],
            imgs: Vec::new(),
        },
        vec![Arc::from(px)],
    )
}

/// The sprite of icon `n` (0..4) of [`icon_pages`].
pub fn icon(n: u8) -> ImageRef {
    let s = f32::from(ICON) / f32::from(ICON_PAGE);
    let u = f32::from(n % 4) * s;
    ImageRef {
        layer: 0,
        uv: [u, 0.0, u + s, s],
    }
}

/// Shapes `text` and places it with its pen at `(x, baseline)` (device px), sizes scaled by `k`. An unknown face
/// gives no item.
fn label(
    text: &mut TextEngine,
    face: FaceId,
    size: f32,
    k: f32,
    s: &str,
    at: Vec2,
    color: Color,
) -> Option<Item> {
    let run = text.shape(&RunStyle::new(face, size), s).ok()?;
    Some(Item::Glyphs(Glyphs::from_run(&run, k, at.x, at.y, color)))
}

/// Builds items in CSS px, scaled by `k` into device px.
struct Kit<'a> {
    k: f32,
    text: &'a mut TextEngine,
    face: FaceId,
    out: ChunkBuilder,
}

impl Kit<'_> {
    fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::new(x * self.k, y * self.k, w * self.k, h * self.k)
    }

    fn fill(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color, radius: f32) {
        let b = BoxItem {
            radii: Radii::uniform(radius * self.k),
            ..BoxItem::solid(self.r(x, y, w, h), c)
        };
        self.out.push(Item::Box(b));
    }

    /// A filled box with a 1 px engraved ring inside (`box-shadow: inset 0 0 0 1px`).
    fn panel(&mut self, x: f32, y: f32, w: f32, h: f32, c: Color, radius: f32) {
        let b = BoxItem {
            rect: self.r(x, y, w, h),
            fill: Fill::Solid(c),
            border: Border::uniform(self.k.round().max(1.0), ENGRAVE),
            radii: Radii::uniform(radius * self.k),
            ..BoxItem::default()
        };
        self.out.push(Item::Box(b));
    }

    /// A hairline: a box with only one border side.
    fn line(&mut self, x: f32, y: f32, w: f32, h: f32, side: usize) {
        let mut border = Border::NONE;
        let t = self.k.round().max(1.0);
        match side {
            0 => border.top = t,
            1 => border.right = t,
            2 => border.bottom = t,
            _ => border.left = t,
        }
        border.color = ENGRAVE;
        self.out.push(Item::Box(BoxItem {
            rect: self.r(x, y, w, h),
            border,
            ..BoxItem::default()
        }));
    }

    /// `box-shadow: 0 <dy>px <blur>px rgba(0, 0, 0, .35)` of a box.
    #[allow(clippy::too_many_arguments)]
    fn shadow(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, dy: f32, blur: f32) {
        self.out.push(Item::Shadow(ShadowItem {
            rect: self.r(x, y, w, h),
            radii: Radii::uniform(radius * self.k),
            offset: Vec2::new(0.0, dy * self.k),
            blur: blur * self.k,
            color: Color::rgba(0, 0, 0, 89),
            ..ShadowItem::default()
        }));
    }

    fn icon(&mut self, n: u8, x: f32, y: f32, size: f32, color: Color) {
        self.out.push(Item::Mask(MaskItem {
            rect: self.r(x, y, size, size),
            mask: icon(n),
            color,
            ..MaskItem::default()
        }));
    }

    fn text(&mut self, s: &str, x: f32, baseline: f32, size: f32, color: Color) {
        let at = Vec2::new(x * self.k, baseline * self.k);
        if let Some(item) = label(self.text, self.face, size, self.k, s, at, color) {
            self.out.push(item);
        }
    }

    fn take(&mut self) -> ChunkItems {
        std::mem::take(&mut self.out).build()
    }
}

/// A plate of the mockup: a lacquer box with an engraved 1 px ring, the corners flagged in `bevels` (tl, tr, br,
/// bl) cut off by `bevel` px with a polygon clip (`clip-path: polygon(...)`), and an engraved line along every
/// cut drawn by a hard-stop `linear-gradient(to <corner>, ...)` in a `bevel` x `bevel` box at that corner.
pub fn plate(rect: Rect, bevel: f32, bevels: [bool; 4]) -> Vec<Item> {
    let (x0, y0, x1, y1) = (rect.x, rect.y, rect.x + rect.w, rect.y + rect.h);
    let c = bevel;
    let mut pts = Vec::new();
    let v = Vec2::new;
    if bevels[0] {
        pts.extend([v(x0, y0 + c), v(x0 + c, y0)]);
    } else {
        pts.push(v(x0, y0));
    }
    if bevels[1] {
        pts.extend([v(x1 - c, y0), v(x1, y0 + c)]);
    } else {
        pts.push(v(x1, y0));
    }
    if bevels[2] {
        pts.extend([v(x1, y1 - c), v(x1 - c, y1)]);
    } else {
        pts.push(v(x1, y1));
    }
    if bevels[3] {
        pts.extend([v(x0 + c, y1), v(x0, y1 - c)]);
    } else {
        pts.push(v(x0, y1));
    }
    let clip = Clip::polygon(&pts).ok();
    let mut out = vec![Item::Box(BoxItem {
        rect,
        fill: Fill::Solid(LACQUER),
        border: Border::uniform(1.0, ENGRAVE),
        clip,
        ..BoxItem::default()
    })];
    let size = Vec2::splat(c);
    // The 50 % line of `to <corner>` joins the two other corners: the cut. Engraved from 50 % - 1 px to
    // 50 % + 0.5 px along the gradient line.
    let len = c * std::f32::consts::SQRT_2;
    let (a, b) = (0.5 - 1.0 / len, 0.5 + 0.5 / len);
    let stops = [
        GradientStop::new(Color::TRANSPARENT, a),
        GradientStop::new(ENGRAVE, a),
        GradientStop::new(ENGRAVE, b),
        GradientStop::new(Color::TRANSPARENT, b),
    ];
    for (k, (right, bottom)) in [(false, false), (true, false), (true, true), (false, true)]
        .into_iter()
        .enumerate()
    {
        if !bevels[k] {
            continue;
        }
        let (from, to) = css_linear_line(css_corner_angle(right, bottom, size), size);
        let Ok(g) = Gradient::linear(from, to, &stops) else {
            continue;
        };
        let at = Vec2::new(
            if right { x1 - c } else { x0 },
            if bottom { y1 - c } else { y0 },
        );
        out.push(Item::Box(BoxItem {
            rect: Rect::new(at.x, at.y, c, c),
            fill: Fill::Gradient(g),
            clip,
            ..BoxItem::default()
        }));
    }
    out
}

/// The end-turn dial of the mockup (the conic-gradient design): a disc with an engraved ring, a scale of
/// `repeating-conic-gradient` ticks (1 degree of every 9), 40 rotated ticks, and a progress arc of `progress`
/// (0..1) made of two half rings, each rotated and clipped to its half.
pub fn dial(center: Vec2, radius: f32, progress: f32) -> Vec<Item> {
    let r = radius;
    let sq = |r: f32| Rect::new(center.x - r, center.y - r, 2.0 * r, 2.0 * r);
    let mut out = vec![Item::Box(BoxItem {
        rect: sq(r),
        fill: Fill::Solid(LACQUER),
        border: Border::uniform(1.0, ENGRAVE),
        radii: Radii::uniform(r),
        ..BoxItem::default()
    })];
    // The scale: a ring between r - 12 and r - 4 of a repeating conic gradient.
    let deg = 1.0 / 360.0;
    if let Ok(g) = Gradient::conic(
        Vec2::splat(r - 4.0),
        0.0,
        &[
            GradientStop::new(ENGRAVE, 0.0),
            GradientStop::new(ENGRAVE, deg),
            GradientStop::new(Color::TRANSPARENT, deg),
            GradientStop::new(Color::TRANSPARENT, 9.0 * deg),
        ],
    ) {
        out.push(Item::Box(BoxItem {
            rect: sq(r - 4.0),
            fill: Fill::Gradient(g.repeating()),
            radii: Radii::uniform(r - 4.0),
            ..BoxItem::default()
        }));
    }
    out.push(Item::Box(BoxItem {
        radii: Radii::uniform(r - 12.0),
        ..BoxItem::solid(sq(r - 12.0), LACQUER)
    }));
    // 40 ticks, each a 2 x 6 box at the top rotated about the centre.
    for i in 0..40 {
        let a = i as f32 / 40.0 * std::f32::consts::TAU;
        let m = Affine2::from_translation(center)
            * Affine2::from_angle(a)
            * Affine2::from_translation(-center);
        out.push(Item::Box(BoxItem {
            transform: Some(m),
            ..BoxItem::solid(
                Rect::new(center.x - 1.0, center.y - r + 14.0, 2.0, 6.0),
                BONE,
            )
        }));
    }
    // The progress arc: a ring of radius r - 24, 4 px wide, in two halves as the mockup builds it: a half ring
    // rotated inside a clip of one half of the dial. The rotated half ring's own edge becomes, in chunk space, a
    // half plane through the centre at the rotated angle: the runtime flattens such nested clips into one clip
    // rectangle (the dial's half) and one polygon (the rotated half plane).
    let ring = |turns: f32, right: bool| {
        let ar = r - 24.0;
        let half = if right {
            Rect::new(center.x, center.y - r, r, 2.0 * r)
        } else {
            Rect::new(center.x - r, center.y - r, r, 2.0 * r)
        };
        // The half plane left of the vertical through the centre (right of it for the second half), rotated
        // clockwise by `turns`.
        let m = Affine2::from_angle(turns * std::f32::consts::TAU);
        let s = if right { -1.0 } else { 1.0 };
        let b = 2.0 * r;
        let quad: Vec<Vec2> = [(0.0, -b), (s * b, -b), (s * b, b), (0.0, b)]
            .into_iter()
            .map(|(x, y)| center + m.transform_vector2(Vec2::new(x, y)))
            .collect();
        Item::Box(BoxItem {
            rect: Rect::new(center.x - ar, center.y - ar, 2.0 * ar, 2.0 * ar),
            border: Border::uniform(4.0, BRASS),
            radii: Radii::uniform(ar),
            clip: Some(Clip {
                rect: Some(half),
                radius: 0.0,
                polygon: Polygon::new(&quad).ok(),
            }),
            ..BoxItem::default()
        })
    };
    // Clockwise from the top: the right half shows angles 0..p of the turn, the left half 0.5..p.
    let p = progress.clamp(0.0, 1.0);
    if p > 0.0 {
        out.push(ring(p.min(0.5), true));
    }
    if p > 0.5 {
        out.push(ring(p - 0.5, false));
    }
    out
}

/// Chunk ids of [`hud_a`] start here.
pub const HUD_CHUNKS: u32 = 100;

/// The heads-up display of the reference mockup (1920 x 1080 CSS px) at scale `k`, as chunks of the `Hud`
/// layer: top strip, notes, unit panel, minimap frame, end-turn dial. Masks refer to [`icon_pages`]; text is
/// shaped with `face` (missing faces give no text).
pub fn hud_a(text: &mut TextEngine, face: FaceId, k: f32) -> Vec<(LayerId, ChunkId, ChunkItems)> {
    let mut kit = Kit {
        k,
        text,
        face,
        out: ChunkBuilder::new(),
    };
    let mut chunks = Vec::new();
    let mut id = HUD_CHUNKS;
    let mut push = |items: ChunkItems, chunks: &mut Vec<_>| {
        chunks.push((LayerId::Hud, ChunkId(id), items));
        id += 1;
    };

    // Top strip: 1920 x 40, groups separated by hairlines, an icon and a value each.
    kit.fill(0.0, 0.0, 1920.0, 40.0, LACQUER, 0.0);
    kit.line(0.0, 0.0, 1920.0, 40.0, 2);
    let groups = [
        ("Research: Education", 3),
        ("Menus", 1),
        ("+24", 4),
        ("+18", 2),
        ("1 204", 3),
        ("Iron 4", 1),
        ("Horses 2", 1),
        ("Turn 87", 2),
    ];
    let mut x = 12.0;
    for (i, (s, icons)) in groups.iter().enumerate() {
        if i > 0 {
            kit.line(x - 12.0, 0.0, 1.0, 40.0, 3);
        }
        for j in 0..*icons {
            kit.icon((i + j) as u8 % 4, x, 12.0, 16.0, BRASS);
            x += 22.0;
        }
        kit.text(s, x, 25.0, 13.0, BONE);
        x += 12.0 * s.len() as f32 * 0.6 + 24.0;
    }
    // The research plate hanging under the strip, bevelled at the bottom.
    for item in plate(
        kit.r(0.0, 40.0, 320.0, 48.0),
        10.0 * k,
        [false, false, true, true],
    ) {
        kit.out.push(item);
    }
    kit.text("Education: 4 turns", 16.0, 70.0, 13.0, BONE);
    push(kit.take(), &mut chunks);

    // Notes column on the right: cards with a title and a line, each with its shadow.
    for i in 0..4 {
        let y = 56.0 + 64.0 * i as f32;
        kit.shadow(1600.0, y, 304.0, 56.0, 6.0, 8.0, 24.0);
        kit.panel(1600.0, y, 304.0, 56.0, LACQUER, 6.0);
        kit.icon(i as u8, 1612.0, y + 12.0, 18.0, BRASS);
        kit.text("Great Library completed", 1640.0, y + 24.0, 13.0, BONE);
        kit.text("Novgorod, 3 turns ago", 1640.0, y + 42.0, 12.0, ENGRAVE);
    }
    kit.fill(1768.0, 320.0, 136.0, 24.0, LACQUER, 4.0);
    kit.text("More", 1820.0, 336.0, 12.0, BONE);
    push(kit.take(), &mut chunks);

    // Unit panel: shadow, portrait frame, name, stats, pips, eight actions with keys.
    kit.shadow(400.0, 880.0, 760.0, 184.0, 8.0, 8.0, 24.0);
    kit.panel(400.0, 880.0, 760.0, 184.0, LACQUER, 8.0);
    kit.panel(416.0, 896.0, 104.0, 104.0, LACQUER_2, 4.0);
    // The unit portrait: a game asset of the `ui:tx` store.
    kit.out.push(Item::Image(ImageItem {
        rect: kit.r(420.0, 900.0, 96.0, 96.0),
        image: ImageRef {
            layer: 0,
            uv: [0.0, 0.0, 1.0, 1.0],
        },
        radii: Radii::uniform(3.0 * k),
        external: Some(PORTRAIT),
        ..ImageItem::default()
    }));
    kit.text("Swordsman", 536.0, 916.0, 18.0, BONE);
    for (i, s) in ["Strength 14", "Moves 2/2", "XP 12/30"].iter().enumerate() {
        let x = 536.0 + 120.0 * i as f32;
        if i > 0 {
            kit.line(x - 12.0, 928.0, 1.0, 24.0, 3);
        }
        kit.text(s, x, 946.0, 13.0, BONE);
    }
    for i in 0..5 {
        let x = 536.0 + 14.0 * i as f32;
        if i < 3 {
            kit.fill(x, 964.0, 10.0, 6.0, BRASS, 2.0);
        } else {
            kit.panel(x, 964.0, 10.0, 6.0, Color::TRANSPARENT, 2.0);
        }
    }
    let actions = [
        "Move", "Attack", "Fortify", "Sleep", "Alert", "Pillage", "Upgrade", "Delete",
    ];
    for (i, s) in actions.iter().enumerate() {
        let x = 536.0 + 76.0 * i as f32;
        kit.panel(x, 1000.0, 72.0, 48.0, LACQUER_2, 4.0);
        kit.icon(i as u8 % 4, x + 28.0, 1006.0, 16.0, BONE);
        kit.text(s, x + 8.0, 1040.0, 11.0, BONE);
        if i >= 2 {
            kit.panel(x + 54.0, 1004.0, 14.0, 14.0, LACQUER, 3.0);
            kit.text(&s[..1], x + 57.0, 1015.0, 10.0, BONE);
        }
    }
    push(kit.take(), &mut chunks);

    // Minimap: the frame, the map (the renderer's minimap target) in its rounded clip, the view frame over it.
    kit.panel(16.0, 920.0, 368.0, 144.0, LACQUER, 4.0);
    let clip = Clip::rounded(kit.r(16.0, 920.0, 368.0, 144.0), 4.0 * k);
    kit.out.push(Item::Image(ImageItem {
        rect: kit.r(17.0, 921.0, 366.0, 142.0),
        image: ImageRef {
            layer: 0,
            uv: [0.0, 0.0, 1.0, 1.0],
        },
        clip: Some(clip),
        external: Some(ExtTexture::MINIMAP),
        ..ImageItem::default()
    }));
    kit.out.push(Item::Box(BoxItem {
        clip: Some(clip),
        ..BoxItem::solid(
            kit.r(100.0, 950.0, 60.0, 40.0),
            Color::rgba(255, 255, 255, 64),
        )
    }));
    push(kit.take(), &mut chunks);

    // End-turn dial with its shadow: disc, conic scale, rotated ticks, progress arc, the turn number.
    kit.shadow(1688.0, 848.0, 216.0, 216.0, 108.0, 8.0, 24.0);
    for item in dial(Vec2::new(1796.0, 956.0) * k, 108.0 * k, 0.7) {
        kit.out.push(item);
    }
    kit.text("87", 1780.0, 966.0, 28.0, BONE);
    kit.text("Next turn", 1764.0, 1000.0, 12.0, BRASS);
    push(kit.take(), &mut chunks);

    chunks
}

/// Puts the chunks of [`hud_a`] into `scene` (the icon pages too) and resolves its glyphs.
pub fn load_hud_a(scene: &mut Scene, text: &mut TextEngine, face: FaceId) {
    let (images, pages) = icon_pages();
    // The table is well formed, so this cannot fail.
    let _ = scene.set_pages(&images, pages);
    for (layer, id, items) in hud_a(text, face, scene.scale()) {
        scene.set_chunk(layer, id, items);
    }
    scene.resolve(text, 1000.0);
}

/// Chunk ids of [`map_banners`] start here.
pub const BANNER_CHUNKS: u32 = 10_000;

/// One city banner in its own space (origin at the anchor): a body with a ring, a name, a strength tab.
pub fn banner(
    text: &mut TextEngine,
    face: FaceId,
    k: f32,
    name: &str,
    slot: AnchorSlot,
) -> ChunkItems {
    let mut kit = Kit {
        k,
        text,
        face,
        out: ChunkBuilder::new().anchor(slot),
    };
    kit.panel(-80.0, -16.0, 160.0, 32.0, LACQUER, 4.0);
    kit.fill(-80.0, -16.0, 28.0, 32.0, BRASS, 4.0);
    kit.icon(0, -74.0, -8.0, 16.0, LACQUER);
    kit.text(name, -44.0, 5.0, 14.0, BONE);
    kit.panel(52.0, -24.0, 24.0, 16.0, LACQUER_2, 3.0);
    kit.text("12", 56.0, -12.0, 11.0, BONE);
    kit.take()
}

/// `n` city banners on the `Map` layer, each with its own anchor placed on a grid of `size` (device px). Returns
/// the anchors in banner order.
pub fn map_banners(
    scene: &mut Scene,
    text: &mut TextEngine,
    face: FaceId,
    n: usize,
    size: Vec2,
) -> Vec<AnchorSlot> {
    let k = scene.scale();
    let cols = 10usize;
    let mut slots = Vec::with_capacity(n);
    for i in 0..n {
        let Ok(slot) = scene.alloc_anchor() else {
            break;
        };
        let (cx, cy) = ((i % cols) as f32 + 0.5, (i / cols) as f32 + 0.5);
        let rows = n.div_ceil(cols) as f32;
        scene.set_anchor(
            slot,
            AnchorXform {
                pos: Vec2::new(cx * size.x / cols as f32, cy * size.y / rows),
                scale: 1.0,
                visible: true,
            },
        );
        let names = ["Novgorod", "Moscow", "Kazan", "Kiev", "Minsk"];
        let items = banner(text, face, k, names[i % names.len()], slot);
        scene.set_chunk(LayerId::Map, ChunkId(BANNER_CHUNKS + i as u32), items);
        slots.push(slot);
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_page_holds_four_icons() {
        let (images, pages) = icon_pages();
        assert_eq!(images.pages.len(), 1);
        assert_eq!(pages[0].len(), 64 * 64);
        // The disc's centre is covered, its corner is not; the ring's centre is empty.
        assert_eq!(pages[0][8 * 64 + 8], 255);
        assert_eq!(pages[0][0], 0);
        assert_eq!(pages[0][8 * 64 + 48 + 8], 0);
        assert_eq!(icon(1).uv, [0.25, 0.0, 0.5, 0.25]);
    }

    #[test]
    fn hud_without_a_face_still_has_boxes_and_icons() {
        let mut text = TextEngine::new();
        let chunks = hud_a(&mut text, FaceId(9), 1.0);
        assert_eq!(chunks.len(), 5);
        let items: usize = chunks.iter().map(|c| c.2.items.len()).sum();
        assert!(items > 80, "{items}");
        assert!(
            chunks
                .iter()
                .flat_map(|c| &c.2.items)
                .all(|i| !matches!(i, Item::Glyphs(_)))
        );
    }

    #[test]
    fn banners_take_one_anchor_and_chunk_each() {
        let mut text = TextEngine::new();
        let mut scene = Scene::new(1.0);
        let slots = map_banners(
            &mut scene,
            &mut text,
            FaceId(9),
            200,
            Vec2::new(1920.0, 1080.0),
        );
        assert_eq!(slots.len(), 200);
        assert_eq!(scene.stats().chunks, 200);
        assert_eq!(scene.layer_instances(LayerId::Map).len(), 200 * 4);
        assert!(
            scene
                .layer_instances(LayerId::Map)
                .iter()
                .all(|i| i.anchor != 0)
        );
    }
}
