//! Quantization helpers shared by the pipeline (encode) and the client (decode).

/// f32 to IEEE binary16 bits, round-to-nearest-even; overflow saturates to infinity.
pub fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let man = bits & 0x007f_ffff;

    if exp == 0xff {
        // Inf or NaN; keep NaN quiet and non-zero.
        return sign | 0x7c00 | if man != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        // Subnormal half: restore the implicit bit and shift.
        let m = man | 0x0080_0000;
        let shift = (14 - e) as u32;
        let mut h = (m >> shift) as u16;
        let rem = m & ((1 << shift) - 1);
        let half = 1 << (shift - 1);
        if rem > half || (rem == half && h & 1 == 1) {
            h += 1;
        }
        return sign | h;
    }
    let mut h = sign | ((e as u16) << 10) | (man >> 13) as u16;
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && h & 1 == 1) {
        // Carry may roll into the exponent, which is the correct result.
        h += 1;
    }
    h
}

pub fn f16_bits_to_f32(h: u16) -> f32 {
    let sign = (u32::from(h) & 0x8000) << 16;
    let exp = u32::from(h >> 10) & 0x1f;
    let man = u32::from(h) & 0x03ff;
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalize.
            let lz = m.leading_zeros() - 21;
            let e = 127 - 15 + 1 - lz;
            sign | (e << 23) | (((m << lz) & 0x03ff) << 13)
        }
        (0x1f, m) => sign | 0x7f80_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

fn sign_not_zero(v: f32) -> f32 {
    if v >= 0.0 { 1.0 } else { -1.0 }
}

/// Unit vector to octahedral snorm16. Input is normalized here; zero maps to +Z.
pub fn oct_encode(n: [f32; 3]) -> [i16; 2] {
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let [x, y, z] = if len > 0.0 {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    };
    let inv_l1 = 1.0 / (x.abs() + y.abs() + z.abs());
    let (mut px, mut py) = (x * inv_l1, y * inv_l1);
    if z < 0.0 {
        let (ox, oy) = (px, py);
        px = (1.0 - oy.abs()) * sign_not_zero(ox);
        py = (1.0 - ox.abs()) * sign_not_zero(oy);
    }
    [snorm16(px), snorm16(py)]
}

pub fn oct_decode(e: [i16; 2]) -> [f32; 3] {
    let (mut x, mut y) = (unsnorm16(e[0]), unsnorm16(e[1]));
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * sign_not_zero(ox);
        y = (1.0 - ox.abs()) * sign_not_zero(oy);
    }
    let len = (x * x + y * y + z * z).sqrt();
    [x / len, y / len, z / len]
}

fn snorm16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn unsnorm16(v: i16) -> f32 {
    (f32::from(v) / 32767.0).max(-1.0)
}

