// The minimap shaders of the original (`MiniMapShader` package, `cvMinimapRenderer`, world-overlays
// WOV3): the vertex shaders of the hex meshes, the ocean quad, the ice, the sprites and the camera
// rectangle, and the pixel shaders of the passes land, ocean, shallows, river, ice, culture, fog,
// dot, textured sprite, camera rectangle and debug copy. Hand ports in the convention of shader-re
// 4.8.1: each entry of the original is a body `fn <entry>_body(inp)` with the registers as member
// names; the `@vertex` / `@fragment` entries only read their inputs, call the body and hand the
// result on.
//
// The minimap draws the world through a camera that is an offset and a scale (`SetupCamera`: the
// centre of the view and 2 / its size), not a matrix. The textures are in gamma 2.2: the pixel
// shaders turn the texture into linear light for the arithmetic they do and back (`log`, `* 1 / 2.2`,
// `exp`).
//
// Group 0, binding 0: the constants; group 1: the textures and their samplers (the slots are those
// of the original: land 1, water 2, fog 3, culture 4, shallows 5, ice 6, sprite 7).

// The names are those of the CTAB constants of the package.
struct MinimapConsts {
    g_vOffset: vec2<f32>,
    g_vScale: vec2<f32>,
    g_vWorldVisibleMin: vec2<f32>,
    g_vWorldVisibleMax: vec2<f32>,
    g_vIceOffset: vec2<f32>,
    g_vIceBBoxMin: vec2<f32>,
    g_vIceBBoxMax: vec2<f32>,
    g_vLandTextureTiling: vec2<f32>,
    g_vWaterTextureTiling: vec2<f32>,
    g_vIceTextureTiling: vec2<f32>,
    g_vCultureTextureTiling: vec2<f32>,
    g_vCityFade: vec2<f32>,
    g_vColor: vec3<f32>,
    g_fFogAlpha: f32,
}

@group(0) @binding(0) var<uniform> minimap_consts: MinimapConsts;

@group(1) @binding(0) var LandTexture: texture_2d<f32>;
@group(1) @binding(1) var LandTexture_smp: sampler;
@group(1) @binding(2) var WaterTexture: texture_2d<f32>;
@group(1) @binding(3) var WaterTexture_smp: sampler;
@group(1) @binding(4) var FogTexture: texture_2d<f32>;
@group(1) @binding(5) var FogTexture_smp: sampler;
@group(1) @binding(6) var CultureTexture: texture_2d<f32>;
@group(1) @binding(7) var CultureTexture_smp: sampler;
@group(1) @binding(8) var ShallowsTexture: texture_2d<f32>;
@group(1) @binding(9) var ShallowsTexture_smp: sampler;
@group(1) @binding(10) var IceTexture: texture_2d<f32>;
@group(1) @binding(11) var IceTexture_smp: sampler;
@group(1) @binding(12) var SpriteTexture: texture_2d<f32>;
@group(1) @binding(13) var SpriteTexture_smp: sampler;
@group(1) @binding(14) var sDebug: texture_2d<f32>;
@group(1) @binding(15) var sDebug_smp: sampler;

// `1 / 2.2` and the `log`, `mul`, `exp` pair that takes a colour out of its gamma and back.
const GAMMA_BACK: f32 = 0.454545468;
const GAMMA: f32 = 2.20000005;

fn to_gamma(x: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        exp2(d3d_log(x.x) * GAMMA_BACK),
        exp2(d3d_log(x.y) * GAMMA_BACK),
        exp2(d3d_log(x.z) * GAMMA_BACK),
    );
}

// ---- the vertex shaders ----

// The vertex shaders write the position in the camera's units (the world minus the centre, times
// 2 / size), the texture's coordinates (world units, for the tiling) and the same position as
// coordinates in the target (`o1`, 0 to 1 from the top left).

struct MeshVsIn {
    // dcl_position v0: the corner in world units.
    v0: vec4<f32>,
}

struct MeshVsOut {
    // dcl_texcoord o0.xy: the world position.
    o0: vec2<f32>,
    // dcl_texcoord1 o1.xy: the place in the target.
    o1: vec2<f32>,
    // dcl_position o2.
    o2: vec4<f32>,
}

// re: MinimapShader::VS_MiniMap (sha1 5ab35345d840)
fn vs_minimap_body(inp: MeshVsIn) -> MeshVsOut {
    let c = minimap_consts;
    let r0 = inp.v0.xy * c.g_vScale - c.g_vOffset;
    var o: MeshVsOut;
    o.o1 = r0 * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    o.o2 = vec4<f32>(r0, 0.0, 1.0);
    o.o0 = inp.v0.xy;
    return o;
}

