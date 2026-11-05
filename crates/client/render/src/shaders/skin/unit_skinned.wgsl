// UnitShader, the skinned entries of the original: `UnitSkinnedVS`,
// `UnitSkinnedVS_Shadow`, and the pixel shaders `UnitSkinnedPS` and `UnitSkinnedAlphaPS`, which are
// the code of `UnitPS` and `UnitAlphaPS` under another name for the team colour constant
// (`TeamColorS`), so they call the bodies of `unit/unit.wgsl`.
//
// Hand port in the convention of shader-re 4.8.1: each entry of the original is a body
// `fn <entry>_body(inp: In) -> Out` with the registers of the original as member names, and a
// `@vertex` / `@fragment` entry that reads the inputs of the instance, calls the body and finishes
// what the original did elsewhere (the world matrix, put on the vertex afterwards).
//
// The formula is the original's. What changes is the source of the matrices (data move
// `bone_texture`): `c8[a0.x]` with `a0.x = 3 * index` becomes the texel `3 * index` of the frame the
// instance names (`bone_base`, `bone_base_next`, `bone_blend`), and the world matrix of the
// instance is put on the skinned position and normal at the end (`skin_place_point`).
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl (render-core), skin/common.wgsl and unit/unit.wgsl.
// Vertex: POSITION0 0, NORMAL0 1 (oct snorm16), TEXCOORD0 2, BLENDINDICES0 14 (the bones of the
// skeleton, u8x4 or u16x4), BLENDWEIGHT0 15 (unorm8x4). Instance (`ObjectInstance`, 64 bytes): the
// free TEXCOORD locations 3..8 (the mesh of a skinned unit has no TEXCOORD1): three rows of the
// matrix, the colour of the team and the alpha, the two frames, the blend.
//
// The fog of war is in the pixel entries (`unit_skinned_ps`, `unit_skinned_alpha_ps`), after the
// body: the model is not drawn where `FowMask` says the ground is not revealed and never dimmed (the
// unit shaders of the original have no fog sampler; a unit whose hex stops being seen fades out
// instead). The bodies are the original's.
// re: cvUnitVisSystem::UpdateFromHandle(cvUnitVis_State) 0x089353fc (alpha 1 -> 0 when the hex is no
// longer visible), shader-re UnitShader::UnitPS (no FOW sampler) (RE-B18 item 3 FOG-1)
// SIMPLIFIED(SIMP-exe-1212): the fade over the unit's fade time (cvUnitVis +0x48) when its hex stops
// being visible is not reproduced: a unit the view no longer has is gone at once.

// ---- UnitSkinnedVS ----

struct UnitSkinnedVsIn {
    // dcl_position v0: the object-space position (x, y, z).
    v0: vec3<f32>,
    // dcl_blendweight v1: the weights of the first three bones (the fourth is not read).
    v1: vec3<f32>,
    // dcl_blendindices v2: the bones, as numbers.
    v2: vec3<f32>,
    // dcl_normal v3.
    v3: vec3<f32>,
    // dcl_texcoord v4: (u, v).
    v4: vec2<f32>,
    // The palette `BoneMatrices` (c8..c103) is the bone texture (data move): the frame the
    // instance names and the blend between it and the next.
    bone_base: u32,
    bone_base_next: u32,
    bone_blend: f32,
}

struct UnitSkinnedVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec3<f32>,
    o4: vec3<f32>,
}

// re: UnitShader::UnitSkinnedVS (sha1 d2b2d4ddbb1a)
fn unit_skinned_vs_body(inp: UnitSkinnedVsIn) -> UnitSkinnedVsOut {
    // def c0, 3, 1, 0, 0: the index of a bone times the three registers of a matrix.
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

    let r0 = vec4<f32>(inp.v0, 1.0);

    // The first bone.
    var a0 = skin_mova(three * inp.v2.x);
    var m0 = palette_row(b, n, k, a0);
    var m1 = palette_row(b, n, k, a0 + 1u);
    var m2 = palette_row(b, n, k, a0 + 2u);
    var r1 = vec3<f32>(dot(r0, m0), dot(r0, m1), dot(r0, m2));
    r1 = r1 * inp.v1.x;
    var r2 = vec3<f32>(dot(inp.v3, m0.xyz), dot(inp.v3, m1.xyz), dot(inp.v3, m2.xyz));
    r2 = r2 * inp.v1.x;
    if (0.0 < inp.v1.y) {
        // The second.
        a0 = skin_mova(three * inp.v2.y);
        m0 = palette_row(b, n, k, a0);
        m1 = palette_row(b, n, k, a0 + 1u);
        m2 = palette_row(b, n, k, a0 + 2u);
        var r3 = vec3<f32>(dot(r0, m0), dot(r0, m1), dot(r0, m2));
        r1 = inp.v1.y * r3 + r1;
        r3 = vec3<f32>(dot(inp.v3, m0.xyz), dot(inp.v3, m1.xyz), dot(inp.v3, m2.xyz));
        r2 = inp.v1.y * r3 + r2;
        if (0.0 < inp.v1.z) {
            // The third.
            a0 = skin_mova(three * inp.v2.z);
            m0 = palette_row(b, n, k, a0);
            m1 = palette_row(b, n, k, a0 + 1u);
            m2 = palette_row(b, n, k, a0 + 2u);
            r3 = vec3<f32>(dot(r0, m0), dot(r0, m1), dot(r0, m2));
            r1 = inp.v1.z * r3 + r1;
            r3 = vec3<f32>(dot(inp.v3, m0.xyz), dot(inp.v3, m1.xyz), dot(inp.v3, m2.xyz));
            r2 = inp.v1.z * r3 + r2;
        }
    }
    // The world matrix of the instance (the identity when the palette has it inside, as in the
    // comparison with the oracle).
    r1 = skin_place_point(r1);
    r2 = skin_place_dir(r2);

    var p = r1.y * c5;
    p = r1.x * c4 + p;
    p = r1.z * c6 + p;
    p = p + c7;
    var o: UnitSkinnedVsOut;
    o.o2 = vec4<f32>(r1 - c3.xyz, inp.v4.y);
    o.o0 = p;
    o.o1 = vec4<f32>(r2, inp.v4.x);
    o.o3 = r1;
    o.o4 = p.xyw;
    return o;
}

