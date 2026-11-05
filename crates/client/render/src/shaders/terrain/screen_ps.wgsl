// The cached page on the screen (world-terrain WT-0, WT-4): `TerrainShader::TerrainRenderCachedDiffusePS`
// (the page of a cell) and `TerrainRenderCachedDiffuseAndSpecPS` (the page and the highlight page),
// SM3 entries of the original. Hand port in the convention of shader-re 4.8.1 (a body
// `fn <entry>_body(inp: In) -> Out` and a `@fragment` entry that only reads its inputs).
//
// The formulas of the end of the chain (shadow, overlay, fog of war, distance fog) are in
// `common.wgsl`; the forward pass on B1's geometry (`m1_forward.wgsl`) calls them with a page it
// makes itself. This file has what reads the page: its samples.
//
// Needs frame.wgsl, sm3.wgsl and common.wgsl (group 0: `frame_globals`, the shadow map, the colour
// volumes and the fog ramp). Group 1 is the cell: the page, the overlay, the fog of war. The page and
// the highlight page are read through `cached_diffuse` and `cached_spec` (`page_2d.wgsl`, or
// `page_array.wgsl` for the batched draw).
//
// Gamma: the page holds gamma 1 / 2.2 and the shader leaves it so; only the highlight page is
// linear (`pow(x, 2.2)` in, `pow(x, 1 / 2.2)` out).

@group(1) @binding(2) var TerrainOverlay: texture_2d<f32>;
@group(1) @binding(3) var TerrainOverlay_smp: sampler;
@group(1) @binding(4) var FOWMaskCached: texture_2d<f32>;
@group(1) @binding(5) var FOWMaskCached_smp: sampler;
@group(1) @binding(6) var FOWBaseCached: texture_2d<f32>;
@group(1) @binding(7) var FOWBaseCached_smp: sampler;
@group(1) @binding(10) var SpecMaskCached: texture_2d<f32>;
@group(1) @binding(11) var SpecMaskCached_smp: sampler;

// What the original keeps in float registers: the eye, and two switches (`bool`s that the compiler
// put into c11 and c12, so they are floats: 0 off).
struct TerrainConsts {
    g_CamPos: vec3<f32>,
    g_bApplyDiffuse: f32,
    g_bApplySpecular: f32,
}
@group(1) @binding(12) var<uniform> terrain_consts: TerrainConsts;

// The exponent of the gamma of the highlight page.
// re: TerrainShader::TerrainRenderCachedDiffuseAndSpecPS (sha1 edd7096eb6ab): def c0.w = 2.2
const SPEC_PAGE_GAMMA: f32 = 2.2;

struct CachedPsIn {
    // dcl_texcoord v0: the place in the page (xy) and in the base of the fog of war (zw).
    v0: vec4<f32>,
    // dcl_texcoord1 v1.xy: the place on the screen overlay and the fog-of-war mask.
    v1: vec2<f32>,
    // dcl_texcoord2 v2.xyz: the point in the world.
    v2: vec3<f32>,
    // dcl_texcoord3 v3.xy: the place in the highlight mask (x); y of the clip position (y).
    v3: vec2<f32>,
}

struct CachedPsOut {
    oc0: vec4<f32>,
}

// ---- TerrainRenderCachedDiffusePS ----

// re: TerrainShader::TerrainRenderCachedDiffusePS (sha1 c2a0240dd9c9)
fn terrain_render_cached_diffuse_ps_body(inp: CachedPsIn) -> CachedPsOut {
    let g_CamPos = terrain_consts.g_CamPos;
    let g_bApplyDiffuse = terrain_consts.g_bApplyDiffuse;
    let fog = terrain_fog_factor(inp.v2 - g_CamPos, frame_globals.g_FogConsts, inp.v3.y);

    let shadow_uv = terrain_shadow_uv(inp.v2, frame_globals.g_ShadowVP);
    let map = textureSample(DynamicShadowTexture, DynamicShadowTexture_smp, shadow_uv);
    let shadow = terrain_shadow_amount(map.x, frame_globals.g_ShadowFade);

    var page = cached_diffuse(inp.v0.xy).xyz;
    // `cmp r1.xyz, -g_bApplyDiffuse, 0, r1`: the page is dropped when the switch is off.
    if (-g_bApplyDiffuse >= 0.0) {
        page = vec3<f32>(0.0);
    }
    let overlay = textureSample(TerrainOverlay, TerrainOverlay_smp, inp.v1);
    let fow_base = textureSample(FOWBaseCached, FOWBaseCached_smp, inp.v0.zw).xyz;
    let fow_mask = textureSample(FOWMaskCached, FOWMaskCached_smp, inp.v1);

    let color = terrain_cached_tail(
        page,
        shadow,
        overlay,
        fow_base,
        fow_mask,
        fog,
        frame_globals.f3FogColor.xyz,
    );
    var o: CachedPsOut;
    o.oc0 = vec4<f32>(color, 1.0);
    return o;
}

