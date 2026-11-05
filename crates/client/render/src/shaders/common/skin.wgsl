// Skinning from the bone texture: `bone_matrix(base, k)`, `skin_position`,
// `skin_normal`, up to 4 weights per vertex.
//
// The texture is `Rgba32Float`, 1536 texels wide: 512 bones a row, 3 texels a bone, one per row of
// the 3 x 4 matrix (the last column is the translation). A bone slot `s` lies at texel
// `((s % 512) * 3, s / 512)`; `skin::bone_texel` is the same formula. An instance gives the slot of
// its first bone as `bone_base`; for blending it also gives `bone_base_next` and `bone_blend`, and
// the matrix of bone `k` is `mix(M(base, k), M(next, k), blend)`. What `bone_blend` means is for
// `world-anim` to say.
//
// Group 1, binding 14 is the texture. The vertex stage reads it with `textureLoad` (`texelFetch`
// in WebGL2); there are no storage buffers.

@group(1) @binding(14) var bone_tex: texture_2d<f32>;

// A bone: the three rows of its matrix.
struct Bone {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
}

const BONES_PER_ROW: u32 = 512u;
const TEXELS_PER_BONE: u32 = 3u;

fn bone_texel(slot: u32) -> vec2<i32> {
    return vec2<i32>(i32((slot % BONES_PER_ROW) * TEXELS_PER_BONE), i32(slot / BONES_PER_ROW));
}

// The bone `k` of the instance whose first bone is in slot `base`.
fn bone_matrix(base: u32, k: u32) -> Bone {
    let t = bone_texel(base + k);
    return Bone(
        textureLoad(bone_tex, t, 0),
        textureLoad(bone_tex, t + vec2<i32>(1, 0), 0),
        textureLoad(bone_tex, t + vec2<i32>(2, 0), 0),
    );
}

// ---- the clock of the animation (B3 packet F, `anim::gpu_clock`) ----
//
// An instance that plays one clip steadily gives, in place of two frames and a blend, the slot of
// the header of its play (`frames.x`), the flag and the slot of the clock (`frames.y`) and the time
// the play began (`blend`). The header bone is `first slot, bones, frames, fps` and `duration,
// mode, repeat, pause`; the clock bone's first float is the time of the service. The frames of the
// moment are found as `AnimServiceOf::step` finds them on the CPU (`clip_time`, `Cycle::time`,
// `bone_ref::locate_at`). Any other instance passes through as it is.

const BONE_CLOCK_FLAG: u32 = 0x80000000u;

struct BonePose {
    base: u32,
    next: u32,
    blend: f32,
}

// `rem_euclid` of Rust: the remainder with the sign of `d` (> 0 here).
fn bone_rem_euclid(x: f32, d: f32) -> f32 {
    let r = x % d;
    return select(r, r + d, r < 0.0);
}

fn bone_pose(frames: vec2<u32>, blend: f32) -> BonePose {
    if ((frames.y & BONE_CLOCK_FLAG) == 0u) {
        return BonePose(frames.x, frames.y, blend);
    }
    let h = bone_texel(frames.x);
    let a = textureLoad(bone_tex, h, 0);
    let b = textureLoad(bone_tex, h + vec2<i32>(1, 0), 0);
    let now = textureLoad(bone_tex, bone_texel(frames.y & ~BONE_CLOCK_FLAG), 0).x;
    let first = u32(a.x);
    let bones = u32(a.y);
    let count = u32(a.z);
    let fps = a.w;
    let duration = b.x;
    let elapsed = now - blend;
    var t = 0.0;
    if (duration > 0.0) {
        if (b.y > 1.5) {
            // A landmark's cycle: the clip `repeat` times, then its last pose for `pause` s.
            let play = duration * b.z;
            let phase = bone_rem_euclid(elapsed, play + b.w);
            t = select(duration, phase % duration, phase < play);
        } else if (b.y > 0.5) {
            t = bone_rem_euclid(elapsed, duration);
        } else {
            t = clamp(elapsed, 0.0, duration);
        }
    }
    if (count < 2u) {
        return BonePose(first, first, 0.0);
    }
    let last = count - 1u;
    let x = max(t * fps, 0.0);
    let k = min(u32(floor(x)), last - 1u);
    let base = first + k * bones;
    return BonePose(base, base + bones, clamp(x - f32(k), 0.0, 1.0));
}

fn bone_scale(b: Bone, s: f32) -> Bone {
    return Bone(b.r0 * s, b.r1 * s, b.r2 * s);
}

fn bone_add(a: Bone, b: Bone) -> Bone {
    return Bone(a.r0 + b.r0, a.r1 + b.r1, a.r2 + b.r2);
}

// `mix(M(base, k), M(next, k), blend)`; the second read is left out when there is no blend.
fn bone_matrix_blend(base: u32, next: u32, blend: f32, k: u32) -> Bone {
    let a = bone_matrix(base, k);
    if (blend <= 0.0) {
        return a;
    }
    return bone_add(bone_scale(a, 1.0 - blend), bone_scale(bone_matrix(next, k), blend));
}

// The matrix of a vertex: the weighted sum of its (up to) four bones. A weight of zero reads nothing.
fn skin_matrix(joints: vec4<u32>, weights: vec4<f32>, base: u32, next: u32, blend: f32) -> Bone {
    var m = bone_scale(bone_matrix_blend(base, next, blend, joints.x), weights.x);
    if (weights.y > 0.0) {
        m = bone_add(m, bone_scale(bone_matrix_blend(base, next, blend, joints.y), weights.y));
    }
    if (weights.z > 0.0) {
        m = bone_add(m, bone_scale(bone_matrix_blend(base, next, blend, joints.z), weights.z));
    }
    if (weights.w > 0.0) {
        m = bone_add(m, bone_scale(bone_matrix_blend(base, next, blend, joints.w), weights.w));
    }
    return m;
}

fn skin_position(m: Bone, p: vec3<f32>) -> vec3<f32> {
    let h = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(m.r0, h), dot(m.r1, h), dot(m.r2, h));
}

// The direction part of the matrix only; uniform scale assumed, as in B1, so no inverse transpose.
fn skin_normal(m: Bone, n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(m.r0.xyz, n), dot(m.r1.xyz, n), dot(m.r2.xyz, n));
}
