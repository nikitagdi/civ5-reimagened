// "Decal Layer" of the original (world-terrain WT-6): the decals of the area of a cell, drawn into the
// albedo and height targets of the cell before the normals are made. `TerrainShader::DecalVSCompressed`
// (the vertex stage: a vertex is a place in the bounds of the decal and a texture coordinate),
// `TerrainShader::DecalPS` (albedo and the height as a signed mix) and `TerrainShader::DecalPSSpec`
// (the same and the highlight). Hand ports in the convention of shader-re 4.8.1: a body
// `fn <entry>_body(inp: In) -> Out` and an entry that only reads its inputs.
//
// Needs sm3.wgsl. Group 1 holds the textures of the decal, group 2 the constants of the vertex stage;
// both are named like the CTAB (the typo `Albiedo` is the original's).

@group(1) @binding(0) var DecalAlbiedoSampler: texture_2d<f32>;
@group(1) @binding(1) var DecalAlbiedoSampler_smp: sampler;
@group(1) @binding(2) var DecalHeightSampler: texture_2d<f32>;
@group(1) @binding(3) var DecalHeightSampler_smp: sampler;
@group(1) @binding(4) var DecalSpecSampler: texture_2d<f32>;
@group(1) @binding(5) var DecalSpecSampler_smp: sampler;

// The constants of the vertex stage. `DecalTransform` is a `row_major float4x4`: register `k` of it is
// the vector `(m[0][k], m[1][k], m[2][k], m[3][k])` (`decal_reg`).
struct DecalVsConsts {
    // c5: the corner (xy) and the opposite corner (zw) of the decal; the vertex is a place between them.
    DecalBounds: vec4<f32>,
    // c4: the scale (xy) and the move (zw) of the texture coordinate.
    DecalTextureScales: vec4<f32>,
    // c0..c3: the place of the decal to the clip space of the page.
    DecalTransform: mat4x4<f32>,
    // c6.x: the depth the decal is drawn at.
    zDist: f32,
}
@group(2) @binding(0) var<uniform> decal_vs_consts: DecalVsConsts;

fn decal_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// ---- DecalVSCompressed ----

struct DecalVsIn {
    // dcl_position v0: the place of the vertex in the bounds of the decal, 0..1.
    v0: vec4<f32>,
    // dcl_texcoord v1: the texture coordinate.
    v1: vec4<f32>,
}

struct DecalVsOut {
    // dcl_position o0, dcl_texcoord o2.xy; the others are written zero.
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec4<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
}

// re: TerrainShader::DecalVSCompressed (sha1 1f02afbecc20): `def c7 = (1, 0, 0, 0)`
fn decal_vs_compressed_body(inp: DecalVsIn) -> DecalVsOut {
    let c = decal_vs_consts;
    // `lrp`: v0 * (c5.zw - c5.xy) + c5.xy.
    var r0_xy = inp.v0.xy * (c.DecalBounds.zw - c.DecalBounds.xy) + c.DecalBounds.xy;
    r0_xy = r0_xy + vec2<f32>(1.0);
    var r0 = r0_xy.y * decal_reg(c.DecalTransform, 1u);
    r0 = r0_xy.x * decal_reg(c.DecalTransform, 0u) + r0;
    r0 = c.zDist * decal_reg(c.DecalTransform, 2u) + r0;
    var o: DecalVsOut;
    o.o0 = r0 + decal_reg(c.DecalTransform, 3u);
    o.o1 = vec4<f32>(0.0);
    o.o2 = vec4<f32>(inp.v1.xy * c.DecalTextureScales.xy + c.DecalTextureScales.zw, 0.0, 0.0);
    o.o3 = vec4<f32>(0.0);
    o.o4 = vec4<f32>(0.0);
    o.o5 = vec4<f32>(0.0);
    return o;
}

