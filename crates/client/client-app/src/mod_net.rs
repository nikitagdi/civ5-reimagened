//! The client halves of mods in the app (T35): a `ModHook` that owns the dispatcher of `mod-client` and drives it
//! from the hook points of the app. It carries the mod layer of `client-core` (`ModNet`: the set of the game, the
//! messages of the servers' halves) and turns what the mods return into the requests of `HookCtl`.
//!
//! - network: `configure_net` puts the built-in mods into the handshake (`ModHello`), `on_net_event` feeds
//!   `ModSet`/`ModMsg`/`Attached`/`Detached` to the dispatcher, `flush_net` sends what the mods queued as
//!   `ClientMsg::Mod`;
//! - input: `claim_input` offers the event to the mods holding `input.claim` (the first `Consume` takes it);
//! - frame: `frame` merges the results of the mods: the camera of the first mod holding `camera` becomes a
//!   first-person camera, `suppress_ui` hides the original UI.
//!
//! The services a mod can call are the ones of `client_core::NetHost` (`send` and the pointer lock); the rest answer
//! `Unavailable` until the app's own host (T36). With no mod built in the hook is not registered at all.

use client_core::{ClientCore, ClientEvent, ModNet, NetConfig};
use draw_list::UiDrawList;
use mod_api::Cap;
use mod_client::{Claim, ClientRegistry, PackState, ScreenInfo};
use platform::InputEvent;

use crate::mod_assets::{PackEvent, PackSet, mods_base_from_ws_url};
use crate::mod_hooks::{FrameInfo, HookCtl, HookEnv, ModHook};
use crate::mod_host::{Env, HostState, HostView, KvSet, ModSystem, TerrainView};

/// The hook of the client mods built into this binary.
pub struct ClientModsHook {
    net: ModNet,
    st: HostState,
    /// The hook has raised `suppress_original_ui` / holds the camera / holds the pointer, so it lowers them again
    /// when no mod asks for them any more (or the game ends).
    suppressing: bool,
    camera_held: bool,
    pointer_held: bool,
    /// The mods of the new set have not been asked for their render systems yet (that waits for the renderer).
    systems_pending: bool,
    /// A required mod cannot run on this client (its pack is missing or refused): the app ends the game with it.
    problem: Option<String>,
}

/// What a call into the mods sees besides the host's own state, from the frame the app gave the hook.
fn env_of<'a>(info: &FrameInfo<'a>) -> Env<'a> {
    Env {
        world: info.world,
        terrain: info.terrain.map(TerrainView),
        gpu: info.gpu,
        screen: Some(ScreenInfo {
            width: info.screen.0,
            height: info.screen.1,
            scale: info.screen.2,
        }),
        hover: info.hover,
        strategic: info.strategic,
    }
}

impl ClientModsHook {
    /// A hook over the registry of the built-in mods. `None` for an empty registry: nothing to hook.
    pub fn new(registry: ClientRegistry) -> Option<Self> {
        if registry.is_empty() {
            return None;
        }
        Some(Self {
            net: ModNet::new(registry),
            st: HostState::new(),
            suppressing: false,
            camera_held: false,
            pointer_held: false,
            systems_pending: false,
            problem: None,
        })
    }

    /// The mod layer, for tests and for the host that extends this hook.
    pub fn net(&mut self) -> &mut ModNet {
        &mut self.net
    }

    fn carry_out_pointer(&mut self, ctl: &mut HookCtl) {
        if let Some(lock) = self.st.net.take_pointer_lock() {
            self.pointer_held = lock;
            ctl.request_pointer_lock(lock);
        }
    }

    /// Moves what the services queued into the requests the app carries out.
    fn flush_requests(&mut self, ctl: &mut HookCtl) {
        for c in self.st.cutout.take() {
            ctl.cutout(c);
        }
        for (id, data) in self.st.hud.take() {
            ctl.ui_texture(id, data);
        }
        let (sounds, mute) = self.st.audio.take();
        for s in sounds {
            ctl.play_sound(s);
        }
        if let Some(m) = mute {
            ctl.mute_world(m);
        }
        for n in self.st.hotkeys.take_notices() {
            tracing::info!("{n}");
        }
    }

    /// The game ended or its mods did: what they had raised comes down.
    fn lower(&mut self, ctl: &mut HookCtl) {
        if std::mem::take(&mut self.suppressing) {
            ctl.suppress_original_ui(false);
        }
        if std::mem::take(&mut self.camera_held) {
            ctl.set_camera(None);
        }
        if std::mem::take(&mut self.pointer_held) {
            ctl.request_pointer_lock(false);
        }
        self.systems_pending = false;
        self.st.end_game();
    }

