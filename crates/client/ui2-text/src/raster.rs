//! Glyph rasterization: `skrifa` outlines, hinted vertically at small sizes, filled by `ab_glyph_rasterizer`, 8-bit
//! linear coverage.
//!
//! A glyph is rasterized at its device pixel size (CSS size times the scale factor `k` = DPR times UI scale: never a
//! bitmap of another size scaled) and at one of 4 horizontal subpixel offsets (0, 1/4, 1/2, 3/4 px). The baseline
//! sits on a whole device pixel (the caller snaps it), so there is no vertical subpixel. Colour is not part of the
//! key: one coverage bitmap serves every colour and hover state.
//!
//! Up to [`HINT_MAX_PX`] the outline is grid-fitted in y only by the automatic hinter in its light mode (as
//! DirectWrite and FreeType's light hinting do): baseline, x-height, cap height and horizontal stems land on whole
//! device pixels, so their edges are one sharp row instead of two grey ones; nothing moves in x, the advances and the
//! subpixel positions stay those of layout. Above it the outline is drawn as designed.
//!
//! The coverage is linear (the fraction of the pixel the outline covers). The renderer turns it into the alpha it
//! blends with by a contrast exponent that depends on the text colour and the size (see
//! `ui2_draw::glyph_contrast`): light text on a dark panel and dark text on a light one then keep the weight of
//! their design, and small sizes get a little more ink (stem darkening).

use std::collections::HashMap;

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{
    DrawSettings, Engine, GlyphStyles, HintingInstance, HintingOptions, OutlineGlyphCollection,
    OutlinePen, SmoothMode, Target,
};
use skrifa::{GlyphId, MetadataProvider};

use crate::{FaceId, TextEngine};

// SIMPLIFIED(SIMP-246): glyph coverage is grayscale (no LCD subpixel anti-aliasing), small sizes are grid-fitted by the automatic hinter's light vertical mode rather than the font's own instructions, and stem darkening is a contrast exponent on the coverage rather than an emboldened outline
/// Number of horizontal subpixel positions of a glyph.
pub const SUBPIXEL_STEPS: u8 = 4;
/// Largest device size (px) whose outlines are hinted.
pub const HINT_MAX_PX: f32 = 48.0;
/// Hinting instances kept (one per face, device size and weight in use); the cache starts again when it is full.
const MAX_HINTERS: usize = 96;
/// Largest standard deviation (device px) of a blurred glyph; a larger blur is drawn with this one.
pub const MAX_BLUR_SIGMA: f32 = 16.0;
/// Largest bitmap side, device px; a glyph that would be bigger is rasterized empty.
const MAX_SIDE: i64 = 2048;

/// Identity of a rasterized glyph: face, glyph, device size, weight and subpixel step; for the blurred coverage
/// of a text shadow also the blur and the contrast exponent ([`GlyphKey::blurred`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    /// Face the glyph belongs to.
    pub face: FaceId,
    /// Glyph id.
    pub glyph: u32,
    /// Device size in px as `f32` bits (CSS size times the scale factor `k`).
    pub size_bits: u32,
    /// CSS weight, sets the `wght` axis of variable faces.
    pub wght: u16,
    /// Subpixel step 0..=3: the glyph origin sits `subpx / 4` px right of a whole pixel.
    pub subpx: u8,
    /// Standard deviation of the shadow blur in 1/8 device px; 0: the sharp glyph.
    pub blur: u16,
    /// Contrast exponent applied before the blur, in 1/255 (0: none); only with `blur`.
    pub ink: u8,
}

impl GlyphKey {
    /// A key for a glyph at `size_px` device pixels.
    pub fn new(face: FaceId, glyph: u32, size_px: f32, wght: f32, subpx: u8) -> Self {
        Self {
            face,
            glyph,
            size_bits: size_px.to_bits(),
            wght: wght.round().clamp(1.0, 1000.0) as u16,
            subpx: subpx % SUBPIXEL_STEPS,
            blur: 0,
            ink: 0,
        }
    }

