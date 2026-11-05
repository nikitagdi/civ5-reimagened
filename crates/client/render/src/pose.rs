//! CPU pose evaluation: animation sample → local → world → skinning palette. GPU-independent.

use asset_format::{AnimationBlob, Skeleton, Transform};
use glam::Mat4;

/// Reusable buffers, so evaluating a pose each frame does not allocate.
#[derive(Debug, Default)]
pub struct PoseEvaluator {
    rest: Vec<Transform>,
    local: Vec<Transform>,
    world: Vec<Mat4>,
    skin: Vec<Mat4>,
}

impl PoseEvaluator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Skin matrices (`world * inverse_bind`) at time `t` seconds, rest pose if `anim` is `None`.
    /// The returned slice has one matrix per bone.
    pub fn evaluate(
        &mut self,
        skeleton: &Skeleton,
        anim: Option<&AnimationBlob>,
        t: f32,
        looping: bool,
    ) -> &[Mat4] {
        let n = skeleton.bones.len();
        self.rest.clear();
        self.rest.extend(skeleton.bones.iter().map(|b| b.rest));
        self.local.clear();
        self.local.extend_from_slice(&self.rest);
        self.world.clear();
        self.world.resize(n, Mat4::IDENTITY);
        self.skin.clear();
        self.skin.resize(n, Mat4::IDENTITY);

        if let Some(a) = anim {
            a.sample(t, looping, &self.rest, &mut self.local);
        }
        skeleton.world_matrices(&self.local, &mut self.world);
        skeleton.skin_matrices(&self.world, &mut self.skin);
        &self.skin
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;

    #[test]
    fn rest_pose_is_identity_skinning() {
        // Inverse bind matrices are the inverse of the rest world matrices.
        let blob = synthetic::skinned_cylinder();
        let mut ev = PoseEvaluator::new();
        for m in ev.evaluate(&blob.model.skeleton, None, 0.0, true) {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-5), "{m:?}");
        }
    }

    #[test]
    fn animation_bends_the_upper_bone() {
        let blob = synthetic::skinned_cylinder();
        let mut ev = PoseEvaluator::new();
        let quarter = blob.animations[0].duration * 0.25;
        let skin = ev
            .evaluate(
                &blob.model.skeleton,
                Some(&blob.animations[0]),
                quarter,
                true,
            )
            .to_vec();
        assert_eq!(skin.len(), 2);
        assert!(skin[0].abs_diff_eq(Mat4::IDENTITY, 1e-5), "root stays put");
        assert!(!skin[1].abs_diff_eq(Mat4::IDENTITY, 1e-3), "bone 1 moves");
        // A rotation keeps determinant 1 (no shear or scale from the pipeline).
        assert!((skin[1].determinant() - 1.0).abs() < 1e-3);
        // Looping: one full period later gives the same palette.
        let again = ev
            .evaluate(
                &blob.model.skeleton,
                Some(&blob.animations[0]),
                quarter + blob.animations[0].duration,
                true,
            )
            .to_vec();
        assert!(again[1].abs_diff_eq(skin[1], 1e-3));
    }
}
