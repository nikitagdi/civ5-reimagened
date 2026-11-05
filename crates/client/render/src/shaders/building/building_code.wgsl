// The `_CODE` and shadow stages of `BuildingShader` that the D3D11 build draws in the captures of
// S1-S4 (their DX11 twins): the colour pass `BuildingPS_CODE` (the light of `BuildingPS` with the
// colour correction, the fog of war and the distance fog of the original), the casters of the
// shadow map `BuildingVS_Shadow`, `BuildingVS_Instanced_Shadow`, `BuildingSkinnedVS_Shadow`, and
// their pixel stages `BuildingPS_CODE_BASE_SHADOW` (the alpha test) and `..._OPAQUE`. Hand port in
// the convention of shader-re 4.8.1: each entry of the original is a body `fn <entry>_body(inp: In)
// -> Out` with the registers of the original as member names, and an entry that only reads its
// inputs and calls the body.
//
// Needs frame.wgsl, sm3.wgsl, common/skin.wgsl and skin/common.wgsl (the palette of the skinned
// caster). Resources carry the names of the CTAB; a sampler `S` is the texture `S` and the sampler
// `S_smp`.
//
// The bindings are those of the draw of `world-objects` (`world::unit::draw`), which draws the
// buildings of the cities with these stages (`skin/building_object.wgsl`): group 0 is its frame
// group (the cubes at 2..7 in its order, the fog ramp at 8, 9), group 2 its fog-of-war group (the
// mask is `skin/common.wgsl`'s `skin_fow_tex` under its CTAB name, at the same binding, which no
// entry reads under both names; the base layer, the colour key of the fog of war and the constants
// from 6 up), group 3 the material (the diffuse, the SREF and the colour volume of the city's
// style).
//
// Gamma: the frame is not sRGB. `BuildingPS_CODE` raises the alphas to 2.2 and the light to 1/2.2
// before the colour volume, as `BuildingPS` does.

// ---- resources of BuildingPS_CODE ----

@group(3) @binding(0) var Diffuse: texture_2d<f32>;
@group(3) @binding(1) var Diffuse_smp: sampler;
@group(3) @binding(2) var BuildingSREF: texture_2d<f32>;
@group(3) @binding(3) var BuildingSREF_smp: sampler;
@group(0) @binding(2) var Irradiance: texture_cube<f32>;
@group(0) @binding(3) var Irradiance_smp: sampler;
@group(0) @binding(6) var SpecSharp: texture_cube<f32>;
@group(0) @binding(7) var SpecSharp_smp: sampler;
@group(0) @binding(4) var SpecSoft: texture_cube<f32>;
@group(0) @binding(5) var SpecSoft_smp: sampler;
@group(0) @binding(8) var DistanceFogRamp: texture_2d<f32>;
@group(0) @binding(9) var DistanceFogRamp_smp: sampler;
@group(3) @binding(6) var ColorCorrection: texture_3d<f32>;
@group(3) @binding(7) var ColorCorrection_smp: sampler;
@group(2) @binding(1) var FOWMask: texture_2d<f32>;
@group(2) @binding(2) var FOWMask_smp: sampler;
@group(2) @binding(6) var FOWBaseLayer: texture_2d<f32>;
@group(2) @binding(7) var FOWBaseLayer_smp: sampler;
@group(2) @binding(8) var FOWColorKey: texture_3d<f32>;
@group(2) @binding(9) var FOWColorKey_smp: sampler;

// The constants of `BuildingPS_CODE` that the frame globals do not hold: the eye of the distance fog
// (`v3CameraWorld`, c6) and the scale of its height (`fZBias`, c8).
struct BuildingCodeConsts {
    v3CameraWorld: vec3<f32>,
    fZBias: f32,
}
@group(2) @binding(10) var<uniform> building_code_consts: BuildingCodeConsts;

// The register `k` of a `row_major float4x4` that was filled by CTAB name (reg k = row k).
fn bcode_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// `v * ViewProjMatrix` as the casters do it (`mul`, `mad`, `mad`, then the last row).
fn bcode_project(p: vec4<f32>) -> vec4<f32> {
    let c0 = bcode_reg(frame_globals.ViewProjMatrix, 0u);
    let c1 = bcode_reg(frame_globals.ViewProjMatrix, 1u);
    let c2 = bcode_reg(frame_globals.ViewProjMatrix, 2u);
    let c3 = bcode_reg(frame_globals.ViewProjMatrix, 3u);
    var r1 = p.y * c1;
    r1 = p.x * c0 + r1;
    r1 = p.z * c2 + r1;
    return p.w * c3 + r1;
}