    /// The key of this glyph's shadow: its coverage raised to `contrast` (the alpha the sharp glyph shows, see
    /// `ui2_draw::glyph_contrast`; 0 or not finite: none) and blurred by a Gaussian of standard deviation `sigma`
    /// device px (cut at `ceil(2 * sigma)` texels, at most [`MAX_BLUR_SIGMA`]). The bitmap is the blurred alpha,
    /// grown by the blur's reach, so a shadow is drawn from the atlas like a sharp glyph: one texture read per
    /// pixel. `sigma` 0 or less gives the sharp key.
    pub fn blurred(self, sigma: f32, contrast: f32) -> Self {
        if !(sigma.is_finite() && sigma > 0.0) {
            return Self {
                blur: 0,
                ink: 0,
                ..self
            };
        }
        let blur = (sigma.min(MAX_BLUR_SIGMA) * 8.0).round().max(1.0) as u16;
        let ink = if contrast.is_finite() && contrast > 0.0 {
            (contrast.min(1.0) * 255.0).round().max(1.0) as u8
        } else {
            0
        };
        Self { blur, ink, ..self }
    }

    /// The shadow blur's standard deviation in device px (0: sharp).
    pub fn sigma(&self) -> f32 {
        f32::from(self.blur) / 8.0
    }

    /// The key for a glyph drawn at pen position `pen_x` (device px), and the whole pixel its bitmap origin
    /// is placed at. `css_size` is the layout font size; `k` is DPR times UI scale.
    pub fn at(
        face: FaceId,
        glyph: u32,
        css_size: f32,
        k: f32,
        wght: f32,
        pen_x: f32,
    ) -> (Self, i32) {
        let (px, sub) = quantize_x(pen_x);
        (Self::new(face, glyph, css_size * k, wght, sub), px)
    }

    /// The device size in px.
    pub fn size_px(&self) -> f32 {
        f32::from_bits(self.size_bits)
    }
}

/// Splits an x position in device px into a whole pixel and a subpixel step (the position rounded to the nearest
/// quarter pixel).
pub fn quantize_x(x: f32) -> (i32, u8) {
    let q = (x * f32::from(SUBPIXEL_STEPS)).round() as i64;
    let steps = i64::from(SUBPIXEL_STEPS);
    (q.div_euclid(steps) as i32, q.rem_euclid(steps) as u8)
}

/// Coverage of one glyph. Pixel `(0, 0)` is the top-left; place it at `(pen_px + left, baseline_px - top)` in
/// y-down device pixels, where `pen_px` is the whole pixel returned by [`GlyphKey::at`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlyphBitmap {
    /// Width in px (0 for glyphs without an outline, such as the space).
    pub w: u32,
    /// Height in px.
    pub h: u32,
    /// Offset of the left edge from the pen position, px (may be negative).
    pub left: i32,
    /// Distance from the baseline up to the top edge, px (may be negative).
    pub top: i32,
    /// `w * h` coverage bytes, row-major, linear (255: the pixel is wholly inside the outline).
    pub coverage: Vec<u8>,
}

/// The hinting instances of the engine and the glyph styles of each face (the automatic hinter's analysis of the
/// font, made once per face).
#[derive(Default)]
pub(crate) struct Hinters {
    styles: HashMap<FaceId, GlyphStyles>,
    /// By face, device size bits and weight; `None`: this face cannot be hinted at that size.
    instances: HashMap<(FaceId, u32, u16), Option<HintingInstance>>,
}

impl Hinters {
    /// Drops what was made for a face.
    pub(crate) fn forget(&mut self, face: FaceId) {
        self.styles.remove(&face);
        self.instances.retain(|k, _| k.0 != face);
    }
}

/// The light vertical hinting of [`TextEngine::raster_glyph`].
fn hint_options(styles: GlyphStyles) -> HintingOptions {
    HintingOptions {
        engine: Engine::Auto(Some(styles)),
        target: Target::Smooth {
            mode: SmoothMode::Light,
            symmetric_rendering: true,
            preserve_linear_metrics: true,
        },
    }
}

