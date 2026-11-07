//! The client mod API: the `ClientMod` trait, the `ClientCtx` facade, the registry and a headless dispatcher.
//! Spec: `specs/modding/SPEC-mod-client.md` in the workspace repository.
//!
//! Only values cross the boundary (D2): bytes, plain structs and ids. A mod never receives `App`, `Renderer` or a
//! wgpu object. The dispatcher runs the mods in load order, checks their capabilities, and is a no-op with no mods
//! (D11). It needs no window or GPU, so a mod's behaviour is testable with [`testing::MockClientCtx`].
//!
//! The few engine types a client mod needs are the engine's own, re-exported here (`render`): the camera
//! (`ModCamera`, `FirstPerson`), the cutout region and grid, the render system and `ModGpu` (see [`gpu`]). A mod
//! crate depends on `mod-client` only (rule R11).
//!
//! ```
//! use mod_api::{Cap, Caps, Version};
//! use mod_client::testing::MockClientCtx;
//! use mod_client::{AttachInfo, ClientCtx, ClientMod, ClientRegistry, ModInfo};
//! use mod_manifest::ModId;
//!
//! struct Hello;
//! impl ClientMod for Hello {
//!     fn attach(&mut self, cx: &mut dyn ClientCtx, _info: &AttachInfo) {
//!         cx.send(0, b"hi").unwrap();
//!     }
//! }
//!
//! pub fn register(r: &mut ClientRegistry) {
//!     r.add("hello", || Box::new(Hello));
//! }
//!
//! let mut reg = ClientRegistry::new();
//! register(&mut reg);
//! let info = ModInfo {
//!     mod_idx: 0,
//!     id: ModId::new("hello").unwrap(),
//!     version: Version::new(1, 0, 0),
//!     caps: Caps::of(&[Cap::NetChannel]),
//! };
//! let (mut mods, missing) = reg.instantiate(&[info]);
//! assert!(missing.is_empty());
//! let mut host = MockClientCtx::new();
//! mods.attach(&mut host);
//! assert_eq!(host.sent_on(0, 0), [&b"hi"[..]]);
//! ```

#![forbid(unsafe_code)]

mod client_mod;
mod ctx;
mod hotkeys;
mod kv;
mod registry;
pub mod testing;
mod types;

pub use client_mod::{ClientMod, ModRenderSystem};
pub use ctx::{
    ClientCtx, ClientHost, CtxError, CutoutMask, Guarded, Hotkeys, HudTextures, ModAssets,
    ModAudio, ModKv, ModTerrain, ModWorld,
};
pub use draw_list::{TextureId, UiBlend, UiDrawList, UiVertex};
pub use hotkeys::{HotkeyTable, KeyUse, game_key};
pub use kv::{KV_KEY_MAX, KV_VALUE_MAX, NamespacedKv};
pub use mod_api::{LogLevel, MapShape, PlayerId, PlotClass, PlotIdx};
pub use platform::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, NamedKey, PhysicalPosition, PhysicalSize,
    PointerEvent, PointerId, PointerKind, PointerPhase, WheelDelta, WheelEvent,
};
pub use registry::{ClientRegistry, ModDispatcher};
pub use render::cutout::{CutoutGrid, CutoutRegion};
pub use types::{
    AttachInfo, Claim, FrameOut, HotkeySpec, ModBuild, ModInfo, PackState, PlayOpts, PlayerInfo,
    PlotView, ScreenInfo, SetEntry, StrategicCamera,
};

/// Labeled timing of a mod's own work, for the engine's slow-frame log and the performance panel. Hold the guard
/// for the block to time: `let _s = mod_client::perf::scope("mod.<id>.mesh");`. Name the scopes `mod.<id>.<what>` so
/// they read apart from the engine's. Costs a flag check when profiling is off.
pub mod perf {
    pub use render::perf::slow::{Scope, scope};
}

/// The engine types behind the hooks, for mods that draw or move the camera. A mod does not name the `render` or
/// `wgpu` crates: R11 lets it depend on `mod-client` only.
pub mod gpu {
    pub use render::frame::{
        DrawContext, FrameShared, InitContext, MainSlot, ModGpu, PassSlot, PrepareContext,
        RenderSystem, SystemId,
    };
    pub use render::glam;
    pub use render::wgpu;
    pub use render::{CameraHints, CameraView, FirstPerson, ModCamera};
}
