// The fog-of-war mask of the original (`FOW` package, `FOWDrawMask`): `VSFowMask` and the pixel
// shaders `Visible`, `VisibleFluid`, `Transparent` and `SolidPS`. Hand ports in the convention of
// shader-re 4.8.1 (world-overlays WOV1): each entry of the original is a body `fn <entry>_body(inp)`
// with the registers as member names; the `@vertex` / `@fragment` entries only read their inputs,
// call the body and hand the result on.
//
// The mask is drawn per hex, 24 hexes to a draw: `WVP` holds the 24 matrices (`row_major
// float4x4 WVP[24]`, register k of element e is row k), the vertex's `v1.x` is the index of its
// hex and `v2.x` its weight (1 in the middle, 0 at the rim). The pixel shaders write the weight to
// the channels of the mask: `Transparent` (w,0,0,1), `Visible` (w,0,w,1), `VisibleFluid` (0,w,0,1).
//
// Group 0, binding 0: the constants (dynamic offset: one block of 24 matrices per draw).

struct FowMaskConsts {
    // Register k of element e: `c(4 e + k)`.
    WVP: array<mat4x4<f32>, 24>,
}

@group(0) @binding(0) var<uniform> fow_mask_consts: FowMaskConsts;

// The register `idx` of `WVP` (the original addresses `c<k>[a0.x]`; the oracle clamps the index to
// the constant file, so does this).
fn fow_wvp_reg(idx: i32) -> vec4<f32> {
    let i = u32(clamp(idx, 0, 95));
    let m = fow_mask_consts.WVP[i / 4u];
    let k = i % 4u;
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// ---- VSFowMask ----

struct FowMaskVsIn {
    // dcl_position v0: the corner in the plane of the hex.
    v0: vec4<f32>,
    // dcl_normal v1: x is the index of the hex among the 24 of the draw.
    v1: vec4<f32>,
    // dcl_texcoord v2: x is the weight.
    v2: vec4<f32>,
}

struct FowMaskVsOut {
    o0: vec4<f32>,
    o1: vec2<f32>,
    o2: vec3<f32>,
    o3: vec2<f32>,
}

// re: FOW::VSFowMask (sha1 293adcac99b0): `frc`/`add`/`mul c96.x` make the register of the hex's
// matrix, then four `mad`s multiply the corner by it; the weight goes to the colour output.
fn fowmask_vs_body(inp: FowMaskVsIn) -> FowMaskVsOut {
    let a0 = i32(floor(floor(inp.v1.x) * 4.0 + 0.5));
    var r = inp.v0.y * fow_wvp_reg(a0 + 1);
    r = inp.v0.x * fow_wvp_reg(a0 + 0) + r;
    r = inp.v0.z * fow_wvp_reg(a0 + 2) + r;
    var o: FowMaskVsOut;
    o.o0 = inp.v0.w * fow_wvp_reg(a0 + 3) + r;
    o.o1 = vec2<f32>(0.0);
    o.o2 = vec3<f32>(inp.v2.x);
    o.o3 = vec2<f32>(0.0);
    return o;
}

struct FowMaskVertexOut {
    @builtin(position) clip: vec4<f32>,
    // The weight, from `o2` of the original.
    @location(0) color: vec3<f32>,
}

@vertex
fn fowmask_vs(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec4<f32>,
) -> FowMaskVertexOut {
    var inp: FowMaskVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    inp.v2 = v2;
    let r = fowmask_vs_body(inp);
    var out: FowMaskVertexOut;
    out.clip = r.o0;
    out.color = r.o2;
    return out;
}

// ---- the pixel shaders ----

struct FowWeightIn {
    // dcl_color v0.x.
    v0: f32,
}

struct FowColorIn {
    // dcl_color v0.xyz.
    v0: vec3<f32>,
}

struct FowPsOut {
    oc0: vec4<f32>,
}

// re: FOW::Visible (sha1 d6a30d350665): `mad oC0, v0.x, c0.xyxy, c0.yyyx` with c0 = (1, 0, 0, 0).
fn visible_ps_body(inp: FowWeightIn) -> FowPsOut {
    var o: FowPsOut;
    o.oc0 = vec4<f32>(inp.v0, 0.0, inp.v0, 1.0);
    return o;
}

// re: FOW::VisibleFluid (sha1 01dd2a488404): c0 = (0, 1, 0, 0).
fn visiblefluid_ps_body(inp: FowWeightIn) -> FowPsOut {
    var o: FowPsOut;
    o.oc0 = vec4<f32>(0.0, inp.v0, 0.0, 1.0);
    return o;
}

// re: FOW::Transparent (sha1 80f4f25bccf7): c0 = (1, 0, 0, 0).
fn transparent_ps_body(inp: FowWeightIn) -> FowPsOut {
    var o: FowPsOut;
    o.oc0 = vec4<f32>(inp.v0, 0.0, 0.0, 1.0);
    return o;
}

// re: FOW::SolidPS (sha1 9502fba404b8): the colour of the vertex, opaque.
fn solid_ps_body(inp: FowColorIn) -> FowPsOut {
    var o: FowPsOut;
    o.oc0 = vec4<f32>(inp.v0, 1.0);
    return o;
}

@fragment
fn visible_ps(in: FowMaskVertexOut) -> @location(0) vec4<f32> {
    var inp: FowWeightIn;
    inp.v0 = in.color.x;
    return visible_ps_body(inp).oc0;
}

@fragment
fn visiblefluid_ps(in: FowMaskVertexOut) -> @location(0) vec4<f32> {
    var inp: FowWeightIn;
    inp.v0 = in.color.x;
    return visiblefluid_ps_body(inp).oc0;
}

@fragment
fn transparent_ps(in: FowMaskVertexOut) -> @location(0) vec4<f32> {
    var inp: FowWeightIn;
    inp.v0 = in.color.x;
    return transparent_ps_body(inp).oc0;
}

@fragment
fn solid_ps(in: FowMaskVertexOut) -> @location(0) vec4<f32> {
    var inp: FowColorIn;
    inp.v0 = in.color;
    return solid_ps_body(inp).oc0;
}
