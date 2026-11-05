//! Mip chains for render targets the client bakes itself (terrain cache pages and the like).
//!
//! One render pass per level and layer: a full-screen triangle samples the level above with a
//! bilinear filter, which at exactly half size is the 2×2 average (the D3D9 auto-mip filter).
//! No compute, so it runs on WebGPU and WebGL2. Textures from the
//! game bring their own mips from the DDS and never come through here.
//!
//! WebGL2 cannot view one layer of an array as a plain 2D texture, so an array is sampled whole
//! (`texture_2d_array`) and the layer comes from a uniform.

use crate::perf::PassCount;
use crate::perf::{Cluster, DeviceTrack};
use std::collections::HashMap;

const COMMON: &str = "
struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@group(0) @binding(1) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    // One triangle covering the target: (0,0) (2,0) (0,2) in uv space.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VOut;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
";

const PLAIN: &str = "
@group(0) @binding(0) var src: texture_2d<f32>;

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, smp, in.uv, 0.0);
}
";

const ARRAY: &str = "
struct Params { layer: vec4<u32> }
@group(0) @binding(0) var src: texture_2d_array<f32>;
@group(0) @binding(2) var<uniform> params: Params;

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, smp, in.uv, i32(params.layer.x), 0.0);
}
";

/// Bind group layout, pipeline layout and shader of one source kind (plain 2D or array).
struct Kind {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
}

impl Kind {
    fn new(device: &wgpu::Device, array: bool) -> Self {
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: if array {
                        wgpu::TextureViewDimension::D2Array
                    } else {
                        wgpu::TextureViewDimension::D2
                    },
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        if array {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            });
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mips"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mips"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let source = format!("{COMMON}{}", if array { ARRAY } else { PLAIN });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mips"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        Self {
            layout,
            pipeline_layout,
            module,
        }
    }
}

/// Fills mip levels `1..levels` of a texture from level 0, one pass per level and layer.
/// Pipelines are cached per target format.
pub struct MipGenerator {
    plain: Kind,
    array: Kind,
    sampler: wgpu::Sampler,
    pipelines: HashMap<(wgpu::TextureFormat, bool), wgpu::RenderPipeline>,
}

impl MipGenerator {
    pub fn new(device: &wgpu::Device) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("mips"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            plain: Kind::new(device, false),
            array: Kind::new(device, true),
            sampler,
            pipelines: HashMap::new(),
        }
    }

    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, array: bool) {
        let kind = if array { &self.array } else { &self.plain };
        self.pipelines.entry((format, array)).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mips"),
                layout: Some(&kind.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &kind.module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &kind.module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
    }

    /// Records the passes into `encoder`: for each of `layers` array layers, levels `1..levels`
    /// from the level above. The texture needs `TEXTURE_BINDING | RENDER_ATTACHMENT`, a
    /// filterable renderable colour format and at least `levels` mips.
    pub fn generate(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        layers: u32,
        levels: u32,
    ) -> Result<(), String> {
        let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        if !texture.usage().contains(usage) {
            return Err(format!("mip generation needs texture usage {usage:?}"));
        }
        if texture.dimension() != wgpu::TextureDimension::D2 {
            return Err("mip generation handles 2D textures and 2D arrays only".to_owned());
        }
        if levels > texture.mip_level_count() || layers > texture.depth_or_array_layers() {
            return Err(format!(
                "{levels} levels x {layers} layers asked of a texture with {} x {}",
                texture.mip_level_count(),
                texture.depth_or_array_layers()
            ));
        }
        let format = texture.format();
        let array = texture.depth_or_array_layers() > 1;
        self.pipeline(device, format, array);
        let pipeline = &self.pipelines[&(format, array)];
        let kind = if array { &self.array } else { &self.plain };
        let dst_view = |level: u32, layer: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("mips"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: level,
                mip_level_count: Some(1),
                base_array_layer: layer,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        for layer in 0..layers {
            // The layer to read, for the array variant.
            // Only the array variant reads it.
            let params = array.then(|| {
                let params = device.buf(
                    &wgpu::BufferDescriptor {
                        label: Some("mips"),
                        size: 16,
                        usage: wgpu::BufferUsages::UNIFORM,
                        mapped_at_creation: true,
                    },
                    Cluster::Other,
                );
                if let Ok(mut m) = params.get_mapped_range_mut(..) {
                    let mut bytes = [0u8; 16];
                    bytes[..4].copy_from_slice(&layer.to_le_bytes());
                    m.copy_from_slice(&bytes);
                }
                params.unmap();
                params
            });
            for level in 1..levels {
                let src = texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("mips"),
                    dimension: Some(if array {
                        wgpu::TextureViewDimension::D2Array
                    } else {
                        wgpu::TextureViewDimension::D2
                    }),
                    base_mip_level: level - 1,
                    mip_level_count: Some(1),
                    ..Default::default()
                });
                let dst = dst_view(level, layer);
                let mut entries = vec![
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ];
                if let Some(params) = &params {
                    entries.push(wgpu::BindGroupEntry {
                        binding: 2,
                        resource: params.as_entire_binding(),
                    });
                }
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("mips"),
                    layout: &kind.layout,
                    entries: &entries,
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("mips"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline_c(pipeline);
                pass.set_bind_group_c(0, &bind, &[]);
                pass.draw_c(0..3, 0..1);
            }
        }
        Ok(())
    }
}

