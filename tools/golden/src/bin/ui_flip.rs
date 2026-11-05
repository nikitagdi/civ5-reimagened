//! `ui_flip <ref.rgba> <test.rgba> <width> <height> <heat.rgba>` (B4 M5 AC-S2): the LDR-FLIP of two raw RGBA8 frames of the
//! same size (the original's golden against our frame). Prints `mean=<f> p95=<f> over02=<share of pixels with error > 0.2>` and
//! writes the error map as a heat map (RGBA8, `tools/playtest/lib/flip.mjs` makes the PNG).
use std::path::Path;

use anyhow::{Context, Result, bail};
use golden::image::read_raw_rgba;
use golden::metrics::flip::flip_ldr;

/// Magma-like ramp for 0..1.
fn heat(v: f32) -> [u8; 4] {
    const STOPS: [[f32; 3]; 5] = [
        [0.0, 0.0, 0.02],
        [0.32, 0.06, 0.43],
        [0.72, 0.21, 0.47],
        [0.99, 0.54, 0.38],
        [0.99, 0.99, 0.75],
    ];
    let t = v.clamp(0.0, 1.0) * 4.0;
    let i = (t as usize).min(3);
    let f = t - i as f32;
    let mix = |c: usize| ((STOPS[i][c] * (1.0 - f) + STOPS[i + 1][c] * f) * 255.0 + 0.5) as u8;
    [mix(0), mix(1), mix(2), 255]
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 5 {
        bail!("usage: ui_flip <ref.rgba> <test.rgba> <width> <height> <heat.rgba>");
    }
    let (w, h): (usize, usize) = (a[2].parse()?, a[3].parse()?);
    let r = read_raw_rgba(Path::new(&a[0]), w, h)?;
    let t = read_raw_rgba(Path::new(&a[1]), w, h)?;
    let map = flip_ldr(
        &r.to_image(),
        &t.to_image(),
        golden::metrics::flip::DEFAULT_PPD,
    );
    let mean = map.iter().map(|&v| f64::from(v)).sum::<f64>() / map.len() as f64;
    let over = map.iter().filter(|&&v| v > 0.2).count() as f64 / map.len() as f64;
    let mut sorted = map.clone();
    sorted.sort_by(|x, y| x.total_cmp(y));
    let p95 = sorted[(sorted.len() as f64 * 0.95) as usize];
    let heat_rgba: Vec<u8> = map.iter().flat_map(|&v| heat(v)).collect();
    std::fs::write(&a[4], heat_rgba).context("write the heat map")?;
    println!("mean={mean:.5} p95={p95:.5} over02={over:.5}");
    Ok(())
}
