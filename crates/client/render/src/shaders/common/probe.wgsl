// Entry points over the common files: they make `shaders::catalog()` a module that `shader-re
// validate` can translate for WebGL2, and the GPU test of `sm3.wgsl` runs them. Not drawn in the
// game. Needs frame.wgsl, sm3.wgsl, gamma.wgsl.
//
// The zero for the special cases is read at run time (`copy_offset.y`, always 0), so no compiler
// can fold the instructions at compile time.

// pipeline-overridable constants
override PROBE_SCALE: f32 = 1.0;
override PROBE_ON: bool = false;

@vertex
fn vs_fullscreen(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// (pow(0, 0), rcp(0), rsq(0), log(0))
@fragment
fn fs_sm3_a() -> @location(0) vec4<f32> {
    let z = view_globals.copy_offset.y;
    return vec4<f32>(d3d_pow(z, z), d3d_rcp(z), d3d_rsq(z), d3d_log(z));
}

// (nrm(0).x, nrm(0).y, pow(-2, 2), rcp(4))
@fragment
fn fs_sm3_b() -> @location(0) vec4<f32> {
    let z = view_globals.copy_offset.y;
    let n = d3d_nrm(vec4<f32>(z, z, z, z));
    return vec4<f32>(n.x, n.y, d3d_pow(z - 2.0, z + 2.0), d3d_rcp(z + 4.0));
}

// cmp over (-1, 0, 1, NaN) between 10 and 20: (20, 10, 10, 20)
@fragment
fn fs_sm3_c() -> @location(0) vec4<f32> {
    let z = view_globals.copy_offset.y;
    let nan_bits = 0x7fc00000u + u32(z);
    let s0 = vec4<f32>(-1.0, z, 1.0, bitcast<f32>(nan_bits));
    return d3d_cmp(s0, vec4<f32>(10.0), vec4<f32>(20.0));
}

// gamma round trip of (0.25, 0.5, 0.75): the terrain curve and the model curve, back again
@fragment
fn fs_gamma() -> @location(0) vec4<f32> {
    let c = vec3<f32>(0.25, 0.5, 0.75) + view_globals.copy_offset.y;
    let t = terrain_to_gamma(terrain_to_linear(c));
    let m = model_to_gamma(model_to_linear(c));
    return vec4<f32>(t.x, m.y, t.z, 1.0);
}

// what the overridable constants are
@fragment
fn fs_override() -> @location(0) vec4<f32> {
    return vec4<f32>(PROBE_SCALE, select(0.0, 1.0, PROBE_ON), 0.0, 1.0);
}
