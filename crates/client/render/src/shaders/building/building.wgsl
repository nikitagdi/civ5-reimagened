// BuildingShader of the original: `BuildingVS` and `BuildingPS` (world-objects WOB1, the city of
// B1). Hand port in the convention of shader-re 4.8.1: each entry of the original is a body
// `fn <entry>_body(inp: In) -> Out` with the registers of the original as member names, and a
// `@vertex` / `@fragment` entry that only reads its inputs, calls the body and finishes the job
// the original did elsewhere (the world matrix of the instance, the fog of war of B1).
//
// `BuildingVS_Instanced` has no SM3 block (DX11 only): its matrix comes from the instance, which is
// what every vertex entry here does (WO-D4), so `building_vs` stands for both.
//
// Needs frame.wgsl (`frame_globals`, group 0 binding 1) and sm3.wgsl. Resources carry the names of
// the CTAB; a sampler `S` is the texture `S` and the sampler `S_smp`.
//
// Groups: 0 the frame (binding 1 `frame_globals`), the light cubes (2..7) and the fog-of-war
// projection of the original (10); 1 the object of the map copy (B1's `Object`); 2 the fog of war of
// B1; 3 the material (`Diffuse` and `BuildingSREF`).
//
// Gamma: the frame is not sRGB. `BuildingPS` raises the textures to 2.2 and the result to 1/2.2
// (the gamma of the terrain, not the 2.0 of the units).

// ---- light cubes, group 0 ----

@group(0) @binding(2) var Irradiance: texture_cube<f32>;
@group(0) @binding(3) var Irradiance_smp: sampler;
@group(0) @binding(4) var SpecSharp: texture_cube<f32>;
@group(0) @binding(5) var SpecSharp_smp: sampler;
@group(0) @binding(6) var SpecSoft: texture_cube<f32>;
@group(0) @binding(7) var SpecSoft_smp: sampler;

// The projection of the fog-of-war mask: the original's `OverlayProjection` and `f4FOWBaseScale`.
// They feed `o1.zw`, `o3` and `o4` of the vertex stage, which `BuildingPS` does not read; B1 darkens
// a hidden building by its cell (`building_ps` below), so the buffer is zero until `FowMask` of
// `world-overlays` supplies it.
struct BuildingFow {
    OverlayProjection: mat4x4<f32>,
    f4FOWBaseScale: vec4<f32>,
}
@group(0) @binding(10) var<uniform> fow_projection: BuildingFow;

// ---- material, group 3 ----

@group(3) @binding(0) var Diffuse: texture_2d<f32>;
@group(3) @binding(1) var Diffuse_smp: sampler;
@group(3) @binding(2) var BuildingSREF: texture_2d<f32>;
@group(3) @binding(3) var BuildingSREF_smp: sampler;

// ---- BuildingVS ----

struct BuildingVsIn {
    // dcl_position v0 (x, y, z; w is not read).
    v0: vec3<f32>,
    // dcl_texcoord v1: (u, v).
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    // `WorldMatrix` (row_major float4x4, c17..c20): the original gets it from the constants, we
    // from the instance (data move `instance:world`). The member `[j][k]` is component j of the
    // register c(17 + k).
    instance_world: mat4x4<f32>,
}

struct BuildingVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec3<f32>,
    o3: vec2<f32>,
    o4: vec2<f32>,
    o5: vec3<f32>,
}

// The register `k` of a `row_major float4x4` that was filled by CTAB name (reg k = row k): the
// matrix member `[j][k]` is component j of register k.
fn bld_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// re: BuildingShader::BuildingVS (sha1 ade9d3190110)
fn building_vs_body(inp: BuildingVsIn) -> BuildingVsOut {
    // Constant of the shader: `def c4, 0.5, -0.5, 0, 0`.
    let c4 = vec4<f32>(0.5, -0.5, 0.0, 0.0);
    let c0 = bld_reg(frame_globals.ViewProjMatrix, 0u);
    let c1 = bld_reg(frame_globals.ViewProjMatrix, 1u);
    let c2 = bld_reg(frame_globals.ViewProjMatrix, 2u);
    let c3 = bld_reg(frame_globals.ViewProjMatrix, 3u);
    let c8 = bld_reg(fow_projection.OverlayProjection, 0u);
    let c9 = bld_reg(fow_projection.OverlayProjection, 1u);
    let c10 = bld_reg(fow_projection.OverlayProjection, 2u);
    let c11 = bld_reg(fow_projection.OverlayProjection, 3u);
    let c12 = fow_projection.f4FOWBaseScale;
    let c17 = bld_reg(inp.instance_world, 0u);
    let c18 = bld_reg(inp.instance_world, 1u);
    let c19 = bld_reg(inp.instance_world, 2u);
    let c20 = bld_reg(inp.instance_world, 3u);
    var o: BuildingVsOut;

    // The normal: the rotation part of the matrix, normalised by rsq.
    var n = c18.xyz * inp.v2.y;
    n = inp.v2.x * c17.xyz + n;
    n = inp.v2.z * c19.xyz + n;
    let n_len = d3d_rsq(dot(n, n));
    o.o5 = n_len * n;

    // The point in the world.
    var r0 = c18 * inp.v0.y;
    r0 = inp.v0.x * c17 + r0;
    r0 = inp.v0.z * c19 + r0;
    r0 = r0 + c20;
    var r1 = r0.y * c1;
    r1 = r0.x * c0 + r1;
    r1 = r0.z * c2 + r1;
    r1 = r0.w * c3 + r1;
    let inv_w = d3d_rcp(r1.w);
    o.o3 = inv_w * r1.xy;
    o.o0 = r1;
    // The fog of war of the original: the base layer and the mask.
    let base_uv = (r0.xy + c12.zw) * c12.xy;
    o.o1 = vec4<f32>(inp.v1, base_uv);
    var m = r0.y * c9.xy;
    m = r0.x * c8.xy + m;
    m = r0.z * c10.xy + m;
    o.o2 = r0.xyz;
    let m2 = m + c11.xy;
    o.o4 = m2 * c4.xy + c4.y;
    return o;
}