    /// Advances the pack downloads and acts on the ones that settled badly (T33).
    fn poll_packs(&mut self) {
        let Some(events) = self.st.packs.as_mut().map(PackSet::poll) else {
            return;
        };
        for ev in events {
            self.on_pack(ev);
        }
    }

    /// A pack settled. `Ready` needs nothing (the mod polls `assets().state()`). A pack that is `Missing` or `Failed`
    /// switches an optional mod off with a line in the log; for a required mod it is the reason the game cannot be
    /// played on this client, and the app shows it. Mods that do not hold `assets` do not use a pack: theirs is
    /// not their business.
    fn on_pack(&mut self, ev: PackEvent) {
        let Some(d) = self.net.dispatcher() else {
            return;
        };
        let uses = d
            .mods()
            .any(|m| m.mod_idx == ev.mod_idx && m.caps.has(Cap::Assets));
        let why = match &ev.state {
            PackState::Missing => "the server has no asset pack for it".to_owned(),
            PackState::Failed(why) => format!("its asset pack was refused ({why})"),
            _ => return,
        };
        if !uses {
            tracing::debug!(mod_id = %ev.id, "{why}; the mod holds no `assets` capability, so it does not matter");
            return;
        }
        if ev.required {
            let text = format!(
                "The mod `{}` is needed for this game, but {why}. Ask the host to run the pack builder for it.",
                ev.id
            );
            tracing::error!("{text}");
            self.problem.get_or_insert(text);
        } else {
            tracing::warn!(mod_id = %ev.id, "mod switched off: {why}");
        }
        {
            let mut host = HostView::new(&mut self.st, Env::default());
            d.detach_one(&mut host, ev.mod_idx);
        }
        self.st.release_mod(ev.mod_idx);
    }
}

impl ModHook for ClientModsHook {
    fn bind(&mut self, env: &HookEnv) {
        let base = mods_base_from_ws_url(&env.server_url);
        if base.is_none() {
            tracing::warn!(url = %env.server_url, "no HTTP address for mod packs; they cannot be fetched");
        }
        self.st.packs = Some(PackSet::new(std::rc::Rc::clone(&env.http), base));
        self.st.kv = Some(KvSet::new(std::rc::Rc::clone(&env.kv)));
    }

    fn configure_net(&self, cfg: &mut NetConfig) {
        self.net.configure(cfg);
    }

    fn on_net_event(&mut self, ev: &ClientEvent, ctl: &mut HookCtl) {
        // The packs are asked for and the namespaces opened before the mods attach, so a mod sees `Pending` and its
        // own keys from its first call.
        if let ClientEvent::ModSet(view) = ev {
            if let Some(packs) = self.st.packs.as_mut() {
                packs.start(view);
            }
            if let Some(kv) = self.st.kv.as_mut() {
                kv.start(view);
            }
            self.systems_pending = true;
        }
        {
            let mut host = HostView::new(&mut self.st, Env::default());
            self.net.on_event(ev, &mut host);
        }
        self.carry_out_pointer(ctl);
        if !self.net.active() {
            self.lower(ctl);
        }
        self.flush_requests(ctl);
    }

    fn flush_net(&mut self, core: &mut ClientCore) {
        self.st.net.flush(core);
    }

    fn take_problem(&mut self) -> Option<String> {
        self.problem.take()
    }

    fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool {
        let Some(d) = self.net.dispatcher() else {
            return false;
        };
        let taken = {
            let mut host = HostView::new(&mut self.st, Env::default());
            d.claim_input(&mut host, event) == Claim::Consume
        };
        self.carry_out_pointer(ctl);
        self.flush_requests(ctl);
        taken
    }

    fn unclaimed_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool {
        let InputEvent::Key(k) = event else {
            return false;
        };
        let (Some(code), true, false) = (k.code.as_deref(), k.pressed, k.modifiers.meta) else {
            return false;
        };
        let Some((idx, id)) = self
            .st
            .hotkeys
            .matches(code, k.modifiers.ctrl, k.modifiers.shift, k.modifiers.alt)
            .map(|(i, id)| (i, id.to_owned()))
        else {
            return false;
        };
        // A held key repeats: swallowed, fired once.
        if !k.repeat
            && let Some(d) = self.net.dispatcher()
        {
            let mut host = HostView::new(&mut self.st, Env::default());
            d.on_hotkey(&mut host, idx, &id);
        }
        self.carry_out_pointer(ctl);
        self.flush_requests(ctl);
        true
    }

