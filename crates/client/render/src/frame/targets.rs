//! Render-target pool: subsystems declare targets by [`ResourceId`] in
//! `init`; the pool creates and recreates them when the screen size or a size key changes.
//! Sizes that depend on a settings key (`ShadowLevel`, `FOWLevel`) are redeclared by the owner
//! with the new absolute size; the pool recreates only what changed.

use crate::perf::{Cluster, DeviceTrack, Tex};
use thiserror::Error;

/// Format of the world: gamma space, not sRGB. The shaders carried over from SM3 do
/// their own `pow`/`sqrt`, as the original does; no hardware sRGB conversion is on the path.
pub const WORLD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Depth and stencil of the main pass: the stencil is for the rivers and the landmarks. Range
/// 0..1 as in D3D, no reversed Z.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Clear colour of the main pass, in gamma space.
// re: GameViewState::RenderGame (clear to 0.33, 0.33, 0.33)
pub const CLEAR_GREY: f64 = 0.33;

/// The resolved world (`Scene`), read by `Present`.
pub const SCENE: ResourceId = ResourceId("Scene");
/// The multisampled colour of the main pass; absent with one sample.
pub const SCENE_MSAA: ResourceId = ResourceId("SceneMsaa");
/// Depth and stencil of the main pass.
pub const SCENE_DEPTH: ResourceId = ResourceId("SceneDepth");

/// The screen-space overlay (`OverlaySystem::Render`, then `TerrainSystem::RenderOverlay`). Only
/// `world-overlays` writes it; `world-terrain` reads it. Size follows the screen.
pub const SCREEN_OVERLAY: ResourceId = ResourceId("ScreenOverlay");
/// The map of the overlay in the projection `m_mOverlayProj` (`g_TexHandles[0]`, "Terrain Overlay"):
/// 2048 x 2048 at `OverlayLevel` 1 and 2, 1024 x 1024 at 0 (RW-3). `world-overlays` writes it.
pub const OVERLAY_MAP: ResourceId = ResourceId("OverlayMap");
/// The colour of the reflection pass (`RenderReflections`), read by the water through
/// `g_ReflectVP` as `ReflectionColor`. Declared by `reflection::declare_targets` while
/// `ReflectionLevel` is 1.
pub const REFLECTION_COLOR: ResourceId = ResourceId("ReflectionColor");
/// The depth of the reflection pass.
pub const REFLECTION_DEPTH: ResourceId = ResourceId("ReflectionDepth");
/// The fog mask (`FluidFOWSystem::RenderFOWMask`).
pub const FOW_MASK: ResourceId = ResourceId("FowMask");
/// The base layer of the fog (`FOWBaseCached`).
pub const FOW_BASE: ResourceId = ResourceId("FowBase");
/// The minimap texture the UI shows (`[MiniMap] Width = 320, Height = 190` of `config.ini`).
pub const MINIMAP: ResourceId = ResourceId("Minimap");
pub const MINIMAP_SIZE: (u32, u32) = (320, 190);

/// Side of `FowMask` and `FowBase` for a `FOWLevel`: 1024 from level 2, else 512. The original
/// works at `min(FOWLevel, 2)` (level 3 is never reached), 1 has a 512 mask and 2 a 1024 one.
// re: FluidFOWSystem.h m_uFogLevel: "min(GeneralSettings::g_uFOWLevel, 2): 0 draws nothing here, 1
// uses a 512 mask, 2 a 1024 mask" (RW-8)
pub fn fow_mask_side(fow_level: i64) -> u32 {
    if fow_level >= 2 { 1024 } else { 512 }
}

