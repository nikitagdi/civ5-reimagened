//! The registry of client mods built into this binary, and the headless dispatcher that runs them.

use std::collections::{BTreeMap, BTreeSet};

use draw_list::UiDrawList;
use mod_api::{Cap, ChannelId, LogLevel, PlayerId, RegistryError};
use mod_manifest::ModId;
use platform::InputEvent;

use crate::client_mod::{ClientMod, ModRenderSystem};
use crate::ctx::{ClientHost, Guarded};
use crate::types::{AttachInfo, Claim, FrameOut, ModInfo};

type Factory = Box<dyn Fn() -> Box<dyn ClientMod>>;

/// Maps mod ids to factories. A mod crate exposes `pub fn register(r: &mut ClientRegistry)`. Same convention as
/// `mod_api::ServerRegistry`: a bad or duplicate id is recorded in [`problems`](Self::problems), the first
/// registration wins, and the host must check `problems()` after `register`.
#[derive(Default)]
pub struct ClientRegistry {
    factories: BTreeMap<ModId, Factory>,
    problems: Vec<RegistryError>,
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add<F>(&mut self, id: &str, factory: F) -> &mut Self
    where
        F: Fn() -> Box<dyn ClientMod> + 'static,
    {
        match ModId::new(id) {
            Err(reason) => self.problems.push(RegistryError::BadId {
                id: id.to_string(),
                reason,
            }),
            Ok(id) if self.factories.contains_key(&id) => {
                self.problems.push(RegistryError::Duplicate(id));
            }
            Ok(id) => {
                self.factories.insert(id, Box::new(factory));
            }
        }
        self
    }

    /// A fresh instance of the mod, if registered.
    pub fn create(&self, id: &ModId) -> Option<Box<dyn ClientMod>> {
        self.factories.get(id).map(|f| f())
    }

    pub fn ids(&self) -> impl Iterator<Item = &ModId> {
        self.factories.keys()
    }

    pub fn problems(&self) -> &[RegistryError] {
        &self.problems
    }

    /// Builds the dispatcher for a game's resolved mod set (in load order). Mods this binary does not contain are
    /// returned as the second value; what that means (a `ModMismatch` for a required mod, nothing for an optional
    /// one) is the network layer's decision (T35).
    pub fn instantiate(&self, set: &[ModInfo]) -> (ModDispatcher, Vec<ModId>) {
        let mut d = ModDispatcher::new();
        let mut missing = Vec::new();
        for info in set {
            match self.create(&info.id) {
                Some(m) => d.push(info.clone(), m, set.to_vec()),
                None => missing.push(info.id.clone()),
            }
        }
        (d, missing)
    }
}

struct Slot {
    info: ModInfo,
    set: Vec<ModInfo>,
    m: Box<dyn ClientMod>,
    enabled: bool,
    /// Capabilities a mod used in a result without holding them; each is logged once.
    warned: BTreeSet<Cap>,
}

/// Runs the client mods in load order against a host. No wgpu, no windowing: everything the host provides comes in
/// as a `&mut dyn ClientHost`. With no mods every method is a no-op (D11).
#[derive(Default)]
pub struct ModDispatcher {
    slots: Vec<Slot>,
}

