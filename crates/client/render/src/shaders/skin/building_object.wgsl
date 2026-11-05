// BuildingShader on the instance of `world-objects` (`ObjectInstance`): the rigid and the skinned
// meshes of the landmarks (the rocks of a stone deposit, the animals of a camp, the fruit trees) use
// the package `BuildingShader` / `BuildingShader_Skinned` like the buildings of a city, but are drawn
// by `UnitDraw` with the frame group, the bone texture and the fog mask of the objects.
//
// Two pixel stages. The buildings of the cities (the draw of `world::city`) take the ported colour
// pass of the D3D11 build (`building/building_code.wgsl`: the light, the colour volume of the city's
// style, the fog of war and the distance fog), `BuildingPS_CODE_Opaque` through
// `building_object_code_ps` and, for a material that blends, `BuildingPS_CODE` through
// `building_object_code_alpha_ps`; the draw gives them sRGB views of the textures and the cubes, as
// the D3D11 build does (S3 ev 13046). The landmarks
// keep `BuildingPS` (`building/building.wgsl`, `building_ps_body`), whose registers are copied here
// because that file's group 0 has its own cube order and the fog-of-war block of B1. The cubes are
// those of `UnitDraw` (binding 4 is the dull cube, 6 the sharp one: the building's `SpecSoft` and
// `SpecSharp`), and the irradiance is the unit's one (the cube `pBuildingGlobal` binds is not
// recovered, RW-4 d, as in `building_family`).
//
// The vertex stages of the colour pass are ours (`BuildingVS_Instanced` is DX11 only and has no
// port): the rigid one is `rigid_world` of the instance, the skinned one the palette walk of
// `UnitSkinnedVS` (the bones of the model are baked in the same texture; the skinning of
// `BuildingSkinnedVS` is assumed to be the unit's: 3 registers a bone, 3 bones a vertex). The
// casters of the shadow map run the ported `BuildingVS_Instanced_Shadow` and
// `BuildingSkinnedVS_Shadow` bodies.
//
// Needs frame.wgsl, sm3.wgsl, skin.wgsl, skin/common.wgsl and building/building_code.wgsl.

// The cubes (group 0, the frame group of `UnitDraw`), the diffuse and the SREF (group 3) are the
// resources of `building/building_code.wgsl`, which this module includes.


struct BuildingObjectVertexIn {
    @location(0) position: vec3<f32>,
    // Octahedral snorm16.
    @location(1) normal_oct: vec2<f32>,
    @location(2) uv: vec2<f32>,
    // The rows of the matrix of the object, our basis.
    @location(3) world0: vec4<f32>,
    @location(4) world1: vec4<f32>,
    @location(5) world2: vec4<f32>,
    @location(6) color: vec4<f32>,
    @location(7) frames: vec2<u32>,
    @location(8) blend: f32,
}

struct BuildingSkinnedVertexIn {
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

struct BuildingObjectVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // The point and the normal in the original's basis.
    @location(1) pos: vec3<f32>,
    @location(2) normal: vec3<f32>,
}

fn bo_project(p: vec3<f32>) -> vec4<f32> {
    let c4 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c5 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c6 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c7 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    return p.x * c4 + p.y * c5 + p.z * c6 + c7;
}

fn bo_out(p: vec3<f32>, n: vec3<f32>, uv: vec2<f32>) -> BuildingObjectVertexOut {
    var out: BuildingObjectVertexOut;
    out.clip = bo_project(p);
    // The pipeline flips V for the D3D textures of the units; the textures of the building materials
    // of the landmarks (`resource_*_diff`, `citrus_trees_diff`) are not stored flipped (golden S1a:
    // the deer stood on their backs and the orchard on its roots without this).
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    out.pos = p;
    out.normal = normalize(n);
    return out;
}

@vertex
fn building_object_vs(v: BuildingObjectVertexIn) -> BuildingObjectVertexOut {
    skin_world = rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend));
    let p = skin_place_point(v.position);
    let n = skin_place_dir(skin_oct_decode(v.normal_oct));
    return bo_out(p, n, v.uv);
}

struct BoSkinned {
    pos: vec3<f32>,
    nrm: vec3<f32>,
}

