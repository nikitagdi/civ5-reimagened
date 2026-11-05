// FXShader of the original: the effect models `FXM_*.gr2` (city damage smoke and fire, pillage smoke,
// reef caustics). The vertex shaders `FXVS` (rigid) and `FXSkinnedVS` (three bones a vertex), and the
// pixel shaders of one, two and three texture layers, `FXPS_CODE1..3`, whose code the skinned
// `FXSkinnedPS_CODE1..3` repeat under other names for the two constants (`fElapsedTimeS`,
// `fScrollCoordS`). `FXPS_CODE4/5` (four and five layers) are not ported: a material with more than
// three layers draws its first three.
//
// Hand port in the convention of shader-re 4.8.1: each entry of the original is a body
// `fn <entry>_body(inp: In) -> Out` with the registers of the original as member names, and an entry
// that reads the inputs of the instance, calls the body and finishes what the original did elsewhere.
//
// A layer of the pixel shader: the scroll map `UVScrollingMapN` (read at the uv) says by its four
// channels how the uv of the base texture moves in time (the `cmp` chain of `fx_scroll_uv`), the base
// `BaseTextureMapN` is read at the moved uv and squared (sRGB to linear, the shader's way), and the
// alpha is the product of two reads of `AlphaLookupMapN`: its alpha at the uv, and its red at
// `(fScrollCoordR, fScrollCoordR)` (a fade over the loop of the effect), both squared. The layers
// are put over each other (`lrp` and `mul` by what is left), and the shader writes the square roots:
// the colour is premultiplied by the alpha of the layers and `w` is what is left behind them.
//
// SIMPLIFIED(SIMP-048): `fElapsedTimeR` and `fScrollCoordR` are constants the original's code fills
// for each effect (the elapsed time of the loop and its fraction); here the instance carries them: the
// colour bytes of the `ObjectInstance` of an FX model are `r` = the phase of the loop in cycles
// (`r / 255`) and `g` = the loop time in 1/16 s (0 means 1.0 s, the `fScrollTime` default of
// `Civ5ArtDefines_VisEffectModels.xml`). `elapsed = fract(g_Time / scroll + phase) * scroll`,
// `scroll_coord = elapsed / scroll`. The vertex entries compute them and pass them to the pixel
// entries as a flat varying. The moment of the loop of an effect model is ours (the original's
// DrawVisEffect offsets it by rand).
//
// Groups: 0 the frame (binding 1 `frame_globals`), 1 the bones, 2 the fog (not read here: the pixel
// shaders of the original read no fog of war), 3 the material. The material group is that of the
// objects with the textures of the layers added: `BaseTextureMap0` 0 (sampler 1, which every read of
// the file uses), then `UVScrollingMap0` 6, `AlphaLookupMap0` 7, `BaseTextureMap1` 8,
// `UVScrollingMap1` 9, `AlphaLookupMap1` 10, `BaseTextureMap2` 11, `UVScrollingMap2` 12,
// `AlphaLookupMap2` 13.
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl and skin/common.wgsl. Vertex: POSITION0 0, NORMAL0 1 (oct
// snorm16), TEXCOORD0 2, BLENDINDICES0 14, BLENDWEIGHT0 15; the instance as in `simple.wgsl`
// (locations 3..8).

@group(3) @binding(0) var BaseTextureMap0: texture_2d<f32>;
@group(3) @binding(1) var BaseTextureMap0_smp: sampler;
@group(3) @binding(6) var UVScrollingMap0: texture_2d<f32>;
@group(3) @binding(7) var AlphaLookupMap0: texture_2d<f32>;
@group(3) @binding(8) var BaseTextureMap1: texture_2d<f32>;
@group(3) @binding(9) var UVScrollingMap1: texture_2d<f32>;
@group(3) @binding(10) var AlphaLookupMap1: texture_2d<f32>;
@group(3) @binding(11) var BaseTextureMap2: texture_2d<f32>;
@group(3) @binding(12) var UVScrollingMap2: texture_2d<f32>;
@group(3) @binding(13) var AlphaLookupMap2: texture_2d<f32>;

// ---- the vertex shaders ----

struct FxVsIn {
    // dcl_position v0: with its w, which the shader uses.
    v0: vec4<f32>,
    // dcl_texcoord v1: (u, v).
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    instance_world: mat4x4<f32>,
}

struct FxVsOut {
    o0: vec4<f32>,
    // The normal, and u in w.
    o1: vec4<f32>,
    // The vector from the eye to the point, and v in w.
    o2: vec4<f32>,
}

// re: FXShader::FXVS (sha1 218379a532f3)
fn fx_vs_body(inp: FxVsIn) -> FxVsOut {
    // c3 = ViewInverseMatrix+3, c4..c7 = ViewProjMatrix, c8..c11 = WorldMatrix.
    let c3 = skin_reg(frame_globals.ViewInverseMatrix, 3u);
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let c8 = skin_reg(inp.instance_world, 0u);
    let c9 = skin_reg(inp.instance_world, 1u);
    let c10 = skin_reg(inp.instance_world, 2u);
    let c11 = skin_reg(inp.instance_world, 3u);
    var o: FxVsOut;
    var r0 = c9.xyz * inp.v0.y;
    r0 = inp.v0.x * c8.xyz + r0;
    r0 = inp.v0.z * c10.xyz + r0;
    r0 = inp.v0.w * c11.xyz + r0;
    var r1 = r0.y * c5;
    r1 = r0.x * c4 + r1;
    r1 = r0.z * c6 + r1;
    let eye = r0 - c3.xyz;
    o.o0 = r1 + c7;
    var n = c9.xyz * inp.v2.y;
    n = inp.v2.x * c8.xyz + n;
    n = inp.v2.z * c10.xyz + n;
    o.o1 = vec4<f32>(n, inp.v1.x);
    o.o2 = vec4<f32>(eye, inp.v1.y);
    return o;
}

struct FxSkinnedVsIn {
    // dcl_position v0.
    v0: vec3<f32>,
    // dcl_texcoord v1: (u, v).
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    // dcl_blendweight v3: the weights of the first three bones.
    v3: vec3<f32>,
    // dcl_blendindices v4: the bones, as numbers.
    v4: vec3<f32>,
    // The palette `BoneMatrices` (c8..c103) is the bone texture (data move).
    bone_base: u32,
    bone_base_next: u32,
    bone_blend: f32,
}

// `dp3 (x, y, z), v, c8[a0..a0+2]`: the rows of the bone `a0 / 3` taken on a direction.
fn fx_bone_dir(v: vec3<f32>, base: u32, next: u32, blend: f32, a0: u32) -> vec3<f32> {
    return vec3<f32>(
        dot(v, palette_row(base, next, blend, a0).xyz),
        dot(v, palette_row(base, next, blend, a0 + 1u).xyz),
        dot(v, palette_row(base, next, blend, a0 + 2u).xyz),
    );
}

// `dp4 (x, y, z), p, c8[a0..a0+2]`: the same on a point (w = 1).
fn fx_bone_point(p: vec4<f32>, base: u32, next: u32, blend: f32, a0: u32) -> vec3<f32> {
    return vec3<f32>(
        dot(p, palette_row(base, next, blend, a0)),
        dot(p, palette_row(base, next, blend, a0 + 1u)),
        dot(p, palette_row(base, next, blend, a0 + 2u)),
    );
}

// re: FXShader::FXSkinnedVS (sha1 9384a34c0999)
fn fx_skinned_vs_body(inp: FxSkinnedVsIn) -> FxVsOut {
    // def c0, 1, 3, 0, 0: the sum of the weights is taken against 1, a bone is three registers.
    let one = 1.0;
    let three = 3.0;
    // c3 = ViewInverseMatrix+3, c4..c7 = ViewProjMatrix.
    let c3 = skin_reg(frame_globals.ViewInverseMatrix, 3u);
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let b = inp.bone_base;
    let n = inp.bone_base_next;
    let k = inp.bone_blend;

    // The weights are scaled to sum to one.
    let sum = dot(inp.v3, vec3<f32>(one));
    let w = d3d_rcp(sum) * inp.v3;
    let idx = three * inp.v4;
    let a0x = skin_mova(idx.x);
    let a0y = skin_mova(idx.y);
    let a0z = skin_mova(idx.z);

    var r1 = w.y * fx_bone_dir(inp.v2, b, n, k, a0y);
    var r2 = fx_bone_dir(inp.v2, b, n, k, a0x);
    r1 = w.x * r2 + r1;
    r2 = fx_bone_dir(inp.v2, b, n, k, a0z);
    let normal = skin_place_dir(w.z * r2 + r1);

    // `mad r1, v0.xyzx, c0.xxxz, c0.zzzx`: (x, y, z, 1).
    let p = vec4<f32>(inp.v0, 1.0);
    var q = w.y * fx_bone_point(p, b, n, k, a0y);
    var q2 = fx_bone_point(p, b, n, k, a0x);
    let q3 = w.x * q2 + q;
    q2 = fx_bone_point(p, b, n, k, a0z);
    let pos = skin_place_point(w.z * q2 + q3);

    var r3 = pos.y * c5;
    r3 = pos.x * c4 + r3;
    r3 = pos.z * c6 + r3;
    var o: FxVsOut;
    o.o2 = vec4<f32>(pos - c3.xyz, inp.v1.y);
    o.o0 = r3 + c7;
    o.o1 = vec4<f32>(normal, inp.v1.x);
    return o;
}

// ---- the pixel shaders ----

struct FxPsIn {
    // dcl_texcoord v0.w: u. dcl_texcoord1 v1.w: v.
    v0: vec4<f32>,
    v1: vec4<f32>,
    // `fElapsedTimeR` and `fScrollCoordR` (`...S` of the skinned ones), from the instance.
    instance_elapsed: f32,
    instance_scroll: f32,
}

struct FxPsOut {
    oc0: vec4<f32>,
}

// The uv of a layer's base texture at the moment `elapsed`: the channels of the scroll map `t` (read
// at the uv) pick, by the four `cmp` of the shader, which of the two speeds `-2 * (x - 0.5)` and
// `2 * (y - 0.5)` apply, and the zw of the same read are the directions.
fn fx_scroll_uv(t: vec4<f32>, uv: vec2<f32>, elapsed: f32) -> vec2<f32> {
    // def c0, -0.5, -0.498, -2, 0.002. def c1, -1, -0, 0, 1.
    let r2 = t.xyxy + vec4<f32>(-0.5, -0.5, -0.498, -0.498);
    var rz = select(-0.0, -1.0, r2.z >= 0.0);
    let r1xy = -r2.xy + 0.002;
    rz = select(-0.0, rz, r1xy.x >= 0.0);
    var rw = select(-0.0, -1.0, r1xy.y >= 0.0);
    rw = select(-0.0, rw, r2.w >= 0.0);
    let r1x = r2.x * -2.0;
    let r1y = r2.y + r2.y;
    let sel = vec2<f32>(select(0.0, r1x, rz >= 0.0), select(0.0, r1y, rw >= 0.0));
    let rzw = vec2<f32>(t.z, t.w) * sel;
    return rzw * elapsed + uv;
}

// The alpha of a layer: the product of the squares of the lookup's alpha at the uv and of its red at
// the scroll coordinate.
fn fx_layer_alpha(at_uv: vec4<f32>, at_scroll: vec4<f32>) -> f32 {
    let a = at_uv.w * at_uv.w;
    let b = at_scroll.x * at_scroll.x;
    return a * b;
}

fn fx_square(x: vec4<f32>) -> vec4<f32> {
    return x * x;
}

// re: FXShader::FXPS_CODE1 (sha1 fe2238051b8d)
fn fx_ps_code1_body(inp: FxPsIn) -> FxPsOut {
    let uv = vec2<f32>(inp.v0.w, inp.v1.w);
    let scroll_uv = vec2<f32>(inp.instance_scroll);
    let s0 = textureSample(UVScrollingMap0, BaseTextureMap0_smp, uv);
    let a0 = fx_layer_alpha(
        textureSample(AlphaLookupMap0, BaseTextureMap0_smp, uv),
        textureSample(AlphaLookupMap0, BaseTextureMap0_smp, scroll_uv),
    );
    let b0 = fx_square(textureSample(BaseTextureMap0, BaseTextureMap0_smp, fx_scroll_uv(s0, uv, inp.instance_elapsed)));
    let p0 = a0 * b0.w;
    let q0 = b0.w * -a0 + 1.0;
    let c = p0 * b0.xyz;
    var out: FxPsOut;
    out.oc0 = vec4<f32>(
        d3d_rcp(d3d_rsq(c.x)),
        d3d_rcp(d3d_rsq(c.y)),
        d3d_rcp(d3d_rsq(c.z)),
        d3d_rcp(d3d_rsq(q0)),
    );
    return out;
}

// re: FXShader::FXPS_CODE2 (sha1 6cc19a163f33)
fn fx_ps_code2_body(inp: FxPsIn) -> FxPsOut {
    let uv = vec2<f32>(inp.v0.w, inp.v1.w);
    let scroll_uv = vec2<f32>(inp.instance_scroll);
    let s0 = textureSample(UVScrollingMap0, BaseTextureMap0_smp, uv);
    let b0 = fx_square(textureSample(BaseTextureMap0, BaseTextureMap0_smp, fx_scroll_uv(s0, uv, inp.instance_elapsed)));
    let a0 = fx_layer_alpha(
        textureSample(AlphaLookupMap0, BaseTextureMap0_smp, uv),
        textureSample(AlphaLookupMap0, BaseTextureMap0_smp, scroll_uv),
    );
    let p0 = a0 * b0.w;
    let q0 = b0.w * -a0 + 1.0;
    let c0 = p0 * b0.xyz;

    let s1 = textureSample(UVScrollingMap1, BaseTextureMap0_smp, uv);
    let a1 = fx_layer_alpha(
        textureSample(AlphaLookupMap1, BaseTextureMap0_smp, uv),
        textureSample(AlphaLookupMap1, BaseTextureMap0_smp, scroll_uv),
    );
    let b1 = fx_square(textureSample(BaseTextureMap1, BaseTextureMap0_smp, fx_scroll_uv(s1, uv, inp.instance_elapsed)));
    let p1 = a1 * b1.w;
    let q1 = b1.w * -a1 + 1.0;
    // `lrp r3.xyz, p1, b1, c0`.
    let c = p1 * (b1.xyz - c0) + c0;
    let q = q0 * q1;
    var out: FxPsOut;
    out.oc0 = vec4<f32>(
        d3d_rcp(d3d_rsq(c.x)),
        d3d_rcp(d3d_rsq(c.y)),
        d3d_rcp(d3d_rsq(c.z)),
        d3d_rcp(d3d_rsq(q)),
    );
    return out;
}

// re: FXShader::FXPS_CODE3 (sha1 b5ed4495c42c)
fn fx_ps_code3_body(inp: FxPsIn) -> FxPsOut {
    let uv = vec2<f32>(inp.v0.w, inp.v1.w);
    let scroll_uv = vec2<f32>(inp.instance_scroll);
    let s0 = textureSample(UVScrollingMap0, BaseTextureMap0_smp, uv);
    let b0 = fx_square(textureSample(BaseTextureMap0, BaseTextureMap0_smp, fx_scroll_uv(s0, uv, inp.instance_elapsed)));
    let a0 = fx_layer_alpha(
        textureSample(AlphaLookupMap0, BaseTextureMap0_smp, uv),
        textureSample(AlphaLookupMap0, BaseTextureMap0_smp, scroll_uv),
    );
    let p0 = a0 * b0.w;
    let q0 = b0.w * -a0 + 1.0;
    let c0 = p0 * b0.xyz;

    let s1 = textureSample(UVScrollingMap1, BaseTextureMap0_smp, uv);
    let b1 = fx_square(textureSample(BaseTextureMap1, BaseTextureMap0_smp, fx_scroll_uv(s1, uv, inp.instance_elapsed)));
    let a1 = fx_layer_alpha(
        textureSample(AlphaLookupMap1, BaseTextureMap0_smp, uv),
        textureSample(AlphaLookupMap1, BaseTextureMap0_smp, scroll_uv),
    );
    let q1 = b1.w * -a1 + 1.0;
    let p1 = a1 * b1.w;
    let c1 = p1 * b1.xyz;
    let c01 = q1 * c0;

    let s2 = textureSample(UVScrollingMap2, BaseTextureMap0_smp, uv);
    let b2 = fx_square(textureSample(BaseTextureMap2, BaseTextureMap0_smp, fx_scroll_uv(s2, uv, inp.instance_elapsed)));
    let a2 = fx_layer_alpha(
        textureSample(AlphaLookupMap2, BaseTextureMap0_smp, uv),
        textureSample(AlphaLookupMap2, BaseTextureMap0_smp, scroll_uv),
    );
    let q2 = b2.w * -a2 + 1.0;
    let p2 = a2 * b2.w;
    let c1q = q2 * c1;
    let c012 = c01 * q2 + c1q;
    let q = q0 * (q1 * q2);
    let c = b2.xyz * p2 + c012;
    var out: FxPsOut;
    out.oc0 = vec4<f32>(
        d3d_rcp(d3d_rsq(c.x)),
        d3d_rcp(d3d_rsq(c.y)),
        d3d_rcp(d3d_rsq(c.z)),
        d3d_rcp(d3d_rsq(q)),
    );
    return out;
}

// The skinned pixel shaders are the same code with the constants named `...S`.

// re: FXShader::FXSkinnedPS_CODE1 (sha1 388844f625d5)
fn fx_skinned_ps_code1_body(inp: FxPsIn) -> FxPsOut {
    return fx_ps_code1_body(inp);
}

// re: FXShader::FXSkinnedPS_CODE2 (sha1 04c982a7fda1)
fn fx_skinned_ps_code2_body(inp: FxPsIn) -> FxPsOut {
    return fx_ps_code2_body(inp);
}

// re: FXShader::FXSkinnedPS_CODE3 (sha1 030df40b1377)
fn fx_skinned_ps_code3_body(inp: FxPsIn) -> FxPsOut {
    return fx_ps_code3_body(inp);
}

// ---- entries: the instance of world-objects ----

struct FxVertexIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    // The phase (r) and the loop time (g) of the effect; see the head of the file.
    @location(6) color: vec4<f32>,
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

struct FxSkinnedVertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(14) joints: vec4<u32>,
    @location(15) weights: vec4<f32>,
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    @location(6) color: vec4<f32>,
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

struct FxVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    // (fElapsedTimeR, fScrollCoordR): one value for the whole instance.
    @location(2) @interpolate(flat) timing: vec2<f32>,
}

