// ArrowShader of the original, the movement arrows on the map (world-overlays): `VSArrow` and
// `PSArrow`. Hand ports in the convention of shader-re 4.8.1 (see river.wgsl): each entry of the
// original is a body `fn <entry>_body(inp: In) -> Out` with the registers of the original as
// member names, and a `@vertex` / `@fragment` entry that only reads its inputs and calls the body.
//
// Needs sm3.wgsl. Group 0 binding 0 the constants (the names of the CTAB): `ViewProj` is the
// vertex shader's c0..c3, `alpha` the pixel shader's c0.x and `ambient` its c1.x.
//
// The arrow is a mesh with a normal, a colour and a flag (`v3.x`, 0 where the colour is flat and
// 1 where it is lit by the fixed light from (0.577, -0.577, 0.577) in the swapped y/z of the
// arrow's own space).

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

struct ArrowConsts {
    // 0: the view-projection (`row_major`), c0..c3 of the vertex shader.
    ViewProj: mat4x4<f32>,
    // 64: c0.x of the pixel shader.
    alpha: vec4<f32>,
    // 80: c1.x of the pixel shader.
    ambient: vec4<f32>,
}

@group(0) @binding(0) var<uniform> arrow_consts: ArrowConsts;

// ---- VSArrow ----

struct ArrowVsIn {
    // dcl_position v0 (w used), dcl_normal v1, dcl_color v2, dcl_color1 v3.
    v0: vec4<f32>,
    v1: vec3<f32>,
    v2: vec4<f32>,
    v3: vec4<f32>,
}

struct ArrowVsOut {
    o0: vec4<f32>,
    o1: vec3<f32>,
    o2: vec4<f32>,
    o3: vec4<f32>,
}

// re: ArrowShader::VSArrow (sha1 5dfd35c166d9)
fn vsarrow_body(inp: ArrowVsIn) -> ArrowVsOut {
    let m = arrow_consts.ViewProj;
    let v0 = inp.v0;
    var o: ArrowVsOut;
    var r0 = row_major_reg(m, 1u) * v0.y;
    r0 = v0.x * row_major_reg(m, 0u) + r0;
    r0 = v0.z * row_major_reg(m, 2u) + r0;
    o.o0 = v0.w * row_major_reg(m, 3u) + r0;
    o.o1 = inp.v1;
    o.o2 = inp.v2;
    o.o3 = inp.v3;
    return o;
}

// ---- PSArrow ----

struct ArrowPsIn {
    // dcl_normal v0.xyz, dcl_color v1.xyz, dcl_color1 v2.x.
    v0: vec3<f32>,
    v1: vec3<f32>,
    v2: f32,
}

struct ArrowPsOut {
    oc0: vec4<f32>,
}

// re: ArrowShader::PSArrow (sha1 8048cbde7585)
fn psarrow_body(inp: ArrowPsIn) -> ArrowPsOut {
    var o: ArrowPsOut;
    // def c2 = (0.577350259, -0.577350259, 1, 0)
    // dp3_sat r0.x, v0.xzyw, c2.xxyw
    var r0x = saturate(dot(inp.v0.xzy, vec3<f32>(0.577350259, 0.577350259, -0.577350259)));
    r0x = r0x * inp.v2;
    let k = 1.0 - inp.v2;
    let flat_part = k * inp.v1;
    let lit = r0x * inp.v1 + flat_part;
    o.oc0 = vec4<f32>(inp.v1 * arrow_consts.ambient.x + lit, arrow_consts.alpha.x);
    return o;
}

// ---- the entries ----

struct ArrowVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) flag: vec4<f32>,
}

@vertex
fn vs_arrow(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) flag: vec4<f32>,
) -> ArrowVertex {
    var inp: ArrowVsIn;
    inp.v0 = vec4<f32>(pos, 1.0);
    inp.v1 = normal;
    inp.v2 = color;
    inp.v3 = flag;
    let r = vsarrow_body(inp);
    var out: ArrowVertex;
    out.clip = r.o0;
    out.normal = r.o1;
    out.color = r.o2;
    out.flag = r.o3;
    return out;
}

@fragment
fn ps_arrow(in: ArrowVertex) -> @location(0) vec4<f32> {
    var inp: ArrowPsIn;
    inp.v0 = in.normal;
    inp.v1 = in.color.xyz;
    inp.v2 = in.flag.x;
    return psarrow_body(inp).oc0;
}
