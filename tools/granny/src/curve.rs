//! Granny curves (`granny_curve2`): all 19 data formats decoded to knots and f32 controls.
//!
//! Format numbering and field layouts follow Granny 2.x. Formats 0-11 are ported from
//! `nwn2mdk-lib/gr2.cpp`; 12-18 from LSLib `Granny/Model/CurveData/*.cs` (see
//! `third_party/NOTICE-granny.md`).
//!
//! SPDX-License-Identifier: MIT AND Apache-2.0. Formats 0-11 and the D4n quaternion tables are a
//! Rust port of nwn2mdk (github.com/Arbos/nwn2mdk, Apache-2.0, text in `tools/granny/LICENSE-APACHE`),
//! changed by the civ5-reimagened authors (rewritten in Rust, with checked reads); the rest is the
//! project's own MIT code.

use std::f32::consts::{FRAC_1_SQRT_2, SQRT_2};

use crate::{GrannyError, Object, Value};

/// Curve data format codes (`CurveDataHeader.Format`).
pub mod format {
    pub const DA_KEYFRAMES_32F: u8 = 0;
    pub const DA_K32F_C32F: u8 = 1;
    pub const DA_IDENTITY: u8 = 2;
    pub const DA_CONSTANT_32F: u8 = 3;
    pub const D3_CONSTANT_32F: u8 = 4;
    pub const D4_CONSTANT_32F: u8 = 5;
    pub const DA_K16U_C16U: u8 = 6;
    pub const DA_K8U_C8U: u8 = 7;
    pub const D4N_K16U_C15U: u8 = 8;
    pub const D4N_K8U_C7U: u8 = 9;
    pub const D3_K16U_C16U: u8 = 10;
    pub const D3_K8U_C8U: u8 = 11;
    pub const D9I1_K16U_C16U: u8 = 12;
    pub const D9I3_K16U_C16U: u8 = 13;
    pub const D9I1_K8U_C8U: u8 = 14;
    pub const D9I3_K8U_C8U: u8 = 15;
    pub const D3I1_K32F_C32F: u8 = 16;
    pub const D3I1_K16U_C16U: u8 = 17;
    pub const D3I1_K8U_C8U: u8 = 18;
    pub const COUNT: usize = 19;

    pub const NAMES: [&str; COUNT] = [
        "DaKeyframes32f",
        "DaK32fC32f",
        "DaIdentity",
        "DaConstant32f",
        "D3Constant32f",
        "D4Constant32f",
        "DaK16uC16u",
        "DaK8uC8u",
        "D4nK16uC15u",
        "D4nK8uC7u",
        "D3K16uC16u",
        "D3K8uC8u",
        "D9I1K16uC16u",
        "D9I3K16uC16u",
        "D9I1K8uC8u",
        "D9I3K8uC8u",
        "D3I1K32fC32f",
        "D3I1K16uC16u",
        "D3I1K8uC8u",
    ];
}

/// Name of a format code, for reports.
pub fn format_name(f: u8) -> &'static str {
    format::NAMES
        .get(usize::from(f))
        .copied()
        .unwrap_or("Unknown")
}

/// Quantised knot/control array of a curve.
#[derive(Debug, Clone, PartialEq)]
pub enum Packed {
    U8(Vec<u8>),
    U16(Vec<u16>),
    F32(Vec<f32>),
}

impl Packed {
    fn len(&self) -> usize {
        match self {
            Packed::U8(v) => v.len(),
            Packed::U16(v) => v.len(),
            Packed::F32(v) => v.len(),
        }
    }

    fn at(&self, i: usize) -> f32 {
        match self {
            Packed::U8(v) => v.get(i).map_or(0.0, |&x| f32::from(x)),
            Packed::U16(v) => v.get(i).map_or(0.0, |&x| f32::from(x)),
            Packed::F32(v) => v.get(i).copied().unwrap_or(0.0),
        }
    }

    fn raw(&self, i: usize) -> u32 {
        match self {
            Packed::U8(v) => v.get(i).map_or(0, |&x| u32::from(x)),
            Packed::U16(v) => v.get(i).map_or(0, |&x| u32::from(x)),
            Packed::F32(_) => 0,
        }
    }
}

