//! The facade a client mod talks to the host through: `ClientCtx` and the service traits it hands out.
//!
//! Everything is a trait with value-only methods (D2): the host implements it with real engine objects, the tests
//! with `testing::MockClientCtx`. Capabilities are enforced once, by [`Guarded`], which wraps whatever the host
//! gives the dispatcher; a host implementation never has to check them.

use draw_list::TextureId;
use mod_api::{Cap, CapDenied, Caps, ChannelId, LogLevel, MapShape, PlayerId, PlotIdx};
use mod_manifest::CHANNEL_PAYLOAD_CEILING;
use thiserror::Error;

use render::cutout::{CutoutGrid, CutoutRegion};
use render::frame::ModGpu;

use crate::types::{
    HotkeySpec, PackState, PlayOpts, PlayerInfo, PlotView, ScreenInfo, StrategicCamera,
};

/// Why a context call failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CtxError {
    #[error(transparent)]
    Denied(#[from] CapDenied),
    #[error("payload of {len} bytes exceeds the {max} byte limit")]
    TooLarge { len: usize, max: usize },
    /// The host has no such service right now (headless run without audio, no terrain yet).
    #[error("`{0}` is not available in this host")]
    Unavailable(&'static str),
    #[error("{0}")]
    Rejected(String),
}

/// Read access to the replica of the game (W1). Fog-aware: only what the player has seen.
pub trait ModWorld {
    /// The local player, once the game has told us (`None` before).
    fn me(&self) -> Option<PlayerId>;
    fn map(&self) -> MapShape;
    fn turn(&self) -> i32;
    /// Grows whenever any plot's owner, class or visibility changes; a mod re-reads what it caches when it moves.
    fn revision(&self) -> u64;
    fn players(&self) -> Vec<PlayerInfo>;
    /// Plot at `(x, y)`, wrapping like the map does; `None` off the map.
    fn plot_at(&self, x: i16, y: i16) -> Option<PlotIdx>;
    fn plot(&self, p: PlotIdx) -> PlotView;

    fn plot_owner(&self, p: PlotIdx) -> Option<PlayerId> {
        self.plot(p).owner
    }

    /// Coordinates of a plot index.
    fn plot_xy(&self, p: PlotIdx) -> (i16, i16) {
        let w = u32::from(self.map().width).max(1);
        ((p % w) as i16, (p / w) as i16)
    }
}

/// Terrain height sampling (W2), in world units. `None` where the terrain is not built yet.
pub trait ModTerrain {
    /// Bilinear height at world `(x, z)`.
    fn height(&self, x: f32, z: f32) -> Option<f32>;
    /// Height of the nearest texel (what the renderer draws).
    ///
    /// Both heights are of the drawn surface, which is never below the sea level (water cells and low ground are
    /// drawn at the sea): standing a person on `height + 6.5` stays above the ground everywhere.
    fn height_nearest(&self, x: f32, z: f32) -> Option<f32>;
    /// The sea level in world units: the lowest ground the renderer draws.
    fn sea_level(&self) -> f32 {
        0.0
    }
    /// Grows whenever heights change (streaming, edits); caches keyed on it stay correct.
    fn height_revision(&self) -> u64;
    /// The hex under world `(x, z)`.
    fn hex_at(&self, x: f32, z: f32) -> Option<(i16, i16)>;
    fn is_built(&self, p: PlotIdx) -> bool;
}

/// The terrain cutout mask (D13). Needs `terrain.cutout`. The host forwards to `Renderer::set_cutout_mask`; the mask
/// is one texture for the whole terrain, so mods share it: a column cut by any mod is cut.
pub trait CutoutMask {
    /// The column grid of the map (`None` before there is a map). A region must lie inside it.
    fn grid(&self) -> Option<CutoutGrid> {
        None
    }

    /// Writes one byte per column of `region` (non-zero = cut out). `bytes.len()` must equal `region.byte_len()`.
    fn set_cutout_mask(&mut self, region: CutoutRegion, bytes: &[u8]) -> Result<(), CtxError>;
    /// Removes every cutout of this mod.
    fn clear_cutout(&mut self);
}

/// Per-mod persistent strings (K1). The host namespaces the keys by mod id, so mods cannot read each other's.
pub trait ModKv {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&mut self, key: &str, value: &str);
    fn remove(&mut self, key: &str);
}

