// Static model pipeline (shared with entity-render): instanced
// rigid models. One draw per (model, material): the vertex buffer holds every variant of the model,
// each vertex tagged with its variant, and an instance shows only the vertices of its own variant
// (the others are collapsed outside the clip volume). Fog from the fog-state texture of the cell
// the instance stands on. Uniform buffers, one 2D texture and one integer texture (WebGL2).

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
@group(3) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(3) @binding(1) var diffuse_samp: sampler;

const AMBIENT: f32 = 0.35;
// Foliage (the cutout entry point) is lit more flatly and brighter than rigid models: the tree
// textures are dark (about a quarter of full brightness) with baked shading, and a forest of them
// read as black clumps under the plain light.
// SIMPLIFIED(SIMP-009): own values; the original ForestShader light is not known.
const FOLIAGE_AMBIENT: f32 = 0.6;
const FOLIAGE_GAIN: f32 = 1.8;
// The smallest alpha a foliage texel is divided by to undo the black bleed.
const FOLIAGE_MIN_ALPHA: f32 = 0.35;
// How much of its colour an instance on a revealed but hidden cell keeps; `FOG_HIDDEN_SATURATION`
// in fog.rs.
const HIDDEN_SATURATION: f32 = 0.55;
// Instance `cell` of an instance that is not fogged.
const NO_CELL: u32 = 0xFFFFFFFFu;
// Texels with less alpha are dropped by the cutout entry point.
const ALPHA_CUTOFF: f32 = 0.5;

struct VsIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) variant: u32,
    // Instance: xyz position, w rotation about Y.
    @location(4) pos_rot: vec4<f32>,
    // Instance: x uniform scale, yzw tint.
    @location(5) scale_tint: vec4<f32>,
    @location(6) alpha: f32,
    @location(7) cell: u32,
    @location(8) inst_variant: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tint: vec4<f32>,
    @location(3) @interpolate(flat) cell: u32,
};

fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}

fn rotate_y(p: vec3<f32>, c: f32, s: f32) -> vec3<f32> {
    return vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let c = cos(v.pos_rot.w);
    let s = sin(v.pos_rot.w);
    let local = rotate_y(v.position * v.scale_tint.x, c, s);
    let world = object.model * vec4<f32>(local + v.pos_rot.xyz, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * world;
    if (v.variant != v.inst_variant) {
        // All three vertices of a foreign triangle land on the same point outside the volume.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.normal = (object.model * vec4<f32>(rotate_y(oct_decode(v.normal_oct), c, s), 0.0)).xyz;
    out.uv = v.uv;
    out.tint = vec4<f32>(v.scale_tint.yzw, v.alpha);
    out.cell = v.cell;
    return out;
}

fn shade(in: VsOut, front: bool, cutout: bool) -> vec4<f32> {
    // sRGB textures are decoded to linear by the sampler.
    let texel = textureSample(diffuse_tex, diffuse_samp, in.uv);
    var fog_factor = 1.0;
    var hidden = false;
    if (in.cell != NO_CELL) {
        let width = max(u32(globals.params.y), 1u);
        let state = textureLoad(fog_tex, vec2<i32>(i32(in.cell % width), i32(in.cell / width)), 0).r;
        if (state == 0u) {
            discard;
        }
        if (state == 1u) {
            fog_factor = globals.params.z;
            hidden = true;
        }
    }
    let alpha = texel.a * in.tint.a;
    if (cutout && alpha < ALPHA_CUTOFF) {
        discard;
    }
    // Foliage is two-sided: the back face is lit with the flipped normal.
    let n = select(-normalize(in.normal), normalize(in.normal), front);
    let lambert = max(dot(n, globals.light_dir.xyz), 0.0);
    let ambient = select(AMBIENT, FOLIAGE_AMBIENT, cutout);
    let gain = select(1.0, FOLIAGE_GAIN, cutout);
    // Foliage texels fade to transparent black at the leaf edges, and the mip levels average that
    // black into the leaves: undo it, or a distant forest turns into a dark clump.
    let albedo = select(texel.rgb, texel.rgb / max(texel.a, FOLIAGE_MIN_ALPHA), cutout);
    var rgb = albedo * in.tint.rgb * (ambient + (1.0 - ambient) * lambert) * gain;
    if (hidden) {
        let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(vec3<f32>(grey), rgb, HIDDEN_SATURATION);
    }
    rgb = rgb * fog_factor;
    if (globals.params.x > 0.5) {
        rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, 1.0);
}

@fragment
fn fs_opaque(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(in, front, false);
}

@fragment
fn fs_cutout(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    return shade(in, front, true);
}
