//! The mod layer of the client core (v13, T35): the client halves of the mods built into this binary, matched
//! against the set a game announces, fed with the messages of their server halves, and a minimal host that turns
//! `ClientCtx::send` into `ClientMsg::Mod`.
//!
//! Still sans-IO. [`ModNet`] owns the registry and the dispatcher and is driven by the events of the core
//! (`ClientEvent::ModSet`, `ModMsg`, `Attached`, `Detached`); the services a mod calls come from the host the shell
//! passes in. [`NetHost`] is the smallest host: it carries the messages a mod sends and the pointer-lock request,
//! and answers every other service with `Unavailable` (the app's host adds the real ones, T36).
//!
//! With an empty registry nothing here does anything: no `ModHello` is sent, every event is ignored at once.

use mod_api::{Cap, LogLevel};
use mod_client::{
    ClientCtx, ClientHost, ClientRegistry, CtxError, CutoutMask, Hotkeys, HudTextures, ModAssets,
    ModAudio, ModDispatcher, ModKv, ModTerrain, ModWorld, ScreenInfo, SetEntry,
};
use protocol::{AttachMode, BuiltMod, ModSetView};

use crate::{ClientCore, ClientEvent, NetConfig};

/// The client halves of the mods built into this binary and the dispatcher of the game that is running.
pub struct ModNet {
    registry: ClientRegistry,
    dispatcher: Option<ModDispatcher>,
    /// The hash of the set the dispatcher was built for.
    set_hash: Option<u64>,
}

impl ModNet {
    pub fn new(registry: ClientRegistry) -> Self {
        Self {
            registry,
            dispatcher: None,
            set_hash: None,
        }
    }

    /// No mod is built in: the app can skip all of it (D11).
    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }

    /// The mods built in, as `ModHello` states them: id, version and code hash of every registered mod.
    pub fn built_in(&self) -> Vec<BuiltMod> {
        self.registry
            .ids()
            .filter_map(|id| {
                let b = self.registry.build(id)?;
                Some(BuiltMod {
                    id: id.to_string(),
                    version: b.version.to_string(),
                    code_hash: b.code_hash,
                })
            })
            .collect()
    }

    /// Puts the built-in mods (and the mod API this client speaks) into the config of the core, so that the next
    /// handshake carries a `ModHello`.
    pub fn configure(&self, cfg: &mut NetConfig) {
        cfg.mod_api = (
            mod_api::MOD_API_VERSION.major as u16,
            mod_api::MOD_API_VERSION.minor as u16,
            mod_api::MOD_API_VERSION.patch as u16,
        );
        cfg.built_in = self.built_in();
    }

    /// The dispatcher of the running game, if its set had a mod this client contains.
    pub fn dispatcher(&mut self) -> Option<&mut ModDispatcher> {
        self.dispatcher.as_mut()
    }

    /// Some mod of the running game is attached.
    pub fn active(&self) -> bool {
        self.dispatcher.as_ref().is_some_and(|d| !d.is_empty())
    }

    /// The hash of the set the mods were attached for.
    pub fn set_hash(&self) -> Option<u64> {
        self.set_hash
    }

    /// Reacts to an event of the core; events that are not about mods or the game's attachment are ignored.
    ///
    /// - `ModSet`: the mods of the previous stream (a fresh stream repeats the set) are detached, the mods present
    ///   in both the set and this binary are created in the load order the server gave and attached; the others
    ///   are skipped (they are optional: the server refused a client that lacks a required one).
    /// - `ModMsg`: delivered to the mod at `mod_idx` (`on_msg(ch, from, payload)`); an unknown index is dropped.
    /// - `Attached` with a fresh stream and `Detached`: the mods of the old stream are detached (the new stream
    ///   brings its own set if the game has mods).
    pub fn on_event(&mut self, ev: &ClientEvent, host: &mut dyn ClientHost) {
        if self.registry.is_empty() {
            return;
        }
        match ev {
            ClientEvent::ModSet(view) => self.set(view, host),
            ClientEvent::ModMsg(m) => {
                if let Some(d) = self.dispatcher.as_mut() {
                    d.on_msg(host, m.mod_idx, m.ch, m.from, &m.payload);
                }
            }
            ClientEvent::Attached {
                mode: AttachMode::Fresh,
                ..
            }
            | ClientEvent::Detached(_) => self.release(host),
            _ => {}
        }
    }

    fn set(&mut self, view: &ModSetView, host: &mut dyn ClientHost) {
        self.release(host);
        let entries: Vec<SetEntry> = view
            .mods
            .iter()
            .map(|m| SetEntry {
                id: m.id.clone(),
                version: m.version.clone(),
                load_idx: m.load_idx,
                available: m.available,
            })
            .collect();
        let (mut dispatcher, skipped) = self.registry.instantiate_set(&entries);
        for id in &skipped {
            tracing::info!(mod_id = %id, "mod of the game is not available in this client, skipped");
        }
        dispatcher.attach(host);
        self.set_hash = Some(view.modset_hash);
        self.dispatcher = Some(dispatcher);
    }

    /// Detaches the mods of the stream that ended.
    fn release(&mut self, host: &mut dyn ClientHost) {
        if let Some(mut d) = self.dispatcher.take() {
            d.detach(host);
        }
        self.set_hash = None;
    }
}

