// "HeightMap" of the original: the composite of up to eight terrain textures of a cell
// (world-terrain WT-3). `TerrainShader::TerrainShaderScreenPS`: eight albedo textures and eight
// height textures, each weighted by its weight in the vertex times its alpha to the 8th power (the
// dominance), into two targets: the albedo and the height of the cell. Hand port in the convention of
// shader-re 4.8.1 (a body `fn <entry>_body(inp: In) -> Out` and a `@fragment` entry).
//
// Needs sm3.wgsl. Resources carry the names of the CTAB; the registers of an array are named
// `AlbiedoSampler30` (s0), `tex1_AlbiedoSampler30` (s1) ... as the oracle names them. Group 1
// holds the 16 textures and their samplers.
//
// Gamma: the albedo is squared (gamma 2.0 of the textures); the target is raw.

@group(1) @binding(0) var AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(1) var AlbiedoSampler30_smp: sampler;
@group(1) @binding(2) var tex1_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(3) var tex1_AlbiedoSampler30_smp: sampler;
@group(1) @binding(4) var tex2_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(5) var tex2_AlbiedoSampler30_smp: sampler;
@group(1) @binding(6) var tex3_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(7) var tex3_AlbiedoSampler30_smp: sampler;
@group(1) @binding(8) var tex4_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(9) var tex4_AlbiedoSampler30_smp: sampler;
@group(1) @binding(10) var tex5_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(11) var tex5_AlbiedoSampler30_smp: sampler;
@group(1) @binding(12) var tex6_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(13) var tex6_AlbiedoSampler30_smp: sampler;
@group(1) @binding(14) var tex7_AlbiedoSampler30: texture_2d<f32>;
@group(1) @binding(15) var tex7_AlbiedoSampler30_smp: sampler;
@group(1) @binding(16) var HeightSampler30: texture_2d<f32>;
@group(1) @binding(17) var HeightSampler30_smp: sampler;
@group(1) @binding(18) var tex9_HeightSampler30: texture_2d<f32>;
@group(1) @binding(19) var tex9_HeightSampler30_smp: sampler;
@group(1) @binding(20) var tex10_HeightSampler30: texture_2d<f32>;
@group(1) @binding(21) var tex10_HeightSampler30_smp: sampler;
@group(1) @binding(22) var tex11_HeightSampler30: texture_2d<f32>;
@group(1) @binding(23) var tex11_HeightSampler30_smp: sampler;
@group(1) @binding(24) var tex12_HeightSampler30: texture_2d<f32>;
@group(1) @binding(25) var tex12_HeightSampler30_smp: sampler;
@group(1) @binding(26) var tex13_HeightSampler30: texture_2d<f32>;
@group(1) @binding(27) var tex13_HeightSampler30_smp: sampler;
@group(1) @binding(28) var tex14_HeightSampler30: texture_2d<f32>;
@group(1) @binding(29) var tex14_HeightSampler30_smp: sampler;
@group(1) @binding(30) var tex15_HeightSampler30: texture_2d<f32>;
@group(1) @binding(31) var tex15_HeightSampler30_smp: sampler;

struct CompositePsIn {
    // dcl_texcoord .. dcl_texcoord3 v0..v3: the place in textures 0..3 (xy) and the weight of
    // textures 0..3 (v0.zw, v1.zw) and 4..7 (v2.zw, v3.zw).
    v0: vec4<f32>,
    v1: vec4<f32>,
    v2: vec4<f32>,
    v3: vec4<f32>,
    // dcl_texcoord4 .. dcl_texcoord7 v4..v7.xy: the place in textures 4..7.
    v4: vec2<f32>,
    v5: vec2<f32>,
    v6: vec2<f32>,
    v7: vec2<f32>,
}

struct CompositePsOut {
    // The albedo of the cell, and the height (in all four components).
    oc0: vec4<f32>,
    oc1: vec4<f32>,
}