/// One-shot form: builds a [`MipGenerator`], records the passes and drops it. Keep a
/// `MipGenerator` instead when generating more than once.
pub fn generate(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    layers: u32,
    levels: u32,
) -> Result<(), String> {
    MipGenerator::new(device).generate(device, encoder, texture, layers, levels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::{Gpu, gpu, read_texture};

    const SIDE: u32 = 16;
    const LAYERS: u32 = 2;
    const LEVELS: u32 = 5;

    fn noise(seed: u32, len: usize) -> Vec<u8> {
        let mut s = seed | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as u8
            })
            .collect()
    }

    fn target(g: &Gpu, usage: wgpu::TextureUsages) -> wgpu::Texture {
        g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("mips test"),
            size: wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: LAYERS,
            },
            mip_level_count: LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        })
    }

    /// Fills level 0 of each layer with noise, generates, and checks every level is the 2x2
    /// average of the one above (to 1/255: the hardware filter rounds its own way).
    fn check(g: &Gpu, label: &str) {
        let texture = target(
            g,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        );
        for layer in 0..LAYERS {
            g.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &noise(layer + 3, (SIDE * SIDE * 4) as usize),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIDE * 4),
                    rows_per_image: Some(SIDE),
                },
                wgpu::Extent3d {
                    width: SIDE,
                    height: SIDE,
                    depth_or_array_layers: 1,
                },
            );
        }
        let mut enc = g
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        generate(&g.device, &mut enc, &texture, LAYERS, LEVELS).unwrap();
        g.queue.submit([enc.finish()]);
        for layer in 0..LAYERS {
            for level in 1..LEVELS {
                let side = SIDE >> level;
                let up = SIDE >> (level - 1);
                let above = read_texture(g, &texture, level - 1, layer, (up, up), 4);
                let got = read_texture(g, &texture, level, layer, (side, side), 4);
                for y in 0..side as usize {
                    for x in 0..side as usize {
                        for c in 0..4 {
                            let at = |dx: usize, dy: usize| {
                                u32::from(above[((2 * y + dy) * up as usize + 2 * x + dx) * 4 + c])
                            };
                            let avg = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4;
                            let v = u32::from(got[(y * side as usize + x) * 4 + c]);
                            assert!(
                                v.abs_diff(avg) <= 1,
                                "{label}: layer {layer} level {level} ({x},{y}) c{c}: {v} vs {avg}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn generate_averages_2x2_on_the_default_backend() {
        if let Some(g) = gpu(
            "generate_averages_2x2_on_the_default_backend",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) {
            check(&g, &format!("{:?}", g.info.backend));
        }
    }

    /// The WebGL2 path: the GL backend, which is what the browser uses.
    #[test]
    fn generate_averages_2x2_on_gl() {
        if let Some(g) = gpu(
            "generate_averages_2x2_on_gl",
            wgpu::Backends::GL,
            wgpu::Features::empty(),
        ) {
            check(&g, "gl");
        }
    }

    #[test]
    fn generate_rejects_a_texture_it_cannot_render_to() {
        let Some(g) = gpu(
            "generate_rejects_a_texture_it_cannot_render_to",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let texture = target(&g, wgpu::TextureUsages::TEXTURE_BINDING);
        let mut enc = g
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        assert!(generate(&g.device, &mut enc, &texture, 1, 3).is_err());
    }
}
