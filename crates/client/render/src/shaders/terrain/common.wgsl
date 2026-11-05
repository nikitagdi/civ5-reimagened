// Terrain: the parts the ported entries of `TerrainShader` share (world-terrain WT-0, WT-D7).
//
// Pure formulas of the original, written once. Each is called by the body of a ported entry (it
// is what `shader-re compare` checks, in `bake_cache.wgsl` and `screen_ps.wgsl`) and by the
// forward pass on B1's geometry (`m1_forward.wgsl`), so the milestone M1 does not copy a formula.
// What needs a texture sample that depends on an earlier result (the colour volumes) is sampled
// by the function itself, from the resources declared here.
//
// Needs sm3.wgsl and frame.wgsl. Group 0 is the frame group; `frame_globals` is binding 1
// (`frame.wgsl`), the light textures of the world are 8..15 (declared here):
//
//   8  DynamicShadowTexture (the shadow map)     10 ShadowColorCached (colour volume)
//   12 TransFOWColorCached (colour volume)       14 FogRampCached
//
// Each texture has its sampler at the next binding (`<name>_smp`). The modules that include this
// file keep their own resources at other bindings.

const LOG2E: f32 = 1.44269502;

@group(0) @binding(8) var DynamicShadowTexture: texture_2d<f32>;
@group(0) @binding(9) var DynamicShadowTexture_smp: sampler;
@group(0) @binding(10) var ShadowColorCached: texture_3d<f32>;
@group(0) @binding(11) var ShadowColorCached_smp: sampler;
@group(0) @binding(12) var TransFOWColorCached: texture_3d<f32>;
@group(0) @binding(13) var TransFOWColorCached_smp: sampler;
@group(0) @binding(14) var FogRampCached: texture_2d<f32>;
@group(0) @binding(15) var FogRampCached_smp: sampler;

// Register `k` of a `row_major float4x4` that was filled by CTAB name (reg k = row k): component j
// of register k is the matrix member `[j][k]` (the layout of `FrameGlobals`, `frame.wgsl`).
fn terrain_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// The clip position of a point of the original's basis: `p.y * c5 + p.x * c4 + p.z * c6 + c7`,
// the order of the vertex shader of the original (`ViewProjMatrix`, rows 0..3).
fn terrain_clip(p: vec3<f32>) -> vec4<f32> {
    let c4 = terrain_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = terrain_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = terrain_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = terrain_reg(frame_globals.ViewProjMatrix, 3u);
    var r = p.y * c5;
    r = p.x * c4 + r;
    r = p.z * c6 + r;
    return r + c7;
}

// D3D9 `lrp(s, a, b)` = s * (a - b) + b.
fn terrain_lrp(s: f32, a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    return s * (a - b) + b;
}

// ---- the light of the cache ("Terrain Cache": TerrainRenderCacheDiffuse and ...AndSpec) ----

// The brightness of the light cubes: the cubes hold values the shader scales by 1.7.
// re: TerrainShader::TerrainRenderCacheDiffuse (sha1 4849b2744930): def c1.x = 1.7
const CUBE_LIGHT_SCALE: f32 = 1.7;
// Added to the lit colour so that its log is finite.
// re: TerrainShader::TerrainRenderCacheDiffuse (sha1 4849b2744930): def c1.y = 0.001
const LIGHT_FLOOR: f32 = 0.001;
// The page holds gamma 1 / 2.2.
// re: TerrainShader::TerrainRenderCacheDiffuse (sha1 4849b2744930): def c1.z = 0.454545468
const PAGE_GAMMA: f32 = 0.454545468;

// The tangent frame of a normal `n` (the original has no tangents in the mesh): `t = n x (0, 1, 0)`
// and `b = n x (1, 0, 0)`, each normalised in its two non-zero components, as `dp2add` / `rsq` do.
// Returns the direction of a tangent-space normal `tn` (0..1 texture value already x2 - 1) in the
// space of `n`.
// re: TerrainShader::TerrainRenderCacheDiffuse (sha1 4849b2744930), the first 12 instructions
fn terrain_light_direction(n: vec3<f32>, tn: vec3<f32>) -> vec3<f32> {
    // t = (-n.z, 0, n.x), b = (0, n.z, -n.y).
    var t = vec3<f32>(-n.z, 0.0, n.x);
    var b = vec3<f32>(0.0, n.z, -n.y);
    t = t * d3d_rsq(dot(t.xz, t.xz));
    b = b * d3d_rsq(dot(b.yz, b.yz));
    var d = b * tn.y;
    d = tn.x * t + d;
    d = tn.z * n + d;
    return d;
}

// The light the cache stores before the gamma and the colour volume: the sun cube, mixed to the
// shadow cube by the macro shadow `s`, times the albedo.
fn terrain_cache_light(sun: vec3<f32>, shade: vec3<f32>, s: f32, albedo: vec3<f32>) -> vec3<f32> {
    let sun_k = sun * CUBE_LIGHT_SCALE;
    let delta = shade * CUBE_LIGHT_SCALE - sun_k;
    let lit = s * delta + sun_k;
    return albedo * lit + LIGHT_FLOOR;
}