// ---- UnitSkinnedVS_Shadow ----

struct UnitSkinnedShadowVsIn {
    // dcl_position v0, dcl_blendweight v1, dcl_blendindices v2.
    v0: vec3<f32>,
    v1: vec3<f32>,
    v2: vec3<f32>,
    // dcl_texcoord v3: (u, v).
    v3: vec2<f32>,
    bone_base: u32,
    bone_base_next: u32,
    bone_blend: f32,
}

struct UnitSkinnedShadowVsOut {
    o0: vec4<f32>,
    o1: vec3<f32>,
}

// re: UnitShader::UnitSkinnedVS_Shadow (sha1 6585865a75c4)
fn unit_skinned_shadow_vs_body(inp: UnitSkinnedShadowVsIn) -> UnitSkinnedShadowVsOut {
    let three = 3.0;
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let b = inp.bone_base;
    let n = inp.bone_base_next;
    let k = inp.bone_blend;

    let r0 = vec4<f32>(inp.v0, 1.0);
    var a0 = skin_mova(three * inp.v2.x);
    var r1 = vec3<f32>(
        dot(r0, palette_row(b, n, k, a0)),
        dot(r0, palette_row(b, n, k, a0 + 1u)),
        dot(r0, palette_row(b, n, k, a0 + 2u)),
    );
    r1 = r1 * inp.v1.x;
    if (0.0 < inp.v1.y) {
        a0 = skin_mova(three * inp.v2.y);
        var r2 = vec3<f32>(
            dot(r0, palette_row(b, n, k, a0)),
            dot(r0, palette_row(b, n, k, a0 + 1u)),
            dot(r0, palette_row(b, n, k, a0 + 2u)),
        );
        r1 = inp.v1.y * r2 + r1;
        if (0.0 < inp.v1.z) {
            a0 = skin_mova(three * inp.v2.z);
            r2 = vec3<f32>(
                dot(r0, palette_row(b, n, k, a0)),
                dot(r0, palette_row(b, n, k, a0 + 1u)),
                dot(r0, palette_row(b, n, k, a0 + 2u)),
            );
            r1 = inp.v1.z * r2 + r1;
        }
    }
    r1 = skin_place_point(r1);

    var p = r1.y * c5;
    p = r1.x * c4 + p;
    p = r1.z * c6 + p;
    var o: UnitSkinnedShadowVsOut;
    o.o0 = p + c7;
    // `mov r1.xy, v3; mov o1.xyz, r1`: the texture coordinates and the height of the point.
    o.o1 = vec3<f32>(inp.v3, r1.z);
    return o;
}

// ---- UnitSkinnedPS, UnitSkinnedAlphaPS ----

// re: UnitShader::UnitSkinnedPS (sha1 2f3587c3459d)
// The code of `UnitPS`; the original names the team colour `TeamColorS` here (data move
// `instance:team_color`, as there).
fn unit_skinned_ps_body(inp: UnitPsIn) -> UnitPsOut {
    return unit_ps_body(inp);
}

// re: UnitShader::UnitSkinnedAlphaPS (sha1 7f4640bb3ad1)
// The same bytes as `UnitAlphaPS`.
fn unit_skinned_alpha_ps_body(inp: UnitAlphaPsIn) -> UnitPsOut {
    return unit_alpha_ps_body(inp);
}

// ---- entries: the instance of world-objects ----

struct UnitSkinnedVertexIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(14) joints: vec4<u32>,
    @location(15) weights: vec4<f32>,
    // The three rows of the matrix of the object, our basis.
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    // The colour of the team and the alpha, unorm8x4.
    @location(6) color: vec4<f32>,
    // The frames: the first bone of this one and of the next.
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

