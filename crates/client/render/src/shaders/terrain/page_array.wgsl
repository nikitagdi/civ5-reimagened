// The page and the highlight page of a cell as layers of the arrays of the pages (render-opt B3): the
// cells of one array are drawn in one instanced draw, the layers come from the instance
// (`cell_array.wgsl` puts them into `cell_page_layers` before it calls the body). A layer of an array
// is sampled as the 2D view of that layer is: the same size, so the same mip and the same filter, and no
// filtering across layers. The variant of `page_2d.wgsl`.

@group(1) @binding(0) var DiffuseCachedTexture: texture_2d_array<f32>;
@group(1) @binding(1) var DiffuseCachedTexture_smp: sampler;
@group(1) @binding(8) var SpecCachedTexture: texture_2d_array<f32>;
@group(1) @binding(9) var SpecCachedTexture_smp: sampler;

// The layer of the page (x) and of the highlight page (y) of the cell being drawn.
var<private> cell_page_layers: vec2<u32>;

fn cached_diffuse(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(DiffuseCachedTexture, DiffuseCachedTexture_smp, uv, cell_page_layers.x);
}

fn cached_spec(uv: vec2<f32>) -> vec4<f32> {
    return textureSample(SpecCachedTexture, SpecCachedTexture_smp, uv, cell_page_layers.y);
}
