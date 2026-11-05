// Water pipeline (SIMP-008): the water cells drawn a second time at the
// water plane over their seabed (the terrain pass drew it), alpha-blended. Colour and opacity come
// from the depth ramp of the game's `waterdepthcolor.dds`, the surface is rippled by two
// scrolling layers of the game's `waterbumps.dds`, with a glint of the sun and a foam line on the
// shore. Uniform buffers, one 2D texture and one integer texture only (WebGL2).
//
// SIMPLIFIED(SIMP-008): flat plane and a hand-written approximation of the water shader of the
// original, which is only in the exe.

struct Globals {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>,
    // x: 1.0 if gamma must be applied in the shader (non-sRGB surface).
    // y: map width in cells (fog texture width). z: colour factor of revealed-but-hidden cells.
    // w: seconds since the start (scrolling of the waves).
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
@group(3) @binding(0) var bump: texture_2d<f32>;
@group(3) @binding(1) var bump_sampler: sampler;

// Same sun as the terrain pass (terrain.wgsl TERRAIN_SUN).
// The original's g_SunDir, see terrain.wgsl.
// SIMPLIFIED(SIMP-025): the ambient is chosen, not taken from the game.
const SUN: vec3<f32> = vec3<f32>(0.4330127, 0.8660254, 0.25);
// How much of its colour a revealed but hidden cell keeps; `FOG_HIDDEN_SATURATION` in fog.rs.
const HIDDEN_SATURATION: f32 = 0.55;
// Depth value (0 shore, 1 deep) to the coordinate on the colour ramp, and the opacity ceiling:
// the seabed stays visible in the open ocean too (the opacity does not come from the ramp, whose
// alpha is for a renderer that blends in gamma space).
const RAMP_SCALE: f32 = 0.68;
const MAX_ALPHA: f32 = 0.94;
// Strength of the ripples (tilt of the normal per unit of height slope).
const BUMP_STRENGTH: f32 = 0.25;
const WATER_TINT: vec3<f32> = vec3<f32>(0.01, 0.20, 0.42);
const TINT_MIX: f32 = 0.75;
const SKY: vec3<f32> = vec3<f32>(0.40, 0.47, 0.53);

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(2) cell: u32,
    @location(4) depth: f32,
    @location(5) shore: f32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) @interpolate(flat) cell: u32,
    @location(2) depth: f32,
    @location(3) shore: f32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let world = object.model * vec4<f32>(v.position, 1.0);
    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.world_pos = world.xyz;
    out.cell = v.cell;
    out.depth = v.depth;
    out.shore = v.shore;
    return out;
}

fn srgb(c: vec3<f32>) -> vec3<f32> {
    return pow(c / 255.0, vec3<f32>(2.2));
}

// The colour ramp of `waterdepthcolor.dds` (x = 0 shallow ... 1 deep; rgb sRGB 0..255, alpha 0..1),
// sampled at x = 0, 0.125, 0.25, 0.5, 0.75, 1.
fn ramp(x: f32) -> vec4<f32> {
    let c0 = vec4<f32>(srgb(vec3<f32>(69.0, 148.0, 140.0)), 0.0);
    let c1 = vec4<f32>(srgb(vec3<f32>(68.0, 132.0, 122.0)), 0.004);
    let c2 = vec4<f32>(srgb(vec3<f32>(69.0, 122.0, 113.0)), 0.385);
    let c3 = vec4<f32>(srgb(vec3<f32>(42.0, 82.0, 83.0)), 1.0);
    let c4 = vec4<f32>(srgb(vec3<f32>(19.0, 42.0, 51.0)), 1.0);
    let c5 = vec4<f32>(srgb(vec3<f32>(15.0, 37.0, 49.0)), 1.0);
    if (x < 0.125) {
        return mix(c0, c1, x / 0.125);
    } else if (x < 0.25) {
        return mix(c1, c2, (x - 0.125) / 0.125);
    } else if (x < 0.5) {
        return mix(c2, c3, (x - 0.25) / 0.25);
    } else if (x < 0.75) {
        return mix(c3, c4, (x - 0.5) / 0.25);
    }
    return mix(c4, c5, (x - 0.75) / 0.25);
}

// Height of the bump map and its slope along world X and Z, from three taps at a step of one texel.
fn bump_layer(uv: vec2<f32>) -> vec3<f32> {
    let e = 1.0 / 512.0;
    let h = textureSampleLevel(bump, bump_sampler, uv, 0.0).r;
    let hx = textureSampleLevel(bump, bump_sampler, uv + vec2<f32>(e, 0.0), 0.0).r;
    let hz = textureSampleLevel(bump, bump_sampler, uv + vec2<f32>(0.0, e), 0.0).r;
    return vec3<f32>(h, (hx - h) / e, (hz - h) / e);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let width = max(u32(globals.params.y), 1u);
    let state = textureLoad(fog_tex, vec2<i32>(i32(in.cell % width), i32(in.cell / width)), 0).r;

    let t = globals.params.w;
    let p = in.world_pos.xz;
    let a = bump_layer(p * 0.012 + vec2<f32>(t * 0.011, t * 0.004));
    let b = bump_layer(p * 0.0071 + vec2<f32>(-t * 0.006, t * 0.009));
    // Slope along the world axes: the slope per UV unit times the UV scale of the layer.
    let slope = (a.yz * 0.012 + b.yz * 0.0071) * BUMP_STRENGTH;
    let n = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));

    let depth = clamp(in.depth, 0.0, 1.0);
    let c = ramp(depth * RAMP_SCALE);
    // The ramp of the game is dull; a bluer tint keeps the water from reading as grey over sand.
    var rgb = mix(c.rgb, WATER_TINT * (1.0 - 0.55 * depth), TINT_MIX);
    // Opacity by depth: blending is in linear light, where dark water over bright sand turns pale
    // quickly, so even the coast is mostly water; the seabed still shows through. Less opaque
    // right at the land.
    var alpha = select(
        mix(0.76, 0.86, (depth - 0.45) / 0.15),
        mix(0.86, MAX_ALPHA, (depth - 0.6) / 0.4),
        depth >= 0.6
    );
    alpha = clamp(alpha, 0.5, MAX_ALPHA) * (1.0 - 0.12 * in.shore);

    // Light, sky reflection (Fresnel) and a glint of the sun.
    let view = normalize(globals.camera_pos.xyz - in.world_pos);
    let sun = normalize(SUN);
    let fresnel = 0.04 + 0.96 * pow(1.0 - max(dot(n, view), 0.0), 4.0);
    let half_vec = normalize(sun + view);
    let glint = pow(max(dot(n, half_vec), 0.0), 220.0);
    let ripple = (a.x + b.x) - 1.0;
    rgb = rgb * (1.0 + ripple * 0.35);
    rgb = mix(rgb, SKY, fresnel * 0.6);
    rgb = rgb + vec3<f32>(1.0, 0.95, 0.85) * glint * 0.5;
    alpha = max(alpha, min(glint, 1.0) * 0.5);

    // Foam on the shore, broken up by the ripples.
    let foam = smoothstep(0.8, 1.0, in.shore) * clamp(0.3 + ripple * 3.0, 0.0, 1.0);
    rgb = mix(rgb, vec3<f32>(0.95, 0.97, 0.98), foam * 0.22);

    if (state == 1u) {
        let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = mix(vec3<f32>(grey), rgb, HIDDEN_SATURATION) * globals.params.z;
    } else if (state == 0u) {
        // The terrain pass already painted the unrevealed cell with the fog colour.
        alpha = 0.0;
    }
    if (globals.params.x > 0.5) {
        rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(rgb, alpha);
}
