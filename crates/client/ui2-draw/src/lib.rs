//! UI v2 draw side: what the UI runtime hands to the renderer, and the bytes the renderer uploads.
//!
//! The runtime builds [`ChunkItems`] (a flat list of [`Item`]s in device pixels, one chunk per panel, screen or map
//! anchor) and gives them to a [`Scene`] under a [`LayerId`]. The scene encodes every chunk into fixed-size GPU
//! instances ([`Ui2Instance`], 96 bytes), keeps the instances of a layer in one contiguous buffer with a revision,
//! and owns the transform and clip tables ([`tables`]) and the glyph atlas ([`glyphs`]). The GPU pass (in the
//! `render` crate) only reads what the scene exposes: layer instance buffers and their change records, the dirty
//! table segments and the dirty glyph atlas rectangles. This crate has no GPU, no I/O and no clock of its own.
//!
//! Conventions:
//! - Coordinates are device pixels, y down, in the chunk's own space. A chunk placed with an [`AnchorSlot`] is
//!   mapped to the screen by the anchor's transform (`screen = pos + scale * chunk`); otherwise chunk space is
//!   screen space.
//! - Colours are straight (not premultiplied) sRGB, 8 bits per channel; the shader premultiplies.
//! - An item that cannot produce visible output (empty rectangle, transparent fill and no border) produces no
//!   instance. A glyph always produces exactly one instance (a zero-area placeholder while its coverage is not in
//!   the atlas yet), so a chunk keeps its instance count when glyphs arrive and the layer buffer can be patched in
//!   place.

#![forbid(unsafe_code)]

pub mod encode;
pub mod glyphs;
pub mod instance;
pub mod pages;
mod scene;
mod stats;
pub mod tables;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use glam::{Affine2, Vec2};

pub use glyphs::GlyphStore;
pub use instance::Ui2Instance;
pub use pages::{ExtSlots, Pages, StaticPage};
pub use scene::{Clock, FALLBACK_MS_PER_GLYPH, LayerChange, Op, Scene};
pub use stats::SceneStats;
pub use tables::{Clip, Polygon, TableSegment};
pub use ui2_text::{DirtyRect, FaceId};

/// Errors of this crate.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DrawError {
    /// All anchor slots are in use.
    #[error("no free anchor slot")]
    AnchorsExhausted,
    /// A clip polygon is not a usable convex polygon of 3 to 8 vertices.
    #[error("invalid clip polygon: {0}")]
    InvalidPolygon(&'static str),
    /// The picture pages handed to [`Scene::set_pages`] do not match the package's page table.
    #[error("invalid picture page: {0}")]
    InvalidPage(String),
    /// A gradient without 2 to 4 finite stops or with non-finite geometry.
    #[error("invalid gradient: {0}")]
    InvalidGradient(&'static str),
}

/// The layers in drawing order: later layers are drawn over earlier ones. The world is drawn before all of them,
/// outside this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LayerId {
    /// UI on the map (banners, flags), anchored to world positions.
    Map,
    /// The heads-up display.
    Hud,
    /// Windows and screens (modal ones included).
    Window,
    /// Tooltips.
    Tooltip,
}

impl LayerId {
    /// Every layer, in drawing order.
    pub const ALL: [LayerId; 4] = [Self::Map, Self::Hud, Self::Window, Self::Tooltip];

    /// Index of the layer in [`LayerId::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// Identity of a chunk, chosen by the runtime (a panel, a screen, one map anchor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkId(pub u32);

/// A slot of the anchor table: the transform that maps a chunk to the screen. Slot 0 is reserved (the screen
/// itself), so a valid slot is never 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnchorSlot(pub u16);

/// The screen placement of an anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorXform {
    /// Screen position of the chunk origin, device px.
    pub pos: Vec2,
    /// Uniform scale of the chunk.
    pub scale: f32,
    /// False: draw nothing for the chunks of this anchor (off screen, behind terrain).
    pub visible: bool,
}

