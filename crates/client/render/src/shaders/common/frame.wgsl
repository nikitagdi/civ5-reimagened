// Group 0: the constants of one view. One uniform buffer per view; a map
// copy of a wrapping map is a view of its own with the matrix shifted by the map width.
//
// The layout is that of `frame::views::ViewGlobals` (112 bytes). `FrameGlobals` (sun, fog, time,
// `ours_to_orig`) joins it with RC-7; the names will be those of CTAB.
//
// Our basis is X east, Y up, Z south; the original's is x east, y north, z up.

struct ViewGlobals {
    view_proj: mat4x4<f32>,
    // Eye in the object space of this copy: the real eye minus the shift.
    eye: vec4<f32>,
    // (shift, 0, 0, 0): where this copy lies relative to the middle one.
    copy_offset: vec4<f32>,
    // (height, on, 0, 0): the water plane the reflection pass cuts at, in the original's basis
    // (z up); `on` is 0 in every other view.
    water_clip: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view_globals: ViewGlobals;

// The reflection variants of the shaders `discard` where this is true (WebGL2 has no clip plane;
// the D3D9 pass used `SetClipPlane`). `p` is a world position in the original's basis.
fn below_water_clip(p: vec3<f32>) -> bool {
    return view_globals.water_clip.y > 0.5 && p.z < view_globals.water_clip.x;
}

// The same cut for the entries whose group 0 is the frame constants only (the objects of
// `world-objects`, the buildings): their `*_reflect_ps` variants discard below the water level of
// the frame (`fWaterHeight`, the height `reflection::CLIP_HEIGHT`). `p` is a world position in the
// original's basis. (An `override` would do for the unit module, but the pipeline constants of a
// module that has none make naga validate its bodies again, and the building body does not pass.)
fn reflection_clipped(p: vec3<f32>) -> bool {
    return p.z < frame_globals.fWaterHeight;
}

// The rotation B of `ours_to_orig`: our (x, y, z) -> the original's (x, -z, y). The translation T
// of the map comes with the frame globals.
fn orig_from_ours_rotation(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(v.x, -v.z, v.y);
}

// Group 0, binding 1: the constants of one frame, the layout of
// `lighting::FrameGlobals` (416 bytes). Names are those of the CTAB where they are known. Matrices
// except `ours_to_orig` are stored transposed: a shader reads them as `v * M` (the original's
// `row_major float4x4`).
struct FrameGlobals {
    // Our basis to the original's (rotation B and the translation T of the map); `M * vec4(p, 1)`.
    ours_to_orig: mat4x4<f32>,
    ViewProjMatrix: mat4x4<f32>,
    // Row 3 is the eye.
    ViewInverseMatrix: mat4x4<f32>,
    g_ShadowVP: mat4x4<f32>,
    g_ReflectVP: mat4x4<f32>,
    f3CameraWorld: vec3<f32>,
    g_Time: f32,
    g_FogConsts: vec4<f32>,
    f3FogColor: vec4<f32>,
    g_SunDir: vec3<f32>,
    g_ShadowFade: f32,
    g_ReflectDir: vec3<f32>,
    g_ReflectAmount: f32,
    g_ReflectCameraDir: vec3<f32>,
    // The level of the water, original's basis (`UnitAlphaPS`).
    fWaterHeight: f32,
}

@group(0) @binding(1) var<uniform> frame_globals: FrameGlobals;