/// Declares the targets of the overlay and fog passes and the minimap. Formats are `Rgba8Unorm`
/// until S1 gives the original's. Redeclared when `FOWLevel` or `OverlayLevel`
/// changes: the pool recreates only what changed. `overlay_map` is the side of the map of the
/// overlay (`world::overlay::map::map_side`), or `None` for no such target.
pub fn declare_overlay_targets(
    pool: &mut TargetPool,
    fow_level: i64,
    overlay_map: Option<u32>,
) -> Result<(), TargetError> {
    let full = TargetSize::Screen { num: 1, den: 1 };
    pool.declare(SCREEN_OVERLAY, ResourceDesc::color(WORLD_FORMAT, full))?;
    if let Some(side) = overlay_map {
        pool.declare(
            OVERLAY_MAP,
            ResourceDesc::color(WORLD_FORMAT, TargetSize::Absolute(side, side)),
        )?;
    } else {
        pool.remove(OVERLAY_MAP);
    }
    let side = fow_mask_side(fow_level);
    let fow = ResourceDesc::color(WORLD_FORMAT, TargetSize::Absolute(side, side));
    pool.declare(FOW_MASK, fow)?;
    pool.declare(FOW_BASE, fow)?;
    pool.declare(
        MINIMAP,
        ResourceDesc::color(
            WORLD_FORMAT,
            TargetSize::Absolute(MINIMAP_SIZE.0, MINIMAP_SIZE.1),
        ),
    )
}

/// Multisample state of every main-pass pipeline.
pub fn multisample(samples: u32) -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: samples,
        mask: !0,
        alpha_to_coverage_enabled: false,
    }
}

/// Declares the targets of the main pass for `samples` (1 or 4): `Scene`, and with MSAA also
/// `SceneMsaa`; `SceneDepth` has the sample count of the colour the pass draws to.
pub fn declare_main_targets(pool: &mut TargetPool, samples: u32) -> Result<(), TargetError> {
    let full = TargetSize::Screen { num: 1, den: 1 };
    let scene = ResourceDesc::color(WORLD_FORMAT, full);
    pool.declare(SCENE, scene)?;
    if samples > 1 {
        pool.declare(SCENE_MSAA, scene.with_samples(samples))?;
    } else {
        pool.remove(SCENE_MSAA);
    }
    pool.declare(
        SCENE_DEPTH,
        ResourceDesc::color(DEPTH_FORMAT, full).with_samples(samples),
    )
}

