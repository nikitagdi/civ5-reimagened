// "Terrain Cache" of the original: the light of a cell, baked into its page (world-terrain WT-0,
// WT-3). `TerrainShader::TerrainRenderCacheDiffuse` (the page) and `TerrainRenderCacheDiffuseAndSpec`
// (the page and the highlight, two targets). Hand port in the convention of shader-re 4.8.1:
// a body `fn <entry>_body(inp: In) -> Out` with the registers of the original as member names, and a
// `@fragment` entry that only reads its inputs and calls the body.
//
// The formulas are in `common.wgsl` (the forward pass of the milestone M1 uses them too); this
// file has the samples. Needs frame.wgsl, sm3.wgsl and common.wgsl.
//
// Resources carry the names of the CTAB (a sampler `S` is the texture `S` and the sampler `S_smp`):
// group 1 holds what the cell's page needs: the normal, the albedo and the macro shadow of the
// cell, the cubes of the sun and of the shadow of the style, the colour volume of the style.
// Gamma: the albedo is x^2, the page holds gamma 1 / 2.2; only the
// light cubes come through an sRGB format.

@group(1) @binding(0) var NormalTexture: texture_2d<f32>;
@group(1) @binding(1) var NormalTexture_smp: sampler;
@group(1) @binding(2) var AlbiedoTexture: texture_2d<f32>;
@group(1) @binding(3) var AlbiedoTexture_smp: sampler;
@group(1) @binding(4) var ShadowTexture: texture_2d<f32>;
@group(1) @binding(5) var ShadowTexture_smp: sampler;
@group(1) @binding(6) var SpecTexture: texture_2d<f32>;
@group(1) @binding(7) var SpecTexture_smp: sampler;
@group(1) @binding(8) var SunCubeLight: texture_cube<f32>;
@group(1) @binding(9) var SunCubeLight_smp: sampler;
@group(1) @binding(10) var ShadowCubeLight: texture_cube<f32>;
@group(1) @binding(11) var ShadowCubeLight_smp: sampler;
@group(1) @binding(12) var ColorCorrectTexture: texture_3d<f32>;
@group(1) @binding(13) var ColorCorrectTexture_smp: sampler;

// `g_Half` of the highlight: the half vector between the sun and the eye. Float register c5.
struct CacheConsts {
    g_Half: vec3<f32>,
}
@group(1) @binding(14) var<uniform> cache_consts: CacheConsts;

// The size of the highlight: `SpecTexture.a * 64 + 1` is the exponent.
// re: TerrainShader::TerrainRenderCacheDiffuseAndSpec (sha1 5ac206dc4a8a): def c1.zw = (64, 1)
const SPEC_EXPONENT_SCALE: f32 = 64.0;

// The light cubes are bound through an sRGB format (`LightingEnv::terrain_sunlight_srgb`): the D3D11
// build of the original binds the same cube resources as `R8G8B8A8_SRGB`/`BC1_SRGB` to
// `BuildingShader` (S1a ev 1897, resources 2002, 2005, 2008; `UnitShader` reads them `UNORM`). The
// sampler decodes them, so the shader reads them as the block does (`texld`, no gamma of its own).

// ---- TerrainRenderCacheDiffuse ----

struct CacheDiffuseIn {
    // dcl_normal v0.xyz: the normal of the ground at the vertex, in the original's basis.
    v0: vec3<f32>,
    // dcl_texcoord v1.xy: the place in the cell's temporary targets.
    v1: vec2<f32>,
}

struct CacheDiffuseOut {
    oc0: vec4<f32>,
}

// re: TerrainShader::TerrainRenderCacheDiffuse (sha1 4849b2744930)
fn terrain_cache_diffuse_ps_body(inp: CacheDiffuseIn) -> CacheDiffuseOut {
    // The normal map holds a tangent-space normal as 0..1.
    let nm = textureSample(NormalTexture, NormalTexture_smp, inp.v1);
    let tn = vec3<f32>(nm.x * 2.0 - 1.0, nm.y * 2.0 - 1.0, nm.z * 2.0 - 1.0);
    let dir = terrain_light_direction(inp.v0, tn);
    let shade = textureSample(ShadowCubeLight, ShadowCubeLight_smp, dir).xyz;
    let sun = textureSample(SunCubeLight, SunCubeLight_smp, dir).xyz;
    let s = textureSample(ShadowTexture, ShadowTexture_smp, inp.v1).x;
    let albedo = textureSample(AlbiedoTexture, AlbiedoTexture_smp, inp.v1).xyz;
    let lit = terrain_cache_light(sun, shade, s, albedo);
    let page = terrain_page_gamma(lit);
    let corrected = textureSample(ColorCorrectTexture, ColorCorrectTexture_smp, page).xyz;
    var o: CacheDiffuseOut;
    o.oc0 = vec4<f32>(corrected, 1.0);
    return o;
}

// ---- TerrainRenderCacheDiffuseAndSpec ----

struct CacheDiffuseAndSpecOut {
    oc0: vec4<f32>,
    oc1: vec4<f32>,
}

