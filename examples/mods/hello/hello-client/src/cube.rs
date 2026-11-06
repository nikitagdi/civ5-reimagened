//! The cube of the example mod `hello`: a render system for `MainSlot::Mod`.
//!
//! It stands on the ground at one hex, drawn after the units and before the particles with depth testing, so the
//! terrain and the units hide it where they should. The pipeline is built from `ModGpu` (the layout of the view of the
//! main pass, its formats and sample count); the cube's own data is one uniform buffer in group 1. It reads the
//! ground height from the terrain sampler the renderer gives `prepare`.

use mod_client::gpu::wgpu;
use mod_client::gpu::{
    DrawContext, InitContext, MainSlot, ModGpu, PassSlot, PrepareContext, RenderSystem, SystemId,
};

/// The hex the cube stands on.
pub const HEX: (i32, i32) = (4, 4);
/// Its side, in world units (a hex is about 64 wide).
pub const SIDE: f32 = 24.0;
/// Its colour.
pub const COLOR: [f32; 4] = [0.95, 0.45, 0.1, 1.0];

/// The WGSL of the cube (checked by the tests of this crate with the parser of the renderer's shader catalog).
pub const SHADER: &str = include_str!("cube.wgsl");

const PASSES: &[PassSlot] = &[PassSlot::Main(MainSlot::Mod)];

pub struct HelloCube {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    /// The ground is known and the uniform holds its place: the cube is drawn.
    visible: bool,
}

impl HelloCube {
    pub fn new(gpu: &ModGpu) -> Self {
        let device = &gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hello cube"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hello cube"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hello cube"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hello cube"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hello cube"),
            bind_group_layouts: &[Some(&gpu.view_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hello cube"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // The faces are made from the index with no winding to keep straight: both sides are drawn.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: gpu.depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: gpu.samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            group,
            uniform,
            visible: false,
        }
    }

    /// Whether the last `prepare` found the ground and so the cube is drawn.
    pub fn visible(&self) -> bool {
        self.visible
    }
}

impl RenderSystem for HelloCube {
    fn id(&self) -> SystemId {
        // The host numbers a mod's systems itself; this is the first and only of the mod.
        SystemId::Mod(0)
    }

    fn passes(&self) -> &'static [PassSlot] {
        PASSES
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        // No map yet: nothing to stand on.
        let Some(ground) = cx.terrain().map(|t| t.hex_location(HEX.0, HEX.1)) else {
            self.visible = false;
            return;
        };
        let mut bytes = [0u8; 32];
        for (i, v) in [ground.x, ground.y, ground.z, SIDE]
            .into_iter()
            .chain(COLOR)
            .enumerate()
        {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        cx.queue.write_buffer(&self.uniform, 0, &bytes);
        self.visible = true;
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if !self.visible || slot != PassSlot::Main(MainSlot::Mod) {
            return;
        }
        cx.pass.set_pipeline(&self.pipeline);
        cx.pass.set_bind_group(0, cx.view_group, &[]);
        cx.pass.set_bind_group(1, &self.group, &[]);
        cx.pass.draw(0..36, 0..1);
    }
}
