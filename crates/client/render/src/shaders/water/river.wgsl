// WaterShader of the original, the rivers (world-water WW-1, R3.6): `VSWaterRiver` and the pixel
// shaders `PSRiverHigh` and `PSRiverLow` (`TerrainWaterQuality` 2 and below), then the shaders of the
// river banners `RiverShader::VSRiver` and `RiverShader::PSRiver`. Hand ports in the convention of
// shader-re 4.8.1 (see ocean.wgsl): each entry of the original is a body `fn <entry>_body(inp: In)
// -> Out` with the registers of the original as member names, and a `@vertex` / `@fragment` entry
// that only reads its inputs, calls the body and does the `texkill` discard.
//
// Needs sm3.wgsl. Group 0: binding 0 the constants (one block, [`WaterConsts`], the 592 bytes of
// `FireGrafix::WaterGlobalData` exactly as in ocean.wgsl, so one uniform buffer serves both files:
// the register `cN` of the pixel shaders is the register `N - 9` of the block and one of the vertex
// shader is `N - 4`), then the textures of the samplers of the CTAB in pairs (texture `S`, sampler
// `S_smp`) with the numbers of ocean.wgsl; the samplers the river alone has come after them: binding
// 23 the `World` matrix of `VSWaterRiver` (c0 of the vertex shader; it is per draw, not in the block),
// 24..29 `LEAN2_1`, `LEAN2_2` and `ColorKey`.
//
// The river is a ribbon of triangles along the bank (R3.6): the vertex is `(x, y, z)` in the local
// space of the ribbon (`World` brings it to the world of the original), two texture coordinates
// (`v1` the pair of the flow, `v2` the place on the colour map `WaterColor` / `WaterBlendMap`). The
// flow is scrolled in the pixel shader by the layers' own repeats (`RiverScale`, `RiverOffset`).
//
// Gamma: as in ocean.wgsl, the shaders do their own `pow`: the reflection is squared in (2.2), the
// colour is squared out (1 / 2.2), the textures are raw `*Unorm` values.

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// `FireGrafix::WaterGlobalData`: float index of the first component in the comment.
struct WaterConsts {
    // 0: the view-projection of the camera (`row_major`), c4 of `VSWaterRiver`.
    g_VP: mat4x4<f32>,
    // 16: the projection of the overlay maps (the fog of war, `OverlayMap`), c8 of `VSWaterRiver`.
    OverlayProjection: mat4x4<f32>,
    // 32: the offset added to the flow coordinate of the river before its scales (c12 of the VS).
    RiverOffset: vec4<f32>,
    // 36: the repeats of the LEAN maps of the four layers per world unit (ocean's c13; the river
    // VS has its own `def` there and does not read it).
    WaterScale: vec4<f32>,
    // 40: (scale of layer 0 xy, scale of the layer 1 coordinate, scale of the layer 2) (c14 of the VS).
    RiverScale: vec4<f32>,
    // 44: the scroll of the LEAN maps (c15).
    WaterOffset: f32,
    pad45: f32,
    pad46: f32,
    pad47: f32,
    // 48: the height of the water in the world of the original (c16).
    fOceanHeight: f32,
    pad49: f32,
    pad50: f32,
    pad51: f32,
    // 52: the divisor of the weights of the layers; the river uses a third of its reciprocal.
    GradientScale: f32,
    pad53: f32,
    pad54: f32,
    pad55: f32,
    // 56
    SunDir: vec3<f32>,
    pad59: f32,
    // 60
    SkyDir: vec3<f32>,
    pad63: f32,
    // 64: colour of the sun's specular lobe; w is its power.
    SpecSun: vec4<f32>,
    // 68: colour of the sky's specular lobe (the river takes 0.8 of it); w is its power.
    SpecSky: vec4<f32>,
    // 72: the direction to the eye the lobe is lit from; w the blend with the real direction.
    FakeViewSun: vec4<f32>,
    // 76
    FakeViewSky: vec4<f32>,
    // 80: (height factor, distance factor, -, row of the fog ramp)
    g_FogConsts: vec4<f32>,
    // 84
    v3CameraWorld: vec3<f32>,
    pad87: f32,
    // 88
    g_ReflectVP: mat4x4<f32>,
    // 104
    g_ShadowVP: mat4x4<f32>,
    // 120: the world to the texture space of the terrain.
    g_WorldToTerrainTextureSpace: mat4x4<f32>,
    // 136
    g_ShadowFade: f32,
    pad137: f32,
    pad138: f32,
    pad139: f32,
    // 140
    f3FogColor: vec4<f32>,
    // 144
    ReflectionAmount: f32,
    pad145: f32,
    pad146: f32,
    pad147: f32,
}

