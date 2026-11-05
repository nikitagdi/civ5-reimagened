// WaterShader of the original, the ocean and the lakes (world-water WW-1, R2.16): `VSWaterOcean`
// and the three pixel shaders `PSOceanHigh`, `PSOceanMedium` and `PSOceanLow` (one per
// `TerrainWaterQuality`: 2, 1, 0). Hand ports in the convention of shader-re 4.8.1: each entry of the
// original is a body `fn <entry>_body(inp: In) -> Out` with the registers of the original as member
// names, and a `@vertex` / `@fragment` entry that only reads its inputs, calls the body and finishes
// the job the original did elsewhere (the discard of `texkill`).
//
// Needs sm3.wgsl. Group 0: binding 0 the constants (one block, [`WaterConsts`]), then the textures
// of the samplers of the CTAB in pairs (texture `S`, sampler `S_smp`). The block is the 592 bytes
// of `FireGrafix::WaterGlobalData` (RW-11, `WaterSystem::Render`), so a register `cN` of the pixel
// shaders is the register `N - 9` of the block and one of the vertex shader is `N - 4`; the names
// are those of the CTAB, the offsets are in the comments.
//
// The surface (WW-D4, RW-11): a flat quad at the height `fOceanHeight`; the vertex is
// `(x, y, u, v)` with `(x, y)` the place in the world of the original (x east, y north) and
// `(u, v)` the place in the height texture `WorldHeightMap`. The pixel shaders find the depth in
// that texture (A8 in the original, `R8Unorm` here: the format map of shader-re says where `w`
// becomes `x`), the water colour in `WaterColor` by the depth, the currents in `WaterBlendMap` and
// light the water with the LEAN maps (`LEAN1_1`: the mean slope and the second moments, `LEAN1_2`:
// the cross moment).
//
// Gamma: the frame is not sRGB. The shaders do their own `pow`: the reflection is squared in
// (2.2), the colour is squared out (1 / 2.2), the textures are raw `*Unorm` values.

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// `FireGrafix::WaterGlobalData`: float index of the first component in the comment.
struct WaterConsts {
    // 0: the view-projection of the camera (`row_major`).
    g_VP: mat4x4<f32>,
    // 16: the projection of the overlay maps (the fog of war, `OverlayMap`).
    OverlayProjection: mat4x4<f32>,
    // 32: `VSWaterRiver` (c12, c14 of the vertex shader; the rivers, R3.6) and the ocean's own.
    unused_c12: vec4<f32>,
    // 36: the repeats of the LEAN maps of the four layers per world unit (c13).
    WaterScale: vec4<f32>,
    // 40
    unused_c14: vec4<f32>,
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
    // 52: the divisor of the four weights of the layers.
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
    // 68
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
@group(0) @binding(11) var WorldHeightMap: texture_2d<f32>;
@group(0) @binding(12) var WorldHeightMap_smp: sampler;
@group(0) @binding(13) var WaterFogRamp: texture_2d<f32>;
@group(0) @binding(14) var WaterFogRamp_smp: sampler;
@group(0) @binding(15) var OverlayMap: texture_2d<f32>;
@group(0) @binding(16) var OverlayMap_smp: sampler;
@group(0) @binding(17) var FOWMask: texture_2d<f32>;
@group(0) @binding(18) var FOWMask_smp: sampler;
@group(0) @binding(19) var TransFOWColor: texture_3d<f32>;
@group(0) @binding(20) var TransFOWColor_smp: sampler;
@group(0) @binding(21) var DynamicShadowTexture: texture_2d<f32>;
@group(0) @binding(22) var DynamicShadowTexture_smp: sampler;

// ---- VSWaterOcean ----

struct OceanVsIn {
    // dcl_position v0: (x, y) in the world, (u, v) in the height texture.
    v0: vec4<f32>,
}

struct OceanVsOut {
    o0: vec4<f32>,
    // The texture coordinates of the four layers of LEAN.
    o1: vec2<f32>,
    o2: vec2<f32>,
    o3: vec2<f32>,
    o4: vec2<f32>,
    // The point in the world.
    o5: vec3<f32>,
    // The clip position without z.
    o6: vec3<f32>,
    // (the overlay maps' xy, the height texture's uv)
    o7: vec4<f32>,
}

// re: WaterShader::VSWaterOcean (sha1 8c7df6f4f161)
fn vswaterocean_body(inp: OceanVsIn) -> OceanVsOut {
    let v0 = inp.v0;
    let ws = water_consts.WaterScale;
    let off = water_consts.WaterOffset;
    let height = water_consts.fOceanHeight;
    var o: OceanVsOut;
    // The four layers: scaled, turned and shifted by the same offset (def c0 = 1, 0, -1, 0).
    o.o1 = v0.xy * ws.x + off;
    let r1 = vec4<f32>(ws.y, 1.0, -1.0, ws.y);
    o.o2 = vec2<f32>(r1.x * v0.x, r1.y * v0.y) * r1.zw + off;
    o.o3 = v0.xy * -ws.z + off;
    let r0 = vec4<f32>(1.0, ws.w, ws.w, -1.0);
    o.o4 = vec2<f32>(r0.x * v0.x, r0.y * v0.y) * r0.zw + off;
    // The overlay maps' coordinates: the world point through `OverlayProjection`, to texture space.
    let ov0 = row_major_reg(water_consts.OverlayProjection, 0u);
    let ov1 = row_major_reg(water_consts.OverlayProjection, 1u);
    let ov2 = row_major_reg(water_consts.OverlayProjection, 2u);
    let ov3 = row_major_reg(water_consts.OverlayProjection, 3u);
    var p = ov1.xy * v0.y;
    p = v0.x * ov0.xy + p;
    p = height * ov2.xy + p;
    p = p + ov3.xy;
    // The clip position.
    var c = row_major_reg(water_consts.g_VP, 1u) * v0.y;
    c = v0.x * row_major_reg(water_consts.g_VP, 0u) + c;
    c = height * row_major_reg(water_consts.g_VP, 2u) + c;
    c = c + row_major_reg(water_consts.g_VP, 3u);
    o.o0 = c;
    o.o6 = c.xyw;
    o.o5 = vec3<f32>(v0.xy, height);
    o.o7 = vec4<f32>(p * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5), v0.zw);
    return o;
}

