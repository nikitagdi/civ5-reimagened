// `Present`: copies the resolved world (`Scene`, gamma space) onto the surface.
// A surface without sRGB takes the values as they are. A surface that only offers sRGB encodes
// what the pass writes, so `fs_linearize` writes the inverse and the same values end up there.
// With a render scale below one the scene is smaller than the surface: `fs_scaled` and
// `fs_scaled_linearize` stretch it with the linear filter (the copies read texel by texel).

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

struct Vertex {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Vertex {
    // One triangle that covers the target.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Vertex;
    out.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_copy(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(textureLoad(scene, vec2<i32>(pos.xy), 0).rgb, 1.0);
}

@fragment
fn fs_linearize(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_to_linear(textureLoad(scene, vec2<i32>(pos.xy), 0).rgb), 1.0);
}

@fragment
fn fs_scaled(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSampleLevel(scene, scene_sampler, uv, 0.0).rgb, 1.0);
}

@fragment
fn fs_scaled_linearize(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(srgb_to_linear(textureSampleLevel(scene, scene_sampler, uv, 0.0).rgb), 1.0);
}
