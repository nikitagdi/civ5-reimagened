// Blend pipeline: alpha-blended map overlays (rivers, borders, grid). Uniform buffers and one
// integer texture only (WebGL2).

struct Globals {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    // x: 1.0 if gamma must be applied in the shader (non-sRGB surface).
    // y: map width in cells (fog texture width). z: colour factor of revealed-but-hidden cells.
    params: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> object: Object;
// Fog state per cell: 0 unrevealed, 1 revealed and hidden, 2 visible.
@group(2) @binding(0) var fog_tex: texture_2d<u32>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) across: f32,
    @location(2) color: vec4<f32>,
    @location(3) cell: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) across: f32,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) cell: u32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = globals.view_proj * (object.model * vec4<f32>(v.position, 1.0));
    out.across = v.across;
    out.color = v.color;
    out.cell = v.cell;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let width = max(u32(globals.params.y), 1u);
    let state = textureLoad(fog_tex, vec2<i32>(i32(in.cell % width), i32(in.cell / width)), 0).r;
    var rgb = in.color.rgb;
    var alpha = in.color.a;
    if (state == 0u) {
        alpha = 0.0;
    } else if (state == 1u) {
        rgb = rgb * globals.params.z;
    }
    if (in.across >= 0.0) {
        alpha = alpha * smoothstep(0.0, 0.3, min(in.across, 1.0 - in.across));
    }
    if (globals.params.x > 0.5) {
        rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, alpha);
}
