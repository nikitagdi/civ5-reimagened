// UnitShader of the original, rigid and opaque: `UnitVS` and `UnitPS` (render-core spec 4.15, RC-15
// pilot). Hand port in the convention of shader-re 4.8.1: each entry of the original is a body
// `fn <entry>_body(inp: In) -> Out` with the registers of the original as member names, and a
// `@vertex` / `@fragment` entry that only reads its inputs, calls the body and finishes the job
// the original did elsewhere (the world matrix, the fog of war of B1).
//
// Needs frame.wgsl (`frame_globals`, group 0 binding 1) and sm3.wgsl. Resources carry the names
// of the CTAB; a sampler `S` is the texture `S` and the sampler `S_smp`.
//
// Groups: 0 the frame (binding 1 `frame_globals`) and the light environment (the cubes and the fog
// ramp, bindings 2..9); 1 the object of the map copy (B1's `Object`); 2 the fog of war of B1;
// 3 the material (the base texture and `SREFMap`).
//
// Gamma: the frame is not sRGB. The shader squares its inputs and takes the square root at the
// end (gamma 2.0 of the models), the textures are raw `*Unorm` values.

// ---- light environment, group 0 ----

@group(0) @binding(2) var IrradianceCubeMap: texture_cube<f32>;
@group(0) @binding(3) var IrradianceCubeMap_smp: sampler;
@group(0) @binding(4) var DullEnvironmentCubeMap: texture_cube<f32>;
@group(0) @binding(5) var DullEnvironmentCubeMap_smp: sampler;
@group(0) @binding(6) var EnvironmentCubeMap: texture_cube<f32>;
@group(0) @binding(7) var EnvironmentCubeMap_smp: sampler;
@group(0) @binding(8) var DistanceFogRamp: texture_2d<f32>;
@group(0) @binding(9) var DistanceFogRamp_smp: sampler;

// ---- material, group 3 ----

@group(3) @binding(0) var BaseTextureMap: texture_2d<f32>;
@group(3) @binding(1) var BaseTextureMap_smp: sampler;
@group(3) @binding(2) var SREFMap: texture_2d<f32>;
@group(3) @binding(3) var SREFMap_smp: sampler;

// ---- UnitVS ----

struct UnitVsIn {
    // dcl_position v0: the object-space position (x, y, z).
    v0: vec3<f32>,
    // dcl_texcoord v1: (u, v).
    v1: vec2<f32>,
    // dcl_normal v2.
    v2: vec3<f32>,
    // `WorldMatrix` (float4x3, c8..c10): the original gets it from the constants, we from the
    // instance (data move `instance:world`). Column j is the register c(8 + j): row j of the
    // object-to-world transform.
    instance_world: mat3x4<f32>,
}

struct UnitVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec4<f32>,
    o3: vec3<f32>,
    o4: vec3<f32>,
}

// The register `k` of a `row_major float4x4` that was filled by CTAB name (reg k = row k): the
// matrix member `[j][k]` is component j of register k.
fn row_major_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// re: UnitShader::UnitVS (sha1 ade84e670527)
fn unit_vs_body(inp: UnitVsIn) -> UnitVsOut {
    let c8 = inp.instance_world[0];
    let c9 = inp.instance_world[1];
    let c10 = inp.instance_world[2];
    // c3 = ViewInverseMatrix+3, c4..c7 = ViewProjMatrix.
    let c3 = row_major_reg(frame_globals.ViewInverseMatrix, 3u);
    let c4 = row_major_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = row_major_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = row_major_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = row_major_reg(frame_globals.ViewProjMatrix, 3u);
    var o: UnitVsOut;
    o.o1 = vec4<f32>(
        dot(inp.v2, c8.xyz),
        dot(inp.v2, c9.xyz),
        dot(inp.v2, c10.xyz),
        inp.v1.x,
    );
    let r0 = vec4<f32>(inp.v0, 1.0);
    let r1 = vec3<f32>(dot(r0, c8), dot(r0, c9), dot(r0, c10));
    o.o2 = vec4<f32>(r1 - c3.xyz, inp.v1.y);
    o.o3 = r1;
    var p = r1.y * c5;
    p = r1.x * c4 + p;
    p = r1.z * c6 + p;
    p = p + c7;
    o.o0 = p;
    o.o4 = p.xyw;
    return o;
}

// ---- UnitPS ----