    fn frame(&mut self, info: &FrameInfo<'_>, ctl: &mut HookCtl) {
        self.poll_packs();
        if let Some(grid) = info.cutout_grid {
            self.st.cutout.grid = Some(grid);
        }
        let Some(d) = self.net.dispatcher() else {
            return;
        };
        let out = {
            let _s = mod_client::perf::scope("mod.frame");
            let mut host = HostView::new(&mut self.st, env_of(info));
            d.frame(&mut host, info.dt)
        };
        // The render systems wait for the renderer (the pipelines are built from its device).
        if self.systems_pending && info.gpu.is_some() {
            self.systems_pending = false;
            let systems = {
                let mut host = HostView::new(&mut self.st, env_of(info));
                d.render_systems(&mut host)
            };
            for (_idx, system) in systems {
                let Some(next) = self.st.next_system.checked_add(1) else {
                    tracing::warn!("too many render systems of mods; one is left out");
                    continue;
                };
                let number = std::mem::replace(&mut self.st.next_system, next);
                ctl.add_render_system(Box::new(ModSystem::new(
                    system,
                    number,
                    std::rc::Rc::clone(&self.st.alive),
                )));
            }
        }
        match out.camera {
            Some(c) => {
                ctl.set_camera(Some(c));
                self.camera_held = true;
            }
            None if self.camera_held => {
                ctl.set_camera(None);
                self.camera_held = false;
            }
            None => {}
        }
        if out.suppress_ui != self.suppressing {
            ctl.suppress_original_ui(out.suppress_ui);
            self.suppressing = out.suppress_ui;
        }
        self.carry_out_pointer(ctl);
        self.flush_requests(ctl);
    }

    fn hud(&mut self, info: &FrameInfo<'_>, list: &mut UiDrawList, ctl: &mut HookCtl) {
        let Some(d) = self.net.dispatcher() else {
            return;
        };
        {
            let _s = mod_client::perf::scope("mod.hud");
            let mut host = HostView::new(&mut self.st, env_of(info));
            d.hud(&mut host, list);
        }
        self.flush_requests(ctl);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mod_hooks::HookCtl;
    use crate::mod_hooks::ModHooks;
    use mod_api::{Cap, Caps, Version};
    use mod_client::{AttachInfo, ClientCtx, ClientMod, FrameOut, ModBuild};
    use platform::{Key, KeyEvent, Modifiers, NamedKey};
    use protocol::{ModS2C, ModSetView, ModSync, ModView};

    /// Echoes every message back on the same channel, raises the UI and the camera on `frame` while `on`, takes
    /// the key `KeyQ`.
    struct Probe {
        on: bool,
    }

    impl ClientMod for Probe {
        fn attach(&mut self, cx: &mut dyn ClientCtx, _: &AttachInfo) {
            cx.send(1, b"here").unwrap();
        }

        fn on_msg(&mut self, cx: &mut dyn ClientCtx, ch: u8, _from: Option<i8>, payload: &[u8]) {
            if payload == b"go" {
                self.on = true;
            }
            cx.send(ch, payload).unwrap();
        }

        fn claim_input(&mut self, cx: &mut dyn ClientCtx, ev: &InputEvent) -> mod_client::Claim {
            match ev {
                InputEvent::Key(k) if k.code.as_deref() == Some("KeyQ") => {
                    cx.request_pointer_lock(true).unwrap();
                    mod_client::Claim::Consume
                }
                _ => mod_client::Claim::Pass,
            }
        }

        fn frame(&mut self, _cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
            if !self.on {
                return FrameOut::default();
            }
            FrameOut {
                camera: Some(Box::new({
                    let mut c = render::FirstPerson::new(glam::Vec3::new(4.0, 6.5, 8.0));
                    c.yaw = 0.5;
                    c.pitch = -0.2;
                    c.near = 0.4;
                    c.far = 900.0;
                    c.fov_y = 60f32.to_radians();
                    c
                })),
                suppress_ui: true,
            }
        }
    }

    fn hooks() -> ModHooks {
        let mut r = ClientRegistry::new();
        r.add_built(
            "probe",
            ModBuild {
                version: Version::new(1, 0, 0),
                caps: Caps::of(&[Cap::NetChannel, Cap::Camera, Cap::InputClaim]),
                code_hash: [3; 16],
            },
            || Box::new(Probe { on: false }),
        );
        let mut h = ModHooks::new();
        h.register(Box::new(ClientModsHook::new(r).expect("a mod is built in")));
        h
    }

    fn set() -> ClientEvent {
        ClientEvent::ModSet(ModSetView {
            modset_hash: 1,
            mods: vec![ModView {
                id: "probe".into(),
                version: "1.0.0".into(),
                load_idx: 3,
                sync: ModSync::Cosmetic,
                client_required: false,
                available: true,
                pack: None,
            }],
        })
    }

    fn info() -> FrameInfo<'static> {
        FrameInfo {
            dt: 0.016,
            ..FrameInfo::default()
        }
    }

