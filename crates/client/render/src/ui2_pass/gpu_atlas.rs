//! The GPU side of the UI v2 texture arrays.
//!
//! - The coverage array (R8, `texture_2d_array`): the mask pages of the skin package first (`A8` pages, icons
//!   painted in one colour), then the glyph pages of the scene (`ui2_draw::GlyphStore`). Its layers are
//!   `Scene::mask_side` pixels square. Room for glyph pages grows in powers of two up to the store's limit, so
//!   a scene whose text keeps growing makes the array again at most a few times.
//! - The image array (RGBA8, premultiplied, `texture_2d_array`): the colour pages of the skin package, in
//!   layers as large as the largest of them.
//!
//! A page sits in the top left corner of its layer; the scene maps sprite rectangles accordingly. An array is
//! made again, and every page of it uploaded, when its layer count or side grows or the package pages change
//! (a skin load: the glyph layers move after the new mask pages); otherwise only the dirty rectangles of the
//! glyph atlas are written, and a frame without changes writes nothing.
//!
//! Both arrays keep at least [`MIN_LAYERS`] layers and are at most `2048` square (the WebGL2 limit), which the
//! package pages (at most 1024 square) and the glyph pages (2048) respect.

use ui2_draw::{GlyphStore, StaticPage};
use ui2_text::DirtyRect;

use crate::perf::gpu_mem::{Cluster, DeviceTrack, Tex};

/// Fewest layers of an array. On WebGL2 a texture with one layer is a plain 2D texture and cannot be bound
/// as an array, so an array always has two at least.
pub const MIN_LAYERS: u32 = 2;

/// A square `texture_2d_array` of one format.
struct Array {
    texture: Tex,
    view: wgpu::TextureView,
    size: u32,
    layers: u32,
    bpp: u32,
}

impl Array {
    fn make(
        device: &wgpu::Device,
        label: &'static str,
        format: wgpu::TextureFormat,
        bpp: u32,
        size: u32,
        layers: u32,
    ) -> Self {
        // SIMPLIFIED(SIMP-247): UI pages and glyphs have no mipmaps (one level, bilinear); a 2x sprite drawn much smaller than its size shimmers
        let texture = device.tex(
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            Cluster::Ui,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(label),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Self {
            texture,
            view,
            size,
            layers,
            bpp,
        }
    }

    fn bytes(&self) -> u64 {
        u64::from(self.size) * u64::from(self.size) * u64::from(self.layers) * u64::from(self.bpp)
    }

    /// Writes a `w` x `h` block at `(x, y)` of `layer`. Returns the bytes written (0 for a short buffer or a
    /// block outside the array).
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
    ) -> u64 {
        let len = u64::from(w) * u64::from(h) * u64::from(self.bpp);
        if (pixels.len() as u64) < len
            || layer >= self.layers
            || x + w > self.size
            || y + h > self.size
            || w == 0
            || h == 0
        {
            return 0;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: layer },
                aspect: wgpu::TextureAspect::All,
            },
            &pixels[..len as usize],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * self.bpp),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        len
    }

    /// Writes package pages into layers `0..pages.len()`.
    fn write_pages(&self, queue: &wgpu::Queue, pages: &[StaticPage]) -> u64 {
        pages
            .iter()
            .enumerate()
            .map(|(i, p)| self.write(queue, i as u32, 0, 0, p.width(), p.height(), &p.pixels))
            .sum()
    }
}

/// The coverage array: package mask pages, then glyph pages.
pub struct MaskArray {
    array: Array,
    /// Layers taken by package pages.
    statics: u32,
}

/// What the scene holds for the coverage array.
pub struct MaskSource<'a> {
    /// The package's mask pages (layers `0..len`).
    pub pages: &'a [StaticPage],
    /// The package pages changed since the last sync.
    pub pages_changed: bool,
    /// The layer side the scene encodes for (`Scene::mask_side`).
    pub side: u32,
    /// The glyph pages (layers after the package pages).
    pub glyphs: &'a GlyphStore,
    /// Glyph page rectangles written since the last sync.
    pub dirty: &'a [DirtyRect],
}

