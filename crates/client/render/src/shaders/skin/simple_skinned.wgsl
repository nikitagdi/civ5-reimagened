// SimpleShader, the skinned vertex shader of the original:
// `Skinned_SimpleVS_Skinned`. Hand port in the convention of shader-re 4.8.1 (a body
// `simple_skinned_vs_body` with the registers of the original as member names, and an entry).
//
// The palette of this shader is `row_major float4x4 BoneMatrices[32]`: four registers a bone,
// `c16 + 4 * index`, and the point is `x * c16 + y * c17 + z * c18 + c19` (the registers are the
// columns of the 3x4 matrix, the fourth the translation). The bone texture holds the 3x4 as three
// rows (texels), so the register `o` of the palette (`c16 + o`) is the column `o % 4` of the bone
// `o / 4`: `(row0[c], row1[c], row2[c])`. A valid index is a multiple of four (`4 * index`); an
// odd one reads across two bones, as the original would, and gets the slow path.
//
// The comparison with the oracle fills the palette by the data move `bone_texture_cols4`: the four
// registers of a bone become its three rows. The world matrix of the instance is put on the point
// and the normal afterwards (`skin_place_*`), as in `unit_skinned.wgsl`.
//
// Needs frame.wgsl, skin.wgsl (render-core) and skin/common.wgsl. Vertex: POSITION0 0, NORMAL0 1
// (oct snorm16), TEXCOORD0 2, BLENDINDICES0 14, BLENDWEIGHT0 15; the instance as in
// `unit_skinned.wgsl` at the free locations 3..8.

// The column `o % 4` of the bone `o / 4` of the frame: the register `c16 + o`, xyz.
fn simple_reg(base: u32, next: u32, blend: f32, o: u32) -> vec3<f32> {
    let t = 3u * (o / 4u);
    let c = o % 4u;
    let r0 = palette_row(base, next, blend, t);
    let r1 = palette_row(base, next, blend, t + 1u);
    let r2 = palette_row(base, next, blend, t + 2u);
    return vec3<f32>(r0[c], r1[c], r2[c]);
}

// The four registers `c16[a0]`..`c19[a0]`.
struct SimpleCols {
    c0: vec3<f32>,
    c1: vec3<f32>,
    c2: vec3<f32>,
    c3: vec3<f32>,
}

fn simple_cols(base: u32, next: u32, blend: f32, a0: u32) -> SimpleCols {
    if (a0 % 4u == 0u) {
        // The bone is whole: three rows give the four columns.
        let t = 3u * (a0 / 4u);
        let r0 = palette_row(base, next, blend, t);
        let r1 = palette_row(base, next, blend, t + 1u);
        let r2 = palette_row(base, next, blend, t + 2u);
        return SimpleCols(
            vec3<f32>(r0.x, r1.x, r2.x),
            vec3<f32>(r0.y, r1.y, r2.y),
            vec3<f32>(r0.z, r1.z, r2.z),
            vec3<f32>(r0.w, r1.w, r2.w),
        );
    }
    return SimpleCols(
        simple_reg(base, next, blend, a0),
        simple_reg(base, next, blend, a0 + 1u),
        simple_reg(base, next, blend, a0 + 2u),
        simple_reg(base, next, blend, a0 + 3u),
    );
}

struct SimpleSkinnedVsIn {
    // dcl_position v0.
    v0: vec3<f32>,
    // dcl_blendweight v1: the four weights.
    v1: vec4<f32>,
    // dcl_blendindices v2: the four bones, as numbers.
    v2: vec4<f32>,
    // dcl_normal v3.
    v3: vec3<f32>,
    // dcl_texcoord v4: (u, v).
    v4: vec2<f32>,
    // The palette `BoneMatrices` (c16..c143) is the bone texture (data move).
    bone_base: u32,
    bone_base_next: u32,
    bone_blend: f32,
}

struct SimpleSkinnedVsOut {
    o0: vec4<f32>,
    o1: vec4<f32>,
    o2: vec3<f32>,
    o3: vec3<f32>,
    o4: vec4<f32>,
    o5: vec4<f32>,
    o6: vec4<f32>,
}

