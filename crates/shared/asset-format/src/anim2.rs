//! `C5A2`: the clips of one model as keys with times.
//!
//! A clip keeps, per bone, a channel for rotation, translation and scale. A channel is the rest
//! pose, one constant value, or keys at moments of the clip: between two keys the value is
//! interpolated linearly (spherically for rotations, by the short way). The converter keeps a key
//! only where the interpolation of its neighbours would be off by more than a tolerance.
//!
//! Reading checks every length, the key count and the times before anything is allocated, so a
//! hostile blob is an error and never a panic or a huge allocation.

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::{FormatError, Transform, blob, quat_unpack};

pub const ANIM_PACK_MAGIC: [u8; 4] = *b"C5A2";

/// Bytes of one rotation key: snorm16 x4, xyzw.
pub const ROT_STRIDE: usize = 8;
/// Bytes of one translation or scale key: f32 x3.
pub const VEC_STRIDE: usize = 12;

/// Value of one channel of a track.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Channel {
    /// The rest pose of the skeleton.
    Rest,
    /// One value, little endian (8 bytes for a rotation, 12 for a vector).
    Const(Vec<u8>),
    /// `count >= 2` keys: `times` are `u16` LE, in 1/65535 of the clip duration, not decreasing;
    /// `values` are `count` values as in [`Channel::Const`].
    Keys {
        count: u32,
        times: Vec<u8>,
        values: Vec<u8>,
    },
}

/// The three channels of one bone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackKeys {
    pub bone: u16,
    pub rot: Channel,
    pub pos: Channel,
    pub scale: Channel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Label of the fxsxml (`Warrior_IdleA`).
    pub name: String,
    /// Seconds.
    pub duration: f32,
    /// By bone, strictly increasing.
    pub tracks: Vec<TrackKeys>,
}

/// All clips of one model (`an:<model>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnimPack {
    pub clips: Vec<Clip>,
}

impl Channel {
    fn check(&self, stride: usize, what: &str) -> Result<(), FormatError> {
        match self {
            Channel::Rest => Ok(()),
            Channel::Const(v) => {
                if v.len() != stride {
                    return Err(FormatError::invalid(format!(
                        "{what}: a constant is {} bytes, expected {stride}",
                        v.len()
                    )));
                }
                check_finite(v, stride, what)
            }
            Channel::Keys {
                count,
                times,
                values,
            } => {
                let count = *count as usize;
                if count < 2 {
                    return Err(FormatError::invalid(format!(
                        "{what}: fewer than two keys (a constant is a Const)"
                    )));
                }
                let times_len = count.checked_mul(2);
                let values_len = count.checked_mul(stride);
                if times_len != Some(times.len()) || values_len != Some(values.len()) {
                    return Err(FormatError::invalid(format!(
                        "{what}: {count} keys do not match {} time bytes and {} value bytes",
                        times.len(),
                        values.len()
                    )));
                }
                let mut prev = 0u16;
                for t in times.chunks_exact(2) {
                    let t = u16::from_le_bytes([t[0], t[1]]);
                    if t < prev {
                        return Err(FormatError::invalid(format!("{what}: key times decrease")));
                    }
                    prev = t;
                }
                check_finite(values, stride, what)
            }
        }
    }

    /// Number of keys: 0 for the rest pose, 1 for a constant.
    pub fn key_count(&self) -> usize {
        match self {
            Channel::Rest => 0,
            Channel::Const(_) => 1,
            Channel::Keys { count, .. } => *count as usize,
        }
    }
}

fn check_finite(bytes: &[u8], stride: usize, what: &str) -> Result<(), FormatError> {
    // Rotations are integers; only vectors can hold a NaN or an infinity.
    if stride == VEC_STRIDE
        && bytes
            .chunks_exact(4)
            .any(|c| !f32::from_le_bytes([c[0], c[1], c[2], c[3]]).is_finite())
    {
        return Err(FormatError::invalid(format!(
            "{what}: a value is not finite"
        )));
    }
    Ok(())
}