// The texture coordinate of the original: the pipeline stores `1 - v` (UV_FLIP_V) while the layer
// textures are the DDS rows as the game binds them and the vertex buffers of the game carry the gr2's
// `v` unflipped (as `unit_texcoord` and the building entries). Unflipped, every layer read the
// mirrored texture and the scroll of `fx_scroll_uv`, which adds to `v`, ran backwards on it: the
// pillaged smoke and the city fire played in reverse (user report fb4).
fn fx_texcoord(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(uv.x, 1.0 - uv.y);
}

// The member form of `WorldMatrix`: the register `8 + k` is `skin_reg(m, k)`, so the member is the
// rows of the 3x4 matrix with the row (0, 0, 0, 1) under them.
fn fx_world_regs(w: mat3x4<f32>) -> mat4x4<f32> {
    return mat4x4<f32>(w[0], w[1], w[2], vec4<f32>(0.0, 0.0, 0.0, 1.0));
}

// `fElapsedTimeR` and `fScrollCoordR` of an instance from its colour bytes.
fn fx_timing(color: vec4<f32>) -> vec2<f32> {
    let ticks = round(color.g * 255.0);
    let scroll = select(ticks / 16.0, 1.0, ticks == 0.0);
    let elapsed = fract(frame_globals.g_Time / scroll + color.r) * scroll;
    return vec2<f32>(elapsed, elapsed / scroll);
}

