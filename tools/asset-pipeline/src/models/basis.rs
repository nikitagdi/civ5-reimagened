//! Coordinate convention change between Civ5/Granny and the engine.
//!
//! A [`Basis`] is a signed permutation matrix `B`; a source point `p` becomes `B * p`. Everything
//! that lives in model space (positions, normals, bone rest transforms, inverse bind matrices and
//! animation keys) must go through the same basis, with transforms conjugated (`B * M * B^-1`)
//! rather than just multiplied, so skinning stays consistent.

use asset_format::Transform;

/// Signed permutation matrix, row-major, `engine = m * source`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Basis {
    m: [[f32; 3]; 3],
}

impl Basis {
    pub const IDENTITY: Basis = Basis {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    /// Civ5 models come from 3ds Max: Z up, right-handed. The engine is Y up, right-handed
    /// (wgpu/glam, hex grid on XZ). `(x, y, z) -> (x, z, -y)` is a proper rotation (-90 degrees
    /// about X), so handedness and triangle winding are preserved.
    pub const CIV5_TO_ENGINE: Basis = Basis {
        m: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
    };

    /// Builds a basis from a signed permutation matrix; `None` for anything else.
    pub fn new(m: [[f32; 3]; 3]) -> Option<Basis> {
        let rows_ok = m.iter().all(|row| {
            row.iter().filter(|v| v.abs() == 1.0).count() == 1
                && row.iter().filter(|v| **v == 0.0).count() == 2
        });
        let cols_ok = (0..3).all(|c| m.iter().filter(|row| row[c] != 0.0).count() == 1);
        (rows_ok && cols_ok).then_some(Basis { m })
    }

    pub fn determinant(&self) -> f32 {
        let m = &self.m;
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    }

    /// True for mirrors: triangle winding must be reversed to keep faces pointing outward.
    pub fn flips_handedness(&self) -> bool {
        self.determinant() < 0.0
    }

    /// Positions, directions and normals.
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|r| (0..3).map(|c| self.m[r][c] * p[c]).sum())
    }

    /// Conjugates a unit quaternion (xyzw). Under a mirror the axis is a pseudovector and gets an
    /// extra sign.
    pub fn rotation(&self, q: [f32; 4]) -> [f32; 4] {
        let v = self.point([q[0], q[1], q[2]]);
        let d = self.determinant().signum();
        [d * v[0], d * v[1], d * v[2], q[3]]
    }

    /// Diagonal scale: axes are permuted, signs drop out.
    pub fn scale(&self, s: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|r| (0..3).map(|c| self.m[r][c].abs() * s[c]).sum())
    }

    pub fn transform(&self, t: &Transform) -> Transform {
        Transform {
            translation: self.point(t.translation),
            rotation: self.rotation(t.rotation),
            scale: self.scale(t.scale),
        }
    }

    /// `B4 * a * B4^-1` for a column-major 4x4.
    pub fn matrix(&self, a: [f32; 16]) -> [f32; 16] {
        let mut b4 = [[0.0f32; 4]; 4];
        for (dst, src) in b4.iter_mut().zip(&self.m) {
            dst[..3].copy_from_slice(src);
        }
        b4[3][3] = 1.0;
        // t = B4 * a
        let mut t = [[0.0f32; 4]; 4];
        for r in 0..4 {
            for c in 0..4 {
                t[r][c] = (0..4).map(|k| b4[r][k] * a[c * 4 + k]).sum();
            }
        }
        // out = t * B4^T
        let mut out = [0.0f32; 16];
        for r in 0..4 {
            for c in 0..4 {
                out[c * 4 + r] = (0..4).map(|k| t[r][k] * b4[c][k]).sum();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
        [
            a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
            a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
            a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
            a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
        ]
    }

    fn quat_rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
        let p = quat_mul(
            quat_mul(q, [v[0], v[1], v[2], 0.0]),
            [-q[0], -q[1], -q[2], q[3]],
        );
        [p[0], p[1], p[2]]
    }

    fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
        let mut m = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                m[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
            }
        }
        m
    }

    fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    fn normalized(q: [f32; 4]) -> [f32; 4] {
        let l = q.iter().map(|c| c * c).sum::<f32>().sqrt();
        q.map(|c| c / l)
    }

    /// Column-major TRS matrix.
    fn to_matrix(t: &Transform) -> [f32; 16] {
        let [x, y, z, w] = t.rotation;
        let r = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        let mut m = [0.0f32; 16];
        for row in 0..3 {
            for col in 0..3 {
                m[col * 4 + row] = r[row][col] * t.scale[col];
            }
            m[12 + row] = t.translation[row];
        }
        m[15] = 1.0;
        m
    }

    const MIRROR_X: Basis = Basis {
        m: [[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    };

    #[test]
    fn civ5_basis_is_a_rotation_that_maps_z_up_to_y_up() {
        let b = Basis::CIV5_TO_ENGINE;
        assert_eq!(b.determinant(), 1.0);
        assert!(!b.flips_handedness());
        assert_eq!(b.point([0.0, 0.0, 1.0]), [0.0, 1.0, 0.0]);
        assert_eq!(b.point([1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        assert_eq!(b.point([0.0, 1.0, 0.0]), [0.0, 0.0, -1.0]);
        assert!(MIRROR_X.flips_handedness());
        assert_eq!(Basis::IDENTITY.point([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn new_accepts_only_signed_permutations() {
        assert!(Basis::new(Basis::CIV5_TO_ENGINE.m).is_some());
        assert!(Basis::new([[1.0, 1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
        assert!(Basis::new([[2.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
        assert!(Basis::new([[1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]).is_none());
    }

    /// `B (R v) == R' (B v)` for the rotation `R'` produced by `rotation`, proper or mirrored.
    #[test]
    fn rotation_conjugation_commutes_with_point_mapping() {
        let q = {
            let (s, c) = (0.7f32 / 2.0).sin_cos();
            let a = [0.3f32, -0.5, 0.8];
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            [s * a[0] / l, s * a[1] / l, s * a[2] / l, c]
        };
        let v = [0.4, -1.2, 2.5];
        for b in [Basis::CIV5_TO_ENGINE, MIRROR_X, Basis::IDENTITY] {
            let lhs = b.point(quat_rotate(q, v));
            let rhs = quat_rotate(b.rotation(q), b.point(v));
            assert!(close3(lhs, rhs), "{lhs:?} vs {rhs:?} for {b:?}");
        }
    }

    #[test]
    fn scale_is_permuted_and_stays_positive() {
        assert_eq!(
            Basis::CIV5_TO_ENGINE.scale([1.0, 2.0, 3.0]),
            [1.0, 3.0, 2.0]
        );
        assert_eq!(MIRROR_X.scale([1.0, 2.0, 3.0]), [1.0, 2.0, 3.0]);
    }

    /// Converting each factor and composing equals converting the composition.
    #[test]
    fn transform_composition_commutes_with_the_basis() {
        let a = Transform {
            translation: [1.0, 2.0, 3.0],
            rotation: normalized([0.1, 0.5, -0.3, 0.8]),
            scale: [1.0, 1.0, 1.0],
        };
        let c = Transform {
            translation: [-0.5, 0.25, 4.0],
            rotation: normalized([-0.4, 0.2, 0.6, 0.5]),
            scale: [1.0, 1.0, 1.0],
        };
        for b in [Basis::CIV5_TO_ENGINE, MIRROR_X] {
            let composed = b.matrix(mat_mul(&to_matrix(&a), &to_matrix(&c)));
            let separate = mat_mul(&to_matrix(&b.transform(&a)), &to_matrix(&b.transform(&c)));
            for (x, y) in composed.iter().zip(&separate) {
                assert!((x - y).abs() < 1e-5, "{composed:?} vs {separate:?}");
            }
            let single = b.matrix(to_matrix(&a));
            let via_trs = to_matrix(&b.transform(&a));
            for (x, y) in single.iter().zip(&via_trs) {
                assert!((x - y).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn matrix_conjugation_round_trips() {
        let b = Basis::CIV5_TO_ENGINE;
        let inv = Basis::new([[1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]]).unwrap();
        let m: [f32; 16] = std::array::from_fn(|i| (i as f32 * 0.37).sin());
        let back = inv.matrix(b.matrix(m));
        for (x, y) in m.iter().zip(&back) {
            assert!((x - y).abs() < 1e-5);
        }
    }
}