// ---- BuildingPS ----

struct BuildingPsIn {
    // dcl_texcoord v0.xy: the uv of the textures.
    v0: vec2<f32>,
    // dcl_texcoord1 v1.xyz: the point in the world.
    v1: vec3<f32>,
    // dcl_normal v2.xyz: the normal in the world.
    v2: vec3<f32>,
}

struct BuildingPsOut {
    oc0: vec4<f32>,
    // `texkill`: the texel is not drawn.
    kill: bool,
}

// pow(x, 2.2) as the shader does it: exp(log(x) * 2.2), per component.
fn bld_pow22(c: vec3<f32>) -> vec3<f32> {
    let l = vec3<f32>(d3d_log(c.x), d3d_log(c.y), d3d_log(c.z));
    return exp2(l * 2.20000005);
}

// re: BuildingShader::BuildingPS (sha1 18d02f5dc05f)
fn building_ps_body(inp: BuildingPsIn) -> BuildingPsOut {
    // Constants of the shader: `def c0, 2.2, -0.1, 199, 1` and `def c1, 0.0529, 0.846, 0.529,
    // 0.4545` (xyz: the direction of the light, w: 1 / 2.2). `c3` is the eye: the row 3 of
    // `ViewInverseMatrix`.
    let c0 = vec4<f32>(2.20000005, -0.100000001, 199.0, 1.0);
    let c1 = vec4<f32>(0.0529000014, 0.846000016, 0.528999984, 0.454545468);
    let c3 = bld_reg(frame_globals.ViewInverseMatrix, 3u);

    let diffuse = textureSample(Diffuse, Diffuse_smp, inp.v0);
    let irradiance = textureSample(Irradiance, Irradiance_smp, inp.v2);
    let sref = textureSample(BuildingSREF, BuildingSREF_smp, inp.v0);

    var out: BuildingPsOut;
    // texkill: the alpha under 0.1.
    out.kill = diffuse.w + c0.y < 0.0;

    // diffuse^2.2 * irradiance^2.2, doubled.
    let base = bld_pow22(diffuse.xyz);
    let irr = bld_pow22(irradiance.xyz);
    var lit = base * irr;
    lit = lit + lit;

    // The view vector, the half vector towards the light (`c1.xyz`), the reflection.
    let view = c3.xyz - inp.v1;
    let inv_len = d3d_rsq(dot(view, view));
    let view_n = inv_len * view;
    let half_v = d3d_nrm(vec4<f32>(view * inv_len + c1.xyz, 0.0)).xyz;
    let n_dot_h = saturate(dot(half_v, inp.v2));
    var k = dot(inp.v2, view_n);
    k = k + k;
    let refl = k * inp.v2 - view_n;
    let soft_s = textureSample(SpecSoft, SpecSoft_smp, refl);
    let sharp_s = textureSample(SpecSharp, SpecSharp_smp, refl);
    let soft = bld_pow22(soft_s.xyz);
    let sharp = bld_pow22(sharp_s.xyz);

    // SREFMap: the colour stays in the log domain (log2 * 2.2), the alpha is the gloss.
    let s_log = vec4<f32>(d3d_log(sref.x), d3d_log(sref.y), d3d_log(sref.z), d3d_log(sref.w)) * c0.x;
    let gloss = exp2(s_log.w);
    // lrp: gloss * (sharp - soft) + soft.
    let spec_color = gloss * (sharp - soft) + soft;
    let power = gloss * c0.z + c0.w;
    let highlight = d3d_pow(n_dot_h, power);
    let strength = power * -c0.y;
    let spec = strength * highlight + spec_color;
    let sref_color = exp2(s_log.xyz);
    let sum = spec * sref_color + lit;

    // Back to gamma: exp(log(x) / 2.2).
    let l = vec3<f32>(d3d_log(sum.x), d3d_log(sum.y), d3d_log(sum.z));
    out.oc0 = vec4<f32>(exp2(l * c1.w), c0.w);
    return out;
}

