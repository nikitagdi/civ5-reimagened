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

pub use glam::{Affine2, Vec2};

pub use glyphs::GlyphStore;
pub use instance::Ui2Instance;
pub use pages::{Pages, StaticPage};
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

/// What fills a box. More kinds (gradients) are added without changing the instance format.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[non_exhaustive]
pub enum Fill {
    /// No fill (a border-only box).
    #[default]
    None,
    /// One colour.
    Solid(Color),
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

/// A colour picture of the package's `Rgba8` pages (premultiplied), tinted and with rounded corners.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageItem {
    /// Destination rectangle, in chunk space.
    pub rect: Rect,
    /// The sprite in the image pages.
    pub image: ImageRef,
    /// Multiplies the picture (straight colour; white opaque leaves it as it is, the alpha is an opacity).
    pub tint: Color,
    /// Corner radii (the picture is cut to the rounded rectangle).
    pub radii: Radii,
    /// Transform from the item's space to chunk space.
    pub transform: Option<Affine2>,
    /// Clip in chunk space.
    pub clip: Option<Clip>,
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

/// One drawable thing. Later primitives (shadows, images, 9-slices) are added to this enum.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Item {
    /// A box.
    Box(BoxItem),
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
