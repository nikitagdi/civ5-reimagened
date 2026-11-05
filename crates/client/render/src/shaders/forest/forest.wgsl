// ForestShader of the original: `ForestVS` and the three pixel shaders `ForestSSOverlayPS`,
// `ForestTopDownOverlayPS` and `ForestSSReflectPS` (world-objects WOB1). Hand port in the convention
// of shader-re 4.8.1: each entry of the original is a body `fn <entry>_body(inp: In) -> Out` with
// the registers of the original as member names, and a `@vertex` / `@fragment` entry that only reads
// its inputs and calls the body.
//
// The catalog picks the pixel shader (render-core 4.15): `ForestSSOverlayPS` in the main pass, where
// the trees are overlaid with the screen-space picture of the ground (borders, grid); the top-down
// variant overlays with the picture projected from above; `ForestSSReflectPS` is the reflection
// pass. The inputs of the main variants (`FOWMask`, `FOWBaseLayer`, `SSOverlaySampler`) are made by
// `world-overlays` and `world-terrain`; until those subsystems give them, `Forest` is drawn by B1's
// static pipeline (`B1Forest`).
//
// Needs frame.wgsl (`frame_globals`, group 0 binding 1) and sm3.wgsl. Resources carry the names of
// the CTAB; a sampler `S` is the texture `S` and the sampler `S_smp`.
//
// The light of the trees is the colour-correction LUT: the base colour goes through a YUV-like
// conversion, the Colorize map tints its chroma, and the 3D `ColorCorrection` table does the rest.

// ---- constants of the vertex stage, group 0 ----

// `treeLocation[90]`: xyz the position of a tree, w its scale; the vertex says which one in
// `BLENDINDICES`. `mtxViewProj` is `FrameGlobals.ViewProjMatrix` (the family copies it).
struct ForestVsConsts {
    mtxViewProj: mat4x4<f32>,
    OverlayProjection: mat4x4<f32>,
    g_FOWBaseScale: vec4<f32>,
    treeLocation: array<vec4<f32>, 90>,
}
@group(0) @binding(11) var<uniform> forest_vs_consts: ForestVsConsts;

// ---- constants and resources of the pixel stage ----

struct ForestPsConsts {
    // The size of the render target, for `vPos`.
    ScreenRes: vec2<f32>,
    fOverlayMultiplier: f32,
    // The eye, in the original's basis (`FrameGlobals.f3CameraWorld`).
    g_CamPos: vec3<f32>,
}
@group(0) @binding(12) var<uniform> forest_ps_consts: ForestPsConsts;

@group(0) @binding(2) var FogRamp: texture_2d<f32>;
@group(0) @binding(3) var FogRamp_smp: sampler;
@group(0) @binding(4) var ColorCorrection: texture_3d<f32>;
@group(0) @binding(5) var ColorCorrection_smp: sampler;
@group(0) @binding(6) var TransFOWColor: texture_3d<f32>;
@group(0) @binding(7) var TransFOWColor_smp: sampler;
@group(0) @binding(8) var FOWMask: texture_2d<f32>;
@group(0) @binding(9) var FOWMask_smp: sampler;
@group(0) @binding(13) var FOWBaseLayer: texture_2d<f32>;
@group(0) @binding(14) var FOWBaseLayer_smp: sampler;
@group(0) @binding(15) var SSOverlaySampler: texture_2d<f32>;
@group(0) @binding(16) var SSOverlaySampler_smp: sampler;
@group(0) @binding(17) var OverlaySampler: texture_2d<f32>;
@group(0) @binding(18) var OverlaySampler_smp: sampler;

// ---- material, group 3 ----

@group(3) @binding(0) var BaseSampler: texture_2d<f32>;
@group(3) @binding(1) var BaseSampler_smp: sampler;
@group(3) @binding(2) var ColorizeSampler: texture_2d<f32>;
@group(3) @binding(3) var ColorizeSampler_smp: sampler;

// ---- ForestVS ----