/// Raw fields of a curve data object, before dequantisation. Unused fields stay at defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveParts {
    pub format: u8,
    pub degree: u8,
    /// `DaIdentity` / `DaKeyframes32f` dimension.
    pub dimension: i32,
    /// `OneOverKnotScaleTrunc`: upper 16 bits of an f32.
    pub one_over_knot_scale_trunc: u16,
    /// `OneOverKnotScale` (D4n formats).
    pub one_over_knot_scale: f32,
    /// `ScaleOffsetTableEntries` (D4n formats).
    pub scale_offset_table_entries: u16,
    /// `ControlScaleOffsets` (Da formats: scales then offsets) or `ControlScales`/`ControlScale`.
    pub control_scales: Vec<f32>,
    /// `ControlOffsets` / `ControlOffset` (D3/D9 formats).
    pub control_offsets: Vec<f32>,
    /// Constant controls, `Controls` of `DaKeyframes32f`, `DaK32fC32f`, `DaConstant32f`, `D*Constant32f`.
    pub controls: Vec<f32>,
    /// `Knots` of `DaK32fC32f`.
    pub knots: Vec<f32>,
    /// `KnotsControls`.
    pub knots_controls: Packed,
}

impl CurveParts {
    pub fn new(format: u8, degree: u8) -> Self {
        CurveParts {
            format,
            degree,
            dimension: 0,
            one_over_knot_scale_trunc: 0,
            one_over_knot_scale: 0.0,
            scale_offset_table_entries: 0,
            control_scales: Vec::new(),
            control_offsets: Vec::new(),
            controls: Vec::new(),
            knots: Vec::new(),
            knots_controls: Packed::U8(Vec::new()),
        }
    }
}

/// A decoded curve of `dim` components. `Identity` carries no data; `Constant` has no knots.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub format: u8,
    pub degree: u8,
    pub dim: usize,
    pub kind: CurveKind,
    /// One knot per control point (empty unless `Spline`).
    pub knots: Vec<f32>,
    /// `dim` values for `Constant`, `knots.len() * dim` for `Spline`.
    pub controls: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveKind {
    Identity,
    Constant,
    Spline,
}

fn bad(msg: &str) -> GrannyError {
    GrannyError::Invalid(format!("curve: {msg}"))
}

fn knot_scale_from_trunc(t: u16) -> f32 {
    f32::from_bits(u32::from(t) << 16)
}

// Quaternion scale/offset tables of the D4n formats (nwn2mdk gr2.cpp).
const SCALE_TABLE: [f32; 16] = [
    SQRT_2,
    FRAC_1_SQRT_2,
    0.35355338,
    0.35355338,
    0.35355338,
    0.17677669,
    0.17677669,
    0.17677669,
    -SQRT_2,
    -FRAC_1_SQRT_2,
    -0.35355338,
    -0.35355338,
    -0.35355338,
    -0.17677669,
    -0.17677669,
    -0.17677669,
];
const OFFSET_TABLE: [f32; 16] = [
    -FRAC_1_SQRT_2,
    -0.35355338,
    -0.53033006,
    -0.17677669,
    0.17677669,
    -0.17677669,
    -0.088388346,
    0.0,
    FRAC_1_SQRT_2,
    0.35355338,
    0.53033006,
    0.17677669,
    -0.17677669,
    0.17677669,
    0.088388346,
    -0.0,
];

/// Decodes one packed smallest-three quaternion; `bits` is 15 or 7 (value bits per component).
fn decode_quat(
    a: u32,
    b: u32,
    c: u32,
    bits: u32,
    scales: &[f32; 4],
    offsets: &[f32; 4],
) -> [f32; 4] {
    let top = 1u32 << bits;
    let mask = top - 1;
    let s1a = (b & top) >> (bits - 1);
    let s1b = (c & top) >> bits;
    let swizzle1 = (s1a | s1b) as usize;
    let swizzle2 = (swizzle1 + 1) & 3;
    let swizzle3 = (swizzle2 + 1) & 3;
    let swizzle4 = (swizzle3 + 1) & 3;
    let da = (a & mask) as f32 * scales[swizzle2] + offsets[swizzle2];
    let db = (b & mask) as f32 * scales[swizzle3] + offsets[swizzle3];
    let dc = (c & mask) as f32 * scales[swizzle4] + offsets[swizzle4];
    let mut dd = (1.0 - (da * da + db * db + dc * dc)).max(0.0).sqrt();
    if a & top != 0 {
        dd = -dd;
    }
    let mut q = [0.0f32; 4];
    q[swizzle2] = da;
    q[swizzle3] = db;
    q[swizzle4] = dc;
    q[swizzle1] = dd;
    q
}