/// One message a mod sent, waiting to go out as `ClientMsg::Mod`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingMod {
    pub mod_idx: u16,
    pub ch: u8,
    pub payload: Vec<u8>,
}

/// Most messages kept between two flushes. The server limits the rate (D7); this only keeps a mod that sends in
/// a loop from growing the queue without end.
const OUTBOX_MAX: usize = 1024;

/// The smallest `ClientHost`: sends and the pointer lock work, every other service is `Unavailable`.
pub struct NetHost {
    cur: u16,
    out: Vec<OutgoingMod>,
    pointer_lock: Option<bool>,
    screen: ScreenInfo,
}

impl Default for NetHost {
    fn default() -> Self {
        Self::new()
    }
}

impl NetHost {
    pub fn new() -> Self {
        Self {
            cur: 0,
            out: Vec::new(),
            pointer_lock: None,
            screen: ScreenInfo {
                width: 0,
                height: 0,
                scale: 1.0,
            },
        }
    }

    pub fn set_screen(&mut self, screen: ScreenInfo) {
        self.screen = screen;
    }

    /// The messages the mods sent since the last call, in order.
    pub fn take_outgoing(&mut self) -> Vec<OutgoingMod> {
        std::mem::take(&mut self.out)
    }

    /// The last pointer-lock request of a mod since the last call.
    pub fn take_pointer_lock(&mut self) -> Option<bool> {
        self.pointer_lock.take()
    }

    pub fn has_outgoing(&self) -> bool {
        !self.out.is_empty()
    }

    /// Sends what the mods queued as `ClientMsg::Mod`. A message the core refuses (no line, no game, too big) is
    /// dropped and logged: a mod's chatter is not worth a retry queue, the mod's own protocol resends what matters.
    pub fn flush(&mut self, core: &mut ClientCore) {
        for m in self.take_outgoing() {
            if let Err(why) = core.send_mod(m.mod_idx, m.ch, m.payload) {
                tracing::debug!(?why, mod_idx = m.mod_idx, ch = m.ch, "mod message dropped");
            }
        }
    }
}

impl ClientHost for NetHost {
    fn ctx(&mut self, mod_idx: u16) -> &mut dyn ClientCtx {
        self.cur = mod_idx;
        self
    }
}

impl ClientCtx for NetHost {
    fn has(&self, _cap: Cap) -> bool {
        true
    }

    fn log(&mut self, level: LogLevel, msg: &str) {
        let idx = self.cur;
        match level {
            LogLevel::Error => tracing::error!(mod_idx = idx, "{msg}"),
            LogLevel::Warn => tracing::warn!(mod_idx = idx, "{msg}"),
            LogLevel::Info => tracing::info!(mod_idx = idx, "{msg}"),
            _ => tracing::debug!(mod_idx = idx, "{msg}"),
        }
    }

    fn send(&mut self, ch: u8, payload: &[u8]) -> Result<(), CtxError> {
        if self.out.len() >= OUTBOX_MAX {
            return Err(CtxError::Rejected("the outgoing queue is full".into()));
        }
        self.out.push(OutgoingMod {
            mod_idx: self.cur,
            ch,
            payload: payload.to_vec(),
        });
        Ok(())
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
        Err(CtxError::Unavailable("assets"))
    }

    fn hud_textures(&mut self) -> Result<&mut dyn HudTextures, CtxError> {
        Err(CtxError::Unavailable("hud textures"))
    }

    fn screen(&self) -> ScreenInfo {
        self.screen
    }

    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        self.pointer_lock = Some(lock);
        Ok(())
    }
}