/// A straight (not premultiplied) sRGB colour with 8-bit channels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color(pub [u8; 4]);

impl Color {
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color([0, 0, 0, 0]);

    /// A colour from its channels.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([r, g, b, a])
    }

    /// The alpha channel.
    pub const fn alpha(self) -> u8 {
        self.0[3]
    }

    /// True if nothing of this colour would be visible.
    pub const fn is_transparent(self) -> bool {
        self.0[3] == 0
    }
}

/// A rectangle: left, top, width, height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

impl Rect {
    /// A rectangle from its position and size.
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    /// True if the rectangle is finite and has a positive area.
    pub fn is_drawable(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.w.is_finite()
            && self.h.is_finite()
            && self.w > 0.0
            && self.h > 0.0
    }
}

/// Corner radii, clockwise from the top-left, device px.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Radii {
    /// Top-left.
    pub tl: f32,
    /// Top-right.
    pub tr: f32,
    /// Bottom-right.
    pub br: f32,
    /// Bottom-left.
    pub bl: f32,
}

impl Radii {
    /// Square corners.
    pub const ZERO: Radii = Radii {
        tl: 0.0,
        tr: 0.0,
        br: 0.0,
        bl: 0.0,
    };

    /// The same radius at every corner.
    pub const fn uniform(r: f32) -> Self {
        Self {
            tl: r,
            tr: r,
            br: r,
            bl: r,
        }
    }

    /// Negative and NaN radii become 0, then all radii are scaled down together by the CSS rule so that no two
    /// adjacent radii exceed the side they share.
    pub fn fitted(self, w: f32, h: f32) -> Radii {
        let c = |v: f32| v.max(0.0);
        let r = Radii {
            tl: c(self.tl),
            tr: c(self.tr),
            br: c(self.br),
            bl: c(self.bl),
        };
        let mut f = 1.0f32;
        for (sum, side) in [
            (r.tl + r.tr, w),
            (r.bl + r.br, w),
            (r.tl + r.bl, h),
            (r.tr + r.br, h),
        ] {
            if sum > side && sum > 0.0 {
                f = f.min(side / sum);
            }
        }
        Radii {
            tl: r.tl * f,
            tr: r.tr * f,
            br: r.br * f,
            bl: r.bl * f,
        }
    }
}

/// A solid border: a width per side and one colour.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Border {
    /// Top width, device px.
    pub top: f32,
    /// Right width.
    pub right: f32,
    /// Bottom width.
    pub bottom: f32,
    /// Left width.
    pub left: f32,
    /// Colour of all four sides.
    pub color: Color,
}

impl Border {
    /// No border.
    pub const NONE: Border = Border {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
        color: Color::TRANSPARENT,
    };

    /// The same width on every side.
    pub const fn uniform(width: f32, color: Color) -> Self {
        Self {
            top: width,
            right: width,
            bottom: width,
            left: width,
            color,
        }
    }

    /// True if the border draws nothing.
    pub fn is_none(&self) -> bool {
        self.color.is_transparent()
            || !(self.top > 0.0 || self.right > 0.0 || self.bottom > 0.0 || self.left > 0.0)
    }
}

/// What fills a box.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum Fill {
    /// No fill (a border-only box).
    #[default]
    None,
    /// One colour.
    Solid(Color),
    /// A linear or conic gradient, plain or repeating, optionally tiled.
    Gradient(Gradient),
}

/// Most stops of a gradient.
pub const MAX_STOPS: usize = 4;

/// One colour stop: a straight colour at a position along the gradient (a fraction of the line from `from` to
/// `to` for a linear gradient, a fraction of a turn for a conic one). Positions may lie outside 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GradientStop {
    /// The colour.
    pub color: Color,
    /// The position.
    pub pos: f32,
}