/// Quaternion xyzw to snorm16. Input is normalized here; components are within [-1, 1].
pub fn quat_pack(q: [f32; 4]) -> [i16; 4] {
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    let q = if len > 0.0 {
        q.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    q.map(snorm16)
}

/// Inverse of [`quat_pack`], renormalized to absorb quantization error.
pub fn quat_unpack(p: [i16; 4]) -> [f32; 4] {
    let q = p.map(unsnorm16);
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 0.0 {
        q.map(|c| c / len)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Weights to unorm8 summing to exactly 255 (largest-remainder rounding).
/// All-zero input puts everything on the first influence.
pub fn quantize_weights(w: [f32; 4]) -> [u8; 4] {
    let w = w.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    let sum: f32 = w.iter().sum();
    if sum <= 0.0 {
        return [255, 0, 0, 0];
    }
    let scaled = w.map(|v| v / sum * 255.0);
    let mut out = scaled.map(|v| v.floor() as u8);
    let mut left = 255 - out.iter().map(|&v| u32::from(v)).sum::<u32>();
    let mut order = [0usize, 1, 2, 3];
    // Stable sort keeps lower indices first on ties.
    order.sort_by(|&a, &b| {
        let fa = scaled[a] - scaled[a].floor();
        let fb = scaled[b] - scaled[b].floor();
        fb.total_cmp(&fa)
    });
    for &i in &order {
        if left == 0 {
            break;
        }
        out[i] += 1;
        left -= 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_known_values() {
        assert_eq!(f32_to_f16_bits(0.0), 0x0000);
        assert_eq!(f32_to_f16_bits(-0.0), 0x8000);
        assert_eq!(f32_to_f16_bits(1.0), 0x3c00);
        assert_eq!(f32_to_f16_bits(-2.0), 0xc000);
        assert_eq!(f32_to_f16_bits(0.5), 0x3800);
        assert_eq!(f32_to_f16_bits(65504.0), 0x7bff);
        assert_eq!(f32_to_f16_bits(1e9), 0x7c00);
        assert_eq!(f32_to_f16_bits(f32::NEG_INFINITY), 0xfc00);
        // Smallest subnormal half.
        assert_eq!(f32_to_f16_bits(5.960_464_5e-8), 0x0001);
        assert_eq!(f32_to_f16_bits(1e-12), 0x0000);
        assert!(f16_bits_to_f32(f32_to_f16_bits(f32::NAN)).is_nan());
    }

    #[test]
    fn f16_decode_known_values() {
        assert_eq!(f16_bits_to_f32(0x3c00), 1.0);
        assert_eq!(f16_bits_to_f32(0xc000), -2.0);
        assert_eq!(f16_bits_to_f32(0x7bff), 65504.0);
        assert_eq!(f16_bits_to_f32(0x0001), 5.960_464_5e-8);
        assert_eq!(f16_bits_to_f32(0x7c00), f32::INFINITY);
    }

    #[test]
    fn f16_roundtrips_every_finite_half() {
        for h in 0..=u16::MAX {
            let f = f16_bits_to_f32(h);
            if f.is_nan() {
                continue;
            }
            assert_eq!(f32_to_f16_bits(f), h, "bits {h:#06x}");
        }
    }

    #[test]
    fn f16_rounds_to_nearest_even() {
        // 1 + 2^-11 is a tie between 0x3c00 and 0x3c01; even wins.
        assert_eq!(f32_to_f16_bits(1.0 + 2.0_f32.powi(-11)), 0x3c00);
        assert_eq!(f32_to_f16_bits(1.0 + 3.0 * 2.0_f32.powi(-11)), 0x3c02);
    }

    #[test]
    fn oct_roundtrip_is_close() {
        let dirs: [[f32; 3]; 7] = [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.3, -0.5, 0.8],
            [-0.7, 0.2, -0.4],
            [-0.1, -0.9, -0.3],
        ];
        for d in dirs {
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let n = d.map(|c| c / len);
            let r = oct_decode(oct_encode(n));
            let dot: f32 = (0..3).map(|i| n[i] * r[i]).sum();
            assert!(dot > 0.9999, "{n:?} -> {r:?}");
        }
    }

    #[test]
    fn oct_zero_maps_to_z() {
        assert_eq!(oct_decode(oct_encode([0.0; 3])), [0.0, 0.0, 1.0]);
    }

    #[test]
    fn quat_roundtrip_is_close() {
        let q = [0.1, -0.5, 0.3, 0.8];
        let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
        let q = q.map(|c| c / len);
        let r = quat_unpack(quat_pack(q));
        for i in 0..4 {
            assert!((q[i] - r[i]).abs() < 1e-4);
        }
        assert_eq!(
            quat_unpack(quat_pack([0.0, 0.0, 0.0, 1.0])),
            [0.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(quat_unpack([0; 4]), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn weights_sum_to_255() {
        let cases = [
            [0.25, 0.25, 0.25, 0.25],
            [1.0, 0.0, 0.0, 0.0],
            [0.5, 0.3, 0.2, 0.0],
            [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, 0.0],
            [10.0, 5.0, 1.0, 1.0],
            [0.7, 0.1, 0.1, 0.1],
        ];
        for w in cases {
            let q = quantize_weights(w);
            assert_eq!(q.iter().map(|&v| u32::from(v)).sum::<u32>(), 255, "{w:?}");
        }
        assert_eq!(quantize_weights([1.0, 0.0, 0.0, 0.0]), [255, 0, 0, 0]);
        assert_eq!(quantize_weights([0.0; 4]), [255, 0, 0, 0]);
        assert_eq!(quantize_weights([f32::NAN, 1.0, 0.0, 0.0]), [0, 255, 0, 0]);
    }
}
