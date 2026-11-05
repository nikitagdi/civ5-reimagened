//! `SplineBorder`: a strip `width` world units wide along the spline round a set of hexes, from the
//! outline inwards, `u` 0 on the line and 1 on the inner side (the strip of the draw in S4's
//! `OverlayShaders::PSContour`: pairs of vertices 6 apart across, `v` 1, a pair every 4.4 along;
//! `BuildSplineBorderStyle`, `TessellateSplineBorderStyle`; RW-20).
//!
//! The outline of the set is the edges between a cell of it and a cell outside it, chained and
//! rounded as the borders' are (`borders/points.rs`, `smooth.rs`), without the inset and the shake.

use super::{HighlightMesh, vertex};
use crate::hexgrid::GridShape;
use crate::world::overlay::OwnersInput;
use crate::world::overlay::borders::mesh::sections;
use crate::world::overlay::borders::points::control_lines;
use crate::world::overlay::borders::smooth::smooth;

/// Adds the strips of the outline of `cells` (normalized map cells).
pub fn append(mesh: &mut HighlightMesh, cells: &[(i32, i32)], shape: &GridShape, width: f32) {
    let (w, h) = (shape.width as u32, shape.height as u32);
    let mut owners = vec![-1i8; w as usize * h as usize];
    for &(x, y) in cells {
        owners[(y * shape.width + x) as usize] = 0;
    }
    let Some(input) = OwnersInput::new(w, h, owners, Vec::new()) else {
        return;
    };
    for line in control_lines(&input, shape, 0, 0.0) {
        let samples = smooth(&line.points, line.closed);
        if samples.points.len() < 2 {
            continue;
        }
        let secs = sections(&samples, line.inward_left);
        let base = mesh.vertices.len() as u32;
        for s in &secs {
            mesh.vertices.push(vertex(s.p, 0.0, 1.0));
            mesh.vertices.push(vertex(s.p + s.n * width, 1.0, 1.0));
        }
        let n = secs.len() as u32;
        let spans = if line.closed { n } else { n - 1 };
        for k in 0..spans {
            let (a, b) = (base + 2 * k, base + 2 * ((k + 1) % n));
            mesh.indices
                .extend_from_slice(&[a, b, b + 1, a, b + 1, a + 1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    fn shape() -> GridShape {
        GridShape {
            width: 8,
            height: 8,
            wrap_x: false,
        }
    }

    #[test]
    fn a_set_gets_one_closed_strip_of_the_width_asked_for() {
        let mut m = HighlightMesh::default();
        append(&mut m, &[(3, 3), (4, 3), (3, 4)], &shape(), 6.0);
        assert!(!m.indices.is_empty());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        for pair in m.vertices.chunks(2) {
            let a = Vec2::new(pair[0].position[0], pair[0].position[1]);
            let b = Vec2::new(pair[1].position[0], pair[1].position[1]);
            assert!((a.distance(b) - 6.0).abs() < 1e-3);
            assert_eq!((pair[0].uv[0], pair[1].uv[0]), (0.0, 1.0));
        }
    }

    #[test]
    fn the_strip_goes_inwards() {
        let mut m = HighlightMesh::default();
        append(&mut m, &[(4, 4)], &shape(), 6.0);
        let c = crate::hexgrid::cell_center(4, 4);
        for pair in m.vertices.chunks(2) {
            let on_line = Vec2::new(pair[0].position[0], pair[0].position[1]).distance(c);
            let inner = Vec2::new(pair[1].position[0], pair[1].position[1]).distance(c);
            assert!(inner < on_line, "{inner} < {on_line}");
        }
    }
}