impl GradientStop {
    /// A stop.
    pub const fn new(color: Color, pos: f32) -> Self {
        Self { color, pos }
    }
}

/// The geometry of a gradient, in the box's own coordinates: `(0, 0)` is the top left corner of the box's
/// `rect`, or of each tile when the gradient is tiled.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientShape {
    /// Colours change along the line from `from` (position 0) to `to` (position 1) and are constant across it.
    Linear {
        /// Where position 0 lies.
        from: Vec2,
        /// Where position 1 lies.
        to: Vec2,
    },
    /// Colours change with the angle around `center`, clockwise from `start` (CSS `conic-gradient(from ...)`).
    Conic {
        /// The centre.
        center: Vec2,
        /// Angle of position 0, in turns, clockwise from the top (0.25 points right).
        start: f32,
    },
}

/// A gradient of 2 to [`MAX_STOPS`] stops (CSS `linear-gradient`, `conic-gradient` and their `repeating-`
/// forms). Colours are interpolated premultiplied, like the browser does; equal positions make a hard stop.
// SIMPLIFIED(SIMP-245): a gradient has at most 4 colour stops (the instance has room for 4); the style profile refuses more
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gradient {
    /// The geometry.
    pub shape: GradientShape,
    /// The pattern between the first and the last stop repeats (CSS `repeating-*`).
    pub repeating: bool,
    /// Tile size, device px (CSS `background-size` with `background-repeat: repeat`): the gradient is drawn in
    /// every tile, tiles start at the top left corner of the box. `None`: one tile, the whole box.
    pub tile: Option<Vec2>,
    stops: [GradientStop; MAX_STOPS],
    len: u8,
}

impl Gradient {
    /// A gradient from 2 to 4 stops. Positions that go backwards are raised to the previous one (the CSS rule),
    /// so the stops are always in order.
    pub fn new(
        shape: GradientShape,
        repeating: bool,
        stops: &[GradientStop],
    ) -> Result<Self, DrawError> {
        if !(2..=MAX_STOPS).contains(&stops.len()) {
            return Err(DrawError::InvalidGradient("needs 2 to 4 stops"));
        }
        let finite = match shape {
            GradientShape::Linear { from, to } => from.is_finite() && to.is_finite(),
            GradientShape::Conic { center, start } => center.is_finite() && start.is_finite(),
        };
        if !finite || stops.iter().any(|s| !s.pos.is_finite()) {
            return Err(DrawError::InvalidGradient("non-finite geometry or stop"));
        }
        let mut out = [GradientStop::default(); MAX_STOPS];
        let mut last = f32::NEG_INFINITY;
        for (o, s) in out.iter_mut().zip(stops) {
            last = s.pos.max(last);
            *o = GradientStop::new(s.color, last);
        }
        Ok(Self {
            shape,
            repeating,
            tile: None,
            stops: out,
            len: stops.len() as u8,
        })
    }

    /// A linear gradient.
    pub fn linear(from: Vec2, to: Vec2, stops: &[GradientStop]) -> Result<Self, DrawError> {
        Self::new(GradientShape::Linear { from, to }, false, stops)
    }

    /// A conic gradient.
    pub fn conic(center: Vec2, start: f32, stops: &[GradientStop]) -> Result<Self, DrawError> {
        Self::new(GradientShape::Conic { center, start }, false, stops)
    }

    /// The same gradient, repeating.
    pub fn repeating(mut self) -> Self {
        self.repeating = true;
        self
    }

    /// The same gradient drawn in tiles of `size` (ignored unless both sides are positive and finite).
    pub fn tiled(mut self, size: Vec2) -> Self {
        self.tile = (size.is_finite() && size.x > 0.0 && size.y > 0.0).then_some(size);
        self
    }

    /// The stops, in order.
    pub fn stops(&self) -> &[GradientStop] {
        &self.stops[..usize::from(self.len)]
    }