fn bcode_log3(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(d3d_log(c.x), d3d_log(c.y), d3d_log(c.z));
}

// ---- BuildingPS_CODE ----

struct BuildingCodePsIn {
    // dcl_texcoord v0: the uv of the material (xy) and of the base layer of the fog of war (zw).
    v0: vec4<f32>,
    // dcl_texcoord1 v1.xyz: the point in the world.
    v1: vec3<f32>,
    // dcl_texcoord2 v2.y: y / w of the clip position, the row of the distance fog ramp.
    v2: vec2<f32>,
    // dcl_texcoord3 v3.xy: the place in the fog-of-war mask.
    v3: vec2<f32>,
    // dcl_normal v4.xyz: the normal in the world.
    v4: vec3<f32>,
}

struct BuildingCodePsOut {
    oc0: vec4<f32>,
    // `texkill`: the texel is not drawn.
    kill: bool,
}

// re: BuildingShader::BuildingPS_CODE (sha1 d92314717b01)
fn building_ps_code_body(inp: BuildingCodePsIn) -> BuildingCodePsOut {
    // `def c0, 2.2, -0.1, 199, 1`, `def c1, 0.0529, 0.846, 0.529, 0.4545` (xyz: the direction of the
    // light, w: 1 / 2.2), `def c2, 0.5, 1.44269502 (log2 e), 0, 0`. They cover `ViewInverseMatrix`
    // but its row 3, the eye (c3).
    let c0 = vec4<f32>(2.20000005, -0.100000001, 199.0, 1.0);
    let c1 = vec4<f32>(0.0529000014, 0.846000016, 0.528999984, 0.454545468);
    let c2 = vec2<f32>(0.5, 1.44269502);
    let c3 = bcode_reg(frame_globals.ViewInverseMatrix, 3u);
    let g_FogConsts = frame_globals.g_FogConsts;
    let f3FogColor = frame_globals.f3FogColor;
    let fWaterHeight = frame_globals.fWaterHeight;
    let v3CameraWorld = building_code_consts.v3CameraWorld;
    let fZBias = building_code_consts.fZBias;

    let diffuse = textureSample(Diffuse, Diffuse_smp, inp.v0.xy);
    let irradiance = textureSample(Irradiance, Irradiance_smp, inp.v4);
    var out: BuildingCodePsOut;
    // The alpha raised to 2.2; texkill under 0.1.
    let alpha = d3d_pow(diffuse.w, c0.x);
    out.kill = alpha + c0.y < 0.0;
    var lit = diffuse.xyz * irradiance.xyz;
    lit = lit + lit;

    // The view vector, the half vector towards the light (`c1.xyz`), the reflection.
    let view = c3.xyz - inp.v1;
    let inv_len = d3d_rsq(dot(view, view));
    let view_n = inv_len * view;
    let half_v = d3d_nrm(vec4<f32>(view * inv_len + c1.xyz, 0.0)).xyz;
    let n_dot_h = saturate(dot(half_v, inp.v4));
    var k = dot(inp.v4, view_n);
    k = k + k;
    let refl = k * inp.v4 - view_n;
    let soft = textureSample(SpecSoft, SpecSoft_smp, refl);
    let sharp = textureSample(SpecSharp, SpecSharp_smp, refl);
    let sref = textureSample(BuildingSREF, BuildingSREF_smp, inp.v0.xy);
    // The gloss: SREF.a ^ 2.2; lrp between the soft and the sharp cube; the highlight.
    let gloss = d3d_pow(sref.w, c0.x);
    let spec_color = gloss * (sharp.xyz - soft.xyz) + soft.xyz;
    let power = gloss * c0.z + c0.w;
    let highlight = d3d_pow(n_dot_h, power);
    let strength = power * -c0.y;
    let spec = strength * highlight + spec_color;
    let sum = spec * sref.xyz + lit;

    // To gamma 1 / 2.2, through the colour volume.
    let page = exp2(bcode_log3(sum) * c1.w);
    let corrected = textureSample(ColorCorrection, ColorCorrection_smp, page);
    // The fog of war: the base layer where the mask is not visible, the colour key where it is
    // revealed but hidden.
    let base_layer = textureSample(FOWBaseLayer, FOWBaseLayer_smp, inp.v0.zw);
    let mask = textureSample(FOWMask, FOWMask_smp, inp.v3);
    let seen = mask.x * (corrected.xyz - base_layer.xyz) + base_layer.xyz;
    let hidden = mask.z * -mask.x + mask.x;
    let key = textureSample(FOWColorKey, FOWColorKey_smp, seen);
    let color = hidden * (key.xyz - seen) + seen;
    let to_fog = f3FogColor.xyz - color;

    // The distance fog: exponential in height and distance, ramp `DistanceFogRamp`.
    let d = vec3<f32>(inp.v1.xy, fZBias * inp.v1.z) - v3CameraWorld;
    var r0w = d.z * g_FogConsts.x;
    r0w = r0w * c2.y;
    r0w = exp2(r0w);
    r0w = -r0w + c0.w;
    let dist2 = dot(d, d);
    let inv_dz = d3d_rcp(d.z);
    var fog = dist2 * g_FogConsts.y;
    fog = r0w * fog;
    fog = inv_dz * fog;
    fog = fog * c2.y;
    fog = saturate(exp2(fog));
    fog = -fog + c0.w;
    let ramp_u = inp.v2.y * c2.x + c2.x;
    let ramp = textureSample(DistanceFogRamp, DistanceFogRamp_smp, vec2<f32>(ramp_u, g_FogConsts.w));
    fog = fog * ramp.x;

    // The alpha: half of it under the water.
    let above = inp.v1.z - fWaterHeight;
    let a = select(alpha * c2.x, alpha, above >= 0.0);
    out.oc0 = vec4<f32>(fog * to_fog + color, a);
    return out;
}

