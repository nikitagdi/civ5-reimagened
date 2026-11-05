//! `SNAPSHOT`, the layout of 501 of the 631 rows (world-objects 4.5, WOB4): "positions and turns
//! from the model". The pipeline merges the meshes of such a model into one whole and puts the
//! `InitialPlacement` of every part into the vertices (`MergeScope::Model`, `bake_placement`), so
//! the model as stored is the snapshot: one instance, the middle of the plot, no turn of its own.
//! Only the scale of the row and the height of the ground are added ([`placement`]).
//!
//! The column `TerrainContour` (the model follows the relief) and the stencil variant of the shader
//! (`LandmarkShader_Stencil`) are RW-15's open questions; until they are answered the model stands
//! at the height of the middle of the plot (and of the water, when the plot is at sea).

use glam::{Mat4, Vec2, Vec3};

/// The matrix `T * S` of a snapshot at `centre` on the ground of height `y`.
pub fn placement(centre: Vec2, y: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(centre.x, y, centre.y))
        * Mat4::from_scale(Vec3::splat(row_scale(scale)))
}

/// The scale a row asks: a missing (`0`) or broken one is 1.
pub fn row_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_stands_in_the_middle_at_the_scale_of_the_row() {
        let m = placement(Vec2::new(10.0, -4.0), 3.0, 0.5);
        assert_eq!(m.transform_point3(Vec3::ZERO), Vec3::new(10.0, 3.0, -4.0));
        assert_eq!(
            m.transform_point3(Vec3::new(2.0, 2.0, 2.0)),
            Vec3::new(11.0, 4.0, -3.0)
        );
    }

    #[test]
    fn a_row_without_a_scale_is_not_shrunk_to_nothing() {
        assert_eq!(row_scale(0.0), 1.0);
        assert_eq!(row_scale(f32::NAN), 1.0);
        assert_eq!(row_scale(-2.0), 1.0);
        assert_eq!(row_scale(0.07), 0.07);
    }
}
