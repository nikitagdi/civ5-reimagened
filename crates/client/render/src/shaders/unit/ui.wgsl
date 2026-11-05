// UIShader of the original, the part the models of the movement path use: `UserInterfaceVS` (the rigid vertex stage) and the pixel stage `UserInterfaceSkinnedPS`,
// which is the code of `UserInterfacePS` too (the two have one sha1). The path models are meshes of
// the package `UIShader` (`Center*`, `Connector*`, `SelectedAni*`); the capture `S4-path` shows
// them going through `UserInterfaceSkinnedPS` after the boundary of the UI.
//
// The model is lit by one irradiance cube (`IrradianceCubeMap`, the cube of the material, in
// practice `cubelight_generic_32.dds`) with a Phong highlight from `SREFMap` (its alpha is the
// power); no fog, no shadow, no team colour. The skinned vertex shader of the package
// (`UserInterfaceSkinnedVS`) is not ported: no mesh of these models has weights, the animated one
// sits on a bone (`MeshV2::bind`), which the instance's bone does for the rigid stage (the same way
// `simple_vs` does it).
//
// Hand port in the convention of shader-re 4.8.1. Groups: 0 the frame (binding 1 `frame_globals`);
// 3 the material (`BaseTextureMap`: 0, 1; `SREFMap`: 2, 3; `IrradianceCubeMap`: 4, 5).
//
// `WorldMatrix` is a `row_major float4x4` (c8..c11, the rows are the images of x, y, z and the
// origin): the comparison fills it from the instance (data move `instance:world`) as a `mat4x4`
// whose register 8 + k is `skin_reg(m, k)`.
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl and skin/common.wgsl.

@group(3) @binding(0) var BaseTextureMap: texture_2d<f32>;
@group(3) @binding(1) var BaseTextureMap_smp: sampler;
@group(3) @binding(2) var SREFMap: texture_2d<f32>;
@group(3) @binding(3) var SREFMap_smp: sampler;
@group(3) @binding(4) var IrradianceCubeMap: texture_cube<f32>;
@group(3) @binding(5) var IrradianceCubeMap_smp: sampler;

// ---- UserInterfaceVS ----

struct UiVsIn {
    // dcl_position v0: with its w, which the shader uses.
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    instance_world: mat4x4<f32>,
}

struct UiVsOut {
    o0: vec4<f32>,
    // dcl_texcoord o1: the normal in the world, and u.
    o1: vec4<f32>,
    // dcl_texcoord1 o2: the vector from the eye to the point, and v.
    o2: vec4<f32>,
}

// re: UIShader::UserInterfaceVS (sha1 218379a532f3)
fn ui_vs_body(inp: UiVsIn) -> UiVsOut {
    // c0..c3 = ViewInverseMatrix (only the row 3 is read), c4..c7 = ViewProjMatrix, c8..c11 = WorldMatrix.
    let c3 = skin_reg(frame_globals.ViewInverseMatrix, 3u);
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let c8 = skin_reg(inp.instance_world, 0u);
    let c9 = skin_reg(inp.instance_world, 1u);
    let c10 = skin_reg(inp.instance_world, 2u);
    let c11 = skin_reg(inp.instance_world, 3u);
    var p = c9.xyz * inp.v0.y;
    p = inp.v0.x * c8.xyz + p;
    p = inp.v0.z * c10.xyz + p;
    p = inp.v0.w * c11.xyz + p;
    var clip = p.y * c5;
    clip = p.x * c4 + clip;
    clip = p.z * c6 + clip;
    var o: UiVsOut;
    o.o2 = vec4<f32>(p - c3.xyz, inp.v1.y);
    o.o0 = clip + c7;
    var n = c9.xyz * inp.v2.y;
    n = inp.v2.x * c8.xyz + n;
    o.o1 = vec4<f32>(inp.v2.z * c10.xyz + n, inp.v1.x);
    return o;
}

// ---- UserInterfaceSkinnedPS (and UserInterfacePS) ----

struct UiPsIn {
    // dcl_texcoord v0: the normal (xyz) and u (w).
    v0: vec4<f32>,
    // dcl_texcoord1 v1: the vector from the eye (xyz) and v (w).
    v1: vec4<f32>,
}

struct UiPsOut {
    oc0: vec4<f32>,
}

// re: UIShader::UserInterfaceSkinnedPS (sha1 fad4a8f01fa0)
fn ui_ps_body(inp: UiPsIn) -> UiPsOut {
    // def c0, 255, 0, 0, 0.
    let n = d3d_nrm(vec4<f32>(inp.v0.xyz, 0.0)).xyz;
    let v = d3d_nrm(vec4<f32>(inp.v1.xyz, 0.0)).xyz;
    // The mirror of the eye vector about the normal.
    let k = dot(v, n);
    let r = n * -(k + k) + v;
    let irr = textureSample(IrradianceCubeMap, IrradianceCubeMap_smp, n);
    let irr2 = irr.xyz * irr.xyz;
    let facing = clamp(dot(r, v), 0.0, 1.0);
    let uv = vec2<f32>(inp.v0.w, inp.v1.w);
    let spec = textureSample(SREFMap, SREFMap_smp, uv);
    let base = textureSample(BaseTextureMap, BaseTextureMap_smp, uv);
    let base2 = base * base;
    var spec2 = spec * spec;
    spec2.w = spec2.w * 255.0;
    let power = d3d_pow(facing, spec2.w);
    let lit = base2.xyz * irr2 + spec2.xyz * power;
    var out: UiPsOut;
    out.oc0 = vec4<f32>(
        d3d_rcp(d3d_rsq(lit.x)),
        d3d_rcp(d3d_rsq(lit.y)),
        d3d_rcp(d3d_rsq(lit.z)),
        d3d_rcp(d3d_rsq(base2.w)),
    );
    return out;
}

// ---- entries: the instance of world-objects ----

struct UiVertexIn {
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

struct UiVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
}

// The member form of `WorldMatrix`: the register `8 + k` is `skin_reg(m, k)`, so the member is the
// rows of the 3x4 matrix with the row (0, 0, 0, 1) under them.
fn ui_world_regs(w: mat3x4<f32>) -> mat4x4<f32> {
    return mat4x4<f32>(w[0], w[1], w[2], vec4<f32>(0.0, 0.0, 0.0, 1.0));
}

@vertex
fn ui_vs(v: UiVertexIn) -> UiVertexOut {
    var b: UiVsIn;
    b.v0 = vec4<f32>(v.position, 1.0);
    b.v1 = v.uv;
    b.v2 = skin_oct_decode(v.normal_oct);
    b.instance_world = ui_world_regs(
        rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend)),
    );
    let r = ui_vs_body(b);
    var out: UiVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1;
    out.v1 = r.o2;
    return out;
}

@fragment
fn ui_ps(in: UiVertexOut) -> @location(0) vec4<f32> {
    var b: UiPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    return ui_ps_body(b).oc0;
}
