// WaveShader of the original, the shore waves (world-water): `VS_Wave` and `PS_Wave`. Hand ports in
// the convention of shader-re 4.8.1 (see river.wgsl): each entry of the original is a body
// `fn <entry>_body(inp: In) -> Out` with the registers of the original as member names, and a
// `@vertex` / `@fragment` entry that only reads its inputs and calls the body.
//
// Needs sm3.wgsl. Group 0: binding 0 the constants of the vertex shader (the names of the CTAB, one
// register or matrix each: c0..c3 `g_ViewProjMatrix`, c4 `g_vWaveData`, c5 `g_vWaveFadeParams`, c6
// `g_vTexturePageSize`), binding 1 the table `WaveData` (c7..c254), binding 2 the colour of the
// pixel shader (c0), then the wave texture of s0.
//
// A wave is a quad of four vertices; the vertex carries the corner (`v0`, half floats), the
// texture coordinate of the corner (`v1.xy` as unorm: the corner in [0, 1]) and the pair of the
// atlas cell (`v1.zw`), and the number `v2.x` of the wave's slot in `WaveData` (each slot: the
// offset in the atlas xy and the age z). The wave grows with its age (`g_vWaveData.x` to `.y` by
// `.z * age`), fades in and out by `g_vWaveFadeParams`.
//
// Gamma: the colour is squared out (1 / 2.2); the texture is read through an sRGB view (the original's
// sampler 3 decodes it, re-b16 W1), so the texel is linear here.

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

struct WaveVsConsts {
    // c0..c3
    g_ViewProjMatrix: mat4x4<f32>,
    // c4
    g_vWaveData: vec4<f32>,
    // c5
    g_vWaveFadeParams: vec4<f32>,
    // c6 (xy used)
    g_vTexturePageSize: vec4<f32>,
}

struct WavePsConsts {
    g_vColor: vec4<f32>,
}

@group(0) @binding(0) var<uniform> wave_vs: WaveVsConsts;
// c7..c254.
@group(0) @binding(1) var<uniform> WaveData: array<vec4<f32>, 248>;
@group(0) @binding(2) var<uniform> wave_ps: WavePsConsts;
@group(0) @binding(3) var g_WaveTexture: texture_2d<f32>;
@group(0) @binding(4) var g_WaveTexture_smp: sampler;

// ---- VS_Wave ----

struct WaveVsIn {
    // dcl_position v0: (x, y, 0, 1), only xy is read.
    v0: vec2<f32>,
    // dcl_texcoord v1.
    v1: vec4<f32>,
    // dcl_blendindices v2: the slot (x).
    v2: f32,
}

struct WaveVsOut {
    o0: vec2<f32>,
    o1: vec2<f32>,
    o2: vec4<f32>,
}

// re: WaveShader::VS_Wave (sha1 799a8edd24a8)
fn vs_wave_body(inp: WaveVsIn) -> WaveVsOut {
    let c = wave_vs;
    var o: WaveVsOut;
    // mova a0.x, v2.x: rounded to the nearest; the register file is clamped as the oracle does.
    let a0 = i32(floor(inp.v2 + 0.5));
    var r0 = WaveData[clamp(a0, 0, 247)].xyz;
    o.o0 = c.g_vTexturePageSize.xy * inp.v1.zw + r0.xy;
    var r0xy = vec2<f32>(r0.z) - vec2<f32>(c.g_vWaveFadeParams.x, c.g_vWaveFadeParams.z);
    let r0z = r0.z * c.g_vWaveData.z;
    // lrp r1.x, r0.z, c4.y, c4.x
    let r1x = r0z * (c.g_vWaveData.y - c.g_vWaveData.x) + c.g_vWaveData.x;
    r0xy = saturate(r0xy * vec2<f32>(c.g_vWaveFadeParams.y, c.g_vWaveFadeParams.w));
    let r0y = -r0xy.y + 1.0;
    o.o1 = vec2<f32>(r0y * r0xy.x);
    var p = inp.v1.xy * 2.0 + vec2<f32>(-1.0);
    p = p * -r1x + inp.v0;
    let m = c.g_ViewProjMatrix;
    var r1 = p.y * row_major_reg(m, 1u);
    var q = p.x * row_major_reg(m, 0u) + r1;
    q = c.g_vWaveData.w * row_major_reg(m, 2u) + q;
    o.o2 = q + row_major_reg(m, 3u);
    return o;
}

// ---- PS_Wave ----

struct WavePsIn {
    // dcl_texcoord v0.xy, dcl_texcoord1 v1.x.
    v0: vec2<f32>,
    v1: f32,
}

struct WavePsOut {
    oc0: vec4<f32>,
}

// re: WaveShader::PS_Wave (sha1 f228f4f63ef9)
fn ps_wave_body(inp: WavePsIn) -> WavePsOut {
    var o: WavePsOut;
    let t = textureSample(g_WaveTexture, g_WaveTexture_smp, inp.v0);
    var r0 = t.xyz * inp.v1;
    r0 = r0 * wave_ps.g_vColor.xyz;
    let l = vec3<f32>(d3d_log(r0.x), d3d_log(r0.y), d3d_log(r0.z));
    let e = l * 0.454545468;
    o.oc0 = vec4<f32>(exp2(e.x), exp2(e.y), exp2(e.z), 1.0);
    return o;
}

// ---- the entries ----

struct WaveVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) t0: vec2<f32>,
    @location(1) fade: vec2<f32>,
}

@vertex
fn vs_wave(
    @location(0) pos: vec2<f32>,
    @location(1) tex: vec4<f32>,
    @location(2) slot: vec4<u32>,
) -> WaveVertex {
    var inp: WaveVsIn;
    inp.v0 = pos;
    inp.v1 = tex;
    inp.v2 = f32(slot.x);
    let r = vs_wave_body(inp);
    var out: WaveVertex;
    out.clip = r.o2;
    out.t0 = r.o0;
    out.fade = r.o1;
    return out;
}

@fragment
fn ps_wave(in: WaveVertex) -> @location(0) vec4<f32> {
    var inp: WavePsIn;
    inp.v0 = in.t0;
    inp.v1 = in.fade.x;
    return ps_wave_body(inp).oc0;
}
