//! The marker models of SIMP-011: a plain box pillar, tinted with the owner's colour per instance.

use asset_format::{Indices, Material, Mesh, ModelBlob};

/// Unit pillar: 8 wide, 24 tall (a bit taller than a Warrior, 16.5).
pub const UNIT_MARKER: (f32, f32) = (4.0, 24.0);
/// City pillar: 24 wide, 36 tall.
pub const CITY_MARKER: (f32, f32) = (12.0, 36.0);

/// Octahedral snorm16 encoding of a unit vector (inverse of the shader's `oct_decode`).
pub fn oct_encode(n: [f32; 3]) -> [i16; 2] {
    let l1 = n[0].abs() + n[1].abs() + n[2].abs();
    let (mut x, mut y) = (n[0] / l1, n[1] / l1);
    if n[2] < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * if ox >= 0.0 { 1.0 } else { -1.0 };
        y = (1.0 - ox.abs()) * if oy >= 0.0 { 1.0 } else { -1.0 };
    }
    [(x * 32767.0).round() as i16, (y * 32767.0).round() as i16]
}

/// A box of half width `half` and height `height`, standing on y = 0, one material, no texture.
pub fn pillar((half, height): (f32, f32)) -> ModelBlob {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    // Face: normal, then two tangent axes.
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    // Half extents per axis, centre of the box at (0, height / 2, 0).
    let ext = [half, height / 2.0, half];
    for (n, u, v) in faces {
        let base = positions.len() as u16;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let p: [f32; 3] = std::array::from_fn(|k| (n[k] + u[k] * su + v[k] * sv) * ext[k]);
            positions.push([p[0], p[1] + height / 2.0, p[2]]);
            normals.push(oct_encode(n));
        }
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let uvs = vec![[0, 0]; positions.len()];
    ModelBlob {
        meshes: vec![Mesh {
            positions,
            normals,
            uvs,
            joints: vec![],
            weights: vec![],
            indices: Indices::U16(indices),
            material: 0,
        }],
        materials: vec![Material::default()],
        ..ModelBlob::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(e: [i16; 2]) -> [f32; 3] {
        let (x, y) = (f32::from(e[0]) / 32767.0, f32::from(e[1]) / 32767.0);
        let mut n = [x, y, 1.0 - x.abs() - y.abs()];
        if n[2] < 0.0 {
            let s = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
            n = [(1.0 - y.abs()) * s(x), (1.0 - x.abs()) * s(y), n[2]];
        }
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        n.map(|v| v / l)
    }

    #[test]
    fn oct_encoding_round_trips_the_axes() {
        for n in [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ] {
            let d = decode(oct_encode(n));
            assert!((0..3).all(|k| (d[k] - n[k]).abs() < 1e-3), "{n:?} -> {d:?}");
        }
    }

    #[test]
    fn pillar_is_a_closed_box_on_the_ground() {
        let b = pillar(UNIT_MARKER);
        let m = &b.meshes[0];
        assert_eq!((m.positions.len(), m.indices.len()), (24, 36));
        let ys = m.positions.iter().map(|p| p[1]);
        assert_eq!(ys.clone().fold(f32::MAX, f32::min), 0.0);
        assert_eq!(ys.fold(f32::MIN, f32::max), 24.0);
        assert!(
            m.positions
                .iter()
                .all(|p| p[0].abs() <= 4.0 && p[2].abs() <= 4.0)
        );
        assert_eq!(m.normals.len(), 24);
    }
}
