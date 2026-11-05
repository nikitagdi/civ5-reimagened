// The classic fog of war of the original (`FOW` package, `FOWClassic`), `FOWLevel` 0: the vertex
// shaders `VSFog`, `VSFogTrans`, `RenderSceneVS` and `RenderSceneTexOnlyVS` and the pixel shaders
// `PSFog`, `PSFogTrans` and `TexturePS`. Hand ports in the convention of shader-re 4.8.1
// (world-overlays WOV1).
//
// `VSFog` and `VSFogTrans` carry the colour of the vertex over (the product draws a hex per cell
// with it: opaque for the unrevealed, half transparent for the hidden); the `RenderScene*` pair and
// `TexturePS` draw the textured parts of the original's fog meshes through the fog mask: the
// puffs of the fluid fog (`world/fow/puffs.rs`) use `RenderSceneTexOnlyVS` and `TexturePS`.
//
// Group 0: the constants (binding 0 `Transform`, binding 1 `g_WVP` and `g_OverlayProjection`);
// group 1: the textures of `TexturePS`.

struct FogConsts {
    // row_major float4x4: register k is row k.
    Transform: mat4x4<f32>,
}

struct SceneConsts {
    g_WVP: mat4x4<f32>,
    g_OverlayProjection: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> fog_consts: FogConsts;
@group(0) @binding(1) var<uniform> scene_consts: SceneConsts;

// The two textures of `TexturePS`. The CTAB names (`FOWMask+FOWMask_texture`) are not WGSL names;
// the variables are named as the oracle names them.
@group(1) @binding(0) var tex0_PartTexture_PartTexture_texture: texture_2d<f32>;
@group(1) @binding(1) var tex0_PartTexture_PartTexture_texture_smp: sampler;
@group(1) @binding(2) var tex1_FOWMask_FOWMask_texture: texture_2d<f32>;
@group(1) @binding(3) var tex1_FOWMask_FOWMask_texture_smp: sampler;

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// ---- VSFog and VSFogTrans ----

struct FogVsIn {
    // dcl_position v0.
    v0: vec4<f32>,
    // dcl_color v1.
    v1: vec4<f32>,
}

struct FogVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
}

// re: FOW::VSFog (sha1 6b6746c33bc2)
fn vsfog_body(inp: FogVsIn) -> FogVsOut {
    let t = fog_consts.Transform;
    var r = inp.v0.y * row_major_reg(t, 1u);
    r = inp.v0.x * row_major_reg(t, 0u) + r;
    r = inp.v0.z * row_major_reg(t, 2u) + r;
    var o: FogVsOut;
    o.o0 = inp.v0.w * row_major_reg(t, 3u) + r;
    o.o1 = inp.v1;
    return o;
}

// re: FOW::VSFogTrans (sha1 e7e765d7ac4d): the same, with the height scaled by 0.66 (`def c4,
// 0.66, 0, 0, 0`), so that the translucent hexes sit lower than the opaque ones.
fn vsfogtrans_body(inp: FogVsIn) -> FogVsOut {
    let t = fog_consts.Transform;
    var r = inp.v0.y * row_major_reg(t, 1u);
    r = inp.v0.x * row_major_reg(t, 0u) + r;
    let z = 0.660000026 * inp.v0.z;
    r = z * row_major_reg(t, 2u) + r;
    var o: FogVsOut;
    o.o0 = inp.v0.w * row_major_reg(t, 3u) + r;
    o.o1 = inp.v1;
    return o;
}