// ---- BuildingPS_CODE_Opaque ----
//
// DX11 only (no SM3 block): ported from the `ps_5_0` block, whose inputs are those of
// `BuildingPS_CODE` (TEXCOORD0..3, NORMAL) and whose `cb1[16..21]` are its constants (the eye,
// `g_FogConsts`, `f3FogColor`, `v3CameraWorld`, `fWaterHeight`, `fZBias`). The colour pass of the
// opaque buildings in every capture of S1-S4 (with the DX11-only `BuildingVS_Instanced`).

// re: BuildingShader::BuildingPS_CODE_Opaque (sha1 5154367387fa): the code of `BuildingPS_CODE`
// without the texkill and without the 2.2 on the alphas (the gloss and the alpha are read raw).
fn building_ps_code_opaque_body(inp: BuildingCodePsIn) -> BuildingCodePsOut {
    let log2e = 1.44269502;
    let light = vec3<f32>(0.0529000014, 0.846000016, 0.528999984);
    let eye = bcode_reg(frame_globals.ViewInverseMatrix, 3u);
    let g_FogConsts = frame_globals.g_FogConsts;
    let f3FogColor = frame_globals.f3FogColor;
    let fWaterHeight = frame_globals.fWaterHeight;
    let v3CameraWorld = building_code_consts.v3CameraWorld;
    let fZBias = building_code_consts.fZBias;

    // The distance fog.
    let d = vec3<f32>(inp.v1.xy, inp.v1.z * fZBias) - v3CameraWorld;
    let height = -exp2(d.z * g_FogConsts.x * log2e) + 1.0;
    var fog = dot(d, d) * g_FogConsts.y;
    fog = height * fog / d.z;
    fog = -min(exp2(fog * log2e), 1.0) + 1.0;
    let ramp_u = inp.v2.y * 0.5 + 0.5;
    let ramp = textureSample(DistanceFogRamp, DistanceFogRamp_smp, vec2<f32>(ramp_u, g_FogConsts.w));
    fog = fog * ramp.x;

    // The view vector, the reflection, the half vector towards the light.
    let view = eye.xyz - inp.v1;
    let inv_len = inverseSqrt(dot(view, view));
    let view_n = view * inv_len;
    let half_raw = view * inv_len + light;
    var k = dot(inp.v4, view_n);
    k = k + k;
    let refl = k * inp.v4 - view_n;
    let sharp = textureSample(SpecSharp, SpecSharp_smp, refl);
    let soft = textureSample(SpecSoft, SpecSoft_smp, refl);
    let sref = textureSample(BuildingSREF, BuildingSREF_smp, inp.v0.xy);
    let spec_color = sref.w * (sharp.xyz - soft.xyz) + soft.xyz;
    let half_v = half_raw * inverseSqrt(dot(half_raw, half_raw));
    let n_dot_h = saturate(dot(half_v, inp.v4));
    let power = sref.w * 199.0 + 1.0;
    let highlight = exp2(log2(n_dot_h) * power);
    let spec = (power * 0.100000001) * highlight + spec_color;
    let irradiance = textureSample(Irradiance, Irradiance_smp, inp.v4);
    let diffuse = textureSample(Diffuse, Diffuse_smp, inp.v0.xy);
    var lit = irradiance.xyz * diffuse.xyz;
    lit = lit + lit;
    let sum = spec * sref.xyz + lit;

    // To gamma 1 / 2.2, the colour volume, the fog of war, the distance fog.
    let page = exp2(log2(sum) * 0.454545468);
    let corrected = textureSample(ColorCorrection, ColorCorrection_smp, page);
    let base_layer = textureSample(FOWBaseLayer, FOWBaseLayer_smp, inp.v0.zw);
    let mask = textureSample(FOWMask, FOWMask_smp, inp.v3);
    let seen = mask.x * (corrected.xyz - base_layer.xyz) + base_layer.xyz;
    let hidden = mask.z * -mask.x + mask.x;
    let key = textureSample(FOWColorKey, FOWColorKey_smp, seen);
    let color = hidden * (key.xyz - seen) + seen;

    var out: BuildingCodePsOut;
    out.kill = false;
    let a = select(diffuse.w, diffuse.w * 0.5, inp.v1.z < fWaterHeight);
    out.oc0 = vec4<f32>(fog * (f3FogColor.xyz - color) + color, a);
    return out;
}

