// "Macro Shadow" of the original (world-terrain WT-8): the shadow of the relief on the page of a cell,
// from the tile of heights. `TerrainShader::TerrainShaderShadowPS` marches a ray from the texel towards
// the sun over the heights (40 steps of one texel along the axis the sun leans to most) and finds out
// whether the ground rises over it; `TerrainShader::TerrainShaderScreenShadowVS` is the vertex stage of
// the full-page quad that gives it the texel. Several passes, each moved by a sub-texel of the jitter,
// add their share `fShadowJitterAdd.z` (one over their number) into `ShadowTemp`. Hand ports in the
// convention of shader-re 4.8.1: a body `fn <entry>_body(inp: In) -> Out` and an entry.
//
// Needs sm3.wgsl. Group 1: the tile of heights `HeightMap` (an A8 texture in the original, the red of
// an `R8Unorm` here, `maps/formats.toml`) and the constants of the pixel stage named by the CTAB.
// Group 2: the vertex stage's `g_TerrainScreenTexelOffsets` (the jitter in the clip space).

@group(1) @binding(0) var HeightMap: texture_2d<f32>;
@group(1) @binding(1) var HeightMap_smp: sampler;

// The constants of the pixel stage; the register of each is in its comment.
struct ShadowPsConsts {
    // c0: the world units of a texel (xy) and the height of raw 255 (z).
    g_WorldScales: vec3<f32>,
    // c1: the direction to the sun, in the original's basis.
    g_SunDir: vec3<f32>,
    // c6: the size of a texel of the tile.
    g_HeightTextureScale: vec2<f32>,
    // c21: the move of a texel of the field into the tile.
    g_f2WorldRebase: vec2<f32>,
    // c23: the share of this pass (z).
    fShadowJitterAdd: vec4<f32>,
}
@group(1) @binding(2) var<uniform> shadow_ps_consts: ShadowPsConsts;

struct ShadowVsConsts {
    // c35: the jitter of the pass in the clip space.
    g_TerrainScreenTexelOffsets: vec2<f32>,
    // Pads the block to 16 bytes (WebGL2 binds uniform blocks in multiples of 16).
    pad: vec2<f32>,
}
@group(2) @binding(0) var<uniform> shadow_vs_consts: ShadowVsConsts;

// ---- TerrainShaderScreenShadowVS ----

struct ShadowVsIn {
    // dcl_position v0: the corner of the page in the clip space (z is passed on).
    v0: vec4<f32>,
    // dcl_texcoord v1: the texel of the field at the corner.
    v1: vec4<f32>,
}

struct ShadowVsOut {
    // dcl_position o0, dcl_color o1 (zero), dcl_texcoord4 o2.xy, dcl_texcoord o3 .. dcl_texcoord3 o6 (zero).
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec4<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
    o6: vec4<f32>,
}

// re: TerrainShader::TerrainShaderScreenShadowVS (sha1 69b5ac81f756): `def c0 = (1, 0, 0, 0)`
fn terrain_shader_screen_shadow_vs_body(inp: ShadowVsIn) -> ShadowVsOut {
    var o: ShadowVsOut;
    o.o0 = vec4<f32>(
        shadow_vs_consts.g_TerrainScreenTexelOffsets + inp.v0.xy,
        inp.v0.z,
        1.0,
    );
    o.o1 = vec4<f32>(0.0);
    o.o2 = vec4<f32>(inp.v1.xy, 0.0, 0.0);
    o.o3 = vec4<f32>(0.0);
    o.o4 = vec4<f32>(0.0);
    o.o5 = vec4<f32>(0.0);
    o.o6 = vec4<f32>(0.0);
    return o;
}

struct ShadowVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec2<f32>,
}

@vertex
fn terrain_shader_screen_shadow_vs(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
) -> ShadowVertexOut {
    var b: ShadowVsIn;
    b.v0 = v0;
    b.v1 = v1;
    let r = terrain_shader_screen_shadow_vs_body(b);
    var out: ShadowVertexOut;
    out.clip = r.o0;
    out.v0 = r.o2.xy;
    return out;
}

// ---- TerrainShaderShadowPS ----

struct ShadowPsIn {
    // dcl_texcoord4 v0.xy: the texel of the field.
    v0: vec4<f32>,
}

struct ShadowPsOut {
    oc0: vec4<f32>,
}

// re: TerrainShader::TerrainShaderShadowPS (sha1 be270504c723): `def c2 = (255, 0, 0.314485461, 0)`,
// `def c3 = (1, 255, -64, 0.010416667)`, `defi i0 = (40, 0, 0, 0)`
fn terrain_shader_shadow_ps_body(inp: ShadowPsIn) -> ShadowPsOut {
    let c = shadow_ps_consts;
    let texel = inp.v0.xy;

    // The step: one texel along the axis the sun leans to most, the other in proportion.
    let sun_abs = abs(c.g_SunDir.xy);
    let lean = sun_abs.x - sun_abs.y;
    let inv_y = d3d_rcp(sun_abs.y);
    let by_y = vec2<f32>(inv_y * c.g_SunDir.x, inv_y * c.g_SunDir.y);
    let inv_x = d3d_rcp(sun_abs.x);
    let by_x = vec2<f32>(inv_x * c.g_SunDir.x, inv_x * c.g_SunDir.y);
    let step_texels = select(by_y, by_x, lean >= 0.0);

    // The height under the pixel, then the height the ray starts at.
    let uv0 = (c.g_f2WorldRebase + texel) * c.g_HeightTextureScale;
    let h0 = textureSample(HeightMap, HeightMap_smp, uv0).x;
    var ray = h0 * c.g_WorldScales.z;
    ray = ray * 255.0 + c.fShadowJitterAdd.z;
    let start = step_texels + texel;
    let step_world = step_texels * c.g_WorldScales.xy;
    let step_len = d3d_rcp(d3d_rsq(dot(step_world, step_world)));
    ray = step_len * 0.314485461 + ray;
    var uv = (start + c.g_f2WorldRebase) * c.g_HeightTextureScale;
    var lit = 1.0;
    for (var i = 0; i < 40; i = i + 1) {
        uv = step_texels * c.g_HeightTextureScale + uv;
        ray = step_len * 0.314485461 + ray;
        let h = textureSample(HeightMap, HeightMap_smp, uv).x;
        let ground = h * 255.0;
        // The ground fades out above the height 64 to 160 (of 255): the high peaks do not shade.
        let fade = 1.0 - saturate((h * 255.0 + -64.0) * 0.010416667);
        let gap = ground * -c.g_WorldScales.z + ray;
        let blocked = min(fade, lit);
        lit = select(blocked, lit, gap >= 0.0);
    }
    var o: ShadowPsOut;
    o.oc0 = vec4<f32>((1.0 - lit) * c.fShadowJitterAdd.z);
    return o;
}

@fragment
fn terrain_shader_shadow_ps(@location(0) v0: vec2<f32>) -> @location(0) vec4<f32> {
    var b: ShadowPsIn;
    b.v0 = vec4<f32>(v0, 0.0, 0.0);
    return terrain_shader_shadow_ps_body(b).oc0;
}
