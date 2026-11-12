//! `ui_flip <ref.rgba> <test.rgba> <width> <height> <heat.rgba> [<mask.r8>]`: the LDR-FLIP of two raw
//! RGBA8 frames of the same size (the original's golden against our frame). Prints `mean=<f> p95=<f> over02=<share of
//! pixels with error > 0.2> pixels=<n>` and writes the error map as a heat map (RGBA8, `tools/playtest/lib/flip.mjs`
//! makes the PNG). With a mask (one byte per pixel), the numbers cover the pixels whose mask byte is not 0 only (the
//! UI v2 goldens: the interface without the solid background around it); `pixels` is how many that is.
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

/// Mean, 95th percentile and the share over 0.2 of `values` (all 0 when there are none).
fn stats(mut values: Vec<f32>) -> (f64, f32, f64) {
    if values.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    let n = values.len() as f64;
    let mean = values.iter().map(|&v| f64::from(v)).sum::<f64>() / n;
    let over = values.iter().filter(|&&v| v > 0.2).count() as f64 / n;
    values.sort_by(|x, y| x.total_cmp(y));
    let p95 = values[((n * 0.95) as usize).min(values.len() - 1)];
    (mean, p95, over)
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 5 && a.len() != 6 {
        bail!("usage: ui_flip <ref.rgba> <test.rgba> <width> <height> <heat.rgba> [<mask.r8>]");
    }
    let (w, h): (usize, usize) = (a[2].parse()?, a[3].parse()?);
    let r = read_raw_rgba(Path::new(&a[0]), w, h)?;
    let t = read_raw_rgba(Path::new(&a[1]), w, h)?;
    let map = flip_ldr(
        &r.to_image(),
        &t.to_image(),
        golden::metrics::flip::DEFAULT_PPD,
    );
    let values: Vec<f32> = match a.get(5) {
        Some(m) => {
            let mask = std::fs::read(m).with_context(|| format!("failed to read {m}"))?;
            if mask.len() != w * h {
                bail!("{m}: {} bytes, a {w}x{h} mask needs {}", mask.len(), w * h);
            }
            map.iter()
                .zip(&mask)
                .filter(|(_, k)| **k != 0)
                .map(|(v, _)| *v)
                .collect()
        }
        None => map.clone(),
    };
    let pixels = values.len();
    let (mean, p95, over) = stats(values);
    let heat_rgba: Vec<u8> = map.iter().flat_map(|&v| heat(v)).collect();
    std::fs::write(&a[4], heat_rgba).context("write the heat map")?;
    println!("mean={mean:.5} p95={p95:.5} over02={over:.5} pixels={pixels}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::stats;

    #[test]
    fn stats_of_nothing_are_zero() {
        assert_eq!(stats(Vec::new()), (0.0, 0.0, 0.0));
    }

    #[test]
    fn stats_of_a_few_values() {
        let (mean, p95, over) = stats(vec![0.0, 0.1, 0.3, 0.4]);
        assert!((mean - 0.2).abs() < 1e-6);
        assert!((p95 - 0.4).abs() < 1e-6);
        assert!((over - 0.5).abs() < 1e-9);
    }
}
