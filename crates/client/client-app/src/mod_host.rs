//! The host the client mods run against in the app: the services `mod_client::ClientCtx` hands out, over the
//! engine's own state.
//!
//! [`HostState`] lives as long as the hook; [`HostView`] is made for one call into the dispatcher and implements
//! `ClientHost` and `ClientCtx` over it. It is unchecked: the capabilities are enforced by `mod_client::Guarded`, which
//! the dispatcher puts around every context it hands to a mod.

use client_core::NetHost;
use mod_api::{Cap, LogLevel};
use mod_client::{
    ClientCtx, ClientHost, CtxError, CutoutMask, Hotkeys, HudTextures, ModAssets, ModAudio, ModKv,
    ModTerrain, ModWorld, ScreenInfo,
};

use crate::mod_assets::PackSet;

/// What outlives a call: the queue of messages and the pointer request (`NetHost`) and the asset packs.
pub struct HostState {
    pub net: NetHost,
    /// `None` until the app told the hook where the server is (`ModHook::bind`).
    pub packs: Option<PackSet>,
}

/// The host for one call: the state, and which mod it speaks for.
pub struct HostView<'a> {
    st: &'a mut HostState,
    cur: u16,
}

impl<'a> HostView<'a> {
    pub fn new(st: &'a mut HostState) -> Self {
        Self { st, cur: 0 }
    }
}

impl ClientHost for HostView<'_> {
    fn ctx(&mut self, mod_idx: u16) -> &mut dyn ClientCtx {
        self.cur = mod_idx;
        // `NetHost` keeps the index for its log lines and the sender of a message.
        self.st.net.ctx(mod_idx);
        if let Some(p) = self.st.packs.as_mut() {
            p.select(mod_idx);
        }
        self
    }
}

impl ClientCtx for HostView<'_> {
    fn has(&self, _cap: Cap) -> bool {
        true
    }

    fn log(&mut self, level: LogLevel, msg: &str) {
        self.st.net.log(level, msg);
    }

    fn send(&mut self, ch: u8, payload: &[u8]) -> Result<(), CtxError> {
        self.st.net.send(ch, payload)
    }

    fn world(&self) -> Result<&dyn ModWorld, CtxError> {
        Err(CtxError::Unavailable("world"))
    }

    fn terrain(&self) -> Result<&dyn ModTerrain, CtxError> {
        Err(CtxError::Unavailable("terrain"))
    }

    fn cutout(&mut self) -> Result<&mut dyn CutoutMask, CtxError> {
        Err(CtxError::Unavailable("cutout"))
    }

    fn kv(&mut self) -> Result<&mut dyn ModKv, CtxError> {
        Err(CtxError::Unavailable("kv"))
    }

    fn hotkeys(&mut self) -> Result<&mut dyn Hotkeys, CtxError> {
        Err(CtxError::Unavailable("hotkeys"))
    }

    fn audio(&mut self) -> Result<&mut dyn ModAudio, CtxError> {
        Err(CtxError::Unavailable("audio"))
    }

    fn assets(&self) -> Result<&dyn ModAssets, CtxError> {
        self.st
            .packs
            .as_ref()
            .map(|p| p as &dyn ModAssets)
            .ok_or(CtxError::Unavailable("assets"))
    }

    fn hud_textures(&mut self) -> Result<&mut dyn HudTextures, CtxError> {
        Err(CtxError::Unavailable("hud textures"))
    }

    fn screen(&self) -> ScreenInfo {
        self.st.net.screen()
    }

    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        self.st.net.request_pointer_lock(lock)
    }
}