/// Key bindings (C4).
pub trait Hotkeys {
    /// Registers a binding. Fails when the host or the game already uses the chord, or the id repeats.
    fn register(&mut self, spec: HotkeySpec) -> Result<(), CtxError>;
    fn unregister(&mut self, id: &str);
}

/// Sound output (A2). Sound ids are logical ids of the mod's pack.
pub trait ModAudio {
    fn play(&mut self, sound: &str, opts: PlayOpts) -> Result<(), CtxError>;
    /// Silences the game's strategic soundscape while the mod owns the screen.
    fn mute_strategic(&mut self, mute: bool);
}

/// The mod's asset pack (A1).
pub trait ModAssets {
    fn state(&self) -> PackState;
    /// Bytes of a file of the pack by logical id; `None` until `Ready` or for an unknown id.
    fn get(&self, id: &str) -> Option<&[u8]>;
}

/// HUD textures (U1). The host uploads them to the UI texture cache.
pub trait HudTextures {
    /// Uploads tightly packed RGBA8 pixels; the returned id is valid in the mod's `UiDrawList`.
    fn upload_rgba8(&mut self, width: u32, height: u32, rgba: &[u8])
    -> Result<TextureId, CtxError>;
}

/// The context of one mod, as the mod sees it. Every method that needs a capability returns
/// `CtxError::Denied` when the mod did not declare it.
pub trait ClientCtx {
    fn has(&self, cap: Cap) -> bool;
    fn log(&mut self, level: LogLevel, msg: &str);
    /// Sends `payload` on a channel to the mod's server half (`ClientMsg::Mod`). Needs `net.channel`.
    fn send(&mut self, ch: ChannelId, payload: &[u8]) -> Result<(), CtxError>;
    /// Needs `world.read`.
    fn world(&self) -> Result<&dyn ModWorld, CtxError>;
    /// Needs `world.read`.
    fn terrain(&self) -> Result<&dyn ModTerrain, CtxError>;
    /// Needs `terrain.cutout`.
    fn cutout(&mut self) -> Result<&mut dyn CutoutMask, CtxError>;
    /// Needs `kv`.
    fn kv(&mut self) -> Result<&mut dyn ModKv, CtxError>;
    /// Needs `input.claim`.
    fn hotkeys(&mut self) -> Result<&mut dyn Hotkeys, CtxError>;
    /// Needs `audio`.
    fn audio(&mut self) -> Result<&mut dyn ModAudio, CtxError>;
    /// Needs `assets`.
    fn assets(&self) -> Result<&dyn ModAssets, CtxError>;
    /// Needs `hud`.
    fn hud_textures(&mut self) -> Result<&mut dyn HudTextures, CtxError>;
    fn screen(&self) -> ScreenInfo;
    /// What a render system needs to build its pipelines (device, queue, layouts, formats). This is the one place a
    /// wgpu handle crosses the boundary: a pipeline cannot be built from values. Needs `render.pipeline`; unavailable
    /// until the renderer exists.
    fn gpu(&self) -> Result<&ModGpu, CtxError> {
        Err(CtxError::Unavailable("gpu"))
    }
    /// The plot under the cursor in the strategic view (what the game itself would pick), by plot index. `None` when
    /// the cursor is over no plot, while a mod holds the camera (the strategic picking follows the mod's place then),
    /// or outside `frame` and `hud`. Needs `world.read`.
    fn hover_plot(&self) -> Result<Option<PlotIdx>, CtxError> {
        Ok(None)
    }
    /// The strategic camera's pose of this frame. While a mod holds the camera it is the pose the player had when the
    /// mod took it (the game camera is only moved to the mod's terrain focus); `None` outside `frame` and `hud`.
    fn strategic_camera(&self) -> Option<StrategicCamera> {
        None
    }
    /// Captures or releases the pointer. Browsers grant the lock only inside a user gesture, so call it from
    /// `claim_input`. Needs `input.claim`.
    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError>;
}

