//! The open end of the frame (client-hooks, R1 to R3): what a render system that is not part of the
//! engine (a mod's) needs to join a frame. The engine's own systems do not use it.
//!
//! - [`ModGpu`]: device, queue, layouts and formats to build a pipeline that fits the main pass.
//! - [`FrameShared`]: the constants of the frame being drawn (sun, fog, time) and their bind group,
//!   read through `PrepareContext::services().frame` and `DrawContext::services().frame`.
//!
//! The terrain sampler comes with the contexts (`PrepareContext::terrain`, `DrawContext::terrain`).

use crate::lighting::FrameGlobals;

/// What a mod needs to build a pipeline for the main pass. Cloning is cheap (wgpu handles).
#[derive(Clone)]
pub struct ModGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Group 0 of the main pass (`DrawContext::view_group`): the view constants, one uniform buffer.
    pub view_layout: wgpu::BindGroupLayout,
    /// The layout of [`FrameShared::bind_group`]: binding 0, `FrameGlobals`, vertex and fragment.
    pub frame_layout: wgpu::BindGroupLayout,
    /// Colour format of the main pass.
    pub color_format: wgpu::TextureFormat,
    /// Depth and stencil format of the main pass (compare `LessEqual`, depth write on).
    pub depth_format: wgpu::TextureFormat,
    /// Multisampling of the main pass; fixed for the life of a renderer.
    pub samples: u32,
}

/// The `FrameGlobals` of the frame being drawn, for systems outside the engine.
#[derive(Default)]
pub struct FrameShared {
    globals: FrameGlobals,
    group: Option<wgpu::BindGroup>,
}

impl FrameShared {
    /// The layout of [`FrameShared::bind_group`] (`binding 0`, uniform, vertex and fragment).
    pub(crate) fn make_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mod frame globals layout"),
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
        })
    }

    /// Called once by the renderer, with the group over its `FrameGlobals` buffer.
    pub(crate) fn bind(&mut self, group: wgpu::BindGroup) {
        self.group = Some(group);
    }

    /// Called every frame by the renderer, after the buffer was written.
    pub(crate) fn set_globals(&mut self, globals: FrameGlobals) {
        self.globals = globals;
    }

    /// The bind group of the frame's `FrameGlobals` (layout: [`ModGpu::frame_layout`]); the buffer behind it
    /// holds this frame's values by the time the main pass runs. `None` before the renderer made it.
    pub fn bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.group.as_ref()
    }

    /// The whole block as the shaders read it (the original's basis, Z up).
    pub fn globals(&self) -> &FrameGlobals {
        &self.globals
    }

    /// The sun's direction (`g_SunDir`, the original's basis).
    pub fn sun_dir(&self) -> [f32; 3] {
        self.globals.g_SunDir
    }

    /// `g_FogConsts`: height falloff, density, unused, ramp row.
    pub fn fog_consts(&self) -> [f32; 4] {
        self.globals.g_FogConsts
    }

    pub fn fog_color(&self) -> [f32; 4] {
        self.globals.f3FogColor
    }

    /// Seconds (`g_Time`).
    pub fn time(&self) -> f32 {
        self.globals.g_Time
    }
}

/// Why a mod's render system was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModSystemError {
    /// Its `id()` is not `SystemId::Mod(_)`: the engine's ids belong to the engine.
    NotModId,
    /// A system with this id is registered already.
    Duplicate,
}

impl std::fmt::Display for ModSystemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModSystemError::NotModId => {
                f.write_str("a mod's render system must have a SystemId::Mod id")
            }
            ModSystemError::Duplicate => f.write_str("a render system with this id is registered"),
        }
    }
}

impl std::error::Error for ModSystemError {}