impl Curve {
    /// Identity curve of `dim` components.
    pub fn identity(dim: usize) -> Curve {
        Curve {
            format: format::DA_IDENTITY,
            degree: 0,
            dim,
            kind: CurveKind::Identity,
            knots: Vec::new(),
            controls: Vec::new(),
        }
    }

    /// Dequantises `p`. `duration` positions the implicit knots of `DaKeyframes32f`.
    pub fn from_parts(p: &CurveParts, duration: f32) -> Result<Curve, GrannyError> {
        use format::*;
        let kc = &p.knots_controls;
        let mk = |dim: usize, knots: Vec<f32>, mut controls: Vec<f32>| {
            if controls.len() != knots.len() * dim {
                return Err(bad("knot/control count mismatch"));
            }
            if dim == 4 {
                make_quaternions_continuous(&mut controls);
            }
            Ok(Curve {
                format: p.format,
                degree: p.degree,
                dim,
                kind: CurveKind::Spline,
                knots,
                controls,
            })
        };
        let constant = |dim: usize, controls: &[f32]| {
            if controls.len() < dim {
                return Err(bad("constant curve has too few controls"));
            }
            Ok(Curve {
                format: p.format,
                degree: p.degree,
                dim,
                kind: CurveKind::Constant,
                knots: Vec::new(),
                controls: controls[..dim].to_vec(),
            })
        };
        // Knots stored as the first `n` entries of KnotsControls, divided by the knot scale.
        let knots_of = |n: usize, scale: f32| -> Result<Vec<f32>, GrannyError> {
            if !(scale.is_finite() && scale != 0.0) {
                return Err(bad("zero or non-finite knot scale"));
            }
            Ok((0..n).map(|i| kc.at(i) / scale).collect())
        };
        let trunc_scale = knot_scale_from_trunc(p.one_over_knot_scale_trunc);
        let total = kc.len();
        match p.format {
            DA_IDENTITY => {
                let dim = usize::try_from(p.dimension).map_err(|_| bad("negative dimension"))?;
                Ok(Curve::identity(dim))
            }
            DA_CONSTANT_32F => constant(p.controls.len(), &p.controls),
            D3_CONSTANT_32F => constant(3, &p.controls),
            D4_CONSTANT_32F => constant(4, &p.controls),
            DA_KEYFRAMES_32F => {
                let dim = usize::try_from(p.dimension)
                    .ok()
                    .filter(|&d| d > 0)
                    .ok_or_else(|| bad("bad keyframe dimension"))?;
                let n = p.controls.len() / dim;
                let step = if n > 1 {
                    duration / (n - 1) as f32
                } else {
                    0.0
                };
                let knots = (0..n).map(|i| i as f32 * step).collect();
                mk(dim, knots, p.controls[..n * dim].to_vec())
            }
            DA_K32F_C32F => {
                let n = p.knots.len();
                if n == 0 || p.controls.is_empty() || !p.controls.len().is_multiple_of(n) {
                    return Err(bad("DaK32fC32f control count is not a multiple of knots"));
                }
                mk(p.controls.len() / n, p.knots.clone(), p.controls.clone())
            }
            DA_K16U_C16U | DA_K8U_C8U => {
                // control_scales holds [scales..., offsets...] in one array.
                let dim = p.control_scales.len() / 2;
                if dim == 0 {
                    return Err(bad("missing control scale/offsets"));
                }
                let n = total / (dim + 1);
                let knots = knots_of(n, trunc_scale)?;
                let controls = (0..n * dim)
                    .map(|i| {
                        let k = i % dim;
                        kc.at(n + i) * p.control_scales[k] + p.control_scales[k + dim]
                    })
                    .collect();
                mk(dim, knots, controls)
            }
            D4N_K16U_C15U | D4N_K8U_C7U => {
                let n = total / 4;
                let knots = knots_of(n, p.one_over_knot_scale)?;
                let (bits, unit) = if p.format == D4N_K16U_C15U {
                    (15, 0.000_030_518_51)
                } else {
                    (7, 0.007_874_016)
                };
                let sel = |s: u32| ((p.scale_offset_table_entries >> s) & 0xf) as usize;
                let selectors = [sel(0), sel(4), sel(8), sel(12)];
                let scales = selectors.map(|s| SCALE_TABLE[s] * unit);
                let offsets = selectors.map(|s| OFFSET_TABLE[s]);
                let mut controls = Vec::with_capacity(n * 4);
                for i in 0..n {
                    let (a, b, c) = (
                        kc.raw(n + i * 3),
                        kc.raw(n + i * 3 + 1),
                        kc.raw(n + i * 3 + 2),
                    );
                    controls.extend(decode_quat(a, b, c, bits, &scales, &offsets));
                }
                mk(4, knots, controls)
            }
            D3_K16U_C16U | D3_K8U_C8U | D3I1_K16U_C16U | D3I1_K8U_C8U | D3I1_K32F_C32F => {
                if p.control_scales.len() < 3 || p.control_offsets.len() < 3 {
                    return Err(bad("missing control scales/offsets"));
                }
                let one_control =
                    matches!(p.format, D3I1_K16U_C16U | D3I1_K8U_C8U | D3I1_K32F_C32F);
                let (n, knots) = if p.format == D3_K16U_C16U || p.format == D3_K8U_C8U {
                    let n = total / 4;
                    (n, knots_of(n, trunc_scale)?)
                } else if p.format == D3I1_K32F_C32F {
                    let n = total / 2;
                    ((n), (0..n).map(|i| kc.at(i)).collect())
                } else {
                    let n = total / 2;
                    (n, knots_of(n, trunc_scale)?)
                };
                let mut controls = Vec::with_capacity(n * 3);
                for i in 0..n {
                    for k in 0..3 {
                        let raw = if one_control {
                            kc.at(n + i)
                        } else {
                            kc.at(n + i * 3 + k)
                        };
                        controls.push(raw * p.control_scales[k] + p.control_offsets[k]);
                    }
                }
                mk(3, knots, controls)
            }
            D9I1_K16U_C16U | D9I1_K8U_C8U | D9I3_K16U_C16U | D9I3_K8U_C8U => {
                let three = matches!(p.format, D9I3_K16U_C16U | D9I3_K8U_C8U);
                let (scales, offsets) = if three {
                    (p.control_scales.clone(), p.control_offsets.clone())
                } else {
                    (
                        vec![p.control_scales.first().copied().unwrap_or(0.0); 3],
                        vec![p.control_offsets.first().copied().unwrap_or(0.0); 3],
                    )
                };
                if scales.len() < 3 || offsets.len() < 3 {
                    return Err(bad("missing control scales/offsets"));
                }
                let n = if three { total / 4 } else { total / 2 };
                let knots = knots_of(n, trunc_scale)?;
                let mut controls = Vec::with_capacity(n * 9);
                for i in 0..n {
                    let mut m = [0.0f32; 9];
                    for k in 0..3 {
                        let raw = if three {
                            kc.at(n + i * 3 + k)
                        } else {
                            kc.at(n + i)
                        };
                        m[k * 4] = raw * scales[k] + offsets[k];
                    }
                    controls.extend(m);
                }
                mk(9, knots, controls)
            }
            f => Err(GrannyError::Invalid(format!("unknown curve format {f}"))),
        }
    }

