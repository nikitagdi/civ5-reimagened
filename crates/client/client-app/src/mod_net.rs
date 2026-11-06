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

use client_core::{ClientCore, ClientEvent, ModNet, NetConfig, NetHost};
use mod_api::Cap;
use mod_client::PackState;
use mod_client::{CameraOverride, Claim, ClientRegistry};
use platform::InputEvent;

use crate::mod_assets::{PackEvent, PackSet, mods_base_from_ws_url};
use crate::mod_hooks::{FrameInfo, HookCtl, HookEnv, ModHook};
use crate::mod_host::{HostState, HostView};

/// The hook of the client mods built into this binary.
pub struct ClientModsHook {
    net: ModNet,
    st: HostState,
    /// The hook has raised `suppress_original_ui` / holds the camera / holds the pointer, so it lowers them again
    /// when no mod asks for them any more (or the game ends).
    suppressing: bool,
    camera_held: bool,
    pointer_held: bool,
    /// A required mod cannot run on this client (its pack is missing or refused): the app ends the game with it.
    problem: Option<String>,
}

impl ClientModsHook {
    /// A hook over the registry of the built-in mods. `None` for an empty registry: nothing to hook.
    pub fn new(registry: ClientRegistry) -> Option<Self> {
        if registry.is_empty() {
            return None;
        }
        Some(Self {
            net: ModNet::new(registry),
            st: HostState {
                net: NetHost::new(),
                packs: None,
            },
            suppressing: false,
            camera_held: false,
            pointer_held: false,
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
        let mut host = HostView::new(&mut self.st);
        d.detach_one(&mut host, ev.mod_idx);
    }
}

/// The stand-in `CameraOverride` of a mod as the first-person camera of the render crate. Its terrain focus is the
/// point under the eye (`CameraOverride::focus_xz` is not carried yet).
fn first_person(c: &CameraOverride) -> render::FirstPerson {
    let mut cam = render::FirstPerson::new(glam::Vec3::from(c.eye));
    cam.yaw = c.yaw;
    cam.pitch = c.pitch;
    cam.near = c.near;
    cam.far = c.far;
    cam.fov_y = c.fov_y_deg.to_radians();
    cam
}

impl ModHook for ClientModsHook {
    fn bind(&mut self, env: &HookEnv) {
        let base = mods_base_from_ws_url(&env.server_url);
        if base.is_none() {
            tracing::warn!(url = %env.server_url, "no HTTP address for mod packs; they cannot be fetched");
        }
        self.st.packs = Some(PackSet::new(std::rc::Rc::clone(&env.http), base));
    }

    fn configure_net(&self, cfg: &mut NetConfig) {
        self.net.configure(cfg);
    }

    fn on_net_event(&mut self, ev: &ClientEvent, ctl: &mut HookCtl) {
        // The packs are asked for before the mods attach, so a mod sees `Pending` from its first call.
        if let (ClientEvent::ModSet(view), Some(packs)) = (ev, self.st.packs.as_mut()) {
            packs.start(view);
        }
        {
            let mut host = HostView::new(&mut self.st);
            self.net.on_event(ev, &mut host);
        }
        self.carry_out_pointer(ctl);
        if !self.net.active() {
            self.lower(ctl);
            if let Some(p) = self.st.packs.as_mut() {
                p.clear();
            }
        }
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
            let mut host = HostView::new(&mut self.st);
            d.claim_input(&mut host, event) == Claim::Consume
        };
        self.carry_out_pointer(ctl);
        taken
    }

    fn frame(&mut self, info: &FrameInfo<'_>, ctl: &mut HookCtl) {
        self.poll_packs();
        let Some(d) = self.net.dispatcher() else {
            return;
        };
        let out = {
            let mut host = HostView::new(&mut self.st);
            d.frame(&mut host, info.dt)
        };
        match out.camera {
            Some(c) => {
                ctl.set_camera(Some(Box::new(first_person(&c))));
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
                camera: Some(CameraOverride {
                    eye: [4.0, 6.5, 8.0],
                    yaw: 0.5,
                    pitch: -0.2,
                    near: 0.4,
                    far: 900.0,
                    fov_y_deg: 60.0,
                    focus_xz: [4.0, 8.0],
                }),
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
            now_ms: 0,
            focus: glam::Vec2::ZERO,
            terrain: None,
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
}
