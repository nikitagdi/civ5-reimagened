//! The stencil of the coast: `GenerateAllCoastStencil` and
//! `GenerateForestLayoutStencilMesh`. For every coastline with the land on its left the strip of
//! the sea side of the waterline, from the high beach (level 1) to the low side (level 4), is put
//! into the stencil of the terrain with the bits the layout of forests reads.

use glam::Vec2;

use super::bspline;
use super::levels::FinalLevels;
use crate::world::terrain::stencil::TerrainStencil;

/// The stencil bits of the strip: bit 0 and bit 3.
// re: GenerateForestLayoutStencilMesh 0x088e8d14 (`m_ucStencilBits = '\t'`)
pub const FOREST_LAYOUT_BITS: u8 = 0x09;

/// The level the strip starts at and the one it ends at.
// re: GenerateForestLayoutStencilMesh 0x088e8d14 (`m_auiLevelRange[1]`, `m_auiLevelRange[4]`)
const FIRST_LEVEL: usize = 1;
const LAST_LEVEL: usize = 4;

/// The two triangles of a quad of the strip: vertices `i`, `i + 1` on this sample and `i + 2`,
/// `i + 3` on the next one.
// TODO(RW-10): the table at 0x092142c8 is not in the dump; the pattern of the strips of the mesh
// (0, 1, 8, 1, 9, 8 for a stride of 8) is taken for the stride of 2. The quad is covered either way.
const QUAD_INDICES: [usize; 6] = [0, 1, 2, 1, 3, 2];

/// The surface of the strip: pairs of vertices along the line, in texels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StencilSurface {
    pub vertices: Vec<Vec2>,
    pub indices: Vec<usize>,
    pub bits: u8,
}

impl StencilSurface {
    /// The triangles of the surface.
    pub fn triangles(&self) -> impl Iterator<Item = [Vec2; 3]> + '_ {
        self.indices.chunks_exact(3).map(|t| {
            [
                self.vertices[t[0]],
                self.vertices[t[1]],
                self.vertices[t[2]],
            ]
        })
    }

    /// Adds the surface to the stencil. The stencil has half the texels of the field, so the
    /// corners go to half their value.
    // re: TerrainSystem::Init (TerrainSystem.cpp:752-757, `AddStencil(kSurface.m_Surface, bits, true)`)
    // TODO(RW-10): `TerrainStencil::AddStencil` (0x087fd588) converts the vertices with
    // `Fun_087fdaa4`, not in the dump; the field texel / 2 is the scale of `SampleWorldPosition`.
    pub fn add_to(&self, stencil: &mut TerrainStencil) {
        for t in self.triangles() {
            stencil.add_triangle(t.map(|p| p * 0.5), self.bits);
        }
    }
}

/// `GenerateForestLayoutStencilMesh`.
// re: cvCoastMeshEngine::GenerateForestLayoutStencilMesh 0x088e8d14
pub fn forest_layout_surface(levels: &FinalLevels) -> StencilSurface {
    let n = levels.level_len();
    let inner = levels.level(FIRST_LEVEL);
    let outer = levels.level(LAST_LEVEL);
    let mut vertices = Vec::with_capacity(2 * (n - 1));
    for k in 0..n - 1 {
        let t = k as f32;
        vertices.push(bspline::value_cp(inner, t).pos);
        vertices.push(bspline::value_cp(outer, t).pos);
    }
    // One quad per segment of the line.
    let mut indices = Vec::with_capacity((n - 2) * 6);
    for j in 0..n - 2 {
        indices.extend(QUAD_INDICES.iter().map(|k| k + 2 * j));
    }
    StencilSurface {
        vertices,
        indices,
        bits: FOREST_LAYOUT_BITS,
    }
}

/// `GenerateAllCoastStencil`: one surface for every line with the land on its left.
// re: cvCoastMeshEngine::GenerateAllCoastStencil 0x088e7046
pub fn generate_all(levels: &[FinalLevels]) -> Vec<StencilSurface> {
    levels
        .iter()
        .filter(|l| l.line_type == 0)
        .map(forest_layout_surface)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::levels::tests::island_levels;
    use super::*;

    #[test]
    fn a_surface_is_a_strip_of_two_vertices_per_sample_and_a_quad_per_segment() {
        let (_, levels, _) = island_levels(false);
        let surfaces = generate_all(&levels);
        assert_eq!(
            surfaces.len(),
            levels.iter().filter(|l| l.line_type == 0).count()
        );
        assert!(!surfaces.is_empty());
        for (s, l) in surfaces
            .iter()
            .zip(levels.iter().filter(|l| l.line_type == 0))
        {
            let n = l.level_len();
            assert_eq!(s.vertices.len(), 2 * (n - 1));
            assert_eq!(s.indices.len(), 6 * (n - 2));
            assert_eq!(s.bits, 0x09);
            assert!(s.indices.iter().all(|&i| i < s.vertices.len()));
        }
    }

    #[test]
    fn the_strip_covers_the_texels_from_the_high_beach_to_the_low_side_and_no_others() {
        let (_, levels, _) = island_levels(false);
        let surface = generate_all(&levels).remove(0);
        let b = super::super::testkit::island(20, 20, false, 2);
        let frame = super::super::testkit::frame_of(&b);
        let (w, h) = frame.texel_size();
        let mut stencil = TerrainStencil::new(w, h);
        surface.add_to(&mut stencil);
        // The middle of the island and the far ocean carry no bits.
        let centre = frame.hex_to_tex(10, 10);
        assert_eq!(
            stencil.sample_texel(centre.x as i32, centre.y as i32, 0xff),
            0
        );
        assert_eq!(stencil.sample_texel(5, 5, 0xff), 0);
        // The strip itself: take the middle of a quad.
        let t = surface.triangles().next().unwrap();
        let mid = (t[0] + t[1] + t[2]) / 3.0;
        assert_eq!(stencil.sample_texel(mid.x as i32, mid.y as i32, 0xff), 0x09);
        assert!(stencil.pages_allocated() > 0);
    }

    #[test]
    fn a_line_with_the_land_on_the_right_gets_no_surface() {
        let (_, mut levels, _) = island_levels(false);
        for l in &mut levels {
            l.line_type = 1;
        }
        assert!(generate_all(&levels).is_empty());
    }
}
