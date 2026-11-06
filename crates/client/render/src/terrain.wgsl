// Terrain pipeline: one texture array, the layer of a vertex
// chosen by its slot, UV from the world XZ position, Lambert light, fog from the fog-state
// texture. Uniform buffers, one texture array and one integer texture only (WebGL2).
//
// SIMPLIFIED(SIMP-005): one tiled texture per cell type, no blending at cell seams.
// SIMPLIFIED(SIMP-013): fog is a flat colour (unrevealed, the clouds are a later pass) or a
// darkening and desaturation (hidden).

// Direction towards the sun, for the ground only (world X east, Y up, Z south). The original's
// `g_SunDir` from `TerrainSystem::Startup` (civ5-re 0x0867eac8): (0.4330127, -0.25, 0.8660254)
// in map space (x east, y north, z up), 60 degrees up from the south-east. The brightness of flat
// ground is kept as it was (`FLAT_LIT`), so only the slopes change. The same direction is in
// water.wgsl.
// SIMPLIFIED(SIMP-025): ambient and FLAT_LIT are chosen; the original lights the ground from the
// Sunlight_*/Shadowlight_* cube maps, which we do not read.
const TERRAIN_SUN: vec3<f32> = vec3<f32>(0.4330127, 0.8660254, 0.25);
const FLAT_LIT: f32 = 0.90;

// How much of its colour a revealed but hidden cell keeps; `FOG_HIDDEN_SATURATION` in fog.rs.
const HIDDEN_SATURATION: f32 = 0.55;

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

struct Slot {
    // rgb: flat colour (linear) used when the slot has no layer.
    color: vec4<f32>,
    // x: array layer, negative for none. y: texture scale (UV per world unit).
    tex: vec4<f32>,
};

struct Material {
    // rgb: colour of unrevealed cells (linear).
    fog_color: vec4<f32>,
    slots: array<Slot, 64>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> object: Object;
// Fog state per cell: 0 unrevealed, 1 revealed and hidden, 2 visible.
@group(2) @binding(0) var fog_tex: texture_2d<u32>;
@group(3) @binding(0) var<uniform> material: Material;
@group(3) @binding(1) var layers: texture_2d_array<f32>;
@group(3) @binding(2) var layer_sampler: sampler;

// The cutout mask of the mod (client-hooks, D13): the same text as `shaders/common/cutout.wgsl`.
struct CutoutParams {
    // x: 1 when a mask is set. y, z: columns and rows of the mask. w: the block row of mask row 0.
    grid: vec4<f32>,
    // x: 1 when the map wraps along X.
    flags: vec4<f32>,
}
@group(3) @binding(3) var CutoutMask: texture_2d<f32>;
@group(3) @binding(4) var<uniform> cutout: CutoutParams;

// The point (x, z) of our basis lies over a cut column.
fn cutout_hit(x: f32, z: f32) -> bool {
    if (cutout.grid.x == 0.0) {
        return false;
    }
    let cols = i32(cutout.grid.y);
    let rows = i32(cutout.grid.z);
    let bx = i32(floor(x * 0.25));
    let row = i32(floor(z * 0.25)) - i32(cutout.grid.w);
    var col = bx;
    if (cutout.flags.x != 0.0) {
        col = ((bx % cols) + cols) % cols;
    }
    if (col < 0 || col >= cols || row < 0 || row >= rows) {
        return false;
    }
    return textureLoad(CutoutMask, vec2<i32>(col, row), 0).x > 0.0;
}

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) cell: u32,
    @location(3) slot: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) cell: u32,
    @location(3) @interpolate(flat) slot: u32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let world = object.model * vec4<f32>(v.position, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = (object.model * vec4<f32>(v.normal, 0.0)).xyz;
    out.cell = v.cell;
    out.slot = v.slot;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Over a column cut by the mod there is no ground.
    if (cutout_hit(in.world_pos.x, in.world_pos.z)) {
        discard;
    }
    let width = max(u32(globals.params.y), 1u);
    let state = textureLoad(fog_tex, vec2<i32>(i32(in.cell % width), i32(in.cell / width)), 0).r;

    let slot = material.slots[in.slot];
    let scale = slot.tex.y;
    let uv = in.world_pos.xz * scale;
    // Gradients from the continuous world position: the scale changes from cell to cell, and
    // derivatives of the UV would jump along the seams and pick a blurry mip level there.
    let ddx = dpdx(in.world_pos.xz) * scale;
    let ddy = dpdy(in.world_pos.xz) * scale;
    let layer = max(i32(slot.tex.x), 0);
    let sampled = textureSampleGrad(layers, layer_sampler, uv, layer, ddx, ddy).rgb;
    let albedo = select(slot.color.rgb, sampled, slot.tex.x >= 0.0);

    let n = normalize(in.normal);
    let sun = normalize(TERRAIN_SUN);
    let diffuse = max(dot(n, sun), 0.0);
    let ambient = 0.18;
    let flat_diffuse = ambient + (1.0 - ambient) * sun.y;
    var rgb = albedo * (ambient + (1.0 - ambient) * diffuse) * (FLAT_LIT / flat_diffuse);
    if (state == 1u) {
        let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(vec3<f32>(grey), rgb, HIDDEN_SATURATION) * globals.params.z;
    } else if (state == 0u) {
        rgb = material.fog_color.rgb;
    }
    if (globals.params.x > 0.5) {
        rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, 1.0);
}
