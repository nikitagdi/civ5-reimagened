//! Keys with a tolerance (`C5A2`): the 30 Hz frames of a reference sampler are reduced to the
//! few nodes after which the interpolation stays within the tolerance.
//!
//! Independent of Granny: the caller supplies the frames of every bone (the reference), so the
//! reduction is testable with hand-made tracks. After the reduction the packed clip is sampled
//! with [`Clip::sample`] on the same frames and compared with the reference; a clip that is off
//! by more than the tolerance (time quantization, packed rotations) is packed again with a
//! tighter working tolerance.

use asset_format::{Channel, Clip, ROT_STRIDE, TrackKeys, Transform, VEC_STRIDE, quat_pack};
use glam::Quat;

use crate::models::resample::{SAMPLE_RATE, key_count, key_time};

/// What the packed clip may differ by from the reference at the frames of the 30 Hz grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tolerance {
    /// Angle between two rotations, degrees.
    pub rot_deg: f64,
    /// Translation difference, percent of the model size.
    pub pos_percent: f64,
    /// Scale difference, percent of the scale.
    pub scale_percent: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        Self {
            rot_deg: super::DEFAULT_ROT_DEG,
            pos_percent: 0.05,
            scale_percent: 0.1,
        }
    }
}

impl Tolerance {
    fn scaled(&self, k: f64) -> Self {
        Self {
            rot_deg: self.rot_deg * k,
            pos_percent: self.pos_percent * k,
            scale_percent: self.scale_percent * k,
        }
    }
}

/// Largest differences found, in the units of [`Tolerance`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Deviation {
    pub rot_deg: f64,
    pub pos_percent: f64,
    pub scale_percent: f64,
}

impl Deviation {
    pub fn within(&self, t: &Tolerance) -> bool {
        self.rot_deg <= t.rot_deg
            && self.pos_percent <= t.pos_percent
            && self.scale_percent <= t.scale_percent
    }

    pub fn max_with(&mut self, o: &Deviation) {
        self.rot_deg = self.rot_deg.max(o.rot_deg);
        self.pos_percent = self.pos_percent.max(o.pos_percent);
        self.scale_percent = self.scale_percent.max(o.scale_percent);
    }
}

/// Reference frames of one bone, in engine convention.
#[derive(Debug, Clone, PartialEq)]
pub struct RefTrack {
    pub bone: u16,
    pub rot: Vec<[f32; 4]>,
    pub pos: Vec<[f32; 3]>,
    pub scale: Vec<[f32; 3]>,
}

/// A reference clip: `frames` frames at [`frame_time`].
#[derive(Debug, Clone, PartialEq)]
pub struct RefClip {
    pub name: String,
    pub duration: f32,
    /// By bone, strictly increasing.
    pub tracks: Vec<RefTrack>,
}

impl RefClip {
    pub fn frames(&self) -> usize {
        key_count(self.duration, SAMPLE_RATE)
    }

    /// Time of frame `i`: `i / 30` s, the last frame on the clip end.
    pub fn frame_time(&self, i: usize) -> f32 {
        key_time(i, self.duration, SAMPLE_RATE)
    }
}

// ---------------------------------------------------------------- errors

/// Angle in degrees between two rotations (xyzw), insensitive to the sign of a quaternion.
fn rot_err(a: &[f32; 4], b: &[f32; 4]) -> f64 {
    let a = a.map(f64::from);
    let b = b.map(f64::from);
    // r = a * conj(b)
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (-b[0], -b[1], -b[2], b[3]);
    let w = aw * bw - ax * bx - ay * by - az * bz;
    let x = aw * bx + ax * bw + ay * bz - az * by;
    let y = aw * by - ax * bz + ay * bw + az * bx;
    let z = aw * bz + ax * by - ay * bx + az * bw;
    let v = (x * x + y * y + z * z).sqrt();
    (2.0 * v.atan2(w.abs())).to_degrees()
}

fn pos_err(a: &[f32; 3], b: &[f32; 3], size: f64) -> f64 {
    let d: f64 = (0..3)
        .map(|k| {
            let x = f64::from(a[k]) - f64::from(b[k]);
            x * x
        })
        .sum::<f64>()
        .sqrt();
    100.0 * d / size
}

fn scale_err(a: &[f32; 3], b: &[f32; 3]) -> f64 {
    (0..3)
        .map(|k| {
            let (x, y) = (f64::from(a[k]), f64::from(b[k]));
            100.0 * (x - y).abs() / x.abs().max(y.abs()).max(1e-6)
        })
        .fold(0.0, f64::max)
}

// ---------------------------------------------------------------- reduction

/// How many frames past the last feasible one a segment may still look for a longer feasible
/// span (the error of an interpolation does not grow steadily with the span).
const LOOK_PAST: usize = 6;

