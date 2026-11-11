//! Plain values that cross the client mod boundary (D2): ids, small structs, bytes. No engine type appears here
//! except `platform::InputEvent` and `draw_list` types, which are already value types shared by the whole client.

use mod_api::{PlayerId, PlotClass, Version};
use mod_manifest::ModId;
use render::ModCamera;

/// One mod of the game as the client sees it: its place in the resolved load order and what it may do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModInfo {
    /// Index in the game's resolved load order; the `mod_idx` of `ClientMsg::Mod` / `GameMsg::Mod`.
    pub mod_idx: u16,
    pub id: ModId,
    pub version: Version,
    pub caps: mod_api::Caps,
}

/// What `ClientMod::attach` is told about the game it joined. Built from the protocol's `ModSetView` by
/// `ClientRegistry::instantiate_set` (T35); it carries only the facts a mod needs from it.
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

/// What a mod returns from `frame`.
#[derive(Default)]
pub struct FrameOut {
    /// The camera the mod wants instead of the strategic one for this frame (needs the `camera` capability). The
    /// first mod in load order wins. `render::FirstPerson` is a ready one; a mod may implement `ModCamera` itself
    /// (planes, shadow range, far-LOD distance, terrain focus, listener position). While a camera is returned the app
    /// moves the strategic camera to its `terrain_focus`, so labels, minimap, picking and world sound follow.
    pub camera: Option<Box<dyn ModCamera>>,
    /// Hide the game's own UI while the mod owns the screen (needs `input.claim`).
    pub suppress_ui: bool,
}

impl FrameOut {
    /// A frame that holds the camera.
    pub fn with_camera(camera: impl ModCamera + 'static) -> Self {
        Self {
            camera: Some(Box::new(camera)),
            suppress_ui: false,
        }
    }
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

/// The pose of the strategic (game) camera, in world units, for a mod that starts from the real view (a descent
/// animation) or wants to know where the player looks. `yaw` and `pitch` use the convention of
/// [`FirstPerson`](crate::gpu::FirstPerson): yaw 0 looks along -Z and grows to +X; pitch is positive looking up, so a
/// strategic view has a negative pitch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrategicCamera {
    pub eye: [f32; 3],
    /// The point on the ground in the middle of the screen.
    pub look_at: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Vertical field of view, radians, for the current window.
    pub fov_y: f32,
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

/// What this binary knows about a mod it contains, declared next to its registration: the version and the
/// capabilities of its manifest (the wire carries neither the capabilities nor a client's code identity) and the
/// hash of the build. The host that assembles the client fills it in from `mod.toml` (`ClientRegistry::set_build`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModBuild {
    pub version: Version,
    pub caps: mod_api::Caps,
    /// Hash of the code identity of the build (16 bytes of the manifest crate's digest); sent in `ModHello`.
    pub code_hash: [u8; 16],
}

impl Default for ModBuild {
    /// A mod registered with no declared build: version 0.0.0, no capabilities, no hash. A server never has such a
    /// version, so it is never attached to a game; it exists for tests of the registry.
    fn default() -> Self {
        Self {
            version: Version::new(0, 0, 0),
            caps: mod_api::Caps::none(),
            code_hash: [0; 16],
        }
    }
}

/// A value a mod writes into its UI v2 model fields or receives with an intent of its panels (API 1.3). The host
/// converts it to the type the field declares in the mod's `ui/model.toml`: `Int` to `i16`/`i32`/`u32` with a range
/// check, `Num` to `f32`, `Str` to `str`/`text`; a value that does not fit is refused with the field's name.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UiValue {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
}

impl UiValue {
    /// The integer, if this is one.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// The text, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The flag, if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The number, for an `Int` or a `Num`.
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Self::Int(n) => Some(*n as f64),
            Self::Num(x) => Some(*x),
            _ => None,
        }
    }
}

impl From<bool> for UiValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for UiValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<i32> for UiValue {
    fn from(v: i32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<u32> for UiValue {
    fn from(v: u32) -> Self {
        Self::Int(i64::from(v))
    }
}

impl From<f64> for UiValue {
    fn from(v: f64) -> Self {
        Self::Num(v)
    }
}

impl From<f32> for UiValue {
    fn from(v: f32) -> Self {
        Self::Num(f64::from(v))
    }
}

impl From<&str> for UiValue {
    fn from(v: &str) -> Self {
        Self::Str(v.to_string())
    }
}

impl From<String> for UiValue {
    fn from(v: String) -> Self {
        Self::Str(v)
    }
}

/// One mod of a game's frozen set as the server announced it (`GameMsg::Mods`), as plain values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetEntry {
    pub id: String,
    pub version: String,
    /// Position in the game's load order: the `mod_idx` of `ClientMsg::Mod` and `GameMsg::Mod`.
    pub load_idx: u16,
    /// The server found the mod in this client's `ModHello` with the version of the set.
    pub available: bool,
}
