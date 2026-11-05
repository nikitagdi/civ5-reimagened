// The page and the highlight page of a cell as plain 2D textures (world-terrain WT-4): one draw a cell,
// the page bound as a view of its layer, as the original binds its render target. `screen_ps.wgsl`
// reads them through `cached_diffuse` and `cached_spec`; `page_array.wgsl` is the variant that reads
// the arrays of the pages, the layer given by the instance (all the cells in a few draws).

@group(1) @binding(0) var DiffuseCachedTexture: texture_2d<f32>;
@group(1) @binding(1) var DiffuseCachedTexture_smp: sampler;
@group(1) @binding(8) var SpecCachedTexture: texture_2d<f32>;
@group(1) @binding(9) var SpecCachedTexture_smp: sampler;

fn cached_diffuse(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(DiffuseCachedTexture, DiffuseCachedTexture_smp, uv);
}

fn cached_spec(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(SpecCachedTexture, SpecCachedTexture_smp, uv);
}