    /// True if no stop has a visible colour.
    pub fn is_transparent(&self) -> bool {
        self.stops().iter().all(|s| s.color.is_transparent())
    }
}

/// The line of a CSS `linear-gradient(<angle>, ...)` in a box of `size`: `(from, to)` in box coordinates. The
/// angle is in degrees, clockwise from the top; the line runs through the centre and is long enough for the
/// corners to get positions 0 and 1.
pub fn css_linear_line(angle_deg: f32, size: Vec2) -> (Vec2, Vec2) {
    let a = angle_deg.to_radians();
    let dir = Vec2::new(a.sin(), -a.cos());
    let len = (size.x * a.sin()).abs() + (size.y * a.cos()).abs();
    let c = size * 0.5;
    (c - dir * len * 0.5, c + dir * len * 0.5)
}

/// The angle of a CSS `linear-gradient(to <corner>, ...)` in a box of `size`, degrees: the 50 % line joins the
/// two other corners.
pub fn css_corner_angle(right: bool, bottom: bool, size: Vec2) -> f32 {
    let sx = if right { 1.0 } else { -1.0 };
    let sy = if bottom { 1.0 } else { -1.0 };
    (sx * size.y).atan2(-sy * size.x).to_degrees()
}

/// A rectangle with a fill, a border and rounded corners.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BoxItem {
    /// The border box, in chunk space.
    pub rect: Rect,
    /// The fill.
    pub fill: Fill,
    /// The border, drawn inside `rect`.
    pub border: Border,
    /// Corner radii.
    pub radii: Radii,
    /// Transform from the item's space to chunk space (`None`: identity). It applies to the whole item, `rect`
    /// included.
    pub transform: Option<Affine2>,
    /// Clip in chunk space.
    pub clip: Option<Clip>,
}

impl BoxItem {
    /// A plain box with a solid fill.
    pub fn solid(rect: Rect, color: Color) -> Self {
        Self {
            rect,
            fill: Fill::Solid(color),
            ..Self::default()
        }
    }
}

/// A box whose corners are quarter ellipses (CSS `border-radius: rx / ry`, `50%` on a non-square box), all with
/// the same ratio of vertical to horizontal radius.
// SIMPLIFIED(SIMP-248): every elliptic corner of a box has the same ry / rx ratio; corners with different ellipses are refused by the style profile
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EllipticBox {
    /// The box; its `radii` are the horizontal radii.
    pub base: BoxItem,
    /// Vertical radius over horizontal radius, positive (1: circular corners, like a plain box).
    pub ratio: f32,
}

impl EllipticBox {
    /// The ellipse inscribed in `rect` (CSS `border-radius: 50%`).
    pub fn ellipse(rect: Rect, fill: Fill, border: Border) -> Self {
        Self {
            base: BoxItem {
                rect,
                fill,
                border,
                radii: Radii::uniform(rect.w * 0.5),
                ..BoxItem::default()
            },
            ratio: if rect.w > 0.0 { rect.h / rect.w } else { 1.0 },
        }
    }
}

/// An outer box shadow (CSS `box-shadow` without `inset`): the rounded rectangle `rect` moved by `offset`, grown
/// by `spread` and blurred with a Gaussian of `blur / 2` standard deviation. Like in CSS, nothing of it is drawn
/// inside `rect` itself (the element's border box), so a translucent element does not show its own shadow.
#[derive(Debug, Clone, PartialEq)]
pub struct ShadowItem {
    /// The element's border box, chunk space (before the offset and the spread).
    pub rect: Rect,
    /// The element's corner radii (the shadow's grow with the spread).
    pub radii: Radii,
    /// Shadow offset, device px.
    pub offset: Vec2,
    /// Blur radius, device px (CSS: twice the standard deviation).
    pub blur: f32,
    /// Spread, device px (negative shrinks).
    pub spread: f32,
    /// Shadow colour.
    pub color: Color,
    /// Transform from the item's space to chunk space.
    pub transform: Option<Affine2>,
    /// Clip in chunk space.
    pub clip: Option<Clip>,
}