@vertex
fn fx_vs(v: FxVertexIn) -> FxVertexOut {
    var b: FxVsIn;
    b.v0 = vec4<f32>(v.position, 1.0);
    b.v1 = fx_texcoord(v.uv);
    b.v2 = skin_oct_decode(v.normal_oct);
    b.instance_world = fx_world_regs(
        rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend)),
    );
    let r = fx_vs_body(b);
    var out: FxVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.timing = fx_timing(v.color);
    return out;
}

// The clock of the god ray: the instance carries the moment of the last highlight event as the four
// bytes of the f32 `g_Time` it was made from (little endian), so that the elapsed time can grow without
// the wrap of the loop of the other effects. `fElapsedTimeR` is `m_fHighlightTime`, the seconds since the
// event, and `fScrollCoordR` is `m_fHighlightTime / m_fHighlightPeriod` with the period 1.0; neither is
// wrapped or clamped in the exe.
// re: re-b11-answers.txt 4 (cvMovementPathVisSystem::RenderInternal 0x088c059c, constant writer 0x088c2898)
// SIMPLIFIED(SIMP-exe-1030): the two constants are taken from the dynamic block by register order.
fn fx_god_ray_timing(color: vec4<f32>) -> vec2<f32> {
    let b = vec4<u32>(round(color * 255.0));
    let started = bitcast<f32>(b.x | (b.y << 8u) | (b.z << 16u) | (b.w << 24u));
    let elapsed = max(frame_globals.g_Time - started, 0.0);
    return vec2<f32>(elapsed, elapsed);
}