@group(0) @binding(0) var<uniform> water_consts: WaterConsts;

@group(0) @binding(1) var LEAN1_1: texture_2d<f32>;
@group(0) @binding(2) var LEAN1_1_smp: sampler;
@group(0) @binding(3) var LEAN1_2: texture_2d<f32>;
@group(0) @binding(4) var LEAN1_2_smp: sampler;
@group(0) @binding(5) var ReflectionMap: texture_2d<f32>;
@group(0) @binding(6) var ReflectionMap_smp: sampler;
@group(0) @binding(7) var WaterBlendMap: texture_2d<f32>;
@group(0) @binding(8) var WaterBlendMap_smp: sampler;
@group(0) @binding(9) var WaterColor: texture_2d<f32>;
@group(0) @binding(10) var WaterColor_smp: sampler;
@group(0) @binding(13) var WaterFogRamp: texture_2d<f32>;
@group(0) @binding(14) var WaterFogRamp_smp: sampler;
@group(0) @binding(15) var OverlayMap: texture_2d<f32>;
@group(0) @binding(16) var OverlayMap_smp: sampler;
@group(0) @binding(17) var FOWMask: texture_2d<f32>;
@group(0) @binding(18) var FOWMask_smp: sampler;
@group(0) @binding(19) var TransFOWColor: texture_3d<f32>;
@group(0) @binding(20) var TransFOWColor_smp: sampler;

// re: the per-draw matrix `World` of `VSWaterRiver` (c0..c3, `row_major`).
@group(0) @binding(23) var<uniform> World: mat4x4<f32>;
@group(0) @binding(24) var LEAN2_1: texture_2d<f32>;
@group(0) @binding(25) var LEAN2_1_smp: sampler;
@group(0) @binding(26) var LEAN2_2: texture_2d<f32>;
@group(0) @binding(27) var LEAN2_2_smp: sampler;
@group(0) @binding(28) var ColorKey: texture_3d<f32>;
@group(0) @binding(29) var ColorKey_smp: sampler;

// ---- VSWaterRiver ----

struct WaterRiverVsIn {
    // dcl_position v0: the point in the local space of the ribbon.
    v0: vec3<f32>,
    // dcl_texcoord v1, dcl_texcoord1 v2.
    v1: vec2<f32>,
    v2: vec2<f32>,
}

struct WaterRiverVsOut {
    o0: vec4<f32>,
    // The texture coordinates of the three layers of LEAN.
    o1: vec2<f32>,
    o2: vec2<f32>,
    o3: vec2<f32>,
    // The colour map's coordinate (v2 as it is).
    o4: vec2<f32>,
    // The point in the world.
    o5: vec3<f32>,
    // The clip position without z.
    o6: vec3<f32>,
    // (the overlay maps' xy, 0, 0)
    o7: vec4<f32>,
}

// re: WaterShader::VSWaterRiver (sha1 9964ed9b8cec)
fn vswaterriver_body(inp: WaterRiverVsIn) -> WaterRiverVsOut {
    let v0 = inp.v0;
    let rs = water_consts.RiverScale;
    var o: WaterRiverVsOut;
    o.o1 = rs.xy * inp.v1;
    let r0 = (water_consts.RiverOffset + inp.v1.xyxy) * rs.xyxy;
    o.o2 = r0.xy * rs.z;
    o.o3 = r0.zw * rs.w;
    o.o4 = inp.v2;
    // The point in the world: v0 through `World`.
    var w = row_major_reg(World, 1u).xyz * v0.y;
    w = v0.x * row_major_reg(World, 0u).xyz + w;
    w = v0.z * row_major_reg(World, 2u).xyz + w;
    w = w + row_major_reg(World, 3u).xyz;
    // The overlay maps' coordinates.
    let ov0 = row_major_reg(water_consts.OverlayProjection, 0u);
    let ov1 = row_major_reg(water_consts.OverlayProjection, 1u);
    let ov2 = row_major_reg(water_consts.OverlayProjection, 2u);
    let ov3 = row_major_reg(water_consts.OverlayProjection, 3u);
    var p = w.y * ov1.xy;
    p = w.x * ov0.xy + p;
    p = w.z * ov2.xy + p;
    p = p + ov3.xy;
    o.o7 = vec4<f32>(p * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5), 0.0, 0.0);
    // The clip position.
    var c = w.y * row_major_reg(water_consts.g_VP, 1u);
    c = w.x * row_major_reg(water_consts.g_VP, 0u) + c;
    c = w.z * row_major_reg(water_consts.g_VP, 2u) + c;
    o.o5 = w;
    c = c + row_major_reg(water_consts.g_VP, 3u);
    o.o0 = c;
    o.o6 = c.xyw;
    return o;
}

