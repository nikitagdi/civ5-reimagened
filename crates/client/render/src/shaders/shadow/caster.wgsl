// What casts a shadow: the static models (trees, units, cities) drawn into the raw shadow map from
// the light, alpha 1 where something stands. Not a port: the
// original's casters are the terrain's own passes, which come with world-terrain.
//
// Group 0: the matrix `ShadowVP` of the light for points of our basis (`clip = vp * p`).
// Vertices and instances are those of the `static` pipeline: only the fields used here are bound.

struct CasterConsts {
    vp: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> caster: CasterConsts;

struct CasterIn {
    @location(0) position: vec3<f32>,
    @location(3) variant: u32,
    // Instance: xyz position, w rotation about Y.
    @location(4) pos_rot: vec4<f32>,
    // Instance: uniform scale.
    @location(5) scale: f32,
    @location(8) inst_variant: u32,
}

@vertex
fn caster_vs(v: CasterIn) -> @builtin(position) vec4<f32> {
    let c = cos(v.pos_rot.w);
    let s = sin(v.pos_rot.w);
    let p = v.position * v.scale;
    let local = vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
    var clip = caster.vp * vec4<f32>(local + v.pos_rot.xyz, 1.0);
    if (v.variant != v.inst_variant) {
        // The vertices of another variant collapse outside the volume.
        clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    }
    return clip;
}

@fragment
fn caster_ps() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, 1.0);
}

// ---- the debug view (`?debug=shadow`) ----

@group(0) @binding(1) var DebugMap: texture_2d<f32>;
@group(0) @binding(2) var DebugMap_smp: sampler;

struct DebugOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn debug_vs(@builtin(vertex_index) i: u32) -> DebugOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: DebugOut;
    o.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

// The shadow as grey: white where something shades.
@fragment
fn debug_ps(in: DebugOut) -> @location(0) vec4<f32> {
    let a = textureSample(DebugMap, DebugMap_smp, in.uv).w;
    return vec4<f32>(a, a, a, 1.0);
}
