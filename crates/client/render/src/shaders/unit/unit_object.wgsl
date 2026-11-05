// UnitShader, the rigid entries on the instance of `world-objects`:
// `UnitVS` and `UnitVS_Shadow` with the world matrix from the instance (`ObjectInstance`, 64 bytes)
// instead of the constants, and from the bone `BIND_BONE` of the instance when the mesh sits on
// one (`HAS_BIND`, WA-D8). The pixel stage is that of `unit_skinned_ps` / `unit_skinned_alpha_ps`
// (`UnitPS` and `UnitAlphaPS` under the same inputs).
//
// `unit_vs_body` (unit.wgsl) is the body of `UnitVS`; the shadow body is ported here.
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl, skin/common.wgsl and unit.wgsl.
// Vertex: POSITION0 0, NORMAL0 1 (oct snorm16), TEXCOORD0 2; the instance at the free TEXCOORD
// locations 3..8 (a rigid mesh has no TEXCOORD1): the rows of the matrix, the colour and alpha, the
// two frames, the blend.

// ---- UnitVS_Shadow ----

struct UnitShadowVsIn {
    // dcl_position v0, dcl_texcoord v1.
    v0: vec3<f32>,
    v1: vec2<f32>,
    // `WorldMatrix` (float4x3, c8..c10): data move `instance:world`; column j is the register c(8+j).
    instance_world: mat3x4<f32>,
}

struct UnitShadowVsOut {
    o0: vec4<f32>,
    o1: vec3<f32>,
}

// re: UnitShader::UnitVS_Shadow (sha1 5a5d130dd35f)
fn unit_shadow_vs_body(inp: UnitShadowVsIn) -> UnitShadowVsOut {
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let c8 = inp.instance_world[0];
    let c9 = inp.instance_world[1];
    let c10 = inp.instance_world[2];
    let r0 = vec4<f32>(inp.v0, 1.0);
    let r1x = dot(r0, c9);
    var r1 = r1x * c5;
    let r2x = dot(r0, c8);
    let r0x = dot(r0, c10);
    r1 = r2x * c4 + r1;
    r1 = r0x * c6 + r1;
    var o: UnitShadowVsOut;
    // `mov o1.z, r0.x` then `mov o1.xy, v1`: the uv and the height of the point.
    o.o1 = vec3<f32>(inp.v1, r0x);
    o.o0 = r1 + c7;
    return o;
}

// ---- entries ----

struct UnitObjectVertexIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    // The rows of the matrix of the object, our basis.
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    // The colour of the team and the alpha, unorm8x4.
    @location(6) color: vec4<f32>,
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

// The vertex stage of `UnitVS` for the instance of `world-objects`: the output is that of
// `unit_skinned_vs`, so the pixel entries are the same.
@vertex
fn unit_object_vs(v: UnitObjectVertexIn) -> UnitSkinnedVertexOut {
    var b: UnitVsIn;
    b.v0 = v.position;
    b.v1 = unit_texcoord(v.uv);
    b.v2 = skin_oct_decode(v.normal_oct);
    b.instance_world = rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend));
    let r = unit_vs_body(b);
    var out: UnitSkinnedVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.v2 = r.o3;
    out.v3 = r.o4;
    out.color = v.color;
    return out;
}

@vertex
fn unit_shadow_vs(v: UnitObjectVertexIn) -> UnitSkinnedShadowVertexOut {
    var b: UnitShadowVsIn;
    b.v0 = v.position;
    b.v1 = unit_texcoord(v.uv);
    b.instance_world = rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend));
    let r = unit_shadow_vs_body(b);
    var out: UnitSkinnedShadowVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.alpha = v.color.w;
    return out;
}