// ---- entries: what B1 draws a city with ----

// The map copy of B1 (`Object`): its model matrix shifts the copy along X (our basis).
struct BuildingObject {
    model: mat4x4<f32>,
    tint: vec4<f32>,
}
@group(1) @binding(0) var<uniform> building_object: BuildingObject;

// Fog state per cell of B1: 0 unrevealed, 1 revealed and hidden, 2 visible.
@group(2) @binding(0) var building_fog_tex: texture_2d<u32>;

// How much of its colour an instance on a revealed but hidden cell keeps, and how dark it is:
// the values of B1 (`terrain::fog::FOG_HIDDEN_SATURATION`, `FOG_HIDDEN_FACTOR`).
const BLD_HIDDEN_SATURATION: f32 = 0.55;
const BLD_HIDDEN_FACTOR: f32 = 0.6;
// Instance `cell` of an instance that is not fogged.
const BLD_NO_CELL: u32 = 0xFFFFFFFFu;

struct BuildingVertexIn {
    // The vertex of a static model of B1 (`StaticVertex`).
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) variant: u32,
    // The instance (`StaticInstance`, 48 bytes): xyz position, w rotation about Y.
    @location(4) pos_rot: vec4<f32>,
    // x uniform scale, yzw tint (not used here).
    @location(5) scale_tint: vec4<f32>,
    @location(6) alpha: f32,
    @location(7) cell: u32,
    @location(8) inst_variant: u32,
    // The colour of the team (not used by buildings).
    @location(9) team: u32,
}

struct BuildingVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec2<f32>,
    @location(1) v1: vec3<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) @interpolate(flat) cell: u32,
}

fn bld_oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}

// The transform of an instance in our basis: the map copy, then the position, the turn about Y and
// the scale (B1's `static.wgsl`), then to the original's basis. The register `c(17 + k)` of the
// original is the column k of this matrix, so the member is its transpose.
fn bld_world(pos_rot: vec4<f32>, scale: f32) -> mat4x4<f32> {
    let c = cos(pos_rot.w);
    let s = sin(pos_rot.w);
    // rotate_y of B1: (x c + z s, y, -x s + z c), as columns, times the scale.
    let m_instance = mat4x4<f32>(
        vec4<f32>(c * scale, 0.0, -s * scale, 0.0),
        vec4<f32>(0.0, scale, 0.0, 0.0),
        vec4<f32>(s * scale, 0.0, c * scale, 0.0),
        vec4<f32>(pos_rot.xyz, 1.0),
    );
    let m = frame_globals.ours_to_orig * (building_object.model * m_instance);
    return transpose(m);
}

@vertex
fn building_vs(v: BuildingVertexIn) -> BuildingVertexOut {
    var b: BuildingVsIn;
    b.v0 = v.position;
    b.v1 = v.uv;
    b.v2 = bld_oct_decode(v.normal_oct);
    b.instance_world = bld_world(v.pos_rot, v.scale_tint.x);
    let r = building_vs_body(b);
    var out: BuildingVertexOut;
    out.clip = r.o0;
    if (v.variant != v.inst_variant) {
        // All three vertices of a foreign triangle land on the same point outside the volume.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.v0 = r.o1.xy;
    out.v1 = r.o2;
    out.v2 = r.o5;
    out.cell = v.cell;
    return out;
}

// `building_ps`, and `building_reflect_ps` that also drops what lies below the water (the reflection
// pass; D3D9 cut with a clip plane).
fn building_ps_impl(in: BuildingVertexOut, reflection: bool) -> vec4<f32> {
    var b: BuildingPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    // The samples come first: the control flow is uniform until the body is done.
    let r = building_ps_body(b);
    if (r.kill || (reflection && reflection_clipped(in.v1))) {
        discard;
    }
    var rgb = r.oc0.xyz;
    if (in.cell != BLD_NO_CELL) {
        let width = max(textureDimensions(building_fog_tex).x, 1u);
        let state = textureLoad(
            building_fog_tex,
            vec2<i32>(i32(in.cell % width), i32(in.cell / width)),
            0,
        ).r;
        if (state == 0u) {
            discard;
        }
        if (state == 1u) {
            let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            rgb = mix(vec3<f32>(grey), rgb, BLD_HIDDEN_SATURATION) * BLD_HIDDEN_FACTOR;
        }
    }
    return vec4<f32>(rgb, 1.0);
}

@fragment
fn building_ps(in: BuildingVertexOut) -> @location(0) vec4<f32> {
    return building_ps_impl(in, false);
}

@fragment
fn building_reflect_ps(in: BuildingVertexOut) -> @location(0) vec4<f32> {
    return building_ps_impl(in, true);
}
