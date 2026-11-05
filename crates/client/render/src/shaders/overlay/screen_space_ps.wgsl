// The pixel stage of the cells of the ground in the screen overlay (`TerrainSystem::RenderOverlay`,
// world-overlays WOV2): `TerrainShader::ScreenSpaceOverlayPS`.
//
// The pixel shader of the original is one `texld`: it reads the map of the overlay at the
// coordinates its vertex shader worked out and writes what it finds. The vertices are those of the
// ground's cells with their relief (`cell_geometry()` of `world-terrain`), so the flat map of the
// overlay is laid over the hills and the result, in screen space, is what the final shader of the
// ground reads as `TerrainOverlay`.
//
// Group 1: the map of the overlay and its sampler.

@group(1) @binding(0) var TerrainOverlay: texture_2d<f32>;
@group(1) @binding(1) var TerrainOverlay_smp: sampler;

struct ScreenSpaceIn {
    // dcl_texcoord1 v0.xy: the place in the map of the overlay.
    v0: vec2<f32>,
}

struct ScreenSpaceOut {
    oc0: vec4<f32>,
}

// re: TerrainShader::ScreenSpaceOverlayPS (sha1 bb683c6e6475)
fn screenspaceoverlayps_body(inp: ScreenSpaceIn) -> ScreenSpaceOut {
    var o: ScreenSpaceOut;
    o.oc0 = textureSample(TerrainOverlay, TerrainOverlay_smp, inp.v0);
    return o;
}