struct ForestVsIn {
    // dcl_position v0: the vertex of the tree model.
    v0: vec3<f32>,
    // dcl_texcoord v1: (u, v).
    v1: vec2<f32>,
    // dcl_blendindices v2: which of the 90 trees of the draw this vertex belongs to.
    v2: vec4<f32>,
}

struct ForestVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec3<f32>,
    o3: vec2<f32>,
    o4: vec2<f32>,
}

// The register `k` of a `row_major float4x4` that was filled by CTAB name (reg k = row k): the
// matrix member `[j][k]` is component j of register k.
fn forest_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// What `ForestVS` does with the location of its tree, which the instance gives at run time
// (WO-D4): the vertex placed at `loc` (xyz, scale w), the clip position and the outputs. The body
// below reads `loc` from `treeLocation` as the original does.
fn forest_vs_core(v0: vec3<f32>, v1: vec2<f32>, loc: vec4<f32>) -> ForestVsOut {
    let c0 = forest_reg(forest_vs_consts.mtxViewProj, 0u);
    let c1 = forest_reg(forest_vs_consts.mtxViewProj, 1u);
    let c2 = forest_reg(forest_vs_consts.mtxViewProj, 2u);
    let c3 = forest_reg(forest_vs_consts.mtxViewProj, 3u);
    let c4 = forest_reg(forest_vs_consts.OverlayProjection, 0u);
    let c5 = forest_reg(forest_vs_consts.OverlayProjection, 1u);
    let c6 = forest_reg(forest_vs_consts.OverlayProjection, 2u);
    let c7 = forest_reg(forest_vs_consts.OverlayProjection, 3u);
    let c8 = forest_vs_consts.g_FOWBaseScale;
    // Constant of the shader: `def c99, 0.5, -0.5, 0, 0`.
    let c99 = vec4<f32>(0.5, -0.5, 0.0, 0.0);
    var o: ForestVsOut;
    // The point in the world.
    let p = v0 * loc.w + loc.xyz;
    o.o2 = loc.xyz;
    var r1 = p.y * c1;
    r1 = p.x * c0 + r1;
    r1 = p.z * c2 + r1;
    r1 = r1 + c3;
    let inv_w = d3d_rcp(r1.w);
    o.o4 = inv_w * r1.xy;
    o.o0 = r1;
    // The fog of war of the original: the base layer and the mask.
    let base_uv = (p.xy + c8.zw) * c8.xy;
    o.o1 = vec4<f32>(v1, base_uv);
    var m = vec2<f32>(p.y * c5.x, p.y * c5.y);
    m = p.x * c4.xy + m;
    m = p.z * c6.xy + m;
    m = m + c7.xy;
    o.o3 = m * c99.xy + c99.y;
    return o;
}

// The constant register `i` of the vertex shader, for the relative read `c9[a0.x]`: the registers
// are the rows of `mtxViewProj` (c0..c3) and `OverlayProjection` (c4..c7), `g_FOWBaseScale` (c8) and
// `treeLocation` (c9..c98), then the constant `def c99`. A read outside is clamped into the file, as
// the oracle of shader-re does it (D3D9 leaves such a read undefined; the vertices of the game
// never make one).
fn forest_creg(i: i32) -> vec4<f32> {
    let k = clamp(i, 0, 98);
    var v: vec4<f32>;
    if (k < 4) {
        v = forest_reg(forest_vs_consts.mtxViewProj, u32(k));
    } else if (k < 8) {
        v = forest_reg(forest_vs_consts.OverlayProjection, u32(k - 4));
    } else if (k == 8) {
        v = forest_vs_consts.g_FOWBaseScale;
    } else {
        v = forest_vs_consts.treeLocation[k - 9];
    }
    if (i == 99) {
        v = vec4<f32>(0.5, -0.5, 0.0, 0.0);
    }
    return v;
}

// re: ForestShader::ForestVS (sha1 6bf5913abad8)
fn forest_vs_body(inp: ForestVsIn) -> ForestVsOut {
    // mova a0.x, v2.x: round to nearest; c9[a0.x] reads the register 9 + a0.x.
    let a0 = i32(floor(inp.v2.x + 0.5));
    return forest_vs_core(inp.v0, inp.v1, forest_creg(9 + a0));
}

