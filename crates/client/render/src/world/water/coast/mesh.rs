//! The mesh of a coast: `GenerateFinalMesh`. A mesh is four strips between
//! the five levels of a coastline, eight vertices per sample of the lines; its triangles are cut
//! into batches of whole segments. [`super::blend`] rasterizes the triangles into the field.

use glam::Vec2;

use super::bspline;
use super::levels::{FinalLevels, StyleSet, StyleWalk};
use super::styles::STRIPS;

/// The two triangles of a quad of a strip: vertices `i`, `i + 1` on this sample and `i + 8`,
/// `i + 9` on the next one.
// re: g_aCoastQuadIndices 0x092142b0 (cvCoastMeshEngine.cpp:24)
pub const QUAD_INDICES: [u16; 6] = [0, 1, 8, 1, 9, 8];

/// Vertices per sample: an inner and an outer one per strip.
pub const VERTICES_PER_SAMPLE: usize = 2 * STRIPS;

/// Triangles per segment: two per strip.
pub const TRIANGLES_PER_SEGMENT: usize = 2 * STRIPS;

/// Triangles of the first batch of a mesh.
// re: GenerateFinalMesh 0x088e8614 (`uiTrisPerBatch = 64`)
const FIRST_BATCH_TRIANGLES: usize = 64;
/// The batch grows by this many triangles while the count of batches is odd and above 1.
// re: GenerateFinalMesh 0x088e8614 (`uiTrisPerBatch += 8`)
const BATCH_GROWTH: usize = 8;

/// The weight of the three edges of a triangle in the quadratic patch of the blend: per segment
/// the eight triangles are (0,0,0), (1,0,0), four times (1,1,1), (0,0,1), (0,0,0).
// re: GenerateFinalMesh 0x088e8614 (`field_18.push_back(kZero, kX, kOne x4, kZ, kZero)`)
const EDGE_WEIGHTS: [[f32; 3]; TRIANGLES_PER_SEGMENT] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, 0.0],
];

/// A vertex of a coast mesh.
// re: class CoastMeshVertex (cvCoastMeshEngine.h:45)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshVertex {
    pub pos: Vec2,
    /// Row of the table of the terrains, [`NO_TERRAIN`] for none.
    pub terrain: u8,
    /// Height as a fraction of 65535.
    pub height: f32,
    /// Weight of the vertex in the blend: 0 on the outer levels, 1 on the inner three.
    pub weight: f32,
}

/// A run of whole segments of a mesh: its vertices and indices are a slice of the mesh's.
// re: class CoastMeshBatch (cvCoastMeshEngine.h:58)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub first_vertex: usize,
    pub num_vertices: usize,
    pub first_index: usize,
    pub num_triangles: usize,
}

/// The mesh of one coastline.
// re: class CoastMesh (cvCoastMeshEngine.h:69)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoastMesh {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u16>,
    /// Eight per segment: the weights of the three edges of every triangle.
    pub edge_weights: Vec<[f32; 3]>,
    pub batches: Vec<Batch>,
}

impl CoastMesh {
    pub fn batch_vertices(&self, b: &Batch) -> &[MeshVertex] {
        &self.vertices[b.first_vertex..b.first_vertex + b.num_vertices]
    }

    pub fn batch_indices(&self, b: &Batch) -> &[u16] {
        &self.indices[b.first_index..b.first_index + b.num_triangles * 3]
    }
}

/// The number of triangles of a batch and the number of batches for a mesh of `triangles`
/// triangles: batches start at 64 triangles and grow by 8 while their number is odd and above 1
/// or the last one would be shorter than 8.
// re: GenerateFinalMesh 0x088e8614 (cvCoastMeshEngine.cpp:275-282)
pub fn batch_plan(triangles: usize) -> (usize, usize) {
    let mut per_batch = FIRST_BATCH_TRIANGLES;
    loop {
        let batches = (triangles.max(1) - 1) / per_batch + 1;
        let remainder = triangles % per_batch;
        if !((batches > 1 && batches % 2 == 1) || (remainder != 0 && remainder < 8)) {
            return (per_batch, batches);
        }
        per_batch += BATCH_GROWTH;
    }
}

