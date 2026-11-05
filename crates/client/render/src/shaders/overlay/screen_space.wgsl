// The cells of the ground into the screen overlay, from plain triangles: the vertex stage of the
// product (not a port) and the entry of `ScreenSpaceOverlayPS` (`screen_space_ps.wgsl` has its body).
// The cells of `world-terrain` go through `cells_ps.wgsl` and the vertex stage of the terrain instead.
// re: TerrainShader::ScreenSpaceOverlayPS (sha1 bb683c6e6475): the entry of this file; the body is `screen_space_ps.wgsl`.
//
// Group 0, binding 0: the vertex constants.

// `world_to_clip` takes a point of our world to the screen, `world_to_map` to the coordinates of the
// map of the overlay (0, 0 top left; the projection `m_mOverlayProj` times the basis change).
struct CellConsts {
    world_to_clip: mat4x4<f32>,
    world_to_map: mat4x4<f32>,
    // The map copy's X shift in our units, added to the vertex.
    shift: vec4<f32>,
}

@group(0) @binding(0) var<uniform> cell_consts: CellConsts;

struct CellVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) map_uv: vec2<f32>,
}

@vertex
fn cell_vs(@location(0) position: vec3<f32>) -> CellVertexOut {
    let p = vec4<f32>(position + cell_consts.shift.xyz, 1.0);
    var out: CellVertexOut;
    out.clip = cell_consts.world_to_clip * p;
    let m = cell_consts.world_to_map * p;
    out.map_uv = m.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return out;
}

@fragment
fn screenspaceoverlayps(in: CellVertexOut) -> @location(0) vec4<f32> {
    var inp: ScreenSpaceIn;
    inp.v0 = in.map_uv;
    return screenspaceoverlayps_body(inp).oc0;
}