#[derive(Clone, Copy)]
enum Cmd {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Records an outline (y up) and its bounding box.
#[derive(Default)]
struct Recorder {
    cmds: Vec<Cmd>,
    min: Option<(f32, f32)>,
    max: Option<(f32, f32)>,
}

impl Recorder {
    fn grow(&mut self, x: f32, y: f32) {
        let (mx, my) = self.min.get_or_insert((x, y));
        *mx = mx.min(x);
        *my = my.min(y);
        let (nx, ny) = self.max.get_or_insert((x, y));
        *nx = nx.max(x);
        *ny = ny.max(y);
    }
}

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.grow(x, y);
        self.cmds.push(Cmd::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.grow(x, y);
        self.cmds.push(Cmd::Line(x, y));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.grow(cx, cy);
        self.grow(x, y);
        self.cmds.push(Cmd::Quad(cx, cy, x, y));
    }
    fn curve_to(&mut self, c1x: f32, c1y: f32, c2x: f32, c2y: f32, x: f32, y: f32) {
        self.grow(c1x, c1y);
        self.grow(c2x, c2y);
        self.grow(x, y);
        self.cmds.push(Cmd::Cubic(c1x, c1y, c2x, c2y, x, y));
    }
    fn close(&mut self) {
        self.cmds.push(Cmd::Close);
    }
}

impl TextEngine {
    /// Turns the vertical hinting of small sizes on or off (on by default; off gives the outlines as designed at
    /// every size, for comparisons).
    pub fn set_hinting(&mut self, on: bool) {
        self.hinting = on;
    }

    /// Whether small sizes are hinted.
    pub fn hinting(&self) -> bool {
        self.hinting
    }

    /// Draws the outline of `glyph` with the hinting instance of its key into `rec`; false when there is none (a
    /// size above [`HINT_MAX_PX`], hinting off, or a face the hinter cannot take).
    fn draw_hinted(
        &self,
        key: GlyphKey,
        outlines: &OutlineGlyphCollection,
        glyph: &skrifa::outline::OutlineGlyph,
        location: LocationRef,
        rec: &mut Recorder,
    ) -> bool {
        let size = key.size_px();
        if !(self.hinting && size > 0.0 && size <= HINT_MAX_PX) {
            return false;
        }
        let Ok(mut h) = self.hinters.lock() else {
            return false;
        };
        let Hinters { styles, instances } = &mut *h;
        if instances.len() >= MAX_HINTERS {
            instances.clear();
        }
        let hinter = instances
            .entry((key.face, key.size_bits, key.wght))
            .or_insert_with(|| {
                let styles = styles
                    .entry(key.face)
                    .or_insert_with(|| GlyphStyles::new(outlines))
                    .clone();
                HintingInstance::new(outlines, Size::new(size), location, hint_options(styles)).ok()
            });
        let Some(hinter) = hinter.as_ref() else {
            return false;
        };
        glyph.draw(DrawSettings::hinted(hinter, false), rec).is_ok()
    }

    /// Rasterizes a glyph (hinted vertically up to [`HINT_MAX_PX`], linear coverage). A glyph without an outline,
    /// an unknown face or a glyph id outside the face gives an empty bitmap.
    pub fn raster_glyph(&self, key: GlyphKey) -> GlyphBitmap {
        if key.blur > 0 {
            let sharp = self.raster_glyph(GlyphKey {
                blur: 0,
                ink: 0,
                ..key
            });
            return blur_bitmap(&sharp, key.sigma(), f32::from(key.ink) / 255.0);
        }
        let Ok(face) = self.face(key.face) else {
            return GlyphBitmap::default();
        };
        let Some(font) = face.font() else {
            return GlyphBitmap::default();
        };
        let outlines = font.outline_glyphs();
        let Some(glyph) = outlines.get(GlyphId::new(key.glyph)) else {
            return GlyphBitmap::default();
        };
        let size = key.size_px();
        let loc = face
            .has_wght
            .then(|| font.axes().location([("wght", f32::from(key.wght))]));
        let location = loc
            .as_ref()
            .map_or(LocationRef::default(), LocationRef::from);
        let mut rec = Recorder::default();
        if !self.draw_hinted(key, &outlines, &glyph, location, &mut rec) {
            rec = Recorder::default();
            if glyph
                .draw(DrawSettings::unhinted(Size::new(size), location), &mut rec)
                .is_err()
            {
                return GlyphBitmap::default();
            }
        }
        let (Some((min_x, min_y)), Some((max_x, max_y))) = (rec.min, rec.max) else {
            return GlyphBitmap::default();
        };

        let off = f32::from(key.subpx) / f32::from(SUBPIXEL_STEPS);
        let left = (min_x + off).floor() as i64;
        let right = (max_x + off).ceil() as i64;
        let top = max_y.ceil() as i64;
        let bottom = min_y.floor() as i64;
        let (w, h) = (right - left, top - bottom);
        if w <= 0 || h <= 0 || w > MAX_SIDE || h > MAX_SIDE {
            return GlyphBitmap::default();
        }

        // Outline space (y up, origin at the pen on the baseline) to bitmap space (y down).
        let map = |x: f32, y: f32| -> Point { point(x + off - left as f32, top as f32 - y) };
        let mut r = Rasterizer::new(w as usize, h as usize);
        let (mut cur, mut start) = (point(0.0, 0.0), point(0.0, 0.0));
        for cmd in &rec.cmds {
            match *cmd {
                Cmd::Move(x, y) => {
                    // An open contour is closed implicitly by the fill.
                    if cur != start {
                        r.draw_line(cur, start);
                    }
                    cur = map(x, y);
                    start = cur;
                }
                Cmd::Line(x, y) => {
                    let p = map(x, y);
                    r.draw_line(cur, p);
                    cur = p;
                }
                Cmd::Quad(cx, cy, x, y) => {
                    let p = map(x, y);
                    r.draw_quad(cur, map(cx, cy), p);
                    cur = p;
                }
                Cmd::Cubic(ax, ay, bx, by, x, y) => {
                    let p = map(x, y);
                    r.draw_cubic(cur, map(ax, ay), map(bx, by), p);
                    cur = p;
                }
                Cmd::Close => {
                    if cur != start {
                        r.draw_line(cur, start);
                    }
                    cur = start;
                }
            }
        }
        if cur != start {
            r.draw_line(cur, start);
        }
        let mut coverage = vec![0u8; (w * h) as usize];
        r.for_each_pixel(|i, c| {
            coverage[i] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        });
        GlyphBitmap {
            w: w as u32,
            h: h as u32,
            left: left as i32,
            top: top as i32,
            coverage,
        }
    }
}