// The palette walk of the skinned vertex, in the space of the instance.
fn bo_skin(v: BuildingSkinnedVertexIn) -> BoSkinned {
    let r0 = vec4<f32>(v.position, 1.0);
    let normal = skin_oct_decode(v.normal_oct);
    let w = v.weights;
    let bone_p = bone_pose(v.frames, v.blend);
    let b = bone_p.base;
    let nx = bone_p.next;
    let k = bone_p.blend;
    var out: BoSkinned;
    out.pos = vec3<f32>(0.0);
    out.nrm = vec3<f32>(0.0);
    // The bones in turn: the first always, the next while the weight is not zero (as the unit's).
    for (var i = 0u; i < 3u; i = i + 1u) {
        let weight = w[i];
        if (i > 0u && weight <= 0.0) {
            break;
        }
        let a0 = skin_mova(3.0 * f32(v.joints[i]));
        let m0 = palette_row(b, nx, k, a0);
        let m1 = palette_row(b, nx, k, a0 + 1u);
        let m2 = palette_row(b, nx, k, a0 + 2u);
        out.pos = out.pos + weight * vec3<f32>(dot(r0, m0), dot(r0, m1), dot(r0, m2));
        out.nrm = out.nrm + weight * vec3<f32>(dot(normal, m0.xyz), dot(normal, m1.xyz), dot(normal, m2.xyz));
    }
    return out;
}

@vertex
fn building_skinned_vs(v: BuildingSkinnedVertexIn) -> BuildingObjectVertexOut {
    let s = bo_skin(v);
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    return bo_out(skin_place_point(s.pos), skin_place_dir(s.nrm), v.uv);
}

// ---- the casters: BuildingVS_Instanced_Shadow, BuildingSkinnedVS_Shadow ----

// The meshes with `Shadows` in their material go into the shadow map (S1a: the rocks of the stone
// deposit and the deer, events 413..522, `BuildingVS_Instanced_Shadow` and `BuildingSkinnedVS_Shadow`).
// The shadow pass gives the frame constants (the light's view-projection) and the bones only, so the
// point is projected as in the main pass and the pixel marks the caster, as `unit_shadow_ps` does;
// `BuildingPS_CODE_BASE_SHADOW` also reads the diffuse for its alpha test, which this pass has not.
struct BuildingShadowVertexOut {
    @builtin(position) clip: vec4<f32>,
}

// re: BuildingShader::BuildingVS_Instanced_Shadow, through `building_vs_instanced_shadow_body`: the
// rows of the instance's matrix (with the bone a rigid mesh sits on) are its v2..v5.
@vertex
fn building_object_shadow_vs(v: BuildingObjectVertexIn) -> BuildingShadowVertexOut {
    let w = rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend));
    var b: BuildingInstancedShadowVsIn;
    b.v0 = v.position;
    b.v1 = v.uv;
    b.v2 = vec4<f32>(w[0].x, w[1].x, w[2].x, 0.0);
    b.v3 = vec4<f32>(w[0].y, w[1].y, w[2].y, 0.0);
    b.v4 = vec4<f32>(w[0].z, w[1].z, w[2].z, 0.0);
    b.v5 = vec4<f32>(w[0].w, w[1].w, w[2].w, 1.0);
    var out: BuildingShadowVertexOut;
    out.clip = building_vs_instanced_shadow_body(b).o0;
    return out;
}

// re: BuildingShader::BuildingSkinnedVS_Shadow, through `building_skinned_vs_shadow_body`.
@vertex
fn building_skinned_shadow_vs(v: BuildingSkinnedVertexIn) -> BuildingShadowVertexOut {
    var b: BuildingSkinnedShadowVsIn;
    b.v0 = v.position;
    b.v1 = v.weights.xyz;
    b.v2 = vec3<f32>(v.joints.xyz);
    b.v3 = v.uv;
    let bone_p = bone_pose(v.frames, v.blend);
    b.bone_base = bone_p.base;
    b.bone_base_next = bone_p.next;
    b.bone_blend = bone_p.blend;
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    var out: BuildingShadowVertexOut;
    out.clip = building_skinned_vs_shadow_body(b).o0;
    return out;
}

// SIMPLIFIED(SIMP-064): a mark of the caster, no alpha test of `BuildingPS_CODE_BASE_SHADOW`.
@fragment
fn building_shadow_ps() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

// ---- BuildingPS ----

// pow(x, 2.2) as the shader does it: exp(log(x) * 2.2), per component.
fn bo_pow22(c: vec3<f32>) -> vec3<f32> {
    let l = vec3<f32>(d3d_log(c.x), d3d_log(c.y), d3d_log(c.z));
    return exp2(l * 2.20000005);
}