// re: TerrainShader::TerrainShaderScreenPS (sha1 63a33c1f1bee)
fn terrain_shader_screen_ps_body(inp: CompositePsIn) -> CompositePsOut {
    // The first four textures (the albedo is squared; its alpha is the dominance, to the 8th).
    var a1 = textureSample(tex1_AlbiedoSampler30, tex1_AlbiedoSampler30_smp, inp.v1.xy);
    a1 = a1 * a1;
    var a0 = textureSample(AlbiedoSampler30, AlbiedoSampler30_smp, inp.v0.xy);
    a0 = a0 * a0;
    var a2 = textureSample(tex2_AlbiedoSampler30, tex2_AlbiedoSampler30_smp, inp.v2.xy);
    a2 = a2 * a2;
    var a3 = textureSample(tex3_AlbiedoSampler30, tex3_AlbiedoSampler30_smp, inp.v3.xy);
    a3 = a3 * a3;
    var d_lo = vec4<f32>(a0.w, a1.w, a2.w, a3.w);
    d_lo = d_lo * d_lo;
    d_lo = d_lo * d_lo;
    let w_lo = vec4<f32>(inp.v0.zw, inp.v1.zw);
    d_lo = d_lo * w_lo;
    var color = a1 * d_lo.y;
    color = d_lo.x * a0 + color;
    color = d_lo.z * a2 + color;
    color = d_lo.w * a3 + color;

    // The last four.
    var a4 = textureSample(tex4_AlbiedoSampler30, tex4_AlbiedoSampler30_smp, inp.v4);
    a4 = a4 * a4;
    var a5 = textureSample(tex5_AlbiedoSampler30, tex5_AlbiedoSampler30_smp, inp.v5);
    a5 = a5 * a5;
    var a6 = textureSample(tex6_AlbiedoSampler30, tex6_AlbiedoSampler30_smp, inp.v6);
    a6 = a6 * a6;
    var a7 = textureSample(tex7_AlbiedoSampler30, tex7_AlbiedoSampler30_smp, inp.v7);
    a7 = a7 * a7;
    var d_hi = vec4<f32>(a4.w, a5.w, a6.w, a7.w);
    d_hi = d_hi * d_hi;
    d_hi = d_hi * d_hi;
    let w_hi = vec4<f32>(inp.v2.zw, inp.v3.zw);
    d_hi = d_hi * w_hi;
    color = d_hi.x * a4 + color;
    color = d_hi.y * a5 + color;
    color = d_hi.z * a6 + color;
    color = d_hi.w * a7 + color;

    // The sum of the dominances normalises the albedo and the height.
    let sum = dot(d_lo, vec4<f32>(1.0)) + dot(d_hi, vec4<f32>(1.0));
    let inv = d3d_rcp(sum);

    let h1 = textureSample(tex9_HeightSampler30, tex9_HeightSampler30_smp, inp.v1.xy).x;
    var h = h1 * d_lo.y;
    let h0 = textureSample(HeightSampler30, HeightSampler30_smp, inp.v0.xy).x;
    h = d_lo.x * h0 + h;
    let h2 = textureSample(tex10_HeightSampler30, tex10_HeightSampler30_smp, inp.v2.xy).x;
    h = d_lo.z * h2 + h;
    let h3 = textureSample(tex11_HeightSampler30, tex11_HeightSampler30_smp, inp.v3.xy).x;
    h = d_lo.w * h3 + h;
    let h4 = textureSample(tex12_HeightSampler30, tex12_HeightSampler30_smp, inp.v4).x;
    h = d_hi.x * h4 + h;
    let h5 = textureSample(tex13_HeightSampler30, tex13_HeightSampler30_smp, inp.v5).x;
    h = d_hi.y * h5 + h;
    let h6 = textureSample(tex14_HeightSampler30, tex14_HeightSampler30_smp, inp.v6).x;
    h = d_hi.z * h6 + h;
    let h7 = textureSample(tex15_HeightSampler30, tex15_HeightSampler30_smp, inp.v7).x;
    h = d_hi.w * h7 + h;

    var o: CompositePsOut;
    o.oc0 = color * inv;
    o.oc1 = vec4<f32>(inv * h);
    return o;
}

// ---- the highlight composite ----

