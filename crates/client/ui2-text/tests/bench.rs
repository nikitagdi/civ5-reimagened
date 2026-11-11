//! Timing of the first-frame work: shape the fixture's runs and rasterize their unique glyphs.
//! Run with `cargo test -p ui2-text --release --test bench -- --ignored --nocapture`.

mod common;

use std::collections::BTreeSet;
use std::time::Instant;

use common::*;
use ui2_text::{FaceId, GlyphKey, NumericVariant, RunStyle};

#[test]
#[ignore = "timing; run in release with --nocapture"]
fn shape_and_rasterize_the_fixture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/chrome-text.json");
    let fx: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let all = fx["runs"].as_array().unwrap();
    for page in ["hud", "all"] {
        let runs = collect(all.iter().filter(|r| page == "all" || r["page"] == page));
        time_set(page, &runs);
    }
}

fn collect<'a>(rows: impl Iterator<Item = &'a serde_json::Value>) -> Vec<(RunStyle, String)> {
    rows.map(|r| {
        let weight = r["weight"].as_u64().unwrap() as u32;
        let face = match (r["family"].as_str().unwrap(), weight) {
            ("Fira Sans", 500) => FIRA_500,
            ("Fira Sans", _) => FIRA_400,
            _ => ALEGREYA,
        };
        let style = RunStyle::new(face, r["size"].as_f64().unwrap() as f32)
            .with_wght(weight as f32)
            .with_numeric(NumericVariant::from_css(r["numeric"].as_str().unwrap()))
            .with_letter_spacing(r["letter_spacing"].as_f64().unwrap() as f32);
        (style, r["s"].as_str().unwrap().to_string())
    })
    .collect()
}

fn time_set(label: &str, runs: &[(RunStyle, String)]) {
    let chars: usize = runs.iter().map(|r| r.1.chars().count()).sum();

    for pass in 0..3 {
        let t0 = Instant::now();
        let mut e = engine(); // includes parsing the three fonts and building the shaper data
        let t_setup = t0.elapsed();

        let t1 = Instant::now();
        let mut keys: BTreeSet<(FaceId, u32, u32, u16)> = BTreeSet::new();
        for (style, text) in runs {
            let run = e.shape(style, text).unwrap();
            for g in run.glyphs.iter() {
                let k = GlyphKey::new(style.face, g.glyph_id, style.size, style.wght, 0);
                keys.insert((k.face, k.glyph, k.size_bits, k.wght));
            }
        }
        let t_shape = t1.elapsed();

        let t2 = Instant::now();
        let mut ink = 0u64;
        for (face, glyph, size_bits, wght) in &keys {
            let k = GlyphKey::new(
                *face,
                *glyph,
                f32::from_bits(*size_bits),
                f32::from(*wght),
                0,
            );
            ink += e
                .raster_glyph(k)
                .coverage
                .iter()
                .map(|&c| u64::from(c))
                .sum::<u64>();
        }
        let t_raster = t2.elapsed();
        println!(
            "{label} pass {pass}: {} runs, {chars} chars, {} unique glyphs; setup {:.2} ms, shape {:.2} ms, raster {:.2} ms (ink {ink})",
            runs.len(),
            keys.len(),
            t_setup.as_secs_f64() * 1e3,
            t_shape.as_secs_f64() * 1e3,
            t_raster.as_secs_f64() * 1e3,
        );
    }
}
