//! Registry of the UI textures: `TextureId` to a GPU texture, loaded lazily and evicted by LRU.
//!
//! The renderer never reads the file system. A command whose texture is not here is skipped and its
//! id lands in the wanted set; the client takes it with [`UiTextures::take_wanted`], gets the bytes
//! from `ui-assets`, decodes them off the frame and calls [`UiTextures::upload`].

use std::collections::{BTreeSet, HashMap, HashSet};

use asset_format::{TexDim, TextureBlob, TextureFormat};
use draw_list::TextureId;

use crate::perf::gpu_mem::{Cluster, DeviceTrack, Tex};
use crate::texture::{self, UploadOptions};

/// A texture as `ui-assets` hands it over: the original's bytes, DXT as they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiTextureData {
    pub width: u32,
    pub height: u32,
    /// `Rgba8` (A8R8G8B8 as RGBA), `R8` (L8), `Rg8` (the L16 `-index` pairs) or BC1-BC5.
    pub format: TextureFormat,
    /// Level 0 first. Only level 0 is uploaded: the UI has one level and samples it.
    pub mips: Vec<Vec<u8>>,
    /// The top left part of the texture that holds the image, when it is smaller than `width` x
    /// `height`: a compressed variant padded to whole 4x4 blocks. The UV
    /// of the draw list spans the image, so the pass scales it by `content / size`. `None`: all of it.
    pub content: Option<(u32, u32)>,
}

impl UiTextureData {
    /// UV scale that maps the image's 0..1 onto the stored texture.
    pub fn uv_scale(&self) -> [f32; 2] {
        match self.content {
            Some((w, h)) if self.width > 0 && self.height > 0 => [
                w.min(self.width) as f32 / self.width as f32,
                h.min(self.height) as f32 / self.height as f32,
            ],
            _ => [1.0, 1.0],
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UiTextureError {
    #[error(
        "texture id {0:#x} is not an asset id (the white texture and external ids are not uploaded)"
    )]
    NotAnAsset(u64),
    #[error("texture has no data or a zero side")]
    Empty,
    #[error("{0}")]
    Gpu(String),
}

/// Frames a texture stays safe from eviction after its last use.
pub const PROTECTED_FRAMES: u64 = 3;

/// Default budget of resident UI textures; a measured value replaces it (`set_budget`).
pub const DEFAULT_BUDGET: u64 = 256 << 20;

struct Entry {
    // Kept alive for the view.
    _texture: Tex,
    view: wgpu::TextureView,
    bytes: u64,
    /// Changes on every upload of the id, so bind group caches can tell a new texture.
    serial: u64,
    last_used: u64,
    /// [`UiTextureData::uv_scale`].
    uv_scale: [f32; 2],
}

/// What happened to the textures in the frame, for the `ui.*` counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TextureFrame {
    pub uploads: u64,
    pub upload_bytes: u64,
    pub evictions: u64,
    pub resident_bytes: u64,
}

pub struct UiTextures {
    device: wgpu::Device,
    queue: wgpu::Queue,
    entries: HashMap<TextureId, Entry>,
    white: Entry,
    wanted: BTreeSet<TextureId>,
    /// Handed out by `take_wanted` and not uploaded yet: not asked for again.
    requested: HashSet<TextureId>,
    budget: u64,
    resident: u64,
    frame: u64,
    next_serial: u64,
    since: TextureFrame,
}

/// Serial of the white texture.
pub(crate) const WHITE_SERIAL: u64 = 0;

