//! The GPU side of the UI v2 coverage atlas: one R8 `texture_2d_array` whose layers are the glyph pages of the
//! scene (`ui2_text::GlyphAtlas`). The static mask pages of a skin package will take the first layers once
//! packages are loaded (`GlyphStore::layer_base` already offsets the glyph layers).
//!
//! The array is made again, and every page uploaded in full, when the page count or size grows; otherwise only
//! the dirty rectangles of the atlas are written.

use ui2_text::{DirtyRect, GlyphAtlas};

use crate::perf::gpu_mem::{Cluster, DeviceTrack, Tex};

/// Fewest layers of the array. On WebGL2 a texture with one layer is a plain 2D texture and cannot be bound
/// as an array, so the array always has two at least.
pub const MIN_LAYERS: u32 = 2;

/// The coverage mask array.
pub struct MaskArray {
    texture: Tex,
    view: wgpu::TextureView,
    size: u32,
    layers: u32,
}

impl MaskArray {
    /// A 1x1 placeholder array (all zero) until the scene has a page.
    pub fn new(device: &wgpu::Device) -> Self {
        Self::make(device, 1, MIN_LAYERS)
    }

    fn make(device: &wgpu::Device, size: u32, layers: u32) -> Self {
        let texture = device.tex(
            &wgpu::TextureDescriptor {
                label: Some("ui2 mask array"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            Cluster::Ui,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("ui2 mask array view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            texture,
            view,
            size,
            layers,
        }
    }

    /// The array view to bind.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// Bytes of the array on the GPU.
    pub fn bytes(&self) -> u64 {
        u64::from(self.size) * u64::from(self.size) * u64::from(self.layers)
    }

    /// Brings the GPU copy up to date with `atlas`, given the rectangles it reported dirty. Returns the bytes
    /// written and whether the array was made again (its view changed: bind groups must be made again).
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &GlyphAtlas,
        dirty: &[DirtyRect],
        full: bool,
    ) -> (u64, bool) {
        let pages = atlas.stats().pages as u32;
        let size = atlas.config().page_size;
        if pages == 0 {
            return (0, false);
        }
        let remade = size != self.size || pages > self.layers;
        if remade {
            *self = Self::make(device, size, pages.max(MIN_LAYERS));
        }
        let mut bytes = 0u64;
        if remade || full {
            for page in 0..pages {
                if let Some(pixels) = atlas.page(page) {
                    self.write(queue, page, 0, 0, size, size, pixels);
                    bytes += pixels.len() as u64;
                }
            }
        } else {
            for r in dirty {
                if r.w == 0 || r.h == 0 || r.page >= self.layers {
                    continue;
                }
                let pixels = atlas.copy_rect(r);
                self.write(queue, r.page, r.x, r.y, r.w, r.h, &pixels);
                bytes += pixels.len() as u64;
            }
        }
        (bytes, remade)
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        queue: &wgpu::Queue,
        layer: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        pixels: &[u8],
    ) {
        if pixels.len() < (w * h) as usize {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
}