    fn key(code: &str) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            code: Some(code.into()),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn an_empty_registry_makes_no_hook() {
        assert!(ClientModsHook::new(ClientRegistry::new()).is_none());
    }

    #[test]
    fn the_built_in_mods_go_into_the_handshake() {
        let h = hooks();
        let mut cfg = NetConfig::new("b", "n");
        assert!(cfg.built_in.is_empty());
        h.configure_net(&mut cfg);
        assert_eq!(cfg.built_in.len(), 1);
        assert_eq!(cfg.built_in[0].id, "probe");
        assert_eq!(cfg.built_in[0].code_hash, [3; 16]);
    }

    #[test]
    fn before_a_game_the_hook_takes_and_asks_for_nothing() {
        let mut h = hooks();
        assert!(!h.claim_input(&key("KeyQ")).taken);
        assert_eq!(h.frame(&info()), None);
        assert!(h.camera().is_none() && !h.ui_suppressed());
    }

    #[test]
    fn the_set_attaches_a_message_comes_back_and_the_screen_follows_the_mod() {
        let mut h = hooks();
        h.net_event(&set());
        // Input: the mod takes KeyQ and asks for the pointer; other keys pass.
        let c = h.claim_input(&key("KeyQ"));
        assert!(c.taken);
        assert_eq!(c.pointer_lock, Some(true));
        assert!(!h.claim_input(&key("KeyW")).taken);
        // A message to the mod at index 3 (and not to 0) wakes it; its camera and UI request follow in `frame`.
        h.net_event(&ClientEvent::ModMsg(ModS2C {
            mod_idx: 0,
            ch: 0,
            from: None,
            payload: b"go".to_vec(),
        }));
        assert!(h.camera().is_none());
        h.net_event(&ClientEvent::ModMsg(ModS2C {
            mod_idx: 3,
            ch: 0,
            from: Some(1),
            payload: b"go".to_vec(),
        }));
        h.frame(&info());
        let cam = h.camera().expect("the mod holds the camera");
        assert_eq!((cam.eye().x, cam.eye().y, cam.eye().z), (4.0, 6.5, 8.0));
        assert_eq!(cam.far(), 900.0);
        assert!(h.ui_suppressed());
    }

    #[test]
    fn the_end_of_the_game_gives_the_screen_back() {
        let mut h = hooks();
        h.net_event(&set());
        h.net_event(&ClientEvent::ModMsg(ModS2C {
            mod_idx: 3,
            ch: 0,
            from: None,
            payload: b"go".to_vec(),
        }));
        h.frame(&info());
        assert!(h.camera().is_some() && h.ui_suppressed());
        h.claim_input(&key("KeyQ"));
        let lock = h.net_event(&ClientEvent::Detached(protocol::DetachReason::Left));
        assert!(h.camera().is_none() && !h.ui_suppressed());
        assert_eq!(lock, Some(false), "the pointer is let go");
        // Nothing is left to ask for.
        assert_eq!(h.frame(&info()), None);
    }

    // ---- asset packs (T33) ----

    use crate::mod_assets::tests::{FakeHttp, pack_bytes};
    use mod_assets::Sha256;
    use std::cell::RefCell;
    use std::rc::Rc;

    type Seen = Rc<RefCell<Vec<(String, Option<Vec<u8>>)>>>;

    /// Records the pack state it sees in `attach` and in each `frame`, and the bytes of one file when ready.
    struct Reader {
        seen: Seen,
        detached: Rc<RefCell<bool>>,
    }

    impl ClientMod for Reader {
        fn attach(&mut self, cx: &mut dyn ClientCtx, _: &AttachInfo) {
            let st = cx.assets().unwrap().state();
            self.seen.borrow_mut().push((format!("{st:?}"), None));
        }

        fn detach(&mut self, _cx: &mut dyn ClientCtx) {
            *self.detached.borrow_mut() = true;
        }

        fn frame(&mut self, cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
            let a = cx.assets().unwrap();
            let bytes = a.get("data/blocks").map(<[u8]>::to_vec);
            self.seen
                .borrow_mut()
                .push((format!("{:?}", a.state()), bytes));
            FrameOut::default()
        }
    }