/// `bitmap`'s coverage raised to `contrast` (0: as it is) and blurred by the separable Gaussian of standard
/// deviation `sigma` px sampled at whole pixels, cut at `ceil(2 * sigma)` pixels and renormalized; the bitmap grows
/// by that reach on every side (its offsets move with it).
pub fn blur_bitmap(bitmap: &GlyphBitmap, sigma: f32, contrast: f32) -> GlyphBitmap {
    if bitmap.w == 0 || bitmap.h == 0 || sigma.is_nan() || sigma <= 0.0 {
        return bitmap.clone();
    }
    let r = (2.0 * sigma).ceil() as usize;
    let k = -0.5 / (sigma * sigma);
    let weights: Vec<f32> = (0..=2 * r)
        .map(|i| {
            let d = i as f32 - r as f32;
            (k * d * d).exp()
        })
        .collect();
    let total: f32 = weights.iter().sum();
    let weights: Vec<f32> = weights.iter().map(|w| w / total).collect();
    let (w, h) = (bitmap.w as usize, bitmap.h as usize);
    let (ow, oh) = (w + 2 * r, h + 2 * r);
    let alpha: Vec<f32> = bitmap
        .coverage
        .iter()
        .map(|&c| {
            let c = f32::from(c) / 255.0;
            if contrast > 0.0 && c > 0.0 {
                c.powf(contrast)
            } else {
                c
            }
        })
        .collect();
    // Along x into the grown width (rows of the source), then along y into the grown height.
    let mut rows = vec![0.0f32; ow * h];
    for y in 0..h {
        let src = &alpha[y * w..(y + 1) * w];
        let dst = &mut rows[y * ow..(y + 1) * ow];
        for (x, &a) in src.iter().enumerate() {
            if a == 0.0 {
                continue;
            }
            for (i, wt) in weights.iter().enumerate() {
                dst[x + i] += a * wt;
            }
        }
    }
    let mut out = vec![0.0f32; ow * oh];
    for y in 0..h {
        for (j, wt) in weights.iter().enumerate() {
            let dst = &mut out[(y + j) * ow..(y + j + 1) * ow];
            for (d, &s) in dst.iter_mut().zip(&rows[y * ow..(y + 1) * ow]) {
                *d += s * wt;
            }
        }
    }
    GlyphBitmap {
        w: ow as u32,
        h: oh as u32,
        left: bitmap.left - r as i32,
        top: bitmap.top + r as i32,
        coverage: out
            .iter()
            .map(|&a| (a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)
            .collect(),
    }
}
