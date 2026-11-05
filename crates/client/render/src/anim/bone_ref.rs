//! `BoneRef` and the time in a clip to the frames of the baked clip.
//!
//! The instance type is the one `world-objects` already has in `ObjectInstance`; it is re-exported
//! here so that the module of the spec owns the name. Its meaning: the first bone of the frame
//! `base`, the first bone of the frame `next`, and `blend` in `[0, 1]`, so that the vertex shader
//! reads `mix(M(base, k), M(next, k), blend)` for bone `k`.

use super::data::BAKE_FPS;
pub use crate::world::objects::instance::BoneRef;

/// The two frames around the time `t` seconds of a clip with `frames` baked frames, and the
/// blend. `t` is already wrapped (a looping clip) or clamped (a clip that plays once) into
/// `[0, duration]`.
///
/// The last frame is the pose at the end of the clip, so the wrap of a looping clip blends the last
/// interval toward it and then starts again at frame 0: with a seamless clip there is no jump.
pub fn locate(t: f32, frames: u32) -> (u32, u32, f32) {
    locate_at(t, frames, BAKE_FPS)
}

/// [`locate`] for a clip baked at `fps` frames a second (`data::bake_fps_of`).
pub fn locate_at(t: f32, frames: u32, fps: f32) -> (u32, u32, f32) {
    let last = frames.saturating_sub(1);
    if last == 0 {
        return (0, 0, 0.0);
    }
    let x = if t.is_finite() {
        (t * fps).max(0.0)
    } else {
        0.0
    };
    let k = (x.floor() as u32).min(last - 1);
    (k, k + 1, (x - k as f32).clamp(0.0, 1.0))
}

/// The reference for the time `t` in a clip whose frames are `bones` slots each and start at slot
/// `first`.
pub fn frame_ref(first: u32, bones: u32, frames: u32, t: f32) -> BoneRef {
    frame_ref_at(first, bones, frames, t, BAKE_FPS)
}

/// [`frame_ref`] for a clip baked at `fps` frames a second.
pub fn frame_ref_at(first: u32, bones: u32, frames: u32, t: f32, fps: f32) -> BoneRef {
    let (k, next, blend) = locate_at(t, frames, fps);
    BoneRef {
        base: first + k * bones,
        next: first + next * bones,
        blend,
    }
}

/// The time inside a clip of `duration` seconds that has played `elapsed` seconds from its start:
/// wrapped into `[0, duration)` for a loop, clamped to `[0, duration]` for a clip that plays once.
pub fn clip_time(elapsed: f32, duration: f32, looping: bool) -> f32 {
    if duration.is_nan() || duration <= 0.0 || !elapsed.is_finite() {
        return 0.0;
    }
    if looping {
        elapsed.rem_euclid(duration)
    } else {
        elapsed.clamp(0.0, duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_is_the_floor_of_thirty_times_the_time() {
        let (k, next, blend) = locate(0.5, 61);
        assert_eq!((k, next), (15, 16));
        assert!(blend.abs() < 1e-5);
        let (k, next, blend) = locate(0.5 + 1.0 / 60.0, 61);
        assert_eq!((k, next), (15, 16));
        assert!((blend - 0.5).abs() < 1e-4, "{blend}");
        assert_eq!(locate(0.0, 61), (0, 1, 0.0));
    }

    #[test]
    fn the_end_of_the_clip_stays_inside_the_frames() {
        // A clip of 2 s: 61 frames, the last at index 60 = the pose at the end.
        let (k, next, blend) = locate(2.0, 61);
        assert_eq!((k, next), (59, 60));
        assert!((blend - 1.0).abs() < 1e-5);
        // A time past the end, or nonsense, never leaves the frames.
        assert!(locate(99.0, 61).1 <= 60);
        assert_eq!(locate(f32::NAN, 61), (0, 1, 0.0));
        assert_eq!(locate(-3.0, 61), (0, 1, 0.0));
        // The wrap of a loop blends the last interval toward the end pose, then frame 0 again.
        let almost = clip_time(2.0 - 1e-4, 2.0, true);
        let (_, next, _) = locate(almost, 61);
        assert_eq!(next, 60);
        let (k, _, _) = locate(clip_time(2.0, 2.0, true), 61);
        assert_eq!(k, 0);
        // A clip with one frame has nothing to blend.
        assert_eq!(locate(1.0, 1), (0, 0, 0.0));
        assert_eq!(locate(1.0, 0), (0, 0, 0.0));
    }

    #[test]
    fn slots_follow_the_frames() {
        // 7 bones a frame from slot 510: frame 3 starts at 531, past a row of 512.
        let r = frame_ref(510, 7, 61, 3.0 / 30.0);
        assert_eq!((r.base, r.next), (510 + 21, 510 + 28));
        assert!(r.blend.abs() < 1e-5);
    }

    #[test]
    fn clip_time_wraps_loops_and_clamps_the_rest() {
        assert!((clip_time(2.5, 2.0, true) - 0.5).abs() < 1e-6);
        assert!((clip_time(-0.5, 2.0, true) - 1.5).abs() < 1e-6);
        assert_eq!(clip_time(2.5, 2.0, false), 2.0);
        assert_eq!(clip_time(-1.0, 2.0, false), 0.0);
        assert_eq!(clip_time(1.0, 0.0, true), 0.0);
        assert_eq!(clip_time(f32::INFINITY, 2.0, true), 0.0);
    }
}