// ---- the pixel shaders: shared pieces ----

struct ForestPsOut {
    oc0: vec4<f32>,
    // `texkill`: the texel is not drawn.
    kill: bool,
}

// The weights of the YUV-like conversion the shaders define as constants.
const FOREST_C_U = vec3<f32>(-0.147129998, -0.288859993, 0.43599999);
const FOREST_C_V = vec3<f32>(0.61500001, -0.514989972, -0.10001);

// Base colour and colorize map to the lookup coordinates of `ColorCorrection`: the chroma of the
// base is mixed to that of the colorize map by the alpha of the latter, and converted back.
fn forest_lut_coord(base: vec4<f32>, colorize: vec4<f32>) -> vec3<f32> {
    let c8 = vec4<f32>(1.0, 1.13982999, 0.0, 2.03210998);
    let c9 = vec3<f32>(1.0, -0.394650012, -0.580600023);
    let r4y = dot(FOREST_C_U, colorize.xyz);
    let r4z = dot(FOREST_C_V, colorize.xyz);
    let r2y = dot(FOREST_C_U, base.xyz);
    let r2z = dot(FOREST_C_V, base.xyz);
    // lrp r0.yz, r2.w, r4, r2
    let y = colorize.w * (r4y - r2y) + r2y;
    let z = colorize.w * (r4z - r2z) + r2z;
    let r0 = vec3<f32>(base.x, y, z);
    // dp2add r2.x, c8, r0.xzzw, c8.z ; dp3 r2.y, c9, r0 ; dp2add r2.z, c8.xwzw, r0, c8.z
    return vec3<f32>(
        c8.x * r0.x + c8.y * r0.z + c8.z,
        dot(c9, r0),
        c8.x * r0.x + c8.w * r0.y + c8.z,
    );
}

// The distance fog of the trees: exponential in height and distance, the ramp `FogRamp`.
// `eye_to_point_z` is the height of the point minus the eye's, `d` the vector from the eye.
fn forest_fog(p: vec3<f32>, clip_y: f32) -> f32 {
    let log2e = 1.44269502;
    let g_FogConsts = frame_globals.g_FogConsts;
    let d = p - forest_ps_consts.g_CamPos;
    let d2 = dot(d, d);
    var fx = d2 * g_FogConsts.y;
    var fy = d.z * g_FogConsts.x;
    fy = fy * log2e;
    fy = exp2(fy);
    fy = -fy + 1.0;
    fx = fy * fx;
    fx = d3d_rcp(d.z) * fx;
    fx = fx * log2e;
    fx = saturate(exp2(fx));
    fx = -fx + 1.0;
    let ramp = textureSample(FogRamp, FogRamp_smp, vec2<f32>(clip_y * 0.5 + 0.5, g_FogConsts.w));
    return fx * ramp.x;
}

// The fog of war of the trees: the mask picks the base layer or the picture, the `TransFOWColor`
// table recolours what is only remembered, not seen (the blue channel of the mask).
fn forest_fow(picture: vec3<f32>, mask: vec4<f32>, base_layer: vec3<f32>) -> vec3<f32> {
    let mixed = mask.x * (picture - base_layer) + base_layer;
    let trans = textureSample(TransFOWColor, TransFOWColor_smp, mixed);
    let w = mask.z * (-mask.x) + mask.x;
    return w * (trans.xyz - mixed) + mixed;
}

// ---- ForestSSOverlayPS ----

struct ForestSsOverlayPsIn {
    // dcl_texcoord v0: the uv of the textures (xy), the uv of the base layer of the fog (zw).
    v0: vec4<f32>,
    // dcl_texcoord1 v1.xyz: the position of the tree in the world.
    v1: vec3<f32>,
    // dcl_texcoord2 v2.xy: the coordinates of the fog mask.
    v2: vec2<f32>,
    // dcl_texcoord3 v3.y: the y of the clip position over w.
    v3: vec2<f32>,
    // dcl vPos.xy: the pixel.
    vpos: vec2<f32>,
}

