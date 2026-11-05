//! The class mask pass, only for `golden`: the same calls as the main pass
//! into a target of `R8Uint` without MSAA and with its own depth, each call writing the number of
//! its class. A pixel then says which subsystem owns it, which `golden` uses to cut its metrics
//! per class.
//!
//! Subsystems draw in `RenderSystem::record_mask` with a pipeline made by [`MaskShader`] from
//! their own vertex stage and `common/mask.wgsl` as the fragment.

use super::targets::{ResourceDesc, ResourceId, TargetError, TargetPool, TargetSize};

/// The target of the pass.
pub const CLASS_MASK: ResourceId = ResourceId("ClassMask");
/// Its own depth, so the classes follow the depth of the mask's draws alone.
pub const CLASS_MASK_DEPTH: ResourceId = ResourceId("ClassMaskDepth");
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Uint;

/// Numbers of the classes. 0 is "nothing drawn".
pub mod class {
    /// Land, with what is baked into it (decals, the road canvas, the farm fields).
    pub const LAND: u32 = 1;
    pub const WATER: u32 = 2;
    pub const RIVER: u32 = 3;
    pub const FOREST: u32 = 4;
    pub const UNIT: u32 = 5;
    /// Cities and buildings.
    pub const CITY: u32 = 6;
    /// Landmarks, bridges and rails (models).
    pub const LANDMARK: u32 = 7;
    pub const ICE: u32 = 8;
    pub const WAVES: u32 = 9;
    pub const PARTICLE: u32 = 10;
    /// Set by `world-overlays` where the fog clouds lie.
    pub const FOG_BIT: u32 = 0x40;
    /// Set by `world-overlays` where the overlay (borders, grid, highlights) lies.
    pub const OVERLAY_BIT: u32 = 0x80;
}

/// Declares the mask target and its depth at the size of the screen.
pub fn declare_mask_targets(pool: &mut TargetPool) -> Result<(), TargetError> {
    let full = TargetSize::Screen { num: 1, den: 1 };
    pool.declare(CLASS_MASK, ResourceDesc::color(MASK_FORMAT, full))?;
    pool.declare(
        CLASS_MASK_DEPTH,
        ResourceDesc::color(super::targets::DEPTH_FORMAT, full),
    )
}

/// What a subsystem tells to make the pipeline of one class.
pub struct MaskPipelineDesc<'a> {
    pub label: &'a str,
    /// The vertex stage of the subsystem and the layouts it needs.
    pub vertex: &'a wgpu::ShaderModule,
    pub vertex_entry: &'a str,
    pub vertex_buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    pub bind_group_layouts: &'a [Option<&'a wgpu::BindGroupLayout>],
    pub cull: Option<wgpu::Face>,
    pub topology: wgpu::PrimitiveTopology,
    /// One of [`class`].
    pub class: u32,
}

/// The depth test of a mask draw. The default is what the opaque classes use: `Less`, with writes.
#[derive(Clone, Copy, Debug)]
pub struct MaskDepth {
    pub compare: wgpu::CompareFunction,
    pub write: bool,
}

impl Default for MaskDepth {
    fn default() -> Self {
        Self {
            compare: wgpu::CompareFunction::Less,
            write: true,
        }
    }
}

impl MaskDepth {
    /// `LessEqual` without writes, as the water and the rivers of the main pass.
    pub const OVER_BED: Self = Self {
        compare: wgpu::CompareFunction::LessEqual,
        write: false,
    };
}

/// The fragment `common/mask.wgsl`, made once.
pub struct MaskShader {
    module: wgpu::ShaderModule,
}

impl MaskShader {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            module: crate::shaders::create_module(
                device,
                "class mask",
                crate::wgsl_module!("common/mask.wgsl"),
            ),
        }
    }

    /// The pipeline that draws `desc` into the class mask with `desc.class`.
    pub fn pipeline(
        &self,
        device: &wgpu::Device,
        desc: &MaskPipelineDesc<'_>,
    ) -> wgpu::RenderPipeline {
        self.pipeline_with(device, desc, MaskDepth::default())
    }

    /// The same with the depth state of the draw: the transparent surfaces (water, rivers) lie
    /// over the bed they were cut from, so they test `LessEqual` and leave the depth alone.
    pub fn pipeline_with(
        &self,
        device: &wgpu::Device,
        desc: &MaskPipelineDesc<'_>,
        depth: MaskDepth,
    ) -> wgpu::RenderPipeline {
        self.pipeline_with_constants(device, desc, depth, &[])
    }

    /// The same with the `override` values of the vertex stage (a family's variant).
    pub fn pipeline_with_constants(
        &self,
        device: &wgpu::Device,
        desc: &MaskPipelineDesc<'_>,
        depth: MaskDepth,
        vertex_constants: &[(&str, f64)],
    ) -> wgpu::RenderPipeline {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(desc.label),
            bind_group_layouts: desc.bind_group_layouts,
            immediate_size: 0,
        });
        let constants = [("MASK_CLASS", f64::from(desc.class))];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(desc.label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: desc.vertex,
                entry_point: Some(desc.vertex_entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: vertex_constants,
                    ..Default::default()
                },
                buffers: desc.vertex_buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: desc.topology,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: desc.cull,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: super::targets::DEPTH_FORMAT,
                depth_write_enabled: Some(depth.write),
                depth_compare: Some(depth.compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.module,
                entry_point: Some("fs_mask"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants: &constants,
                    ..Default::default()
                },
                targets: &[Some(wgpu::ColorTargetState {
                    format: MASK_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

/// Begins the mask pass: both targets cleared (class 0, depth 1).
pub fn begin_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("class mask"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: color,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(1.0),
                store: wgpu::StoreOp::Discard,
            }),
            stencil_ops: Some(wgpu::Operations {
                load: wgpu::LoadOp::Clear(0),
                store: wgpu::StoreOp::Discard,
            }),
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_class_numbers_are_the_table() {
        use class::*;
        assert_eq!(
            [
                LAND, WATER, RIVER, FOREST, UNIT, CITY, LANDMARK, ICE, WAVES, PARTICLE
            ],
            [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
        // The bits stay clear of the numbers.
        const _: () = assert!(PARTICLE < FOG_BIT && FOG_BIT & OVERLAY_BIT == 0);
        assert_eq!((FOG_BIT, OVERLAY_BIT), (0x40, 0x80));
    }

    #[test]
    fn the_mask_targets_are_screen_sized_and_within_the_limits() {
        let mut pool = TargetPool::new(false, (800, 450));
        declare_mask_targets(&mut pool).unwrap();
        assert_eq!(pool.size_of(CLASS_MASK), Some((800, 450)));
        assert_eq!(pool.size_of(CLASS_MASK_DEPTH), Some((800, 450)));
        assert_eq!(MASK_FORMAT, wgpu::TextureFormat::R8Uint);
    }
}
