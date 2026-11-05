//! Uniform resampling of one bone's animation into blob keys.
//!
//! Independent of Granny: the caller supplies a closure that evaluates the local pose at a time,
//! so the logic is testable with hand-made tracks.

use asset_format::{Track, Transform, quat_pack};

/// Keys per second of every animation blob.
pub const SAMPLE_RATE: f32 = 30.0;

/// Local pose of one bone at one time, already in engine convention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub translation: [f32; 3],
    /// Unit quaternion xyzw.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// Keys needed to cover `[0, duration]` inclusive at `rate`. At least one.
pub fn key_count(duration: f32, rate: f32) -> usize {
    if duration.is_nan() || duration <= 0.0 || rate.is_nan() || rate <= 0.0 {
        return 1;
    }
    // The epsilon keeps clips that are an exact number of frames from growing an extra key.
    let frames = (duration * rate - 1e-3).ceil().max(0.0) as usize;
    frames + 1
}

/// Time at which key `i` is evaluated. The last key is clamped to the clip end, so a clip whose
/// length is not a whole number of frames still ends exactly on its final pose.
pub fn key_time(i: usize, duration: f32, rate: f32) -> f32 {
    (i as f32 / rate).min(duration.max(0.0))
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5 * (1.0 + a.abs().max(b.abs()))
}

fn approx3(a: &[f32; 3], b: &[f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| approx(*x, *y))
}

/// Quaternions equal up to sign.
fn approx_quat(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let sign = if a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
        -1.0
    } else {
        1.0
    };
    a.iter().zip(b).all(|(x, y)| (x - sign * y).abs() <= 1e-5)
}

/// Collapses a channel: all keys equal means one key; one key equal to the rest value means none.
fn collapse<T: Copy>(keys: Vec<T>, rest: T, eq: impl Fn(&T, &T) -> bool) -> Vec<T> {
    let Some(first) = keys.first().copied() else {
        return keys;
    };
    if keys.iter().all(|k| eq(k, &first)) {
        if eq(&first, &rest) {
            Vec::new()
        } else {
            vec![first]
        }
    } else {
        keys
    }
}