// pow(c, 1 / 2.2) per component, as the shader does it: log, multiply, exp.
fn terrain_page_gamma(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        exp2(d3d_log(c.x) * PAGE_GAMMA),
        exp2(d3d_log(c.y) * PAGE_GAMMA),
        exp2(d3d_log(c.z) * PAGE_GAMMA),
    );
}

// pow(c, e) per component (log, multiply, exp).
fn terrain_pow3(c: vec3<f32>, e: f32) -> vec3<f32> {
    return vec3<f32>(
        exp2(d3d_log(c.x) * e),
        exp2(d3d_log(c.y) * e),
        exp2(d3d_log(c.z) * e),
    );
}

// ---- the cached page on the screen (TerrainRenderCachedDiffusePS and ...AndSpecPS) ----

// The fog of the distance: exponential in height and distance (`g_FogConsts` = height, distance, -,
// row of the ramp), times the ramp at the height of the point on the screen (`ndc_y`, -1..1).
// re: TerrainShader::TerrainRenderCachedDiffusePS (sha1 c2a0240dd9c9)
fn terrain_fog_factor(d: vec3<f32>, fog_consts: vec4<f32>, ndc_y: f32) -> f32 {
    var a = d.z * fog_consts.x;
    a = a * LOG2E;
    a = exp2(a);
    a = -a + 1.0;
    let dist2 = dot(d, d);
    let inv_dz = d3d_rcp(d.z);
    var f = dist2 * fog_consts.y;
    f = a * f;
    f = inv_dz * f;
    f = f * LOG2E;
    f = saturate(exp2(f));
    f = -f + 1.0;
    let ramp_u = ndc_y * 0.5 + 0.5;
    let ramp = textureSample(FogRampCached, FogRampCached_smp, vec2<f32>(ramp_u, fog_consts.w));
    return f * ramp.x;
}

// The coordinates of a point in the shadow map: `p` through `g_ShadowVP` (x, y and w of the clip
// position), then (0.5, -0.5) * ndc - 0.5 (the sampler repeats, so this is 0.5 * x + 0.5 and
// 0.5 - 0.5 * y).
// re: TerrainShader::TerrainRenderCachedDiffusePS (sha1 c2a0240dd9c9)
fn terrain_shadow_uv(p: vec3<f32>, g_ShadowVP: mat4x4<f32>) -> vec2<f32> {
    let c15 = terrain_reg(g_ShadowVP, 0u);
    let c16 = terrain_reg(g_ShadowVP, 1u);
    let c17 = terrain_reg(g_ShadowVP, 2u);
    let c18 = terrain_reg(g_ShadowVP, 3u);
    var r = vec3<f32>(c16.x, c16.y, c16.w) * p.y;
    r = p.x * vec3<f32>(c15.x, c15.y, c15.w) + r;
    r = p.z * vec3<f32>(c17.x, c17.y, c17.w) + r;
    r = r + vec3<f32>(c18.x, c18.y, c18.w);
    let inv_w = d3d_rcp(r.z);
    let ndc = vec2<f32>(inv_w * r.x, inv_w * r.y);
    return ndc * vec2<f32>(0.5, -0.5) + vec2<f32>(-0.5, -0.5);
}

// What the shadow does to the page: the strength is the map's value, clamped, times
// `g_ShadowFade`.
fn terrain_shadow_amount(map_x: f32, g_ShadowFade: f32) -> f32 {
    return saturate(map_x) * g_ShadowFade;
}

// From the colour of the page to the colour on the screen: the shadow recolours it through
// `ShadowColorCached`, the overlay is laid over it, the fog of war takes its base colour where
// nothing is known and its volume `TransFOWColorCached` where the ground is known but not seen, and
// the distance fog goes last. `fog` is [`terrain_fog_factor`].
//
// `fow_mask`: x 1 where the ground is known, z 1 where it is seen now. `shadow` is
// [`terrain_shadow_amount`]. `overlay` is the screen overlay (rgb, coverage).
// re: TerrainShader::TerrainRenderCachedDiffusePS (sha1 c2a0240dd9c9)
fn terrain_cached_tail(
    page: vec3<f32>,
    shadow: f32,
    overlay: vec4<f32>,
    fow_base: vec3<f32>,
    fow_mask: vec4<f32>,
    fog: f32,
    fog_color: vec3<f32>,
) -> vec3<f32> {
    let shade_color = textureSample(ShadowColorCached, ShadowColorCached_smp, page).xyz;
    let shaded = terrain_lrp(shadow, shade_color, page);
    let covered = shaded * (1.0 - overlay.w) + overlay.xyz;
    let seen = terrain_lrp(fow_mask.x, covered, fow_base);
    let known_not_seen = fow_mask.z * -fow_mask.x + fow_mask.x;
    let trans = textureSample(TransFOWColorCached, TransFOWColorCached_smp, seen).xyz;
    let c = terrain_lrp(known_not_seen, trans, seen);
    return fog * (fog_color - c) + c;
}
