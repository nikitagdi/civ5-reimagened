// `cvResolvePass` of the original: the copy of the resolved back buffer onto the screen (S3 draws it,
// before the UI). Hand port in the convention of shader-re 4.8.1: a body `fn <entry>_body(inp: In)
// -> Out` with the registers of the original as member names, and an entry that only reads its
// inputs and calls the body.
//
// Self-contained: no frame globals, no sm3 helpers. Group 0: the resolved image
// `ResolvedBackBuffer` with its sampler.

@group(0) @binding(0) var ResolvedBackBuffer: texture_2d<f32>;
@group(0) @binding(1) var ResolvedBackBuffer_smp: sampler;

// ---- VS ----

struct ResolveVsIn {
    // dcl_texcoord v0.
    v0: vec4<f32>,
    // dcl_position v1.
    v1: vec4<f32>,
}

struct ResolveVsOut {
    // dcl_texcoord o0.xy.
    o0: vec2<f32>,
    // dcl_position o1.
    o1: vec4<f32>,
}

// re: cvResolvePass::VS (sha1 003e91a284e9): `mov o0.xy, v0; mov o1, v1`.
fn resolve_vs_body(inp: ResolveVsIn) -> ResolveVsOut {
    var o: ResolveVsOut;
    o.o0 = inp.v0.xy;
    o.o1 = inp.v1;
    return o;
}

// ---- PS ----

struct ResolvePsIn {
    // dcl_texcoord v0.xy.
    v0: vec2<f32>,
}

struct ResolvePsOut {
    oc0: vec4<f32>,
}

// re: cvResolvePass::PS (sha1 df80586f12fe): `texldl` at level 0 of the image, the colour with an
// alpha of 1 (`def c0, 1, 0, 0, 0`).
fn resolve_ps_body(inp: ResolvePsIn) -> ResolvePsOut {
    let c = textureSampleLevel(ResolvedBackBuffer, ResolvedBackBuffer_smp, inp.v0, 0.0);
    var o: ResolvePsOut;
    o.oc0 = vec4<f32>(c.xyz, 1.0);
    return o;
}

// ---- entry points ----

struct ResolveVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn resolve_vs(@location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>) -> ResolveVertexOut {
    var inp: ResolveVsIn;
    inp.v0 = v0;
    inp.v1 = v1;
    let r = resolve_vs_body(inp);
    var out: ResolveVertexOut;
    out.clip = r.o1;
    out.uv = r.o0;
    return out;
}

@fragment
fn resolve_ps(in: ResolveVertexOut) -> @location(0) vec4<f32> {
    var inp: ResolvePsIn;
    inp.v0 = in.uv;
    return resolve_ps_body(inp).oc0;
}
