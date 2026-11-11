//! The registry of client mods built into this binary, and the headless dispatcher that runs them.

use std::collections::{BTreeMap, BTreeSet};

use draw_list::UiDrawList;
use mod_api::{Cap, ChannelId, LogLevel, PlayerId, RegistryError};
use mod_manifest::ModId;
use platform::InputEvent;

use crate::client_mod::{ClientMod, ModRenderSystem};
use crate::ctx::{ClientHost, Control, Guarded};
use crate::types::{AttachInfo, Claim, FrameOut, ModBuild, ModInfo, SetEntry, UiValue};

type Factory = Box<dyn Fn() -> Box<dyn ClientMod>>;

/// Maps mod ids to factories. A mod crate exposes `pub fn register(r: &mut ClientRegistry)`. Same convention as
/// `mod_api::ServerRegistry`: a bad or duplicate id is recorded in [`problems`](Self::problems), the first
/// registration wins, and the host must check `problems()` after `register`.
#[derive(Default)]
pub struct ClientRegistry {
    factories: BTreeMap<ModId, Factory>,
    builds: BTreeMap<ModId, ModBuild>,
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

    /// [`add`](Self::add) with the build facts of the mod at once.
    pub fn add_built<F>(&mut self, id: &str, build: ModBuild, factory: F) -> &mut Self
    where
        F: Fn() -> Box<dyn ClientMod> + 'static,
    {
        self.add(id, factory);
        self.set_build(id, build)
    }

    /// Declares the version, capabilities and code hash of a registered mod (taken from its `mod.toml` by the host
    /// that assembles the client). An unknown id is ignored: `add` has already recorded why it did not register.
    pub fn set_build(&mut self, id: &str, build: ModBuild) -> &mut Self {
        if let Ok(id) = ModId::new(id)
            && self.factories.contains_key(&id)
        {
            self.builds.insert(id, build);
        }
        self
    }

