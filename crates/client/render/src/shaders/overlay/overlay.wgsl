// The overlay shaders of the original that draw the hex grid (`OverlayShaders` package, which
// `DrawSimpleOverlay` uses for `HexOverlay`): `VSOverlay` and `PSSimpleTexture`. Hand ports in the
// convention of shader-re 4.8.1 (world-overlays WOV2): each entry of the original is a body
// `fn <entry>_body(inp)` with the registers as member names, and the `@vertex` / `@fragment` entries
// only read their inputs, call the body and hand the result on.
//
// The grid is drawn into the map of the overlay (`m_mOverlayProj`): the vertex shader carries the
// corner of a hex through `Local` and `WVP` and turns the grid texture's coordinates into the
// two coordinate pairs `o2.xy` and `o2.zw` (`TransScale`, `TransScaleRot`); the pixel shader drops
// what the grid texture leaves clear and writes colour times alpha (premultiplied: the pipeline
// blends with ONE, INV_SRC_ALPHA, `m_hAlphaState` of `OverlaySystem::Startup`).
//
// Group 0, binding 0: the constants of `VSOverlay`; group 1: the grid texture and its sampler.

struct OverlayVsConsts {
    // row_major float4x4: register k is row k.
    WVP: mat4x4<f32>,
    Local: mat4x4<f32>,
    TransScaleRot: mat4x4<f32>,
    TransScale: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> overlay_vs_consts: OverlayVsConsts;

@group(1) @binding(0) var GlowSampler: texture_2d<f32>;
@group(1) @binding(1) var GlowSampler_smp: sampler;

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// ---- VSOverlay ----

struct OverlayVsIn {
    // dcl_position v0.
    v0: vec4<f32>,
    // dcl_color v1.
    v1: vec4<f32>,
    // dcl_texcoord v2: x and y are the grid texture's coordinates.
    v2: vec2<f32>,
}

struct OverlayVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
}

// re: OverlayShaders::VSOverlay (sha1 9ffd8f9118e1): the corner through `Local`, then `WVP`; the
// texture coordinates through the 2D part of `TransScale` (into `o2.xy`) and of `TransScaleRot`
// (into `o2.zw`).
fn vsoverlay_body(inp: OverlayVsIn) -> OverlayVsOut {
    let c = overlay_vs_consts;
    var r0 = row_major_reg(c.Local, 1u) * inp.v0.y;
    r0 = inp.v0.x * row_major_reg(c.Local, 0u) + r0;
    r0 = inp.v0.z * row_major_reg(c.Local, 2u) + r0;
    r0 = inp.v0.w * row_major_reg(c.Local, 3u) + r0;
    var r1 = r0.y * row_major_reg(c.WVP, 1u);
    r1 = r0.x * row_major_reg(c.WVP, 0u) + r1;
    r1 = r0.z * row_major_reg(c.WVP, 2u) + r1;
    var o: OverlayVsOut;
    o.o0 = r0.w * row_major_reg(c.WVP, 3u) + r1;
    var t = row_major_reg(c.TransScale, 1u).xy * inp.v2.y;
    t = inp.v2.x * row_major_reg(c.TransScale, 0u).xy + t;
    let xy = t + row_major_reg(c.TransScale, 3u).xy;
    var u = row_major_reg(c.TransScaleRot, 1u).xy * inp.v2.y;
    u = inp.v2.x * row_major_reg(c.TransScaleRot, 0u).xy + u;
    let zw = u + row_major_reg(c.TransScaleRot, 3u).xy;
    o.o2 = vec4<f32>(xy, zw);
    o.o1 = inp.v1;
    return o;
}

struct OverlayVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec4<f32>,
}

@vertex
fn vsoverlay(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec2<f32>,
) -> OverlayVertexOut {
    var inp: OverlayVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    inp.v2 = v2;
    let r = vsoverlay_body(inp);
    var out: OverlayVertexOut;
    out.clip = r.o0;
    out.color = r.o1;
    out.uv = r.o2;
    return out;
}

// ---- PSSimpleTexture ----

struct SimpleTextureIn {
    // dcl_texcoord v0.xy.
    v0: vec2<f32>,
}

struct SimpleTextureOut {
    oc0: vec4<f32>,
    kill: bool,
}

// re: OverlayShaders::PSSimpleTexture (sha1 05af36e52ea2): a pixel with 0.001 or less of alpha is
// dropped (`texkill` of `cmp(alpha - 0.001, -0, -1)`), else colour times alpha, alpha itself.
fn pssimpletexture_body(inp: SimpleTextureIn) -> SimpleTextureOut {
    let t = textureSample(GlowSampler, GlowSampler_smp, inp.v0);
    var o: SimpleTextureOut;
    o.kill = !((t.w + -0.00100000005) >= 0.0);
    o.oc0 = t * vec4<f32>(t.w, t.w, t.w, 1.0);
    return o;
}

@fragment
fn pssimpletexture(in: OverlayVertexOut) -> @location(0) vec4<f32> {
    var inp: SimpleTextureIn;
    inp.v0 = in.uv.xy;
    let r = pssimpletexture_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}

// ---- the grid's own pixel shader (not a port) ----

// The colour of `HexGrid.xml` over `PSSimpleTexture`: its rgb only. The dumped `HexOverlay::Render`
// never writes `m_vColor` into the draw's constants, and the S1a capture agrees: ev 741 is the plain
// `PSSimpleTexture` with white vertices, and the lines in the map reach the texture's own alpha
// (0.37 against 0.58 at most), not 75 / 255 of it. The shipped alpha 75 is not applied.
struct GridTint {
    color: vec4<f32>,
}
@group(2) @binding(0) var<uniform> grid_tint: GridTint;

@fragment
fn grid_ps(in: OverlayVertexOut) -> @location(0) vec4<f32> {
    var inp: SimpleTextureIn;
    inp.v0 = in.uv.xy;
    let r = pssimpletexture_body(inp);
    if r.kill {
        discard;
    }
    let tint = grid_tint.color;
    return vec4<f32>(r.oc0.xyz * tint.xyz, r.oc0.w);
}