// The highlight of a cell ("HeightMap" with `SpecTemp`, world-terrain 4.7): the specular maps of the rows
// (`TerrainDef.csv` Specmap: marsh, snow, tundra, mesa, mountains, coast), blended with the dominances of
// the albedo composite. The original's composite with the highlight is the DX11-only
// `TerrainSpecShaderScreenPS40`, which the oracle has no bytecode of (shader-re status `todo`): this is a
// reconstruction, the same weights as `TerrainShaderScreenPS` over the specular maps, read raw (as
// `DecalPSSpec` reads the highlight of a decal). The pipeline binds the specular map of each slot where the
// composite binds its height (bindings 16..31); a row without one binds black: no highlight.
// SIMPLIFIED(SIMP-063): the highlight composite is a reconstruction (no bytecode of the original's)
fn terrain_spec_composite_ps_body(inp: CompositePsIn) -> vec4<f32> {
    let a0 = textureSample(AlbiedoSampler30, AlbiedoSampler30_smp, inp.v0.xy).w;
    let a1 = textureSample(tex1_AlbiedoSampler30, tex1_AlbiedoSampler30_smp, inp.v1.xy).w;
    let a2 = textureSample(tex2_AlbiedoSampler30, tex2_AlbiedoSampler30_smp, inp.v2.xy).w;
    let a3 = textureSample(tex3_AlbiedoSampler30, tex3_AlbiedoSampler30_smp, inp.v3.xy).w;
    let a4 = textureSample(tex4_AlbiedoSampler30, tex4_AlbiedoSampler30_smp, inp.v4).w;
    let a5 = textureSample(tex5_AlbiedoSampler30, tex5_AlbiedoSampler30_smp, inp.v5).w;
    let a6 = textureSample(tex6_AlbiedoSampler30, tex6_AlbiedoSampler30_smp, inp.v6).w;
    let a7 = textureSample(tex7_AlbiedoSampler30, tex7_AlbiedoSampler30_smp, inp.v7).w;
    // The dominance as the albedo composite has it: the alpha to the 8th times the weight.
    var d_lo = vec4<f32>(a0, a1, a2, a3);
    d_lo = d_lo * d_lo;
    d_lo = d_lo * d_lo;
    d_lo = d_lo * d_lo;
    d_lo = d_lo * vec4<f32>(inp.v0.zw, inp.v1.zw);
    var d_hi = vec4<f32>(a4, a5, a6, a7);
    d_hi = d_hi * d_hi;
    d_hi = d_hi * d_hi;
    d_hi = d_hi * d_hi;
    d_hi = d_hi * vec4<f32>(inp.v2.zw, inp.v3.zw);
    var s = d_lo.x * textureSample(HeightSampler30, HeightSampler30_smp, inp.v0.xy);
    s = d_lo.y * textureSample(tex9_HeightSampler30, tex9_HeightSampler30_smp, inp.v1.xy) + s;
    s = d_lo.z * textureSample(tex10_HeightSampler30, tex10_HeightSampler30_smp, inp.v2.xy) + s;
    s = d_lo.w * textureSample(tex11_HeightSampler30, tex11_HeightSampler30_smp, inp.v3.xy) + s;
    s = d_hi.x * textureSample(tex12_HeightSampler30, tex12_HeightSampler30_smp, inp.v4) + s;
    s = d_hi.y * textureSample(tex13_HeightSampler30, tex13_HeightSampler30_smp, inp.v5) + s;
    s = d_hi.z * textureSample(tex14_HeightSampler30, tex14_HeightSampler30_smp, inp.v6) + s;
    s = d_hi.w * textureSample(tex15_HeightSampler30, tex15_HeightSampler30_smp, inp.v7) + s;
    let sum = dot(d_lo, vec4<f32>(1.0)) + dot(d_hi, vec4<f32>(1.0));
    return s * d3d_rcp(sum);
}

// ---- TerrainShaderScreenVS: the vertex stage of the composite ----

// Registers of the vertex stage. `g_fTextureMultiplierAndProjMode` is indexed by the slot of the
// texture (the `aTextures` of the vertex): `.x` the multiplier of its scale (`TerrainDef` scale after
// the snap of the cell), `.y` its projection mode (0 from above, 1 and 2 on the sides).
struct ScreenVsConsts {
    // Float register c35: the half texel of the page that moves the vertex on the screen.
    g_TerrainScreenTexelOffsets: vec2<f32>,
    // Float registers c109..c204.
    g_fTextureMultiplierAndProjMode: array<vec4<f32>, 96>,
}
@group(2) @binding(0) var<uniform> screen_vs_consts: ScreenVsConsts;

struct ScreenVsIn {
    // dcl_position v0: the corner of the cell in the clip space of the page (z is passed on).
    v0: vec4<f32>,
    // dcl_texcoord1 v1: the point of the world (x, y, height).
    v1: vec4<f32>,
    // dcl_texcoord2 v2, dcl_texcoord3 v3: the weights of the eight textures.
    v2: vec4<f32>,
    v3: vec4<f32>,
    // dcl_blendindices v4, v5: the slots of the eight textures.
    v4: vec4<f32>,
    v5: vec4<f32>,
}

struct ScreenVsOut {
    // dcl_position o0, dcl_color o1 and o2 (zero), dcl_texcoord o3..o6: the place in textures 0..3
    // and the weights, o7..o10: the place in textures 4..7.
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec4<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
    o6: vec4<f32>,
    o7: vec4<f32>,
    o8: vec4<f32>,
    o9: vec4<f32>,
    o10: vec4<f32>,
}

// `c109[a0.k]` with the relative address of D3D9: a slot out of the array reads zero.
fn texture_mult_and_mode(slot: i32) -> vec4<f32> {
    if slot < 0 || slot >= 96 {
        return vec4<f32>(0.0);
    }
    return screen_vs_consts.g_fTextureMultiplierAndProjMode[slot];
}

