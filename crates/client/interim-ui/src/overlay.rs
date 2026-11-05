//! [`InterimUi`]: runs an egui frame and draws it through `egui-wgpu` as a `render::Overlay`.
//!
//! Per presented frame: [`InterimUi::run`] (egui pass and tessellation, no GPU), then the
//! renderer calls `prepare` (textures, buffers) and `draw` (inside the overlay pass).

use egui::epaint::ClippedPrimitive;
use egui::{Context, TexturesDelta};
use egui_wgpu::{Renderer, RendererOptions, ScreenDescriptor};
use platform::InputEvent;
use render::wgpu;
use render::{MinimapInfo, Overlay, OverlayTarget, OverlayTextures};

use crate::input::InputBridge;

struct Frame {
    jobs: Vec<ClippedPrimitive>,
    pixels_per_point: f32,
}

pub struct InterimUi {
    ctx: Context,
    input: InputBridge,
    gpu: Option<(wgpu::TextureFormat, Renderer)>,
    frame: Option<Frame>,
    /// Texture changes not yet uploaded, and textures to free after the next draw.
    textures: TexturesDelta,
    screen: ScreenDescriptor,
    /// The minimap texture the renderer offers this frame, and the one registered with egui.
    offered_minimap: Option<(wgpu::TextureView, MinimapInfo)>,
    minimap: Option<RegisteredMinimap>,
}

/// A renderer texture registered with `egui-wgpu`, valid for one `generation`.
struct RegisteredMinimap {
    id: egui::TextureId,
    info: MinimapInfo,
}

impl Default for InterimUi {
    fn default() -> Self {
        Self::new()
    }
}

impl InterimUi {
    pub fn new() -> Self {
        Self {
            ctx: Context::default(),
            input: InputBridge::new(),
            gpu: None,
            frame: None,
            textures: TexturesDelta::default(),
            screen: ScreenDescriptor {
                size_in_pixels: [1, 1],
                pixels_per_point: 1.0,
            },
            offered_minimap: None,
            minimap: None,
        }
    }

    /// The minimap texture of the renderer as an egui texture and its size in pixels, once it was
    /// registered (the frame after the renderer first offered it, and again after a new
    /// generation). The shell asks it for [`screens::minimap::MinimapSource::Texture`]; `run` hands
    /// it to the panel.
    ///
    /// [`screens::minimap::MinimapSource::Texture`]: crate::screens::minimap::MinimapSource::Texture
    pub fn minimap_texture(&self) -> Option<(egui::TextureId, [u32; 2])> {
        let m = self.minimap.as_ref()?;
        Some((m.id, [m.info.size.0, m.info.size.1]))
    }

    /// The registered minimap texture as a panel source (size and generation).
    pub fn minimap_source(&self) -> Option<crate::screens::minimap::MinimapSource> {
        let m = self.minimap.as_ref()?;
        Some(crate::screens::minimap::MinimapSource::Texture {
            size: m.info.size,
            generation: m.info.generation,
        })
    }

    pub fn ctx(&self) -> &Context {
        &self.ctx
    }

    /// The pointer is over a window or a floating panel (not the map, its flags or the panels at the
    /// screen edges, which egui keeps on the background layer).
    pub fn pointer_over_window(&self) -> bool {
        self.ctx
            .input(|i| i.pointer.hover_pos())
            .and_then(|p| self.ctx.layer_id_at(p))
            .is_some_and(|l| l.order != egui::Order::Background)
    }

    /// Feeds an input event to egui. `true` means the map should handle it too.
    pub fn handle_input(&mut self, event: &InputEvent) -> bool {
        self.input.handle(event, &self.ctx)
    }

    /// Runs one egui frame. `size` is the surface in physical pixels, `time` seconds.
    pub fn run(
        &mut self,
        size: (u32, u32),
        scale: f32,
        time: f64,
        mut build: impl FnMut(&Context),
    ) {
        let raw = self.input.take(size, scale, time);
        // The minimap panel's `Texture` branch reads the renderer's texture from the context.
        crate::screens::minimap::provide_native(&self.ctx, self.minimap_texture());
        let out = self.ctx.run_pass(raw, |ctx| build(ctx));
        self.textures.append(out.textures_delta);
        let pixels_per_point = out.pixels_per_point;
        let jobs = self.ctx.tessellate(out.shapes, pixels_per_point);
        self.screen = ScreenDescriptor {
            size_in_pixels: [size.0.max(1), size.1.max(1)],
            pixels_per_point,
        };
        self.frame = Some(Frame {
            jobs,
            pixels_per_point,
        });
    }

    /// Registers the offered minimap texture with egui when its generation is new, and drops the
    /// registration when the renderer offers none.
    fn sync_minimap(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        let offered = self.offered_minimap.take();
        let current = self.minimap.as_ref().map(|m| m.info.generation);
        if offered.as_ref().map(|(_, i)| i.generation) == current {
            return;
        }
        let old = self.minimap.take();
        if let Some(old) = old {
            self.renderer(device, format).free_texture(&old.id);
        }
        if let Some((view, info)) = offered {
            let id = self.renderer(device, format).register_native_texture(
                device,
                &view,
                wgpu::FilterMode::Linear,
            );
            self.minimap = Some(RegisteredMinimap { id, info });
        }
    }

    fn renderer(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> &mut Renderer {
        let rebuild = self.gpu.as_ref().is_none_or(|(f, _)| *f != format);
        if rebuild {
            // The view format is fixed per surface, so this runs once. A rebuilt renderer would
            // lack the font atlas (egui sends texture deltas only).
            let renderer = Renderer::new(device, format, RendererOptions::default());
            self.gpu = Some((format, renderer));
        }
        &mut self.gpu.as_mut().expect("just set").1
    }
}

impl Overlay for InterimUi {
    fn offer(&mut self, textures: &OverlayTextures<'_>) {
        self.offered_minimap = textures.minimap.cloned().zip(textures.minimap_info);
    }

    fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &OverlayTarget,
    ) {
        self.sync_minimap(device, target.format);
        let Some(frame) = self.frame.take() else {
            return;
        };
        let screen = ScreenDescriptor {
            size_in_pixels: [target.size.0.max(1), target.size.1.max(1)],
            pixels_per_point: frame.pixels_per_point,
        };
        let sets = std::mem::take(&mut self.textures.set);
        let renderer = self.renderer(device, target.format);
        for (id, deltas) in &sets {
            for delta in deltas {
                renderer.update_texture(device, queue, *id, delta);
            }
        }
        // No paint callbacks are used, so there are no extra command buffers to submit.
        let extra = renderer.update_buffers(device, queue, encoder, &frame.jobs, &screen);
        debug_assert!(extra.is_empty());
        self.screen = screen;
        self.frame = Some(frame);
    }

    fn draw(&mut self, pass: &mut wgpu::RenderPass<'static>) {
        let Some(frame) = self.frame.take() else {
            return;
        };
        if let Some((_, renderer)) = &mut self.gpu {
            renderer.render(pass, &frame.jobs, &self.screen);
            for id in std::mem::take(&mut self.textures.free) {
                renderer.free_texture(&id);
            }
        }
    }
}
