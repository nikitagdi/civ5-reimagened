// The cloud layers of the fluid fog (`FOW` package, `FOWBaseLayer`): `VSFogBaseLayer` and `BasePS`.
// Hand ports in the convention of shader-re 4.8.1 (world-overlays WOV1).
//
// A layer is a set of 64 x 64 tiles at one height, 24 to a draw. The vertex shader places a tile
// (`FOWBaseWVP[e]`, `FOWBaseWorld[e]`, `e` is the vertex's `v1.z`), scrolls the cloud texture
// (`FOWBaseScale`: xy the repeats per world unit, zw the scroll in world units) and finds the
// tile's point in the fog mask through `FOWOverlay`; the pixel shader multiplies the texture's alpha
// by the mask's green channel (the cloud coverage) and drops what is not covered.
//
// Group 0, binding 0: the constants (dynamic offset: one block per draw); group 1: the textures.
// The mask is read with a repeating sampler: the vertex shader offsets its coordinates by -0.5 and
// relies on the wrap.

struct FowBaseConsts {
    // c0 .. c95: element e, register k is `c(4 e + k)`.
    FOWBaseWVP: array<mat4x4<f32>, 24>,
    // c96 .. c191.
    FOWBaseWorld: array<mat4x4<f32>, 24>,
    // c192 .. c195.
    FOWOverlay: mat4x4<f32>,
    // c196.
    FOWBaseScale: vec4<f32>,
}

@group(0) @binding(0) var<uniform> fow_base_consts: FowBaseConsts;

@group(1) @binding(0) var FOWBaseLayerTexture: texture_2d<f32>;
@group(1) @binding(1) var FOWBaseLayerTexture_smp: sampler;
@group(1) @binding(2) var FOWBaseMask: texture_2d<f32>;
@group(1) @binding(3) var FOWBaseMask_smp: sampler;

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// The constant file by register number, c0 .. c196, as the original addresses it (`c1[a0.x]`: the
// index runs over WVP, then World, then the overlay and the scale; clamped like the oracle's).
fn fow_base_reg(idx: i32) -> vec4<f32> {
    let i = u32(clamp(idx, 0, 196));
    if i < 96u {
        return row_major_reg(fow_base_consts.FOWBaseWVP[i / 4u], i % 4u);
    }
    if i < 192u {
        return row_major_reg(fow_base_consts.FOWBaseWorld[(i - 96u) / 4u], i % 4u);
    }
    if i < 196u {
        return row_major_reg(fow_base_consts.FOWOverlay, i - 192u);
    }
    return fow_base_consts.FOWBaseScale;
}

struct FowBaseVsIn {
    // dcl_position v0: the corner of the tile.
    v0: vec4<f32>,
    // dcl_normal v1: z is the index of the tile among the 24 of the draw.
    v1: vec4<f32>,
}

struct FowBaseVsOut {
    o0: vec4<f32>,
    // The cloud texture's coordinates.
    o1: vec2<f32>,
    o2: vec3<f32>,
    // The mask's coordinates.
    o3: vec2<f32>,
}

// re: FOW::VSFogBaseLayer (sha1 b6f3d5ec8647)
fn fogbaselayer_vs_body(inp: FowBaseVsIn) -> FowBaseVsOut {
    let a0 = i32(floor(floor(inp.v1.z) * 4.0 + 0.5));
    let scale = fow_base_consts.FOWBaseScale;
    let overlay = fow_base_consts.FOWOverlay;
    var o: FowBaseVsOut;
    // The clip position.
    var r = inp.v0.y * fow_base_reg(a0 + 1);
    r = inp.v0.x * fow_base_reg(a0 + 0) + r;
    r = inp.v0.z * fow_base_reg(a0 + 2) + r;
    o.o0 = r + fow_base_reg(a0 + 3);
    // The world position of the corner.
    var w = inp.v0.y * fow_base_reg(a0 + 97).xyz;
    w = inp.v0.x * fow_base_reg(a0 + 96).xyz + w;
    w = inp.v0.z * fow_base_reg(a0 + 98).xyz + w;
    w = w + fow_base_reg(a0 + 99).xyz;
    // The cloud texture: scrolled, then repeated.
    o.o1 = (w.xy + scale.zw) * scale.xy;
    // The mask: the world position through the overlay projection, to texture space.
    let o1 = row_major_reg(overlay, 1u);
    let ry = vec2<f32>(w.y * o1.x, w.y * o1.y);
    var p = w.x * row_major_reg(overlay, 0u).xy + ry;
    p = w.z * row_major_reg(overlay, 2u).xy + p;
    p = p + row_major_reg(overlay, 3u).xy;
    o.o3 = p * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5);
    o.o2 = vec3<f32>(0.0);
    return o;
}

struct FowBasePsIn {
    // dcl_texcoord v0.xy: the cloud texture; v1.xy: the mask.
    v0: vec2<f32>,
    v1: vec2<f32>,
}

struct FowBasePsOut {
    oc0: vec4<f32>,
    kill: bool,
}

// re: FOW::BasePS (sha1 311bec4adaae): the coverage is the mask's green channel; a pixel with
// 0.01 or less is dropped, else the cloud texture's alpha is multiplied by it.
fn baseps_body(inp: FowBasePsIn) -> FowBasePsOut {
    let m = textureSample(FOWBaseMask, FOWBaseMask_smp, inp.v1);
    let covered = m.y + -0.01;
    let t = textureSample(FOWBaseLayerTexture, FOWBaseLayerTexture_smp, inp.v0);
    var o: FowBasePsOut;
    o.kill = !(covered >= 0.0);
    o.oc0 = vec4<f32>(t.xyz, m.y * t.w);
    return o;
}

struct FowBaseVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) cloud_uv: vec2<f32>,
    @location(1) mask_uv: vec2<f32>,
}

@vertex
fn fogbaselayer_vs(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> FowBaseVertexOut {
    var inp: FowBaseVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = fogbaselayer_vs_body(inp);
    var out: FowBaseVertexOut;
    out.clip = r.o0;
    out.cloud_uv = r.o1;
    out.mask_uv = r.o3;
    return out;
}

@fragment
fn baseps(in: FowBaseVertexOut) -> @location(0) vec4<f32> {
    var inp: FowBasePsIn;
    inp.v0 = in.cloud_uv;
    inp.v1 = in.mask_uv;
    let r = baseps_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}