fn rot_at(values: &[u8], i: usize) -> [f32; 4] {
    let b = &values[i * ROT_STRIDE..(i + 1) * ROT_STRIDE];
    quat_unpack([
        i16::from_le_bytes([b[0], b[1]]),
        i16::from_le_bytes([b[2], b[3]]),
        i16::from_le_bytes([b[4], b[5]]),
        i16::from_le_bytes([b[6], b[7]]),
    ])
}

fn vec_at(values: &[u8], i: usize) -> [f32; 3] {
    let b = &values[i * VEC_STRIDE..(i + 1) * VEC_STRIDE];
    std::array::from_fn(|k| {
        f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
    })
}

fn time_at(times: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([times[2 * i], times[2 * i + 1]])
}

/// Keys `i0`, `i1` around `u` (in 1/65535 of the duration) and the blend factor.
fn locate(times: &[u8], count: usize, u: f32) -> (usize, usize, f32) {
    let last = count - 1;
    if u <= f32::from(time_at(times, 0)) {
        return (0, 0, 0.0);
    }
    if u >= f32::from(time_at(times, last)) {
        return (last, last, 0.0);
    }
    // First key with time > u.
    let (mut lo, mut hi) = (0usize, last);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        if f32::from(time_at(times, mid)) <= u {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let (t0, t1) = (f32::from(time_at(times, lo)), f32::from(time_at(times, hi)));
    let f = if t1 > t0 { (u - t0) / (t1 - t0) } else { 1.0 };
    (lo, hi, f)
}

fn sample_rot(c: &Channel, u: f32, out: &mut [f32; 4]) {
    match c {
        Channel::Rest => {}
        Channel::Const(v) => *out = rot_at(v, 0),
        Channel::Keys {
            count,
            times,
            values,
        } => {
            let (i0, i1, f) = locate(times, *count as usize, u);
            let a = Quat::from_array(rot_at(values, i0));
            if i0 == i1 {
                *out = a.to_array();
                return;
            }
            let mut b = Quat::from_array(rot_at(values, i1));
            if a.dot(b) < 0.0 {
                b = -b;
            }
            *out = a.slerp(b, f).normalize().to_array();
        }
    }
}

fn sample_vec(c: &Channel, u: f32, out: &mut [f32; 3]) {
    match c {
        Channel::Rest => {}
        Channel::Const(v) => *out = vec_at(v, 0),
        Channel::Keys {
            count,
            times,
            values,
        } => {
            let (i0, i1, f) = locate(times, *count as usize, u);
            *out = Vec3::from_array(vec_at(values, i0))
                .lerp(Vec3::from_array(vec_at(values, i1)), f)
                .to_array();
        }
    }
}

impl Clip {
    /// Writes `min(rest, out)` bones: the rest pose, replaced by what the clip says. `t` seconds
    /// are clamped to `[0, duration]` (a looping caller wraps `t` itself). Tracks of bones outside
    /// `out` are ignored.
    pub fn sample(&self, t: f32, rest: &[Transform], out: &mut [Transform]) {
        let n = rest.len().min(out.len());
        out[..n].copy_from_slice(&rest[..n]);
        let u = if t.is_finite() && self.duration > 0.0 {
            (t / self.duration).clamp(0.0, 1.0) * 65535.0
        } else {
            0.0
        };
        for track in &self.tracks {
            let Some(o) = out[..n].get_mut(usize::from(track.bone)) else {
                continue;
            };
            sample_rot(&track.rot, u, &mut o.rotation);
            sample_vec(&track.pos, u, &mut o.translation);
            sample_vec(&track.scale, u, &mut o.scale);
        }
    }

    /// Number of keys over all channels (what the size comes from).
    pub fn key_count(&self) -> usize {
        self.tracks
            .iter()
            .map(|t| t.rot.key_count() + t.pos.key_count() + t.scale.key_count())
            .sum()
    }

    fn check(&self) -> Result<(), FormatError> {
        if !self.duration.is_finite() || self.duration < 0.0 {
            return Err(FormatError::invalid(format!(
                "clip {:?}: duration must be finite and >= 0",
                self.name
            )));
        }
        let mut prev: Option<u16> = None;
        for t in &self.tracks {
            if prev.is_some_and(|p| t.bone <= p) {
                return Err(FormatError::invalid(format!(
                    "clip {:?}: tracks are not in increasing bone order",
                    self.name
                )));
            }
            prev = Some(t.bone);
            let what = format!("clip {:?} bone {}", self.name, t.bone);
            t.rot.check(ROT_STRIDE, &format!("{what} rotation"))?;
            t.pos.check(VEC_STRIDE, &format!("{what} translation"))?;
            t.scale.check(VEC_STRIDE, &format!("{what} scale"))?;
        }
        Ok(())
    }
}

impl AnimPack {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(ANIM_PACK_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, p): (u16, Self) =
            blob::decode_range(ANIM_PACK_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        for c in &p.clips {
            c.check()?;
        }
        Ok(p)
    }

    /// Longest bone index of any track plus one (a skeleton must have at least that many bones).
    pub fn bone_span(&self) -> usize {
        self.clips
            .iter()
            .flat_map(|c| c.tracks.iter())
            .map(|t| usize::from(t.bone) + 1)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quat_pack;

    fn f32s(v: &[[f32; 3]]) -> Vec<u8> {
        v.iter()
            .flat_map(|p| p.iter().flat_map(|c| c.to_le_bytes()))
            .collect()
    }

    fn quats(v: &[Quat]) -> Vec<u8> {
        v.iter()
            .flat_map(|q| {
                quat_pack(q.to_array())
                    .into_iter()
                    .flat_map(i16::to_le_bytes)
            })
            .collect()
    }

    fn times(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|t| t.to_le_bytes()).collect()
    }

    fn rest(n: usize) -> Vec<Transform> {
        vec![
            Transform {
                translation: [9.0, 9.0, 9.0],
                ..Transform::IDENTITY
            };
            n
        ]
    }

    fn clip() -> Clip {
        Clip {
            name: "walk".into(),
            duration: 2.0,
            tracks: vec![
                TrackKeys {
                    bone: 1,
                    rot: Channel::Keys {
                        count: 3,
                        times: times(&[0, 32768, 65535]),
                        values: quats(&[
                            Quat::IDENTITY,
                            Quat::from_rotation_z(90f32.to_radians()),
                            Quat::from_rotation_z(90f32.to_radians()),
                        ]),
                    },
                    pos: Channel::Keys {
                        count: 2,
                        times: times(&[0, 65535]),
                        values: f32s(&[[0.0; 3], [4.0, 0.0, 0.0]]),
                    },
                    scale: Channel::Const(f32s(&[[2.0; 3]])),
                },
                TrackKeys {
                    bone: 3,
                    rot: Channel::Rest,
                    pos: Channel::Rest,
                    scale: Channel::Rest,
                },
            ],
        }
    }

    fn sample(c: &Clip, t: f32) -> Vec<Transform> {
        let r = rest(4);
        let mut out = vec![Transform::IDENTITY; 4];
        c.sample(t, &r, &mut out);
        out
    }

    #[test]
    fn roundtrip_is_deterministic() {
        let p = AnimPack {
            clips: vec![clip()],
        };
        let a = p.encode();
        assert_eq!(a, p.encode());
        assert_eq!(AnimPack::decode(&a).unwrap(), p);
        assert_eq!(p.bone_span(), 4);
    }

    #[test]
    fn samples_between_keys() {
        let c = clip();
        let o = sample(&c, 1.0);
        assert!((o[1].translation[0] - 2.0).abs() < 1e-3);
        assert_eq!(o[1].scale, [2.0; 3]);
        let q = Quat::from_array(o[1].rotation);
        assert!(q.dot(Quat::from_rotation_z(90f32.to_radians())).abs() > 0.9999);
        // A quarter of the clip: halfway to the second key.
        let q = Quat::from_array(sample(&c, 0.5)[1].rotation);
        assert!(q.dot(Quat::from_rotation_z(45f32.to_radians())).abs() > 0.9999);
    }

    #[test]
    fn rest_and_untracked_bones_keep_rest() {
        let o = sample(&clip(), 0.7);
        assert_eq!(o[0], rest(1)[0]);
        assert_eq!(o[2], rest(1)[0]);
        assert_eq!(o[3], rest(1)[0]);
    }

    #[test]
    fn time_clamps_and_bad_time_is_start() {
        let c = clip();
        assert_eq!(sample(&c, 10.0)[1].translation, [4.0, 0.0, 0.0]);
        assert_eq!(sample(&c, -1.0)[1].translation, [0.0; 3]);
        assert_eq!(sample(&c, f32::NAN)[1].translation, [0.0; 3]);
    }

    #[test]
    fn rotation_takes_the_short_way() {
        let q0 = Quat::from_rotation_z(20f32.to_radians());
        let q1 = -Quat::from_rotation_z((-20f32).to_radians());
        let c = Clip {
            name: "x".into(),
            duration: 1.0,
            tracks: vec![TrackKeys {
                bone: 0,
                rot: Channel::Keys {
                    count: 2,
                    times: times(&[0, 65535]),
                    values: quats(&[q0, q1]),
                },
                pos: Channel::Rest,
                scale: Channel::Rest,
            }],
        };
        let q = Quat::from_array(sample(&c, 0.5)[0].rotation);
        assert!(q.dot(Quat::IDENTITY).abs() > 0.9999, "{q:?}");
    }

    #[test]
    fn zero_duration_clip_is_its_first_key() {
        let mut c = clip();
        c.duration = 0.0;
        assert_eq!(sample(&c, 5.0)[1].translation, [0.0; 3]);
    }

    #[test]
    fn equal_times_do_not_divide_by_zero() {
        let c = Clip {
            name: "step".into(),
            duration: 1.0,
            tracks: vec![TrackKeys {
                bone: 0,
                rot: Channel::Rest,
                pos: Channel::Keys {
                    count: 3,
                    times: times(&[0, 30000, 30000]),
                    values: f32s(&[[0.0; 3], [1.0, 0.0, 0.0], [5.0, 0.0, 0.0]]),
                },
                scale: Channel::Rest,
            }],
        };
        for t in [0.2, 30000.0 / 65535.0, 0.9] {
            assert!(sample(&c, t)[0].translation[0].is_finite());
        }
        assert_eq!(sample(&c, 0.9)[0].translation[0], 5.0);
    }

    fn bad(f: impl FnOnce(&mut Clip)) -> Result<AnimPack, FormatError> {
        let mut c = clip();
        f(&mut c);
        AnimPack::decode(&AnimPack { clips: vec![c] }.encode())
    }

    #[test]
    fn hostile_blobs_are_errors() {
        assert!(bad(|c| c.duration = f32::NAN).is_err());
        assert!(bad(|c| c.duration = -1.0).is_err());
        assert!(bad(|c| c.tracks[1].bone = 1).is_err());
        assert!(bad(|c| c.tracks.swap(0, 1)).is_err());
        assert!(bad(|c| c.tracks[0].scale = Channel::Const(vec![0; 5])).is_err());
        assert!(bad(|c| c.tracks[0].pos = Channel::Const(f32s(&[[f32::NAN; 3]]))).is_err());
        // A count that does not match the bytes, and one that would overflow a multiplication.
        for count in [3u32, 1, 0, u32::MAX] {
            assert!(
                bad(|c| {
                    c.tracks[0].pos = Channel::Keys {
                        count,
                        times: times(&[0, 65535]),
                        values: f32s(&[[0.0; 3], [4.0, 0.0, 0.0]]),
                    }
                })
                .is_err(),
                "count {count}"
            );
        }
        assert!(
            bad(|c| {
                c.tracks[0].pos = Channel::Keys {
                    count: 2,
                    times: times(&[500, 400]),
                    values: f32s(&[[0.0; 3], [4.0, 0.0, 0.0]]),
                }
            })
            .is_err()
        );
        let good = AnimPack {
            clips: vec![clip()],
        }
        .encode();
        for cut in 0..good.len() {
            assert!(AnimPack::decode(&good[..cut]).is_err(), "cut {cut}");
        }
        let mut wrong = good;
        wrong[4] = 9;
        assert!(matches!(
            AnimPack::decode(&wrong),
            Err(FormatError::UnsupportedVersion { .. })
        ));
    }
}