    fn pack_hook(serve: Option<Vec<u8>>) -> (ClientModsHook, Seen, Rc<RefCell<bool>>, Sha256) {
        let seen: Seen = Rc::default();
        let detached = Rc::new(RefCell::new(false));
        let (s2, d2) = (Rc::clone(&seen), Rc::clone(&detached));
        let mut r = ClientRegistry::new();
        r.add_built(
            "reader",
            ModBuild {
                version: Version::new(1, 0, 0),
                caps: Caps::of(&[Cap::Assets]),
                code_hash: [1; 16],
            },
            move || {
                Box::new(Reader {
                    seen: Rc::clone(&s2),
                    detached: Rc::clone(&d2),
                })
            },
        );
        let http = Rc::new(FakeHttp::default());
        let base = "http://h/mods/";
        let hash = match serve {
            Some(bytes) => {
                let want = Sha256::of(&pack_bytes("reader"));
                // `serve` files the blob under the hash of what it is; a tampered blob is filed under the genuine hash.
                if bytes == pack_bytes("reader") {
                    http.serve(base, &bytes)
                } else {
                    http.routes.borrow_mut().insert(
                        format!("{base}{want}"),
                        Ok(platform::HttpResponse {
                            status: 200,
                            body: bytes,
                        }),
                    );
                    want
                }
            }
            None => Sha256::of(b"not on the server"),
        };
        let mut hook = ClientModsHook::new(r).unwrap();
        hook.bind(&HookEnv {
            http,
            server_url: "ws://h/ws".into(),
            kv: Rc::new(platform::MemoryStore::new()),
        });
        (hook, seen, detached, hash)
    }

    fn pack_set(hash: &Sha256, required: bool) -> ClientEvent {
        ClientEvent::ModSet(ModSetView {
            modset_hash: 9,
            mods: vec![ModView {
                id: "reader".into(),
                version: "1.0.0".into(),
                load_idx: 2,
                sync: ModSync::Cosmetic,
                client_required: required,
                available: true,
                pack: Some(hash.to_string()),
            }],
        })
    }

    fn tick(h: &mut ClientModsHook) {
        h.frame(&info(), &mut HookCtl::default());
    }

    #[test]
    fn a_mod_sees_its_pack_pending_then_ready() {
        let (mut h, seen, detached, hash) = pack_hook(Some(pack_bytes("reader")));
        h.on_net_event(&pack_set(&hash, false), &mut HookCtl::default());
        tick(&mut h);
        tick(&mut h);
        let seen = seen.borrow();
        assert_eq!(
            seen[0].0, "Pending",
            "the mod attaches before the pack is here"
        );
        assert_eq!(seen[1], ("Ready".to_string(), Some(b"dirt".to_vec())));
        assert!(!*detached.borrow());
    }

    #[test]
    fn a_missing_pack_switches_an_optional_mod_off_and_does_not_stop_the_game() {
        let (mut h, seen, detached, hash) = pack_hook(None);
        h.on_net_event(&pack_set(&hash, false), &mut HookCtl::default());
        tick(&mut h);
        assert!(*detached.borrow(), "detach was called");
        assert_eq!(h.take_problem(), None);
        let calls = seen.borrow().len();
        tick(&mut h);
        assert_eq!(
            seen.borrow().len(),
            calls,
            "a switched-off mod gets no more frames"
        );
    }

    #[test]
    fn a_missing_pack_of_a_required_mod_is_a_readable_error() {
        let (mut h, _seen, detached, hash) = pack_hook(None);
        h.on_net_event(&pack_set(&hash, true), &mut HookCtl::default());
        tick(&mut h);
        let text = h.take_problem().expect("an error for the player");
        assert!(
            text.contains("`reader`") && text.contains("asset pack"),
            "{text}"
        );
        assert!(*detached.borrow());
        assert_eq!(h.take_problem(), None, "told once");
    }

    #[test]
    fn a_tampered_pack_is_refused() {
        let mut bytes = pack_bytes("reader");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let (mut h, seen, detached, hash) = pack_hook(Some(bytes));
        h.on_net_event(&pack_set(&hash, true), &mut HookCtl::default());
        tick(&mut h);
        let text = h.take_problem().expect("refused");
        assert!(text.contains("refused"), "{text}");
        assert!(*detached.borrow());
        assert!(
            seen.borrow().iter().all(|(_, bytes)| bytes.is_none()),
            "no byte of it reached the mod"
        );
    }

