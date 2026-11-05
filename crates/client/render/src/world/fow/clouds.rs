//! The cloud layers of the fluid fog (world-overlays WOV1, `FluidFOWSystem::RenderFogLayer`): three
//! layers of 64 x 64 tiles at 56, 191.25 and 255 times the terrain scale, drawn after the scene in
//! `Main.FluidFow` with `VSFogBaseLayer` and `BasePS` (`shaders/fow/base.wgsl`), 24 tiles to a
//! draw. A pixel of a tile is as opaque as the cloud texture there times the mask's cloud channel
//! at the same world position: the clouds show over the unrevealed hexes and nowhere else.
//!
//! The texture scrolls by `-2`, `-3` and `-4` world units a second and repeats `floor(0.002 W)`
//! times over the map width `W`. Tiles over culled cells only are left out (`cull.rs`).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2, Vec3};

use super::cull::CullGrid;
use super::mask::{BATCH, MAX_INSTANCES, MaskView, TILE, row_major};
use super::state::{CLOUD_LAYERS, CloudLayer, cloud_uv_scale};
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT, targets::Target};
use crate::shaders::create_module;

/// Tiles of one layer at most (`kMaxFogTiles`).
pub const MAX_TILES: usize = MAX_INSTANCES;
/// Bytes of the constants of one draw: `FOWBaseWVP[24]`, `FOWBaseWorld[24]`, `FOWOverlay`,
/// `FOWBaseScale`.
pub const BLOCK_BYTES: usize = BATCH * 64 * 2 + 64 + 16;
/// Distance between two draws in the uniform buffer: the block, up to the offset alignment.
pub const BLOCK_STRIDE: usize = BLOCK_BYTES.div_ceil(256) * 256;
/// Draws of the three layers at most.
const MAX_DRAWS: usize = 3 * MAX_TILES.div_ceil(BATCH);

/// `g_TerrainScale.z`: the heights of the layers are this times 56, 191.25 and 255.
// `m_vTerrainScale.z` of the capture (S1a terrain cb1[16] = (1, 0.866, 0.2, 0)); the cloud layers of S1a ev
// 2194..2256 sit at 0.2 x (56, 191.25, 255) = 11.2, 38.25 and 51.0.
pub const TERRAIN_SCALE_Z: f32 = crate::world::terrain::sampler::TERRAIN_SCALE_Z;

/// The height of a layer above the plane z = 0 of the original's basis.
pub fn layer_height(layer: &CloudLayer) -> f32 {
    TERRAIN_SCALE_Z * layer.height
}

/// `FOWBaseScale` of a layer: the repeats per world unit in x and y, and the scroll in x and y at
/// `time` seconds (`xy`, `zw`).
pub fn base_scale(world_width: f32, time: f32, layer: &CloudLayer) -> [f32; 4] {
    let s = cloud_uv_scale(world_width);
    [s, s, time * layer.scroll_x, 0.0]
}

/// The tiles of the rectangle, by their centre in the original's basis, without those that lie
/// over culled cells. Returns the centres and the number that were left out.
pub fn visible_tiles(
    view: &MaskView,
    ours_to_orig: &Mat4,
    cull: Option<&CullGrid>,
) -> (Vec<Vec2>, u32) {
    let to_ours = ours_to_orig.inverse();
    let nx = (view.size.x / TILE).round() as usize;
    let ny = (view.size.y / TILE).round() as usize;
    let mut tiles = Vec::new();
    let mut skipped = 0;
    for j in 0..ny {
        for i in 0..nx {
            let lo = view.min + Vec2::new(i as f32, j as f32) * TILE;
            let centre = lo + Vec2::splat(TILE * 0.5);
            if let Some(cull) = cull {
                let corners = [lo, lo + Vec2::splat(TILE)].map(|c| {
                    let p = to_ours.transform_point3(c.extend(0.0));
                    Vec2::new(p.x, p.z)
                });
                if cull.tile_is_culled(corners[0].min(corners[1]), corners[0].max(corners[1])) {
                    skipped += 1;
                    continue;
                }
            }
            tiles.push(centre);
        }
    }
    (tiles, skipped)
}

