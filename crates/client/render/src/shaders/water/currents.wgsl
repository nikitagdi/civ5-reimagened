// WorldCurrents of the original (world-water WW-2, R2.16): the map of the currents of the ocean,
// 256 x 256, drawn once by `WaterSystem::RenderOceanCurrentMap` (RW-11) with `PSCombined` and read
// by the water as `WaterBlendMap`. The five entries are hand ports in the convention of shader-re
// 4.8.1: each is a body `fn <entry>_body(inp: In) -> Out` with the registers of the original as
// member names and an entry that only reads its inputs and calls the body.
//
// Needs sm3.wgsl. Group 0: binding 0 the constants (the 64 bytes of `FireGrafix::WorldCurrentsData`:
// one register each), binding 1 and 2 the height map (`TerrainHeight`, read as its red channel).
//
// The red and green channels of the result are the current, as `(0.5 - gy / 2, 0.5 + gx / 2)` of the
// gradient `g` of the heights times `FlowStrength`, the blue channel is 0 and the alpha its length;
// `PSCombined` mixes in a noise (`PSCurlNoise`) where the gradient is short.

struct CurrentsConsts {
    // c0: the size of the map in hexes, which a hex of the height map is one step of.
    @size(16) GridSize: vec2<f32>,
    // c1
    @size(16) NoiseStrength: f32,
    // c2: the cells of the noise are `NoiseScale` hexes wide.
    @size(16) NoiseScale: f32,
    // c3
    @size(16) FlowStrength: f32,
}

@group(0) @binding(0) var<uniform> currents_consts: CurrentsConsts;
@group(0) @binding(1) var TerrainHeight: texture_2d<f32>;
@group(0) @binding(2) var TerrainHeight_smp: sampler;

// `frc` of the original: `x - floor(x)`.
fn frc4(x: vec4<f32>) -> vec4<f32> {
    return x - floor(x);
}

// ---- VSFallThrough ----

struct FallThroughVsIn {
    // dcl_position v0: a corner of the screen, (-1, -1) to (1, 1).
    v0: vec3<f32>,
    // dcl_texcoord v1
    v1: vec2<f32>,
}

struct FallThroughVsOut {
    o0: vec4<f32>,
    o1: vec2<f32>,
}

// re: WorldCurrents::VSFallThrough (sha1 6c7b9ddc9db1)
fn vsfallthrough_body(inp: FallThroughVsIn) -> FallThroughVsOut {
    var o: FallThroughVsOut;
    o.o0 = vec4<f32>(inp.v0, 1.0);
    o.o1 = inp.v1;
    return o;
}

// ---- PSRiverCurrent ----

struct RiverCurrentPsIn {
    v0: vec2<f32>,
}

struct CurrentsPsOut {
    oc0: vec4<f32>,
}

// re: WorldCurrents::PSRiverCurrent (sha1 f29747ef33b9)
fn psrivercurrent_body(inp: RiverCurrentPsIn) -> CurrentsPsOut {
    var o: CurrentsPsOut;
    o.oc0 = vec4<f32>(
        inp.v0.x * -0.5 + 0.5,
        inp.v0.y * 0.5 + 0.5,
        0.0,
        1.0,
    );
    return o;
}

// ---- the gradient of the heights ----

// The Sobel gradient of the height map at `uv`: the eight neighbours one hex away (`GridSize` hexes
// across the map), (`gx`, `gy`); the sign of `gy` is such that the current flows around the land.
fn currents_gradient(uv: vec2<f32>) -> vec2<f32> {
    let grid = currents_consts.GridSize;
    let dx = d3d_rcp(grid.x);
    let dy = d3d_rcp(grid.y);
    let h_e = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(dx, 0.0)).x;
    let h_ne = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(dx, dy)).x;
    let h_nw = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(-dx, dy)).x;
    let east = dot(vec3<f32>(h_ne, h_nw, h_e), vec3<f32>(-1.0, 1.0, -2.0));
    let h_w = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(-dx, 0.0)).x;
    let h_se = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(dx, -dy)).x;
    let h_sw = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(-dx, -dy)).x;
    let h_s = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(0.0, -dy)).x;
    let west = dot(vec3<f32>(h_sw, h_se, h_w), vec3<f32>(1.0, -1.0, 2.0));
    let south = dot(vec3<f32>(h_sw, h_se, h_s), vec3<f32>(1.0, 1.0, 2.0));
    let h_n = textureSample(TerrainHeight, TerrainHeight_smp, uv + vec2<f32>(0.0, dy)).x;
    let north = dot(vec3<f32>(h_nw, h_ne, h_n), vec3<f32>(-1.0, -1.0, -2.0));
    return vec2<f32>(east + west, north + south);
}

