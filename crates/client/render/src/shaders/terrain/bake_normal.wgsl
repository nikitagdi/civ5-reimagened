// "Normal Calculation" of the original (world-terrain WT-3): the normals of the cell from its height
// target. `TerrainShader::MakeNormalPS` (a Sobel filter of eight taps) and `TerrainShader::VSFallThrough`
// (the vertex stage that passes the corner and the texture coordinate on). Hand ports in the
// convention of shader-re 4.8.1: a body `fn <entry>_body(inp: In) -> Out`, and an entry.
//
// Needs sm3.wgsl. Group 1 holds the resources, named like the CTAB: the height target of the cell
// `HeightSampler`, the size of its texel `HeightTexelSize` and the scale of the height `HeightScale`.

@group(1) @binding(0) var HeightSampler: texture_2d<f32>;
@group(1) @binding(1) var HeightSampler_smp: sampler;
// (1 / width, 1 / height, 1 / height) of the height target. Float register c0.
@group(1) @binding(2) var<uniform> HeightTexelSize: vec4<f32>;
// The height of the cell in the unit of the horizontal one. Float register c1.
@group(1) @binding(3) var<uniform> HeightScale: vec4<f32>;

// ---- MakeNormalPS ----

struct MakeNormalIn {
    // dcl_texcoord v0.xy: the place in the height target.
    v0: vec4<f32>,
}

struct MakeNormalOut {
    oc0: vec4<f32>,
}

fn height_at(uv: vec2<f32>) -> f32 {
    return textureSample(HeightSampler, HeightSampler_smp, uv).x;
}

// The normal of the height as a tangent-space colour: (-2 gx, 2 gy, 4) normalised, as 0..1. The
// filter is the Sobel one of six taps on each axis; the taps are named after the point of the
// compass in the original's order of reading. The original sets `z` of `HeightTexelSize` to zero
// (TerrainCell.cpp:933), so the taps that step by it are on the middle row or column: the plain Sobel.
// re: TerrainShader::MakeNormalPS (sha1 e7aef5ac5ab8): c2 = (1, -1, 2, -2), c3 = (0.5, 0, 4, 2)
fn make_normal_ps_body(inp: MakeNormalIn) -> MakeNormalOut {
    let t = HeightTexelSize.xyz;
    let uv = inp.v0.xy;
    let h_a = height_at(vec2<f32>(uv.x - t.x, uv.y + t.z));
    let h_b = height_at(vec2<f32>(uv.x - t.x, uv.y - t.y));
    let h_c = height_at(vec2<f32>(uv.x + t.x, uv.y - t.y));
    let h_d = height_at(vec2<f32>(uv.x + t.x, uv.y + t.z));
    let h_e = height_at(vec2<f32>(uv.x - t.x, uv.y + t.y));
    let h_f = height_at(vec2<f32>(uv.x + t.x, uv.y + t.y));
    let h_g = height_at(vec2<f32>(uv.x + t.z, uv.y - t.y));
    let h_h = height_at(vec2<f32>(uv.x + t.z, uv.y + t.y));

    let half_scale = 0.5 * HeightScale.x;
    // The west column against the east one.
    let gx = half_scale * ((h_e - h_f - 2.0 * h_d) + (h_b - h_c + 2.0 * h_a));
    // The south row against the north one.
    let gy = half_scale * ((h_b + h_c + 2.0 * h_g) - (h_e + h_f + 2.0 * h_h));

    let n = vec3<f32>(-(gx * 2.0), -(gy * -2.0), 4.0);
    let inv = d3d_rsq(dot(n, n));
    var o: MakeNormalOut;
    o.oc0 = vec4<f32>(n.x * inv * 0.5 + 0.5, n.y * inv * 0.5 + 0.5, inv * 2.0 + 0.5, 0.0);
    return o;
}

// ---- VSFallThrough ----

struct FallThroughIn {
    // dcl_position v0.
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec4<f32>,
}

struct FallThroughOut {
    // dcl_position o0, dcl_normal o1.xyz, dcl_texcoord o2.
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
}

// The corner as it is (w = 1), a zero normal, the texture coordinate twice.
// re: TerrainShader::VSFallThrough (sha1 ac60879a1630): `mad o0, v0.xyzx, c0.xxxy, c0.yyyx`
fn terrain_vs_fall_through_body(inp: FallThroughIn) -> FallThroughOut {
    var o: FallThroughOut;
    o.o0 = vec4<f32>(inp.v0.xyz, 1.0);
    o.o1 = vec4<f32>(0.0);
    o.o2 = inp.v1.xyxy;
    return o;
}

// ---- entries ----

struct NormalVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec4<f32>,
}

@vertex
fn terrain_vs_fall_through(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> NormalVaryings {
    var b: FallThroughIn;
    b.v0 = v0;
    b.v1 = v1;
    let r = terrain_vs_fall_through_body(b);
    var out: NormalVaryings;
    out.clip = r.o0;
    out.uv = r.o2;
    return out;
}

@fragment
fn make_normal_ps(in: NormalVaryings) -> @location(0) vec4<f32> {
    var b: MakeNormalIn;
    b.v0 = in.uv;
    return make_normal_ps_body(b).oc0;
}