    /// What is known of a registered mod's build (the default for one that declared none).
    pub fn build(&self, id: &ModId) -> Option<ModBuild> {
        self.factories
            .contains_key(id)
            .then(|| self.builds.get(id).cloned().unwrap_or_default())
    }

    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
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

impl ClientRegistry {
    /// Builds the dispatcher for the set a server announced. A mod joins it when the server marks it `available`
    /// and this binary contains it; its capabilities are the ones its build declares. The mods keep the load order
    /// and the `load_idx` the server gave them (the index of every message). The second value lists the mods of the
    /// set that stay out (not available, or not in this binary), in load order: the server has already refused a
    /// client that lacks a required one, so these are optional mods.
    pub fn instantiate_set(&self, set: &[SetEntry]) -> (ModDispatcher, Vec<ModId>) {
        let mut present = Vec::new();
        let mut missing = Vec::new();
        for e in set {
            let Ok(id) = ModId::new(&e.id) else { continue };
            match self.build(&id) {
                Some(build) if e.available => present.push(ModInfo {
                    mod_idx: e.load_idx,
                    version: e.version.parse().unwrap_or(build.version),
                    caps: build.caps,
                    id,
                }),
                _ => missing.push(id),
            }
        }
        let mut d = ModDispatcher::new();
        for info in &present {
            if let Some(m) = self.create(&info.id) {
                d.push(info.clone(), m, present.clone());
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
    /// The holder of the camera this mod was told about (its override lost to it); logged once per holder.
    warned_busy: Option<u16>,
}

/// Runs the client mods in load order against a host. No wgpu, no windowing: everything the host provides comes in
/// as a `&mut dyn ClientHost`. With no mods every method is a no-op (D11).
///
/// It also keeps who holds the camera and the input (API 1.1, `ClientCtx::acquire_control`): one mod at a time.
#[derive(Default)]
pub struct ModDispatcher {
    slots: Vec<Slot>,
    control: Control,
}

/// The checked context of one slot; the dispatcher's [`Control`] arbitrates the camera and the input.
macro_rules! guarded {
    ($host:expr, $s:expr, $control:expr) => {
        Guarded::arbitrated(
            $host.ctx($s.info.mod_idx),
            &$s.info.caps,
            $control,
            $s.info.mod_idx,
            &$s.info.id,
        )
    };
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
            warned_busy: None,
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

    /// The mod that holds the camera and the input (API 1.1): the one that acquired them, or the one whose camera
    /// override the last frame used. `None`: the game has them.
    pub fn control_holder(&self) -> Option<(u16, &ModId)> {
        self.control.holder().map(|(i, id)| (*i, id))
    }

    /// Switches a mod off or on without detaching it (no further calls reach it). Returns false for an unknown
    /// index. Use `detach_one` first when the mod holds state in the host (pointer lock, cutouts). A mod switched
    /// off gives up the camera and the input.
    pub fn set_enabled(&mut self, mod_idx: u16, enabled: bool) -> bool {
        match self.slots.iter_mut().find(|s| s.info.mod_idx == mod_idx) {
            Some(s) => {
                s.enabled = enabled;
                if !enabled {
                    self.control.forget(mod_idx);
                }
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
            let mut g = guarded!(host, s, &mut self.control);
            s.m.attach(&mut g, &info);
        }
    }

    /// `detach` for every enabled mod, in reverse load order. Nobody holds the camera and the input after it.
    pub fn detach(&mut self, host: &mut dyn ClientHost) {
        for s in self.slots.iter_mut().rev().filter(|s| s.enabled) {
            let mut g = guarded!(host, s, &mut self.control);
            s.m.detach(&mut g);
        }
        self.control = Control::default();
    }

    /// Detaches one mod and switches it off (it gives up the camera and the input).
    pub fn detach_one(&mut self, host: &mut dyn ClientHost, mod_idx: u16) {
        if let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.info.mod_idx == mod_idx && s.enabled)
        {
            let mut g = guarded!(host, s, &mut self.control);
            s.m.detach(&mut g);
            s.enabled = false;
            self.control.forget(mod_idx);
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
        let mut g = guarded!(host, s, &mut self.control);
        s.m.on_msg(&mut g, ch, from, payload);
        true
    }

    /// A registered hotkey fired. Returns whether a mod saw it. Hotkeys reach a mod even while another one holds
    /// the input, so it can try `acquire_control` and tell the player why it cannot.
    pub fn on_hotkey(&mut self, host: &mut dyn ClientHost, mod_idx: u16, id: &str) -> bool {
        let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.info.mod_idx == mod_idx && s.enabled)
        else {
            return false;
        };
        let mut g = guarded!(host, s, &mut self.control);
        s.m.on_hotkey(&mut g, id);
        true
    }

    /// Delivers an intent of the UI v2 panels to the mod at `mod_idx` (API 1.3): `name` is the part after
    /// `mods.<id>.`. Only an enabled mod holding `ui` gets it; returns whether a mod saw it. The host has already
    /// checked that the intent came from this mod's own panels or script.
    pub fn on_ui_intent(
        &mut self,
        host: &mut dyn ClientHost,
        mod_idx: u16,
        name: &str,
        args: &[(String, UiValue)],
    ) -> bool {
        let Some(s) = self
            .slots
            .iter_mut()
            .find(|s| s.info.mod_idx == mod_idx && s.enabled)
        else {
            return false;
        };
        if !s.info.caps.has(Cap::Ui) {
            return false;
        }
        let mut g = guarded!(host, s, &mut self.control);
        s.m.on_ui_intent(&mut g, name, args);
        true
    }

    /// The load index of the enabled mod with this id, for a host that routes by id (UI v2 intents name the mod).
    pub fn idx_of(&self, id: &str) -> Option<u16> {
        self.slots
            .iter()
            .find(|s| s.enabled && s.info.id.as_str() == id)
            .map(|s| s.info.mod_idx)
    }

    /// Offers the event to the mods that hold `input.claim`, in load order; the first `Consume` wins and the later
    /// mods do not see the event. While a mod holds the input through `acquire_control`, only that mod is offered
    /// events.
    pub fn claim_input(&mut self, host: &mut dyn ClientHost, ev: &InputEvent) -> Claim {
        let lease = self.control.lease.as_ref().map(|(i, _)| *i);
        for s in self.slots.iter_mut().filter(|s| {
            s.enabled
                && s.info.caps.has(Cap::InputClaim)
                && lease.is_none_or(|l| l == s.info.mod_idx)
        }) {
            let mut g = guarded!(host, s, &mut self.control);
            if s.m.claim_input(&mut g, ev) == Claim::Consume {
                return Claim::Consume;
            }
        }
        Claim::Pass
    }

    /// Runs `frame` of every enabled mod and merges the results: the first camera override (from a mod holding
    /// `camera`) wins, `suppress_ui` is the OR over mods holding `input.claim`. A mod that returns something it has
    /// no capability for is logged once and the part is dropped.
    ///
    /// While a mod holds the camera and the input (`acquire_control`, as it stands after every mod's `frame`), only
    /// its camera override and `suppress_ui` count; another mod's are dropped with one line in the log. Without a
    /// lease, the mod whose camera won holds them until a frame where it returns none.
    pub fn frame(&mut self, host: &mut dyn ClientHost, dt: f32) -> FrameOut {
        // (mod_idx, camera, suppress_ui) of every mod, with the capability checks applied.
        let mut parts = Vec::with_capacity(self.slots.len());
        for s in self.slots.iter_mut().filter(|s| s.enabled) {
            let idx = s.info.mod_idx;
            let r = {
                let mut g = guarded!(host, s, &mut self.control);
                s.m.frame(&mut g, dt)
            };
            let camera = match r.camera {
                Some(_) if !s.info.caps.has(Cap::Camera) => {
                    warn_once(s, host, Cap::Camera, "returned a camera override");
                    None
                }
                c => c,
            };
            let suppress = r.suppress_ui && {
                let ok = s.info.caps.has(Cap::InputClaim);
                if !ok {
                    warn_once(s, host, Cap::InputClaim, "asked to suppress the game UI");
                }
                ok
            };
            parts.push((idx, camera, suppress));
        }
        let lease = self.control.lease.clone();
        let mut out = FrameOut::default();
        let mut used = None;
        for (idx, camera, suppress) in parts {
            if let Some((holder, holder_id)) = &lease
                && *holder != idx
            {
                if camera.is_some() || suppress {
                    self.warn_busy(host, idx, *holder, holder_id);
                }
                continue;
            }
            if let Some(cam) = camera
                && out.camera.is_none()
            {
                out.camera = Some(cam);
                used = Some(idx);
            }
            out.suppress_ui |= suppress;
        }
        self.control.implicit = match (&lease, used) {
            (None, Some(idx)) => self
                .slots
                .iter()
                .find(|s| s.info.mod_idx == idx)
                .map(|s| (idx, s.info.id.clone())),
            _ => None,
        };
        out
    }

    /// Lets every mod holding `hud` draw into `list`, in load order.
    pub fn hud(&mut self, host: &mut dyn ClientHost, list: &mut UiDrawList) {
        for s in self
            .slots
            .iter_mut()
            .filter(|s| s.enabled && s.info.caps.has(Cap::Hud))
        {
            let mut g = guarded!(host, s, &mut self.control);
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
            let mut g = guarded!(host, s, &mut self.control);
            out.extend(s.m.render_systems(&mut g).into_iter().map(|r| (idx, r)));
        }
        out
    }

    /// Mod `idx` returned a camera or `suppress_ui` while `holder` holds them: one line per holder.
    fn warn_busy(&mut self, host: &mut dyn ClientHost, idx: u16, holder: u16, holder_id: &ModId) {
        let Some(s) = self.slots.iter_mut().find(|s| s.info.mod_idx == idx) else {
            return;
        };
        if s.warned_busy == Some(holder) {
            return;
        }
        s.warned_busy = Some(holder);
        host.ctx(idx).log(
            LogLevel::Warn,
            &format!(
                "mod `{}` returned a camera or suppress_ui while mod `{holder_id}` holds the camera and the input; \
                 ignored",
                s.info.id
            ),
        );
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
