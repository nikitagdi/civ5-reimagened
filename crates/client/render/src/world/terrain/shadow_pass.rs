//! "Macro Shadow" in the bake: the shadow of the relief on the page of a
//! cell, into `ShadowTemp`. `TerrainShaderScreenShadowVS` + `TerrainShaderShadowPS`
//! (`shaders/terrain/bake_shadow.wgsl`) march a ray over the tile of heights towards the sun; the pass
//! is drawn `TerrainShadowQuality` times, each moved by a pair of the jitter table (`FRandom`, seed 7)
//! and with the weight one over their number, added up. Quality 0 draws nothing: the target is cleared
//! and the page has no macro shadow. "Decal Macro Shadow" then draws the decals that cast a shadow over
//! it (`decal_pass.rs`), and "Terrain Cache" reads the target as the shadow of the page.
//!
//! The ray is 40 texels long and reads the tile of the cell; a ray that leaves the tile reads its edge
//! (the tile has an overlap of a texel only).
// TODO(RW-2): whether the macro shadow of the original reads the tile of the cell or a larger map

use super::build::rand::{JITTER_PAIRS, sample_jitter};
use super::cells::CellBox;
use super::heights_gpu::{HeightTiles, TILE_SIDE, tile_of_cell, tile_origin};
use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use crate::shaders::{catalog, create_module};

/// The target of the macro shadow.
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Bytes of the constants of the pixel stage (std140, 64) and of the vertex stage (16).
const PS_BYTES: usize = 64;
const VS_BYTES: usize = 16;
/// Bytes of a vertex of the quad: the corner in the clip space and the texel, two `vec4`.
const VERTEX_STRIDE: u64 = 32;

/// What the macro shadow reads of the world.
pub struct ShadowBake<'a> {
    /// The tiles of heights on the GPU.
    pub tiles: &'a HeightTiles,
    /// The direction to the sun in the original's basis (`g_SunDir`).
    pub sun_dir: [f32; 3],
    /// `TerrainShadowQuality`: how many passes.
    pub quality: u32,
    /// The world units of a texel and the height of raw 255 (`g_WorldScales`).
    pub world_scales: [f32; 3],
}

/// The pipeline and the resources of the pass.
pub struct ShadowPass {
    pipeline: wgpu::RenderPipeline,
    layout1: wgpu::BindGroupLayout,
    layout2: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    jitter: [[f32; 2]; JITTER_PAIRS],
}

