// Original-UI pass: textured, tinted triangles in physical pixels (Y down).
// Group 0: per-frame constants and (dynamic offset) per-command mask transform.
// Group 1: texture, second texture (mask or white) and sampler.

struct Frame {
    // (2 / width, 2 / height): pixels to NDC.
    to_ndc: vec2<f32>,
    // 1.0 when the target is sRGB and the output must be linearised to land unchanged.
    target_srgb: f32,
    pad: f32,
}

struct Cmd {
    // Mask UV from the vertex position in pixels: scale xy, offset zw.
    mask_xform: vec4<f32>,
    // xy: scale of the texture's UV, below 1 for a texture padded to whole 4x4 blocks
    // zw unused.
    uv_scale: vec4<f32>,
}

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var<uniform> cmd: Cmd;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var tex2: texture_2d<f32>;
@group(1) @binding(2) var smp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) mask_uv: vec2<f32>,
}

@vertex
fn vs(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos.x * frame.to_ndc.x - 1.0, 1.0 - pos.y * frame.to_ndc.y, 0.0, 1.0);
    o.uv = uv * cmd.uv_scale.xy;
    o.color = color;
    o.mask_uv = pos * cmd.mask_xform.xy + cmd.mask_xform.zw;
    return o;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn finish(c: vec4<f32>) -> vec4<f32> {
    if frame.target_srgb > 0.5 {
        return vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}

@fragment
fn fs_plain(in: VsOut) -> @location(0) vec4<f32> {
    return finish(textureSampleLevel(tex, smp, in.uv, 0.0) * in.color);
}

@fragment
fn fs_masked(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(tex, smp, in.uv, 0.0) * in.color;
    // SIMPLIFIED(SIMP-uir-5): the exe's mask channel is not read from its shader.
    // Product of red and alpha: an L8/R8 mask (alpha 1) and an alpha-only RGBA mask (red 1) both work.
    let m = textureSampleLevel(tex2, smp, in.mask_uv, 0.0);
    c.a = c.a * m.r * m.a;
    return finish(c);
}