struct UnitSkinnedVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec3<f32>,
    @location(4) @interpolate(flat) color: vec4<f32>,
}

// The texture coordinate of the original from the one of the blob. The pipeline stores `1 - v`
// (`UV_FLIP_V` of `asset-pipeline`), but the textures of `assets-b2` are the rows of the DDS files
// as the game binds them: S1a has the same mips byte for byte and `v` unflipped in its vertex
// buffers (ev 2095, `euro_donkey`), so the original's `v` is `1 - v` of the blob.
fn unit_texcoord(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(uv.x, 1.0 - uv.y);
}

@vertex
fn unit_skinned_vs(v: UnitSkinnedVertexIn) -> UnitSkinnedVertexOut {
    var b: UnitSkinnedVsIn;
    b.v0 = v.position;
    b.v1 = v.weights.xyz;
    b.v2 = vec3<f32>(v.joints.xyz);
    b.v3 = skin_oct_decode(v.normal_oct);
    b.v4 = unit_texcoord(v.uv);
    let bone_p = bone_pose(v.frames, v.blend);
    b.bone_base = bone_p.base;
    b.bone_base_next = bone_p.next;
    b.bone_blend = bone_p.blend;
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    let r = unit_skinned_vs_body(b);
    var out: UnitSkinnedVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.v2 = r.o3;
    out.v3 = r.o4;
    out.color = v.color;
    return out;
}

struct UnitSkinnedShadowVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec3<f32>,
    // The alpha of the instance, for the caster `UnitSkinnedPS_CODE_Shadow` (`unit/unit_code_world.wgsl`).
    @location(1) @interpolate(flat) alpha: f32,
}

@vertex
fn unit_skinned_shadow_vs(v: UnitSkinnedVertexIn) -> UnitSkinnedShadowVertexOut {
    var b: UnitSkinnedShadowVsIn;
    b.v0 = v.position;
    b.v1 = v.weights.xyz;
    b.v2 = vec3<f32>(v.joints.xyz);
    b.v3 = unit_texcoord(v.uv);
    let bone_p = bone_pose(v.frames, v.blend);
    b.bone_base = bone_p.base;
    b.bone_base_next = bone_p.next;
    b.bone_blend = bone_p.blend;
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    let r = unit_skinned_shadow_vs_body(b);
    var out: UnitSkinnedShadowVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.alpha = v.color.w;
    return out;
}

// The raw shadow map keeps a mark where something stands (alpha 1), as `shadow/caster.wgsl` does.
// Ours, for the rigid casters (`UnitPS_CODE_Shadow` is not ported, no call in S1-S4); the skinned
// ones draw with the original's `UnitSkinnedPS_CODE_Shadow` (`unit_code_shadow_ps`).
@fragment
fn unit_shadow_ps() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

// `unit_skinned_ps`, and `unit_skinned_reflect_ps` that also drops what lies below the water (the
// reflection pass; the original cut with a clip plane).
fn unit_skinned_ps_impl(in: UnitSkinnedVertexOut, reflection: bool) -> vec4<f32> {
    var b: UnitPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    // The colour of the team as the instance gives it: gamma values 0..1.
    b.instance_team_color = vec4<f32>(in.color.xyz, 1.0);
    let r = unit_skinned_ps_body(b);
    let m = skin_fow_mask(in.v2.xy);
    if (m.x < FOW_UNREVEALED || (reflection && reflection_clipped(in.v2))) {
        discard;
    }
    return vec4<f32>(r.oc0.xyz, 1.0);
}

@fragment
fn unit_skinned_ps(in: UnitSkinnedVertexOut) -> @location(0) vec4<f32> {
    return unit_skinned_ps_impl(in, false);
}

@fragment
fn unit_skinned_reflect_ps(in: UnitSkinnedVertexOut) -> @location(0) vec4<f32> {
    return unit_skinned_ps_impl(in, true);
}

fn unit_skinned_alpha_ps_impl(in: UnitSkinnedVertexOut, reflection: bool) -> vec4<f32> {
    var b: UnitAlphaPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    let r = unit_skinned_alpha_ps_body(b);
    let m = skin_fow_mask(in.v2.xy);
    if (m.x < FOW_UNREVEALED || (reflection && reflection_clipped(in.v2))) {
        discard;
    }
    // The alpha of the instance (`SetAlphaValues`) multiplies the alpha of the texture.
    return vec4<f32>(r.oc0.xyz, r.oc0.w * in.color.w);
}

@fragment
fn unit_skinned_alpha_ps(in: UnitSkinnedVertexOut) -> @location(0) vec4<f32> {
    return unit_skinned_alpha_ps_impl(in, false);
}

@fragment
fn unit_skinned_alpha_reflect_ps(in: UnitSkinnedVertexOut) -> @location(0) vec4<f32> {
    return unit_skinned_alpha_ps_impl(in, true);
}