/// Samples `eval` at `key_count` times and builds the track for `bone`. Returns `None` when every
/// channel is constant and equal to `rest` (the client already shows the rest pose for it).
pub fn resample_track(
    bone: u16,
    duration: f32,
    rate: f32,
    rest: &Transform,
    eval: impl Fn(f32) -> Sample,
) -> Option<Track> {
    let n = key_count(duration, rate);
    let mut rotations = Vec::with_capacity(n);
    let mut translations = Vec::with_capacity(n);
    let mut scales = Vec::with_capacity(n);
    let mut prev: Option<[f32; 4]> = None;
    for i in 0..n {
        let s = eval(key_time(i, duration, rate));
        // Same hemisphere as the previous key keeps interpolation on the short path.
        let mut q = s.rotation;
        if prev.is_some_and(|p| p.iter().zip(&q).map(|(a, b)| a * b).sum::<f32>() < 0.0) {
            q = q.map(|c| -c);
        }
        prev = Some(q);
        rotations.push(q);
        translations.push(s.translation);
        scales.push(s.scale);
    }
    let rotations = collapse(rotations, rest.rotation, approx_quat)
        .into_iter()
        .map(quat_pack)
        .collect::<Vec<_>>();
    // Quantized keys can differ from each other even if the floats were equal: re-check.
    let rotations = if rotations.len() > 1 && rotations.windows(2).all(|w| w[0] == w[1]) {
        rotations[..1].to_vec()
    } else {
        rotations
    };
    let translations = collapse(translations, rest.translation, approx3);
    let scales = collapse(scales, rest.scale, approx3);
    if rotations.is_empty() && translations.is_empty() && scales.is_empty() {
        return None;
    }
    Some(Track {
        bone,
        rotations,
        translations,
        scales,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::quat_unpack;

    fn rest() -> Transform {
        Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    fn sample_at(t: f32) -> Sample {
        Sample {
            translation: [t * 10.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    #[test]
    fn key_counts_cover_the_end_inclusive() {
        assert_eq!(key_count(0.0, 30.0), 1);
        assert_eq!(key_count(1.0, 30.0), 31);
        assert_eq!(key_count(2.0 / 30.0, 30.0), 3);
        // 1.01 s is 30.3 frames: 31 frames needed, so 32 keys.
        assert_eq!(key_count(1.01, 30.0), 32);
        assert_eq!(key_time(0, 1.01, 30.0), 0.0);
        assert_eq!(key_time(31, 1.01, 30.0), 1.01);
        assert!((key_time(15, 1.01, 30.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn linear_motion_is_sampled_at_frame_times() {
        let track = resample_track(4, 3.0 / 30.0, 30.0, &rest(), sample_at).unwrap();
        assert_eq!(track.bone, 4);
        assert_eq!(track.translations.len(), 4);
        for (i, t) in track.translations.iter().enumerate() {
            assert!((t[0] - i as f32 / 30.0 * 10.0).abs() < 1e-5);
        }
        // Rotation and scale stay at rest and are dropped.
        assert!(track.rotations.is_empty());
        assert!(track.scales.is_empty());
    }

    #[test]
    fn constant_channels_collapse_to_one_key_or_nothing() {
        let moved = |_t: f32| Sample {
            translation: [5.0, 5.0, 5.0],
            rotation: [0.0, 0.0, 1.0f32.sin(), 1.0f32.cos()],
            scale: [2.0, 2.0, 2.0],
        };
        let track = resample_track(0, 1.0, 30.0, &rest(), moved).unwrap();
        assert_eq!(track.translations, [[5.0, 5.0, 5.0]]);
        assert_eq!(track.scales, [[2.0; 3]]);
        assert_eq!(track.rotations.len(), 1);

        let at_rest = |_t: f32| Sample {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        assert!(resample_track(0, 1.0, 30.0, &rest(), at_rest).is_none());
    }

    #[test]
    fn rotation_keys_stay_in_one_hemisphere() {
        // Alternating sign of the same rotation must still count as constant.
        let flip = |t: f32| {
            let s = if (t * 30.0).round() as i32 % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            Sample {
                translation: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.6 * s, 0.8 * s],
                scale: [1.0; 3],
            }
        };
        let track = resample_track(0, 5.0 / 30.0, 30.0, &rest(), flip).unwrap();
        assert_eq!(track.rotations.len(), 1);

        // A real swing keeps neighbouring keys within 90 degrees of each other in 4D.
        let swing = |t: f32| {
            let a = (t * 30.0 * 0.5).sin();
            let q = [
                0.0,
                0.0,
                a.sin() * 0.5,
                (1.0 - (a.sin() * 0.5).powi(2)).sqrt(),
            ];
            Sample {
                translation: [1.0, 2.0, 3.0],
                rotation: q,
                scale: [1.0; 3],
            }
        };
        let track = resample_track(0, 1.0, 30.0, &rest(), swing).unwrap();
        for w in track.rotations.windows(2) {
            let (a, b) = (quat_unpack(w[0]), quat_unpack(w[1]));
            assert!(a.iter().zip(&b).map(|(x, y)| x * y).sum::<f32>() > 0.0);
        }
    }

    #[test]
    fn zero_duration_gives_a_single_key() {
        let track = resample_track(2, 0.0, 30.0, &rest(), sample_at).unwrap();
        assert_eq!(track.translations.len(), 1);
    }

    #[test]
    fn resampled_track_reproduces_the_curve_when_sampled_back() {
        use asset_format::AnimationBlob;
        let d = 20.0 / 30.0;
        let track = resample_track(1, d, 30.0, &rest(), sample_at).unwrap();
        let blob = AnimationBlob {
            duration: d,
            sample_rate: 30.0,
            tracks: vec![track],
        };
        let r = [rest(), rest()];
        let mut out = r;
        for t in [0.0, 0.123, 0.4, d] {
            blob.sample(t, false, &r, &mut out);
            assert!((out[1].translation[0] - t * 10.0).abs() < 1e-3, "t={t}");
        }
    }
}