/// A clip with more frames than this (15 s) is a long one: its segments are at most
/// [`LONG_CLIP_SPAN`] frames, because checking a segment costs its length and the number of
/// segments to check grows with it (the 48 s breathing clips of the landmarks took minutes). A
/// shorter segment never loosens the tolerance; it only keeps a node where the clip is smooth.
const LONG_CLIP_FRAMES: usize = 450;
const LONG_CLIP_SPAN: usize = 120;

/// The fewest frames that must stay as nodes (the end frames are always nodes) so that the
/// interpolation between neighbouring nodes is within `tol` of every frame in between. A shortest
/// path over the frames: an edge `a -> b` exists when the interpolation of `a` and `b` fits.
fn select_nodes<V>(
    vals: &[V],
    u: &[f64],
    tol: f64,
    err: &impl Fn(&V, &V) -> f64,
    lerp: &impl Fn(&V, &V, f64) -> V,
) -> Vec<usize> {
    let n = vals.len();
    // A node sits at its frame time rounded to 1/65535 of the clip; the frames are sampled at
    // their exact times, so a fast move is off by its speed times the rounding. The check uses
    // the rounded node times, and the end frames count when the sample falls inside the segment.
    let uq: Vec<f64> = u
        .iter()
        .map(|&x| f64::from(quantize_time(x)) / 65535.0)
        .collect();
    let fits = |a: usize, b: usize| -> bool {
        let span = uq[b] - uq[a];
        (a..=b).all(|j| {
            let f = if span > 0.0 {
                (u[j] - uq[a]) / span
            } else {
                1.0
            };
            if (j == a && f < 0.0) || (j == b && f > 1.0) {
                return true;
            }
            err(&lerp(&vals[a], &vals[b], f.clamp(0.0, 1.0)), &vals[j]) <= tol
        })
    };
    // best[b]: fewest segments from frame 0 to b; from[b]: the frame before b on that path.
    let max_span = if n > LONG_CLIP_FRAMES {
        LONG_CLIP_SPAN
    } else {
        usize::MAX
    };
    let mut best = vec![usize::MAX; n];
    let mut from = vec![0usize; n];
    best[0] = 0;
    for a in 0..n - 1 {
        if best[a] == usize::MAX {
            continue;
        }
        let mut misses = 0;
        let mut b = a + 1;
        while b < n && misses <= LOOK_PAST && b - a <= max_span {
            if b == a + 1 || fits(a, b) {
                misses = 0;
                // Later start of the last segment wins a tie: it is the longer one.
                if best[a] < best[b] {
                    best[b] = best[a] + 1;
                    from[b] = a;
                }
            } else {
                misses += 1;
            }
            b += 1;
        }
    }
    let mut nodes = vec![n - 1];
    while let Some(&last) = nodes.last() {
        if last == 0 {
            break;
        }
        nodes.push(from[last]);
    }
    nodes.reverse();
    nodes
}

fn quantize_time(u: f64) -> u16 {
    (u.clamp(0.0, 1.0) * 65535.0).round() as u16
}

fn build_channel<V: Copy>(
    vals: &[V],
    u: &[f64],
    rest: V,
    tol: f64,
    err: impl Fn(&V, &V) -> f64,
    lerp: impl Fn(&V, &V, f64) -> V,
    enc: impl Fn(&V) -> Vec<u8>,
) -> Channel {
    if vals.iter().all(|v| err(v, &rest) <= tol) {
        return Channel::Rest;
    }
    if vals.iter().all(|v| err(v, &vals[0]) <= tol) {
        return Channel::Const(enc(&vals[0]));
    }
    let nodes = select_nodes(vals, u, tol, &err, &lerp);
    let mut times = Vec::with_capacity(nodes.len() * 2);
    let mut values = Vec::new();
    for &i in &nodes {
        times.extend_from_slice(&quantize_time(u[i]).to_le_bytes());
        values.extend_from_slice(&enc(&vals[i]));
    }
    Channel::Keys {
        count: nodes.len() as u32,
        times,
        values,
    }
}

