//! The map of the currents (world-water WW-2, RW-11): 256 x 256, `Rgba8Unorm`, drawn once by
//! `WaterSystem::RenderOceanCurrentMap` with `PSCombined` of `WorldCurrents` and read by the
//! surface as `WaterBlendMap`. It is drawn again only when the heights change (`WaterPrep`), and
//! only at `TerrainWaterQuality` above 0.
//!
//! The other entries of the family (`PSRiverCurrent`, `PSCurrent`, `PSCurlNoise`) are ported and
//! compared, but the original draws only `PSCombined` for the ocean.
// re: WaterSystem::RenderOceanCurrentMap (0x0868b9ee): one BeginCommandSet("WaterCurrent") with
// one DrawSurface(WorldCurrents SPI, pass 3 = PSCombined) into the map; the WorldCurrents SPI is
// created only in WaterSystem::Startup (0x0868ae38) and no river code (RiverVisSystem::Render,
// 0x088da5c8) touches it, so PSRiverCurrent (pass 0) is never drawn into the map.
// TODO(RW-12): PSRiverCurrent / PSCurrent / PSCurlNoise stay unused by the frame, as in the original.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};

use crate::frame::{ResourceDesc, ResourceId, TargetPool, TargetSize, targets::TargetError};
use crate::shaders::create_module;
use crate::uniform_block;

/// The target of the map.
pub const CURRENTS: ResourceId = ResourceId("WaterCurrents");

/// Its side.
// re: WaterSystem::RenderOceanCurrentMap (RW-11): 256 x 256
pub const CURRENTS_SIDE: u32 = 256;

/// Declares the target (`TerrainWaterQuality` above 0) or drops it.
pub fn declare_target(pool: &mut TargetPool, wanted: bool) -> Result<(), TargetError> {
    if wanted {
        pool.declare(
            CURRENTS,
            ResourceDesc::color(
                wgpu::TextureFormat::Rgba8Unorm,
                TargetSize::Absolute(CURRENTS_SIDE, CURRENTS_SIDE),
            ),
        )
    } else {
        pool.remove(CURRENTS);
        Ok(())
    }
}

// re: WaterSystem::RenderOceanCurrentMap (RW-11): the constants of the draw
pub const FLOW_STRENGTH: f32 = 0.1;
pub const NOISE_STRENGTH: f32 = 0.1;
pub const NOISE_SCALE: f32 = 2.0;

uniform_block! {
    /// `FireGrafix::WorldCurrentsData`: one register each.
    pub struct CurrentsConsts {
        pub GridSize: [f32; 2],
        pub pad2: [f32; 2],
        pub NoiseStrength: f32,
        pub pad5a: f32,
        pub pad5b: f32,
        pub pad5c: f32,
        pub NoiseScale: f32,
        pub pad9a: f32,
        pub pad9b: f32,
        pub pad9c: f32,
        pub FlowStrength: f32,
        pub pad13a: f32,
        pub pad13b: f32,
        pub pad13c: f32,
    }
}

impl CurrentsConsts {
    /// `grid`: the size of the padded game grid in hexes.
    pub fn new(grid: [f32; 2]) -> Self {
        Self {
            GridSize: grid,
            pad2: [0.0; 2],
            NoiseStrength: NOISE_STRENGTH,
            pad5a: 0.0,
            pad5b: 0.0,
            pad5c: 0.0,
            NoiseScale: NOISE_SCALE,
            pad9a: 0.0,
            pad9b: 0.0,
            pad9c: 0.0,
            FlowStrength: FLOW_STRENGTH,
            pad13a: 0.0,
            pad13b: 0.0,
            pad13c: 0.0,
        }
    }
}

/// `(x, y, z, u, v)`: a corner of the screen and its place in the height map.
type Vertex = [f32; 5];

/// One triangle that covers the target; `v` runs from the top as the rows of the height map do.
const TRIANGLE: [Vertex; 3] = [
    [-1.0, -1.0, 0.0, 0.0, 1.0],
    [3.0, -1.0, 0.0, 2.0, 1.0],
    [-1.0, 3.0, 0.0, 0.0, -1.0],
];

