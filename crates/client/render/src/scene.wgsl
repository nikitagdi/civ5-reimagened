// Test scene shader: vertex color, Lambert + Blinn-Phong. Uniform buffers only (WebGL2).

struct Globals {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: unit direction to light.
    light_dir: vec4<f32>,
    // x: 1.0 if gamma must be applied in the shader (non-sRGB surface).
    params: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    // xyz: color multiplier, w: specular strength.
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> object: Object;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let world = object.model * vec4<f32>(v.position, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    // Uniform scale, so no inverse transpose is needed.
    out.normal = (object.model * vec4<f32>(v.normal, 0.0)).xyz;
    out.color = v.color * object.tint.xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let l = globals.light_dir.xyz;
    let v = normalize(globals.camera_pos.xyz - in.world_pos);
    let h = normalize(l + v);
    let diffuse = max(dot(n, l), 0.0);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * object.tint.w * step(0.0, dot(n, l));
    let ambient = 0.18;
    var rgb = in.color * (ambient + (1.0 - ambient) * diffuse) + vec3<f32>(spec);
    if (globals.params.x > 0.5) {
        rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, 1.0);
}
