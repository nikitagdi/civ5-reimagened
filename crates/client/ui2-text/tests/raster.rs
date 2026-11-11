//! Glyph rasterization: coverage, gamma, subpixel steps, the weight axis, determinism.

mod common;

use common::*;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use ui2_text::{FaceId, GlyphBitmap, GlyphKey, RunStyle, TextEngine, quantize_x};

/// Glyph id of a single character (no shaping features involved).
fn glyph_of(e: &mut TextEngine, face: FaceId, wght: f32, c: char) -> u32 {
    let run = e
        .shape(&RunStyle::new(face, 16.0).with_wght(wght), &c.to_string())
        .unwrap();
    assert_eq!(run.glyphs.len(), 1);
    run.glyphs[0].glyph_id
}

fn key(face: FaceId, glyph: u32, size: f32, wght: f32, sub: u8) -> GlyphKey {
    GlyphKey::new(face, glyph, size, wght, sub)
}

/// Sum of the coverage with the gamma undone: the glyph's area in px^2.
fn area(b: &GlyphBitmap) -> f64 {
    b.coverage
        .iter()
        .map(|&c| (f64::from(c) / 255.0).powf(2.2))
        .sum()
}

fn centroid_x(b: &GlyphBitmap) -> f64 {
    let (mut s, mut m) = (0.0, 0.0);
    for (i, &c) in b.coverage.iter().enumerate() {
        let lin = (f64::from(c) / 255.0).powf(2.2);
        s += lin;
        m += lin * ((i as u32 % b.w) as f64 + 0.5);
    }
    f64::from(b.left) + m / s
}

/// Area by the shoelace formula of an outline made of straight lines only.
#[derive(Default)]
struct Shoelace {
    start: (f32, f32),
    cur: (f32, f32),
    twice_area: f64,
    curves: bool,
}

impl Shoelace {
    fn edge(&mut self, to: (f32, f32)) {
        self.twice_area +=
            f64::from(self.cur.0) * f64::from(to.1) - f64::from(to.0) * f64::from(self.cur.1);
        self.cur = to;
    }
}

impl OutlinePen for Shoelace {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = (x, y);
        self.cur = (x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.edge((x, y));
    }
    fn quad_to(&mut self, _: f32, _: f32, x: f32, y: f32) {
        self.curves = true;
        self.edge((x, y));
    }
    fn curve_to(&mut self, _: f32, _: f32, _: f32, _: f32, x: f32, y: f32) {
        self.curves = true;
        self.edge((x, y));
    }
    fn close(&mut self) {
        self.edge(self.start);
    }
}

#[test]
fn quantize_x_rounds_to_quarter_pixels() {
    assert_eq!(quantize_x(0.0), (0, 0));
    assert_eq!(quantize_x(3.0), (3, 0));
    assert_eq!(quantize_x(0.24), (0, 1));
    assert_eq!(quantize_x(0.5), (0, 2));
    assert_eq!(quantize_x(0.9), (1, 0));
    assert_eq!(quantize_x(-0.3), (-1, 3));
    assert_eq!(quantize_x(10.74), (10, 3));
}

#[test]
fn key_carries_the_device_size() {
    let (k, px) = GlyphKey::at(FIRA_400, 5, 14.0, 1.5, 400.0, 10.3);
    assert_eq!(k.size_px(), 21.0);
    assert_eq!((px, k.subpx), (10, 1));
    assert_ne!(k, GlyphKey::at(FIRA_400, 5, 14.0, 2.0, 400.0, 10.3).0);
    assert_ne!(k, GlyphKey::new(FIRA_400, 5, 21.0, 500.0, 1));
}

#[test]
fn space_and_unknown_things_rasterize_empty() {
    let mut e = engine();
    let sp = glyph_of(&mut e, FIRA_400, 400.0, ' ');
    assert_eq!(
        e.raster_glyph(key(FIRA_400, sp, 14.0, 400.0, 0)),
        GlyphBitmap::default()
    );
    assert_eq!(e.raster_glyph(key(FaceId(42), 5, 14.0, 400.0, 0)).w, 0);
    assert_eq!(e.raster_glyph(key(FIRA_400, 60_000, 14.0, 400.0, 0)).w, 0);
}

