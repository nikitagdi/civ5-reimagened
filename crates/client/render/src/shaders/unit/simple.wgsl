// SimpleShader of the original: the rigid pair `SimpleVS` and
// `SimplePS`, and the pixel shader of the skinned vertex shader, `Skinned_SimplePS_Skinned` (the
// skinned vertex shader is `skin/simple_skinned.wgsl`). The model is lit by one cube map
// (`LightCubeMapSampler`) and has no other light.
//
// Hand port in the convention of shader-re 4.8.1. Groups: 0 the frame (binding 1 `frame_globals`)
// and the light cube (2, 3); 3 the material (`BaseSampler`: 0, 1).
//
// `WorldMatrix` is a `row_major float4x4` (c12..c15, the rows are the images of x, y, z and the
// origin): the comparison fills it from the instance (data move `instance:world`) as a `mat4x4`
// whose register 12 + k is `skin_reg(m, k)`.
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl and skin/common.wgsl, and simple_skinned.wgsl for the
// entries that share `SimpleSkinnedVertexOut`.

@group(0) @binding(2) var LightCubeMapSampler: texture_cube<f32>;
@group(0) @binding(3) var LightCubeMapSampler_smp: sampler;
@group(3) @binding(0) var BaseSampler: texture_2d<f32>;
@group(3) @binding(1) var BaseSampler_smp: sampler;

// ---- SimpleVS ----

struct SimpleVsIn {
    // dcl_position v0: with its w, which the shader uses.
    v0: vec4<f32>,
    // dcl_texcoord v1.
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    instance_world: mat4x4<f32>,
}

struct SimpleVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec3<f32>,
    o3: vec4<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
}

// re: SimpleShader::SimpleVS (sha1 2171825d3f57)
fn simple_vs_body(inp: SimpleVsIn) -> SimpleVsOut {
    // def c4, 1, 0, 0, 0.
    let one = 1.0;
    let zero = 0.0;
    // c0..c3 = ViewProjMatrix, c11 = ViewInverseMatrix+3, c12..c15 = WorldMatrix.
    let c0 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c1 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c2 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c3 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let c11 = skin_reg(frame_globals.ViewInverseMatrix, 3u);
    let c12 = skin_reg(inp.instance_world, 0u);
    let c13 = skin_reg(inp.instance_world, 1u);
    let c14 = skin_reg(inp.instance_world, 2u);
    let c15 = skin_reg(inp.instance_world, 3u);
    var o: SimpleVsOut;
    var n = c13.xyz * inp.v2.y;
    n = inp.v2.x * c12.xyz + n;
    o.o2 = inp.v2.z * c14.xyz + n;
    var r0 = c13 * inp.v0.y;
    r0 = inp.v0.x * c12 + r0;
    r0 = inp.v0.z * c14 + r0;
    r0 = inp.v0.w * c15 + r0;
    var r1 = r0.y * c1;
    r1 = r0.x * c0 + r1;
    r1 = r0.z * c2 + r1;
    o.o0 = r0.w * c3 + r1;
    o.o4 = vec4<f32>(r0.xyz - c11.xyz, zero);
    o.o3 = vec4<f32>(r0.xyz, zero);
    // `mad o1, v1.xyxx, c4.xxyy, c4.yyxx`: (u, v, 1, 1).
    o.o1 = vec4<f32>(inp.v1.x * one + zero, inp.v1.y * one + zero, inp.v1.x * zero + one, inp.v1.x * zero + one);
    o.o5 = vec4<f32>(zero);
    return o;
}

// ---- SimplePS ----

struct SimplePsIn {
    // dcl_texcoord v0.xy: the uv.
    v0: vec2<f32>,
    // dcl_texcoord1 v1.xyz: the normal in the world.
    v1: vec3<f32>,
}

struct SimplePsOut {
    oc0: vec4<f32>,
    // `texkill`: the texel is not drawn.
    kill: bool,
}

// re: SimpleShader::SimplePS (sha1 b0d8891bd192)
fn simple_ps_body(inp: SimplePsIn) -> SimplePsOut {
    // def c0, -0.5, -0, -1, 0.
    let base = textureSample(BaseSampler, BaseSampler_smp, inp.v0);
    var out: SimplePsOut;
    // texkill of the cmp: the alpha under one half (and NaN).
    out.kill = !(base.w + -0.5 >= 0.0);
    let base2 = base.xyz * base.xyz;
    let n = d3d_nrm(vec4<f32>(inp.v1, 0.0)).xyz;
    let light = textureSample(LightCubeMapSampler, LightCubeMapSampler_smp, n);
    // (light.rgb, 1) times (base^2, base.a).
    let lit = base2 * light.xyz;
    out.oc0 = vec4<f32>(
        d3d_rcp(d3d_rsq(lit.x)),
        d3d_rcp(d3d_rsq(lit.y)),
        d3d_rcp(d3d_rsq(lit.z)),
        base.w,
    );
    return out;
}

// ---- Skinned_SimplePS_Skinned ----

// re: SimpleShader::Skinned_SimplePS_Skinned (sha1 7fbf881d9eca)
fn simple_skinned_ps_body(inp: SimplePsIn) -> SimplePsOut {
    // The vector into the cube is not normalized here, and there is no alpha test.
    let light = textureSample(LightCubeMapSampler, LightCubeMapSampler_smp, inp.v1);
    let light2 = light.xyz * light.xyz;
    let base = textureSample(BaseSampler, BaseSampler_smp, inp.v0);
    let base2 = base.xyz * base.xyz;
    let lit = light2 * base2;
    var out: SimplePsOut;
    out.kill = false;
    out.oc0 = vec4<f32>(
        d3d_rcp(d3d_rsq(lit.x)),
        d3d_rcp(d3d_rsq(lit.y)),
        d3d_rcp(d3d_rsq(lit.z)),
        base.w,
    );
    return out;
}

// ---- entries: the instance of world-objects ----

struct SimpleVertexIn {
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

// The member form of `WorldMatrix`: the register `12 + k` is `skin_reg(m, k)`, so the member is the
// rows of the 3x4 matrix with the row (0, 0, 0, 1) under them.
fn simple_world_regs(w: mat3x4<f32>) -> mat4x4<f32> {
    return mat4x4<f32>(w[0], w[1], w[2], vec4<f32>(0.0, 0.0, 0.0, 1.0));
}

@vertex
fn simple_vs(v: SimpleVertexIn) -> SimpleSkinnedVertexOut {
    var b: SimpleVsIn;
    b.v0 = vec4<f32>(v.position, 1.0);
    b.v1 = v.uv;
    b.v2 = skin_oct_decode(v.normal_oct);
    b.instance_world = simple_world_regs(
        rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend)),
    );
    let r = simple_vs_body(b);
    var out: SimpleSkinnedVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1.xy;
    out.v1 = r.o2;
    return out;
}

@fragment
fn simple_ps(in: SimpleSkinnedVertexOut) -> @location(0) vec4<f32> {
    var b: SimplePsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    // The samples come first: the control flow is uniform until the body is done.
    let r = simple_ps_body(b);
    if (r.kill) {
        discard;
    }
    return r.oc0;
}

@fragment
fn simple_skinned_ps(in: SimpleSkinnedVertexOut) -> @location(0) vec4<f32> {
    var b: SimplePsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    let r = simple_skinned_ps_body(b);
    return r.oc0;
}