/// The constants of one draw (`FogLayerConstants`), padded to [`BLOCK_STRIDE`]: the tiles of
/// `centres` (at most 24) at `height`, seen through `view_proj_orig` (`clip = G * point` of the
/// original's basis), the mask through `overlay` (the projection of the mask's rectangle).
pub fn layer_block(
    view_proj_orig: &Mat4,
    overlay: &Mat4,
    scale: [f32; 4],
    height: f32,
    centres: &[Vec2],
) -> Vec<u8> {
    let mut out = vec![0u8; BLOCK_STRIDE];
    for (i, c) in centres.iter().take(BATCH).enumerate() {
        // The quad of the mesh is [-1, 1]: a tile is half a tile wide.
        let world = Mat4::from_translation(Vec3::new(c.x, c.y, height))
            * Mat4::from_scale(Vec3::new(TILE * 0.5, TILE * 0.5, 1.0));
        let wvp = *view_proj_orig * world;
        out[i * 64..(i + 1) * 64].copy_from_slice(bytemuck::bytes_of(&row_major(&wvp)));
        let at = BATCH * 64 + i * 64;
        out[at..at + 64].copy_from_slice(bytemuck::bytes_of(&row_major(&world)));
    }
    let at = BATCH * 128;
    out[at..at + 64].copy_from_slice(bytemuck::bytes_of(&row_major(overlay)));
    out[at + 64..at + 80].copy_from_slice(bytemuck::bytes_of(&scale));
    out
}

/// A vertex of the tile quad: `dcl_position` and `dcl_normal` (z: the tile among the 24 of a draw).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TileVertex {
    pub position: [f32; 4],
    pub slot: [f32; 4],
}