// ---- the pixel shaders ----

struct RiverPsOut {
    oc0: vec4<f32>,
    kill: bool,
}

// A LEAN map as the original stores it, 16 bits per channel unorm; ours is half float, so the value
// read is rounded to the grid of 16 bits (as in ocean.wgsl, and the oracle of shader-re does too).
fn river_lean_1a(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(LEAN1_1, LEAN1_1_smp, uv) * 65535.0) / 65535.0;
}

fn river_lean_1b(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(LEAN1_2, LEAN1_2_smp, uv) * 65535.0) / 65535.0;
}

fn river_lean_2a(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(LEAN2_1, LEAN2_1_smp, uv) * 65535.0) / 65535.0;
}

fn river_lean_2b(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(LEAN2_2, LEAN2_2_smp, uv) * 65535.0) / 65535.0;
}

// One lobe of the specular light (same as `ocean_lobe`): a Gaussian of the slope `h` of the half
// vector with the covariance of the LEAN map plus the roughness of the light: `a = cxx + e`,
// `b = cyy + e`, `cc = cxy^2`, `cxy2 = 2 cxy`. Gives (density, exponent, how many of the two
// safeguards fired: the determinant is not positive, the exponent is 16 or more).
fn river_lobe(a: f32, b: f32, cc: f32, cxy2: f32, h: vec2<f32>) -> vec3<f32> {
    let det = a * b - cc;
    var q = h.y * (a * h.y - h.x * cxy2);
    q = h.x * h.x * b + q;
    q = q * 0.5;
    let rdet = d3d_rcp(det);
    let ex = rdet * q;
    let flag_det = select(0.0, 1.0, -det >= 0.0);
    let flag_ex = select(0.0, 1.0, q * rdet + -16.0 >= 0.0);
    var d = exp2(ex * -1.44269502);
    d = d * d3d_rsq(det);
    return vec3<f32>(d, ex, flag_det + flag_ex);
}

// A light is off where a safeguard fired.
fn river_gate(flag: f32, v: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(0.0), v, -flag >= 0.0);
}

// The half vector of a light in the slope space, minus the mean slope `m`.
fn river_half_slope(view: vec3<f32>, fake: vec4<f32>, dir: vec3<f32>, m: vec2<f32>) -> vec2<f32> {
    var v = view - fake.xyz;
    v = fake.w * v + fake.xyz;
    let h = v * d3d_rsq(dot(v, v)) + dir;
    return h.xy * d3d_rcp(h.z) + -m;
}

// The reflection map: four taps around the point, spread by the covariance and by `spread` (the
// determinant of the sky lobe), averaged; rgb in linear light.
fn river_reflection(world: vec3<f32>, cxx: f32, cyy: f32, spread: f32) -> vec4<f32> {
    let m = water_consts.g_ReflectVP;
    var r = row_major_reg(m, 1u).xyw * world.y;
    r = world.x * row_major_reg(m, 0u).xyw + r;
    r = world.z * row_major_reg(m, 2u).xyw + r;
    r = r + row_major_reg(m, 3u).xyw;
    let ndc = d3d_rcp(r.z) * r.xy;
    let uv0 = ndc * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5);
    let uv = vec2<f32>(cxx, cyy) * 6.0 + uv0;
    let s1 = textureSample(ReflectionMap, ReflectionMap_smp, spread * 2.0 + uv);
    let s2 = textureSample(ReflectionMap, ReflectionMap_smp, spread * vec2<f32>(2.0, -2.0) + uv);
    let s3 = textureSample(ReflectionMap, ReflectionMap_smp, spread * vec2<f32>(-2.0, 6.0) + uv);
    let s4 = textureSample(ReflectionMap, ReflectionMap_smp, spread * -2.0 + uv);
    var t = s2 * 0.25;
    t = s1 * 0.25 + t;
    t = s3 * 0.25 + t;
    t = s4 * 0.25 + t;
    let lin = exp2(vec3<f32>(d3d_log(t.x), d3d_log(t.y), d3d_log(t.z)) * 2.20000005);
    return vec4<f32>(lin, t.w);
}