    // ---- the services of the host (T36) ----

    use crate::mod_host::ReplicaWorld;
    use draw_list::UiDrawList;
    use mod_client::{CutoutRegion, HotkeySpec, PlayOpts};
    use platform::KeyValueStore as _;
    use render::world::terrain::{TerrainFrame, TerrainSampler};

    /// A mod that uses every service the host has: a counter in `kv` that survives a restart, a hotkey that sends,
    /// plays a sound, cuts a column and counts; a HUD with a texture; the world and the terrain read in `frame`.
    struct Services {
        count: u32,
        tex: Option<draw_list::TextureId>,
        log: Rc<RefCell<Vec<String>>>,
    }

    impl ClientMod for Services {
        fn attach(&mut self, cx: &mut dyn ClientCtx, _: &AttachInfo) {
            self.count = cx
                .kv()
                .unwrap()
                .get("count")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            // A bare key of the game is refused, a free key works.
            assert!(
                cx.hotkeys()
                    .unwrap()
                    .register(HotkeySpec::new("bad", "KeyW", "w"))
                    .is_err()
            );
            cx.hotkeys()
                .unwrap()
                .register(HotkeySpec::new("go", "F13", "Go"))
                .unwrap();
            // No world, terrain or gpu outside a frame.
            assert!(cx.world().is_err() && cx.terrain().is_err() && cx.gpu().is_err());
        }

        fn on_hotkey(&mut self, cx: &mut dyn ClientCtx, id: &str) {
            assert_eq!(id, "go");
            self.count += 1;
            cx.kv().unwrap().set("count", &self.count.to_string());
            cx.send(0, &self.count.to_le_bytes()).unwrap();
            cx.audio()
                .unwrap()
                .play("sounds/dig", PlayOpts::default())
                .unwrap();
            assert!(
                cx.audio()
                    .unwrap()
                    .play("sounds/none", PlayOpts::default())
                    .is_err()
            );
            let g = cx
                .cutout()
                .unwrap()
                .grid()
                .expect("the last frame had a map");
            cx.cutout()
                .unwrap()
                .set_cutout_mask(
                    CutoutRegion {
                        x0: 3,
                        z0: g.bz0 + 2,
                        width: 2,
                        height: 1,
                    },
                    &[1, 1],
                )
                .unwrap();
        }

        fn frame(&mut self, cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
            let w = cx.world().unwrap();
            let t = cx.terrain().unwrap();
            self.log.borrow_mut().push(format!(
                "turn={} me={:?} h={:?} near={:?} built={}",
                w.turn(),
                w.me(),
                t.height(10.0, -10.0).map(|h| h as i32),
                t.height_nearest(10.0, -10.0).map(|h| h as i32),
                t.is_built(0)
            ));
            if self.tex.is_none() {
                self.tex = Some(
                    cx.hud_textures()
                        .unwrap()
                        .upload_rgba8(1, 1, &[255; 4])
                        .unwrap(),
                );
            }
            FrameOut::default()
        }

        fn hud(&mut self, _cx: &mut dyn ClientCtx, list: &mut UiDrawList) {
            if let Some(t) = self.tex {
                list.add_rect(
                    t,
                    glam::Vec2::ZERO,
                    glam::Vec2::splat(8.0),
                    glam::Vec2::ZERO,
                    glam::Vec2::ONE,
                    [255; 4],
                );
            }
        }
    }

    fn all_caps() -> Caps {
        Caps::of(&[
            Cap::NetChannel,
            Cap::WorldRead,
            Cap::InputClaim,
            Cap::Hud,
            Cap::Kv,
            Cap::Audio,
            Cap::Assets,
            Cap::TerrainCutout,
        ])
    }

    fn services_hooks(
        store: &Rc<platform::MemoryStore>,
        http: &Rc<FakeHttp>,
        log: &Rc<RefCell<Vec<String>>>,
    ) -> ModHooks {
        let log = Rc::clone(log);
        let mut r = ClientRegistry::new();
        r.add_built(
            "services",
            ModBuild {
                version: Version::new(1, 0, 0),
                caps: all_caps(),
                code_hash: [2; 16],
            },
            move || {
                Box::new(Services {
                    count: 0,
                    tex: None,
                    log: Rc::clone(&log),
                })
            },
        );
        let mut hook = ClientModsHook::new(r).unwrap();
        let kv: Rc<dyn platform::KeyValueStore> = store.clone();
        hook.bind(&HookEnv {
            http: http.clone(),
            server_url: "ws://h/ws".into(),
            kv,
        });
        let mut hooks = ModHooks::new();
        hooks.register(Box::new(hook));
        hooks
    }