// re: ForestShader::ForestSSOverlayPS (sha1 f4440041ff99)
fn forest_ss_overlay_ps_body(inp: ForestSsOverlayPsIn) -> ForestPsOut {
    var out: ForestPsOut;
    let base = textureSample(BaseSampler, BaseSampler_smp, inp.v0.xy);
    let mask = textureSample(FOWMask, FOWMask_smp, inp.v2);
    // texkill: the alpha under 0.1, and the mask at 0.
    out.kill = (base.w + -0.100000001 < 0.0) || (-abs(mask.x) >= 0.0);
    let screen = vec2<f32>(
        d3d_rcp(forest_ps_consts.ScreenRes.x) * inp.vpos.x,
        d3d_rcp(forest_ps_consts.ScreenRes.y) * inp.vpos.y,
    );
    let colorize = textureSample(ColorizeSampler, ColorizeSampler_smp, 0.00100000005 * inp.v1.xy);
    let overlay = textureSample(SSOverlaySampler, SSOverlaySampler_smp, screen);
    let lut_at = forest_lut_coord(base, colorize);
    let lut = textureSample(ColorCorrection, ColorCorrection_smp, lut_at);
    let fOM = forest_ps_consts.fOverlayMultiplier;
    let keep = overlay.w * -fOM - -1.0;
    let picture = lut.xyz * keep + overlay.xyz * fOM;
    let base_layer = textureSample(FOWBaseLayer, FOWBaseLayer_smp, inp.v0.zw);
    let lit = forest_fow(picture, mask, base_layer.xyz);
    let fog = forest_fog(inp.v1, inp.v3.y);
    out.oc0 = vec4<f32>(fog * (frame_globals.f3FogColor.xyz - lit) + lit, base.w);
    return out;
}

// ---- ForestTopDownOverlayPS ----

struct ForestTopDownPsIn {
    v0: vec4<f32>,
    v1: vec3<f32>,
    v2: vec2<f32>,
    v3: vec2<f32>,
}

// re: ForestShader::ForestTopDownOverlayPS (sha1 c34fbed1d748)
fn forest_top_down_overlay_ps_body(inp: ForestTopDownPsIn) -> ForestPsOut {
    var out: ForestPsOut;
    let base = textureSample(BaseSampler, BaseSampler_smp, inp.v0.xy);
    let mask = textureSample(FOWMask, FOWMask_smp, inp.v2);
    out.kill = (base.w + -0.100000001 < 0.0) || (-abs(mask.x) >= 0.0);
    let colorize = textureSample(ColorizeSampler, ColorizeSampler_smp, 0.00100000005 * inp.v1.xy);
    let overlay = textureSample(OverlaySampler, OverlaySampler_smp, inp.v2);
    let lut_at = forest_lut_coord(base, colorize);
    let lut = textureSample(ColorCorrection, ColorCorrection_smp, lut_at);
    let fOM = forest_ps_consts.fOverlayMultiplier;
    let keep = overlay.w * -fOM - -1.0;
    let picture = lut.xyz * keep + overlay.xyz * fOM;
    let base_layer = textureSample(FOWBaseLayer, FOWBaseLayer_smp, inp.v0.zw);
    let lit = forest_fow(picture, mask, base_layer.xyz);
    let fog = forest_fog(inp.v1, inp.v3.y);
    out.oc0 = vec4<f32>(fog * (frame_globals.f3FogColor.xyz - lit) + lit, base.w);
    return out;
}

// ---- ForestSSReflectPS ----

struct ForestSsReflectPsIn {
    // dcl_texcoord v0.xy: the uv of the base texture.
    v0: vec2<f32>,
    // dcl_texcoord1 v1.xy: the position of the tree, for the colorize map.
    v1: vec2<f32>,
}