struct FogVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vsfog(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> FogVertexOut {
    var inp: FogVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = vsfog_body(inp);
    var out: FogVertexOut;
    out.clip = r.o0;
    out.color = r.o1;
    return out;
}

@vertex
fn vsfogtrans(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> FogVertexOut {
    var inp: FogVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = vsfogtrans_body(inp);
    var out: FogVertexOut;
    out.clip = r.o0;
    out.color = r.o1;
    return out;
}

// ---- PSFog and PSFogTrans ----

struct FogPsIn {
    // dcl_color v0.xyz.
    v0: vec3<f32>,
}

struct FogPsOut {
    oc0: vec4<f32>,
}

// re: FOW::PSFog (sha1 9502fba404b8): the colour of the vertex, opaque.
fn psfog_body(inp: FogPsIn) -> FogPsOut {
    var o: FogPsOut;
    o.oc0 = vec4<f32>(inp.v0, 1.0);
    return o;
}

// re: FOW::PSFogTrans (sha1 7a85d061c75b): the colour of the vertex at half opacity.
fn psfogtrans_body(inp: FogPsIn) -> FogPsOut {
    var o: FogPsOut;
    o.oc0 = vec4<f32>(inp.v0, 0.5);
    return o;
}

@fragment
fn psfog(in: FogVertexOut) -> @location(0) vec4<f32> {
    var inp: FogPsIn;
    inp.v0 = in.color.xyz;
    return psfog_body(inp).oc0;
}

@fragment
fn psfogtrans(in: FogVertexOut) -> @location(0) vec4<f32> {
    var inp: FogPsIn;
    inp.v0 = in.color.xyz;
    return psfogtrans_body(inp).oc0;
}

// ---- RenderSceneVS and RenderSceneTexOnlyVS ----

struct SceneVsIn {
    // dcl_position v0 (w is 1).
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec4<f32>,
}

struct SceneVsOut {
    o0: vec4<f32>,
    // The texture's coordinates.
    o1: vec2<f32>,
    o2: vec3<f32>,
    // The mask's coordinates.
    o3: vec2<f32>,
}

struct SceneTexOnlyVsOut {
    o0: vec4<f32>,
    o1: vec2<f32>,
    o2: vec2<f32>,
}

// The point through `g_WVP`, the fourth component of the corner is 1.
fn scene_position(v0: vec4<f32>) -> vec4<f32> {
    let m = scene_consts.g_WVP;
    var r = v0.y * row_major_reg(m, 1u);
    r = v0.x * row_major_reg(m, 0u) + r;
    r = v0.z * row_major_reg(m, 2u) + r;
    return r + row_major_reg(m, 3u);
}

// The point in the mask: through `g_OverlayProjection`, to texture space.
fn scene_mask_uv(v0: vec4<f32>) -> vec2<f32> {
    let m = scene_consts.g_OverlayProjection;
    var p = v0.y * row_major_reg(m, 1u).xy;
    p = v0.x * row_major_reg(m, 0u).xy + p;
    p = v0.z * row_major_reg(m, 2u).xy + p;
    p = p + row_major_reg(m, 3u).xy;
    return p * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
}

// re: FOW::RenderSceneVS (sha1 5feb3e3a1f60)
fn renderscenevs_body(inp: SceneVsIn) -> SceneVsOut {
    var o: SceneVsOut;
    o.o0 = scene_position(inp.v0);
    o.o1 = inp.v1.xy;
    o.o2 = vec3<f32>(0.0);
    o.o3 = scene_mask_uv(inp.v0);
    return o;
}

// re: FOW::RenderSceneTexOnlyVS (sha1 26f222494d7a): the clip position is (x, y, w / 1024, w)
// (`def c8, 0.5, -0.5, 0.0009765625, 0`), the mask's coordinates as above.
fn renderscenetexonlyvs_body(inp: SceneVsIn) -> SceneTexOnlyVsOut {
    let p = scene_position(inp.v0);
    var o: SceneTexOnlyVsOut;
    o.o0 = vec4<f32>(p.x, p.y, p.w * 0.0009765625, p.w);
    o.o1 = inp.v1.xy;
    o.o2 = scene_mask_uv(inp.v0);
    return o;
}

struct SceneVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) mask_uv: vec2<f32>,
}

@vertex
fn renderscenevs(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> SceneVertexOut {
    var inp: SceneVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = renderscenevs_body(inp);
    var out: SceneVertexOut;
    out.clip = r.o0;
    out.uv = r.o1;
    out.mask_uv = r.o3;
    return out;
}

@vertex
fn renderscenetexonlyvs(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> SceneVertexOut {
    var inp: SceneVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = renderscenetexonlyvs_body(inp);
    var out: SceneVertexOut;
    out.clip = r.o0;
    out.uv = r.o1;
    out.mask_uv = r.o2;
    return out;
}

// ---- TexturePS ----

struct TexturePsIn {
    // dcl_texcoord v0.xy: the part's texture; v1.xy: the mask.
    v0: vec2<f32>,
    v1: vec2<f32>,
}

struct TexturePsOut {
    oc0: vec4<f32>,
    kill: bool,
}

// re: FOW::TexturePS (sha1 55f399129a11): the part's texture through the fog mask. A pixel is
// dropped where the mask's green channel is under 0.01, or where its product with the texture's
// alpha is; the alpha written is that product.
fn textureps_body(inp: TexturePsIn) -> TexturePsOut {
    let m = textureSample(
        tex1_FOWMask_FOWMask_texture,
        tex1_FOWMask_FOWMask_texture_smp,
        inp.v1,
    );
    let t = textureSample(
        tex0_PartTexture_PartTexture_texture,
        tex0_PartTexture_PartTexture_texture_smp,
        inp.v0,
    );
    let covered = m.y * t.w;
    var o: TexturePsOut;
    o.kill = !((m.y + -0.01) >= 0.0) || !((covered + -0.01) >= 0.0);
    o.oc0 = vec4<f32>(t.xyz, covered);
    return o;
}

@fragment
fn textureps(in: SceneVertexOut) -> @location(0) vec4<f32> {
    var inp: TexturePsIn;
    inp.v0 = in.uv;
    inp.v1 = in.mask_uv;
    let r = textureps_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}