// The page of the diffuse light into `oc0` and the highlight into `oc1`: the highlight is the
// specular texture times `(g_Half . dir) ^ (a * 64 + 1)` times the exponent times the light that
// is not in shadow, with the gamma and the colour volume of the page.
// re: TerrainShader::TerrainRenderCacheDiffuseAndSpec (sha1 5ac206dc4a8a)
fn terrain_cache_diffuse_and_spec_ps_body(inp: CacheDiffuseIn) -> CacheDiffuseAndSpecOut {
    let g_Half = cache_consts.g_Half;
    let nm = textureSample(NormalTexture, NormalTexture_smp, inp.v1);
    let tn = vec3<f32>(nm.x * 2.0 - 1.0, nm.y * 2.0 - 1.0, nm.z * 2.0 - 1.0);
    let dir = terrain_light_direction(inp.v0, tn);
    let shade = textureSample(ShadowCubeLight, ShadowCubeLight_smp, dir).xyz;
    let sun = textureSample(SunCubeLight, SunCubeLight_smp, dir).xyz;
    let n_dot_h = saturate(dot(g_Half, dir));
    let s = textureSample(ShadowTexture, ShadowTexture_smp, inp.v1).x;
    let unshadowed = saturate(-s + 1.0);
    let albedo = textureSample(AlbiedoTexture, AlbiedoTexture_smp, inp.v1).xyz;
    let lit = terrain_cache_light(sun, shade, s, albedo);
    let page = terrain_page_gamma(lit);
    let corrected = textureSample(ColorCorrectTexture, ColorCorrectTexture_smp, page).xyz;

    let spec = textureSample(SpecTexture, SpecTexture_smp, inp.v1);
    let exponent = spec.w * SPEC_EXPONENT_SCALE + 1.0;
    let power = d3d_pow(n_dot_h, exponent);
    var k = exponent * power;
    k = unshadowed * k;
    let glint = spec * k;
    let glint_page = terrain_page_gamma(glint.xyz);
    let glint_corrected = textureSample(ColorCorrectTexture, ColorCorrectTexture_smp, glint_page).xyz;

    var o: CacheDiffuseAndSpecOut;
    o.oc0 = vec4<f32>(corrected, 1.0);
    o.oc1 = vec4<f32>(glint_corrected, glint.w);
    return o;
}

// ---- TerrainRenderVSScreenSpace: the vertex stage of the bake ----

struct ScreenSpaceIn {
    // dcl_normal v0: the normal of the ground at the vertex.
    v0: vec4<f32>,
    // dcl_texcoord v1: the place in the cell's page, 0..1.
    v1: vec4<f32>,
}

struct ScreenSpaceOut {
    // dcl_position o0, dcl_normal o1.xyz, dcl_texcoord o2, dcl_texcoord1 o3, dcl_texcoord2 o4.xyz,
    // dcl_texcoord3 o5.xy (the last three are zero).
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec4<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
}

// The place in the page as a point of the whole target (y down), the normal and the place passed on.
// re: TerrainShader::TerrainRenderVSScreenSpace (sha1 1a614daba14b): `mad o0, v1.xyxx, r0.xyzz, c0.xyzy`
// with r0 = (2, -2, 0) and c0 = (-1, 1, 0.5, 0)
fn terrain_render_vs_screen_space_body(inp: ScreenSpaceIn) -> ScreenSpaceOut {
    var o: ScreenSpaceOut;
    o.o0 = vec4<f32>(inp.v1.x * 2.0 - 1.0, inp.v1.y * -2.0 + 1.0, 0.5, 1.0);
    o.o1 = vec4<f32>(inp.v0.xyz, 0.0);
    o.o2 = vec4<f32>(inp.v1.x, inp.v1.y, 0.0, 0.0);
    o.o3 = vec4<f32>(0.0);
    o.o4 = vec4<f32>(0.0);
    o.o5 = vec4<f32>(0.0);
    return o;
}

// ---- entries: the interface of the vertex stage of the bake ----

struct CacheVaryings {
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

struct CacheVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn terrain_render_vs_screen_space(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> CacheVertexOut {
    var b: ScreenSpaceIn;
    b.v0 = v0;
    b.v1 = v1;
    let r = terrain_render_vs_screen_space_body(b);
    var out: CacheVertexOut;
    out.clip = r.o0;
    out.normal = r.o1.xyz;
    out.uv = r.o2.xy;
    return out;
}

@fragment
fn terrain_cache_diffuse_ps(in: CacheVaryings) -> @location(0) vec4<f32> {
    var b: CacheDiffuseIn;
    b.v0 = in.normal;
    b.v1 = in.uv;
    return terrain_cache_diffuse_ps_body(b).oc0;
}

struct CacheTargets {
    @location(0) page: vec4<f32>,
    @location(1) glint: vec4<f32>,
}

@fragment
fn terrain_cache_diffuse_and_spec_ps(in: CacheVaryings) -> CacheTargets {
    var b: CacheDiffuseIn;
    b.v0 = in.normal;
    b.v1 = in.uv;
    let r = terrain_cache_diffuse_and_spec_ps_body(b);
    var o: CacheTargets;
    o.page = r.oc0;
    o.glint = r.oc1;
    return o;
}