@vertex
fn fx_god_ray_vs(v: FxVertexIn) -> FxVertexOut {
    var b: FxVsIn;
    b.v0 = vec4<f32>(v.position, 1.0);
    b.v1 = fx_texcoord(v.uv);
    b.v2 = skin_oct_decode(v.normal_oct);
    b.instance_world = fx_world_regs(
        rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend)),
    );
    let r = fx_vs_body(b);
    var out: FxVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.timing = fx_god_ray_timing(v.color);
    return out;
}

@vertex
fn fx_skinned_vs(v: FxSkinnedVertexIn) -> FxVertexOut {
    var b: FxSkinnedVsIn;
    b.v0 = v.position;
    b.v1 = fx_texcoord(v.uv);
    b.v2 = skin_oct_decode(v.normal_oct);
    b.v3 = v.weights.xyz;
    b.v4 = vec3<f32>(v.joints.xyz);
    let bone_p = bone_pose(v.frames, v.blend);
    b.bone_base = bone_p.base;
    b.bone_base_next = bone_p.next;
    b.bone_blend = bone_p.blend;
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    let r = fx_skinned_vs_body(b);
    var out: FxVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.timing = fx_timing(v.color);
    return out;
}

fn fx_ps_in(in: FxVertexOut) -> FxPsIn {
    var b: FxPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.instance_elapsed = in.timing.x;
    b.instance_scroll = in.timing.y;
    return b;
}

@fragment
fn fx_ps_code1(in: FxVertexOut) -> @location(0) vec4<f32> {
    return fx_ps_code1_body(fx_ps_in(in)).oc0;
}

@fragment
fn fx_ps_code2(in: FxVertexOut) -> @location(0) vec4<f32> {
    return fx_ps_code2_body(fx_ps_in(in)).oc0;
}

@fragment
fn fx_ps_code3(in: FxVertexOut) -> @location(0) vec4<f32> {
    return fx_ps_code3_body(fx_ps_in(in)).oc0;
}
