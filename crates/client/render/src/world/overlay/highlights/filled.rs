//! `FilledHex`: each hex as a fan of six triangles, `width` the part of the hex covered
//! (`HighlightSystem::RenderFilledHexStyle`; RW-20). The pixel shader is `PSNoTexture`.

use super::{HighlightMesh, vertex};
use crate::hexgrid::{cell_center, corner_offset};

/// Adds the fills of `cells`.
pub fn append(mesh: &mut HighlightMesh, cells: &[(i32, i32)], width: f32) {
    for &(x, y) in cells {
        let c = cell_center(x, y);
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(vertex(c, 0.5, 0.5));
        for i in 0..6 {
            mesh.vertices
                .push(vertex(c + corner_offset(i) * width, 0.0, 0.0));
        }
        for i in 0..6u32 {
            mesh.indices
                .extend_from_slice(&[base, base + 1 + i, base + 1 + (i + 1) % 6]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fan_of_six_per_cell_covering_the_part_asked_for() {
        let mut m = HighlightMesh::default();
        append(&mut m, &[(1, 1)], 0.5);
        assert_eq!((m.vertices.len(), m.indices.len()), (7, 18));
        let c = cell_center(1, 1);
        let r = glam::Vec2::new(m.vertices[1].position[0], m.vertices[1].position[1]) - c;
        assert!((r.length() - crate::hexgrid::CORNER_RADIUS * 0.5).abs() < 1e-3);
    }
}
