// The cells of `world-terrain` into the screen overlay: the pixel entry that goes with the vertex stage
// of the terrain (`terrain/cell_vs.wgsl`, `TerrainShaderVS`) and the body of `ScreenSpaceOverlayPS`
// (`screen_space_ps.wgsl`). Not a port: the vertex stage of the original hands the pixel stage the
// place in the map of the overlay (`o3`); ours works that out here from the point in the world
// (`o4`), because the matrix of the vertex stage covers the whole field (the fog mask), and the map
// of the overlay is the rectangle the camera sees (`MapView`).
//
// Group 0, binding 0: the map's projection. Groups 1 and 2 are the map and the tile of the cell.

struct MapConsts {
    // `MapView::projection`: a point of the ground (the original's basis) to the clip space of the
    // map. The copy's shift is in the point already (`World` of the vertex stage).
    world_to_map: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> map_consts: MapConsts;

@fragment
fn screenspaceoverlay_cells_ps(in: CellVertexOut) -> @location(0) vec4<f32> {
    let m = map_consts.world_to_map * vec4<f32>(in.v2, 1.0);
    var inp: ScreenSpaceIn;
    inp.v0 = m.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return screenspaceoverlayps_body(inp).oc0;
}
