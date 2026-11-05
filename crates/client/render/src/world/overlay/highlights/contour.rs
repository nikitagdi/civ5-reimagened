//! `HexContour`: the outline of each hex, a ring `width` wide (a part of the corner radius) inside
//! the hex's edge, the profile texture across it: `u` is 0 on the edge and 1 on the inner ring
//! (`HighlightSystem::AddHexContourStyle`, `RenderHexContourStyle`; RW-20). The ring is the grid's
//! (`grid.rs`): a quad per side between the twelve vertices.

use super::{HighlightMesh, vertex};
use crate::hexgrid::{cell_center, corner_offset};
use crate::world::overlay::grid::{INDICES_PER_HEX, VERTICES_PER_HEX, ring_indices};

/// Adds the rings of `cells` (normalized map cells).
pub fn append(mesh: &mut HighlightMesh, cells: &[(i32, i32)], width: f32) {
    let template = ring_indices();
    let keep = 1.0 - width;
    for &(x, y) in cells {
        let c = cell_center(x, y);
        let base = mesh.vertices.len() as u32;
        for i in 0..VERTICES_PER_HEX {
            let (ring, u) = if i < 6 { (1.0, 0.0) } else { (keep, 1.0) };
            mesh.vertices
                .push(vertex(c + corner_offset(i % 6) * ring, u, 1.0));
        }
        debug_assert_eq!(template.len(), INDICES_PER_HEX);
        mesh.indices
            .extend(template.iter().map(|&i| u32::from(i) + base));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::CORNER_RADIUS;
    use glam::Vec2;

    #[test]
    fn a_ring_per_cell_with_the_profile_across_it() {
        let mut m = HighlightMesh::default();
        append(&mut m, &[(2, 2), (3, 2)], 0.2);
        assert_eq!(m.vertices.len(), 24);
        assert_eq!(m.indices.len(), 72);
        let c = cell_center(2, 2);
        for (i, v) in m.vertices[..12].iter().enumerate() {
            let d = (Vec2::new(v.position[0], v.position[1]) - c).length();
            let want = if i < 6 {
                CORNER_RADIUS
            } else {
                CORNER_RADIUS * 0.8
            };
            assert!((d - want).abs() < 1e-3);
            assert_eq!(v.uv[0], if i < 6 { 0.0 } else { 1.0 });
        }
        assert!(m.indices[36..].iter().all(|&i| i >= 12));
    }
}