struct UnitPsIn {
    // dcl_texcoord v0: the normal in the world (xyz) and the u of the texture.
    v0: vec4<f32>,
    // dcl_texcoord1 v1: the vector from the eye to the point (xyz) and the v of the texture.
    v1: vec4<f32>,
    // dcl_texcoord2 v2.xyz: the point in the world.
    v2: vec3<f32>,
    // dcl_texcoord3 v3.yz: the y and w of the clip position (x unused).
    v3: vec3<f32>,
    // `TeamColorR` (c4): the original gets it from the constants, we from the instance (data move
    // `instance:team_color`). Gamma values, as the player's colour is.
    instance_team_color: vec4<f32>,
}

struct UnitPsOut {
    oc0: vec4<f32>,
}

// The distance fog: exponential in height and distance, ramp `DistanceFogRamp`, colour
// `f3FogColor`. `g_FogConsts` = (height, distance, -, ramp row); `f3CameraWorld` the eye.
// re: UnitShader::UnitPS (sha1 cb0f37374433)
fn unit_ps_body(inp: UnitPsIn) -> UnitPsOut {
    // Constants of the shader: `def c0, 1, 0.5, 1.44269502 (log2 e), 0`.
    let one = 1.0;
    let half = 0.5;
    let log2e = 1.44269502;
    let g_FogConsts = frame_globals.g_FogConsts;
    let f3FogColor = frame_globals.f3FogColor;
    let f3CameraWorld = frame_globals.f3CameraWorld;
    let TeamColorR = inp.instance_team_color;

    // The fog factor (r0.x).
    let d = inp.v2 - f3CameraWorld;
    var r0w = d.z * g_FogConsts.x;
    r0w = r0w * log2e;
    r0w = exp2(r0w);
    r0w = -r0w + one;
    let dist2 = dot(d, d);
    let inv_dz = d3d_rcp(d.z);
    var fog = dist2 * g_FogConsts.y;
    fog = r0w * fog;
    fog = inv_dz * fog;
    fog = fog * log2e;
    fog = saturate(exp2(fog));
    fog = -fog + one;
    // The ramp coordinate: y / w of the clip position, 0..1.
    let ramp_u = (d3d_rcp(inp.v3.z) * inp.v3.y) * half + half;
    let ramp = textureSample(DistanceFogRamp, DistanceFogRamp_smp, vec2<f32>(ramp_u, g_FogConsts.w));
    fog = fog * ramp.x;

    // Light.
    let n_view = d3d_nrm(inp.v1).xyz;
    let n_surf = d3d_nrm(inp.v0).xyz;
    var k = dot(n_view, n_surf);
    k = k + k;
    let refl = n_surf * (-k) + n_view;
    let irradiance = textureSample(IrradianceCubeMap, IrradianceCubeMap_smp, n_surf);
    let irradiance2 = irradiance.xyz * irradiance.xyz;
    let sharp = textureSample(EnvironmentCubeMap, EnvironmentCubeMap_smp, refl);
    let dull = textureSample(DullEnvironmentCubeMap, DullEnvironmentCubeMap_smp, refl);
    let dull2 = dull.xyz * dull.xyz;
    let sharp_minus_dull = sharp.xyz * sharp.xyz - dull2;
    let uv = vec2<f32>(inp.v0.w, inp.v1.w);
    var sref = textureSample(SREFMap, SREFMap_smp, uv);
    var base = textureSample(BaseTextureMap, BaseTextureMap_smp, uv);
    base = base * base;
    sref = sref * sref;
    // The weight of the sharp reflection: SREF.a ^ 8.
    var gloss = sref.w * sref.w;
    gloss = saturate(gloss * gloss);
    var spec = gloss * sharp_minus_dull + dull2;
    spec = spec * sref.xyz;

    // The colour of the team: base.rgb^2 * lerp(TeamColor^2, 1, base.a^2).
    let team2 = TeamColorR.xyz * TeamColorR.xyz;
    let not_team = -TeamColorR.xyz * TeamColorR.xyz + one;
    var tint = base.w * not_team + team2;
    tint = base.xyz * tint;
    let lit = tint * irradiance2 + spec;

    // Square root back to gamma 2.0: rcp(rsq(x)).
    let color = vec3<f32>(
        d3d_rcp(d3d_rsq(lit.x)),
        d3d_rcp(d3d_rsq(lit.y)),
        d3d_rcp(d3d_rsq(lit.z)),
    );
    var o: UnitPsOut;
    o.oc0 = vec4<f32>(fog * (f3FogColor.xyz - color) + color, one);
    return o;
}

// ---- UnitAlphaPS ----
//
// The unit drawn with transparency (`ALPHA_SORT` materials): the same inputs as `UnitPS`,
// the same light and fog, but no team colour: the colour is `base^2 * lerp(base^2, 1, base.a^2)`,
// the alpha `base.a^2`, a third of it where the point is under the water (`fWaterHeight`).