// ---- the pixel shaders ----

struct OceanPsOut {
    oc0: vec4<f32>,
    kill: bool,
}

// A LEAN map as the original stores it, 16 bits per channel unorm; ours is half float, which is exact
// for the values of the maps to within 5e-4, so the value read is rounded to the grid of 16 bits
// (the oracle of shader-re does the same: the format map).
fn lean_a(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(LEAN1_1, LEAN1_1_smp, uv) * 65535.0) / 65535.0;
}

fn lean_b(uv: vec2<f32>) -> vec4<f32> {
    return round(textureSample(LEAN1_2, LEAN1_2_smp, uv) * 65535.0) / 65535.0;
}

// `(1 - depth, 0)` in `WaterColor`: the colour of the water from its depth (A8 height tile). The
// height is saturated at 1 / 4 of the range: the sea level is at 0.198.
fn ocean_water_color(height_tile: f32) -> vec4<f32> {
    let depth = saturate(height_tile * 4.0);
    return textureSampleLevel(WaterColor, WaterColor_smp, vec2<f32>(1.0 - depth, 0.0), 0.0);
}

// The current (`WaterBlendMap` at the world point through `g_WorldToTerrainTextureSpace`) in
// [-1, 1] x [-1, 1].
fn ocean_flow(world: vec3<f32>) -> vec2<f32> {
    let m = water_consts.g_WorldToTerrainTextureSpace;
    let m0 = row_major_reg(m, 0u);
    let m1 = row_major_reg(m, 1u);
    let m2 = row_major_reg(m, 2u);
    let m3 = row_major_reg(m, 3u);
    var uv = m1.xy * world.y;
    uv = world.x * m0.xy + uv;
    uv = world.z * m2.xy + uv;
    uv = uv + m3.xy;
    let c = textureSample(WaterBlendMap, WaterBlendMap_smp, uv);
    return c.xy * 2.0 + -1.0;
}