// The distance fog of both pixel shaders: the ramp by the height on the screen, the density by the
// height and the distance of the point. `lit` is the colour after the fog of war.
fn river_fog(lit: vec3<f32>, world: vec3<f32>, clip_y: f32, clip_w: f32) -> vec3<f32> {
    let ramp = textureSample(
        WaterFogRamp,
        WaterFogRamp_smp,
        vec2<f32>(d3d_rcp(clip_w) * clip_y * 0.5 + 0.5, water_consts.g_FogConsts.w),
    );
    let d = world - water_consts.v3CameraWorld;
    let dist2 = dot(d, d) * water_consts.g_FogConsts.y;
    let height_term = d.z * water_consts.g_FogConsts.x * 1.44269502;
    var fog = (-exp2(height_term) + 1.0) * dist2;
    fog = d3d_rcp(d.z) * fog;
    fog = fog * 1.44269502;
    fog = -saturate(exp2(fog)) + 1.0;
    let amount = fog * ramp.x;
    return amount * (water_consts.f3FogColor.xyz - lit) + lit;
}

// The overlay maps (premultiplied) over the colour keyed colour `col`, then the fog of war: toward
// the colour key by what was seen and what is seen now.
fn river_overlay_fow(col_in: vec3<f32>, overlay_uv: vec2<f32>, fow: vec4<f32>) -> vec3<f32> {
    let keyed = textureSample(ColorKey, ColorKey_smp, col_in).xyz;
    let ov = textureSample(OverlayMap, OverlayMap_smp, overlay_uv);
    var col = keyed * (-ov.w + 1.0) + ov.xyz;
    col = col + -0.400000006;
    col = fow.x * col + 0.400000006;
    let key = textureSample(TransFOWColor, TransFOWColor_smp, col).xyz;
    let seen = fow.z * -fow.x + fow.x;
    return seen * (key - col) + col;
}

// ---- PSRiverHigh ----

struct RiverPsHighIn {
    // dcl_texcoord .. 2 v0 .. v2: the three layers of LEAN (the first of LEAN1, the others LEAN2).
    v0: vec2<f32>,
    v1: vec2<f32>,
    v2: vec2<f32>,
    // dcl_texcoord3 v3: the colour map's coordinate.
    v3: vec2<f32>,
    // dcl_texcoord4 v4.xyz: the point in the world.
    v4: vec3<f32>,
    // dcl_texcoord5 v5.yz: the clip y and w.
    v5: vec3<f32>,
    // dcl_texcoord6 v6.xy: the overlay maps' xy.
    v6: vec2<f32>,
}

