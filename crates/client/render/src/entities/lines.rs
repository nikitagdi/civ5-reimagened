//! Flat strips and squares over the map for the blend pipeline: the shared builder of roads, the
//! highlight outlines and the path. CPU only.
//!
//! Every primitive lies on the ground: its corners take the terrain height under them, lifted a
//! little; the blend depth bias does the rest. Coordinates are the unwrapped frame of the cell the
//! primitive belongs to, so the renderer's map copies (wrap) show it at the seam too.

use glam::{Vec2, Vec3};

use crate::blend::{BlendVertex, NO_FEATHER};
use crate::terrain::{TerrainSnapshot, terrain_height_at};

/// Lift of a primitive over the ground, world units.
pub const LIFT: f32 = 1.0;
/// A strip over a slope is cut into pieces of at most this length so it follows the ground.
const PIECE: f32 = 16.0;

/// Vertices and triangles for the blend pipeline; the same vertex as rivers and borders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineMesh {
    pub vertices: Vec<BlendVertex>,
    pub indices: Vec<u32>,
}

impl LineMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn indices_valid(&self) -> bool {
        let n = self.vertices.len() as u32;
        self.indices.len().is_multiple_of(3) && self.indices.iter().all(|&i| i < n)
    }

    /// A quad `[a, b, c, d]` going round its outline; `color` is linear RGBA, `cell` indexes the
    /// fog-state texture.
    pub fn quad(&mut self, corners: [Vec3; 4], color: [f32; 4], cell: u32) {
        let base = self.vertices.len() as u32;
        for p in corners {
            self.vertices.push(BlendVertex {
                position: p.to_array(),
                across: NO_FEATHER,
                color,
                cell,
            });
        }
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    /// A quad whose corners are map points `(X, Z)`, lifted onto the ground.
    pub fn ground_quad(
        &mut self,
        terrain: &TerrainSnapshot,
        corners: [Vec2; 4],
        color: [f32; 4],
        cell: u32,
    ) {
        self.quad(corners.map(|p| on_ground(terrain, p)), color, cell);
    }

    /// A strip of `width` from `a` to `b`, cut into pieces that follow the ground.
    pub fn strip(
        &mut self,
        terrain: &TerrainSnapshot,
        a: Vec2,
        b: Vec2,
        width: f32,
        color: [f32; 4],
        cell: u32,
    ) {
        let d = b - a;
        let len = d.length();
        if len < 1e-3 {
            return;
        }
        let side = Vec2::new(-d.y, d.x) / len * (width * 0.5);
        let pieces = (len / PIECE).ceil().max(1.0) as u32;
        for i in 0..pieces {
            let p0 = a + d * (i as f32 / pieces as f32);
            let p1 = a + d * ((i + 1) as f32 / pieces as f32);
            self.ground_quad(
                terrain,
                [p0 - side, p1 - side, p1 + side, p0 + side],
                color,
                cell,
            );
        }
    }

    /// An axis-aligned square of half side `half` around `centre`.
    pub fn square(
        &mut self,
        terrain: &TerrainSnapshot,
        centre: Vec2,
        half: f32,
        color: [f32; 4],
        cell: u32,
    ) {
        let h = Vec2::splat(half);
        self.ground_quad(
            terrain,
            [
                centre + Vec2::new(-h.x, -h.y),
                centre + Vec2::new(h.x, -h.y),
                centre + Vec2::new(h.x, h.y),
                centre + Vec2::new(-h.x, h.y),
            ],
            color,
            cell,
        );
    }

    /// A diamond (a square turned by 45 degrees) of radius `radius`.
    pub fn diamond(
        &mut self,
        terrain: &TerrainSnapshot,
        centre: Vec2,
        radius: f32,
        color: [f32; 4],
        cell: u32,
    ) {
        self.ground_quad(
            terrain,
            [
                centre + Vec2::new(0.0, -radius),
                centre + Vec2::new(radius, 0.0),
                centre + Vec2::new(0.0, radius),
                centre + Vec2::new(-radius, 0.0),
            ],
            color,
            cell,
        );
    }

    /// Appends `other`, shifting its indices.
    pub fn append(&mut self, other: &LineMesh) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&other.vertices);
        self.indices.extend(other.indices.iter().map(|i| i + base));
    }
}

/// The map point `(X, Z)` on the ground, lifted by [`LIFT`].
pub fn on_ground(terrain: &TerrainSnapshot, p: Vec2) -> Vec3 {
    Vec3::new(p.x, terrain_height_at(terrain, p) + LIFT, p.y)
}

/// Linear RGBA from sRGB bytes and a byte alpha.
pub fn rgba(srgb: [u8; 3], alpha: u8) -> [f32; 4] {
    let [r, g, b] = crate::blend::srgb_to_linear(srgb);
    [r, g, b, f32::from(alpha) / 255.0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map};

    fn area(m: &LineMesh) -> f32 {
        m.indices
            .chunks_exact(3)
            .map(|t| {
                let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
                let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
                0.5 * ((b.x - a.x) * (c.z - a.z) - (c.x - a.x) * (b.z - a.z)).abs()
            })
            .sum()
    }

    #[test]
    fn a_strip_has_the_area_of_its_rectangle() {
        let t = map(3, 3, |_, _| land());
        let mut m = LineMesh::default();
        m.strip(&t, Vec2::ZERO, Vec2::new(40.0, 0.0), 3.0, [1.0; 4], 0);
        assert!((area(&m) - 120.0).abs() < 1e-2, "{}", area(&m));
        // 40 long, pieces of at most 16: three of them.
        assert_eq!(m.indices.len(), 3 * 6);
        assert!(m.indices_valid());
        assert!(
            m.vertices
                .iter()
                .all(|v| (v.position[1] - (crate::terrain::LAND_HEIGHT + LIFT)).abs() < 1e-4)
        );
    }

    #[test]
    fn degenerate_strips_are_skipped() {
        let t = map(1, 1, |_, _| land());
        let mut m = LineMesh::default();
        m.strip(&t, Vec2::ONE, Vec2::ONE, 3.0, [1.0; 4], 0);
        assert!(m.is_empty());
    }

    #[test]
    fn append_shifts_the_indices() {
        let t = map(1, 1, |_, _| land());
        let (mut a, mut b) = (LineMesh::default(), LineMesh::default());
        a.square(&t, Vec2::ZERO, 2.0, [1.0; 4], 0);
        b.diamond(&t, Vec2::ZERO, 2.0, [1.0; 4], 0);
        a.append(&b);
        assert_eq!(a.vertices.len(), 8);
        assert_eq!(&a.indices[6..9], &[4, 5, 6]);
        assert!(a.indices_valid());
    }

    #[test]
    fn colours_are_linear() {
        let c = rgba([255, 0, 0], 200);
        assert_eq!((c[0], c[1], c[2]), (1.0, 0.0, 0.0));
        assert!((c[3] - 200.0 / 255.0).abs() < 1e-6);
    }
}