// The normalised direction from the point to the eye.
fn ocean_to_eye(world: vec3<f32>) -> vec3<f32> {
    let d = water_consts.v3CameraWorld - world;
    return d * d3d_rsq(dot(d, d));
}

// One lobe of the specular light: a Gaussian of the slope `h` of the half vector with the covariance
// of the LEAN map plus the roughness of the light: `a = cxx + e`, `b = cyy + e`, `cc = cxy^2`,
// `cxy2 = 2 cxy`. Gives (density, exponent, how many of the two safeguards fired: the determinant
// is not positive, the exponent is 16 or more).
fn ocean_lobe(a: f32, b: f32, cc: f32, cxy2: f32, h: vec2<f32>) -> vec3<f32> {
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
fn ocean_gate(flag: f32, v: vec3<f32>) -> vec3<f32> {
    return select(vec3<f32>(0.0), v, -flag >= 0.0);
}

// The half vector of a light in the slope space, minus the mean slope `m`: `view` is the direction
// to the eye, `fake` (xyz, blend w) the direction the lobe is lit from, `dir` the light's direction.
fn ocean_half_slope(view: vec3<f32>, fake: vec4<f32>, dir: vec3<f32>, m: vec2<f32>) -> vec2<f32> {
    var v = view - fake.xyz;
    v = fake.w * v + fake.xyz;
    let h = v * d3d_rsq(dot(v, v)) + dir;
    return h.xy * d3d_rcp(h.z) + -m;
}

// The reflection map: four taps around the point, spread by the covariance and by `spread` (the
// determinant of the sky lobe, which the original uses so), averaged; rgb in linear light.
fn ocean_reflection(world: vec3<f32>, cxx: f32, cyy: f32, spread: f32) -> vec4<f32> {
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

// The light of the LEAN qualities (High and Medium): the diffuse of the sun, two lobes of the sun
// specular and one of the sky, the Fresnel term, the reflection and the shine of the depth.
// `mean` is the mean slope of the surface and (`cxx`, `cyy`, `cxy`) its covariance. Gives the colour
// and the opacity.
fn ocean_light_lean(
    wc: vec4<f32>,
    depth: f32,
    mean: vec2<f32>,
    cxx: f32,
    cyy: f32,
    cxy: f32,
    world: vec3<f32>,
) -> vec4<f32> {
    let normal = vec3<f32>(mean, 1.0);
    let ndl = saturate(dot(water_consts.SunDir, normal));
    let view = ocean_to_eye(world);
    let cc = cxy * cxy;
    let cxy2 = cxy + cxy;
    // The sun: the sharp lobe and a wide one (16 times the roughness) scaled by 0.005.
    let h_sun = ocean_half_slope(view, water_consts.FakeViewSun, water_consts.SunDir, mean);
    let e_sun = d3d_rcp(water_consts.SpecSun.w);
    let sun1 = ocean_lobe(cxx + e_sun, cyy + e_sun, cc, cxy2, h_sun);
    let sun2 = ocean_lobe(e_sun * 16.0 + cxx, e_sun * 16.0 + cyy, cc, cxy2, h_sun);
    // The sky.
    let h_sky = ocean_half_slope(view, water_consts.FakeViewSky, water_consts.SkyDir, mean);
    let e_sky = d3d_rcp(water_consts.SpecSky.w);
    let a_sky = e_sky + cxx;
    let b_sky = e_sky + cyy;
    let sky = ocean_lobe(a_sky, b_sky, cc, cxy2, h_sky);
    let det_sky = a_sky * b_sky - cc;
    let refl = ocean_reflection(world, cxx, cyy, det_sky);
    // Fresnel: (1 - n.v)^5 of the unnormalised normal.
    let nv = 1.0 - dot(normal, view);
    let nv2 = nv * nv;
    let fresnel = nv * (nv2 * nv2);
    let spec = ocean_gate(sky.z, sky.x * water_consts.SpecSky.xyz) +
        ocean_gate(sun1.z, sun1.x * water_consts.SpecSun.xyz);
    var col = spec * fresnel + wc.xyz;
    let wide = ocean_gate(sun2.z, (sun2.x * 0.00499999989) * (wc.xyz * water_consts.SpecSun.xyz));
    col = col * ndl + wide;
    col = refl.xyz * water_consts.ReflectionAmount + col;
    let shine = depth * (sky.y * (sky.y * sky.y));
    col = shine * wc.xyz + col;
    return vec4<f32>(col, saturate(refl.w * wc.w + wc.w));
}

// What every quality does after the light: the shadow, gamma, the overlay maps, the fog of war and
// the distance fog.
fn ocean_finish(
    light: vec3<f32>,
    world: vec3<f32>,
    clip_y: f32,
    clip_w: f32,
    overlay_uv: vec2<f32>,
    fow: vec4<f32>,
) -> vec3<f32> {
    // The shadow (the sun map): the colour is darkened by up to 0.67.
    let sm = water_consts.g_ShadowVP;
    var s = row_major_reg(sm, 1u).xyw * world.y;
    s = world.x * row_major_reg(sm, 0u).xyw + s;
    s = world.z * row_major_reg(sm, 2u).xyw + s;
    s = s + row_major_reg(sm, 3u).xyw;
    let sndc = d3d_rcp(s.z) * s.xy;
    let suv = sndc * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5);
    let shadow = saturate(textureSample(DynamicShadowTexture, DynamicShadowTexture_smp, suv).x);
    let k = shadow * water_consts.g_ShadowFade;
    var col = (k * light) * -0.670000017 + light;
    // Gamma 1 / 2.2.
    col = exp2(vec3<f32>(d3d_log(col.x), d3d_log(col.y), d3d_log(col.z)) * 0.454545468);
    // The overlay maps: premultiplied.
    let ov = textureSample(OverlayMap, OverlayMap_smp, overlay_uv);
    col = col * (-ov.w + 1.0) + ov.xyz;
    // The fog of war: toward the colour key by what was seen and what is seen now.
    col = col + -0.400000006;
    col = fow.x * col + 0.400000006;
    let key = textureSample(TransFOWColor, TransFOWColor_smp, col).xyz;
    let seen = fow.z * -fow.x + fow.x;
    let lit = seen * (key - col) + col;
    // The distance fog: the ramp by the height on the screen, the density by the height and the
    // distance of the point.
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

// The four directions of the current `f` the layers are weighted by: `f.x - f.y`, `-f.x - f.y` and
// their opposites.
fn ocean_current_signs(f: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(f.x - f.y, -f.x - f.y, -(f.x - f.y), f.x + f.y);
}

// The weights of the two layers of Medium and Low: (2 + s) normalised, divided by `GradientScale`.
fn ocean_two_weights(f: vec2<f32>) -> vec2<f32> {
    var w0 = (f.x - f.y) + 2.0;
    var w1 = (-f.x - f.y) + 2.0;
    var r = d3d_rcp(w0 + w1);
    w0 = w0 * r;
    w1 = w1 * r;
    r = d3d_rcp(water_consts.GradientScale);
    return vec2<f32>(w0 * r, w1 * r);
}

// ---- PSOceanHigh ----

struct OceanPsHighIn {
    // dcl_texcoord .. 3 v0 .. v3: the four layers of LEAN.
    v0: vec2<f32>,
    v1: vec2<f32>,
    v2: vec2<f32>,
    v3: vec2<f32>,
    // dcl_texcoord4 v4.xyz: the point in the world.
    v4: vec3<f32>,
    // dcl_texcoord5 v5.yz: the clip y and w.
    v5: vec3<f32>,
    // dcl_texcoord6 v6: the overlay maps' xy, the height texture's zw.
    v6: vec4<f32>,
}

// re: WaterShader::PSOceanHigh (sha1 9b0080e09057)
fn psoceanhigh_body(inp: OceanPsHighIn) -> OceanPsOut {
    var o: OceanPsOut;
    let fow = textureSample(FOWMask, FOWMask_smp, inp.v6.xy);
    o.kill = fow.x == 0.0;
    let tile = textureSample(WorldHeightMap, WorldHeightMap_smp, inp.v6.zw).x;
    let depth = saturate(tile * 4.0);
    let wc = ocean_water_color(tile);
    let f = ocean_flow(inp.v4);
    // The weights of the four layers: (2 + s)^4, normalised.
    var w = ocean_current_signs(f) + 2.0;
    w = w * w;
    w = w * w;
    w = d3d_rcp(w.x + w.y + w.z + w.w) * w;
    w = d3d_rcp(water_consts.GradientScale) * w;
    let w2 = w * w;
    // Layers 0 and 1: the mean slope n, the second moments (xx, yy, xy) m.
    let a0 = lean_a(inp.v0);
    let b0 = lean_b(inp.v0);
    let n0 = a0.xy * 2.0 + -1.0;
    let wn0 = w.x * n0;
    let m0 = vec3<f32>(a0.z, a0.w, b0.x * 2.0 + -1.0);
    let a1 = lean_a(inp.v1);
    let b1 = lean_b(inp.v1);
    let t1 = a1.xy + a1.xy;
    let n1 = t1 * vec2<f32>(-1.0, 1.0) + vec2<f32>(1.0, -1.0);
    let wn1 = w.y * n1;
    var mean = n0 * w.x + wn1;
    let m1 = vec3<f32>(a1.z, a1.w, b1.x * -2.0 + 1.0);
    let m1w = w2.y * m1;
    var mom = m0 * w2.x + m1w;
    // The products of the layers' means (twice, the cross terms of the square of the sum).
    let d0 = wn0 + wn0;
    mom = mom + vec3<f32>(wn1.x * d0.x, wn1.y * d0.y, dot(wn0, vec2<f32>(wn1.y, wn1.x)));
    // Layer 2.
    let a2 = lean_a(inp.v2);
    let b2 = lean_b(inp.v2);
    let n2 = a2.xy * -2.0 + 1.0;
    let wn2 = w.z * n2;
    mean = n2 * w.z + mean;
    let m2 = vec3<f32>(a2.z, a2.w, b2.x * 2.0 + -1.0);
    mom = m2 * w2.z + mom;
    mom = mom + vec3<f32>(d0.x * wn2.x, d0.y * wn2.y, dot(wn0, vec2<f32>(wn2.y, wn2.x)));
    let d1 = wn1 + wn1;
    mom = mom + vec3<f32>(wn2.x * d1.x, wn2.y * d1.y, dot(wn1, vec2<f32>(wn2.y, wn2.x)));
    // Layer 3, turned a quarter.
    let a3 = lean_a(inp.v3);
    let b3 = lean_b(inp.v3);
    let u3 = vec2<f32>((a3.y + a3.y) * -1.0 + 1.0, (a3.x + a3.x) * 1.0 + -1.0);
    let wn3 = w.w * u3;
    mean = vec2<f32>(u3.y, u3.x) * w.w + mean;
    let m3 = vec3<f32>(a3.z, a3.w, b3.x * -2.0 + 1.0);
    mom = m3 * w2.w + mom;
    mom = mom + vec3<f32>(d0.x * wn3.y, d0.y * wn3.x, dot(wn0, wn3));
    mom = mom + vec3<f32>(d1.x * wn3.y, d1.y * wn3.x, dot(wn1, wn3));
    let c23 = vec2<f32>(wn2.x * wn3.y, wn2.y * wn3.x);
    mom = mom + vec3<f32>(c23 + c23, dot(wn2, wn3));
    // The covariance of the slopes: the moments minus the square of the mean.
    let cxx = mom.x + mean.x * -mean.x;
    let cyy = mom.y + mean.y * -mean.y;
    let cxy = mom.z + mean.x * -mean.y;
    let lit = ocean_light_lean(wc, depth, mean, cxx, cyy, cxy, inp.v4);
    o.oc0 = vec4<f32>(
        ocean_finish(lit.xyz, inp.v4, inp.v5.y, inp.v5.z, inp.v6.xy, fow),
        lit.w,
    );
    return o;
}

// ---- PSOceanMedium ----

struct OceanPsMediumIn {
    // dcl_texcoord, dcl_texcoord1 v0, v1: two layers of LEAN.
    v0: vec2<f32>,
    v1: vec2<f32>,
    // dcl_texcoord4 v2.xyz: the point in the world.
    v2: vec3<f32>,
    // dcl_texcoord5 v3.yz: the clip y and w.
    v3: vec3<f32>,
    // dcl_texcoord6 v4: the overlay maps' xy, the height texture's zw.
    v4: vec4<f32>,
}

// re: WaterShader::PSOceanMedium (sha1 19ce65aeff0e)
fn psoceanmedium_body(inp: OceanPsMediumIn) -> OceanPsOut {
    var o: OceanPsOut;
    let fow = textureSample(FOWMask, FOWMask_smp, inp.v4.xy);
    o.kill = fow.x == 0.0;
    let tile = textureSample(WorldHeightMap, WorldHeightMap_smp, inp.v4.zw).x;
    let depth = saturate(tile * 4.0);
    let wc = ocean_water_color(tile);
    let f = ocean_flow(inp.v2);
    let w = ocean_two_weights(f);
    let w2 = w * w;
    let a0 = lean_a(inp.v0);
    let b0 = lean_b(inp.v0);
    let n0 = a0.xy * 2.0 + -1.0;
    let wn0 = w.x * n0;
    let m0 = vec3<f32>(a0.z, a0.w, b0.x * 2.0 + -1.0);
    // Layer 1, turned: p = (2 y - 1, 1 - 2 x), its mean is (p.y, p.x).
    let a1 = lean_a(inp.v1);
    let b1 = lean_b(inp.v1);
    let p1 = vec2<f32>((a1.y + a1.y) * 1.0 + -1.0, (a1.x + a1.x) * -1.0 + 1.0);
    let wp1 = w.y * p1;
    let mean = n0 * w.x + vec2<f32>(wp1.y, wp1.x);
    let m1 = vec3<f32>(a1.z, a1.w, b1.x * -2.0 + 1.0);
    let m1w = w2.y * m1;
    var mom = m0 * w2.x + m1w;
    let c01 = vec2<f32>(wn0.x * wp1.y, wn0.y * wp1.x);
    mom = mom + vec3<f32>(c01 + c01, dot(wn0, wp1));
    let cxx = mom.x + mean.x * -mean.x;
    let cyy = mom.y + mean.y * -mean.y;
    let cxy = mom.z + mean.x * -mean.y;
    let lit = ocean_light_lean(wc, depth, mean, cxx, cyy, cxy, inp.v2);
    o.oc0 = vec4<f32>(
        ocean_finish(lit.xyz, inp.v2, inp.v3.y, inp.v3.z, inp.v4.xy, fow),
        lit.w,
    );
    return o;
}

// ---- PSOceanLow ----

struct OceanPsLowIn {
    v0: vec2<f32>,
    v1: vec2<f32>,
    v2: vec3<f32>,
    v3: vec3<f32>,
    v4: vec4<f32>,
}

// re: WaterShader::PSOceanLow (sha1 e0c64489fa3d): no reflection and no specular, the diffuse of
// the sun on the colour of the water; the slope is the mean of two layers.
fn psoceanlow_body(inp: OceanPsLowIn) -> OceanPsOut {
    var o: OceanPsOut;
    let fow = textureSample(FOWMask, FOWMask_smp, inp.v4.xy);
    o.kill = fow.x == 0.0;
    let tile = textureSample(WorldHeightMap, WorldHeightMap_smp, inp.v4.zw).x;
    let wc = ocean_water_color(tile);
    let f = ocean_flow(inp.v2);
    let w = ocean_two_weights(f);
    let a0 = lean_a(inp.v0);
    let n0 = a0.xy * 2.0 + -1.0;
    let a1 = lean_a(inp.v1);
    let t1 = a1.xy + a1.xy;
    let n1 = t1 * vec2<f32>(-1.0, 1.0) + vec2<f32>(1.0, -1.0);
    let wn1 = w.y * n1;
    let mean = n0 * w.x + wn1;
    let ndl = saturate(dot(water_consts.SunDir, vec3<f32>(mean, 1.0)));
    let light = ndl * wc.xyz;
    o.oc0 = vec4<f32>(
        ocean_finish(light, inp.v2, inp.v3.y, inp.v3.z, inp.v4.xy, fow),
        saturate(wc.w),
    );
    return o;
}

// ---- the entries ----

struct OceanVertex {
    @builtin(position) clip: vec4<f32>,
    @location(0) t0: vec2<f32>,
    @location(1) t1: vec2<f32>,
    @location(2) t2: vec2<f32>,
    @location(3) t3: vec2<f32>,
    @location(4) world: vec3<f32>,
    @location(5) clip_yw: vec2<f32>,
    @location(6) overlay: vec4<f32>,
}

@vertex
fn vswaterocean(@location(0) v0: vec4<f32>) -> OceanVertex {
    var inp: OceanVsIn;
    inp.v0 = v0;
    let r = vswaterocean_body(inp);
    var out: OceanVertex;
    out.clip = r.o0;
    out.t0 = r.o1;
    out.t1 = r.o2;
    out.t2 = r.o3;
    out.t3 = r.o4;
    out.world = r.o5;
    out.clip_yw = r.o6.yz;
    out.overlay = r.o7;
    return out;
}

@fragment
fn psoceanhigh(in: OceanVertex) -> @location(0) vec4<f32> {
    var inp: OceanPsHighIn;
    inp.v0 = in.t0;
    inp.v1 = in.t1;
    inp.v2 = in.t2;
    inp.v3 = in.t3;
    inp.v4 = in.world;
    inp.v5 = vec3<f32>(0.0, in.clip_yw);
    inp.v6 = in.overlay;
    let r = psoceanhigh_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}

@fragment
fn psoceanmedium(in: OceanVertex) -> @location(0) vec4<f32> {
    var inp: OceanPsMediumIn;
    inp.v0 = in.t0;
    inp.v1 = in.t1;
    inp.v2 = in.world;
    inp.v3 = vec3<f32>(0.0, in.clip_yw);
    inp.v4 = in.overlay;
    let r = psoceanmedium_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}

@fragment
fn psoceanlow(in: OceanVertex) -> @location(0) vec4<f32> {
    var inp: OceanPsLowIn;
    inp.v0 = in.t0;
    inp.v1 = in.t1;
    inp.v2 = in.world;
    inp.v3 = vec3<f32>(0.0, in.clip_yw);
    inp.v4 = in.overlay;
    let r = psoceanlow_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}