    /// Reads and decodes a `CurveData` object (the variant behind `granny_curve2.CurveData`).
    pub fn read(data: &Object<'_>, duration: f32) -> Result<Curve, GrannyError> {
        Curve::from_parts(&read_parts(data)?, duration)
    }

    /// Value at `t` with `N` components. The curve must have exactly `N` components
    /// (checked when animations are loaded); a mismatch yields zeros.
    pub fn sample<const N: usize>(&self, t: f32) -> [f32; N] {
        let mut out = [0.0f32; N];
        self.sample_into(t, &mut out);
        out
    }

    pub fn sample_into(&self, t: f32, out: &mut [f32]) {
        match self.kind {
            CurveKind::Identity => identity_into(out),
            CurveKind::Constant => copy_prefix(out, &self.controls),
            CurveKind::Spline => {
                let mut tmp = vec![0.0f32; self.dim];
                eval_spline(
                    &self.knots,
                    &self.controls,
                    self.dim,
                    self.degree,
                    t,
                    &mut tmp,
                );
                copy_prefix(out, &tmp);
            }
        }
    }
}

fn copy_prefix(out: &mut [f32], src: &[f32]) {
    out.fill(0.0);
    let n = out.len().min(src.len());
    out[..n].copy_from_slice(&src[..n]);
}

