// The pixel entries of the cells on the screen with the cutout mask (client-hooks, D13): the ground
// over a cut column is not drawn. The bodies are those of `screen_ps.wgsl`, untouched, so the entries
// that port the original stay as they were.
//
// Needs cutout.wgsl and frame.wgsl.

// The point in the original's basis (the pixel stage gets it in `v2`) lies over a cut column. The
// translation of `ours_to_orig` is its column 3; a copy of the map to the east is already in `x`.
fn cutout_hit_orig(p: vec3<f32>) -> bool {
    let t = frame_globals.ours_to_orig[3];
    return cutout_hit(p.x - t.x, -(p.y - t.y));
}

@fragment
fn terrain_cell_diffuse_ps(in: CachedVaryings) -> @location(0) vec4<f32> {
    if (cutout_hit_orig(in.v2)) {
        discard;
    }
    var b: CachedPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    return terrain_render_cached_diffuse_ps_body(b).oc0;
}

@fragment
fn terrain_cell_diffuse_and_spec_ps(in: CachedVaryings) -> @location(0) vec4<f32> {
    if (cutout_hit_orig(in.v2)) {
        discard;
    }
    var b: CachedPsIn;
    b.v0 = in.v0;
    b.v1 = in.v1;
    b.v2 = in.v2;
    b.v3 = in.v3;
    return terrain_render_cached_diffuse_and_spec_ps_body(b).oc0;
}