// re: WaterShader::PSRiverHigh (sha1 f781f5a8efb5): three layers of LEAN (each weighted by a third
// of 1 / GradientScale, no flow weights), the light of ocean's High, and the alpha from
// `WaterBlendMap`.
fn psriverhigh_body(inp: RiverPsHighIn) -> RiverPsOut {
    var o: RiverPsOut;
    let fow = textureSample(FOWMask, FOWMask_smp, inp.v6);
    o.kill = fow.x == 0.0;
    let wc = textureSample(WaterColor, WaterColor_smp, inp.v3);
    let k = d3d_rcp(water_consts.GradientScale) * 0.333000004;
    let k2 = k * k;
    // Layer 0: the mean slope n and the second moments (xx, yy, xy).
    let a0 = river_lean_1a(inp.v0);
    let b0 = river_lean_1b(inp.v0);
    let n0 = a0.xy * 2.0 + -1.0;
    let wn0 = k * n0;
    let m0 = vec3<f32>(a0.z, a0.w, b0.x * 2.0 + -1.0);
    // Layer 1, turned: p = (1 - 2 x, 2 y - 1).
    let a1 = river_lean_2a(inp.v1);
    let b1 = river_lean_2b(inp.v1);
    let t1 = a1.xy + a1.xy;
    let p1 = t1 * vec2<f32>(-1.0, 1.0) + vec2<f32>(1.0, -1.0);
    let wp1 = k * p1;
    var mean = n0 * k + wp1;
    let m1 = vec3<f32>(a1.z, a1.w, b1.x * -2.0 + 1.0);
    var mom = m0 * k2 + k2 * m1;
    // The products of the layers' means (twice, the cross terms of the square of the sum).
    let d0 = wn0 + wn0;
    mom = mom + vec3<f32>(wp1.x * d0.x, wp1.y * d0.y, dot(wn0, vec2<f32>(wp1.y, wp1.x)));
    // Layer 2, turned the other way: q = (2 y - 1, 2 x - 1), its mean is (q.y, q.x).
    let a2 = river_lean_2a(inp.v2);
    let b2 = river_lean_2b(inp.v2);
    let q2 = a2.yx * 2.0 + -1.0;
    let wq2 = k * q2;
    mean = vec2<f32>(wq2.y, wq2.x) + mean;
    let m2 = vec3<f32>(a2.z, a2.w, b2.x * 2.0 + -1.0);
    mom = m2 * k2 + mom;
    mom = mom + vec3<f32>(d0.x * wq2.y, d0.y * wq2.x, dot(wn0, wq2));
    let c12 = vec2<f32>(wp1.x * wq2.y, wp1.y * wq2.x);
    mom = mom + vec3<f32>(c12 + c12, dot(wp1, wq2));
    // The covariance of the slopes: the moments minus the square of the mean.
    let cxx = mom.x + mean.x * -mean.x;
    let cyy = mom.y + mean.y * -mean.y;
    let cxy = mom.z + mean.x * -mean.y;
    // The light: the diffuse of the sun, two lobes of the sun, one of the sky (0.8 of it).
    let normal = vec3<f32>(mean, 1.0);
    let ndl = saturate(dot(water_consts.SunDir, normal));
    let to_eye = water_consts.v3CameraWorld - inp.v4;
    let view = to_eye * d3d_rsq(dot(to_eye, to_eye));
    let cc = cxy * cxy;
    let cxy2 = cxy + cxy;
    let h_sun = river_half_slope(view, water_consts.FakeViewSun, water_consts.SunDir, mean);
    let e_sun = d3d_rcp(water_consts.SpecSun.w);
    let sun1 = river_lobe(cxx + e_sun, cyy + e_sun, cc, cxy2, h_sun);
    let sun2 = river_lobe(e_sun * 16.0 + cxx, e_sun * 16.0 + cyy, cc, cxy2, h_sun);
    let h_sky = river_half_slope(view, water_consts.FakeViewSky, water_consts.SkyDir, mean);
    let e_sky = d3d_rcp(water_consts.SpecSky.w);
    let a_sky = e_sky + cxx;
    let b_sky = e_sky + cyy;
    let sky = river_lobe(a_sky, b_sky, cc, cxy2, h_sky);
    let det_sky = a_sky * b_sky - cc;
    let refl = river_reflection(inp.v4, cxx, cyy, det_sky);
    // Fresnel: (1 - n.v)^5 of the unnormalised normal.
    let nv = 1.0 - dot(normal, view);
    let nv2 = nv * nv;
    let nv4 = nv2 * nv2;
    let fresnel = nv * nv4;
    let spec = river_gate(sun1.z, sun1.x * water_consts.SpecSun.xyz) +
        river_gate(sky.z, (sky.x * water_consts.SpecSky.xyz) * 0.800000012);
    var col = spec * fresnel + wc.xyz;
    let wide = river_gate(sun2.z, (sun2.x * 0.00499999989) * (wc.xyz * water_consts.SpecSun.xyz));
    col = col * ndl + wide;
    col = refl.xyz * water_consts.ReflectionAmount + col;
    // The colour is scaled by 0.5 + (1 - n.v)^5 (brighter at a grazing angle) and the opacity comes
    // from the blend map.
    let blend = textureSample(WaterBlendMap, WaterBlendMap_smp, inp.v3);
    col = (nv * nv4 + 0.5) * col;
    let alpha = blend.w * saturate(refl.w + 1.0);
    // Gamma 1 / 2.2, the colour key, the overlay maps and the fog of war, the distance fog.
    col = exp2(vec3<f32>(d3d_log(col.x), d3d_log(col.y), d3d_log(col.z)) * 0.454545468);
    let lit = river_overlay_fow(col, inp.v6, fow);
    o.oc0 = vec4<f32>(river_fog(lit, inp.v4, inp.v5.y, inp.v5.z), alpha);
    return o;
}