fn uniform_entry(binding: u32, stage: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: stage,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

/// The constants of the pixel stage: `g_WorldScales`, `g_SunDir`, `g_HeightTextureScale`,
/// `g_f2WorldRebase`, `fShadowJitterAdd` (in that order, with the padding of std140).
pub fn ps_constants(
    scales: [f32; 3],
    sun: [f32; 3],
    tile_origin: (i32, i32),
    weight: f32,
) -> [f32; PS_BYTES / 4] {
    let mut c = [0.0f32; PS_BYTES / 4];
    c[0..3].copy_from_slice(&scales);
    c[4..7].copy_from_slice(&sun);
    let scale = 1.0 / TILE_SIDE as f32;
    c[8] = scale;
    c[9] = scale;
    // A texel of the field is read at its middle.
    c[10] = -(tile_origin.0 as f32) + 0.5;
    c[11] = -(tile_origin.1 as f32) + 0.5;
    c[14] = weight;
    c
}

impl ShadowPass {
    pub fn new(device: &wgpu::Device) -> Self {
        let m = catalog()
            .iter()
            .find(|m| m.name == "terrain_bake_shadow")
            .expect("the module terrain_bake_shadow is in the catalog");
        let module = create_module(device, "terrain_bake_shadow", &m.source());
        let layout1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain shadow ps"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
                uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
            ],
        });
        let layout2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain shadow vs"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain shadow"),
            bind_group_layouts: &[None, Some(&layout1), Some(&layout2)],
            immediate_size: 0,
        });
        let attributes = [0u32, 1].map(|i| wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: u64::from(i) * 16,
            shader_location: i,
        });
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Macro Shadow"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("terrain_shader_screen_shadow_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("terrain_shader_shadow_ps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SHADOW_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: add,
                        alpha: add,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain shadow"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout1,
            layout2,
            sampler,
            jitter: sample_jitter(),
        }
    }

    fn buffer(device: &wgpu::Device, floats: &[f32]) -> Buf {
        let bytes: Vec<u8> = floats.iter().flat_map(|f| f.to_le_bytes()).collect();
        let b = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("terrain shadow"),
                size: bytes.len().max(16) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::VERTEX,
                mapped_at_creation: true,
            },
            Cluster::Shadows,
        );
        if let Ok(mut m) = b.get_mapped_range_mut(..) {
            let mut padded = bytes.clone();
            padded.resize(m.len(), 0);
            m.copy_from_slice(&padded);
        }
        b.unmap();
        b
    }

    /// The pairs of the jitter table the passes use: pass `i` of `n` takes pair `i`.
    pub fn jitter_of(&self, pass: usize) -> [f32; 2] {
        self.jitter[pass.min(JITTER_PAIRS - 1)]
    }

    /// Draws the passes of the macro shadow of the cell `rect` into `shadow`, which is cleared first.
    /// Returns the number of passes drawn (0 when the quality is 0 or the tile is not there).
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        shadow: &wgpu::TextureView,
        size: u32,
        rect: &CellBox,
        bake: &ShadowBake<'_>,
    ) -> usize {
        let clear = wgpu::RenderPassColorAttachment {
            view: shadow,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        };
        let cell = (rect.x >> 6, rect.y >> 6);
        let tile_key = tile_of_cell(cell.0, cell.1);
        let tile = bake.tiles.tile(tile_key);
        let passes = bake.quality as usize;
        let (Some(tile), true) = (tile, passes > 0) else {
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Macro Shadow"),
                color_attachments: &[Some(clear)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
            return 0;
        };
        // The page covers the texels from one before the cell to one past its box.
        let (x0, x1) = (rect.x as f32 - 1.0, (rect.x + rect.w) as f32);
        let (y0, y1) = (rect.y as f32 - 1.0, (rect.y + rect.h) as f32);
        let corners = [
            (-1.0, 1.0, x0, y0),
            (1.0, 1.0, x1, y0),
            (1.0, -1.0, x1, y1),
            (-1.0, -1.0, x0, y1),
        ];
        let vertices: Vec<f32> = corners
            .iter()
            .flat_map(|&(cx, cy, tx, ty)| [cx, cy, 0.5, 1.0, tx, ty, 0.0, 0.0])
            .collect();
        let vb = Self::buffer(device, &vertices);
        let ib = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("terrain shadow quad"),
                size: 16,
                usage: wgpu::BufferUsages::INDEX,
                mapped_at_creation: true,
            },
            Cluster::Shadows,
        );
        {
            let mut m = ib.get_mapped_range_mut(..).expect("mapped");
            let idx: [u16; 6] = [0, 1, 2, 0, 2, 3];
            let mut bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
            bytes.resize(16, 0);
            m.copy_from_slice(&bytes);
        }
        ib.unmap();
        let ps = Self::buffer(
            device,
            &ps_constants(
                bake.world_scales,
                bake.sun_dir,
                tile_origin(tile_key),
                1.0 / passes as f32,
            ),
        );
        let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain shadow ps"),
            layout: &self.layout1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&tile.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: ps.as_entire_binding(),
                },
            ],
        });
        // The jitter of each pass, in the clip space of the page.
        let clip = 2.0 / size as f32;
        let groups2: Vec<(Buf, wgpu::BindGroup)> = (0..passes)
            .map(|i| {
                let j = self.jitter_of(i);
                let mut c = [0.0f32; VS_BYTES / 4];
                c[0] = j[0] * clip;
                c[1] = -j[1] * clip;
                let b = Self::buffer(device, &c);
                let g = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("terrain shadow vs"),
                    layout: &self.layout2,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: b.as_entire_binding(),
                    }],
                });
                (b, g)
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Macro Shadow"),
            color_attachments: &[Some(clear)],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(1, &group1, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint16);
        for (_, g) in &groups2 {
            pass.set_bind_group_c(2, g, &[]);
            pass.draw_indexed_c(0..6, 0, 0..1);
        }
        passes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_constants_hold_the_scales_the_sun_the_tile_and_the_share() {
        let c = ps_constants([1.0, 2.0, 0.2], [0.4, -0.25, 0.87], (1024, 0), 0.25);
        assert_eq!(&c[0..3], &[1.0, 2.0, 0.2]);
        assert_eq!(&c[4..7], &[0.4, -0.25, 0.87]);
        assert_eq!(c[8], 1.0 / 1025.0);
        assert_eq!((c[10], c[11]), (-1023.5, 0.5));
        assert_eq!(c[14], 0.25);
    }
}
