//! Decoder tests for every curve format 0-18 on hand-built data with known values.

use crate::GrannyError;
use crate::curve::format::*;
use crate::curve::{Curve, CurveKind, CurveParts, Packed, format_name};

fn close(got: &[f32], want: &[f32], tol: f32) {
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() <= tol, "{got:?} vs {want:?}");
    }
}

/// `OneOverKnotScaleTrunc` for a scale that is exactly representable in 16 bits.
fn trunc(scale: f32) -> u16 {
    let bits = scale.to_bits();
    assert_eq!(bits & 0xFFFF, 0, "scale not representable");
    (bits >> 16) as u16
}

fn parts(format: u8, degree: u8) -> CurveParts {
    CurveParts::new(format, degree)
}

fn decode(p: &CurveParts) -> Curve {
    Curve::from_parts(p, 2.0).unwrap()
}

#[test]
fn identity_curves_give_identity_values() {
    let mut p = parts(DA_IDENTITY, 0);
    for (dim, want) in [
        (4, vec![0.0, 0.0, 0.0, 1.0]),
        (3, vec![0.0; 3]),
        (9, vec![1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
    ] {
        p.dimension = dim;
        let c = decode(&p);
        assert_eq!((c.kind, c.dim), (CurveKind::Identity, dim as usize));
        let mut out = vec![9.0; dim as usize];
        c.sample_into(0.7, &mut out);
        assert_eq!(out, want);
    }
    p.dimension = -1;
    assert!(Curve::from_parts(&p, 1.0).is_err());
}

#[test]
fn constant_curves() {
    let mut p = parts(DA_CONSTANT_32F, 0);
    p.controls = vec![1.0, 2.0];
    let c = decode(&p);
    assert_eq!((c.kind, c.dim), (CurveKind::Constant, 2));
    assert_eq!(c.sample::<2>(5.0), [1.0, 2.0]);

    let mut p = parts(D3_CONSTANT_32F, 0);
    p.controls = vec![1.0, 2.0, 3.0];
    assert_eq!(decode(&p).sample::<3>(-1.0), [1.0, 2.0, 3.0]);
    p.controls.pop();
    assert!(Curve::from_parts(&p, 1.0).is_err());

    let mut p = parts(D4_CONSTANT_32F, 0);
    p.controls = vec![0.0, 0.0, 0.0, 1.0];
    assert_eq!(decode(&p).sample::<4>(1.0), [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn keyframes_32f_are_spread_evenly_over_the_duration() {
    let mut p = parts(DA_KEYFRAMES_32F, 1);
    p.dimension = 3;
    p.controls = vec![0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 2.0, 4.0, 6.0];
    let c = Curve::from_parts(&p, 2.0).unwrap();
    assert_eq!(c.knots, [0.0, 1.0, 2.0]);
    close(&c.sample::<3>(0.5), &[0.5, 1.0, 1.5], 1e-6);
    close(&c.sample::<3>(1.5), &[1.5, 3.0, 4.5], 1e-6);
    p.dimension = 0;
    assert!(Curve::from_parts(&p, 2.0).is_err());
}

#[test]
fn da_k32f_c32f_linear() {
    let mut p = parts(DA_K32F_C32F, 1);
    p.knots = vec![0.0, 1.0, 3.0];
    p.controls = vec![0.0, 10.0, 20.0];
    let c = decode(&p);
    assert_eq!(c.dim, 1);
    close(&c.sample::<1>(0.5), &[5.0], 1e-6);
    close(&c.sample::<1>(2.0), &[15.0], 1e-6);
    p.controls.push(1.0);
    assert!(
        Curve::from_parts(&p, 1.0).is_err(),
        "controls not a multiple of knots"
    );
}

#[test]
fn da_k16u_c16u_and_k8u_c8u() {
    // dimension 2: scales [0.5, 2], offsets [1, -1]; knot scale 10 (knots 0 and 1)
    let scale_offsets = vec![0.5, 2.0, 1.0, -1.0];
    for packed in [
        Packed::U16(vec![0, 10, 2, 4, 4, 6]),
        Packed::U8(vec![0, 10, 2, 4, 4, 6]),
    ] {
        let fmt = if matches!(packed, Packed::U16(_)) {
            DA_K16U_C16U
        } else {
            DA_K8U_C8U
        };
        let mut p = parts(fmt, 1);
        p.one_over_knot_scale_trunc = trunc(10.0);
        p.control_scales = scale_offsets.clone();
        p.knots_controls = packed;
        let c = decode(&p);
        assert_eq!((c.dim, c.knots.clone()), (2, vec![0.0, 1.0]));
        // controls: (2*.5+1, 4*2-1) = (2, 7) and (4*.5+1, 6*2-1) = (3, 11)
        assert_eq!(c.controls, [2.0, 7.0, 3.0, 11.0]);
        close(&c.sample::<2>(0.5), &[2.5, 9.0], 1e-5);
    }
    let mut p = parts(DA_K16U_C16U, 1);
    p.knots_controls = Packed::U16(vec![0, 1, 2]);
    assert!(Curve::from_parts(&p, 1.0).is_err(), "no scale/offsets");
    p.control_scales = vec![1.0, 0.0];
    assert!(Curve::from_parts(&p, 1.0).is_err(), "zero knot scale");
}

/// Encodes `q` like Granny's smallest-three formats: drops component `drop`, stores the others
/// with `bits` value bits and selector 0 (scale `sqrt(2)/2^bits`-ish, offset -sqrt(2)/2).
fn encode_quat(q: [f32; 4], drop: usize, bits: u32) -> [u32; 3] {
    let top = 1u32 << bits;
    let unit = if bits == 15 {
        0.000_030_518_51
    } else {
        0.007_874_016
    };
    let scale = std::f32::consts::SQRT_2 * unit;
    let offset = -0.707_106_77;
    let (s2, s3, s4) = ((drop + 1) & 3, (drop + 2) & 3, (drop + 3) & 3);
    let enc = |c: usize| ((q[c] - offset) / scale).round() as u32;
    let (mut a, mut b, mut c) = (enc(s2), enc(s3), enc(s4));
    if q[drop] < 0.0 {
        a |= top;
    }
    b |= ((drop as u32) >> 1) << bits;
    c |= ((drop as u32) & 1) << bits;
    [a, b, c]
}

#[test]
fn smallest_three_quaternions_16_and_8_bit() {
    let q = [0.5f32, -0.5, 0.5, 0.5];
    let q_neg_w = [0.5f32, 0.5, -0.5, -0.5];
    for (fmt, bits, tol) in [(D4N_K16U_C15U, 15, 2e-3), (D4N_K8U_C7U, 7, 2e-2)] {
        for drop in 0..4 {
            for want in [q, q_neg_w] {
                let [a, b, c] = encode_quat(want, drop, bits);
                let mut p = parts(fmt, 2);
                p.one_over_knot_scale = 2.0;
                p.knots_controls = if bits == 15 {
                    Packed::U16(vec![
                        0, 2, a as u16, b as u16, c as u16, a as u16, b as u16, c as u16,
                    ])
                } else {
                    Packed::U8(vec![
                        0, 2, a as u8, b as u8, c as u8, a as u8, b as u8, c as u8,
                    ])
                };
                let cv = decode(&p);
                assert_eq!((cv.dim, cv.knots.clone()), (4, vec![0.0, 1.0]));
                close(&cv.controls[..4], &want, tol);
            }
        }
    }
    // Identity quaternion: w dropped, x/y/z at the offset-cancelling value.
    let [a, b, c] = encode_quat([0.0, 0.0, 0.0, 1.0], 3, 15);
    let mut p = parts(D4N_K16U_C15U, 2);
    p.one_over_knot_scale = 1.0;
    p.knots_controls = Packed::U16(vec![0, a as u16, b as u16, c as u16]);
    close(&decode(&p).controls, &[0.0, 0.0, 0.0, 1.0], 1e-3);
}

#[test]
fn smallest_three_selectors_pick_scale_and_offset_entries() {
    // Selector 1 for component 0: scale 0.7071 * unit, offset -0.35355.
    let mut p = parts(D4N_K16U_C15U, 2);
    p.one_over_knot_scale = 1.0;
    p.scale_offset_table_entries = 0x0001;
    // swizzle1 = 3 (b and c top bits set): da -> x (selector 1), db -> y, dc -> z (selector 0).
    let x_raw = ((0.25f32 + 0.353_553_38) / (0.707_106_77 * 0.000_030_518_51)).round() as u16;
    let zero = 16384u16; // 0 with selector 0
    p.knots_controls = Packed::U16(vec![0, x_raw, zero | 0x8000, zero | 0x8000]);
    let c = decode(&p);
    let w = (1.0f32 - 0.0625).sqrt();
    close(&c.controls, &[0.25, 0.0, 0.0, w], 2e-3);
}

#[test]
fn d3_k16u_k8u_decode_per_axis_scales() {
    // Two knots (scale 4 -> 0 and 2), controls (1,2,3) and (4,5,6), scales (1,2,3), offsets (10,20,30).
    for packed in [
        Packed::U16(vec![0, 8, 1, 2, 3, 4, 5, 6]),
        Packed::U8(vec![0, 8, 1, 2, 3, 4, 5, 6]),
    ] {
        let fmt = if matches!(packed, Packed::U16(_)) {
            D3_K16U_C16U
        } else {
            D3_K8U_C8U
        };
        let mut p = parts(fmt, 1);
        p.one_over_knot_scale_trunc = trunc(4.0);
        p.control_scales = vec![1.0, 2.0, 3.0];
        p.control_offsets = vec![10.0, 20.0, 30.0];
        p.knots_controls = packed;
        let c = decode(&p);
        assert_eq!(c.knots, [0.0, 2.0]);
        assert_eq!(c.controls, [11.0, 24.0, 39.0, 14.0, 30.0, 48.0]);
        close(&c.sample::<3>(1.0), &[12.5, 27.0, 43.5], 1e-5);
    }
    let mut p = parts(D3_K16U_C16U, 1);
    p.knots_controls = Packed::U16(vec![0, 1, 2, 3]);
    assert!(Curve::from_parts(&p, 1.0).is_err(), "missing scales");
}

#[test]
fn d3i1_one_control_drives_all_three_axes() {
    // n = len/2: two knots and two controls.
    for (fmt, packed) in [
        (D3I1_K16U_C16U, Packed::U16(vec![0, 8, 2, 4])),
        (D3I1_K8U_C8U, Packed::U8(vec![0, 8, 2, 4])),
    ] {
        let mut p = parts(fmt, 1);
        p.one_over_knot_scale_trunc = trunc(4.0);
        p.control_scales = vec![1.0, 10.0, 100.0];
        p.control_offsets = vec![0.0, 1.0, 2.0];
        p.knots_controls = packed;
        let c = decode(&p);
        assert_eq!(c.knots, [0.0, 2.0]);
        assert_eq!(c.controls, [2.0, 21.0, 202.0, 4.0, 41.0, 402.0]);
    }
    let mut p = parts(D3I1_K32F_C32F, 1);
    p.control_scales = vec![1.0, 2.0, 3.0];
    p.control_offsets = vec![0.0; 3];
    p.knots_controls = Packed::F32(vec![0.0, 1.5, 1.0, 3.0]);
    let c = decode(&p);
    assert_eq!(c.knots, [0.0, 1.5]);
    assert_eq!(c.controls, [1.0, 2.0, 3.0, 3.0, 6.0, 9.0]);
    close(&c.sample::<3>(0.75), &[2.0, 4.0, 6.0], 1e-5);
}

#[test]
fn d9_curves_expand_to_diagonal_scale_matrices() {
    // I1: one uniform scale per knot (n = len/2); I3: three per knot (n = len/4).
    for (fmt, packed) in [
        (D9I1_K16U_C16U, Packed::U16(vec![0, 4, 10, 20])),
        (D9I1_K8U_C8U, Packed::U8(vec![0, 4, 10, 20])),
    ] {
        let mut p = parts(fmt, 1);
        p.one_over_knot_scale_trunc = trunc(4.0);
        p.control_scales = vec![0.1];
        p.control_offsets = vec![0.5];
        p.knots_controls = packed;
        let c = decode(&p);
        assert_eq!((c.dim, c.knots.clone()), (9, vec![0.0, 1.0]));
        close(
            &c.controls[..9],
            &[1.5, 0., 0., 0., 1.5, 0., 0., 0., 1.5],
            1e-5,
        );
        close(
            &c.controls[9..],
            &[2.5, 0., 0., 0., 2.5, 0., 0., 0., 2.5],
            1e-5,
        );
        close(
            &c.sample::<9>(0.5),
            &[2.0, 0., 0., 0., 2.0, 0., 0., 0., 2.0],
            1e-5,
        );
    }
    for (fmt, packed) in [
        (D9I3_K16U_C16U, Packed::U16(vec![0, 4, 1, 2, 3, 4, 5, 6])),
        (D9I3_K8U_C8U, Packed::U8(vec![0, 4, 1, 2, 3, 4, 5, 6])),
    ] {
        let mut p = parts(fmt, 1);
        p.one_over_knot_scale_trunc = trunc(4.0);
        p.control_scales = vec![1.0, 2.0, 3.0];
        p.control_offsets = vec![0.0, 0.0, 1.0];
        p.knots_controls = packed;
        let c = decode(&p);
        assert_eq!((c.dim, c.knots.clone()), (9, vec![0.0, 1.0]));
        assert_eq!(&c.controls[..9], &[1., 0., 0., 0., 4., 0., 0., 0., 10.]);
        assert_eq!(&c.controls[9..], &[4., 0., 0., 0., 10., 0., 0., 0., 19.]);
    }
}

#[test]
fn unknown_format_is_an_error() {
    let e = Curve::from_parts(&parts(19, 1), 1.0).unwrap_err();
    assert!(matches!(e, GrannyError::Invalid(m) if m.contains("19")));
    assert_eq!(format_name(19), "Unknown");
    assert_eq!(format_name(D3I1_K8U_C8U), "D3I1K8uC8u");
}

#[test]
fn mismatched_knots_and_controls_are_rejected() {
    let mut p = parts(DA_K32F_C32F, 1);
    p.knots = vec![0.0, 1.0];
    p.controls = vec![];
    assert!(Curve::from_parts(&p, 1.0).is_err());
}

#[test]
fn quadratic_with_three_controls_is_a_bezier() {
    // Granny stores [k0, k0, k1] for 3 controls of degree 2: a single Bezier segment.
    let mut p = parts(DA_K32F_C32F, 2);
    p.knots = vec![0.0, 0.0, 1.0];
    p.controls = vec![0.0, 4.0, 0.0];
    let c = decode(&p);
    close(&c.sample::<1>(0.0), &[0.0], 1e-6);
    close(&c.sample::<1>(0.5), &[2.0], 1e-6); // 0.25*0 + 0.5*4 + 0.25*0
    close(&c.sample::<1>(1.0), &[0.0], 1e-6);
    assert!(c.is_finite());
}

#[test]
fn cubic_spline_interpolates_ends_and_stays_inside_the_hull() {
    let mut p = parts(DA_K32F_C32F, 3);
    p.knots = vec![0.0, 0.0, 0.0, 0.5, 1.0];
    p.controls = vec![0.0, 1.0, 3.0, 2.0, 5.0];
    let c = decode(&p);
    close(&c.sample::<1>(0.0), &[0.0], 1e-6);
    close(&c.sample::<1>(1.0), &[5.0], 1e-6);
    for i in 0..=20 {
        let v = c.sample::<1>(i as f32 / 20.0)[0];
        assert!((-1e-5..=5.0 + 1e-5).contains(&v), "{v}");
    }
}

#[test]
fn quaternion_controls_are_made_continuous() {
    // The second control is the negated first; blending them directly would give zero.
    let mut p = parts(DA_K32F_C32F, 1);
    p.knots = vec![0.0, 1.0];
    p.controls = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
    let c = decode(&p);
    close(&c.sample::<4>(0.5), &[0.0, 0.0, 0.0, 1.0], 1e-6);
}

#[test]
fn piecewise_constant_when_degree_is_zero() {
    let mut p = parts(DA_K32F_C32F, 0);
    p.knots = vec![0.0, 1.0, 2.0];
    p.controls = vec![1.0, 2.0, 3.0];
    let c = decode(&p);
    assert_eq!(c.sample::<1>(0.5), [1.0]);
    assert_eq!(c.sample::<1>(1.5), [2.0]);
    assert_eq!(c.sample::<1>(9.0), [3.0]);
}
