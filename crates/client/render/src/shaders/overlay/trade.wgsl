// TradeArrowShader of the original, the trade route lines (`TradeVisSystem`, world-overlays, card
// R5.9). The bytecode of its vertex and pixel shaders is not in the game's files (RW-22b), so this is
// written from what the constants and the vertex say, not ported from a disassembly: the vertex
// shader carries a point of the plane of the map through `Local` and the projection of the overlay
// (as `VSOverlayCountry` does), the pixel shader reads `ArrowTexture` at `(u, v + phase)` and
// multiplies it by the vertex colour.
//
// Group 0, binding 0: the constants (+0 the projection, +0x40 the scroll phase of
// `TradeVisSystem::Update`, `frac(phase + dt * 0.75)`; `Local` between them is ours: the original
// draws the route in the map of the overlay directly, we also move it by the copy of a wrapping map).
// Group 1: `ArrowTexture`. `v = -2t` falls along the route, so adding the phase moves the texture
// from the start of the route to its end.
//
// The pipeline blends ONE, INV_SRC_ALPHA like the overlay's alpha state: the pixel shader writes the
// colour times its alpha.

struct TradeConsts {
    // row_major float4x4: register k is row k.
    Projection: mat4x4<f32>,
    Local: mat4x4<f32>,
    // .x: the scroll phase.
    phase: vec4<f32>,
}

@group(0) @binding(0) var<uniform> trade_consts: TradeConsts;
@group(1) @binding(0) var ArrowTexture: texture_2d<f32>;
@group(1) @binding(1) var ArrowTexture_smp: sampler;

// Register `k` of a `row_major float4x4`: `[j][k]` of the WGSL matrix is component j of register k.
fn trade_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

struct TradeVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
}

@vertex
fn vs_trade(
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
) -> TradeVertexOut {
    let c = trade_consts;
    var r0 = trade_reg(c.Local, 1u) * pos.y;
    r0 = pos.x * trade_reg(c.Local, 0u) + r0;
    r0 = r0 + trade_reg(c.Local, 3u);
    var r1 = r0.y * trade_reg(c.Projection, 1u);
    r1 = r0.x * trade_reg(c.Projection, 0u) + r1;
    r1 = r0.z * trade_reg(c.Projection, 2u) + r1;
    var out: TradeVertexOut;
    out.clip = r0.w * trade_reg(c.Projection, 3u) + r1;
    out.color = color;
    out.uv = uv;
    return out;
}

@fragment
fn ps_trade(in: TradeVertexOut) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(in.uv.x, in.uv.y + trade_consts.phase.x);
    // The texture is a mask: white arrow on black, alpha 255 everywhere (arrow*.dds). Its red is the
    // coverage; the colour is the player's (`arrow_pixel` in trade.rs is the CPU twin).
    let mask = textureSample(ArrowTexture, ArrowTexture_smp, uv).r;
    let a = mask * in.color.a;
    return vec4<f32>(in.color.rgb * a, a);
}
