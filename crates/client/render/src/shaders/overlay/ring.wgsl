// The pixel shader of the selection ring (`OverlayShaders::PSValidMoves`, world-overlays WOV5, RW-20):
// hand port in the convention of shader-re 4.8.1. The vertex stage is `VSOverlay` of `overlay.wgsl`,
// which hands on the glow's coordinates in `o2.xy` (`TransScale`) and the ring's, turned about the
// middle of the texture, in `o2.zw` (`TransScaleRot`). Output is premultiplied (the pipeline blends
// with ONE, INV_SRC_ALPHA).
//
// Group 1: the glow texture and its sampler (bindings 0, 1), the ring texture and its sampler (2, 3);
// group 2, binding 0: `DrawColor` and `fAlpha` (a block of 32 bytes: the colour, then the pulse).

struct RingPsConsts {
    DrawColor: vec4<f32>,
    fAlpha: vec4<f32>,
}

@group(2) @binding(0) var<uniform> ring_ps_consts: RingPsConsts;

@group(1) @binding(0) var GlowSampler: texture_2d<f32>;
@group(1) @binding(1) var GlowSampler_smp: sampler;
@group(1) @binding(2) var RingSampler: texture_2d<f32>;
@group(1) @binding(3) var RingSampler_smp: sampler;

struct ValidMovesIn {
    // dcl_texcoord v0: xy the glow's coordinates, zw the ring's.
    v0: vec4<f32>,
}

struct ValidMovesOut {
    oc0: vec4<f32>,
    kill: bool,
}

// re: OverlayShaders::PSValidMoves (sha1 ba0d489b9bba): the alpha is the glow's alpha, capped by
// `fAlpha`, plus the ring's alpha; a pixel with 0.001 or less is dropped (`texkill` of
// `cmp(a - 0.001, -0, -1)`), else colour times alpha, alpha itself.
fn psvalidmoves_body(inp: ValidMovesIn) -> ValidMovesOut {
    let glow = textureSample(GlowSampler, GlowSampler_smp, inp.v0.xy);
    let r1x = min(glow.w, ring_ps_consts.fAlpha.x);
    let ring = textureSample(RingSampler, RingSampler_smp, inp.v0.zw);
    let a = r1x + ring.w;
    var o: ValidMovesOut;
    o.kill = !((a + -0.00100000005) >= 0.0);
    o.oc0 = vec4<f32>(a * ring_ps_consts.DrawColor.xyz, a);
    return o;
}

@fragment
fn psvalidmoves(
    @location(0) color: vec4<f32>,
    @location(1) uv: vec4<f32>,
) -> @location(0) vec4<f32> {
    var inp: ValidMovesIn;
    inp.v0 = uv;
    let r = psvalidmoves_body(inp);
    if r.kill {
        discard;
    }
    return r.oc0;
}