struct DecalVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn decal_vs_compressed(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> DecalVertexOut {
    var b: DecalVsIn;
    b.v0 = v0;
    b.v1 = v1;
    let r = decal_vs_compressed_body(b);
    var out: DecalVertexOut;
    out.clip = r.o0;
    out.uv = r.o2.xy;
    return out;
}

// ---- DecalPS, DecalPSSpec ----

struct DecalPsIn {
    // dcl_texcoord v0.xy: the place in the textures of the decal.
    v0: vec4<f32>,
}

struct DecalPsOut {
    // The albedo, the height (a signed mix: the sign picks the side, the alpha is its size), and for
    // `DecalPSSpec` the highlight.
    oc0: vec4<f32>,
    oc1: vec4<f32>,
    oc2: vec4<f32>,
}

// The height of the decal as the target holds it. `r0.y` is the signed part: from 0.5 up the colour is
// the height itself, below it is turned over; the alpha is how far from 0.5 it is, doubled.
// re: TerrainShader::DecalPS (sha1 e3625359ea30): c0 = (1, 2, 0, -1), c1 = (2, -0.9999, 1, -1)
fn decal_height(h: vec4<f32>) -> vec4<f32> {
    let sel = h.y * 2.0 - 0.99989998;
    let r1 = h.xxxy * vec4<f32>(1.0, 1.0, 1.0, 2.0) + vec4<f32>(0.0, 0.0, 0.0, -1.0);
    let r2 = h.xxxy * vec4<f32>(-1.0, -1.0, -1.0, -2.0) + vec4<f32>(1.0);
    return d3d_cmp(vec4<f32>(sel), r1, r2);
}

fn decal_ps_body(inp: DecalPsIn) -> DecalPsOut {
    var o: DecalPsOut;
    o.oc0 = textureSample(DecalAlbiedoSampler, DecalAlbiedoSampler_smp, inp.v0.xy);
    let h = textureSample(DecalHeightSampler, DecalHeightSampler_smp, inp.v0.xy);
    o.oc1 = decal_height(h);
    o.oc2 = vec4<f32>(0.0);
    return o;
}

// re: TerrainShader::DecalPSSpec (sha1 7d8988c5b07a)
fn decal_ps_spec_body(inp: DecalPsIn) -> DecalPsOut {
    var o: DecalPsOut;
    o.oc0 = textureSample(DecalAlbiedoSampler, DecalAlbiedoSampler_smp, inp.v0.xy);
    o.oc2 = textureSample(DecalSpecSampler, DecalSpecSampler_smp, inp.v0.xy);
    let h = textureSample(DecalHeightSampler, DecalHeightSampler_smp, inp.v0.xy);
    o.oc1 = decal_height(h);
    return o;
}

struct DecalFragmentOut {
    @location(0) albedo: vec4<f32>,
    @location(1) height: vec4<f32>,
}

struct DecalFragmentSpecOut {
    @location(0) albedo: vec4<f32>,
    @location(1) height: vec4<f32>,
    @location(2) spec: vec4<f32>,
}

// The albedo of a decal in the space of the albedo target. The composite squares its textures
// (`TerrainShaderScreenPS`, gamma 2.0) and the bake lights the target as linear light; `DecalPS` writes
// its texture as it reads it, so the original reads it decoded (a view or sampler state the captures
// cannot show: they hold no bake). Read raw, the decals are a gamma too bright: pale grey river banks,
// washed-out flood plains (`floodplains_D.dds` is a saturated yellow green that the square takes to the
// green of the original). Here the entries decode with the composite's square; the alpha is the
// coverage and stays. The macro shadow reads its texture raw (an amount, not a colour).
fn decal_albedo(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.xyz * c.xyz, c.w);
}

@fragment
fn decal_ps(@location(0) uv: vec2<f32>) -> DecalFragmentOut {
    var b: DecalPsIn;
    b.v0 = vec4<f32>(uv, 0.0, 0.0);
    let r = decal_ps_body(b);
    var out: DecalFragmentOut;
    out.albedo = decal_albedo(r.oc0);
    out.height = r.oc1;
    return out;
}

@fragment
fn decal_ps_spec(@location(0) uv: vec2<f32>) -> DecalFragmentSpecOut {
    var b: DecalPsIn;
    b.v0 = vec4<f32>(uv, 0.0, 0.0);
    let r = decal_ps_spec_body(b);
    var out: DecalFragmentSpecOut;
    out.albedo = decal_albedo(r.oc0);
    out.height = r.oc1;
    out.spec = r.oc2;
    return out;
}

// "Decal Macro Shadow": the same pixel stage into the one target of the macro shadow (the albedo of the
// decal is its shadow). Not an entry of the original: `DecalPS` with its second output left out.
@fragment
fn decal_ps_shadow(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    var b: DecalPsIn;
    b.v0 = vec4<f32>(uv, 0.0, 0.0);
    return decal_ps_body(b).oc0;
}
