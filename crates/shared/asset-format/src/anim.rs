//! Animation blob and CPU sampling.

use glam::{Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::{FormatError, Transform, blob, quat_unpack};

pub const ANIM_MAGIC: [u8; 4] = *b"C5AN";

/// Per-bone channels at `sample_rate` keys per second. One key means constant; none means rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub bone: u16,
    /// Snorm16 quaternion xyzw.
    pub rotations: Vec<[i16; 4]>,
    pub translations: Vec<[f32; 3]>,
    pub scales: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimationBlob {
    /// Seconds.
    pub duration: f32,
    /// Keys per second, 30 for Civ5 clips.
    pub sample_rate: f32,
    pub tracks: Vec<Track>,
}

/// Lower key, upper key and blend factor for time `t`.
fn key_pos(len: usize, t: f32, rate: f32) -> (usize, usize, f32) {
    let last = len - 1;
    let f = (t * rate).clamp(0.0, last as f32);
    let i0 = (f.floor() as usize).min(last);
    let i1 = (i0 + 1).min(last);
    (i0, i1, f - i0 as f32)
}

impl AnimationBlob {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(ANIM_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, a): (u16, Self) =
            blob::decode_range(ANIM_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        if !a.duration.is_finite() || a.duration < 0.0 {
            return Err(FormatError::invalid("duration must be finite and >= 0"));
        }
        if !a.sample_rate.is_finite() || a.sample_rate <= 0.0 {
            return Err(FormatError::invalid("sample_rate must be finite and > 0"));
        }
        Ok(a)
    }

    /// Writes `min(rest, out)` bones. Looping wraps `t` into `[0, duration)`, otherwise it clamps.
    pub fn sample(&self, t: f32, looping: bool, rest: &[Transform], out: &mut [Transform]) {
        let n = rest.len().min(out.len());
        out[..n].copy_from_slice(&rest[..n]);

        let t = if !t.is_finite() || self.duration <= 0.0 {
            0.0
        } else if looping {
            t.rem_euclid(self.duration)
        } else {
            t.clamp(0.0, self.duration)
        };

        for track in &self.tracks {
            let Some(o) = out[..n].get_mut(usize::from(track.bone)) else {
                continue;
            };
            let rate = self.sample_rate;
            match track.rotations.as_slice() {
                [] => {}
                [q] => o.rotation = quat_unpack(*q),
                keys => {
                    let (i0, i1, f) = key_pos(keys.len(), t, rate);
                    let a = Quat::from_array(quat_unpack(keys[i0]));
                    let mut b = Quat::from_array(quat_unpack(keys[i1]));
                    if a.dot(b) < 0.0 {
                        b = -b;
                    }
                    o.rotation = a.lerp(b, f).normalize().to_array();
                }
            }
            sample_vec3(&track.translations, t, rate, &mut o.translation);
            sample_vec3(&track.scales, t, rate, &mut o.scale);
        }
    }
}

fn sample_vec3(keys: &[[f32; 3]], t: f32, rate: f32, out: &mut [f32; 3]) {
    match keys {
        [] => {}
        [v] => *out = *v,
        keys => {
            let (i0, i1, f) = key_pos(keys.len(), t, rate);
            *out = Vec3::from_array(keys[i0])
                .lerp(Vec3::from_array(keys[i1]), f)
                .to_array();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quat_pack;

    fn rest(n: usize) -> Vec<Transform> {
        vec![
            Transform {
                translation: [9.0, 9.0, 9.0],
                ..Transform::IDENTITY
            };
            n
        ]
    }

    fn translate_clip() -> AnimationBlob {
        // Keys at 0, 1/30 .. : x = 0, 3, 6 over 2/30 s.
        AnimationBlob {
            duration: 2.0 / 30.0,
            sample_rate: 30.0,
            tracks: vec![Track {
                bone: 1,
                rotations: vec![],
                translations: vec![[0.0; 3], [3.0, 0.0, 0.0], [6.0, 0.0, 0.0]],
                scales: vec![],
            }],
        }
    }

    fn sample(a: &AnimationBlob, t: f32, looping: bool) -> Vec<Transform> {
        let r = rest(3);
        let mut out = vec![Transform::IDENTITY; 3];
        a.sample(t, looping, &r, &mut out);
        out
    }

    #[test]
    fn roundtrip_and_rejects() {
        let a = translate_clip();
        let bytes = a.encode();
        assert_eq!(AnimationBlob::decode(&bytes).unwrap(), a);
        let mut bad = bytes.clone();
        bad[1] = b'x';
        assert!(matches!(
            AnimationBlob::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut bad = bytes;
        bad[5] = 9;
        assert!(matches!(
            AnimationBlob::decode(&bad),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        let zero_rate = AnimationBlob {
            sample_rate: 0.0,
            ..a
        };
        assert!(AnimationBlob::decode(&zero_rate.encode()).is_err());
    }

    #[test]
    fn endpoints_and_midpoint() {
        let a = translate_clip();
        assert_eq!(sample(&a, 0.0, false)[1].translation, [0.0; 3]);
        assert_eq!(
            sample(&a, a.duration, false)[1].translation,
            [6.0, 0.0, 0.0]
        );
        let mid = sample(&a, 0.5 / 30.0, false)[1].translation;
        assert!((mid[0] - 1.5).abs() < 1e-4);
    }

    #[test]
    fn untracked_bones_keep_rest() {
        let out = sample(&translate_clip(), 0.01, false);
        assert_eq!(out[0], rest(1)[0]);
        assert_eq!(out[2], rest(1)[0]);
        // Track channels that are empty keep rest values too.
        assert_eq!(out[1].rotation, Transform::IDENTITY.rotation);
    }

    #[test]
    fn clamps_when_not_looping() {
        let a = translate_clip();
        assert_eq!(sample(&a, 10.0, false)[1].translation[0], 6.0);
        assert_eq!(sample(&a, -1.0, false)[1].translation[0], 0.0);
    }

    #[test]
    fn wraps_when_looping() {
        let a = translate_clip();
        let base = sample(&a, 0.5 / 30.0, true)[1].translation[0];
        let wrapped = sample(&a, a.duration * 3.0 + 0.5 / 30.0, true)[1].translation[0];
        assert!((base - wrapped).abs() < 1e-3);
        let neg = sample(&a, -a.duration + 0.5 / 30.0, true)[1].translation[0];
        assert!((base - neg).abs() < 1e-3);
    }

    #[test]
    fn single_key_is_constant() {
        let a = AnimationBlob {
            duration: 1.0,
            sample_rate: 30.0,
            tracks: vec![Track {
                bone: 0,
                rotations: vec![],
                translations: vec![[1.0, 2.0, 3.0]],
                scales: vec![[2.0; 3]],
            }],
        };
        for t in [0.0, 0.4, 1.0] {
            let o = sample(&a, t, false)[0];
            assert_eq!(o.translation, [1.0, 2.0, 3.0]);
            assert_eq!(o.scale, [2.0; 3]);
        }
    }

    #[test]
    fn rotation_takes_shortest_path() {
        // +20 and -20 degrees around Z, second key negated (same rotation, other hemisphere).
        let q0 = Quat::from_rotation_z(20f32.to_radians());
        let q1 = -Quat::from_rotation_z((-20f32).to_radians());
        let a = AnimationBlob {
            duration: 1.0 / 30.0,
            sample_rate: 30.0,
            tracks: vec![Track {
                bone: 0,
                rotations: vec![quat_pack(q0.to_array()), quat_pack(q1.to_array())],
                translations: vec![],
                scales: vec![],
            }],
        };
        let mid = Quat::from_array(sample(&a, 0.5 / 30.0, false)[0].rotation);
        // Shortest path goes through 0 degrees, not 180.
        let expected = Quat::from_rotation_z(0.0);
        assert!(mid.dot(expected).abs() > 0.9999, "{mid:?}");
        assert!((mid.length() - 1.0).abs() < 1e-5);

        // Same-hemisphere sanity: plain midpoint.
        let b = AnimationBlob {
            tracks: vec![Track {
                bone: 0,
                rotations: vec![
                    quat_pack(Quat::IDENTITY.to_array()),
                    quat_pack(Quat::from_rotation_z(90f32.to_radians()).to_array()),
                ],
                translations: vec![],
                scales: vec![],
            }],
            ..a
        };
        let mid = Quat::from_array(sample(&b, 0.5 / 30.0, false)[0].rotation);
        assert!(mid.dot(Quat::from_rotation_z(45f32.to_radians())).abs() > 0.999);
    }

    #[test]
    fn out_of_range_bone_is_ignored() {
        let mut a = translate_clip();
        a.tracks[0].bone = 50;
        let out = sample(&a, 0.01, false);
        assert!(out.iter().all(|t| *t == rest(1)[0]));
    }
}
