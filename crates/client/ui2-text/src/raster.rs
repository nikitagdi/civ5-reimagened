//! Glyph rasterization: unhinted `skrifa` outlines filled by `ab_glyph_rasterizer`, gamma 2.2, 8-bit coverage.
//!
//! A glyph is rasterized at a device pixel size (CSS size times the scale factor `k` = DPR times UI scale) and at
//! one of 4 horizontal subpixel offsets (0, 1/4, 1/2, 3/4 px). The baseline is expected to sit on a whole device
//! pixel (the caller snaps it), so there is no vertical subpixel. Colour is not part of the key: one coverage
//! bitmap serves every colour and hover state.
//!
//! The coverage is gamma-corrected as `c' = c^(1/2.2)`, the best fit to Chrome's text on Windows. Chrome hints
//! small text vertically (DirectWrite), we do not, so pixels differ from Chrome's at 12-16 px; the geometry
//! (advances, baselines) is what matches.

use ab_glyph_rasterizer::{Point, Rasterizer, point};
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{GlyphId, MetadataProvider};

use crate::{FaceId, TextEngine};

/// Number of horizontal subpixel positions of a glyph.
pub const SUBPIXEL_STEPS: u8 = 4;
/// The gamma applied to coverage.
pub const GAMMA: f32 = 2.2;
/// Largest bitmap side, device px; a glyph that would be bigger is rasterized empty.
const MAX_SIDE: i64 = 2048;

/// Identity of a rasterized glyph: face, glyph, device size, weight and subpixel step.
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
        }
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
    /// `w * h` coverage bytes, row-major, gamma applied.
    pub coverage: Vec<u8>,
}

/// The gamma table: input is 8-bit linear coverage.
pub(crate) fn gamma_lut() -> [u8; 256] {
    let mut lut = [0u8; 256];
    let inv = 1.0 / f64::from(GAMMA);
    for (i, v) in lut.iter_mut().enumerate() {
        *v = ((i as f64 / 255.0).powf(inv) * 255.0 + 0.5).floor() as u8;
    }
    lut
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
    /// Rasterizes a glyph (unhinted outline, gamma 2.2). A glyph without an outline, an unknown face or a glyph
    /// id outside the face gives an empty bitmap.
    pub fn raster_glyph(&self, key: GlyphKey) -> GlyphBitmap {
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
        if glyph
            .draw(DrawSettings::unhinted(Size::new(size), location), &mut rec)
            .is_err()
        {
            return GlyphBitmap::default();
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
            let linear = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as usize;
            coverage[i] = self.gamma[linear.min(255)];
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