struct BoPsOut {
    rgb: vec3<f32>,
    // `texkill`: the texel is not drawn.
    kill: bool,
}

// re: BuildingShader::BuildingPS (sha1 18d02f5dc05f); the body of `building_ps_body`.
fn bo_ps_body(uv: vec2<f32>, pos: vec3<f32>, normal: vec3<f32>) -> BoPsOut {
    let c0 = vec4<f32>(2.20000005, -0.100000001, 199.0, 1.0);
    let c1 = vec4<f32>(0.0529000014, 0.846000016, 0.528999984, 0.454545468);
    let c3 = skin_reg(frame_globals.ViewInverseMatrix, 3u);

    let diffuse = textureSample(Diffuse, Diffuse_smp, uv);
    let irradiance = textureSample(Irradiance, Irradiance_smp, normal);
    let sref = textureSample(BuildingSREF, BuildingSREF_smp, uv);

    var out: BoPsOut;
    out.kill = diffuse.w + c0.y < 0.0;

    let base = bo_pow22(diffuse.xyz);
    let irr = bo_pow22(irradiance.xyz);
    var lit = base * irr;
    lit = lit + lit;

    let view = c3.xyz - pos;
    let inv_len = d3d_rsq(dot(view, view));
    let view_n = inv_len * view;
    let half_v = d3d_nrm(vec4<f32>(view * inv_len + c1.xyz, 0.0)).xyz;
    let n_dot_h = saturate(dot(half_v, normal));
    var k = dot(normal, view_n);
    k = k + k;
    let refl = k * normal - view_n;
    let soft = bo_pow22(textureSample(SpecSoft, SpecSoft_smp, refl).xyz);
    let sharp = bo_pow22(textureSample(SpecSharp, SpecSharp_smp, refl).xyz);

    let s_log = vec4<f32>(d3d_log(sref.x), d3d_log(sref.y), d3d_log(sref.z), d3d_log(sref.w)) * c0.x;
    let gloss = exp2(s_log.w);
    let spec_color = gloss * (sharp - soft) + soft;
    let power = gloss * c0.z + c0.w;
    let highlight = d3d_pow(n_dot_h, power);
    let strength = power * -c0.y;
    let spec = strength * highlight + spec_color;
    let sref_color = exp2(s_log.xyz);
    let sum = spec * sref_color + lit;

    let l = vec3<f32>(d3d_log(sum.x), d3d_log(sum.y), d3d_log(sum.z));
    out.rgb = exp2(l * c1.w);
    return out;
}

// `building_object_ps`, and `building_object_reflect_ps` that also drops what lies below the water.
fn building_object_ps_impl(in: BuildingObjectVertexOut, reflection: bool) -> vec4<f32> {
    let r = bo_ps_body(in.uv, in.pos, in.normal);
    let m = skin_fow_mask(in.pos.xy);
    if (r.kill || m.x < FOW_UNREVEALED || (reflection && reflection_clipped(in.pos))) {
        discard;
    }
    return vec4<f32>(skin_fow_color(r.rgb, m), 1.0);
}

@fragment
fn building_object_ps(in: BuildingObjectVertexOut) -> @location(0) vec4<f32> {
    return building_object_ps_impl(in, false);
}

@fragment
fn building_object_reflect_ps(in: BuildingObjectVertexOut) -> @location(0) vec4<f32> {
    return building_object_ps_impl(in, true);
}

// ---- the colour pass of the cities: BuildingPS_CODE_Opaque ----

// `f4FOWBaseScale` of the original's vertex stage: the base layer of the fog of war at a point of
// the world is `(xy + zw) * xy` (the clouds' scroll and repeat).
struct BuildingFowBase {
    f4FOWBaseScale: vec4<f32>,
}
@group(2) @binding(11) var<uniform> building_fow_base: BuildingFowBase;

struct BuildingObjectCodeOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // The point and the normal in the original's basis.
    @location(1) pos: vec3<f32>,
    // y / w of the clip position (`o3.y` of `BuildingVS`), the row of the distance fog ramp.
    @location(2) ndc_y: f32,
    @location(3) normal: vec3<f32>,
}

