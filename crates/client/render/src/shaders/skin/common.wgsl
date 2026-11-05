// Skinning from the baked clips of `world-anim`, on top of `common/skin.wgsl`
// of render-core (4.12). Needs frame.wgsl and skin.wgsl; the file has no entry points.
//
// The bone texture holds, for every bone of a baked frame, the skin matrix `world * inverse_bind`
// of the model in its own space (3 texels, the rows of the 3x4). An instance names two frames
// (`BoneRef`: the first bone of each) and a blend; the matrix of bone `k` is
// `mix(M(base, k), M(next, k), blend)` (SIMP-050: the matrices are blended, not the quaternions).
//
// The skinning shaders of the original find the matrix of a bone in the registers of a palette that
// already has the object's world matrix inside (`c8[a0.x]`, `mova` of `3 * index`). Ours keep the
// formula and change the source: the palette is read from the bone texture, and the world matrix
// of the instance is put on the result afterwards, once a vertex (`skin_place_*`). Both are the
// same by linearity: `W * sum(w_i * B_i * p) = sum(w_i * (W * B_i) * p)`.

// The pose of an instance: the `BoneRef` of `world-anim` (12 bytes in the instance).
struct BoneRef {
    base: u32,
    next: u32,
    blend: f32,
}

// A rigid mesh on a bone (`Mesh.bind = k`, WA-D8): the matrix of bone `k` of the instance. The rigid
// vertex shaders of `world-objects` put `world * bind_matrix(bones, k)` on the vertex under
// `HAS_BIND`.
// The `BoneRef` of the instance attributes `frames` and `blend`: a clocked one (B3 packet F) is
// turned into the frames of the moment (`bone_pose`), once a vertex.
fn bone_ref_of(frames: vec2<u32>, blend: f32) -> BoneRef {
    let p = bone_pose(frames, blend);
    return BoneRef(p.base, p.next, p.blend);
}

fn bind_matrix(bones: BoneRef, bind: u32) -> Bone {
    return bone_matrix_blend(bones.base, bones.next, bones.blend, bind);
}

// ---- the palette as the shaders of the original read it ----
//
// `t` counts texels from the first bone of the frame: the register `c<first>[a0.x]` with
// `a0.x = 3 * index` is the texel `3 * index` for a 3-register-a-bone palette (UnitShader), so the
// bone `k` is where `skin.wgsl` puts it. Any `t` is a valid texel, as any `a0.x` is a valid
// register: an odd index reads across two bones, as the original would.

fn palette_texel(first_slot: u32, t: u32) -> vec4<f32> {
    let n = first_slot * 3u + t;
    return textureLoad(bone_tex, vec2<i32>(i32(n % 1536u), i32(n / 1536u)), 0);
}

// `mix(texel of the frame `base`, texel of the frame `next`, blend)`; the second read is left out
// when there is no blend.
fn palette_row(base: u32, next: u32, blend: f32, t: u32) -> vec4<f32> {
    let a = palette_texel(base, t);
    if (blend <= 0.0) {
        return a;
    }
    return a * (1.0 - blend) + palette_texel(next, t) * blend;
}

// `mova a0.x, src`: the register index, rounded to the nearest (`floor(x + 0.5)`), as the oracle does.
fn skin_mova(x: f32) -> u32 {
    return u32(max(floor(x + 0.5), 0.0));
}

// ---- the world matrix of the instance, put on the skinned vertex ----
//
// Row `j` of `skin_world` is the register `c(8 + j)` of the original (x, y, z, translation of the
// object-to-world transform). It is the identity until an entry sets it, which is what the oracle
// comparison needs: the palette of the original has the world inside already.

var<private> skin_world: mat3x4<f32> = mat3x4<f32>(
    vec4<f32>(1.0, 0.0, 0.0, 0.0),
    vec4<f32>(0.0, 1.0, 0.0, 0.0),
    vec4<f32>(0.0, 0.0, 1.0, 0.0),
);

fn skin_place_point(p: vec3<f32>) -> vec3<f32> {
    let h = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(skin_world[0], h), dot(skin_world[1], h), dot(skin_world[2], h));
}

fn skin_place_dir(n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(skin_world[0].xyz, n), dot(skin_world[1].xyz, n), dot(skin_world[2].xyz, n));
}

// The rows of the 3x4 matrix of an `ObjectInstance` (our basis) as the registers of the original
// (its basis): `ours_to_orig * M`.
fn skin_instance_world(w0: vec4<f32>, w1: vec4<f32>, w2: vec4<f32>) -> mat3x4<f32> {
    let m = mat4x4<f32>(
        vec4<f32>(w0.x, w1.x, w2.x, 0.0),
        vec4<f32>(w0.y, w1.y, w2.y, 0.0),
        vec4<f32>(w0.z, w1.z, w2.z, 0.0),
        vec4<f32>(w0.w, w1.w, w2.w, 1.0),
    );
    let o = frame_globals.ours_to_orig * m;
    return mat3x4<f32>(
        vec4<f32>(o[0][0], o[1][0], o[2][0], o[3][0]),
        vec4<f32>(o[0][1], o[1][1], o[2][1], o[3][1]),
        vec4<f32>(o[0][2], o[1][2], o[2][2], o[3][2]),
    );
}

