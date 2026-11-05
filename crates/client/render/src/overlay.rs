//! Overlay hook (play-ui §3.1): a second render pass after the 3D passes, so a later crate
//! (`interim-ui`, egui-wgpu) can draw into the same frame without `render` depending on it.

use crate::perf::PassCount;

/// Where the overlay draws: the frame's colour format and size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlayTarget {
    /// Format of the colour view the overlay pass writes to (the render-view format).
    pub format: wgpu::TextureFormat,
    /// Surface size in physical pixels.
    pub size: (u32, u32),
    /// Device pixel ratio (1.0 when unknown).
    pub scale: f32,
}

impl OverlayTarget {
    pub fn aspect(&self) -> f32 {
        self.size.0 as f32 / self.size.1.max(1) as f32
    }
}

/// Size of the minimap texture and its number; `generation` grows only when the texture is made
/// again, so the UI registers the view once per generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapInfo {
    /// 320x190 (`[MiniMap]` of `config.ini`).
    pub size: (u32, u32),
    pub generation: u64,
}

/// Textures of the renderer that the UI may draw. `minimap_info` goes
/// with `minimap`; both are `None` while there is no minimap target or `Minimap` is switched off.
#[derive(Debug, Clone, Copy, Default)]
pub struct OverlayTextures<'a> {
    pub minimap: Option<&'a wgpu::TextureView>,
    pub minimap_info: Option<MinimapInfo>,
}

/// Draws on top of the 3D scene. The pass loads the existing colour and has no depth attachment.
/// `prepare` runs before the pass (upload buffers, textures, even extra passes on the encoder);
/// `draw` runs inside it. Both run once per presented frame, in that order.
pub trait Overlay {
    /// Called every frame before `prepare` with the textures of the renderer. The default takes
    /// nothing. A view is valid for the frame; keep a clone to use it later.
    fn offer(&mut self, _textures: &OverlayTextures<'_>) {}

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &OverlayTarget,
    );
    fn draw(&mut self, pass: &mut wgpu::RenderPass<'static>);
}

/// Runs the overlay protocol: `prepare`, then a load-colour/no-depth pass calling `draw`.
pub(crate) fn run_overlay(
    overlay: &mut dyn Overlay,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    target: &OverlayTarget,
    timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
) {
    overlay.prepare(device, queue, encoder, target);
    let mut pass = encoder
        .begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        })
        .forget_lifetime();
    overlay.draw(&mut pass);
}

const DEBUG_RECT_WGSL: &str = r#"
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    var p = array<vec2<f32>, 6>(
        vec2(-0.9, -0.9), vec2(-0.5, -0.9), vec2(-0.5, -0.6),
        vec2(-0.9, -0.9), vec2(-0.5, -0.6), vec2(-0.9, -0.6));
    return vec4(p[i], 0.0, 1.0);
}
@fragment
fn fs() -> @location(0) vec4<f32> {
    return vec4(1.0, 0.0, 1.0, 1.0);
}
"#;

/// Test overlay: a magenta rectangle in the bottom-left corner. Proves the hook end to end.
#[derive(Default)]
pub struct DebugRectOverlay {
    pipeline: Option<(wgpu::TextureFormat, wgpu::RenderPipeline)>,
}

impl Overlay for DebugRectOverlay {
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        target: &OverlayTarget,
    ) {
        if self
            .pipeline
            .as_ref()
            .is_some_and(|(f, _)| *f == target.format)
        {
            return;
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("debug-rect"),
            source: wgpu::ShaderSource::Wgsl(DEBUG_RECT_WGSL.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("debug-rect"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("debug-rect"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(target.format.into())],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        self.pipeline = Some((target.format, pipeline));
    }

    fn draw(&mut self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some((_, p)) = &self.pipeline {
            pass.set_pipeline_c(p);
            pass.draw_c(0..6, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_handles_zero_height() {
        let t = OverlayTarget {
            format: wgpu::TextureFormat::Bgra8Unorm,
            size: (1280, 720),
            scale: 1.0,
        };
        assert!((t.aspect() - 16.0 / 9.0).abs() < 1e-6);
        assert!(OverlayTarget { size: (10, 0), ..t }.aspect().is_finite());
    }
}
