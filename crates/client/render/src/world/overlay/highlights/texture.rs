//! `StaticTexture`: a texture over the box of each hex (`static_texture_highlight.dds`), the way the
//! grid's texture covers it (`grid.rs`: the width of the hex by twice the corner radius, north up).
//! The pixel shader is `PSSimpleTexture` times the colour of the call.

use glam::Vec2;

use super::{HighlightMesh, vertex};
use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH, cell_center};

/// Adds the quads of `cells`.
pub fn append(mesh: &mut HighlightMesh, cells: &[(i32, i32)]) {
    for &(x, y) in cells {
        let c = cell_center(x, y);
        let base = mesh.vertices.len() as u32;
        // Our `Z` grows to the south, so the top of the texture is the smaller `Z`.
        for (dx, dz) in [(-0.5, -1.0), (0.5, -1.0), (0.5, 1.0), (-0.5, 1.0)] {
            let p = c + Vec2::new(dx * HEX_WIDTH, dz * CORNER_RADIUS);
            mesh.vertices.push(vertex(p, 0.5 + dx, 0.5 + dz * 0.5));
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quad_covers_the_box_of_the_hex_with_north_at_the_top() {
        let mut m = HighlightMesh::default();
        append(&mut m, &[(0, 0)]);
        assert_eq!((m.vertices.len(), m.indices.len()), (4, 6));
        let top = &m.vertices[0];
        assert_eq!(top.uv, [0.0, 0.0]);
        let bottom = &m.vertices[2];
        assert_eq!(bottom.uv, [1.0, 1.0]);
        assert!(
            top.position[1] < bottom.position[1],
            "north is the smaller Z"
        );
        assert!((bottom.position[0] - top.position[0] - HEX_WIDTH).abs() < 1e-3);
    }
}