/// Name of a target, unique in the pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ResourceId(pub &'static str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetSize {
    Absolute(u32, u32),
    /// Screen size times `num / den`, at least 1 pixel.
    Screen {
        num: u32,
        den: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceDesc {
    pub format: wgpu::TextureFormat,
    pub size: TargetSize,
    /// 1 or 4: WebGPU guarantees nothing else.
    pub samples: u32,
    /// Full mip chain (the terrain cache pages).
    pub mips: bool,
    pub usage: wgpu::TextureUsages,
}

impl ResourceDesc {
    /// A colour target that is also sampled by later passes.
    pub fn color(format: wgpu::TextureFormat, size: TargetSize) -> Self {
        Self {
            format,
            size,
            samples: 1,
            mips: false,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                // `Renderer::read_frame(Target(..))` copies any target out.
                | wgpu::TextureUsages::COPY_SRC,
        }
    }

    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples;
        // A multisampled texture is only resolved, never bound.
        if samples > 1 {
            self.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        self
    }

    /// Pixel size for a screen of `screen`.
    pub fn resolve_size(&self, screen: (u32, u32)) -> (u32, u32) {
        match self.size {
            TargetSize::Absolute(w, h) => (w, h),
            TargetSize::Screen { num, den } => {
                let f = |v: u32| (v as u64 * num as u64 / den.max(1) as u64).max(1) as u32;
                (f(screen.0), f(screen.1))
            }
        }
    }

    fn is_float(&self) -> bool {
        use wgpu::TextureFormat as F;
        matches!(
            self.format,
            F::R16Float
                | F::Rg16Float
                | F::Rgba16Float
                | F::R32Float
                | F::Rg32Float
                | F::Rgba32Float
        )
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TargetError {
    #[error("target {0}: sample count {1} is not 1 or 4")]
    Samples(&'static str, u32),
    #[error("target {0}: size is zero")]
    ZeroSize(&'static str),
    #[error("target {0}: float format without float-target support")]
    NoFloatTargets(&'static str),
    #[error("target {0}: {1} bytes per sample, the limit is 32")]
    TooWide(&'static str, u32),
}

/// A created target.
pub struct Target {
    pub texture: Tex,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
    /// Number of the creation: grows each time the texture is made again, so a user that
    /// registered the view (the UI, for the minimap) knows when to register it anew.
    pub generation: u64,
}

struct Entry {
    id: ResourceId,
    desc: ResourceDesc,
    gpu: Option<Target>,
}

/// Bytes per sample allowed for one colour target (`downlevel_webgl2_defaults`).
const MAX_BYTES_PER_SAMPLE: u32 = 32;

pub struct TargetPool {
    float_targets: bool,
    screen: (u32, u32),
    entries: Vec<Entry>,
    created: u64,
}

impl TargetPool {
    /// `float_targets`: the device renders to float formats (`EXT_color_buffer_float` on WebGL2).
    pub fn new(float_targets: bool, screen: (u32, u32)) -> Self {
        Self {
            float_targets,
            screen,
            entries: Vec::new(),
            created: 0,
        }
    }

    /// Subsystems with a float path and a fallback ask this.
    pub fn has_float_targets(&self) -> bool {
        self.float_targets
    }

    fn validate(&self, id: ResourceId, d: &ResourceDesc) -> Result<(), TargetError> {
        if !matches!(d.samples, 1 | 4) {
            return Err(TargetError::Samples(id.0, d.samples));
        }
        let (w, h) = d.resolve_size((1, 1));
        if w == 0 || h == 0 {
            return Err(TargetError::ZeroSize(id.0));
        }
        if d.is_float() && !self.float_targets {
            return Err(TargetError::NoFloatTargets(id.0));
        }
        if let Some(bytes) = d.format.block_copy_size(None)
            && bytes > MAX_BYTES_PER_SAMPLE
        {
            return Err(TargetError::TooWide(id.0, bytes));
        }
        Ok(())
    }

    /// Declares or redeclares a target. Unchanged descriptions keep the existing texture.
    pub fn declare(&mut self, id: ResourceId, desc: ResourceDesc) -> Result<(), TargetError> {
        self.validate(id, &desc)?;
        match self.entries.iter_mut().find(|e| e.id == id) {
            Some(e) if e.desc == desc => {}
            Some(e) => {
                e.desc = desc;
                e.gpu = None;
            }
            None => self.entries.push(Entry {
                id,
                desc,
                gpu: None,
            }),
        }
        Ok(())
    }

    /// Drops a target (its texture is freed); unknown ids are ignored.
    pub fn remove(&mut self, id: ResourceId) {
        self.entries.retain(|e| e.id != id);
    }

    /// New screen size: screen-relative targets are recreated on the next [`Self::sync`].
    pub fn set_screen(&mut self, screen: (u32, u32)) {
        if screen == self.screen {
            return;
        }
        self.screen = screen;
        for e in &mut self.entries {
            if matches!(e.desc.size, TargetSize::Screen { .. }) {
                e.gpu = None;
            }
        }
    }

    /// Creates the targets that are missing. Zero screen sizes (minimised) are skipped.
    pub fn sync(&mut self, device: &wgpu::Device) {
        if self.screen.0 == 0 || self.screen.1 == 0 {
            return;
        }
        for e in &mut self.entries {
            if e.gpu.is_some() {
                continue;
            }
            self.created += 1;
            let (w, h) = e.desc.resolve_size(self.screen);
            let mips = if e.desc.mips {
                32 - w.max(h).leading_zeros()
            } else {
                1
            };
            let texture = device.tex(
                &wgpu::TextureDescriptor {
                    label: Some(e.id.0),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: mips,
                    sample_count: e.desc.samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: e.desc.format,
                    usage: e.desc.usage,
                    view_formats: &[],
                },
                Cluster::RenderTargets,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            e.gpu = Some(Target {
                texture,
                view,
                size: (w, h),
                generation: self.created,
            });
        }
    }

    pub fn get(&self, id: ResourceId) -> Option<&Target> {
        self.entries
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.gpu.as_ref())
    }

    /// The description a target was declared with.
    pub fn desc_of(&self, id: ResourceId) -> Option<ResourceDesc> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.desc)
    }

    /// Size a declared target has or will have, without a GPU.
    pub fn size_of(&self, id: ResourceId) -> Option<(u32, u32)> {
        let e = self.entries.iter().find(|e| e.id == id)?;
        Some(e.desc.resolve_size(self.screen))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::TextureFormat as F;

    const SHADOW: ResourceId = ResourceId("ShadowMap");
    const SCENE: ResourceId = ResourceId("SceneMsaa");

    #[test]
    fn sizes_follow_the_screen_only_when_relative() {
        let mut pool = TargetPool::new(false, (1920, 1080));
        pool.declare(
            SHADOW,
            ResourceDesc::color(F::Rgba8Unorm, TargetSize::Absolute(1024, 1024)),
        )
        .unwrap();
        pool.declare(
            SCENE,
            ResourceDesc::color(F::Rgba8Unorm, TargetSize::Screen { num: 1, den: 1 })
                .with_samples(4),
        )
        .unwrap();
        pool.set_screen((800, 600));
        assert_eq!(pool.size_of(SHADOW), Some((1024, 1024)));
        assert_eq!(pool.size_of(SCENE), Some((800, 600)));
        let half = ResourceDesc::color(F::Rgba8Unorm, TargetSize::Screen { num: 1, den: 2 });
        assert_eq!(half.resolve_size((1, 1)), (1, 1));
    }

    #[test]
    fn overlay_targets_follow_the_fow_level_and_the_overlay_level() {
        let mut pool = TargetPool::new(false, (1280, 720));
        declare_overlay_targets(&mut pool, 3, None).unwrap();
        assert_eq!(pool.size_of(SCREEN_OVERLAY), Some((1280, 720)));
        assert_eq!(pool.size_of(FOW_MASK), Some((1024, 1024)));
        assert_eq!(pool.size_of(FOW_BASE), Some((1024, 1024)));
        assert_eq!(pool.size_of(MINIMAP), Some((320, 190)));
        assert_eq!(
            pool.size_of(OVERLAY_MAP),
            None,
            "no map of the overlay asked for"
        );
        // The level goes down: the fog targets are redeclared smaller, the rest is untouched.
        declare_overlay_targets(&mut pool, 1, Some(2048)).unwrap();
        assert_eq!(pool.size_of(FOW_MASK), Some((512, 512)));
        assert_eq!(pool.size_of(OVERLAY_MAP), Some((2048, 2048)));
        declare_overlay_targets(&mut pool, 1, Some(1024)).unwrap();
        assert_eq!(pool.size_of(OVERLAY_MAP), Some((1024, 1024)));
        declare_overlay_targets(&mut pool, 1, None).unwrap();
        assert_eq!(pool.size_of(OVERLAY_MAP), None);
        assert_eq!(
            (fow_mask_side(0), fow_mask_side(2), fow_mask_side(3)),
            (512, 1024, 1024)
        );
    }

    #[test]
    fn declare_rejects_what_the_limits_do_not_allow() {
        let mut pool = TargetPool::new(false, (640, 480));
        let rgba = ResourceDesc::color(F::Rgba8Unorm, TargetSize::Absolute(64, 64));
        assert_eq!(
            pool.declare(SHADOW, rgba.with_samples(2)),
            Err(TargetError::Samples("ShadowMap", 2))
        );
        let float = ResourceDesc::color(F::Rgba16Float, TargetSize::Absolute(64, 64));
        assert_eq!(
            pool.declare(ResourceId("HeightMap"), float),
            Err(TargetError::NoFloatTargets("HeightMap"))
        );
        let mut with_float = TargetPool::new(true, (640, 480));
        assert!(with_float.has_float_targets());
        assert!(with_float.declare(ResourceId("HeightMap"), float).is_ok());
        assert_eq!(
            pool.declare(
                SHADOW,
                ResourceDesc::color(F::Rgba8Unorm, TargetSize::Absolute(0, 8))
            ),
            Err(TargetError::ZeroSize("ShadowMap"))
        );
    }
}
