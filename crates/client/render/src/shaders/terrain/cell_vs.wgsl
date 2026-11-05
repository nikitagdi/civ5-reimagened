// The vertex stage of the cells on the screen (world-terrain WT-4): `TerrainShader::TerrainShaderVS`.
// The lattice of a cell gives the texel `v0.xy` inside the cell; the height comes from the tile of
// the displacement map (`R8Unorm`, read in the vertex stage), the point goes through `World` and the
// view. Hand port in the convention of shader-re 4.8.1 (a body `fn <entry>_body(inp: In) -> Out` and a
// `@vertex` entry that only reads its inputs).
//
// Needs sm3.wgsl. Group 2: the tile of heights of the cell (`DisplacementMap`) and the constants of
// the stage, named like the CTAB. `Offset` is not in the block: it is the origin of the cell, which the
// original set before each draw and we take from the instance (data move `instance:offset`).
//
// Gamma and light are not here: the page of the cell holds them (`screen_ps.wgsl`).

// The height of the tile: the alpha of an A8 texture in the original, the red of an `R8Unorm` here
// (`maps/formats.toml`).
@group(2) @binding(0) var DisplacementMap: texture_2d<f32>;
@group(2) @binding(1) var DisplacementMap_smp: sampler;

// The constants of the vertex stage. Matrices are `row_major float4x4`: register `k` of one is the
// vector `(m[0][k], m[1][k], m[2][k], m[3][k])` (`cell_vs_reg`), the layout of `FrameGlobals`.
struct CellVsConsts {
    // c0..c3: the view and projection, in the original's basis.
    mtxViewProj: mat4x4<f32>,
    // c4..c7: the world to the place in the maps of the screen (overlay, fog of war mask).
    mtxCompositeVP: mat4x4<f32>,
    // c35..c38: the move of the cell's point into the world (the middle of the map, the copy).
    World: mat4x4<f32>,
    // c9: the scale (xy) and the move (zw) of the point into the base layer of the fog of war.
    g_FOWBaseScale: vec4<f32>,
    // c22: the size of a texel of the tile.
    g_HeightTextureScale: vec2<f32>,
    // c16: the world units of a texel (xy) and the height of raw 255 (z).
    g_WorldScales: vec3<f32>,
}
@group(2) @binding(2) var<uniform> cell_vs_consts: CellVsConsts;

// Register `k` of a `row_major float4x4` filled by CTAB name.
fn cell_vs_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

struct CellVsIn {
    // dcl_position v0: the texel of the lattice inside the cell (xy).
    v0: vec4<f32>,
    // dcl_normal v1: the normal of the lattice vertex, passed on.
    v1: vec3<f32>,
    // dcl_texcoord v2: the place in the page of the cell.
    v2: vec2<f32>,
    // The origin of the cell in the field (xy) and the move of the field into the tile (zw), the
    // constant `Offset` of the original (data move `instance:offset`).
    instance_offset: vec4<f32>,
}

struct CellVsOut {
    // dcl_position o0, dcl_normal o1.xyz, dcl_texcoord o2 (the place in the page and in the base of the
    // fog of war), dcl_texcoord1 o3 (the place in the maps of the screen), dcl_texcoord2 o4.xyz (the
    // point in the world), dcl_texcoord3 o5.xy (the clip position divided by w).
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec4<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
}

// re: TerrainShader::TerrainShaderVS (sha1 dba830761636): `def c8 = (0.5, 0, 255, 0)`,
// `def c10 = (0.5, -0.5, 1, 0)`
fn terrain_shader_vs_body(inp: CellVsIn) -> CellVsOut {
    let c = cell_vs_consts;
    let offset = inp.instance_offset;

    // The texel in the field, and in the tile; the point in the world units.
    var r0_xy = offset.xy + inp.v0.xy;
    var r0_zw = r0_xy + offset.zw;
    r0_xy = r0_xy * c.g_WorldScales.xy;
    r0_zw = r0_zw + vec2<f32>(0.5);

    // The height of the tile at the middle of the texel (`texldl`, level 0).
    let uv = r0_zw * c.g_HeightTextureScale;
    let tile = textureSampleLevel(DisplacementMap, DisplacementMap_smp, uv, 0.0);
    var z = tile.x * c.g_WorldScales.z;
    z = z * 255.0;

    // The point through `World` (c35..c38).
    let w0 = cell_vs_reg(c.World, 0u);
    let w1 = cell_vs_reg(c.World, 1u);
    let w2 = cell_vs_reg(c.World, 2u);
    let w3 = cell_vs_reg(c.World, 3u);
    var p = r0_xy.y * w1;
    p = r0_xy.x * w0 + p;
    p = z * w2 + p;
    p = p + w3;

    let base = (p.xy + c.g_FOWBaseScale.zw) * c.g_FOWBaseScale.xy;

    // The clip position (c0..c3).
    let v0 = cell_vs_reg(c.mtxViewProj, 0u);
    let v1 = cell_vs_reg(c.mtxViewProj, 1u);
    let v2 = cell_vs_reg(c.mtxViewProj, 2u);
    let v3 = cell_vs_reg(c.mtxViewProj, 3u);
    var clip = p.y * v1;
    clip = p.x * v0 + clip;
    clip = p.z * v2 + clip;
    clip = p.w * v3 + clip;
    let inv_w = d3d_rcp(clip.w);

    // The place in the maps of the screen (c4..c7).
    let m0 = cell_vs_reg(c.mtxCompositeVP, 0u);
    let m1 = cell_vs_reg(c.mtxCompositeVP, 1u);
    let m2 = cell_vs_reg(c.mtxCompositeVP, 2u);
    let m3 = cell_vs_reg(c.mtxCompositeVP, 3u);
    var comp = p.y * m1;
    comp = p.x * m0 + comp;
    comp = p.z * m2 + comp;
    comp = p.w * m3 + comp;

    var o: CellVsOut;
    o.o0 = clip;
    o.o1 = vec4<f32>(inp.v1, 0.0);
    o.o2 = vec4<f32>(inp.v2, base);
    o.o3 = comp * vec4<f32>(0.5, -0.5, 1.0, 1.0) + vec4<f32>(0.5, 0.5, 0.0, 0.0);
    o.o4 = vec4<f32>(p.xyz, 0.0);
    o.o5 = vec4<f32>(clip.xy * inv_w, 0.0, 0.0);
    return o;
}

// ---- entry: the interface of the vertex stage of the cached terrain (`screen_ps.wgsl`) ----

struct CellVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec2<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec2<f32>,
}

@vertex
fn terrain_shader_vs(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec3<f32>,
    @location(2) v2: vec2<f32>,
    @location(3) offset: vec4<f32>,
) -> CellVertexOut {
    var b: CellVsIn;
    b.v0 = v0;
    b.v1 = v1;
    b.v2 = v2;
    b.instance_offset = offset;
    let r = terrain_shader_vs_body(b);
    var out: CellVertexOut;
    out.clip = r.o0;
    out.v0 = r.o2;
    out.v1 = r.o3.xy;
    out.v2 = r.o4.xyz;
    out.v3 = r.o5.xy;
    return out;
}