// ---- BuildingPS_CODE_BASE_SHADOW, BuildingPS_CODE_BASE_SHADOW_OPAQUE ----

struct BuildingShadowPsIn {
    // dcl_texcoord v0.xyz: the uv (xy) and the height of the point (z), as the casters give them.
    v0: vec3<f32>,
}

struct BuildingShadowPsOut {
    oc0: vec4<f32>,
}

// re: BuildingShader::BuildingPS_CODE_BASE_SHADOW (sha1 8e17a57e70f9)
// White; the alpha of the diffuse (half of it under the water), zero where it is under 0.1
// (`def c0, 0.5, 0.1, 0, 1`).
fn building_ps_code_base_shadow_body(inp: BuildingShadowPsIn) -> BuildingShadowPsOut {
    let above = inp.v0.z - frame_globals.fWaterHeight;
    let diffuse = textureSample(Diffuse, Diffuse_smp, inp.v0.xy);
    let a = select(diffuse.w * 0.5, diffuse.w, above >= 0.0);
    let cut = -diffuse.w + 0.100000001;
    var o: BuildingShadowPsOut;
    o.oc0 = vec4<f32>(vec3<f32>(1.0), select(a, 0.0, cut >= 0.0));
    return o;
}

// re: BuildingShader::BuildingPS_CODE_BASE_SHADOW_OPAQUE (sha1 53a357768fd6): `mov oC0, c0.x` with
// `def c0, 1, 0, 0, 0`.
fn building_ps_code_base_shadow_opaque_body(inp: BuildingShadowPsIn) -> BuildingShadowPsOut {
    var o: BuildingShadowPsOut;
    o.oc0 = vec4<f32>(1.0);
    return o;
}

// ---- BuildingVS_Shadow ----

struct BuildingShadowVsIn {
    // dcl_position v0 (x, y, z; w is not read).
    v0: vec3<f32>,
    // dcl_texcoord v1: (u, v).
    v1: vec2<f32>,
    // `WorldMatrix` (row_major float4x4, c17..c20), from the instance (data move `instance:world`),
    // as `BuildingVS`: the member `[j][k]` is component j of the register c(17 + k).
    instance_world: mat4x4<f32>,
}

struct BuildingShadowVsOut {
    o0: vec4<f32>,
    // dcl_texcoord o1.xyz: the uv and the height of the point.
    o1: vec3<f32>,
}