// ---- PSCurrent ----

struct CurrentPsIn {
    v0: vec2<f32>,
}

// re: WorldCurrents::PSCurrent (sha1 492e367d06ef)
fn pscurrent_body(inp: CurrentPsIn) -> CurrentsPsOut {
    var o: CurrentsPsOut;
    let g = currents_gradient(inp.v0) * currents_consts.FlowStrength;
    let len2 = dot(g, g);
    let half = vec3<f32>(g.y, g.x, g.y) * 0.5;
    o.oc0 = vec4<f32>(
        half.x * -1.0 + 0.5,
        half.y * 1.0 + 0.5,
        half.z * 0.0 + 0.0,
        d3d_rcp(d3d_rsq(len2)),
    );
    return o;
}

// ---- the noise ----

// The gradient noise of `PSCurlNoise`: the map is cut into `GridSize / NoiseScale` cells a side,
// the corner of each gets a pseudo random sign from a hash of its index; returns the noise before
// `NoiseStrength`, in [-1, 1] about.
fn currents_noise(uv: vec2<f32>) -> vec2<f32> {
    let grid = currents_consts.GridSize;
    var r0 = d3d_rcp(currents_consts.NoiseScale) * vec4<f32>(grid.y, grid.y, grid.x, grid.x);
    var r1 = frc4(r0.yyww);
    r0 = r0 + (-r1);
    let inv_ny = d3d_rcp(r0.y);
    let scaled = vec2<f32>(r0.w * uv.x, r0.y * uv.y);
    var r2 = vec4<f32>(scaled.x - floor(scaled.x), scaled.y - floor(scaled.y), 0.0, 0.0);
    var r3 = vec4<f32>(uv.y, uv.y, uv.x, uv.x) * r0 + (-r2.yyxx);
    let inv_nx = d3d_rcp(r0.w);
    r3 = r3 + vec4<f32>(0.0, 1.0, 0.0, 1.0);
    var r4 = r3.xyxy * 17.0;
    r1 = vec4<f32>(inv_ny) * r4;
    r1 = frc4(r1);
    r1 = r1 * r1;
    r1 = r1 * 3721.0 + r3.zzww;
    r1 = r1 * 13.0;
    r0 = vec4<f32>(inv_nx) * r1;
    r0 = frc4(r0);
    r0 = r0 * r0;
    r1 = r0 * 930.25;
    r0 = r0 * 1860.5;
    r0 = frc4(r0);
    r0 = r0 + -0.5;
    r1 = frc4(r1);
    r1 = r1 + -0.5;
    r3 = d3d_cmp(-r1, vec4<f32>(0.0), vec4<f32>(1.0));
    r1 = d3d_cmp(r1, vec4<f32>(-0.0), vec4<f32>(-1.0));
    r1 = r1 + r3;
    r3 = r2.xxyy + (-vec4<f32>(0.0, 1.0, 0.0, 1.0));
    r4 = r1 * r3.zwzw;
    var r5 = d3d_cmp(-r0, vec4<f32>(0.0), vec4<f32>(1.0));
    r0 = d3d_cmp(r0, vec4<f32>(-0.0), vec4<f32>(-1.0));
    r0 = r0 + r5;
    r3 = r3.xxyy * r0 + r4;
    r3 = (-r3.xyxz) + r3.zwyw;
    let zw = r2.xyxy * -6.0 + 6.0;
    r2 = vec4<f32>(r2.x, r2.y, zw.z, zw.w);
    r4 = r2.xyxy * r2;
    let fade = r2 * -2.0 + 3.0;
    r2 = vec4<f32>(fade.x, fade.y, r2.z, r2.w);
    let faded = r2 * r4;
    r2 = vec4<f32>(faded.x, faded.y, r2.z, r2.w);
    let top = r2.x * (r0.zwzw - r0) + r0;
    let bottom = r2.yyyy * (r1.xyyw - r1.xyxz) + r1.xyxz;
    r5 = vec4<f32>(top.x, top.y, bottom.z, bottom.w);
    r0 = r3 * r4.zzww + r5;
    let mixed = r2 * (r0.wyzw - r0.zxzw) + r0.zxzw;
    return mixed.xy;
}

