// Fork particles (world-objects WOB8b): the quads the CPU expanded (`world/particle/draw.rs`),
// one pipeline per alpha state of the property (0x10C). The original's packages (rw-a6 RA6c):
// ForkDefaultShader 0x2D = DefaultVS + DefaultPS (pass 1 DefaultLinearPS), ForkBrightnessShader
// 0x2F = DefaultVS + BrightnessPS (one per-instance float4), ForkFakeVolumeShader 0x31 =
// CalcRotVS + FakeVolumePS; the globals are 8 float4 (view, projection). The VS rotation of
// CalcRotVS is done on the CPU here (`draw.rs`). The map draws pass 0 (RA6d): slot 0 BaseSampler
// is the batch texture, slot 1 AuxSampler0 the material texture (the FakeVolume gradient), one
// sampler (wrap, linear). The PS bodies were not read.
//
// SIMPLIFIED(SIMP-048): stand-in pixel stages. Default: texture * COLOR0 * COLOR1 (the tint);
// FakeVolume: the gradient texture looked up by the texture's luminance, times the colours, the
// texture's alpha; Brightness: Default with the colour times (1 + the package's float).
//
// Group 0 is the view (`frame.wgsl`); group 1 the texture, the gradient and the sampler.

@group(1) @binding(0) var fork_texture: texture_2d<f32>;
@group(1) @binding(1) var fork_gradient: texture_2d<f32>;
@group(1) @binding(2) var fork_sampler: sampler;

struct ForkVertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color0: vec4<f32>,
    @location(3) color1: vec4<f32>,
    // x: 0 default, 1 FakeVolume, 2 Brightness; y: the package's float.
    @location(4) mode: vec2<f32>,
}

struct ForkVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color0: vec4<f32>,
    @location(2) color1: vec4<f32>,
    @location(3) mode: vec2<f32>,
}

@vertex
fn fork_vs(v: ForkVertexIn) -> ForkVertexOut {
    var o: ForkVertexOut;
    o.clip = view_globals.view_proj * vec4<f32>(v.position, 1.0);
    o.uv = v.uv;
    o.color0 = v.color0;
    o.color1 = v.color1;
    o.mode = v.mode;
    return o;
}

@fragment
fn fork_ps(i: ForkVertexOut) -> @location(0) vec4<f32> {
    let t = textureSample(fork_texture, fork_sampler, i.uv);
    let lum = dot(t.rgb, vec3<f32>(0.299, 0.587, 0.114));
    let g = textureSample(fork_gradient, fork_sampler, vec2<f32>(clamp(lum, 0.0, 1.0), 0.5));
    var rgb = t.rgb;
    if (i.mode.x > 0.5 && i.mode.x < 1.5) {
        rgb = g.rgb;
    }
    rgb = rgb * i.color0.rgb * i.color1.rgb;
    if (i.mode.x > 1.5) {
        rgb = rgb * (1.0 + i.mode.y);
    }
    return vec4<f32>(rgb, t.a * i.color0.a * i.color1.a);
}
