// LandmarkShader of the original: `LandmarkVS` and `LandmarkPS`. The
// pixel shader is a texture read with no light of its own (the landmark textures carry it); the
// vertex shader gives the point, the normal and the uv. The packages `LandmarkShader_Stencil` and
// `LandmarkShader_Shadow` use the same entries under other states of the pipeline.
//
// Hand port in the convention of shader-re 4.8.1. Groups: 0 the frame (binding 1
// `frame_globals`); 3 the material (`BaseTextureMap`: 0, 1).
//
// `WorldMatrix` (float4x3, c8..c10) comes from the instance (data move `instance:world`), and from
// the bone of the instance first when the mesh sits on an animated bone (`HAS_BIND`, WA-D8: the
// whale, the polder).
//
// Needs frame.wgsl, skin.wgsl and skin/common.wgsl.

@group(3) @binding(0) var BaseTextureMap: texture_2d<f32>;
@group(3) @binding(1) var BaseTextureMap_smp: sampler;

// ---- LandmarkVS ----

struct LandmarkVsIn {
    // dcl_position v0, with its w (the shader uses all four components).
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    // `WorldMatrix` (float4x3, c8..c10): column j is the register c(8 + j).
    instance_world: mat3x4<f32>,
}

struct LandmarkVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
}

// re: LandmarkShader::LandmarkVS (sha1 373d86bbea7d)
fn landmark_vs_body(inp: LandmarkVsIn) -> LandmarkVsOut {
    // c3 = ViewInverseMatrix+3, c4..c7 = ViewProjMatrix, c8..c10 = WorldMatrix.
    let c3 = skin_reg(frame_globals.ViewInverseMatrix, 3u);
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let c8 = inp.instance_world[0];
    let c9 = inp.instance_world[1];
    let c10 = inp.instance_world[2];
    let r0y = dot(inp.v0, c9);
    var r1 = r0y * c5;
    let r0x = dot(inp.v0, c8);
    r1 = r0x * c4 + r1;
    let r0z = dot(inp.v0, c10);
    r1 = r0z * c6 + r1;
    var o: LandmarkVsOut;
    o.o2 = vec4<f32>(vec3<f32>(r0x, r0y, r0z) - c3.xyz, inp.v1.y);
    o.o0 = r1 + c7;
    o.o1 = vec4<f32>(
        dot(inp.v2, c8.xyz),
        dot(inp.v2, c9.xyz),
        dot(inp.v2, c10.xyz),
        inp.v1.x,
    );
    return o;
}

// ---- LandmarkPS ----

struct LandmarkPsIn {
    // dcl_texcoord v0.w and dcl_texcoord1 v1.w: the u and the v of the texture.
    v0: vec4<f32>,
    v1: vec4<f32>,
}

struct LandmarkPsOut {
    oc0: vec4<f32>,
}

// re: LandmarkShader::LandmarkPS (sha1 92451f5fce86)
fn landmark_ps_body(inp: LandmarkPsIn) -> LandmarkPsOut {
    var o: LandmarkPsOut;
    o.oc0 = textureSample(BaseTextureMap, BaseTextureMap_smp, vec2<f32>(inp.v0.w, inp.v1.w));
    return o;
}

// ---- entries: the instance of world-objects ----

struct LandmarkVertexIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    @location(6) color: vec4<f32>,
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

struct LandmarkVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
}

@vertex
fn landmark_vs(v: LandmarkVertexIn) -> LandmarkVertexOut {
    var b: LandmarkVsIn;
    b.v0 = vec4<f32>(v.position, 1.0);
    b.v1 = v.uv;
    b.v2 = skin_oct_decode(v.normal_oct);
    b.instance_world = rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend));
    let r = landmark_vs_body(b);
    var out: LandmarkVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    return out;
}

@fragment
fn landmark_ps(in: LandmarkVertexOut) -> @location(0) vec4<f32> {
    var b: LandmarkPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    return landmark_ps_body(b).oc0;
}
