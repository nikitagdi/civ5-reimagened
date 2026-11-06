//! Plain values that cross the client mod boundary (D2): ids, small structs, bytes. No engine type appears here
//! except `platform::InputEvent` and `draw_list` types, which are already value types shared by the whole client.

use mod_api::{PlayerId, PlotClass, Version};
use mod_manifest::ModId;

/// One mod of the game as the client sees it: its place in the resolved load order and what it may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    /// Index in the game's resolved load order; the `mod_idx` of `ClientMsg::Mod` / `GameMsg::Mod`.
    pub mod_idx: u16,
    pub id: ModId,
    pub version: Version,
    pub caps: mod_api::Caps,
}

/// What `ClientMod::attach` is told about the game it joined.
///
/// TO-BE-WIRED (T35): built from the protocol's `ModSetView` when the lobby/handshake delivers it; until then it
/// carries only the facts a mod needs from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachInfo {
    /// The mod's own entry.
    pub this: ModInfo,
    /// The whole resolved set, in load order (a mod may adapt to an optional sibling being present).
    pub set: Vec<ModInfo>,
}

impl AttachInfo {
    /// The sibling `id`, if the game runs it.
    pub fn sibling(&self, id: &str) -> Option<&ModInfo> {
        self.set.iter().find(|m| m.id.as_str() == id)
    }
}

/// Answer of `ClientMod::claim_input`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Claim {
    /// The mod does not want the event; the next mod, then the game, sees it.
    #[default]
    Pass,
    /// The mod handled it: nobody else sees it.
    Consume,
}

/// A camera the mod wants instead of the strategic one for this frame.
///
/// STAND-IN (T27/T36): replaced by the engine's `CameraView` override (near/far/FOV, shadow range, far-LOD distance,
/// terrain focus, listener position). Only values: the host builds its camera from these.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraOverride {
    /// Eye position in world units.
    pub eye: [f32; 3],
    /// Rotation around the vertical axis, radians.
    pub yaw: f32,
    /// Up/down rotation, radians (negative looks down).
    pub pitch: f32,
    pub near: f32,
    pub far: f32,
    pub fov_y_deg: f32,
    /// Virtual focus for streaming, audio listener, labels and minimap: world XZ.
    pub focus_xz: [f32; 2],
}

/// What a mod returns from `frame`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameOut {
    /// Camera override for this frame (needs the `camera` capability). The first mod in load order wins.
    pub camera: Option<CameraOverride>,
    /// Hide the game's own UI while the mod owns the screen (needs `input.claim`).
    pub suppress_ui: bool,
}

/// A player in the game, as the replica knows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerInfo {
    pub id: PlayerId,
    pub nick: String,
    pub color: [u8; 3],
}

/// What the replica knows about one plot. The client sees through fog only what the player has seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlotView {
    /// Owner as last seen; `None` when unowned or not known.
    pub owner: Option<PlayerId>,
    /// Terrain class; `None` while the plot is unrevealed.
    pub class: Option<PlotClass>,
    pub revealed: bool,
    /// Currently in sight (not just remembered).
    pub visible: bool,
}

impl PlotView {
    /// A plot the player has never seen.
    pub const UNKNOWN: PlotView = PlotView {
        owner: None,
        class: None,
        revealed: false,
        visible: false,
    };
}

/// Window size in physical pixels and the scale factor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenInfo {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// A key binding a mod asks for (handled only when no mod has claimed input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    /// Mod-local id, passed back to `ClientMod::on_hotkey`.
    pub id: String,
    /// DOM `KeyboardEvent.code` name (layout independent), e.g. `KeyB`.
    pub code: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Text for the key-binding list.
    pub label: String,
}

impl HotkeySpec {
    pub fn new(id: &str, code: &str, label: &str) -> Self {
        Self {
            id: id.to_string(),
            code: code.to_string(),
            ctrl: false,
            shift: false,
            alt: false,
            label: label.to_string(),
        }
    }
}

/// Options of one sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayOpts {
    /// World position for positional sound; `None` plays it flat.
    pub pos: Option<[f32; 3]>,
    pub volume: f32,
    /// Playback rate multiplier (1.0 = as recorded).
    pub pitch: f32,
}

impl Default for PlayOpts {
    fn default() -> Self {
        Self {
            pos: None,
            volume: 1.0,
            pitch: 1.0,
        }
    }
}

/// State of the mod's asset pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackState {
    /// Being fetched or checked.
    Pending,
    /// Loaded and verified; `ModAssets::get` answers.
    Ready,
    /// There is no pack (not built, not served). An optional mod disables itself, a required one reports it.
    Missing,
    /// Fetched but rejected (hash, lock or format).
    Failed(String),
}

/// Columns of the terrain mask a mod writes (D13): a rectangle of the column grid.
///
/// TO-BE-WIRED (T28): the column grid and the byte layout (one R8 texel per column) are fixed by the render hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutoutRegion {
    pub x0: i32,
    pub y0: i32,
    pub width: u32,
    pub height: u32,
}

impl CutoutRegion {
    /// Number of bytes a mask for this region holds.
    pub fn byte_len(&self) -> usize {
        self.width as usize * self.height as usize
    }
}