// ---- PSCurlNoise ----

struct CurlNoisePsIn {
    v0: vec2<f32>,
}

// re: WorldCurrents::PSCurlNoise (sha1 0932b5de0669)
fn pscurlnoise_body(inp: CurlNoisePsIn) -> CurrentsPsOut {
    var o: CurrentsPsOut;
    let n = currents_noise(inp.v0) * currents_consts.NoiseStrength;
    o.oc0 = vec4<f32>(n.x * -0.5 + 0.5, n.y * 0.5 + 0.5, 0.0, 1.0);
    return o;
}

// ---- PSCombined ----

struct CombinedPsIn {
    v0: vec2<f32>,
}

// re: WorldCurrents::PSCombined (sha1 3b7cd0b362fe)
fn pscombined_body(inp: CombinedPsIn) -> CurrentsPsOut {
    var o: CurrentsPsOut;
    // The noise, as a colour (0.5 - n.x / 2, 0.5 + n.y / 2, 0, 1).
    var noise = vec4<f32>(currents_noise(inp.v0), 0.0, 0.0) * currents_consts.NoiseStrength;
    noise = noise * 0.5;
    noise = noise * vec4<f32>(-1.0, 1.0, 0.0, 0.0) + vec4<f32>(0.5, 0.5, 0.0, 1.0);
    // The current, as in `PSCurrent`.
    let g = currents_gradient(inp.v0) * currents_consts.FlowStrength;
    let half = vec3<f32>(g.y, g.x, g.y) * 0.5;
    let len = d3d_rcp(d3d_rsq(dot(g, g)));
    let cur = vec4<f32>(
        half.x * -1.0 + 0.5,
        half.y * 1.0 + 0.5,
        half.z * 0.0 + 0.0,
        len,
    );
    let t = saturate(len);
    o.oc0 = t * (cur - noise) + noise;
    return o;
}

// ---- the entries ----

struct CurrentsVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vsfallthrough(@location(0) v0: vec3<f32>, @location(1) v1: vec2<f32>) -> CurrentsVertex {
    var inp: FallThroughVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = vsfallthrough_body(inp);
    var out: CurrentsVertex;
    out.clip = r.o0;
    out.uv = r.o1;
    return out;
}

@fragment
fn psrivercurrent(in: CurrentsVertex) -> @location(0) vec4<f32> {
    var inp: RiverCurrentPsIn;
    inp.v0 = in.uv;
    return psrivercurrent_body(inp).oc0;
}

@fragment
fn pscurrent(in: CurrentsVertex) -> @location(0) vec4<f32> {
    var inp: CurrentPsIn;
    inp.v0 = in.uv;
    return pscurrent_body(inp).oc0;
}

@fragment
fn pscurlnoise(in: CurrentsVertex) -> @location(0) vec4<f32> {
    var inp: CurlNoisePsIn;
    inp.v0 = in.uv;
    return pscurlnoise_body(inp).oc0;
}

@fragment
fn pscombined(in: CurrentsVertex) -> @location(0) vec4<f32> {
    var inp: CombinedPsIn;
    inp.v0 = in.uv;
    return pscombined_body(inp).oc0;
}