pub struct Currents {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    consts: Buf,
    triangle: Buf,
}

impl Currents {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = create_module(
            device,
            "water currents",
            crate::wgsl_module!("common/sm3.wgsl", "water/currents.wgsl"),
        );
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water currents"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<CurrentsConsts>() as u64,
                        ),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water currents"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water currents"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vsfallthrough"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("pscombined"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            // Wrap, linear: the map of the heights repeats along X.
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("water currents"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            consts: device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("water currents consts"),
                    size: std::mem::size_of::<CurrentsConsts>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            ),
            triangle: device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("water currents triangle"),
                    contents: bytemuck::cast_slice(&TRIANGLE),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Other,
            ),
        }
    }

    /// Draws the map into `target` from the heights (`height`, `R8Unorm`, row 0 the north).
    /// `grid` is the padded game grid in hexes. Returns the number of vertices drawn.
    pub fn record(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        height: &wgpu::TextureView,
        target: &wgpu::TextureView,
        grid: [f32; 2],
    ) -> u32 {
        queue.write_buffer(
            &self.consts,
            0,
            bytemuck::bytes_of(&CurrentsConsts::new(grid)),
        );
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water currents"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.consts.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(height),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("water currents"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(0, &group, &[]);
        pass.set_vertex_buffer(0, self.triangle.slice(..));
        pass.draw_c(0..3, 0..1);
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::{gpu, read_texture};
    use crate::world::water::ocean::HeightMap;

    #[test]
    fn the_block_is_the_64_bytes_of_the_original() {
        assert_eq!(std::mem::size_of::<CurrentsConsts>(), 64);
        let f = |n: &str| {
            <CurrentsConsts as crate::shaders::UniformBlock>::field(n)
                .unwrap()
                .offset
        };
        assert_eq!(
            [
                f("GridSize"),
                f("NoiseStrength"),
                f("NoiseScale"),
                f("FlowStrength")
            ],
            [0, 16, 32, 48]
        );
    }

    /// A hill in the middle of a 64 x 64 map.
    fn hill() -> HeightMap {
        let mut base = vec![0u8; 64 * 64];
        for y in 0..64usize {
            for x in 0..64usize {
                let d = (((x as f32 - 32.0).powi(2) + (y as f32 - 32.0).powi(2)).sqrt() * 4.0)
                    .min(120.0);
                base[y * 64 + x] = (120.0 - d) as u8 + 20;
            }
        }
        HeightMap {
            width: 64,
            height: 64,
            bytes: base,
        }
    }

    fn draw(g: &crate::gpu_test::Gpu) -> Vec<u8> {
        let currents = Currents::new(&g.device);
        let mut ocean = crate::world::water::ocean::Ocean::new(&g.device, &g.queue, 1);
        ocean.set_height(&hill());
        let target = g.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("currents"),
            size: wgpu::Extent3d {
                width: CURRENTS_SIDE,
                height: CURRENTS_SIDE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut encoder = g.device.create_command_encoder(&Default::default());
        let height = ocean.height_view().expect("height");
        assert_eq!(
            currents.record(
                &g.device,
                &g.queue,
                &mut encoder,
                height,
                &view,
                [64.0, 64.0]
            ),
            3
        );
        g.queue.submit([encoder.finish()]);
        read_texture(g, &target, 0, 0, (CURRENTS_SIDE, CURRENTS_SIDE), 4)
    }

    #[test]
    fn the_map_of_the_currents_is_the_same_twice_and_is_not_flat() {
        let Some(g) = gpu(
            "water currents",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let a = draw(&g);
        let b = draw(&g);
        assert_eq!(a, b, "two runs differ");
        // The hill pushes the water away: the red and green channels are not all one value.
        let reds: std::collections::BTreeSet<u8> = a.chunks(4).map(|p| p[0]).collect();
        let greens: std::collections::BTreeSet<u8> = a.chunks(4).map(|p| p[1]).collect();
        assert!(
            reds.len() > 4 && greens.len() > 4,
            "{} {}",
            reds.len(),
            greens.len()
        );
    }
}