impl UiTextures {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let texture = device.tex_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("ui white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
            Cluster::Ui,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            device: device.clone(),
            queue: queue.clone(),
            entries: HashMap::new(),
            white: Entry {
                _texture: texture,
                view,
                bytes: 0,
                serial: WHITE_SERIAL,
                last_used: 0,
                uv_scale: [1.0, 1.0],
            },
            wanted: BTreeSet::new(),
            requested: HashSet::new(),
            budget: DEFAULT_BUDGET,
            resident: 0,
            frame: 0,
            next_serial: 1,
            since: TextureFrame::default(),
        }
    }

    /// Bytes of GPU memory the registry may hold; textures used in the last few frames stay
    /// even above it.
    pub fn set_budget(&mut self, bytes: u64) {
        self.budget = bytes;
        self.evict_over_budget();
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }

    pub fn resident_bytes(&self) -> u64 {
        self.resident
    }

    pub fn contains(&self, id: TextureId) -> bool {
        id == TextureId::WHITE || self.entries.contains_key(&id)
    }

    /// Ids that commands asked for and that are not resident, each once until it is uploaded or
    /// `evict`ed. In id order.
    pub fn take_wanted(&mut self) -> Vec<TextureId> {
        let out: Vec<TextureId> = std::mem::take(&mut self.wanted).into_iter().collect();
        self.requested.extend(out.iter().copied());
        out
    }

    /// Makes `data` the texture `id`, replacing an older one.
    pub fn upload(&mut self, id: TextureId, data: &UiTextureData) -> Result<(), UiTextureError> {
        if id == TextureId::WHITE || id.is_external() {
            return Err(UiTextureError::NotAnAsset(id.0));
        }
        let Some(level0) = data.mips.first() else {
            return Err(UiTextureError::Empty);
        };
        if data.width == 0 || data.height == 0 || level0.is_empty() {
            return Err(UiTextureError::Empty);
        }
        let blob = TextureBlob {
            format: data.format,
            dim: TexDim::D2,
            srgb: false,
            width: data.width,
            height: data.height,
            depth: 1,
            mips: vec![level0.clone()],
        };
        let label = format!("ui:{:#x}", id.0);
        let up = texture::upload_blob(
            &self.device,
            &self.queue,
            &blob,
            &label,
            UploadOptions::WORLD,
        )
        .map_err(UiTextureError::Gpu)?;
        let view = up
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let (bw, bh) = up.format.block_dimensions();
        let block = u64::from(up.format.block_copy_size(None).unwrap_or(4));
        let bytes =
            u64::from(data.width.div_ceil(bw)) * u64::from(data.height.div_ceil(bh)) * block;
        if let Some(old) = self.entries.remove(&id) {
            self.resident -= old.bytes;
        }
        self.entries.insert(
            id,
            Entry {
                _texture: up.texture,
                view,
                bytes,
                serial: self.next_serial,
                last_used: self.frame,
                uv_scale: data.uv_scale(),
            },
        );
        self.next_serial += 1;
        self.resident += bytes;
        self.wanted.remove(&id);
        self.requested.remove(&id);
        self.since.uploads += 1;
        self.since.upload_bytes += bytes;
        self.evict_over_budget();
        Ok(())
    }

    /// Drops `id`; a later command that needs it asks for it again.
    pub fn evict(&mut self, id: TextureId) {
        self.requested.remove(&id);
        self.wanted.remove(&id);
        if let Some(e) = self.entries.remove(&id) {
            self.resident -= e.bytes;
            self.since.evictions += 1;
        }
    }

    /// A new frame: the LRU clock advances and the budget is enforced.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
        self.evict_over_budget();
    }

    /// What happened since the last call.
    pub(crate) fn take_frame(&mut self) -> TextureFrame {
        let mut t = std::mem::take(&mut self.since);
        t.resident_bytes = self.resident;
        t
    }

    /// Marks `id` as used in this frame and returns its serial; `None` and a request for it when
    /// it is not resident.
    pub(crate) fn touch(&mut self, id: TextureId) -> Option<u64> {
        if id == TextureId::WHITE {
            return Some(WHITE_SERIAL);
        }
        match self.entries.get_mut(&id) {
            Some(e) => {
                e.last_used = self.frame;
                Some(e.serial)
            }
            None => {
                if !self.requested.contains(&id) {
                    self.wanted.insert(id);
                }
                None
            }
        }
    }

    /// The UV scale of a resident texture ([`UiTextureData::uv_scale`]); 1 for the white one and
    /// for any other id.
    pub(crate) fn uv_scale(&self, id: TextureId) -> [f32; 2] {
        self.entries.get(&id).map_or([1.0, 1.0], |e| e.uv_scale)
    }

    /// The view of a resident texture (or of the white one).
    pub(crate) fn view(&self, id: TextureId) -> Option<&wgpu::TextureView> {
        if id == TextureId::WHITE {
            return Some(&self.white.view);
        }
        self.entries.get(&id).map(|e| &e.view)
    }

    fn evict_over_budget(&mut self) {
        while self.resident > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, e)| self.frame.saturating_sub(e.last_used) > PROTECTED_FRAMES)
                .min_by_key(|(id, e)| (e.last_used, **id))
                .map(|(id, _)| *id);
            let Some(id) = victim else { return };
            self.evict(id);
        }
    }
}