/// Flips quaternions so consecutive controls lie in one hemisphere (q and -q are the same
/// rotation, but blending them across the sign change would collapse the result).
fn make_quaternions_continuous(controls: &mut [f32]) {
    let mut prev: Option<[f32; 4]> = None;
    for q in controls.chunks_exact_mut(4) {
        if let Some(p) = prev {
            let dot: f32 = p.iter().zip(q.iter()).map(|(a, b)| a * b).sum();
            if dot < 0.0 {
                for x in q.iter_mut() {
                    *x = -*x;
                }
            }
        }
        prev = Some([q[0], q[1], q[2], q[3]]);
    }
}

/// Identity value for a vector of `out.len()` components: quaternion (4), 3x3 matrix (9) or zero.
fn identity_into(out: &mut [f32]) {
    out.fill(0.0);
    match out.len() {
        4 => out[3] = 1.0,
        9 => {
            out[0] = 1.0;
            out[4] = 1.0;
            out[8] = 1.0;
        }
        _ => {}
    }
}

/// Uniform-knot-vector B-spline evaluation (see `knot_vector`).
fn eval_spline(knots: &[f32], controls: &[f32], dim: usize, degree: u8, t: f32, out: &mut [f32]) {
    let n = knots.len();
    out.fill(0.0);
    if n == 0 {
        return;
    }
    let d = usize::from(degree).min(n - 1);
    if n == 1 || d == 0 {
        // Piecewise constant: last control whose knot is <= t.
        let i = knots.partition_point(|&k| k <= t).saturating_sub(1);
        out.copy_from_slice(&controls[i * dim..(i + 1) * dim]);
        return;
    }
    let kv = knot_vector(knots, d);
    // Span: kv[s] <= t < kv[s+1], d <= s < n.
    let lo = kv[d];
    let hi = kv[n];
    let t = t.clamp(lo, hi);
    let mut s = d;
    while s + 1 < n && kv[s + 1] <= t {
        s += 1;
    }
    // de Boor
    let mut pts: Vec<f32> = (0..=d)
        .flat_map(|j| {
            controls[(s - d + j) * dim..(s - d + j + 1) * dim]
                .iter()
                .copied()
        })
        .collect();
    for r in 1..=d {
        for j in (r..=d).rev() {
            let i = s - d + j;
            let den = kv[i + d - r + 1] - kv[i];
            let a = if den.abs() > 1e-20 {
                (t - kv[i]) / den
            } else {
                0.0
            };
            for c in 0..dim {
                let lo_v = pts[(j - 1) * dim + c];
                let hi_v = pts[j * dim + c];
                pts[j * dim + c] = (1.0 - a) * lo_v + a * hi_v;
            }
        }
    }
    out.copy_from_slice(&pts[d * dim..(d + 1) * dim]);
}

/// Full clamped B-spline knot vector (`n + d + 1` entries) from Granny's `n` stored knots.
///
/// Granny stores the first knot `d` times and the last once; the implied vector has one more
/// leading copy and `d` more trailing copies, giving `d + 1` equal knots at both ends.
fn knot_vector(knots: &[f32], d: usize) -> Vec<f32> {
    let n = knots.len();
    let mut kv = Vec::with_capacity(n + d + 1);
    kv.push(knots[0]);
    kv.extend_from_slice(knots);
    kv.extend(std::iter::repeat_n(knots[n - 1], d));
    kv
}