fn bo_code_out(p: vec3<f32>, n: vec3<f32>, uv: vec2<f32>) -> BuildingObjectCodeOut {
    let o = bo_out(p, n, uv);
    var out: BuildingObjectCodeOut;
    out.clip = o.clip;
    out.uv = o.uv;
    out.pos = o.pos;
    out.ndc_y = o.clip.y / o.clip.w;
    out.normal = o.normal;
    return out;
}

@vertex
fn building_object_code_vs(v: BuildingObjectVertexIn) -> BuildingObjectCodeOut {
    skin_world = rigid_world(v.world0, v.world1, v.world2, bone_ref_of(v.frames, v.blend));
    let p = skin_place_point(v.position);
    let n = skin_place_dir(skin_oct_decode(v.normal_oct));
    return bo_code_out(p, n, v.uv);
}

@vertex
fn building_skinned_code_vs(v: BuildingSkinnedVertexIn) -> BuildingObjectCodeOut {
    let s = bo_skin(v);
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    return bo_code_out(skin_place_point(s.pos), skin_place_dir(s.nrm), v.uv);
}

// The inputs of the bodies as the original's vertex stage makes them: the uv of the base layer from
// the point and `f4FOWBaseScale`, the place in the mask from the rectangle the mask covers
// (`skin/common.wgsl`).
fn bo_code_in(in: BuildingObjectCodeOut) -> BuildingCodePsIn {
    let s = building_fow_base.f4FOWBaseScale;
    let t = (in.pos.xy - skin_fow_rect.min) * skin_fow_rect.inv_size;
    var b: BuildingCodePsIn;
    b.v0 = vec4<f32>(in.uv, (in.pos.xy + s.zw) * s.xy);
    b.v1 = in.pos;
    b.v2 = vec2<f32>(0.0, in.ndc_y);
    b.v3 = vec2<f32>(t.x, 1.0 - t.y);
    b.v4 = in.normal;
    return b;
}

// The mask under its CTAB name (`skin_fow_mask` reads the same texture as `skin_fow_tex`, whose
// binding `FOWMask` shares); white without a mask.
fn bo_code_mask(b: BuildingCodePsIn) -> vec4<f32> {
    return select(
        vec4<f32>(1.0),
        textureSample(FOWMask, FOWMask_smp, b.v3),
        skin_fow_rect.inv_size.x > 0.0,
    );
}

// `BuildingPS_CODE_Opaque` (the materials that do not blend; S3: 232 draws). The texkill of
// `BuildingPS` (an alpha under 0.1) stays, for the cut-out meshes the opaque stage would fill; a
// point nobody has seen is not drawn (the clouds cover it), and the reflection twin cuts at the water
// plane.
fn building_object_code_ps_impl(in: BuildingObjectCodeOut, reflection: bool) -> vec4<f32> {
    let b = bo_code_in(in);
    let r = building_ps_code_opaque_body(b);
    let alpha = textureSample(Diffuse, Diffuse_smp, in.uv).w;
    let m = bo_code_mask(b);
    if (alpha < 0.1 || m.x < FOW_UNREVEALED || (reflection && reflection_clipped(in.pos))) {
        discard;
    }
    return vec4<f32>(r.oc0.xyz, 1.0);
}

@fragment
fn building_object_code_ps(in: BuildingObjectCodeOut) -> @location(0) vec4<f32> {
    return building_object_code_ps_impl(in, false);
}

@fragment
fn building_object_code_reflect_ps(in: BuildingObjectCodeOut) -> @location(0) vec4<f32> {
    return building_object_code_ps_impl(in, true);
}

// `BuildingPS_CODE` (the materials that blend; S3: 947 draws, `SrcAlpha`/`InvSrcAlpha`): its own
// texkill and alpha (the diffuse's raised to 2.2, half of it under the water).
fn building_object_code_alpha_ps_impl(in: BuildingObjectCodeOut, reflection: bool) -> vec4<f32> {
    let b = bo_code_in(in);
    let r = building_ps_code_body(b);
    let m = bo_code_mask(b);
    if (r.kill || m.x < FOW_UNREVEALED || (reflection && reflection_clipped(in.pos))) {
        discard;
    }
    return r.oc0;
}

@fragment
fn building_object_code_alpha_ps(in: BuildingObjectCodeOut) -> @location(0) vec4<f32> {
    return building_object_code_alpha_ps_impl(in, false);
}

@fragment
fn building_object_code_alpha_reflect_ps(in: BuildingObjectCodeOut) -> @location(0) vec4<f32> {
    return building_object_code_alpha_ps_impl(in, true);
}