fn enc_rot(q: &[f32; 4]) -> Vec<u8> {
    let p = quat_pack(*q);
    let mut v = Vec::with_capacity(ROT_STRIDE);
    for c in p {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v
}

fn enc_vec(p: &[f32; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity(VEC_STRIDE);
    for c in p {
        v.extend_from_slice(&c.to_le_bytes());
    }
    v
}

/// The same rotation of every frame in the hemisphere of the previous frame, so the
/// interpolation takes the short way.
fn continuous(rot: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let mut out: Vec<[f32; 4]> = Vec::with_capacity(rot.len());
    for q in rot {
        let mut q = *q;
        if let Some(p) = out.last()
            && p.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>() < 0.0
        {
            q = q.map(|c| -c);
        }
        out.push(q);
    }
    out
}

fn reduce_track(
    t: &RefTrack,
    rest: &Transform,
    u: &[f64],
    size: f64,
    tol: &Tolerance,
) -> Option<TrackKeys> {
    let rot = continuous(&t.rot);
    let rot = build_channel(
        &rot,
        u,
        rest.rotation,
        tol.rot_deg,
        rot_err,
        |a, b, f| {
            let (qa, qb) = (Quat::from_array(*a), Quat::from_array(*b));
            let qb = if qa.dot(qb) < 0.0 { -qb } else { qb };
            qa.slerp(qb, f as f32).normalize().to_array()
        },
        enc_rot,
    );
    let lerp3 = |a: &[f32; 3], b: &[f32; 3], f: f64| -> [f32; 3] {
        std::array::from_fn(|k| (f64::from(a[k]) + (f64::from(b[k]) - f64::from(a[k])) * f) as f32)
    };
    let pos = build_channel(
        &t.pos,
        u,
        rest.translation,
        tol.pos_percent,
        |a, b| pos_err(a, b, size),
        lerp3,
        enc_vec,
    );
    let scale = build_channel(
        &t.scale,
        u,
        rest.scale,
        tol.scale_percent,
        scale_err,
        lerp3,
        enc_vec,
    );
    if rot == Channel::Rest && pos == Channel::Rest && scale == Channel::Rest {
        return None;
    }
    Some(TrackKeys {
        bone: t.bone,
        rot,
        pos,
        scale,
    })
}

/// Positions of the frames in the clip, `0..=1`.
fn frame_fractions(c: &RefClip) -> Vec<f64> {
    (0..c.frames())
        .map(|i| {
            if c.duration > 0.0 {
                f64::from(c.frame_time(i)) / f64::from(c.duration)
            } else {
                0.0
            }
        })
        .collect()
}

/// A clip with the frames of `reference` reduced to nodes under `tol`, and how far it is from the
/// reference at the frames. `size` is the model size the translation tolerance is a percent of.
pub fn pack_clip(
    reference: &RefClip,
    rest: &[Transform],
    size: f32,
    tol: &Tolerance,
) -> (Clip, Deviation) {
    let size = if size.is_finite() && size > 0.0 {
        f64::from(size)
    } else {
        1.0
    };
    let u = frame_fractions(reference);
    let mut last = None;
    // The working tolerance leaves room for what packing adds (snorm16 rotations, u16 times).
    for k in [0.9, 0.7, 0.5, 0.25, 0.1] {
        let work = tol.scaled(k);
        let tracks: Vec<TrackKeys> = reference
            .tracks
            .iter()
            .filter_map(|t| {
                let r = rest.get(usize::from(t.bone))?;
                reduce_track(t, r, &u, size, &work)
            })
            .collect();
        let clip = Clip {
            name: reference.name.clone(),
            duration: reference.duration,
            tracks,
        };
        let dev = deviation(&clip, reference, rest, size as f32);
        let ok = dev.within(tol);
        last = Some((clip, dev));
        if ok {
            break;
        }
    }
    last.expect("the loop runs")
}

/// Largest difference between `clip` sampled with [`Clip::sample`] and the reference, over the
/// frames of the grid and every bone of the reference.
pub fn deviation(clip: &Clip, reference: &RefClip, rest: &[Transform], size: f32) -> Deviation {
    let size = if size.is_finite() && size > 0.0 {
        f64::from(size)
    } else {
        1.0
    };
    let mut dev = Deviation::default();
    let mut out = rest.to_vec();
    for i in 0..reference.frames() {
        clip.sample(reference.frame_time(i), rest, &mut out);
        for t in &reference.tracks {
            let Some(o) = out.get(usize::from(t.bone)) else {
                continue;
            };
            dev.max_with(&Deviation {
                rot_deg: rot_err(&o.rotation, &t.rot[i]),
                pos_percent: pos_err(&o.translation, &t.pos[i], size),
                scale_percent: scale_err(&o.scale, &t.scale[i]),
            });
        }
    }
    dev
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rest() -> Vec<Transform> {
        vec![Transform::IDENTITY; 3]
    }

    /// A reference clip of one bone built from `f(time)`.
    fn reference(
        duration: f32,
        bone: u16,
        f: impl Fn(f32) -> ([f32; 4], [f32; 3], [f32; 3]),
    ) -> RefClip {
        let mut c = RefClip {
            name: "t".into(),
            duration,
            tracks: vec![RefTrack {
                bone,
                rot: vec![],
                pos: vec![],
                scale: vec![],
            }],
        };
        for i in 0..c.frames() {
            let (r, p, s) = f(c.frame_time(i));
            c.tracks[0].rot.push(r);
            c.tracks[0].pos.push(p);
            c.tracks[0].scale.push(s);
        }
        c
    }

    fn rz(deg: f32) -> [f32; 4] {
        Quat::from_rotation_z(deg.to_radians()).to_array()
    }

    #[test]
    fn rest_everywhere_is_no_track() {
        let c = reference(1.0, 1, |_| ([0.0, 0.0, 0.0, 1.0], [0.0; 3], [1.0; 3]));
        let (clip, dev) = pack_clip(&c, &rest(), 2.0, &Tolerance::default());
        assert!(clip.tracks.is_empty());
        assert!(dev.within(&Tolerance::default()));
    }

    #[test]
    fn a_constant_pose_is_one_value() {
        let c = reference(1.0, 1, |_| (rz(30.0), [1.0, 2.0, 3.0], [1.0; 3]));
        let (clip, _) = pack_clip(&c, &rest(), 10.0, &Tolerance::default());
        let t = &clip.tracks[0];
        assert!(matches!(t.rot, Channel::Const(_)));
        assert!(matches!(t.pos, Channel::Const(_)));
        assert_eq!(t.scale, Channel::Rest);
    }

    #[test]
    fn a_straight_line_needs_two_nodes_and_a_bend_needs_three() {
        let line = reference(1.0, 1, |t| {
            ([0.0, 0.0, 0.0, 1.0], [3.0 * t, 0.0, 0.0], [1.0; 3])
        });
        let (clip, dev) = pack_clip(&line, &rest(), 10.0, &Tolerance::default());
        assert_eq!(clip.tracks[0].pos.key_count(), 2);
        assert!(dev.within(&Tolerance::default()));
        let bend = reference(1.0, 1, |t| {
            let x = if t < 0.5 { 4.0 * t } else { 2.0 - 0.0 * t };
            ([0.0, 0.0, 0.0, 1.0], [x, 0.0, 0.0], [1.0; 3])
        });
        let (clip, dev) = pack_clip(&bend, &rest(), 10.0, &Tolerance::default());
        assert!(dev.within(&Tolerance::default()), "{dev:?}");
        assert!(clip.tracks[0].pos.key_count() >= 3);
        assert!(clip.tracks[0].pos.key_count() < 10);
    }

    #[test]
    fn a_swing_stays_within_tolerance_with_fewer_keys_than_frames() {
        let c = reference(3.0, 2, |t| {
            (
                rz(40.0 * (t * 2.0).sin()),
                [0.2 * t.cos(), 0.0, 0.1 * (3.0 * t).sin()],
                [1.0 + 0.05 * t, 1.0, 1.0],
            )
        });
        let tol = Tolerance::default();
        let (clip, dev) = pack_clip(&c, &rest(), 4.0, &tol);
        assert!(dev.within(&tol), "{dev:?}");
        assert!(
            clip.key_count() < 3 * c.frames() / 2,
            "{} keys for {} frames",
            clip.key_count(),
            c.frames()
        );
        // The packed clip, sampled again on the grid, is the measured deviation.
        let again = deviation(&clip, &c, &rest(), 4.0);
        assert_eq!(again, dev);
    }

    #[test]
    fn a_rotation_that_flips_sign_stays_on_the_short_way() {
        // The reference negates its quaternion from frame 5 on (the same rotation).
        let c = reference(0.5, 0, |t| {
            let q = rz(20.0 * t * 10.0);
            if t > 5.0 / 30.0 {
                ([-q[0], -q[1], -q[2], -q[3]], [0.0; 3], [1.0; 3])
            } else {
                (q, [0.0; 3], [1.0; 3])
            }
        });
        let tol = Tolerance::default();
        let (_, dev) = pack_clip(&c, &rest(), 1.0, &tol);
        assert!(dev.within(&tol), "{dev:?}");
    }

    #[test]
    fn a_clip_of_one_frame_and_a_clip_without_duration() {
        let c = reference(0.0, 1, |_| (rz(10.0), [1.0, 0.0, 0.0], [1.0; 3]));
        assert_eq!(c.frames(), 1);
        let (clip, dev) = pack_clip(&c, &rest(), 1.0, &Tolerance::default());
        assert!(dev.within(&Tolerance::default()));
        assert!(matches!(clip.tracks[0].rot, Channel::Const(_)));
    }

    #[test]
    fn a_bone_outside_the_skeleton_is_dropped() {
        let c = reference(1.0, 9, |t| ([0.0, 0.0, 0.0, 1.0], [t, 0.0, 0.0], [1.0; 3]));
        let (clip, _) = pack_clip(&c, &rest(), 1.0, &Tolerance::default());
        assert!(clip.tracks.is_empty());
    }

    #[test]
    fn rotation_error_is_exact_for_small_angles() {
        let e = rot_err(&rz(0.05), &rz(0.0));
        assert!((e - 0.05).abs() < 1e-4, "{e}");
        assert!((rot_err(&rz(10.0), &rz(10.0).map(|c| -c))).abs() < 1e-6);
    }
}