struct UnitAlphaPsIn {
    // dcl_texcoord v0, v1, v2.xyz, v3.yz: as `UnitPsIn`.
    v0: vec4<f32>,
    v1: vec4<f32>,
    v2: vec3<f32>,
    v3: vec3<f32>,
}

// re: UnitShader::UnitAlphaPS (sha1 7f4640bb3ad1)
fn unit_alpha_ps_body(inp: UnitAlphaPsIn) -> UnitPsOut {
    // Constants of the shader: `def c4, 1, 0.33, 0.5, 1.44269502 (log2 e)`.
    let one = 1.0;
    let third = 0.330000013;
    let half = 0.5;
    let log2e = 1.44269502;
    let fWaterHeight = frame_globals.fWaterHeight;
    let g_FogConsts = frame_globals.g_FogConsts;
    let f3FogColor = frame_globals.f3FogColor;
    let f3CameraWorld = frame_globals.f3CameraWorld;

    // Above the water or under it.
    let above = inp.v2.z - fWaterHeight;
    let uv = vec2<f32>(inp.v0.w, inp.v1.w);
    let base = textureSample(BaseTextureMap, BaseTextureMap_smp, uv);
    let sref = textureSample(SREFMap, SREFMap_smp, uv);
    let sref2 = sref * sref;
    let base2 = base * base;
    // base^2 * lerp(base^2, 1, base.a^2).
    var tint = base.xyz * (-base.xyz) + vec3<f32>(one);
    tint = base2.w * tint + base2.xyz;
    tint = base2.xyz * tint;
    let alpha = select(base2.w * third, base2.w, above >= 0.0);

    // The weight of the sharp reflection: SREF.a ^ 8.
    var gloss = sref2.w * sref2.w;
    gloss = saturate(gloss * gloss);
    let n_view = d3d_nrm(inp.v1).xyz;
    let n_surf = d3d_nrm(inp.v0).xyz;
    var k = dot(n_view, n_surf);
    k = k + k;
    let refl = n_surf * (-k) + n_view;
    let irradiance = textureSample(IrradianceCubeMap, IrradianceCubeMap_smp, n_surf);
    let irradiance2 = irradiance.xyz * irradiance.xyz;
    let sharp = textureSample(EnvironmentCubeMap, EnvironmentCubeMap_smp, refl);
    let dull = textureSample(DullEnvironmentCubeMap, DullEnvironmentCubeMap_smp, refl);
    let dull2 = dull.xyz * dull.xyz;
    let sharp_minus_dull = sharp.xyz * sharp.xyz - dull2;
    let spec = gloss * sharp_minus_dull + dull2;
    let lit = tint * irradiance2 + sref2.xyz * spec;

    // Square root back to gamma 2.0: rcp(rsq(x)).
    let color = vec3<f32>(
        d3d_rcp(d3d_rsq(lit.x)),
        d3d_rcp(d3d_rsq(lit.y)),
        d3d_rcp(d3d_rsq(lit.z)),
    );

    // The distance fog, as `UnitPS`.
    let d = inp.v2 - f3CameraWorld;
    var r0w = d.z * g_FogConsts.x;
    r0w = r0w * log2e;
    r0w = exp2(r0w);
    r0w = -r0w + one;
    let dist2 = dot(d, d);
    let inv_dz = d3d_rcp(d.z);
    var fog = dist2 * g_FogConsts.y;
    fog = r0w * fog;
    fog = inv_dz * fog;
    fog = fog * log2e;
    fog = saturate(exp2(fog));
    fog = -fog + one;
    let ramp_u = (d3d_rcp(inp.v3.z) * inp.v3.y) * half + half;
    let ramp = textureSample(DistanceFogRamp, DistanceFogRamp_smp, vec2<f32>(ramp_u, g_FogConsts.w));
    fog = fog * ramp.x;

    var o: UnitPsOut;
    o.oc0 = vec4<f32>(fog * (f3FogColor.xyz - color) + color, alpha);
    return o;
}

// ---- entries: what B1 draws units with ----

// The map copy of B1 (`Object`): its model matrix shifts the copy along X (our basis).
struct UnitObject {
    model: mat4x4<f32>,
    tint: vec4<f32>,
}
@group(1) @binding(0) var<uniform> unit_object: UnitObject;

// Fog state per cell of B1: 0 unrevealed, 1 revealed and hidden, 2 visible.
@group(2) @binding(0) var unit_fog_tex: texture_2d<u32>;