#[test]
fn coverage_sum_is_the_outline_area_and_gamma_brightens_it() {
    let mut e = engine();
    let gid = glyph_of(&mut e, FIRA_400, 400.0, 'I');
    let size = 120.0;
    let bmp = e.raster_glyph(key(FIRA_400, gid, size, 400.0, 0));
    assert!(bmp.w > 0 && bmp.h > 0);

    let bytes = read_font("fira-sans/FiraSans-Regular.ttf");
    let font = FontRef::new(&bytes).unwrap();
    let mut pen = Shoelace::default();
    font.outline_glyphs()
        .get(GlyphId::new(gid))
        .unwrap()
        .draw(
            DrawSettings::unhinted(Size::new(size), LocationRef::default()),
            &mut pen,
        )
        .unwrap();
    assert!(!pen.curves, "the capital I is straight lines only");
    let expected = pen.twice_area.abs() / 2.0;
    let got = area(&bmp);
    assert!(
        (got - expected).abs() / expected < 0.01,
        "area {got} vs outline {expected}"
    );

    // With the gamma applied the raw byte sum is larger than the linear area.
    let raw: f64 = bmp.coverage.iter().map(|&c| f64::from(c) / 255.0).sum();
    assert!(raw > expected * 1.01, "{raw} vs {expected}");
    // The bitmap sits on the baseline: the top of a capital is above it, the bottom at or just below.
    assert!(bmp.top > 0 && bmp.top as f32 <= size);
    assert!(bmp.top - bmp.h as i32 <= 0);
}

#[test]
fn rasterization_is_deterministic() {
    let mut e = engine();
    let g = glyph_of(&mut e, ALEGREYA, 500.0, 'g');
    let k = key(ALEGREYA, g, 33.0, 500.0, 3);
    let a = e.raster_glyph(k);
    let b = e.raster_glyph(k);
    assert_eq!(a, b);
    // A second engine (fresh caches) gives the same bytes.
    let c = engine().raster_glyph(k);
    assert_eq!(a, c);
    assert!(a.coverage.iter().any(|&v| v > 200));
}

#[test]
fn subpixel_steps_shift_the_glyph_by_a_quarter_pixel() {
    let mut e = engine();
    let gid = glyph_of(&mut e, FIRA_400, 400.0, 'o');
    let c: Vec<f64> = (0..4)
        .map(|s| centroid_x(&e.raster_glyph(key(FIRA_400, gid, 48.0, 400.0, s))))
        .collect();
    for s in 1..4 {
        let d = c[s] - c[s - 1];
        assert!((d - 0.25).abs() < 0.03, "step {s}: shift {d}");
    }
}

#[test]
fn variable_weight_axis_thickens_the_outline() {
    let mut e = engine();
    let g = glyph_of(&mut e, ALEGREYA, 400.0, 'o');
    let a400 = area(&e.raster_glyph(key(ALEGREYA, g, 60.0, 400.0, 0)));
    let a500 = area(&e.raster_glyph(key(ALEGREYA, g, 60.0, 500.0, 0)));
    let a900 = area(&e.raster_glyph(key(ALEGREYA, g, 60.0, 900.0, 0)));
    assert!(
        a500 > a400 * 1.03 && a900 > a500 * 1.1,
        "{a400} {a500} {a900}"
    );
}

#[test]
fn scale_factor_scales_the_bitmap() {
    let mut e = engine();
    let g = glyph_of(&mut e, FIRA_500, 500.0, 'H');
    let (k1, _) = GlyphKey::at(FIRA_500, g, 16.0, 1.0, 500.0, 0.0);
    let (k2, _) = GlyphKey::at(FIRA_500, g, 16.0, 2.0, 500.0, 0.0);
    let (b1, b2) = (e.raster_glyph(k1), e.raster_glyph(k2));
    let ratio = area(&b2) / area(&b1);
    assert!((ratio - 4.0).abs() < 0.2, "area ratio {ratio}");
    assert!((b2.h as f32 / b1.h as f32 - 2.0).abs() < 0.3);
}

#[test]
fn a_run_of_glyphs_has_ink() {
    let mut e = engine();
    let run = e
        .shape(&RunStyle::new(FIRA_400, 14.0), "Education")
        .unwrap();
    for g in run.glyphs.iter() {
        let b = e.raster_glyph(key(FIRA_400, g.glyph_id, 14.0, 400.0, 0));
        assert!(b.w > 0 && b.coverage.iter().any(|&c| c > 0));
    }
}
