// The cull layer of the fluid fog (`FOW` package, `FOWDrawMask` subset 3): `VSFowMaskCull`,
// `VSFowMaskCullShaded`, `CullMask` and `CullMaskShaded`. Hand ports in the convention of
// shader-re 4.8.1 (world-overlays WOV1).
//
// The original draws one quad per cell where no unrevealed cell is within two steps, with the clip
// depth forced to 0, so the cloud layers that follow fail the depth test there. The product does
// not draw it (the cloud tiles over those cells are left out on the CPU, `fow/cull.rs`: the main
// depth buffer is shared with what draws after the fog); the shaders are kept as the reference of
// the port and for the debug view.
//
// Group 0, binding 0: `WVP[24]` as in `mask.wgsl`; group 1: the mask the shaded variant shows.

struct FowCullConsts {
    // Register k of element e: `c(4 e + k)`.
    WVP: array<mat4x4<f32>, 24>,
}

@group(0) @binding(0) var<uniform> fow_cull_consts: FowCullConsts;

@group(1) @binding(0) var FOWBaseMask: texture_2d<f32>;
@group(1) @binding(1) var FOWBaseMask_smp: sampler;

fn fow_cull_reg(idx: i32) -> vec4<f32> {
    let i = u32(clamp(idx, 0, 95));
    let m = fow_cull_consts.WVP[i / 4u];
    let k = i % 4u;
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

struct FowCullVsIn {
    // dcl_position v0.
    v0: vec4<f32>,
    // dcl_normal v1: z is the index of the quad among the 24 of the draw.
    v1: vec4<f32>,
}

struct FowCullVsOut {
    o0: vec4<f32>,
}

struct FowCullShadedVsOut {
    o0: vec4<f32>,
    o1: vec2<f32>,
    o2: vec3<f32>,
    o3: vec2<f32>,
}

// The position without a depth: `r0.xyz = (x, y, w)` of the sum of the four registers (`.xyww`),
// the depth `o0.z` is 0.
fn cull_position(v0: vec4<f32>, v1: vec4<f32>) -> vec4<f32> {
    let a0 = i32(floor(floor(v1.z) * 4.0 + 0.5));
    var r = v0.y * fow_cull_reg(a0 + 1).xyw;
    r = v0.x * fow_cull_reg(a0 + 0).xyw + r;
    r = v0.z * fow_cull_reg(a0 + 2).xyw + r;
    let c3 = fow_cull_reg(a0 + 3);
    return vec4<f32>(r.x + c3.x, r.y + c3.y, 0.0, r.z + c3.w);
}

// re: FOW::VSFowMaskCull (sha1 fba35bfdeb15)
fn fowmaskcull_vs_body(inp: FowCullVsIn) -> FowCullVsOut {
    var o: FowCullVsOut;
    o.o0 = cull_position(inp.v0, inp.v1);
    return o;
}

// re: FOW::VSFowMaskCullShaded (sha1 effd8bc5e162)
fn fowmaskcullshaded_vs_body(inp: FowCullVsIn) -> FowCullShadedVsOut {
    var o: FowCullShadedVsOut;
    o.o0 = cull_position(inp.v0, inp.v1);
    o.o1 = vec2<f32>(0.0);
    o.o2 = vec3<f32>(0.0);
    o.o3 = vec2<f32>(0.0);
    return o;
}

struct FowCullPsIn {
    // dcl_texcoord1 v0.xy: the position in the mask.
    v0: vec2<f32>,
}

struct FowCullPsOut {
    oc0: vec4<f32>,
}

struct FowCullFlatIn {
    // The original reads no input.
    pad: f32,
}

// re: FOW::CullMask (sha1 27f861de2216): writes 0 to every channel.
fn cullmask_ps_body(inp: FowCullFlatIn) -> FowCullPsOut {
    var o: FowCullPsOut;
    o.oc0 = vec4<f32>(0.0);
    return o;
}

// re: FOW::CullMaskShaded (sha1 c1be76d4ab6a): the mask's colour with a constant 0.5 alpha.
fn cullmaskshaded_ps_body(inp: FowCullPsIn) -> FowCullPsOut {
    let t = textureSample(FOWBaseMask, FOWBaseMask_smp, inp.v0);
    var o: FowCullPsOut;
    o.oc0 = vec4<f32>(t.xyz, 0.5);
    return o;
}

struct FowCullVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn fowmaskcull_vs(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> FowCullVertexOut {
    var inp: FowCullVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    var out: FowCullVertexOut;
    out.clip = fowmaskcull_vs_body(inp).o0;
    out.uv = vec2<f32>(0.0);
    return out;
}

@vertex
fn fowmaskcullshaded_vs(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> FowCullVertexOut {
    var inp: FowCullVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = fowmaskcullshaded_vs_body(inp);
    var out: FowCullVertexOut;
    out.clip = r.o0;
    out.uv = r.o3;
    return out;
}

@fragment
fn cullmask_ps(in: FowCullVertexOut) -> @location(0) vec4<f32> {
    var inp: FowCullFlatIn;
    inp.pad = 0.0;
    return cullmask_ps_body(inp).oc0;
}

@fragment
fn cullmaskshaded_ps(in: FowCullVertexOut) -> @location(0) vec4<f32> {
    var inp: FowCullPsIn;
    inp.v0 = in.uv;
    return cullmaskshaded_ps_body(inp).oc0;
}
