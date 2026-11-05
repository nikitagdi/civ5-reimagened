//! The GPU part of the river water (world-water WW-4): the pipelines of `VSWaterRiver` with
//! `PSRiverHigh` / `PSRiverLow`, the buffers of the ribbon, and the bind group. The constants block,
//! the samplers and most textures are the ocean's (`ocean.rs`), so the ocean hands them over in a
//! [`RiverFeed`] (`Water::river_feed`).
//!
//! The shader works in the world of the original; the mesh already is in it, so the matrix `World`
//! of the draw is the identity.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use glam::Mat4;

use super::mesh::{RiverMesh, RiverVertex};
use crate::frame::mask::{MaskDepth, MaskPipelineDesc, MaskShader, class};
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT, multisample};
use crate::shaders::create_module;
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::ocean::{
    CONSTS_STRIDE, COPIES, OceanInputs, OceanMap, WaterConsts, WaterFrame, copy_slot, one_texel,
    sampler_entry, texture_entry,
};

/// `RiverScale`: the repeats of the LEAN layers per unit of the mesh's flow `U` (texels / 100) and
/// across `V` (`x`, `y`), then the factors of layer 1 and layer 2 (`z`, `w`).
// re: S2 ev 1032 (`VSWaterRiver`), cb1 floats 40..43 of `WaterGlobalData` = (1.7, 1.2, 1.7, 1.2)
const RIVER_SCALE: [f32; 4] = [1.7, 1.2, 1.7, 1.2];
/// The scroll of `RiverOffset` (the `c12` of `WaterGlobalData`) per second of the water time: `x, y` is the
/// scroll of layer 1 in (`U`, `V`), `z, w` that of layer 2; the layers flow in `V` too.
/// `ScrollPhaseB` grows by 0.03 a second and the offsets are `ScrollPhaseB * sqrt 2 / (1.8, 1.5, 7.2,
/// -1.2)`. The captures agree: S1a (ev 1104) has (0.386162, 0.463394, 0.0965405, -0.579243) at the
/// `ScrollPhaseA` 11.7961 (`WaterOffset` / `WaterScale.x`), which is 11.7961 / 0.72 seconds.
// re: WaterSystem::Update 0x0868bbae, TerrainSystem.cpp:1308, 1412, 1606-1609 (re-b16 O5)
const FLOW_RATE: [f32; 4] = [
    0.03 * std::f32::consts::SQRT_2 / 1.8,
    0.03 * std::f32::consts::SQRT_2 / 1.5,
    0.03 * std::f32::consts::SQRT_2 / 7.2,
    0.03 * std::f32::consts::SQRT_2 / -1.2,
];
/// `RiverOffset` at the water time `t` (seconds). The layers flow downstream, where `U` falls: the
/// coordinate they are read at grows.
fn river_offset(t: f32) -> [f32; 4] {
    FLOW_RATE.map(|r| r * t)
}

/// The opacity of the stand-in for the river's `WaterBlendMap`.
// TODO(RW-12): the river's own blend map (`WaterBlendMap.w` is the alpha of the river).
const BLEND_ALPHA: u8 = 204;

/// The textures and maps the rivers read, as the surface has them. Equal values mean the bind
/// group is still right.
#[derive(Clone, PartialEq)]
pub struct RiverTextures {
    pub lean: wgpu::TextureView,
    pub lean_cross: wgpu::TextureView,
    pub color: wgpu::TextureView,
    pub fog_ramp: wgpu::TextureView,
    pub lut: wgpu::TextureView,
    pub reflection: Option<wgpu::TextureView>,
    pub fow: Option<wgpu::TextureView>,
    pub overlay: Option<wgpu::TextureView>,
    /// `Rivers_Generic_ColorKey` (S1a ev 1104 t12); none: `lut` stands in.
    pub key: Option<wgpu::TextureView>,
    /// The river's own `WaterColor` (`river_diffuse_new`, sRGB view) and `WaterBlendMap` (`river_alpha_new`);
    /// none: the ocean's ramp and a flat opacity stand in.
    pub river_color: Option<wgpu::TextureView>,
    pub blend: Option<wgpu::TextureView>,
}