// re: BuildingShader::BuildingVS_Shadow (sha1 0d9e501bcfd3)
fn building_vs_shadow_body(inp: BuildingShadowVsIn) -> BuildingShadowVsOut {
    let c17 = bcode_reg(inp.instance_world, 0u);
    let c18 = bcode_reg(inp.instance_world, 1u);
    let c19 = bcode_reg(inp.instance_world, 2u);
    let c20 = bcode_reg(inp.instance_world, 3u);
    var r0 = c18 * inp.v0.y;
    r0 = inp.v0.x * c17 + r0;
    r0 = inp.v0.z * c19 + r0;
    r0 = r0 + c20;
    var o: BuildingShadowVsOut;
    o.o0 = bcode_project(r0);
    o.o1 = vec3<f32>(inp.v1, r0.z);
    return o;
}

// ---- BuildingVS_Instanced_Shadow ----

struct BuildingInstancedShadowVsIn {
    // dcl_position v0 (x, y, z), dcl_texcoord v1 (u, v).
    v0: vec3<f32>,
    v1: vec2<f32>,
    // dcl_texcoord1..3 v2..v4 and dcl_color v5: the world matrix of the instance, one register a
    // row of the original's `row_major float4x4`.
    v2: vec4<f32>,
    v3: vec4<f32>,
    v4: vec4<f32>,
    v5: vec4<f32>,
}

// re: BuildingShader::BuildingVS_Instanced_Shadow (sha1 bebb4b952411)
fn building_vs_instanced_shadow_body(inp: BuildingInstancedShadowVsIn) -> BuildingShadowVsOut {
    var r1 = inp.v0.y * inp.v3;
    r1 = inp.v0.x * inp.v2 + r1;
    var r0 = inp.v0.z * inp.v4 + r1;
    r0 = r0 + inp.v5;
    var o: BuildingShadowVsOut;
    o.o0 = bcode_project(r0);
    o.o1 = vec3<f32>(inp.v1, r0.z);
    return o;
}

// ---- BuildingSkinnedVS_Shadow ----

struct BuildingSkinnedShadowVsIn {
    // dcl_position v0, dcl_blendweight v1, dcl_blendindices v2.
    v0: vec3<f32>,
    v1: vec3<f32>,
    v2: vec3<f32>,
    // dcl_texcoord v3: (u, v).
    v3: vec2<f32>,
    // The palette `BoneMatrices` (c17, three registers a bone) is the bone texture (data move
    // `bone_texture`).
    bone_base: u32,
    bone_base_next: u32,
    bone_blend: f32,
}

// re: BuildingShader::BuildingSkinnedVS_Shadow (sha1 22390584ec44)
// Three bones a vertex, the next while its weight is above zero (`def c4, 3, 1, 0, 0`).
fn building_skinned_vs_shadow_body(inp: BuildingSkinnedShadowVsIn) -> BuildingShadowVsOut {
    let three = 3.0;
    let b = inp.bone_base;
    let n = inp.bone_base_next;
    let k = inp.bone_blend;
    let r0 = vec4<f32>(inp.v0, 1.0);
    var a0 = skin_mova(three * inp.v2.x);
    var r1 = vec3<f32>(
        dot(r0, palette_row(b, n, k, a0)),
        dot(r0, palette_row(b, n, k, a0 + 1u)),
        dot(r0, palette_row(b, n, k, a0 + 2u)),
    );
    r1 = r1 * inp.v1.x;
    if (0.0 < inp.v1.y) {
        a0 = skin_mova(three * inp.v2.y);
        var r2 = vec3<f32>(
            dot(r0, palette_row(b, n, k, a0)),
            dot(r0, palette_row(b, n, k, a0 + 1u)),
            dot(r0, palette_row(b, n, k, a0 + 2u)),
        );
        r1 = inp.v1.y * r2 + r1;
        if (0.0 < inp.v1.z) {
            a0 = skin_mova(three * inp.v2.z);
            r2 = vec3<f32>(
                dot(r0, palette_row(b, n, k, a0)),
                dot(r0, palette_row(b, n, k, a0 + 1u)),
                dot(r0, palette_row(b, n, k, a0 + 2u)),
            );
            r1 = inp.v1.z * r2 + r1;
        }
    }
    // The world matrix of the instance (the identity when the palette has it inside, as in the
    // comparison with the oracle).
    r1 = skin_place_point(r1);
    let c0 = bcode_reg(frame_globals.ViewProjMatrix, 0u);
    let c1 = bcode_reg(frame_globals.ViewProjMatrix, 1u);
    let c2 = bcode_reg(frame_globals.ViewProjMatrix, 2u);
    let c3 = bcode_reg(frame_globals.ViewProjMatrix, 3u);
    var p = r1.y * c1;
    p = r1.x * c0 + p;
    p = r1.z * c2 + p;
    var o: BuildingShadowVsOut;
    o.o0 = p + c3;
    o.o1 = vec3<f32>(inp.v3, r1.z);
    return o;
}