impl Default for ShadowItem {
    fn default() -> Self {
        Self {
            rect: Rect::default(),
            radii: Radii::ZERO,
            offset: Vec2::ZERO,
            blur: 0.0,
            spread: 0.0,
            color: Color::TRANSPARENT,
            transform: None,
            clip: None,
        }
    }
}

/// Where a sprite lies: a layer and the normalized rectangle `(u0, v0, u1, v1)`.
///
/// For a [`MaskItem`] the layer counts the package's `A8` pages in order (`0` is the first mask page); for an
/// [`ImageItem`] it counts the package's `Rgba8` pages in order. `uv` is normalized to that page (`(0, 0, 1, 1)`
/// is the whole page); the scene maps it into the GPU array ([`pages`]). A mask layer past the package's mask
/// pages is a glyph page of the scene and its `uv` is used as it is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ImageRef {
    /// Page (array layer).
    pub layer: u8,
    /// Normalized rectangle of the sprite in the page: left, top, right, bottom.
    pub uv: [f32; 4],
}

/// A coverage mask (an icon of the package's mask pages) painted in one colour.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaskItem {
    /// Destination rectangle, in chunk space.
    pub rect: Rect,
    /// The sprite in the mask array.
    pub mask: ImageRef,
    /// Paint colour.
    pub color: Color,
    /// Transform from the item's space to chunk space.
    pub transform: Option<Affine2>,
    /// Clip in chunk space.
    pub clip: Option<Clip>,
}

/// A texture the renderer owns rather than the skin package: a game asset of the `ui:tx` store (portraits,
/// emblems, unit icons) or a render target (the minimap). The value is the renderer's texture id
/// (`draw_list::TextureId`, the id space of the original UI); the pass resolves it every frame and draws
/// nothing (counting `ui2.ext_missing`) while the texture is not resident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExtTexture(pub u64);

impl ExtTexture {
    /// The minimap render target (`draw_list::TextureId::MINIMAP`).
    pub const MINIMAP: ExtTexture = ExtTexture((1 << 63) | 1);
}

/// A 9-slice (CSS `border-image`): the sprite is cut by `src` into corners, edges and a middle, the corners
/// are drawn at the sizes `dst`, the edges and the middle stretched between them.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct NineSlice {
    /// Where the sprite is cut, as fractions of its height (top, bottom) and width (right, left): top, right,
    /// bottom, left (`border-image-slice`).
    pub src: [f32; 4],
    /// Widths of the destination's border area, device px: top, right, bottom, left (`border-image-width`).
    /// When opposite widths exceed the rectangle, all are scaled down together.
    pub dst: [f32; 4],
    /// The middle is drawn too (`border-image-slice: ... fill`).
    pub fill: bool,
}

/// A colour picture: a sprite of the package's `Rgba8` pages (premultiplied), or an external texture, tinted
/// and with rounded corners, whole or as a 9-slice.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageItem {
    /// Destination rectangle, in chunk space.
    pub rect: Rect,
    /// The sprite in the image pages. With `external`, only its `uv` counts: the rectangle in that texture,
    /// normalized to it (`(0, 0, 1, 1)` is the whole texture).
    pub image: ImageRef,
    /// Multiplies the picture (straight colour; white opaque leaves it as it is, the alpha is an opacity).
    pub tint: Color,
    /// Corner radii (the picture is cut to the rounded rectangle). Not used by a 9-slice.
    pub radii: Radii,
    /// Transform from the item's space to chunk space.
    pub transform: Option<Affine2>,
    /// Clip in chunk space.
    pub clip: Option<Clip>,
    /// Draw the picture as a 9-slice.
    pub nine: Option<NineSlice>,
    /// Sample this renderer texture instead of the package pages. Items of different external textures are
    /// drawn by different draw calls (the only thing that splits a layer's draw).
    pub external: Option<ExtTexture>,
}