// ---- PSRiverLow ----

struct RiverPsLowIn {
    // dcl_texcoord3 v0: the colour map's coordinate.
    v0: vec2<f32>,
    // dcl_texcoord4 v1.xyz: the point in the world.
    v1: vec3<f32>,
    // dcl_texcoord5 v2.yz: the clip y and w.
    v2: vec3<f32>,
    // dcl_texcoord6 v3.xy: the overlay maps' xy.
    v3: vec2<f32>,
}

// re: WaterShader::PSRiverLow (sha1 1ca0a6959556): no light, the colour of the water as it is, the
// alpha from `WaterBlendMap`.
fn psriverlow_body(inp: RiverPsLowIn) -> RiverPsOut {
    var o: RiverPsOut;
    let fow = textureSample(FOWMask, FOWMask_smp, inp.v3);
    o.kill = fow.x == 0.0;
    let wc = textureSample(WaterColor, WaterColor_smp, inp.v0);
    let blend = textureSample(WaterBlendMap, WaterBlendMap_smp, inp.v0);
    let col = exp2(vec3<f32>(d3d_log(wc.x), d3d_log(wc.y), d3d_log(wc.z)) * 0.454545468);
    let lit = river_overlay_fow(col, inp.v3, fow);
    o.oc0 = vec4<f32>(river_fog(lit, inp.v1, inp.v2.y, inp.v2.z), blend.w);
    return o;
}

// ---- RiverShader (the river banners) ----

// `RiverShader::VSRiver`: c0 `WVP`, c8 `OverlayProjection`, c12 `Local`, c16 `fTime`. The names are
// those of the CTAB, in a block of their own.
struct RiverBannerVsConsts {
    WVP: mat4x4<f32>,
    OverlayProjection: mat4x4<f32>,
    Local: mat4x4<f32>,
    fTime: f32,
    pad_a: f32,
    pad_b: f32,
    pad_c: f32,
}

@group(0) @binding(30) var<uniform> river_banner_vs: RiverBannerVsConsts;

// `RiverShader::PSRiver`: c0 `CameraPosition`, s1 `CultureBorderTexture`.
struct RiverBannerPsConsts {
    CameraPosition: vec3<f32>,
    pad_a: f32,
}

@group(0) @binding(31) var<uniform> river_banner_ps: RiverBannerPsConsts;
@group(0) @binding(32) var CultureBorderTexture: texture_2d<f32>;
@group(0) @binding(33) var CultureBorderTexture_smp: sampler;

struct BannerVsIn {
    // dcl_position v0: the point (w used), dcl_color v1, dcl_texcoord v2.
    v0: vec4<f32>,
    v1: vec4<f32>,
    v2: vec2<f32>,
}

struct BannerVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    // The texture coordinate, scrolled by the time along v.
    o2: vec2<f32>,
    // The overlay maps' coordinates.
    o3: vec2<f32>,
    // The point in the world.
    o4: vec3<f32>,
}

// re: RiverShader::VSRiver (sha1 37eeafee9116)
fn vsriver_body(inp: BannerVsIn) -> BannerVsOut {
    let v0 = inp.v0;
    let c = river_banner_vs;
    var o: BannerVsOut;
    var w = row_major_reg(c.Local, 1u) * v0.y;
    w = v0.x * row_major_reg(c.Local, 0u) + w;
    w = v0.z * row_major_reg(c.Local, 2u) + w;
    w = v0.w * row_major_reg(c.Local, 3u) + w;
    var p = w.y * row_major_reg(c.WVP, 1u);
    p = w.x * row_major_reg(c.WVP, 0u) + p;
    p = w.z * row_major_reg(c.WVP, 2u) + p;
    o.o0 = w.w * row_major_reg(c.WVP, 3u) + p;
    o.o4 = w.xyz;
    var q = row_major_reg(c.OverlayProjection, 1u).xy * v0.y;
    q = v0.x * row_major_reg(c.OverlayProjection, 0u).xy + q;
    q = v0.z * row_major_reg(c.OverlayProjection, 2u).xy + q;
    q = v0.w * row_major_reg(c.OverlayProjection, 3u).xy + q;
    o.o3 = q * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5);
    // def c4 = 0.5, -0.5, 0, 0.05: the texture runs along v at 0.05 per unit of time.
    o.o2 = c.fTime * -vec2<f32>(0.0, 0.0500000007) + inp.v2;
    o.o1 = inp.v1;
    return o;
}

