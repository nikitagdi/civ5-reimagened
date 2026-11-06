// The ground of the milestone M1 (world-terrain WT-0, WT-D7): the light of the original on the
// geometry of B1, in one pass. The fragment shader makes in place what the original bakes into the
// page of a cell and reads back from it:
//
//   1. "Terrain Cache": the light of the sun cube and the shadow cube of the continent style
//      times the albedo, gamma 1 / 2.2, the colour volume of the style (`common.wgsl`:
//      `terrain_cache_light`, `terrain_page_gamma`; the same functions are the body of
//      `TerrainRenderCacheDiffuse`, `bake_cache.wgsl`);
//   2. the end of the chain, the cached page on the screen: the shadow of the objects through the
//      shadow colour volume, the screen overlay, the fog of war, the distance fog (`common.wgsl`:
//      `terrain_cached_tail`, the same as in `TerrainRenderCachedDiffusePS`, `screen_ps.wgsl`).
//
// B1's geometry has no cells, no pages, no normal map and no macro shadow, so:
//   - the albedo is B1's texture of the slot (the same `slots` table and texture array as
//     `terrain.wgsl`), linear;
//   - the normal map is flat (`tn = (0, 0, 1)`), so the direction into the cubes is the normal of
//     the mesh in the original's basis; the macro shadow is 0 (the light of the sun, no shade);
//   - the screen overlay is empty (the overlay is B1's own lines, drawn over the ground);
//   - the fog of war mask comes from the fog state of B1's cells (below).
//
// This closes the terrain part of SIMP-025 (ambient and `FLAT_LIT` of `terrain.wgsl` are chosen,
// not the light of the original): with the boot group the ground has the light of the original.
// `terrain.wgsl` stays as the fallback for the assets of B1, which have no cubes. The textures and
// the geometry of the original come with WT-3/WT-4.
//
// Needs frame.wgsl, sm3.wgsl, gamma.wgsl and common.wgsl. Groups: 0 the frame (`frame_globals`,
// binding 1) and the light of the style of the draw (2..7 here, 8..15 in `common.wgsl`);
// 1 the object of the map copy (B1's `Object`); 2 the fog state of B1; 3 B1's material (the slots
// and the texture array). B1's vertex stays as it is (`TerrainGpuVertex`).

@group(0) @binding(2) var SunCubeLight: texture_cube<f32>;
@group(0) @binding(3) var SunCubeLight_smp: sampler;
@group(0) @binding(4) var ShadowCubeLight: texture_cube<f32>;
@group(0) @binding(5) var ShadowCubeLight_smp: sampler;
@group(0) @binding(6) var ColorCorrectTexture: texture_3d<f32>;
@group(0) @binding(7) var ColorCorrectTexture_smp: sampler;

// The map copy of B1 (`Object`): its model matrix shifts the copy along X (our basis), so the
// texture coordinates of the ground stay continuous across the seam.
struct M1Object {
    model: mat4x4<f32>,
    tint: vec4<f32>,
}
@group(1) @binding(0) var<uniform> m1_object: M1Object;

// Fog state per cell of B1: 0 unrevealed, 1 revealed and hidden, 2 visible.
@group(2) @binding(0) var m1_fog_tex: texture_2d<u32>;

struct M1Slot {
    // rgb: the flat colour (linear) of a slot without a layer.
    color: vec4<f32>,
    // x: the layer of the array, negative for none. y: texture scale (uv per world unit).
    tex: vec4<f32>,
}
struct M1Material {
    // rgb: the colour of unrevealed cells (linear).
    fog_color: vec4<f32>,
    slots: array<M1Slot, 64>,
}
@group(3) @binding(0) var<uniform> m1_material: M1Material;
@group(3) @binding(1) var m1_layers: texture_2d_array<f32>;
@group(3) @binding(2) var m1_layer_sampler: sampler;

struct M1VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) cell: u32,
    @location(3) slot: u32,
}

struct M1VertexOut {
    @builtin(position) clip: vec4<f32>,
    // The point and the normal in the original's basis.
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // The point in our basis (x, z), for the texture coordinates of B1.
    @location(2) ours_xz: vec2<f32>,
    // y and w of the clip position (the row of the fog ramp is y / w of the pixel, which the
    // perspective-correct interpolation of both gives exactly).
    @location(3) clip_yw: vec2<f32>,
    @location(4) @interpolate(flat) cell: u32,
    @location(5) @interpolate(flat) slot: u32,
}