struct OceanVsIn {
    // dcl_texcoord v0: the corner of the quad, 0 to 1.
    v0: vec4<f32>,
}

// re: MinimapShader::VS_MiniMap_Ocean (sha1 7d947db45f69): the quad runs over the visible world.
fn vs_minimap_ocean_body(inp: OceanVsIn) -> MeshVsOut {
    let c = minimap_consts;
    let world = inp.v0.xy * (c.g_vWorldVisibleMax - c.g_vWorldVisibleMin) + c.g_vWorldVisibleMin;
    let r = world * c.g_vScale - c.g_vOffset;
    var o: MeshVsOut;
    o.o0 = world;
    o.o1 = r * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    o.o2 = vec4<f32>(r, 0.0, 1.0);
    return o;
}

// re: MinimapShader::VS_MinimapIce (sha1 7c4bdb5f4daa): the quad runs over the bounding box of the
// ice surface, moved by the offset of this copy.
fn vs_minimap_ice_body(inp: OceanVsIn) -> MeshVsOut {
    let c = minimap_consts;
    let size = c.g_vIceBBoxMax - c.g_vIceBBoxMin;
    let t = inp.v0.xy * 0.5 + vec2<f32>(0.5);
    let world = t * size + c.g_vIceBBoxMin + c.g_vIceOffset;
    let r = world * c.g_vScale - c.g_vOffset;
    var o: MeshVsOut;
    o.o0 = world;
    o.o1 = r * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    o.o2 = vec4<f32>(r, 0.0, 1.0);
    return o;
}

struct SpriteVsIn {
    // dcl_position v0: the corner in world units.
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec4<f32>,
    // dcl_color v2: the colour in gamma.
    v2: vec4<f32>,
}

struct SpriteVsOut {
    o0: vec2<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
}

// re: MinimapShader::VS_Sprite (sha1 297c4c83b0b9): the colour goes to linear light (`^ 2.2`) here,
// the pixel shaders take it back.
fn vs_sprite_body(inp: SpriteVsIn) -> SpriteVsOut {
    let c = minimap_consts;
    var o: SpriteVsOut;
    o.o1 = vec4<f32>(
        exp2(d3d_log(inp.v2.x) * GAMMA),
        exp2(d3d_log(inp.v2.y) * GAMMA),
        exp2(d3d_log(inp.v2.z) * GAMMA),
        inp.v2.w,
    );
    o.o2 = vec4<f32>(inp.v0.xy * c.g_vScale - c.g_vOffset, 0.0, 1.0);
    o.o0 = inp.v1.xy;
    return o;
}

struct RectVsIn {
    // dcl_position v0, dcl_color v1.
    v0: vec4<f32>,
    v1: vec4<f32>,
}

struct RectVsOut {
    // dcl_texcoord o0.x: the weight of the corner (the pixel shader takes its root).
    o0: f32,
    o1: vec4<f32>,
}

// re: MinimapShader::VS_CameraRect (sha1 af0956fb930e)
fn vs_camerarect_body(inp: RectVsIn) -> RectVsOut {
    let c = minimap_consts;
    var o: RectVsOut;
    o.o1 = vec4<f32>(inp.v0.xy * c.g_vScale - c.g_vOffset, 0.0, 1.0);
    o.o0 = inp.v1.x;
    return o;
}

struct DebugVsIn {
    v0: vec4<f32>,
    v1: vec4<f32>,
}

struct DebugVsOut {
    o0: vec2<f32>,
    o1: vec4<f32>,
}

// re: MinimapShader::VS_DebugCopy (sha1 cb7757777d2d)
fn vs_debugcopy_body(inp: DebugVsIn) -> DebugVsOut {
    var o: DebugVsOut;
    o.o0 = inp.v1.xy;
    o.o1 = inp.v0;
    return o;
}

// ---- the pixel shaders ----

struct UvIn {
    // dcl_texcoord v0.xy.
    v0: vec2<f32>,
}

struct ColorOut {
    oc0: vec4<f32>,
}

// The coordinates of a tiled layer: `uv * tiling.x + tiling.y`.
fn tiled(uv: vec2<f32>, tiling: vec2<f32>) -> vec2<f32> {
    return uv * tiling.x + tiling.y;
}