/// The 24 quads of a draw.
pub fn tile_quads() -> (Vec<TileVertex>, Vec<u16>) {
    let mut vertices = Vec::with_capacity(4 * BATCH);
    let mut indices = Vec::with_capacity(6 * BATCH);
    for slot in 0..BATCH {
        let base = vertices.len() as u16;
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            vertices.push(TileVertex {
                position: [x, y, 0.0, 1.0],
                slot: [0.0, 0.0, slot as f32, 0.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    (vertices, indices)
}

#[derive(Clone, Copy)]
struct LayerDraw {
    layer: usize,
    first_block: u32,
    tiles: u32,
}

/// What a frame of clouds is made from.
pub struct CloudFrame<'a> {
    /// The camera in the original's basis ([`view_proj_orig`]).
    pub view_proj_orig: Mat4,
    pub view: MaskView,
    /// The centres of the tiles ([`visible_tiles`]) and how many were left out.
    pub tiles: &'a [Vec2],
    pub culled: u32,
    /// Width of the map in world units.
    pub world_width: f32,
    pub time: f32,
}

/// What the last `upload` made of the frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudCounters {
    pub tiles: u32,
    pub tiles_culled: u32,
    pub draws: u32,
}

/// The pipeline and buffers of the cloud layers.
pub struct FowClouds {
    pipeline: wgpu::RenderPipeline,
    /// The same tiles into the class mask pass, writing `FOG_BIT` where the clouds show.
    mask_pipeline: wgpu::RenderPipeline,
    texture_layout: wgpu::BindGroupLayout,
    uniform: Buf,
    consts_group: wgpu::BindGroup,
    vertices: Buf,
    indices: Buf,
    cloud_sampler: wgpu::Sampler,
    mask_sampler: wgpu::Sampler,
    textures: Option<[wgpu::TextureView; 3]>,
    /// The bind groups of the three layers, made for the generation of the mask target they hold.
    groups: Option<(u64, [wgpu::BindGroup; 3])>,
    draws: Vec<LayerDraw>,
    counters: CloudCounters,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

impl FowClouds {
    pub fn new(device: &wgpu::Device, samples: u32) -> Self {
        let module = create_module(
            device,
            "fow base layer",
            crate::wgsl_module!("fow/base.wgsl"),
        );
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fow base constants"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(BLOCK_BYTES as u64),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fow base textures"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fow base layer"),
            bind_group_layouts: &[Some(&consts_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fow base layer"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("fogbaselayer_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TileVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            // The clouds are behind what stands in front of them and write no depth.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: crate::frame::multisample(samples),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("baseps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mask_module = create_module(
            device,
            "fow base mask",
            crate::wgsl_module!("fow/base.wgsl", "fow/base_mask.wgsl"),
        );
        let constants = [("MASK_CLASS", f64::from(crate::frame::mask::class::FOG_BIT))];
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fow base mask"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &mask_module,
                entry_point: Some("fogbaselayer_vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<TileVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &mask_module,
                entry_point: Some("fogbase_mask_ps"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::frame::mask::MASK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("fow base constants"),
                size: (BLOCK_STRIDE * MAX_DRAWS) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fow base constants"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &uniform,
                    offset: 0,
                    size: wgpu::BufferSize::new(BLOCK_BYTES as u64),
                }),
            }],
        });
        let (v, i) = tile_quads();
        let vertices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("fow tile vertices"),
                contents: bytemuck::cast_slice(&v),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        );
        let indices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("fow tile indices"),
                contents: bytemuck::cast_slice(&i),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        );
        let cloud_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fow cloud sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // The vertex shader offsets the mask's coordinates by -0.5 and relies on the wrap.
        let mask_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fow mask sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            mask_pipeline,
            texture_layout,
            uniform,
            consts_group,
            vertices,
            indices,
            cloud_sampler,
            mask_sampler,
            textures: None,
            groups: None,
            draws: Vec::new(),
            counters: CloudCounters::default(),
        }
    }

    pub fn counters(&self) -> CloudCounters {
        self.counters
    }

    /// The three textures (`CLOUD_LAYERS`) have arrived: the clouds can be drawn.
    pub fn has_textures(&self) -> bool {
        self.textures.is_some()
    }

    /// The texture of the lowest layer: what the original binds as `FOWBaseCached` (terrain) and
    /// `FOWBaseLayer` (trees), the resource of the first `BasePS` draw (S1a ev 819 t5, 1383 t7,
    /// 2194 t0).
    pub fn base_view(&self) -> Option<&wgpu::TextureView> {
        self.textures.as_ref().map(|t| &t[0])
    }

    /// Takes the cloud textures from `blobs`; with one missing there are no clouds.
    pub fn set_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blobs: &std::collections::HashMap<asset_format::LogicalId, asset_format::TextureBlob>,
    ) -> Result<(), String> {
        let mut views = Vec::new();
        for layer in CLOUD_LAYERS {
            let blob = blobs
                .get(&asset_format::LogicalId::new(layer.texture))
                .ok_or_else(|| format!("{} is not loaded", layer.texture))?;
            let tex = crate::texture::upload(device, queue, blob, layer.texture)?;
            views.push(tex.create_view(&wgpu::TextureViewDescriptor::default()));
        }
        let Ok(views) = <[wgpu::TextureView; 3]>::try_from(views) else {
            return Err("three cloud textures expected".to_owned());
        };
        self.textures = Some(views);
        self.groups = None;
        Ok(())
    }

    /// Writes the constants of the frame and plans the draws.
    pub fn upload(&mut self, queue: &wgpu::Queue, frame: &CloudFrame<'_>) {
        self.draws.clear();
        let mut bytes: Vec<u8> = Vec::new();
        let overlay = frame.view.projection();
        let tiles = &frame.tiles[..frame.tiles.len().min(MAX_TILES)];
        if self.textures.is_some() && !tiles.is_empty() {
            for (li, layer) in CLOUD_LAYERS.iter().enumerate() {
                let first_block = (bytes.len() / BLOCK_STRIDE) as u32;
                let scale = base_scale(frame.world_width, frame.time, layer);
                for chunk in tiles.chunks(BATCH) {
                    bytes.extend_from_slice(&layer_block(
                        &frame.view_proj_orig,
                        &overlay,
                        scale,
                        layer_height(layer),
                        chunk,
                    ));
                }
                self.draws.push(LayerDraw {
                    layer: li,
                    first_block,
                    tiles: tiles.len() as u32,
                });
            }
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.uniform, 0, &bytes);
        }
        self.counters = CloudCounters {
            tiles: tiles.len() as u32,
            tiles_culled: frame.culled,
            draws: (bytes.len() / BLOCK_STRIDE) as u32,
        };
    }

    /// Makes the bind groups for the mask target, if the target is not the one they hold.
    pub fn bind_mask(&mut self, device: &wgpu::Device, mask: &Target) {
        let Some(textures) = &self.textures else {
            return;
        };
        if self
            .groups
            .as_ref()
            .is_some_and(|(generation, _)| *generation == mask.generation)
        {
            return;
        }
        let group = |t: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fow base textures"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(t),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.cloud_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&mask.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.mask_sampler),
                    },
                ],
            })
        };
        let groups = [
            group(&textures[0]),
            group(&textures[1]),
            group(&textures[2]),
        ];
        self.groups = Some((mask.generation, groups));
    }

    /// Draws the layers into `pass`. Returns the sizes of the indexed draws (for the counters);
    /// empty when nothing is drawn.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> Vec<u32> {
        self.draw_with(pass, &self.pipeline)
    }

    /// Draws the layers into the class mask pass.
    pub fn draw_mask(&self, pass: &mut wgpu::RenderPass<'_>) -> Vec<u32> {
        self.draw_with(pass, &self.mask_pipeline)
    }

    fn draw_with(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
    ) -> Vec<u32> {
        let mut sizes = Vec::new();
        let Some((_, groups)) = &self.groups else {
            return sizes;
        };
        if self.draws.is_empty() {
            return sizes;
        }
        pass.set_pipeline_c(pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        for d in &self.draws {
            pass.set_bind_group_c(1, &groups[d.layer], &[]);
            let mut left = d.tiles as usize;
            let mut block = d.first_block as usize;
            while left > 0 {
                let n = left.min(BATCH);
                pass.set_bind_group_c(0, &self.consts_group, &[(block * BLOCK_STRIDE) as u32]);
                pass.draw_indexed_c(0..(n * 6) as u32, 0, 0..1);
                sizes.push((n * 6) as u32);
                left -= n;
                block += 1;
            }
        }
        sizes
    }
}