impl MaskArray {
    /// A 1x1 placeholder array (all zero) until the scene has a page.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            array: Self::make(device, 1, MIN_LAYERS),
            statics: 0,
        }
    }

    fn make(device: &wgpu::Device, size: u32, layers: u32) -> Array {
        Array::make(
            device,
            "ui2 mask array",
            wgpu::TextureFormat::R8Unorm,
            1,
            size,
            layers,
        )
    }

    /// The array view to bind.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.array.view
    }

    /// Bytes of the array on the GPU.
    pub fn bytes(&self) -> u64 {
        self.array.bytes()
    }

    /// Bytes of the layers that hold package pages.
    pub fn page_bytes(&self) -> u64 {
        u64::from(self.array.size) * u64::from(self.array.size) * u64::from(self.statics)
    }

    /// Layers of the array (package pages, glyph pages and room for more glyph pages).
    pub fn layers(&self) -> u32 {
        self.array.layers
    }

    /// Brings the GPU copy up to date with the scene. `full` writes every page. Returns the bytes written and
    /// whether the array was made again (its view changed: bind groups must be made again).
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        src: &MaskSource<'_>,
        full: bool,
    ) -> (u64, bool) {
        let statics = src.pages.len() as u32;
        let glyphs = src.glyphs.page_count() as u32;
        let total = statics + glyphs;
        if total == 0 {
            self.statics = 0;
            return (0, false);
        }
        let side = src.side.max(src.glyphs.page_size());
        let remade = side != self.array.size || total > self.array.layers;
        if remade {
            let room = glyph_room(glyphs, src.glyphs.config().max_pages);
            self.array = Self::make(device, side, (statics + room).max(MIN_LAYERS));
        }
        self.statics = statics;
        let mut bytes = 0u64;
        if remade || full || src.pages_changed {
            bytes += self.array.write_pages(queue, src.pages);
            let size = src.glyphs.page_size();
            for page in 0..glyphs {
                if let Some(pixels) = src.glyphs.page(page) {
                    bytes += self
                        .array
                        .write(queue, statics + page, 0, 0, size, size, pixels);
                }
            }
        } else {
            for r in src.dirty {
                if r.w == 0 || r.h == 0 || r.page >= glyphs {
                    continue;
                }
                let pixels = src.glyphs.copy_rect(r);
                bytes += self
                    .array
                    .write(queue, statics + r.page, r.x, r.y, r.w, r.h, &pixels);
            }
        }
        (bytes, remade)
    }
}

/// Layers kept for `glyphs` glyph pages: the next power of two, at most the store's limit (and never fewer than
/// the pages).
pub fn glyph_room(glyphs: u32, max_pages: u32) -> u32 {
    if glyphs == 0 {
        return 0;
    }
    glyphs.next_power_of_two().min(max_pages).max(glyphs)
}

/// The image array: the package's colour pages (premultiplied RGBA).
pub struct ImageArray {
    array: Array,
    /// Layers that hold pages.
    used: u32,
}

impl ImageArray {
    /// A 1x1 placeholder array (all zero) until the scene has a colour page.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            array: Self::make(device, 1, MIN_LAYERS),
            used: 0,
        }
    }

    fn make(device: &wgpu::Device, size: u32, layers: u32) -> Array {
        Array::make(
            device,
            "ui2 image array",
            wgpu::TextureFormat::Rgba8Unorm,
            4,
            size,
            layers,
        )
    }

    /// The array view to bind.
    pub fn view(&self) -> &wgpu::TextureView {
        &self.array.view
    }

    /// Bytes of the layers that hold pages.
    pub fn page_bytes(&self) -> u64 {
        u64::from(self.array.size) * u64::from(self.array.size) * 4 * u64::from(self.used)
    }

    /// Bytes of the array on the GPU.
    pub fn bytes(&self) -> u64 {
        self.array.bytes()
    }

    /// Uploads the colour pages when they changed (or `full`); `side` is `Pages::image_side`. Returns the bytes
    /// written and whether the array was made again.
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pages: &[StaticPage],
        side: u32,
        changed: bool,
        full: bool,
    ) -> (u64, bool) {
        if !(changed || full) {
            return (0, false);
        }
        let count = pages.len() as u32;
        self.used = count;
        if count == 0 {
            return (0, false);
        }
        let remade = side != self.array.size || count > self.array.layers;
        if remade {
            self.array = Self::make(device, side, count.max(MIN_LAYERS));
        }
        (self.array.write_pages(queue, pages), remade)
    }
}