/// What the dispatcher is given by the app: a context bound to a mod (the host knows which mod index a `send` is
/// from, which kv namespace to open). The host's contexts are unchecked; [`Guarded`] adds the capability checks.
pub trait ClientHost {
    fn ctx(&mut self, mod_idx: u16) -> &mut dyn ClientCtx;
}

/// A context with the mod's capability set enforced, wrapping the host's context for that mod.
pub struct Guarded<'a> {
    inner: &'a mut dyn ClientCtx,
    caps: &'a Caps,
}

impl<'a> Guarded<'a> {
    pub fn new(inner: &'a mut dyn ClientCtx, caps: &'a Caps) -> Self {
        Self { inner, caps }
    }
}

impl ClientCtx for Guarded<'_> {
    fn has(&self, cap: Cap) -> bool {
        self.caps.has(cap)
    }

    fn log(&mut self, level: LogLevel, msg: &str) {
        self.inner.log(level, msg);
    }

    fn send(&mut self, ch: ChannelId, payload: &[u8]) -> Result<(), CtxError> {
        self.caps.require(Cap::NetChannel)?;
        // The server enforces the channel's own limits (D7); this only keeps a frame that can never fit off the wire.
        let max = CHANNEL_PAYLOAD_CEILING as usize;
        if payload.len() > max {
            return Err(CtxError::TooLarge {
                len: payload.len(),
                max,
            });
        }
        self.inner.send(ch, payload)
    }

    fn world(&self) -> Result<&dyn ModWorld, CtxError> {
        self.caps.require(Cap::WorldRead)?;
        self.inner.world()
    }

    fn terrain(&self) -> Result<&dyn ModTerrain, CtxError> {
        self.caps.require(Cap::WorldRead)?;
        self.inner.terrain()
    }

    fn cutout(&mut self) -> Result<&mut dyn CutoutMask, CtxError> {
        self.caps.require(Cap::TerrainCutout)?;
        self.inner.cutout()
    }

    fn kv(&mut self) -> Result<&mut dyn ModKv, CtxError> {
        self.caps.require(Cap::Kv)?;
        self.inner.kv()
    }

    fn hotkeys(&mut self) -> Result<&mut dyn Hotkeys, CtxError> {
        self.caps.require(Cap::InputClaim)?;
        self.inner.hotkeys()
    }

    fn audio(&mut self) -> Result<&mut dyn ModAudio, CtxError> {
        self.caps.require(Cap::Audio)?;
        self.inner.audio()
    }

    fn assets(&self) -> Result<&dyn ModAssets, CtxError> {
        self.caps.require(Cap::Assets)?;
        self.inner.assets()
    }

    fn hud_textures(&mut self) -> Result<&mut dyn HudTextures, CtxError> {
        self.caps.require(Cap::Hud)?;
        self.inner.hud_textures()
    }

    fn screen(&self) -> ScreenInfo {
        self.inner.screen()
    }

    fn gpu(&self) -> Result<&ModGpu, CtxError> {
        self.caps.require(Cap::RenderPipeline)?;
        self.inner.gpu()
    }

    fn hover_plot(&self) -> Result<Option<PlotIdx>, CtxError> {
        self.caps.require(Cap::WorldRead)?;
        self.inner.hover_plot()
    }

    fn strategic_camera(&self) -> Option<StrategicCamera> {
        self.inner.strategic_camera()
    }

    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        self.caps.require(Cap::InputClaim)?;
        self.inner.request_pointer_lock(lock)
    }
}
