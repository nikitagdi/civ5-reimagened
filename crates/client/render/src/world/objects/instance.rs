//! `ObjectInstance`: the data of one object in the instance buffer of a ported family (spec 4.9,
//! WO-D4). The original gives the matrix of a unit or landmark to the shader as constants, one
//! draw per object; we give it as the attributes of the instance, one draw per model. The
//! formula of the shader is the same, only the source of the number changes (`data_moves` of
//! `shader-re` says so: `WorldMatrix` -> `instance:world`, `TeamColorR` -> `instance:team_color`).
//!
//! 64 bytes: six attributes with the five of the vertex (`POSITION`, `NORMAL`, `TEXCOORD0`,
//! `BLENDINDICES`, `BLENDWEIGHT`) stay inside the 16 of WebGL2.
//!
//! B1's pipeline (`StaticInstance`, 48 bytes, `unit_family::instance_layout`) is what the families
//! of WOB1 are drawn with today; [`ObjectInstance::from_static`] is the same transform in the
//! shape of the spec, for the families that take it (`ForestShader`, the instanced buildings, the
//! skinned units of `world-anim`).

use bytemuck::{Pod, Zeroable};

use crate::material::InstanceLayout;
use crate::static_model::StaticInstance;

/// The pose of an object in the baked clips (`world-anim` 4.2): the first frame, the next one and
/// the blend between them. Zeros for a rigid object.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct BoneRef {
    pub base: u32,
    pub next: u32,
    pub blend: f32,
}

/// One object of a model. The rows of the 3x4 matrix are in our basis (X east, Y up, Z south); the
/// vertex shader multiplies the result by `ours_to_orig`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Pod, Zeroable)]
pub struct ObjectInstance {
    /// The rows of the 3x4 matrix of the object: `p' = (row0 . (p, 1), row1 . (p, 1), row2 . (p, 1))`.
    pub world: [[f32; 4]; 3],
    /// `TeamColor` as sRGB bytes; the alpha is the transparency (`SetAlphaValues`), 255 opaque.
    pub color: [u8; 4],
    pub bones: BoneRef,
}

impl ObjectInstance {
    pub const STRIDE: u64 = 64;

    /// The matrix `T * R_y * S` of a B1 instance: B1's `rotate_y` and uniform scale, then the
    /// position. The team colour is `0x01RRGGBB` (`pack_team`), the alpha is that of the instance.
    pub fn from_static(i: &StaticInstance) -> Self {
        let (s, c) = i.rotation_y.sin_cos();
        let k = i.scale;
        // rotate_y of B1: (x c + z s, y, -x s + z c), times the scale.
        let world = [
            [c * k, 0.0, s * k, i.position[0]],
            [0.0, k, 0.0, i.position[1]],
            [-s * k, 0.0, c * k, i.position[2]],
        ];
        let [r, g, b] = [(i.team >> 16) as u8, (i.team >> 8) as u8, i.team as u8];
        Self {
            world,
            color: [r, g, b, (i.alpha.clamp(0.0, 1.0) * 255.0).round() as u8],
            bones: BoneRef::default(),
        }
    }

    /// `p` through the matrix.
    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let row = |r: &[f32; 4]| r[0] * p[0] + r[1] * p[1] + r[2] * p[2] + r[3];
        [
            row(&self.world[0]),
            row(&self.world[1]),
            row(&self.world[2]),
        ]
    }
}

/// The layout of the instance buffer for the pipeline cache: three rows, the colour, the bones.
pub fn instance_layout() -> InstanceLayout {
    use wgpu::VertexFormat as F;
    InstanceLayout::new(
        ObjectInstance::STRIDE,
        vec![
            (F::Float32x4, 0),  // row 0
            (F::Float32x4, 16), // row 1
            (F::Float32x4, 32), // row 2
            (F::Unorm8x4, 48),  // team colour and alpha
            (F::Uint32x2, 52),  // bones: base, next
            (F::Float32, 60),   // bones: blend
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::unit_family::vertex_layout;
    use crate::shaders::MAX_VERTEX_ATTRIBUTES;
    use crate::static_model::pack_team;

    #[test]
    fn an_instance_is_64_bytes_and_the_layout_says_so() {
        assert_eq!(std::mem::size_of::<ObjectInstance>(), 64);
        assert_eq!(std::mem::size_of::<BoneRef>(), 12);
        assert_eq!(instance_layout().stride(), 64);
    }

    /// The six attributes of the instance and the vertex of B1 fit the 16 of WebGL2 (4.9).
    #[test]
    fn the_instance_and_the_vertex_fit_the_attribute_limit() {
        let attrs = instance_layout().attributes(&vertex_layout()).unwrap();
        assert_eq!(attrs.len(), 6);
        assert!(attrs.len() + vertex_layout().attrs().len() <= MAX_VERTEX_ATTRIBUTES as usize);
        // The offsets are those of the struct.
        let offsets: Vec<u64> = attrs.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, [0, 16, 32, 48, 52, 60]);
    }

    /// The matrix of a B1 instance is B1's own transform: scale, turn about Y, move.
    #[test]
    fn from_static_is_the_transform_of_b1() {
        let mut i = StaticInstance::new([10.0, 2.0, -5.0], 0.7, 1.5);
        i.team = pack_team([200, 30, 40]);
        i.alpha = 0.5;
        let o = ObjectInstance::from_static(&i);
        let p = [1.0, 2.0, 3.0];
        // B1: scale, then rotate_y (x c + z s, y, -x s + z c), then translate.
        let (s, c) = 0.7f32.sin_cos();
        let q = [p[0] * 1.5, p[1] * 1.5, p[2] * 1.5];
        let want = [
            q[0] * c + q[2] * s + 10.0,
            q[1] + 2.0,
            -q[0] * s + q[2] * c - 5.0,
        ];
        let got = o.transform_point(p);
        for k in 0..3 {
            assert!((got[k] - want[k]).abs() < 1e-5, "{got:?} {want:?}");
        }
        assert_eq!(o.color, [200, 30, 40, 128]);
        assert_eq!(o.bones, BoneRef::default());
    }
}