impl RiverTextures {
    /// The river's own colour and blend maps.
    pub fn with_own(
        mut self,
        river_color: Option<wgpu::TextureView>,
        blend: Option<wgpu::TextureView>,
    ) -> Self {
        self.river_color = river_color;
        self.blend = blend;
        self
    }

    /// From the views of the pack and the inputs of the surface; `None` while one is missing.
    pub fn new(
        views: (wgpu::TextureView, wgpu::TextureView, wgpu::TextureView),
        inputs: &OceanInputs,
    ) -> Option<Self> {
        Some(Self {
            lean: views.0,
            lean_cross: views.1,
            color: views.2,
            fog_ramp: inputs.fog_ramp.clone()?,
            lut: inputs.lut.clone()?,
            reflection: inputs.reflection.clone(),
            fow: inputs.fow.clone(),
            overlay: inputs.overlay.clone(),
            key: inputs.river_key.clone(),
            river_color: None,
            blend: None,
        })
    }
}

/// Everything the rivers take from the surface in a frame.
#[derive(Clone)]
pub struct RiverFeed {
    pub map: OceanMap,
    pub terrain: TerrainFrame,
    pub frame: WaterFrame,
    /// `TerrainWaterQuality`: 0 low, 1 medium, 2 high.
    pub quality: usize,
    pub textures: RiverTextures,
}

/// The river shader of a quality: the original has two (`PSRiverLow` below the highest level of
/// the three of the ocean).
// TODO(RW-12): which of the three levels takes `PSRiverLow` is a guess (0 only).
fn variant(quality: usize) -> usize {
    usize::from(quality >= 1)
}

/// Bindings of the textures in the order of [`RiverGpu::views`]: `(binding, 3D, clamped)`; the
/// sampler is the next binding.
const TEXTURES: [(u32, bool, bool); 12] = [
    (1, false, false),  // LEAN1_1
    (3, false, false),  // LEAN1_2
    (5, false, false),  // ReflectionMap
    (7, false, false),  // WaterBlendMap
    (9, false, true),   // WaterColor
    (13, false, true),  // WaterFogRamp
    (15, false, false), // OverlayMap
    (17, false, false), // FOWMask
    (19, true, true),   // TransFOWColor
    (24, false, false), // LEAN2_1
    (26, false, false), // LEAN2_2
    (28, true, true),   // ColorKey
];

/// The binding of the matrix `World`.
const WORLD_BINDING: u32 = 23;

/// The pipelines, buffers and group of the rivers.
pub struct RiverGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    samples: u32,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: [Option<wgpu::RenderPipeline>; 2],
    wrap: wgpu::Sampler,
    clamp: wgpu::Sampler,
    consts: Buf,
    world: Buf,
    vertices: Option<Buf>,
    indices: Option<Buf>,
    index_count: u32,
    // Stand-ins for what the pack and other subsystems do not give yet.
    black: wgpu::TextureView,
    clear: wgpu::TextureView,
    visible: wgpu::TextureView,
    blend: wgpu::TextureView,
    bound_to: Option<RiverTextures>,
    group: Option<wgpu::BindGroup>,
    /// The class mask's pipeline (`golden` only), made on the first mask draw.
    mask_pipeline: std::cell::OnceCell<wgpu::RenderPipeline>,
}