/// The camera in the original's basis: `clip = G * point`.
pub fn view_proj_orig(view_proj: &Mat4, ours_to_orig: &Mat4) -> Mat4 {
    *view_proj * ours_to_orig.inverse()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Fog;
    use crate::world::fow::cull::cull_grid;
    use crate::world::fow::state::FogInput;

    #[test]
    fn a_block_is_the_constants_of_the_original_padded_to_the_offset_alignment() {
        assert_eq!(BLOCK_BYTES, 3152);
        assert_eq!(BLOCK_STRIDE, 3328);
        assert_eq!(BLOCK_STRIDE % 256, 0);
        assert_eq!(MAX_DRAWS, 3 * 86);
    }

    #[test]
    fn the_layers_are_at_the_heights_of_the_original() {
        let h: Vec<f32> = CLOUD_LAYERS.iter().map(layer_height).collect();
        assert!((h[0] - 11.2).abs() < 1e-3, "{h:?}");
        assert!((h[1] - 38.25).abs() < 1e-3);
        assert!((h[2] - 51.0).abs() < 1e-3);
        // Between the water (10.12) and a plain (12.0) at the lowest, above the hills.
        assert!(h[0] > 10.12 && h[0] < 12.0 + 1.0);
    }

    #[test]
    fn the_scale_repeats_the_texture_and_scrolls_it_by_the_layer() {
        let w = 80.0 * 64.0;
        let s0 = base_scale(w, 10.0, &CLOUD_LAYERS[0]);
        let s2 = base_scale(w, 10.0, &CLOUD_LAYERS[2]);
        // floor(0.002 * 5120) = 10 repeats over the map.
        assert!((s0[0] * w - 10.0).abs() < 1e-4);
        assert_eq!(s0[0], s0[1]);
        assert_eq!((s0[2], s0[3]), (-20.0, 0.0));
        assert_eq!(s2[2], -40.0);
    }

    #[test]
    fn the_quads_have_the_slot_of_their_tile() {
        let (v, i) = tile_quads();
        assert_eq!((v.len(), i.len()), (4 * BATCH, 6 * BATCH));
        assert!(v[4 * 7..4 * 8].iter().all(|p| p.slot[2] == 7.0));
        assert!(i.iter().all(|&k| (k as usize) < v.len()));
    }

    #[test]
    fn a_block_puts_the_tile_where_the_world_matrix_says() {
        let vp = Mat4::IDENTITY;
        let overlay = MaskView::snapped(Vec2::ZERO, Vec2::splat(128.0)).projection();
        let block = layer_block(
            &vp,
            &overlay,
            [0.01, 0.01, -2.0, 0.0],
            11.0,
            &[Vec2::new(96.0, 32.0)],
        );
        assert_eq!(block.len(), BLOCK_STRIDE);
        // Element 0 of FOWBaseWorld: register 3 is the translation, register 0 the scale.
        let world: &[[f32; 4]; 4] = bytemuck::from_bytes(&block[BATCH * 64..BATCH * 64 + 64]);
        assert_eq!([world[0][3], world[1][3], world[2][3]], [96.0, 32.0, 11.0]);
        assert_eq!(world[0][0], 32.0);
        assert_eq!(world[1][1], 32.0);
        // Element 1 is unused: zero.
        assert!(block[64..128].iter().all(|&b| b == 0));
        // The scale constant is last.
        let scale: &[f32; 4] = bytemuck::from_bytes(&block[BATCH * 128 + 64..BATCH * 128 + 80]);
        assert_eq!(scale, &[0.01, 0.01, -2.0, 0.0]);
    }

    #[test]
    fn tiles_over_culled_cells_are_left_out() {
        let f = FogInput::new(24, 24, false, {
            let mut v = vec![Fog::Visible; 24 * 24];
            v[2 * 24 + 2] = Fog::Unrevealed;
            v
        })
        .unwrap();
        let to_orig = crate::frame::basis_rotation();
        let view = MaskView::snapped(Vec2::new(0.0, 0.0), Vec2::new(24.0 * 64.0, 20.0 * 55.4));
        let (all, none) = visible_tiles(&view, &to_orig, None);
        assert_eq!(none, 0);
        assert_eq!(
            all.len(),
            (view.size.x / 64.0 * view.size.y / 64.0) as usize
        );
        let (some, skipped) = visible_tiles(&view, &to_orig, Some(&cull_grid(&f)));
        assert_eq!(some.len() + skipped as usize, all.len());
        assert!(skipped > 0 && !some.is_empty());
        // What is left lies over the fogged corner or the edge of the map.
        assert!(some.iter().any(|t| t.x < 3.0 * 64.0 && t.y < 5.0 * 64.0));
    }

    #[test]
    fn a_camera_in_the_original_basis_is_the_one_of_ours() {
        let to_orig = crate::frame::basis_rotation();
        let vp = Mat4::from_scale(Vec3::new(0.01, 0.02, 0.003))
            * Mat4::from_translation(Vec3::new(3.0, -4.0, 5.0));
        let g = view_proj_orig(&vp, &to_orig);
        let ours = Vec3::new(10.0, 3.0, -20.0);
        let orig = to_orig.transform_point3(ours);
        let a = vp.project_point3(ours);
        let b = g.project_point3(orig);
        assert!((a - b).length() < 1e-4, "{a:?} {b:?}");
    }
}