// re: MinimapShader::PS_Land (sha1 d825f819faab): the alpha of the texture stays.
fn ps_land_body(inp: UvIn) -> ColorOut {
    let t = textureSample(
        LandTexture,
        LandTexture_smp,
        tiled(inp.v0, minimap_consts.g_vLandTextureTiling),
    );
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz), t.w);
    return o;
}

// re: MinimapShader::PS_Ocean (sha1 1a692b822f64): opaque.
fn ps_ocean_body(inp: UvIn) -> ColorOut {
    let t = textureSample(
        WaterTexture,
        WaterTexture_smp,
        tiled(inp.v0, minimap_consts.g_vWaterTextureTiling),
    );
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz), 1.0);
    return o;
}

// re: MinimapShader::PS_Shallows (sha1 3ce6f50e745b): the alpha of the texture stays.
fn ps_shallows_body(inp: UvIn) -> ColorOut {
    let t = textureSample(
        ShallowsTexture,
        ShallowsTexture_smp,
        tiled(inp.v0, minimap_consts.g_vWaterTextureTiling),
    );
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz), t.w);
    return o;
}

// re: MinimapShader::PS_River (sha1 3ce6f50e745b): the block of `PS_Shallows`, the same texture.
fn ps_river_body(inp: UvIn) -> ColorOut {
    let t = textureSample(
        ShallowsTexture,
        ShallowsTexture_smp,
        tiled(inp.v0, minimap_consts.g_vWaterTextureTiling),
    );
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz), t.w);
    return o;
}

// re: MinimapShader::PS_MinimapIce (sha1 c37494803c08): opaque.
fn ps_minimapice_body(inp: UvIn) -> ColorOut {
    let t = textureSample(
        IceTexture,
        IceTexture_smp,
        tiled(inp.v0, minimap_consts.g_vIceTextureTiling),
    );
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz), 1.0);
    return o;
}

// re: MinimapShader::PS_Culture (sha1 d4f816e6f6cd): the texture tinted by the owner's colour (given
// in linear light), opaque.
fn ps_culture_body(inp: UvIn) -> ColorOut {
    let c = minimap_consts;
    let t = textureSample(
        CultureTexture,
        CultureTexture_smp,
        tiled(inp.v0, c.g_vCultureTextureTiling),
    );
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz * c.g_vColor), 1.0);
    return o;
}

// re: MinimapShader::PS_Fog (sha1 a6928b0d83b1): the place in the target (`texcoord1`), no tiling;
// the alpha is the fog's.
fn ps_fog_body(inp: UvIn) -> ColorOut {
    let t = textureSample(FogTexture, FogTexture_smp, inp.v0);
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz), minimap_consts.g_fFogAlpha);
    return o;
}

struct DotIn {
    // dcl_texcoord v0.xy: 0 to 1 over the quad; dcl_color v1.
    v0: vec2<f32>,
    v1: vec4<f32>,
}

// re: MinimapShader::PS_Dot (sha1 fe62fa2c22eb): a round dot, solid in the middle and falling off
// from `g_vCityFade.x` to `g_vCityFade.y` of the distance to the centre (twice it), a smoothstep.
fn ps_dot_body(inp: DotIn) -> ColorOut {
    let fade = minimap_consts.g_vCityFade;
    let d = inp.v0 - vec2<f32>(0.5);
    let dist = d3d_rcp(d3d_rsq(dot(d, d)));
    let x = clamp(d3d_rcp(fade.y - fade.x) * (dist * 2.0 - fade.x), 0.0, 1.0);
    let s = (3.0 - 2.0 * x) * (x * x);
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(inp.v1.xyz), (1.0 - s) * inp.v1.w);
    return o;
}

struct SpritePsIn {
    // dcl_texcoord v0.xy, dcl_color v1.
    v0: vec2<f32>,
    v1: vec4<f32>,
}

// re: MinimapShader::PS_TexturedSprite (sha1 1f7b7614fda5)
fn ps_texturedsprite_body(inp: SpritePsIn) -> ColorOut {
    let t = textureSample(SpriteTexture, SpriteTexture_smp, inp.v0);
    var o: ColorOut;
    o.oc0 = vec4<f32>(to_gamma(t.xyz * inp.v1.xyz), t.w * inp.v1.w);
    return o;
}

struct WeightIn {
    // dcl_texcoord v0.x.
    v0: f32,
}