struct BannerPsIn {
    // dcl_color v0.w: the opacity of the vertex.
    v0: vec4<f32>,
    // dcl_texcoord1 v1.xy: the overlay maps' coordinates.
    v1: vec2<f32>,
    // dcl_texcoord2 v2.xyz: the point in the world.
    v2: vec3<f32>,
}

// re: RiverShader::PSRiver (sha1 cb53c618b5be): a banner of the colour (2 - k, ..) near the camera
// fading to the vertex's alpha with the distance (`x` is smoothstep twice of the cosine of the
// angle to the ground), over `CultureBorderTexture` by its alpha.
fn psriver_body(inp: BannerPsIn) -> RiverPsOut {
    var o: RiverPsOut;
    let d = river_banner_ps.CameraPosition - inp.v2;
    var x = d3d_rsq(dot(d, d));
    x = saturate(x * d.z);
    var y = x * -2.0 + 3.0;
    x = saturate((x * x) * y);
    y = x * -2.0 + 3.0;
    x = (x * x) * y;
    let r1 = vec4<f32>(2.0, 2.0, 2.0, inp.v0.w);
    let r0 = x * vec4<f32>(-1.70588231, -1.64313722, -1.60000002, 0.0) + r1;
    let tex = textureSample(CultureBorderTexture, CultureBorderTexture_smp, inp.v1);
    o.oc0 = tex.w * (tex - r0) + r0;
    o.kill = false;
    return o;
}

// ---- the entries ----

struct WaterRiverVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) t0: vec2<f32>,
    @location(1) t1: vec2<f32>,
    @location(2) t2: vec2<f32>,
    @location(3) t3: vec2<f32>,
    @location(4) world: vec3<f32>,
    @location(5) clip_yw: vec2<f32>,
    @location(6) overlay: vec2<f32>,
}

@vertex
fn vswaterriver(
    @location(0) v0: vec3<f32>,
    @location(1) v1: vec2<f32>,
    @location(2) v2: vec2<f32>,
) -> WaterRiverVertex {
    var inp: WaterRiverVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    inp.v2 = v2;
    let r = vswaterriver_body(inp);
    var out: WaterRiverVertex;
    out.clip = r.o0;
    out.t0 = r.o1;
    out.t1 = r.o2;
    out.t2 = r.o3;
    out.t3 = r.o4;
    out.world = r.o5;
    out.clip_yw = r.o6.yz;
    out.overlay = r.o7.xy;
    return out;
}

@fragment
fn psriverhigh(in: WaterRiverVertex) -> @location(0) vec4<f32> {
    var inp: RiverPsHighIn;
    inp.v0 = in.t0;
    inp.v1 = in.t1;
    inp.v2 = in.t2;
    inp.v3 = in.t3;
    inp.v4 = in.world;
    inp.v5 = vec3<f32>(0.0, in.clip_yw);
    inp.v6 = in.overlay;
    let r = psriverhigh_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}

@fragment
fn psriverlow(in: WaterRiverVertex) -> @location(0) vec4<f32> {
    var inp: RiverPsLowIn;
    inp.v0 = in.t3;
    inp.v1 = in.world;
    inp.v2 = vec3<f32>(0.0, in.clip_yw);
    inp.v3 = in.overlay;
    let r = psriverlow_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}

struct BannerVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) t0: vec2<f32>,
    @location(2) overlay: vec2<f32>,
    @location(3) world: vec3<f32>,
}

@vertex
fn vsriver(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec2<f32>,
) -> BannerVertex {
    var inp: BannerVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    inp.v2 = v2;
    let r = vsriver_body(inp);
    var out: BannerVertex;
    out.clip = r.o0;
    out.color = r.o1;
    out.t0 = r.o2;
    out.overlay = r.o3;
    out.world = r.o4;
    return out;
}

@fragment
fn psriver(in: BannerVertex) -> @location(0) vec4<f32> {
    var inp: BannerPsIn;
    inp.v0 = in.color;
    inp.v1 = in.overlay;
    inp.v2 = in.world;
    let r = psriver_body(inp);
    return r.oc0;
}