// ---- TerrainRenderCachedDiffuseAndSpecPS ----

// The page and the highlight page are added in linear light (`pow(x, 2.2)`), the sum goes back to
// the gamma of the page; the switch `g_bApplySpecular` off leaves the page as it is.
// re: TerrainShader::TerrainRenderCachedDiffuseAndSpecPS (sha1 edd7096eb6ab)
fn terrain_render_cached_diffuse_and_spec_ps_body(inp: CachedPsIn) -> CachedPsOut {
    let g_CamPos = terrain_consts.g_CamPos;
    let g_bApplyDiffuse = terrain_consts.g_bApplyDiffuse;
    let g_bApplySpecular = terrain_consts.g_bApplySpecular;

    let spec_mask = textureSample(
        SpecMaskCached,
        SpecMaskCached_smp,
        inp.v3.xy * 0.5 + vec2<f32>(0.5),
    ).xyz;
    let spec = cached_spec(inp.v0.xy).xyz;
    let spec_linear = terrain_pow3(spec, SPEC_PAGE_GAMMA);
    var diffuse = cached_diffuse(inp.v0.xy).xyz;
    if (-g_bApplyDiffuse >= 0.0) {
        diffuse = vec3<f32>(0.0);
    }
    let diffuse_linear = terrain_pow3(diffuse, SPEC_PAGE_GAMMA);
    let sum = spec_linear * spec_mask + diffuse_linear;
    let sum_gamma = terrain_pow3(sum, PAGE_GAMMA);
    var page = sum_gamma;
    if (-g_bApplySpecular >= 0.0) {
        page = diffuse;
    }

    let shadow_uv = terrain_shadow_uv(inp.v2, frame_globals.g_ShadowVP);
    let map = textureSample(DynamicShadowTexture, DynamicShadowTexture_smp, shadow_uv);
    let shadow = terrain_shadow_amount(map.x, frame_globals.g_ShadowFade);
    let overlay = textureSample(TerrainOverlay, TerrainOverlay_smp, inp.v1);
    let fow_base = textureSample(FOWBaseCached, FOWBaseCached_smp, inp.v0.zw).xyz;
    let fow_mask = textureSample(FOWMaskCached, FOWMaskCached_smp, inp.v1);
    let fog = terrain_fog_factor(inp.v2 - g_CamPos, frame_globals.g_FogConsts, inp.v3.y);

    let color = terrain_cached_tail(
        page,
        shadow,
        overlay,
        fow_base,
        fow_mask,
        fog,
        frame_globals.f3FogColor.xyz,
    );
    var o: CachedPsOut;
    o.oc0 = vec4<f32>(color, 1.0);
    return o;
}

// ---- entries: the interface of the vertex stage of the cached terrain ----

struct CachedVaryings {
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec2<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec2<f32>,
}

@fragment
fn terrain_render_cached_diffuse_ps(in: CachedVaryings) -> @location(0) vec4<f32> {
    var b: CachedPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    return terrain_render_cached_diffuse_ps_body(b).oc0;
}

@fragment
fn terrain_render_cached_diffuse_and_spec_ps(in: CachedVaryings) -> @location(0) vec4<f32> {
    var b: CachedPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    return terrain_render_cached_diffuse_and_spec_ps_body(b).oc0;
}
