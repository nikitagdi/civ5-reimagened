//! The client mod API: the `ClientMod` trait, the `ClientCtx` facade, the registry and a headless dispatcher.
//! Spec: `specs/modding/SPEC-mod-client.md` in the workspace repository.
//!
//! Only values cross the boundary (D2): bytes, plain structs and ids. A mod never receives `App`, `Renderer` or a
//! wgpu object. The dispatcher runs the mods in load order, checks their capabilities, and is a no-op with no mods
//! (D11). It needs no window or GPU, so a mod's behaviour is testable with [`testing::MockClientCtx`].
//!
//! Some hook types do not exist in the engine yet; this crate carries marked stand-ins (`CameraOverride`,
//! `ModRenderSystem`, `CutoutRegion`, `AttachInfo`) that the wiring tasks T35/T36 replace or connect.
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
mod kv;
mod registry;
pub mod testing;
mod types;

pub use client_mod::{ClientMod, ModRenderSystem};
pub use ctx::{
    ClientCtx, ClientHost, CtxError, CutoutMask, Guarded, Hotkeys, HudTextures, ModAssets,
    ModAudio, ModKv, ModTerrain, ModWorld,
};
pub use kv::{KV_KEY_MAX, KV_VALUE_MAX, NamespacedKv};
pub use registry::{ClientRegistry, ModDispatcher};
pub use types::{
    AttachInfo, CameraOverride, Claim, CutoutRegion, FrameOut, HotkeySpec, ModInfo, PackState,
    PlayOpts, PlayerInfo, PlotView, ScreenInfo,
};