// re: MinimapShader::PS_CameraRect (sha1 060e65dd7ae1): the weight is the colour, its root the alpha
// (the rectangle's edge softens outwards).
fn ps_camerarect_body(inp: WeightIn) -> ColorOut {
    var o: ColorOut;
    o.oc0 = vec4<f32>(vec3<f32>(inp.v0), d3d_rcp(d3d_rsq(inp.v0)));
    return o;
}

// re: MinimapShader::PS_DebugCopy (sha1 fec77ddfb281): the whole texel through the gamma pair.
fn ps_debugcopy_body(inp: UvIn) -> ColorOut {
    let t = textureSample(sDebug, sDebug_smp, inp.v0);
    var o: ColorOut;
    o.oc0 = vec4<f32>(
        exp2(d3d_log(t.x) * GAMMA_BACK),
        exp2(d3d_log(t.y) * GAMMA_BACK),
        exp2(d3d_log(t.z) * GAMMA_BACK),
        exp2(d3d_log(t.w) * GAMMA_BACK),
    );
    return o;
}

// ---- entries ----

struct MeshVertexOut {
    @builtin(position) clip: vec4<f32>,
    // The world position (`o0`) and the place in the target (`o1`).
    @location(0) world: vec2<f32>,
    @location(1) target_uv: vec2<f32>,
}

fn mesh_out(r: MeshVsOut) -> MeshVertexOut {
    var out: MeshVertexOut;
    out.clip = r.o2;
    out.world = r.o0;
    out.target_uv = r.o1;
    return out;
}

@vertex
fn vs_minimap(@location(0) v0: vec4<f32>) -> MeshVertexOut {
    var inp: MeshVsIn;
    inp.v0 = v0;
    return mesh_out(vs_minimap_body(inp));
}

@vertex
fn vs_minimap_ocean(@location(0) v0: vec4<f32>) -> MeshVertexOut {
    var inp: OceanVsIn;
    inp.v0 = v0;
    return mesh_out(vs_minimap_ocean_body(inp));
}

@vertex
fn vs_minimap_ice(@location(0) v0: vec4<f32>) -> MeshVertexOut {
    var inp: OceanVsIn;
    inp.v0 = v0;
    return mesh_out(vs_minimap_ice_body(inp));
}

struct SpriteVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_sprite(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec4<f32>,
) -> SpriteVertexOut {
    var inp: SpriteVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    inp.v2 = v2;
    let r = vs_sprite_body(inp);
    var out: SpriteVertexOut;
    out.clip = r.o2;
    out.uv = r.o0;
    out.color = r.o1;
    return out;
}

struct RectVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) weight: f32,
}

@vertex
fn vs_camerarect(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> RectVertexOut {
    var inp: RectVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = vs_camerarect_body(inp);
    var out: RectVertexOut;
    out.clip = r.o1;
    out.weight = r.o0;
    return out;
}

struct DebugVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_debugcopy(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> DebugVertexOut {
    var inp: DebugVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = vs_debugcopy_body(inp);
    var out: DebugVertexOut;
    out.clip = r.o1;
    out.uv = r.o0;
    return out;
}

@fragment
fn ps_land(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_land_body(UvIn(in.world)).oc0;
}

@fragment
fn ps_ocean(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_ocean_body(UvIn(in.world)).oc0;
}

@fragment
fn ps_shallows(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_shallows_body(UvIn(in.world)).oc0;
}

@fragment
fn ps_river(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_river_body(UvIn(in.world)).oc0;
}

@fragment
fn ps_minimapice(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_minimapice_body(UvIn(in.world)).oc0;
}

@fragment
fn ps_culture(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_culture_body(UvIn(in.world)).oc0;
}

@fragment
fn ps_fog(in: MeshVertexOut) -> @location(0) vec4<f32> {
    return ps_fog_body(UvIn(in.target_uv)).oc0;
}

@fragment
fn ps_dot(in: SpriteVertexOut) -> @location(0) vec4<f32> {
    return ps_dot_body(DotIn(in.uv, in.color)).oc0;
}

@fragment
fn ps_texturedsprite(in: SpriteVertexOut) -> @location(0) vec4<f32> {
    return ps_texturedsprite_body(SpritePsIn(in.uv, in.color)).oc0;
}

@fragment
fn ps_camerarect(in: RectVertexOut) -> @location(0) vec4<f32> {
    return ps_camerarect_body(WeightIn(in.weight)).oc0;
}

@fragment
fn ps_debugcopy(in: DebugVertexOut) -> @location(0) vec4<f32> {
    return ps_debugcopy_body(UvIn(in.uv)).oc0;
}