// re: SimpleShader::Skinned_SimpleVS_Skinned (sha1 11b175a2ecd4)
fn simple_skinned_vs_body(inp: SimpleSkinnedVsIn) -> SimpleSkinnedVsOut {
    // def c4, 4, 1, 0, 0: four registers a bone.
    let four = 4.0;
    let one = 1.0;
    let zero = 0.0;
    // c0..c3 = ViewProjMatrix, c11 = ViewInverseMatrix+3.
    let c0 = skin_reg(frame_globals.ViewProjMatrix, 0u);
    let c1 = skin_reg(frame_globals.ViewProjMatrix, 1u);
    let c2 = skin_reg(frame_globals.ViewProjMatrix, 2u);
    let c3 = skin_reg(frame_globals.ViewProjMatrix, 3u);
    let c11 = skin_reg(frame_globals.ViewInverseMatrix, 3u);
    let b = inp.bone_base;
    let n = inp.bone_base_next;
    let k = inp.bone_blend;
    let v0 = inp.v0;
    let v3 = inp.v3;

    let a0x = skin_mova(four * inp.v2.x);
    let a0y = skin_mova(four * inp.v2.y);
    let a0z = skin_mova(four * inp.v2.z);
    let a0w = skin_mova(four * inp.v2.w);
    let m0 = simple_cols(b, n, k, a0x);
    let m1 = simple_cols(b, n, k, a0y);
    let m2 = simple_cols(b, n, k, a0z);
    let m3 = simple_cols(b, n, k, a0w);

    // The normal: x * c16 + y * c17 + z * c18 of each bone, weighted.
    var r0 = v3.y * m0.c1;
    r0 = v3.x * m0.c0 + r0;
    r0 = v3.z * m0.c2 + r0;
    var r1 = v3.y * m1.c1;
    r1 = v3.x * m1.c0 + r1;
    r1 = v3.z * m1.c2 + r1;
    r1 = r1 * inp.v1.y;
    r0 = inp.v1.x * r0 + r1;
    r1 = v3.y * m2.c1;
    r1 = v3.x * m2.c0 + r1;
    r1 = v3.z * m2.c2 + r1;
    r0 = inp.v1.z * r1 + r0;
    r1 = v3.y * m3.c1;
    r1 = v3.x * m3.c0 + r1;
    r1 = v3.z * m3.c2 + r1;
    let normal = skin_place_dir(inp.v1.w * r1 + r0);

    // The point: the same and the translation c19.
    r0 = v0.y * m0.c1;
    r0 = v0.x * m0.c0 + r0;
    r0 = v0.z * m0.c2 + r0;
    r0 = r0 + m0.c3;
    r1 = v0.y * m1.c1;
    r1 = v0.x * m1.c0 + r1;
    r1 = v0.z * m1.c2 + r1;
    r1 = r1 + m1.c3;
    r1 = r1 * inp.v1.y;
    r0 = inp.v1.x * r0 + r1;
    r1 = v0.y * m2.c1;
    r1 = v0.x * m2.c0 + r1;
    r1 = v0.z * m2.c2 + r1;
    r1 = r1 + m2.c3;
    r0 = inp.v1.z * r1 + r0;
    r1 = v0.y * m3.c1;
    r1 = v0.x * m3.c0 + r1;
    r1 = v0.z * m3.c2 + r1;
    r1 = r1 + m3.c3;
    r0 = inp.v1.w * r1 + r0;
    let p = skin_place_point(r0);

    var q = p.y * c1;
    q = p.x * c0 + q;
    q = p.z * c2 + q;
    var o: SimpleSkinnedVsOut;
    o.o0 = q + c3;
    o.o5 = vec4<f32>(p - c11.xyz, zero);
    o.o4 = vec4<f32>(p, zero);
    o.o1 = vec4<f32>(one * inp.v4.x, one * inp.v4.y, zero, zero);
    o.o2 = normal;
    o.o3 = vec3<f32>(zero);
    o.o6 = vec4<f32>(one);
    return o;
}

struct SimpleSkinnedVertexIn {
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

struct SimpleSkinnedVertexOut {
    @builtin(position) clip: vec4<f32>,
    // `SimplePS_Skinned` reads the uv (o1.xy) and the normal as the vector into the light cube (o2).
    @location(0) v0: vec2<f32>,
    @location(1) v1: vec3<f32>,
}

@vertex
fn simple_skinned_vs(v: SimpleSkinnedVertexIn) -> SimpleSkinnedVertexOut {
    var b: SimpleSkinnedVsIn;
    b.v0 = v.position;
    b.v1 = v.weights;
    b.v2 = vec4<f32>(v.joints);
    b.v3 = skin_oct_decode(v.normal_oct);
    b.v4 = v.uv;
    let bone_p = bone_pose(v.frames, v.blend);
    b.bone_base = bone_p.base;
    b.bone_base_next = bone_p.next;
    b.bone_blend = bone_p.blend;
    skin_world = skin_instance_world(v.world0, v.world1, v.world2);
    let r = simple_skinned_vs_body(b);
    var out: SimpleSkinnedVertexOut;
    out.clip = r.o0;
    out.v0 = r.o1.xy;
    out.v1 = r.o2;
    return out;
}
