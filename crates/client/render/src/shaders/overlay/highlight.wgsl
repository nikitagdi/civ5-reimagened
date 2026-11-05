// The pixel shaders of the highlights (`HighlightSystem`, world-overlays WOV5, RW-20): `PSContour`
// (a strip with a texture that is a profile across it), `PSNoTexture` (a flat fill) and `PSOverlay`
// (a strip of one colour). Hand ports in the convention of shader-re 4.8.1. The vertex stage is
// `VSOverlay` of `overlay.wgsl`; the entries here take its two outputs, the colour at location 0 and
// the coordinates at location 1, and write colour times alpha (premultiplied, the pipeline blends
// with ONE, INV_SRC_ALPHA).
//
// Group 1: the profile texture and its sampler (`PSContour` only); group 2, binding 0: `DrawColor`
// (a dynamic offset per style).

struct HighlightPsConsts {
    DrawColor: vec4<f32>,
}

@group(2) @binding(0) var<uniform> highlight_ps_consts: HighlightPsConsts;

@group(1) @binding(0) var AlphaSampler: texture_2d<f32>;
@group(1) @binding(1) var AlphaSampler_smp: sampler;

// ---- PSContour ----

struct ContourIn {
    // dcl_color v0.xyz.
    v0: vec3<f32>,
    // dcl_texcoord v1.x: the place across the strip, 0 to 1.
    v1: f32,
}

struct ContourOut {
    oc0: vec4<f32>,
}

// re: OverlayShaders::PSContour (sha1 310753c72601): the alpha is the texture read at (v1.x, 0.5)
// times the alpha of `DrawColor`; the colour is `DrawColor` times the vertex colour; the whole
// thing is multiplied by that alpha.
fn pscontour_body(inp: ContourIn) -> ContourOut {
    let c = highlight_ps_consts.DrawColor;
    // c1 = (1, 0, 0.5, 0): mad r0.xy, v1.x, c1, c1.yzzw.
    let uv = vec2<f32>(inp.v1) * vec2<f32>(1.0, 0.0) + vec2<f32>(0.0, 0.5);
    let t = textureSample(AlphaSampler, AlphaSampler_smp, uv);
    let a = t.x * c.w;
    let rgb = c.xyz * inp.v0;
    var o: ContourOut;
    o.oc0 = a * vec4<f32>(rgb, 1.0);
    return o;
}

// ---- PSNoTexture ----

struct NoTextureIn {
    // The shader reads no input: a member only because a struct needs one.
    pad: f32,
}

struct NoTextureOut {
    oc0: vec4<f32>,
}

// re: OverlayShaders::PSNoTexture (sha1 3e793bd16091): `DrawColor` with its colour times its alpha.
fn psnotexture_body(inp: NoTextureIn) -> NoTextureOut {
    let c = highlight_ps_consts.DrawColor;
    // c1 = (1, 0, 0, 0): mad r0, r0.w, c1.xxxy, c1.yyyx with r0.w = c0.w.
    let r0 = c.w * vec4<f32>(1.0, 1.0, 1.0, 0.0) + vec4<f32>(0.0, 0.0, 0.0, 1.0);
    var o: NoTextureOut;
    o.oc0 = r0 * c;
    return o;
}

// ---- PSOverlay ----

struct OverlayPsIn {
    // dcl_color v0.
    v0: vec4<f32>,
}

struct OverlayPsOut {
    oc0: vec4<f32>,
    kill: bool,
}

// re: OverlayShaders::PSOverlay (sha1 f07d61f7ae4c): a pixel whose alpha (the vertex's times
// `DrawColor`'s) is 0.001 or less is dropped (`texkill` of `cmp(alpha - 0.001, -0, -1)`), else
// colour times alpha.
fn psoverlay_body(inp: OverlayPsIn) -> OverlayPsOut {
    let c = highlight_ps_consts.DrawColor;
    let r0 = c * inp.v0;
    let x = c.w * inp.v0.w + -0.00100000005;
    let keep = select(-1.0, -0.0, x >= 0.0);
    // c1 = (-0.001, -0, -1, 0): mad r1, r0.w, -c1.zzzy, -c1.wwwz = (w, w, w, 1).
    let r1 = r0.w * vec4<f32>(1.0, 1.0, 1.0, 0.0) + vec4<f32>(-0.0, -0.0, -0.0, 1.0);
    var o: OverlayPsOut;
    o.kill = keep < 0.0;
    o.oc0 = r0 * r1;
    return o;
}

// ---- the entries ----

@fragment
fn pscontour(
    @location(0) color: vec4<f32>,
    @location(1) uv: vec4<f32>,
) -> @location(0) vec4<f32> {
    var inp: ContourIn;
    inp.v0 = color.xyz;
    inp.v1 = uv.x;
    return pscontour_body(inp).oc0;
}

@fragment
fn psnotexture(
    @location(0) color: vec4<f32>,
    @location(1) uv: vec4<f32>,
) -> @location(0) vec4<f32> {
    var inp: NoTextureIn;
    inp.pad = 0.0;
    return psnotexture_body(inp).oc0;
}

@fragment
fn psoverlay(
    @location(0) color: vec4<f32>,
    @location(1) uv: vec4<f32>,
) -> @location(0) vec4<f32> {
    var inp: OverlayPsIn;
    inp.v0 = color;
    let r = psoverlay_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}
