// Skinned/rigid model shader: bones from the bone texture (`skin.wgsl`, joined in front of this
// file; group 1 binding 14), diffuse texture, Lambert + ambient. No storage buffers (WebGL2).

struct Globals {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: unit direction to light.
    light_dir: vec4<f32>,
    // x: 1.0 if gamma must be applied in the shader (non-sRGB surface).
    params: vec4<f32>,
};

// Group 3, the call. `bone_base_next` and `bone_blend` are for the blending of
// `skin.wgsl`; B1's showcase has no blend.
struct Instance {
    model: mat4x4<f32>,
    bone_base: u32,
    bone_base_next: u32,
    bone_blend: f32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(2) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(2) @binding(1) var diffuse_samp: sampler;
@group(3) @binding(0) var<uniform> instance: Instance;

const AMBIENT: f32 = 0.35;

struct VsIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) joints: vec4<u32>,
    @location(4) weights: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let skin = skin_matrix(
        v.joints, v.weights, instance.bone_base, instance.bone_base_next, instance.bone_blend);
    let n = oct_decode(v.normal_oct);
    var out: VsOut;
    let local = skin_position(skin, v.position);
    out.clip = globals.view_proj * (instance.model * vec4<f32>(local, 1.0));
    // Uniform scale assumed, so no inverse transpose.
    out.normal = (instance.model * vec4<f32>(skin_normal(skin, n), 0.0)).xyz;
    out.uv = v.uv;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // sRGB textures are decoded to linear by the sampler.
    let albedo = textureSample(diffuse_tex, diffuse_samp, in.uv).rgb;
    let n = normalize(in.normal);
    let lambert = max(dot(n, globals.light_dir.xyz), 0.0);
    var rgb = albedo * (AMBIENT + (1.0 - AMBIENT) * lambert);
    if (globals.params.x > 0.5) {
        rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, 1.0);
}
