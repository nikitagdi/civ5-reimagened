// The cube of the example mod `hello`: 36 vertices made from the vertex index, no vertex buffer.
//
// Group 0 is the view of the main pass (`ModGpu::view_layout`, the engine's `ViewGlobals`); group 1 is the cube's
// own uniform. The world is the engine's basis for our code: X east, Y up, Z south.

struct ViewGlobals {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
    copy_offset: vec4<f32>,
    water_clip: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view_globals: ViewGlobals;

struct Cube {
    // xyz: the middle of the base, w: the side.
    place: vec4<f32>,
    color: vec4<f32>,
}

@group(1) @binding(0) var<uniform> cube: Cube;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) shade: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // Six vertices a face (two triangles of the square -1..1), six faces: +X, -X, +Y, -Y, +Z, -Z.
    let face = index / 6u;
    let corner = index % 6u;
    var square = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = square[corner];
    let axis = face / 2u;
    let sign = 1.0 - 2.0 * f32(face % 2u);
    var normal = vec3<f32>(0.0, 0.0, 0.0);
    normal[axis] = sign;
    var p = normal;
    p[(axis + 1u) % 3u] = c.x;
    p[(axis + 2u) % 3u] = c.y;
    let half = 0.5 * cube.place.w;
    let world = cube.place.xyz + vec3<f32>(0.0, half, 0.0) + p * half;
    var out: VsOut;
    out.clip = view_globals.view_proj * vec4<f32>(world, 1.0);
    let sun = normalize(vec3<f32>(0.4, 0.8, 0.3));
    out.shade = 0.45 + 0.55 * max(dot(normal, sun), 0.0);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(cube.color.rgb * in.shade, cube.color.a);
}
