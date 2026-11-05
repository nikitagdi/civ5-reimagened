// The borders of the civilizations (`CultureBorders::Render`, world-overlays WOV4, RW-19): the
// vertex and pixel shaders the original draws the border mesh with into the map of the overlay.
// Hand ports in the convention of shader-re 4.8.1: each entry of the original is a body
// `fn <entry>_body(inp)` with the registers as member names, and the `@vertex` / `@fragment` entries
// only read their inputs, call the body and hand the result on.
//
// The mesh is a list of points in the plane of the map with a colour each (`POSITION` float2,
// `COLOR` four bytes); the vertex shader carries a point through `Local` and `WVP` and hands the
// colour on, the pixel shader multiplies it by `DrawColor` and writes colour times alpha
// (premultiplied: the pipeline blends with ONE, INV_SRC_ALPHA like `OverlaySystem`'s state).
//
// Group 0, binding 0: the constants of `VSOverlayCountry`; group 1, binding 0: `DrawColor`.

struct CountryVsConsts {
    // row_major float4x4: register k is row k. `WVP` is c0..c3, `Local` is c4..c7.
    WVP: mat4x4<f32>,
    Local: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> country_vs_consts: CountryVsConsts;

struct CountryPsConsts {
    DrawColor: vec4<f32>,
}

@group(1) @binding(0) var<uniform> country_ps_consts: CountryPsConsts;

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// ---- VSOverlayCountry ----

struct CountryVsIn {
    // dcl_position v0.
    v0: vec4<f32>,
    // dcl_color v1.
    v1: vec4<f32>,
}

struct CountryVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
}

// re: OverlayShaders::VSOverlayCountry (sha1 b567dfe098c1): the point through `Local` (only x, y and
// the translation), then `WVP`; the colour on.
fn vsoverlaycountry_body(inp: CountryVsIn) -> CountryVsOut {
    let c = country_vs_consts;
    var r0 = row_major_reg(c.Local, 1u) * inp.v0.y;
    r0 = inp.v0.x * row_major_reg(c.Local, 0u) + r0;
    r0 = r0 + row_major_reg(c.Local, 3u);
    var r1 = r0.y * row_major_reg(c.WVP, 1u);
    r1 = r0.x * row_major_reg(c.WVP, 0u) + r1;
    r1 = r0.z * row_major_reg(c.WVP, 2u) + r1;
    var o: CountryVsOut;
    o.o0 = r0.w * row_major_reg(c.WVP, 3u) + r1;
    o.o1 = inp.v1;
    return o;
}

struct CountryVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vsoverlaycountry(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> CountryVertexOut {
    var inp: CountryVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = vsoverlaycountry_body(inp);
    var out: CountryVertexOut;
    out.clip = r.o0;
    out.color = r.o1;
    return out;
}

// ---- PSCountryBorder ----

struct CountryBorderIn {
    // dcl_color v0.
    v0: vec4<f32>,
}

struct CountryBorderOut {
    oc0: vec4<f32>,
}

// re: OverlayShaders::PSCountryBorder (sha1 14813ec24959): colour times `DrawColor`, then the colour
// channels times the alpha.
fn pscountryborder_body(inp: CountryBorderIn) -> CountryBorderOut {
    let r0 = country_ps_consts.DrawColor * inp.v0;
    // c1 = (1, 0, 0, 0): c1.xxxy = (1, 1, 1, 0), c1.yyyx = (0, 0, 0, 1).
    let r1 = r0.w * vec4<f32>(1.0, 1.0, 1.0, 0.0) + vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var o: CountryBorderOut;
    o.oc0 = r0 * r1;
    return o;
}

@fragment
fn pscountryborder(in: CountryVertexOut) -> @location(0) vec4<f32> {
    var inp: CountryBorderIn;
    inp.v0 = in.color;
    return pscountryborder_body(inp).oc0;
}