// The place of a point of the world in a texture: the projection from above takes (x, y) with the
// scale snapped to 0.33, the sides take (x, z) or (y, z) with 0.66. Here `p` is `v1`, with the height
// in `z`; mode 1 takes (x, z), mode 2 (y, z).
// re: TerrainShader::TerrainShaderScreenVS (sha1 acc72e60604d): `c0 = (-1, -2, 0.33, 0.66)`,
// `if_ge -|r.z|, |r.z|` is the test `mode - 1 == 0`
fn texture_place(slot: i32, p: vec4<f32>) -> vec2<f32> {
    let m = texture_mult_and_mode(slot);
    var uv: vec2<f32>;
    if (-1.0 + m.y) == 0.0 {
        uv = vec2<f32>(p.x * m.x, p.z * m.x) * 0.660000026;
    } else {
        uv = vec2<f32>(p.x * m.x, p.y * m.x) * 0.330000013;
    }
    if (-2.0 + m.y) == 0.0 {
        uv = vec2<f32>(p.y * m.x, p.z * m.x) * 0.660000026;
    }
    return uv;
}

// `mova`: the nearest integer.
fn mova(v: vec4<f32>) -> vec4<i32> {
    return vec4<i32>(floor(v + vec4<f32>(0.5)));
}

fn terrain_shader_screen_vs_body(inp: ScreenVsIn) -> ScreenVsOut {
    let a = mova(inp.v4);
    let b = mova(inp.v5);
    let p = inp.v1;
    var o: ScreenVsOut;
    o.o0 = vec4<f32>(
        inp.v0.x + screen_vs_consts.g_TerrainScreenTexelOffsets.x,
        inp.v0.y + screen_vs_consts.g_TerrainScreenTexelOffsets.y,
        inp.v0.z,
        1.0,
    );
    o.o1 = vec4<f32>(0.0);
    o.o2 = vec4<f32>(0.0);
    o.o3 = vec4<f32>(texture_place(a.x, p), inp.v2.x, inp.v2.y);
    o.o4 = vec4<f32>(texture_place(a.y, p), inp.v2.z, inp.v2.w);
    o.o5 = vec4<f32>(texture_place(a.z, p), inp.v3.x, inp.v3.y);
    o.o6 = vec4<f32>(texture_place(a.w, p), inp.v3.z, inp.v3.w);
    o.o7 = vec4<f32>(texture_place(b.x, p), 0.0, 0.0);
    o.o8 = vec4<f32>(texture_place(b.y, p), 0.0, 0.0);
    o.o9 = vec4<f32>(texture_place(b.z, p), 0.0, 0.0);
    o.o10 = vec4<f32>(texture_place(b.w, p), 0.0, 0.0);
    return o;
}

// ---- entry: the interface of the vertex stage of the composite (`TerrainShaderScreenVS`) ----

struct CompositeVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec4<f32>,
    @location(3) v3: vec4<f32>,
    @location(4) v4: vec2<f32>,
    @location(5) v5: vec2<f32>,
    @location(6) v6: vec2<f32>,
    @location(7) v7: vec2<f32>,
}

@vertex
fn terrain_shader_screen_vs(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec4<f32>,
    @location(3) v3: vec4<f32>,
    @location(4) v4: vec4<f32>,
    @location(5) v5: vec4<f32>,
) -> CompositeVertexOut {
    var b: ScreenVsIn;
    b.v0 = v0;
    b.v1 = v1;
    b.v2 = v2;
    b.v3 = v3;
    b.v4 = v4;
    b.v5 = v5;
    let r = terrain_shader_screen_vs_body(b);
    var out: CompositeVertexOut;
    out.clip = r.o0;
    out.v0 = r.o3;
    out.v1 = r.o4;
    out.v2 = r.o5;
    out.v3 = r.o6;
    out.v4 = r.o7.xy;
    out.v5 = r.o8.xy;
    out.v6 = r.o9.xy;
    out.v7 = r.o10.xy;
    return out;
}

// ---- entry: the pixel stage of the composite ----

struct CompositeVaryings {
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec4<f32>,
    @location(3) v3: vec4<f32>,
    @location(4) v4: vec2<f32>,
    @location(5) v5: vec2<f32>,
    @location(6) v6: vec2<f32>,
    @location(7) v7: vec2<f32>,
}

struct CompositeTargets {
    @location(0) albedo: vec4<f32>,
    @location(1) height: vec4<f32>,
}

@fragment
fn terrain_shader_screen_ps(in: CompositeVaryings) -> CompositeTargets {
    var b: CompositePsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    b.v4 = in.v4;
    b.v5 = in.v5;
    b.v6 = in.v6;
    b.v7 = in.v7;
    let r = terrain_shader_screen_ps_body(b);
    var o: CompositeTargets;
    o.albedo = r.oc0;
    o.height = r.oc1;
    return o;
}

@fragment
fn terrain_spec_composite_ps(in: CompositeVaryings) -> @location(0) vec4<f32> {
    var b: CompositePsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    b.v4 = in.v4;
    b.v5 = in.v5;
    b.v6 = in.v6;
    b.v7 = in.v7;
    return terrain_spec_composite_ps_body(b);
}
