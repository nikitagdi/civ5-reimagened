// The cutout mask of the terrain (client-hooks, D13): one R8 texel per column of the mod's grid (a
// block is 4 world units, 16 columns to the width of a hex, X wraps at `width * 16`). A texel above
// 0 means the terrain over that column is not drawn. `grid.x` is 0 while no mask is set: the test
// then costs one uniform branch and the picture is the one without the hook.
//
// Group 3, bindings 3 and 4 (`cutout.rs` makes the layout; the ground of B1 and of M1 has its
// material in the same group at bindings 0 to 2). `terrain.wgsl` repeats the block below for the
// plain B1 pipeline; the text of the two must stay the same (`cutout::tests`).

struct CutoutParams {
    // x: 1 when a mask is set. y, z: columns and rows of the mask. w: the block row of mask row 0.
    grid: vec4<f32>,
    // x: 1 when the map wraps along X.
    flags: vec4<f32>,
}
@group(3) @binding(3) var CutoutMask: texture_2d<f32>;
@group(3) @binding(4) var<uniform> cutout: CutoutParams;

// The point (x, z) of our basis lies over a cut column.
fn cutout_hit(x: f32, z: f32) -> bool {
    if (cutout.grid.x == 0.0) {
        return false;
    }
    let cols = i32(cutout.grid.y);
    let rows = i32(cutout.grid.z);
    let bx = i32(floor(x * 0.25));
    let row = i32(floor(z * 0.25)) - i32(cutout.grid.w);
    var col = bx;
    if (cutout.flags.x != 0.0) {
        col = ((bx % cols) + cols) % cols;
    }
    if (col < 0 || col >= cols || row < 0 || row >= rows) {
        return false;
    }
    return textureLoad(CutoutMask, vec2<i32>(col, row), 0).x > 0.0;
}