// How much of its colour an instance on a revealed but hidden cell keeps, and how dark it is:
// the values of B1 (`terrain::fog::FOG_HIDDEN_SATURATION`, `FOG_HIDDEN_FACTOR`).
const HIDDEN_SATURATION: f32 = 0.55;
const HIDDEN_FACTOR: f32 = 0.6;
// Instance `cell` of an instance that is not fogged.
const NO_CELL: u32 = 0xFFFFFFFFu;

struct UnitVertexIn {
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
    // The colour of the team, `0x01RRGGBB`.
    @location(9) team: u32,
}

struct UnitVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec4<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec3<f32>,
    @location(4) @interpolate(flat) cell: u32,
    @location(5) @interpolate(flat) team: u32,
}

fn unit_oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}

// The transform of an instance in our basis: the map copy, then the position, the turn about Y and
// the scale (B1's `static.wgsl`), then to the original's basis.
fn unit_world(pos_rot: vec4<f32>, scale: f32) -> mat3x4<f32> {
    let c = cos(pos_rot.w);
    let s = sin(pos_rot.w);
    // rotate_y of B1: (x c + z s, y, -x s + z c), as columns, times the scale.
    let m_instance = mat4x4<f32>(
        vec4<f32>(c * scale, 0.0, -s * scale, 0.0),
        vec4<f32>(0.0, scale, 0.0, 0.0),
        vec4<f32>(s * scale, 0.0, c * scale, 0.0),
        vec4<f32>(pos_rot.xyz, 1.0),
    );
    let m = frame_globals.ours_to_orig * (unit_object.model * m_instance);
    // Row j of the matrix is the register c(8 + j) of the original.
    return mat3x4<f32>(
        vec4<f32>(m[0][0], m[1][0], m[2][0], m[3][0]),
        vec4<f32>(m[0][1], m[1][1], m[2][1], m[3][1]),
        vec4<f32>(m[0][2], m[1][2], m[2][2], m[3][2]),
    );
}

@vertex
fn unit_vs(v: UnitVertexIn) -> UnitVertexOut {
    var b: UnitVsIn;
    b.v0 = v.position;
    b.v1 = v.uv;
    b.v2 = unit_oct_decode(v.normal_oct);
    b.instance_world = unit_world(v.pos_rot, v.scale_tint.x);
    let r = unit_vs_body(b);
    var out: UnitVertexOut;
    out.clip = r.o0;
    if (v.variant != v.inst_variant) {
        // All three vertices of a foreign triangle land on the same point outside the volume.
        out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    out.v0 = r.o1;
    out.v1 = r.o2;
    out.v2 = r.o3;
    out.v3 = r.o4;
    out.cell = v.cell;
    out.team = v.team;
    return out;
}

@fragment
fn unit_ps(in: UnitVertexOut) -> @location(0) vec4<f32> {
    var b: UnitPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    // 0x01RRGGBB: gamma values 0..1.
    b.instance_team_color = vec4<f32>(
        f32((in.team >> 16u) & 255u),
        f32((in.team >> 8u) & 255u),
        f32(in.team & 255u),
        255.0,
    ) / 255.0;
    // The samples come first: the control flow is uniform until the body is done.
    let r = unit_ps_body(b);
    var rgb = r.oc0.xyz;
    if (in.cell != NO_CELL) {
        let width = max(textureDimensions(unit_fog_tex).x, 1u);
        let state = textureLoad(
            unit_fog_tex,
            vec2<i32>(i32(in.cell % width), i32(in.cell / width)),
            0,
        ).r;
        if (state == 0u) {
            discard;
        }
        if (state == 1u) {
            let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            rgb = mix(vec3<f32>(grey), rgb, HIDDEN_SATURATION) * HIDDEN_FACTOR;
        }
    }
    return vec4<f32>(rgb, 1.0);
}

// The transparent unit: the same vertex stage, the alpha of `UnitAlphaPS`. The fog of war of B1
// treats the colour as `unit_ps` does; the alpha stays.
@fragment
fn unit_alpha_ps(in: UnitVertexOut) -> @location(0) vec4<f32> {
    var b: UnitAlphaPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    let r = unit_alpha_ps_body(b);
    var rgb = r.oc0.xyz;
    if (in.cell != NO_CELL) {
        let width = max(textureDimensions(unit_fog_tex).x, 1u);
        let state = textureLoad(
            unit_fog_tex,
            vec2<i32>(i32(in.cell % width), i32(in.cell / width)),
            0,
        ).r;
        if (state == 0u) {
            discard;
        }
        if (state == 1u) {
            let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
            rgb = mix(vec3<f32>(grey), rgb, HIDDEN_SATURATION) * HIDDEN_FACTOR;
        }
    }
    return vec4<f32>(rgb, r.oc0.w);
}
