// The blur of the shadow map: `Shadow::BlurAndProcess` (PS) and `Shadow::VSFallThrough` (VS) of the
// original, hand ports in the convention of shader-re 4.8.1.
//
// Self-contained: no frame globals, no sm3 helpers, so `shader-re compare` needs no `--module`.
//
// Group 0: the constants (`g_ScreenPixelSize`, a member named like the CTAB constant), the raw
// shadow map `ShadowMap` with its sampler. The shadow is the alpha of the raw map; the result is
// the mean of eight bilinear taps, written to every channel.

struct BlurConsts {
    // (1/w, 1/h, -1/w, -1/h) of the raw map.
    g_ScreenPixelSize: vec4<f32>,
}

@group(0) @binding(0) var<uniform> blur_consts: BlurConsts;
@group(0) @binding(1) var ShadowMap: texture_2d<f32>;
@group(0) @binding(2) var ShadowMap_smp: sampler;

// ---- VSFallThrough ----

struct BlurVsIn {
    // dcl_position v0.
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec4<f32>,
}

struct BlurVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
}

// re: Shadow::VSFallThrough (sha1 6c7b9ddc9db1): `mad o0, v0.xyzx, c0.xxxy, c0.yyyx` with
// `def c0, 1, 0, 0, 0`, and the texture coordinate passed on.
fn blur_vs_body(inp: BlurVsIn) -> BlurVsOut {
    var o: BlurVsOut;
    o.o0 = vec4<f32>(inp.v0.xyz, 1.0);
    o.o1 = vec4<f32>(inp.v1.xy, 0.0, 0.0);
    return o;
}

// ---- BlurAndProcess ----

struct BlurPsIn {
    // dcl_texcoord v0.xy.
    v0: vec4<f32>,
}

struct BlurPsOut {
    oc0: vec4<f32>,
}

fn shadow_tap(uv: vec2<f32>) -> f32 {
    return textureSample(ShadowMap, ShadowMap_smp, uv).w;
}

// re: Shadow::BlurAndProcess (sha1 47a739394e40): four taps one texel away on the diagonals, four
// half a texel away; the sum of their alpha times 0.125 (`def c1, 0.5, 0.125, 0, 0`).
fn blur_ps_body(inp: BlurPsIn) -> BlurPsOut {
    let c = blur_consts.g_ScreenPixelSize;
    let uv = inp.v0.xy;
    var sum = shadow_tap(uv + c.xy);
    sum = sum + shadow_tap(uv + c.xw);
    sum = sum + shadow_tap(uv + c.zy);
    sum = sum + shadow_tap(uv + c.zw);
    sum = sum + shadow_tap(uv + c.xy * 0.5);
    sum = sum + shadow_tap(uv + c.xw * 0.5);
    sum = sum + shadow_tap(uv + c.zy * 0.5);
    sum = sum + shadow_tap(uv + c.zw * 0.5);
    var o: BlurPsOut;
    o.oc0 = vec4<f32>(sum * 0.125);
    return o;
}

// ---- entry points ----

struct BlurVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec4<f32>,
}

@vertex
fn blur_vs(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> BlurVertexOut {
    var inp: BlurVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = blur_vs_body(inp);
    var out: BlurVertexOut;
    out.clip = r.o0;
    out.uv = r.o1;
    return out;
}

@fragment
fn blur_ps(in: BlurVertexOut) -> @location(0) vec4<f32> {
    var inp: BlurPsIn;
    inp.v0 = in.uv;
    return blur_ps_body(inp).oc0;
}