impl ModDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a mod at the end of the load order. `set` is the whole resolved set `attach` will report.
    pub fn push(&mut self, info: ModInfo, m: Box<dyn ClientMod>, set: Vec<ModInfo>) {
        self.slots.push(Slot {
            info,
            set,
            m,
            enabled: true,
            warned: BTreeSet::new(),
        });
    }

    /// No mods at all: the app can skip every hook.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn mods(&self) -> impl Iterator<Item = &ModInfo> {
        self.slots.iter().map(|s| &s.info)
    }

    pub fn is_enabled(&self, mod_idx: u16) -> bool {
        self.slots
            .iter()
            .any(|s| s.info.mod_idx == mod_idx && s.enabled)
    }

    /// Switches a mod off or on without detaching it (no further calls reach it). Returns false for an unknown
    /// index. Use `detach_one` first when the mod holds state in the host (pointer lock, cutouts).
    pub fn set_enabled(&mut self, mod_idx: u16, enabled: bool) -> bool {
        match self.slots.iter_mut().find(|s| s.info.mod_idx == mod_idx) {
            Some(s) => {
                s.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// `attach` for every enabled mod.
    pub fn attach(&mut self, host: &mut dyn ClientHost) {
        for s in self.slots.iter_mut().filter(|s| s.enabled) {
            let info = AttachInfo {
                this: s.info.clone(),
                set: s.set.clone(),
            };
            let mut g = Guarded::new(host.ctx(s.info.mod_idx), &s.info.caps);
            s.m.attach(&mut g, &info);
        }
    }

    /// `detach` for every enabled mod, in reverse load order.
    pub fn detach(&mut self, host: &mut dyn ClientHost) {
        for s in self.slots.iter_mut().rev().filter(|s| s.enabled) {
            let mut g = Guarded::new(host.ctx(s.info.mod_idx), &s.info.caps);
            s.m.detach(&mut g);
        }
    }

    /// Detaches one mod and switches it off.
    pub fn detach_one(&mut self, host: &mut dyn ClientHost, mod_idx: u16) {
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.info.mod_idx == mod_idx && s.enabled)
        {
            let mut g = Guarded::new(host.ctx(s.info.mod_idx), &s.info.caps);
            s.m.detach(&mut g);
            s.enabled = false;
        }
    }

    /// Delivers `GameMsg::Mod` to the mod at `mod_idx`. Needs `net.channel`; a message for a mod without it, for a
    /// disabled mod or for an unknown index is dropped. Returns whether a mod saw it.
    pub fn on_msg(
        &mut self,
        host: &mut dyn ClientHost,
        mod_idx: u16,
        ch: ChannelId,
        from: Option<PlayerId>,
        payload: &[u8],
    ) -> bool {
        let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.info.mod_idx == mod_idx && s.enabled)
        else {
            return false;
        };
        if !s.info.caps.has(Cap::NetChannel) {
            return false;
        }
        let mut g = Guarded::new(host.ctx(mod_idx), &s.info.caps);
        s.m.on_msg(&mut g, ch, from, payload);
        true
    }

    /// A registered hotkey fired. Returns whether a mod saw it.
    pub fn on_hotkey(&mut self, host: &mut dyn ClientHost, mod_idx: u16, id: &str) -> bool {
        let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.info.mod_idx == mod_idx && s.enabled)
        else {
            return false;
        };
        let mut g = Guarded::new(host.ctx(mod_idx), &s.info.caps);
        s.m.on_hotkey(&mut g, id);
        true
    }

    /// Offers the event to the mods that hold `input.claim`, in load order; the first `Consume` wins and the later
    /// mods do not see the event.
    pub fn claim_input(&mut self, host: &mut dyn ClientHost, ev: &InputEvent) -> Claim {
        for s in self
            .slots
            .iter_mut()
            .filter(|s| s.enabled && s.info.caps.has(Cap::InputClaim))
        {
            let mut g = Guarded::new(host.ctx(s.info.mod_idx), &s.info.caps);
            if s.m.claim_input(&mut g, ev) == Claim::Consume {
                return Claim::Consume;
            }
        }
        Claim::Pass
    }

    /// Runs `frame` of every enabled mod and merges the results: the first camera override (from a mod holding
    /// `camera`) wins, `suppress_ui` is the OR over mods holding `input.claim`. A mod that returns something it has
    /// no capability for is logged once and the part is dropped.
    pub fn frame(&mut self, host: &mut dyn ClientHost, dt: f32) -> FrameOut {
        let mut out = FrameOut::default();
        for s in self.slots.iter_mut().filter(|s| s.enabled) {
            let idx = s.info.mod_idx;
            let r = {
                let mut g = Guarded::new(host.ctx(idx), &s.info.caps);
                s.m.frame(&mut g, dt)
            };
            if let Some(cam) = r.camera {
                if s.info.caps.has(Cap::Camera) {
                    out.camera.get_or_insert(cam);
                } else {
                    warn_once(s, host, Cap::Camera, "returned a camera override");
                }
            }
            if r.suppress_ui {
                if s.info.caps.has(Cap::InputClaim) {
                    out.suppress_ui = true;
                } else {
                    warn_once(s, host, Cap::InputClaim, "asked to suppress the game UI");
                }
            }
        }
        out
    }

    /// Lets every mod holding `hud` draw into `list`, in load order.
    pub fn hud(&mut self, host: &mut dyn ClientHost, list: &mut UiDrawList) {
        for s in self
            .slots
            .iter_mut()
            .filter(|s| s.enabled && s.info.caps.has(Cap::Hud))
        {
            let mut g = Guarded::new(host.ctx(s.info.mod_idx), &s.info.caps);
            s.m.hud(&mut g, list);
        }
    }

    /// Collects the render systems of the mods holding `render.pipeline`, tagged with the mod index.
    pub fn render_systems(
        &mut self,
        host: &mut dyn ClientHost,
    ) -> Vec<(u16, Box<dyn ModRenderSystem>)> {
        let mut out = Vec::new();
        for s in self.slots.iter_mut().filter(|s| s.enabled) {
            let idx = s.info.mod_idx;
            if !s.info.caps.has(Cap::RenderPipeline) {
                continue;
            }
            let mut g = Guarded::new(host.ctx(idx), &s.info.caps);
            out.extend(s.m.render_systems(&mut g).into_iter().map(|r| (idx, r)));
        }
        out
    }
}

fn warn_once(s: &mut Slot, host: &mut dyn ClientHost, cap: Cap, what: &str) {
    if s.warned.insert(cap) {
        host.ctx(s.info.mod_idx).log(
            LogLevel::Warn,
            &format!(
                "mod `{}` {what} without the `{cap}` capability; ignored",
                s.info.id
            ),
        );
    }
}