    fn services_set(hash: &Sha256) -> ClientEvent {
        let mut ev = pack_set(hash, false);
        if let ClientEvent::ModSet(v) = &mut ev {
            v.mods[0].id = "services".into();
        }
        ev
    }

    fn f13(pressed: bool, repeat: bool, shift: bool) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::Escape),
            code: Some("F13".into()),
            pressed,
            repeat,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
        })
    }

    #[test]
    fn a_mod_uses_every_service_and_its_state_survives_a_restart() {
        let store = Rc::new(platform::MemoryStore::new());
        let http = Rc::new(FakeHttp::default());
        let hash = http.serve("http://h/mods/", &pack_bytes("services"));
        let log: Rc<RefCell<Vec<String>>> = Rc::default();
        let mut hooks = services_hooks(&store, &http, &log);
        hooks.net_event(&services_set(&hash));

        let world = crate::test_world::world();
        let replica = ReplicaWorld::new(&world, 5);
        let sampler = TerrainSampler::new(TerrainFrame::new(16, 10, true));
        let grid = render::cutout::CutoutGrid::for_map(16, 10, true);
        let info = FrameInfo {
            dt: 0.016,
            world: Some(&replica),
            terrain: Some(&sampler),
            cutout_grid: Some(grid),
            screen: (1280, 720, 1.0),
            ..FrameInfo::default()
        };
        // The pack arrives during the first frames; the frame reads the world and the ground.
        hooks.frame(&info);
        hooks.frame(&info);
        let l = log.borrow().clone();
        assert!(
            l[0].contains(&format!("turn={}", world.turn)) && l[0].contains("built=true"),
            "{l:?}"
        );

        // The hotkey is taken once the UI has not used the key; claim_input (before the UI) leaves it alone.
        assert!(!hooks.claim_input(&f13(true, false, false)).taken);
        assert!(hooks.unclaimed_input(&f13(true, false, false)).taken);
        // A repeat of the held key is swallowed, not fired; a release and a chord with Shift are not the hotkey.
        assert!(hooks.unclaimed_input(&f13(true, true, false)).taken);
        assert!(!hooks.unclaimed_input(&f13(false, false, false)).taken);
        assert!(!hooks.unclaimed_input(&f13(true, false, true)).taken);

        let req = hooks.take_requests();
        assert_eq!(req.sounds.len(), 1, "one press, one sound");
        assert_eq!(req.sounds[0].sound, "sounds/dig");
        assert_eq!(req.cutouts.len(), 1);
        assert_eq!(req.textures.len(), 1);
        assert_eq!(store.get("civ5.mod.services.count").as_deref(), Some("1"));
        assert_eq!(store.get("count"), None, "keys are namespaced");

        let mut list = UiDrawList::new();
        hooks.hud(&info, &mut list);
        assert_eq!(list.commands().len(), 1);
        assert_eq!(list.commands()[0].texture, req.textures[0].0);

        // The game ends: the cutout is taken back.
        hooks.net_event(&ClientEvent::Detached(protocol::DetachReason::Left));
        let end = hooks.take_requests();
        assert_eq!(end.cutouts, [crate::mod_hooks::CutoutCmd::Clear]);

        // A new run of the client with the same store: the mod starts from the saved count.
        let mut hooks2 = services_hooks(&store, &http, &log);
        hooks2.net_event(&services_set(&hash));
        hooks2.frame(&info);
        hooks2.frame(&info);
        assert!(hooks2.unclaimed_input(&f13(true, false, false)).taken);
        assert_eq!(store.get("civ5.mod.services.count").as_deref(), Some("2"));
    }

    #[test]
    fn without_a_game_the_hotkey_is_nobodys() {
        let store = Rc::new(platform::MemoryStore::new());
        let http = Rc::new(FakeHttp::default());
        let log = Rc::default();
        let mut hooks = services_hooks(&store, &http, &log);
        assert!(!hooks.unclaimed_input(&f13(true, false, false)).taken);
    }

    // ---- render systems (T36, R1) ----

    use render::frame::{
        DrawContext, InitContext, MainSlot, ModGpu, PassSlot, PrepareContext, RenderSystem,
        SystemId,
    };
    use render::wgpu;

    /// Every mod's system says it is `SystemId::Mod(0)`; the host numbers them.
    struct Stub(Rc<RefCell<Vec<&'static str>>>);

    impl RenderSystem for Stub {
        fn id(&self) -> SystemId {
            SystemId::Mod(0)
        }
        fn passes(&self) -> &'static [PassSlot] {
            &[PassSlot::Main(MainSlot::Mod)]
        }
        fn init(&mut self, _: &mut InitContext<'_>) {}
        fn prepare(&mut self, _: &mut PrepareContext<'_>) {
            self.0.borrow_mut().push("prepare");
        }
        fn record(&self, _: PassSlot, _: &mut DrawContext<'_, '_>) {}
    }

    struct Drawer(Rc<RefCell<Vec<&'static str>>>);

    impl ClientMod for Drawer {
        fn render_systems(
            &mut self,
            cx: &mut dyn ClientCtx,
        ) -> Vec<Box<dyn mod_client::ModRenderSystem>> {
            // The device is what the mod builds its pipelines from; without it there is no system.
            if cx.gpu().is_err() {
                return Vec::new();
            }
            vec![Box::new(Stub(Rc::clone(&self.0)))]
        }
    }

    fn test_device() -> Option<ModGpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster_free_block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        )
        .ok()?;
        let (device, queue) =
            pollster_free_block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .ok()?;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[],
        });
        Some(ModGpu {
            device,
            queue,
            view_layout: layout.clone(),
            frame_layout: layout,
            color_format: wgpu::TextureFormat::Rgba8Unorm,
            depth_format: wgpu::TextureFormat::Depth32Float,
            samples: 1,
        })
    }

    fn pollster_free_block_on<F: std::future::Future>(fut: F) -> F::Output {
        use std::task::{Context, Poll, Wake, Waker};
        struct Park(std::thread::Thread);
        impl Wake for Park {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let mut fut = std::pin::pin!(fut);
        let waker = Waker::from(std::sync::Arc::new(Park(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::park();
        }
    }

    #[test]
    fn render_systems_wait_for_the_renderer_and_get_numbers_of_their_own() {
        let calls: Rc<RefCell<Vec<&'static str>>> = Rc::default();
        let c2 = Rc::clone(&calls);
        let mut r = ClientRegistry::new();
        for id in ["drawer", "painter"] {
            let c = Rc::clone(&c2);
            r.add_built(
                id,
                ModBuild {
                    version: Version::new(1, 0, 0),
                    caps: Caps::of(&[Cap::RenderPipeline]),
                    code_hash: [4; 16],
                },
                move || Box::new(Drawer(Rc::clone(&c))),
            );
        }
        let mut hooks = ModHooks::new();
        hooks.register(Box::new(ClientModsHook::new(r).unwrap()));
        let set = ClientEvent::ModSet(ModSetView {
            modset_hash: 1,
            mods: ["drawer", "painter"]
                .iter()
                .enumerate()
                .map(|(i, id)| ModView {
                    id: (*id).into(),
                    version: "1.0.0".into(),
                    load_idx: i as u16,
                    sync: ModSync::Cosmetic,
                    client_required: false,
                    available: true,
                    pack: None,
                })
                .collect(),
        });
        hooks.net_event(&set);
        // No renderer yet: no systems are asked for.
        hooks.frame(&info());
        assert!(hooks.take_requests().systems.is_empty());
        let Some(gpu) = test_device() else {
            eprintln!("SKIP render systems: no adapter");
            return;
        };
        let with_gpu = FrameInfo {
            gpu: Some(&gpu),
            ..info()
        };
        hooks.frame(&with_gpu);
        let req = hooks.take_requests();
        let ids: Vec<SystemId> = req.systems.iter().map(|s| s.id()).collect();
        assert_eq!(
            ids,
            [SystemId::Mod(0), SystemId::Mod(1)],
            "numbered by the host"
        );
        // Asked once per game, not every frame.
        hooks.frame(&with_gpu);
        assert!(hooks.take_requests().systems.is_empty());
        // After the game the systems draw nothing any more: their prepare is not called.
        let mut systems = req.systems;
        let services = render::frame::WorldServices::default();
        let mut frame = None;
        systems[0].prepare(&mut PrepareContext::new(
            &gpu.device,
            &gpu.queue,
            &services,
            &mut frame,
        ));
        assert_eq!(calls.borrow().len(), 1);
        hooks.net_event(&ClientEvent::Detached(protocol::DetachReason::Left));
        systems[0].prepare(&mut PrepareContext::new(
            &gpu.device,
            &gpu.queue,
            &services,
            &mut frame,
        ));
        assert_eq!(
            calls.borrow().len(),
            1,
            "a finished game's system is dormant"
        );
    }
}