// ---- a rigid mesh on a bone (WA-D8) ----
//
// The rigid vertex shaders of `world-objects` (units, landmarks, simple models) draw a mesh that
// sits on an animated bone with `world * bind_matrix(bones, k)`. The call says so by the
// `override`s: one mesh, one call, one bone.

override HAS_BIND: bool = false;
override BIND_BONE: u32 = 0u;

// `w * b`: both are 3x4 (the rows of the matrix, the last column is the translation); the result
// is the matrix that does `b` first.
fn skin_mul_world(w: mat3x4<f32>, b: Bone) -> mat3x4<f32> {
    return mat3x4<f32>(
        w[0].x * b.r0 + w[0].y * b.r1 + w[0].z * b.r2 + vec4<f32>(0.0, 0.0, 0.0, w[0].w),
        w[1].x * b.r0 + w[1].y * b.r1 + w[1].z * b.r2 + vec4<f32>(0.0, 0.0, 0.0, w[1].w),
        w[2].x * b.r0 + w[2].y * b.r1 + w[2].z * b.r2 + vec4<f32>(0.0, 0.0, 0.0, w[2].w),
    );
}

// The world matrix of a rigid object in the original's basis: `ours_to_orig * M`, and the bone
// `BIND_BONE` of the instance first when the call sits on one.
fn rigid_world(w0: vec4<f32>, w1: vec4<f32>, w2: vec4<f32>, bones: BoneRef) -> mat3x4<f32> {
    var w = skin_instance_world(w0, w1, w2);
    if (HAS_BIND) {
        w = skin_mul_world(w, bind_matrix(bones, BIND_BONE));
    }
    return w;
}

// The register `k` of a `row_major float4x4` that was filled by CTAB name (reg k = row k).
fn skin_reg(m: mat4x4<f32>, k: u32) -> vec4<f32> {
    return vec4<f32>(m[0][k], m[1][k], m[2][k], m[3][k]);
}

// Octahedral snorm16 normal of the vertex.
fn skin_oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e.x, e.y, 1.0 - abs(e.x) - abs(e.y));
    if (n.z < 0.0) {
        let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>((1.0 - abs(n.yx)) * s, n.z);
    }
    return normalize(n);
}

// ---- the fog of war on a model (FowMask contract of world-overlays) ----
//
// The pixel entries of the objects of `world-objects` read the mask `FowMask` (the target of the pass
// of that name) where the point stands: channel x is how far the ground is revealed, z how far it is
// seen now (y, the clouds, is not read). Revealed but not seen, the colour is the one B1 gives a
// hidden unit (`FOW_HIDDEN_SATURATION`, `FOW_HIDDEN_FACTOR`); not revealed, the model is not drawn.
// The rectangle of the mask is in the original's basis, as the point (`MaskView`).
//
// Group 2, bindings 1..3: binding 0 of the group is the fog-state texture of B1's entries
// (`unit.wgsl`), which the entries of `world-objects` do not use.

struct SkinFowRect {
    // The corner of the mask rectangle (x east, y north).
    min: vec2<f32>,
    // One over its size; zero when there is no mask (the model is then not dimmed).
    inv_size: vec2<f32>,
}

@group(2) @binding(1) var skin_fow_tex: texture_2d<f32>;
@group(2) @binding(2) var skin_fow_smp: sampler;
@group(2) @binding(3) var<uniform> skin_fow_rect: SkinFowRect;

const FOW_HIDDEN_SATURATION: f32 = 0.55;
const FOW_HIDDEN_FACTOR: f32 = 0.6;
// Below this much revealed, the point is on ground nobody has seen.
const FOW_UNREVEALED: f32 = 0.01;

// The mask at a point of the world (original's basis). White (everything seen) without a mask.
fn skin_fow_mask(world_xy: vec2<f32>) -> vec4<f32> {
    let t = (world_xy - skin_fow_rect.min) * skin_fow_rect.inv_size;
    let m = textureSample(skin_fow_tex, skin_fow_smp, vec2<f32>(t.x, 1.0 - t.y));
    return select(vec4<f32>(1.0), m, skin_fow_rect.inv_size.x > 0.0);
}

// The colour of a point the mask `m` covers: dimmed where it is revealed and not seen.
fn skin_fow_color(rgb: vec3<f32>, m: vec4<f32>) -> vec3<f32> {
    let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let hidden = mix(vec3<f32>(grey), rgb, FOW_HIDDEN_SATURATION) * FOW_HIDDEN_FACTOR;
    return mix(hidden, rgb, saturate(m.z));
}