impl RiverGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let module = create_module(
            device,
            "water river",
            crate::wgsl_module!("common/sm3.wgsl", "water/river.wgsl"),
        );
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<WaterConsts>() as u64
                    ),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: WORLD_BINDING,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(64),
                },
                count: None,
            },
        ];
        for (binding, d3, _) in TEXTURES {
            let dim = if d3 {
                wgpu::TextureViewDimension::D3
            } else {
                wgpu::TextureViewDimension::D2
            };
            entries.push(texture_entry(binding, dim));
            entries.push(sampler_entry(binding + 1));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water river"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water river"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let d2 = wgpu::TextureDimension::D2;
        Self {
            device: device.clone(),
            queue: queue.clone(),
            samples,
            layout,
            pipeline_layout,
            module,
            pipelines: [None, None],
            wrap: sampler("river wrap", wgpu::AddressMode::Repeat),
            clamp: sampler("river clamp", wgpu::AddressMode::ClampToEdge),
            consts: device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("river consts"),
                    size: CONSTS_STRIDE * COPIES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            ),
            world: device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("river world"),
                    contents: bytemuck::bytes_of(&Mat4::IDENTITY.to_cols_array()),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
                Cluster::Other,
            ),
            vertices: None,
            indices: None,
            index_count: 0,
            black: one_texel(device, queue, "river black", rgba, [0, 0, 0, 255], d2),
            clear: one_texel(device, queue, "river clear", rgba, [0, 0, 0, 0], d2),
            visible: one_texel(device, queue, "river visible", rgba, [255, 0, 255, 255], d2),
            blend: one_texel(
                device,
                queue,
                "river blend",
                rgba,
                [255, 255, 255, BLEND_ALPHA],
                d2,
            ),
            bound_to: None,
            group: None,
            mask_pipeline: std::cell::OnceCell::new(),
        }
    }

    /// Uploads the ribbon.
    pub fn set_mesh(&mut self, mesh: &RiverMesh) {
        self.index_count = mesh.indices.len() as u32;
        if mesh.is_empty() {
            self.vertices = None;
            self.indices = None;
            return;
        }
        self.vertices = Some(self.device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("river vertices"),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        ));
        self.indices = Some(self.device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("river indices"),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        ));
    }

    pub fn has_mesh(&self) -> bool {
        self.index_count > 0
    }

    /// Writes the constants of each copy of the map: the block of the ocean, with the register of
    /// the flow (`RiverOffset`) and the scales of the river in place of the ocean's unused ones.
    pub fn prepare(&mut self, feed: &RiverFeed) {
        let offset = river_offset(feed.frame.time);
        for shift in feed.map.shifts() {
            let slot = copy_slot(shift).min(COPIES - 1);
            let mut consts = WaterConsts::new(&feed.map, &feed.frame, shift);
            consts.unused_c12 = offset;
            consts.unused_c14 = RIVER_SCALE;
            self.queue.write_buffer(
                &self.consts,
                slot as u64 * CONSTS_STRIDE,
                bytemuck::bytes_of(&consts),
            );
        }
    }

    /// The pipeline of a quality.
    fn pipeline(&mut self, quality: usize) -> &wgpu::RenderPipeline {
        let q = variant(quality);
        if self.pipelines[q].is_none() {
            let fragment = ["psriverlow", "psriverhigh"][q];
            let attributes =
                wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2];
            let pipeline = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(fragment),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.module,
                        entry_point: Some("vswaterriver"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<RiverVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    // As the ocean: no depth write, `LessEqual`, over the bed it was cut from.
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: multisample(self.samples),
                    fragment: Some(wgpu::FragmentState {
                        module: &self.module,
                        entry_point: Some(fragment),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: WORLD_FORMAT,
                            // TODO(RW-12): the render states of the river are not in the
                            // captures; the ocean's blend (colour SrcAlpha / InvSrcAlpha, alpha
                            // One / One) stands in.
                            blend: Some(wgpu::BlendState {
                                color: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::SrcAlpha,
                                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                    operation: wgpu::BlendOperation::Add,
                                },
                                alpha: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::One,
                                    dst_factor: wgpu::BlendFactor::One,
                                    operation: wgpu::BlendOperation::Add,
                                },
                            }),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            self.pipelines[q] = Some(pipeline);
        }
        self.pipelines[q].as_ref().expect("made above")
    }

    /// Makes the pipeline of `quality` ready, so that `draw` can run from `&self`.
    pub fn ensure_pipeline(&mut self, quality: usize) {
        let _ = self.pipeline(quality);
    }

    /// The group, made again where a texture changed.
    pub fn bind(&mut self, t: &RiverTextures) {
        if self.group.is_some() && self.bound_to.as_ref() == Some(t) {
            return;
        }
        // LEAN1_1 ... ColorKey, in the order of `TEXTURES`.
        // TODO(RW-12): river LEAN maps (`LEAN2_1`, `LEAN2_2` are the ocean's two LEAN textures
        // again). `ColorKey`, `WaterColor` and `WaterBlendMap` are the river's own where they are in.
        let views: [&wgpu::TextureView; 12] = [
            &t.lean,
            &t.lean_cross,
            t.reflection.as_ref().unwrap_or(&self.black),
            t.blend.as_ref().unwrap_or(&self.blend),
            t.river_color.as_ref().unwrap_or(&t.color),
            &t.fog_ramp,
            t.overlay.as_ref().unwrap_or(&self.clear),
            t.fow.as_ref().unwrap_or(&self.visible),
            &t.lut,
            &t.lean,
            &t.lean_cross,
            t.key.as_ref().unwrap_or(&t.lut),
        ];
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<WaterConsts>() as u64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: WORLD_BINDING,
                resource: self.world.as_entire_binding(),
            },
        ];
        for (view, (binding, _, clamped)) in views.iter().zip(TEXTURES) {
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: binding + 1,
                resource: wgpu::BindingResource::Sampler(if clamped {
                    &self.clamp
                } else {
                    &self.wrap
                }),
            });
        }
        self.group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water river"),
            layout: &self.layout,
            entries: &entries,
        }));
        self.bound_to = Some(t.clone());
    }

    /// Draws the copy of the map shifted by `shift`; returns the number of indices drawn.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, quality: usize, shift: f32) -> u32 {
        let (Some(group), Some(vertices), Some(indices), Some(pipeline)) = (
            &self.group,
            &self.vertices,
            &self.indices,
            &self.pipelines[variant(quality)],
        ) else {
            return 0;
        };
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(
            0,
            group,
            &[(copy_slot(shift) as u64 * CONSTS_STRIDE) as u32],
        );
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed_c(0..self.index_count, 0, 0..1);
        self.index_count
    }

    /// The same ribbon into the class mask as [`class::RIVER`].
    pub fn draw_mask(&self, pass: &mut wgpu::RenderPass<'_>, shift: f32) {
        let (Some(group), Some(vertices), Some(indices)) =
            (&self.group, &self.vertices, &self.indices)
        else {
            return;
        };
        let pipeline = self.mask_pipeline.get_or_init(|| {
            let attributes =
                wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x2];
            MaskShader::new(&self.device).pipeline_with(
                &self.device,
                &MaskPipelineDesc {
                    label: "water river mask",
                    vertex: &self.module,
                    vertex_entry: "vswaterriver",
                    vertex_buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<RiverVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                    bind_group_layouts: &[Some(&self.layout)],
                    cull: None,
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    class: class::RIVER,
                },
                MaskDepth::OVER_BED,
            )
        });
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(
            0,
            group,
            &[(copy_slot(shift) as u64 * CONSTS_STRIDE) as u32],
        );
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed_c(0..self.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 4], b: [f32; 4]) {
        for (x, y) in a.iter().zip(&b) {
            assert!((x - y).abs() < 2e-5 * y.abs().max(1.0), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn the_river_offset_is_what_the_captures_have_at_their_times() {
        // The captures give the phase of the ripples, `WaterOffset / WaterScale.x`, which grows 0.72 a
        // second (re-b16 O2, O5): their times in seconds are that over 0.72.
        let seconds = |phase: f32| phase / crate::world::water::ocean::SCROLL_RATE;
        // S1a ev 1104: WaterOffset 0.0921993 / WaterScale.x 0.00781609.
        near(
            river_offset(seconds(11.79608)),
            [0.386162, 0.463394, 0.0965405, -0.579243],
        );
        // S2 ev 1032: WaterOffset 0.158569 / WaterScale.x 0.00781609.
        near(
            river_offset(seconds(20.28747)),
            [0.664138, 0.796966, 0.166035, -0.996208],
        );
    }

    #[test]
    fn the_second_layer_flows_against_the_first_across_the_river() {
        let o = river_offset(10.0);
        assert!(o[0] > 0.0 && o[1] > 0.0 && o[2] > 0.0 && o[3] < 0.0);
        assert_eq!(river_offset(0.0), [0.0; 4]);
    }
}