/// `GenerateFinalMesh`: the vertices, the indices of the four strips and the batches of one set
/// of levels.
// re: cvCoastMeshEngine::GenerateFinalMesh 0x088e8614 (cvCoastMeshEngine.cpp:266-382)
pub fn generate_final_mesh(levels: &FinalLevels, set: &StyleSet) -> CoastMesh {
    let n = levels.level_len();
    let samples = n - 1;
    let segments = n - 2;
    let triangles = segments * TRIANGLES_PER_SEGMENT;
    let mut vertices = Vec::with_capacity(samples * VERTICES_PER_SAMPLE);

    let mut walk = StyleWalk::new(set);
    for i in 0..samples {
        let t = i as f32;
        for strip in 0..STRIPS {
            let inner = bspline::value_cp(levels.level(strip), t);
            let blend = walk.at(inner.style_a, inner.style_b, inner.blend);
            let inv = 1.0 - blend;
            let terrain = (if blend > 0.5 { &walk.a } else { &walk.b }).terrain[strip];
            let (a, b) = (&walk.a, &walk.b);
            vertices.push(MeshVertex {
                pos: inner.pos,
                terrain,
                height: b.levels[strip].height * inv + a.levels[strip].height * blend,
                weight: b.levels[strip].value * inv + a.levels[strip].value * blend,
            });
            let outer = bspline::value_cp(levels.level(strip + 1), t);
            vertices.push(MeshVertex {
                pos: outer.pos,
                terrain,
                height: b.levels[strip + 1].height * inv + a.levels[strip + 1].height * blend,
                weight: b.levels[strip + 1].value * inv + a.levels[strip + 1].value * blend,
            });
        }
    }

    let (per_batch, num_batches) = batch_plan(triangles);
    let mut batches = vec![Batch::default(); num_batches];
    let mut indices = Vec::with_capacity(segments * 24);
    let mut edge_weights = Vec::with_capacity(triangles);
    let (mut batch, mut first_vertex, mut first_index, mut base_vertex) = (0usize, 0, 0, 0u16);
    batches[0] = Batch {
        first_vertex: 0,
        num_vertices: VERTICES_PER_SAMPLE,
        first_index: 0,
        num_triangles: 0,
    };
    for _ in 0..segments {
        if batches[batch].num_triangles == per_batch {
            batch += 1;
            batches[batch] = Batch {
                first_vertex,
                num_vertices: VERTICES_PER_SAMPLE,
                first_index,
                num_triangles: 0,
            };
            base_vertex = 0;
        }
        for strip in 0..STRIPS as u16 {
            indices.extend(QUAD_INDICES.iter().map(|k| k + base_vertex + strip * 2));
        }
        first_vertex += VERTICES_PER_SAMPLE;
        first_index += 24;
        base_vertex += VERTICES_PER_SAMPLE as u16;
        batches[batch].num_vertices += VERTICES_PER_SAMPLE;
        batches[batch].num_triangles += TRIANGLES_PER_SEGMENT;
        edge_weights.extend_from_slice(&EDGE_WEIGHTS);
    }
    CoastMesh {
        vertices,
        indices,
        edge_weights,
        batches,
    }
}

#[cfg(test)]
mod tests {
    use super::super::levels::NO_TERRAIN;
    use super::super::levels::tests::island_levels;
    use super::*;
    use crate::world::terrain::sampler::SEA_LEVEL_RAW;

    #[test]
    fn batches_start_at_64_triangles_and_grow_while_their_count_is_odd() {
        // 8 triangles per segment; a line of 4 points has levels of 18 points: 16 segments,
        // 128 triangles: two batches of 64.
        assert_eq!(batch_plan(128), (64, 2));
        // 192 triangles are three batches of 64: odd, so the batch grows until the count is even.
        assert_eq!(batch_plan(192), (96, 2));
        assert_eq!(batch_plan(8), (64, 1));
        for t in (8..4000).step_by(8) {
            let (per, n) = batch_plan(t);
            assert!(n == 1 || n % 2 == 0, "{t}: {n} batches");
            assert!(per >= 64 && per % 8 == 0);
            assert!(n * per >= t);
        }
    }

    fn vertices_of(triangles: usize) -> usize {
        triangles / TRIANGLES_PER_SEGMENT * VERTICES_PER_SAMPLE + VERTICES_PER_SAMPLE
    }

    #[test]
    fn a_mesh_has_eight_vertices_per_sample_and_24_indices_per_segment() {
        let (splines, levels, set) = island_levels(false);
        for (s, l) in splines.iter().zip(&levels) {
            let m = generate_final_mesh(l, &set);
            let n = 8 * s.points.len() - 14;
            assert_eq!(m.vertices.len(), 8 * n - 8);
            assert_eq!(m.indices.len(), 24 * (n - 2));
            assert_eq!(m.edge_weights.len(), 8 * (n - 2));
            // The batches cover the triangles once and each indexes inside its own vertices.
            let tris: usize = m.batches.iter().map(|b| b.num_triangles).sum();
            assert_eq!(tris, 8 * (n - 2));
            for b in &m.batches {
                let max = m.batch_indices(b).iter().copied().max().unwrap();
                assert!(usize::from(max) < b.num_vertices);
                assert_eq!(b.num_vertices, vertices_of(b.num_triangles));
            }
        }
    }

    #[test]
    fn the_waterline_vertices_have_the_height_of_the_water_and_the_outer_ones_a_weight_of_zero() {
        let (_, levels, set) = island_levels(false);
        let m = generate_final_mesh(&levels[0], &set);
        for sample in m.vertices.chunks(VERTICES_PER_SAMPLE) {
            // Vertex 2 * strip is the inner one of the strip, 2 * strip + 1 the outer one:
            // level l is the outer vertex of strip l - 1 and the inner vertex of strip l.
            let raw = |v: &MeshVertex| (v.height * 65535.0).round() as u16;
            assert_eq!(raw(&sample[4]), SEA_LEVEL_RAW);
            assert_eq!(raw(&sample[3]), raw(&sample[4]));
            // Level 0 and level 4 are the outer levels: weight 0; the three inner ones weight 1.
            assert_eq!(sample[0].weight, 0.0);
            assert_eq!(sample[7].weight, 0.0);
            assert!((sample[2].weight - 1.0).abs() < 1e-6);
            assert!((sample[5].weight - 1.0).abs() < 1e-6);
            // Terrain: strips 0 and 3 have none, the high beach strip has the row of type 5.
            assert_eq!(sample[0].terrain, NO_TERRAIN);
            assert_eq!(sample[6].terrain, NO_TERRAIN);
            assert_ne!(sample[2].terrain, NO_TERRAIN);
        }
    }

    #[test]
    fn the_edge_weights_repeat_a_pattern_of_eight_per_segment() {
        let (_, levels, set) = island_levels(false);
        let m = generate_final_mesh(&levels[0], &set);
        for seg in m.edge_weights.chunks(8) {
            assert_eq!(seg, EDGE_WEIGHTS.as_slice());
        }
    }
}