@vertex
fn terrain_m1_vs(v: M1VertexIn) -> M1VertexOut {
    let ours = m1_object.model * vec4<f32>(v.position, 1.0);
    let orig = (frame_globals.ours_to_orig * vec4<f32>(ours.xyz, 1.0)).xyz;
    let clip = terrain_clip(orig);
    var out: M1VertexOut;
    out.clip = clip;
    out.world = orig;
    // The rotation of `ours_to_orig` only (w = 0).
    out.normal = (frame_globals.ours_to_orig * vec4<f32>((m1_object.model * vec4<f32>(v.normal, 0.0)).xyz, 0.0)).xyz;
    out.ours_xz = ours.xz;
    out.clip_yw = vec2<f32>(clip.y, clip.w);
    out.cell = v.cell;
    out.slot = v.slot;
    return out;
}

// The fog-of-war mask of the original (`FOWMaskCached`: x the ground is known, z it is seen) from
// the state of B1's cell: unrevealed (0, 0), revealed and hidden (1, 0), visible (1, 1).
// TODO(RW-8): the contract of the mask channels is `world-overlays`' (FowMask); this is the
// reading of `TerrainRenderCachedDiffusePS` (known and not seen -> `TransFOWColorCached`).
fn m1_fow_mask(state: u32) -> vec4<f32> {
    let known = select(0.0, 1.0, state >= 1u);
    let seen = select(0.0, 1.0, state >= 2u);
    return vec4<f32>(known, 0.0, seen, 0.0);
}

@fragment
fn terrain_m1_ps(in: M1VertexOut) -> @location(0) vec4<f32> {
    // Over a column cut by the mod (`cutout.wgsl`) there is no ground.
    if (cutout_hit(in.ours_xz.x, in.ours_xz.y)) {
        discard;
    }
    let width = max(textureDimensions(m1_fog_tex).x, 1u);
    let state = textureLoad(
        m1_fog_tex,
        vec2<i32>(i32(in.cell % width), i32(in.cell / width)),
        0,
    ).r;

    // The albedo: B1's texture of the slot, uv from the position in our basis. The gradients
    // come from the continuous position: the scale changes from cell to cell, and the derivatives of
    // the uv would jump along the seams and pick a blurry mip level there.
    let slot = m1_material.slots[in.slot];
    let scale = slot.tex.y;
    let uv = in.ours_xz * scale;
    let ddx = dpdx(in.ours_xz) * scale;
    let ddy = dpdy(in.ours_xz) * scale;
    let layer = max(i32(slot.tex.x), 0);
    let sampled = textureSampleGrad(m1_layers, m1_layer_sampler, uv, layer, ddx, ddy).rgb;
    let albedo = select(slot.color.rgb, sampled, slot.tex.x >= 0.0);

    // "Terrain Cache".
    let dir = normalize(in.normal);
    let sun = textureSample(SunCubeLight, SunCubeLight_smp, dir).xyz;
    let shade = textureSample(ShadowCubeLight, ShadowCubeLight_smp, dir).xyz;
    let lit = terrain_cache_light(sun, shade, 0.0, albedo);
    let page = textureSample(ColorCorrectTexture, ColorCorrectTexture_smp, terrain_page_gamma(lit)).xyz;

    // The cached page on the screen.
    let shadow_uv = terrain_shadow_uv(in.world, frame_globals.g_ShadowVP);
    let map = textureSample(DynamicShadowTexture, DynamicShadowTexture_smp, shadow_uv);
    let shadow = terrain_shadow_amount(map.x, frame_globals.g_ShadowFade);
    let fog = terrain_fog_factor(
        in.world - frame_globals.f3CameraWorld,
        frame_globals.g_FogConsts,
        in.clip_yw.x / in.clip_yw.y,
    );
    // The colour of the cells nobody has seen: B1's flat colour, as the gamma of the page.
    let fow_base = terrain_to_gamma(m1_material.fog_color.rgb);
    let color = terrain_cached_tail(
        page,
        shadow,
        vec4<f32>(0.0),
        fow_base,
        m1_fow_mask(state),
        fog,
        frame_globals.f3FogColor.xyz,
    );
    return vec4<f32>(color, 1.0);
}
