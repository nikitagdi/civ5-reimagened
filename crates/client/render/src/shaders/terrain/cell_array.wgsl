// The entries of the batched cell draw (render-opt B3): `terrain_shader_vs` with the layers of the
// pages of the instance passed on flat, and the two pixel stages of the cached page reading the arrays
// (`page_array.wgsl`). The bodies are those of `cell_vs.wgsl` and `screen_ps.wgsl`.

struct CellArrayVertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec2<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec2<f32>,
    @location(4) @interpolate(flat) layers: vec2<u32>,
}

@vertex
fn terrain_shader_vs_array(
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec3<f32>,
    @location(2) v2: vec2<f32>,
    @location(3) offset: vec4<f32>,
    @location(4) layers: vec2<u32>,
) -> CellArrayVertexOut {
    var b: CellVsIn;
    b.v0 = v0;
    b.v1 = v1;
    b.v2 = v2;
    b.instance_offset = offset;
    let r = terrain_shader_vs_body(b);
    var out: CellArrayVertexOut;
    out.clip = r.o0;
    out.v0 = r.o2;
    out.v1 = r.o3.xy;
    out.v2 = r.o4.xyz;
    out.v3 = r.o5.xy;
    out.layers = layers;
    return out;
}

struct CachedArrayVaryings {
    @location(0) v0: vec4<f32>,
    @location(1) v1: vec2<f32>,
    @location(2) v2: vec3<f32>,
    @location(3) v3: vec2<f32>,
    @location(4) @interpolate(flat) layers: vec2<u32>,
}

@fragment
fn terrain_render_cached_diffuse_array_ps(in: CachedArrayVaryings) -> @location(0) vec4<f32> {
    if (cutout_hit_orig(in.v2)) {
        discard;
    }
    cell_page_layers = in.layers;
    var b: CachedPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    return terrain_render_cached_diffuse_ps_body(b).oc0;
}

@fragment
fn terrain_render_cached_diffuse_and_spec_array_ps(in: CachedArrayVaryings) -> @location(0) vec4<f32> {
    if (cutout_hit_orig(in.v2)) {
        discard;
    }
    cell_page_layers = in.layers;
    var b: CachedPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    return terrain_render_cached_diffuse_and_spec_ps_body(b).oc0;
}