impl Default for ImageItem {
    fn default() -> Self {
        Self {
            rect: Rect::default(),
            image: ImageRef::default(),
            tint: Color::rgba(255, 255, 255, 255),
            radii: Radii::ZERO,
            transform: None,
            clip: None,
            nine: None,
            external: None,
        }
    }
}

/// One glyph of a [`Glyphs`] item: the glyph id and its pen position in chunk space, device px, as shaped (no
/// rounding): `x` is the pen position, `y` the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphPos {
    /// Glyph id in the face.
    pub glyph: u32,
    /// Pen x.
    pub x: f32,
    /// Baseline y.
    pub y: f32,
}

impl From<(u32, f32, f32)> for GlyphPos {
    fn from((glyph, x, y): (u32, f32, f32)) -> Self {
        Self { glyph, x, y }
    }
}

/// A run of glyphs of one face, size, weight and colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyphs {
    /// The face.
    pub face: FaceId,
    /// Font size in device px (CSS size times the UI scale factor), not quantized.
    pub size_px: f32,
    /// CSS weight (sets the `wght` axis of variable faces).
    pub wght: f32,
    /// Text colour.
    pub color: Color,
    /// The glyphs.
    pub glyphs: Vec<GlyphPos>,
    /// Transform from the item's space to chunk space.
    pub transform: Option<Affine2>,
    /// Clip in chunk space.
    pub clip: Option<Clip>,
}

impl Glyphs {
    /// The glyphs of a shaped run (CSS px) drawn with its origin at `(x, baseline_y)` device px and scaled by `k`
    /// (the UI scale factor): `size_px = size * k`, positions as shaped, times `k`.
    pub fn from_run(
        run: &ui2_text::ShapedRun,
        k: f32,
        x: f32,
        baseline_y: f32,
        color: Color,
    ) -> Self {
        Self {
            face: run.style.face,
            size_px: run.style.size * k,
            wght: run.style.wght,
            color,
            glyphs: run
                .glyphs
                .iter()
                .map(|g| GlyphPos {
                    glyph: g.glyph_id,
                    x: x + g.x * k,
                    y: baseline_y + g.y * k,
                })
                .collect(),
            transform: None,
            clip: None,
        }
    }
}

/// One drawable thing.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Item {
    /// A box.
    Box(BoxItem),
    /// A box with elliptic corners.
    EllipticBox(EllipticBox),
    /// An outer blurred shadow.
    Shadow(ShadowItem),
    /// A mask sprite.
    Mask(MaskItem),
    /// A colour picture.
    Image(ImageItem),
    /// A run of glyphs.
    Glyphs(Glyphs),
}

/// The items of one chunk, in drawing order (later over earlier), and the anchor of the chunk.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChunkItems {
    /// The items.
    pub items: Vec<Item>,
    /// The anchor that places the chunk on the screen (`None`: chunk space is screen space).
    pub anchor: Option<AnchorSlot>,
}

/// Builds [`ChunkItems`].
#[derive(Debug, Clone, Default)]
pub struct ChunkBuilder {
    chunk: ChunkItems,
}

impl ChunkBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Places the chunk with an anchor.
    pub fn anchor(mut self, slot: AnchorSlot) -> Self {
        self.chunk.anchor = Some(slot);
        self
    }

    /// Appends an item (drawn over the earlier ones).
    pub fn push(&mut self, item: Item) -> &mut Self {
        self.chunk.items.push(item);
        self
    }

    /// Number of items so far.
    pub fn len(&self) -> usize {
        self.chunk.items.len()
    }

    /// True if no item was added.
    pub fn is_empty(&self) -> bool {
        self.chunk.items.is_empty()
    }

    /// The finished chunk.
    pub fn build(self) -> ChunkItems {
        self.chunk
    }
}
