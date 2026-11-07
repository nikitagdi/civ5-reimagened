// The vertex stage of the trees with the vertices pulled (B3, WebGPU and native; WebGL2 has no
// storage buffers in the vertex stage and keeps `forest_vs`). Joined after `forest.wgsl`.
//
// `forest_vs` runs every variant of the model for each tree and folds the triangles of the others;
// a forest model has 12 variants, a jungle 17. Here a tree runs its own variant's indices alone:
// `pulled_indices` holds the run length (the longest variant's, `stride`) at 0, then one run per
// variant, padded with `PAD`. The draw runs `stride` vertices per tree, so the triangles that show
// reach the rasteriser in the same order as with `forest_vs`, and through the same `forest_vs_core`.

// A vertex of the model: the position in the original's basis and the uv, widened from the f16
// pair of the vertex buffer of `forest_vs` as its vertex fetch widens it.
struct PulledVertex {
    px: f32,
    py: f32,
    pz: f32,
    u: f32,
    v: f32,
}

@group(1) @binding(0) var<storage, read> pulled_vertices: array<PulledVertex>;
@group(1) @binding(1) var<storage, read> pulled_indices: array<u32>;

// The padding of a run that is shorter than the longest.
const PAD: u32 = 0xffffffffu;

struct ForestPulledIn {
    @builtin(vertex_index) vertex: u32,
    @location(4) tree_location: vec4<f32>,
    @location(5) tree_variant: u32,
}

@vertex
fn forest_vs_pulled(v: ForestPulledIn) -> ForestVertexOut {
    var out: ForestVertexOut;
    let stride = pulled_indices[0];
    let i = pulled_indices[1u + v.tree_variant * stride + v.vertex];
    let loc = forest_near_location(v.tree_location);
    if (i == PAD || forest_near_hidden(loc)) {
        // The padding (or a tree at a low eye) folds to one point behind the near plane, as another
        // variant does in `forest_vs`.
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        return out;
    }
    let p = pulled_vertices[i];
    let r = forest_vs_core(
        vec3<f32>(p.px, p.py, p.pz),
        forest_texcoord(vec2<f32>(p.u, p.v)),
        loc,
    );
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.v2 = r.o3;
    out.v3 = r.o4;
    return out;
}