// re: ForestShader::ForestSSReflectPS (sha1 cabe64e8906c)
fn forest_ss_reflect_ps_body(inp: ForestSsReflectPsIn) -> ForestPsOut {
    var out: ForestPsOut;
    let base = textureSample(BaseSampler, BaseSampler_smp, inp.v0);
    out.kill = base.w + -0.100000001 < 0.0;
    let colorize = textureSample(ColorizeSampler, ColorizeSampler_smp, 0.00100000005 * inp.v1);
    let lut_at = forest_lut_coord(base, colorize);
    let lut = textureSample(ColorCorrection, ColorCorrection_smp, lut_at);
    out.oc0 = vec4<f32>(lut.xyz, base.w);
    return out;
}

// ---- entries ----

// The instance of a tree: its position and scale in the original's basis (the family fills
// them from `StaticInstance`) and the variant (mesh) of the model it shows. One draw runs every
// variant for each tree, in the back-to-front order of the trees (draw.rs).
struct ForestVertexIn {
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // The variant (mesh) of the model this vertex belongs to.
    @location(3) variant: u32,
    // The tree: xyz where, w its scale (`treeLocation` of the original).
    @location(4) tree_location: vec4<f32>,
    @location(5) tree_variant: u32,
}

// The texture coordinate of the original from the one of the blob. The pipeline stores `1 - v`
// (`UV_FLIP_V` of `asset-pipeline`), but the tree textures of `assets-b2` are the DDS rows as the
// game binds them (S1a ev 1383: `forest_asia_2.dds` and its overlay equal the bound 12883 and
// 12886 byte for byte; the vertex buffer 12879 has the `v` of the gr2 unflipped: 1.0 at the foot
// of a tree, our blob 0.0). Unflipped, every tree read the mirrored cell of the 4x4 atlas,
// upside down: darker and harder-edged than the capture.
fn forest_texcoord(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(uv.x, 1.0 - uv.y);
}

struct ForestVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec3<f32>,
    @location(2) v2: vec2<f32>,
    @location(3) v3: vec2<f32>,
}

@vertex
fn forest_vs(v: ForestVertexIn) -> ForestVertexOut {
    var out: ForestVertexOut;
    if (v.variant != v.tree_variant) {
        // Another variant than the tree's: the triangle folds to one point behind the near plane.
        out.clip = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        return out;
    }
    let r = forest_vs_core(v.position, forest_texcoord(v.uv), v.tree_location);
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.v2 = r.o3;
    out.v3 = r.o4;
    return out;
}

@fragment
fn forest_ss_overlay_ps(in: ForestVertexOut) -> @location(0) vec4<f32> {
    var b: ForestSsOverlayPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    // `vPos` is the pixel centre, as DX11 gives it (the captures were made with DX11); D3D9's
    // was the corner, `clip.xy - 0.5`.
    b.vpos = in.clip.xy;
    let r = forest_ss_overlay_ps_body(b);
    if (r.kill) {
        discard;
    }
    return r.oc0;
}

@fragment
fn forest_top_down_overlay_ps(in: ForestVertexOut) -> @location(0) vec4<f32> {
    var b: ForestTopDownPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    let r = forest_top_down_overlay_ps_body(b);
    if (r.kill) {
        discard;
    }
    return r.oc0;
}

// The class mask (`golden` only), not an entry of the original: the forest
// class where the leaf hides what lies behind it. The texkill of `ForestSSOverlayPS` (the fog mask
// at 0), and the alpha at a half, since the leaves are blended.
override MASK_CLASS: u32 = 4u;

@fragment
fn forest_mask_ps(in: ForestVertexOut) -> @location(0) u32 {
    let base = textureSample(BaseSampler, BaseSampler_smp, in.v0.xy);
    let mask = textureSample(FOWMask, FOWMask_smp, in.v2);
    if (base.w < 0.5 || -abs(mask.x) >= 0.0) {
        discard;
    }
    return MASK_CLASS;
}

@fragment
fn forest_ss_reflect_ps(in: ForestVertexOut) -> @location(0) vec4<f32> {
    var b: ForestSsReflectPsIn;
    b.v0 = in.v0.xy;
    b.v1 = in.v1.xy;
    let r = forest_ss_reflect_ps_body(b);
    if (r.kill) {
        discard;
    }
    return r.oc0;
}
