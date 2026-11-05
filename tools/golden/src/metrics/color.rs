//! Colour primitives shared by the metrics (sRGB, XYZ D65, CIELAB).

/// sRGB transfer function threshold (IEC 61966-2-1).
const SRGB_THRESHOLD: f64 = 0.04045;

/// D65 white point for CIELAB (CIE 15:2004, 2 degree observer), used by the ΔE2000 path.
pub const WHITE_D65: [f64; 3] = [0.950_47, 1.0, 1.088_83];

/// Linear sRGB -> XYZ (IEC 61966-2-1, D65), row major.
pub const SRGB_TO_XYZ: [[f64; 3]; 3] = [
    [0.412_456_4, 0.357_576_1, 0.180_437_5],
    [0.212_672_9, 0.715_152_2, 0.072_175_0],
    [0.019_333_9, 0.119_192_0, 0.950_304_1],
];

/// 8-bit sRGB channel to linear light, 0..1.
pub fn srgb8_to_linear(v: u8) -> f64 {
    srgb_to_linear(f64::from(v) / 255.0)
}

/// sRGB-encoded 0..1 to linear light.
pub fn srgb_to_linear(c: f64) -> f64 {
    if c <= SRGB_THRESHOLD {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn mul3(m: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// CIELAB companding: `t^(1/3)` above (6/29)^3, linear piece below.
pub fn lab_f(t: f64) -> f64 {
    const DELTA: f64 = 6.0 / 29.0;
    if t > DELTA * DELTA * DELTA {
        t.cbrt()
    } else {
        t / (3.0 * DELTA * DELTA) + 4.0 / 29.0
    }
}

/// XYZ to CIELAB for the given white.
pub fn xyz_to_lab(xyz: [f64; 3], white: [f64; 3]) -> [f64; 3] {
    let fx = lab_f(xyz[0] / white[0]);
    let fy = lab_f(xyz[1] / white[1]);
    let fz = lab_f(xyz[2] / white[2]);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// 8-bit sRGB pixel to CIELAB (D65).
pub fn srgb8_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = [
        srgb8_to_linear(rgb[0]),
        srgb8_to_linear(rgb[1]),
        srgb8_to_linear(rgb[2]),
    ];
    xyz_to_lab(mul3(&SRGB_TO_XYZ, lin), WHITE_D65)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_and_black() {
        let w = srgb8_to_lab([255, 255, 255]);
        assert!(
            (w[0] - 100.0).abs() < 1e-3 && w[1].abs() < 2e-3 && w[2].abs() < 2e-3,
            "{w:?}"
        );
        let k = srgb8_to_lab([0, 0, 0]);
        assert!(k[0].abs() < 1e-9);
    }

    #[test]
    fn red_matches_known_lab() {
        // sRGB (255, 0, 0) in Lab D65 is about (53.24, 80.09, 67.20).
        let r = srgb8_to_lab([255, 0, 0]);
        assert!(
            (r[0] - 53.24).abs() < 0.02
                && (r[1] - 80.09).abs() < 0.05
                && (r[2] - 67.20).abs() < 0.05,
            "{r:?}"
        );
    }
}