// ---- entry points: the vertex of `world-objects` (`ObjectInstance`) ----

struct BuildingCodeVertexIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    // The rows of the matrix of the object, our basis.
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
}

struct BuildingCodeSkinnedVertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(14) joints: vec4<u32>,
    @location(15) weights: vec4<f32>,
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

struct BuildingCodeShadowVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec3<f32>,
}

// The `row_major float4x4` of the original for the rows of the instance (original's basis): row j of
// the object-to-world transform is the member `[j]`, so the register c(17 + k) is its column k.
fn bcode_world(w0: vec4<f32>, w1: vec4<f32>, w2: vec4<f32>) -> mat4x4<f32> {
    let w = skin_instance_world(w0, w1, w2);
    return mat4x4<f32>(w[0], w[1], w[2], vec4<f32>(0.0, 0.0, 0.0, 1.0));
}

@vertex
fn building_vs_shadow(v: BuildingCodeVertexIn) -> BuildingCodeShadowVertexOut {
    var b: BuildingShadowVsIn;
    b.v0 = v.position;
    b.v1 = v.uv;
    b.instance_world = bcode_world(v.world0, v.world1, v.world2);
    let r = building_vs_shadow_body(b);
    var out: BuildingCodeShadowVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    return out;
}

@vertex
fn building_vs_instanced_shadow(v: BuildingCodeVertexIn) -> BuildingCodeShadowVertexOut {
    let m = bcode_world(v.world0, v.world1, v.world2);
    var b: BuildingInstancedShadowVsIn;
    b.v0 = v.position;
    b.v1 = v.uv;
    b.v2 = bcode_reg(m, 0u);
    b.v3 = bcode_reg(m, 1u);
    b.v4 = bcode_reg(m, 2u);
    b.v5 = bcode_reg(m, 3u);
    let r = building_vs_instanced_shadow_body(b);
    var out: BuildingCodeShadowVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    return out;
}

@vertex
fn building_skinned_vs_shadow(v: BuildingCodeSkinnedVertexIn) -> BuildingCodeShadowVertexOut {
    var b: BuildingSkinnedShadowVsIn;
    b.v0 = v.position;
    b.v1 = v.weights.xyz;
    b.v2 = vec3<f32>(v.joints.xyz);
    b.v3 = v.uv;
    let bone_p = bone_pose(v.frames, v.blend);
    b.bone_base = bone_p.base;
    b.bone_base_next = bone_p.next;
    b.bone_blend = bone_p.blend;
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    let r = building_skinned_vs_shadow_body(b);
    var out: BuildingCodeShadowVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    return out;
}

@fragment
fn building_ps_code_base_shadow(in: BuildingCodeShadowVertexOut) -> @location(0) vec4<f32> {
    var b: BuildingShadowPsIn;
    b.v0 = in.v0;
    return building_ps_code_base_shadow_body(b).oc0;
}

@fragment
fn building_ps_code_base_shadow_opaque(in: BuildingCodeShadowVertexOut) -> @location(0) vec4<f32> {
    var b: BuildingShadowPsIn;
    b.v0 = in.v0;
    return building_ps_code_base_shadow_opaque_body(b).oc0;
}

// ---- entry points: the colour pass ----

struct BuildingCodeVaryings {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec3<f32>,
    @location(2) v2: vec2<f32>,
    @location(3) v3: vec2<f32>,
    @location(4) v4: vec3<f32>,
}

@fragment
fn building_ps_code(in: BuildingCodeVaryings) -> @location(0) vec4<f32> {
    var b: BuildingCodePsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    b.v4 = in.v4;
    // The samples come first: the control flow is uniform until the body is done.
    let r = building_ps_code_body(b);
    if (r.kill) {
        discard;
    }
    return r.oc0;
}

@fragment
fn building_ps_code_opaque(in: BuildingCodeVaryings) -> @location(0) vec4<f32> {
    var b: BuildingCodePsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    b.v4 = in.v4;
    return building_ps_code_opaque_body(b).oc0;
}