/// Convenience for reporting: finite check over every control and knot.
impl Curve {
    pub fn is_finite(&self) -> bool {
        self.knots
            .iter()
            .chain(&self.controls)
            .all(|x| x.is_finite())
    }
}

fn array_f32(o: &Object<'_>, name: &str) -> Result<Vec<f32>, GrannyError> {
    match o.get(name)? {
        Value::Fixed(f) => Ok(f
            .iter()
            .filter_map(|v| {
                if let Value::Real32(x) = v {
                    Some(x)
                } else {
                    None
                }
            })
            .collect()),
        Value::Array(a) => {
            let d = a.resolve()?;
            Ok(d.bytes()
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect())
        }
        Value::Real32(x) => Ok(vec![x]),
        v => Err(GrannyError::TypeMismatch {
            field: name.to_owned(),
            expected: "real32 array",
            found: v.kind_name(),
        }),
    }
}

/// `KnotsControls`: an array of UInt8, UInt16 or Real32 elements, one scalar each.
fn array_packed(o: &Object<'_>, name: &str) -> Result<Packed, GrannyError> {
    let Value::Array(a) = o.get(name)? else {
        return Err(bad("KnotsControls is not an array"));
    };
    let d = a.resolve()?;
    let Some(ty) = d.element_type() else {
        return Ok(Packed::U8(Vec::new()));
    };
    let kind = ty.members.first().map(|m| m.kind);
    let b = d.bytes();
    use crate::MemberKind::*;
    match kind {
        Some(UInt8) => Ok(Packed::U8(b.to_vec())),
        Some(UInt16) => Ok(Packed::U16(
            b.chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        )),
        Some(Real32) => Ok(Packed::F32(
            b.chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )),
        _ => Err(bad("unexpected KnotsControls element type")),
    }
}

fn opt<T>(r: Result<T, GrannyError>) -> Result<Option<T>, GrannyError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(GrannyError::MissingField(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Reads the raw fields of a curve data object by member name.
pub fn read_parts(o: &Object<'_>) -> Result<CurveParts, GrannyError> {
    let header = o
        .fields()
        .find_map(|(_, v)| match v {
            Value::Inline(h) if h.field("Format").is_some() => Some(h),
            _ => None,
        })
        .ok_or_else(|| bad("curve data has no header"))?;
    let format = u8::try_from(header.u32("Format")?).map_err(|_| bad("format out of range"))?;
    let degree = u8::try_from(header.u32("Degree")?).map_err(|_| bad("degree out of range"))?;
    let mut p = CurveParts::new(format, degree);
    if let Some(d) = opt(o.i32("Dimension"))? {
        p.dimension = d;
    }
    if let Some(t) = opt(o.u32("OneOverKnotScaleTrunc"))? {
        p.one_over_knot_scale_trunc = t as u16;
    }
    if let Some(s) = opt(o.f32("OneOverKnotScale"))? {
        p.one_over_knot_scale = s;
    }
    if let Some(e) = opt(o.u32("ScaleOffsetTableEntries"))? {
        p.scale_offset_table_entries = e as u16;
    }
    if let Some(v) = opt(array_f32(o, "ControlScaleOffsets"))? {
        p.control_scales = v;
    } else if let Some(v) = opt(array_f32(o, "ControlScales"))? {
        p.control_scales = v;
    } else if let Some(v) = opt(array_f32(o, "ControlScale"))? {
        p.control_scales = v;
    }
    if let Some(v) = opt(array_f32(o, "ControlOffsets"))? {
        p.control_offsets = v;
    } else if let Some(v) = opt(array_f32(o, "ControlOffset"))? {
        p.control_offsets = v;
    }
    if let Some(v) = opt(array_f32(o, "Controls"))? {
        p.controls = v;
    }
    if let Some(v) = opt(array_f32(o, "Knots"))? {
        p.knots = v;
    }
    if let Some(v) = opt(array_packed(o, "KnotsControls"))? {
        p.knots_controls = v;
    }
    Ok(p)
}
