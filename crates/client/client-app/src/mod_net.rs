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
use mod_client::{CameraOverride, Claim, ClientRegistry};
use platform::InputEvent;

use crate::mod_hooks::{FrameInfo, HookCtl, ModHook};

/// The hook of the client mods built into this binary.
pub struct ClientModsHook {
    net: ModNet,
    host: NetHost,
    /// The hook has raised `suppress_original_ui` / holds the camera / holds the pointer, so it lowers them again
    /// when no mod asks for them any more (or the game ends).
    suppressing: bool,
    camera_held: bool,
    pointer_held: bool,
}

impl ClientModsHook {
    /// A hook over the registry of the built-in mods. `None` for an empty registry: nothing to hook.
    pub fn new(registry: ClientRegistry) -> Option<Self> {
        if registry.is_empty() {
            return None;
        }
        Some(Self {
            net: ModNet::new(registry),
            host: NetHost::new(),
            suppressing: false,
            camera_held: false,
            pointer_held: false,
        })
    }

    /// The mod layer, for tests and for the host that extends this hook.
    pub fn net(&mut self) -> &mut ModNet {
        &mut self.net
    }

    fn carry_out_pointer(&mut self, ctl: &mut HookCtl) {
        if let Some(lock) = self.host.take_pointer_lock() {
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
    fn configure_net(&self, cfg: &mut NetConfig) {
        self.net.configure(cfg);
    }

    fn on_net_event(&mut self, ev: &ClientEvent, ctl: &mut HookCtl) {
        self.net.on_event(ev, &mut self.host);
        self.carry_out_pointer(ctl);
        if !self.net.active() {
            self.lower(ctl);
        }
    }

    fn flush_net(&mut self, core: &mut ClientCore) {
        self.host.flush(core);
    }

    fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool {
        let Some(d) = self.net.dispatcher() else {
            return false;
        };
        let taken = d.claim_input(&mut self.host, event) == Claim::Consume;
        self.carry_out_pointer(ctl);
        taken
    }

    fn frame(&mut self, info: &FrameInfo<'_>, ctl: &mut HookCtl) {
        let Some(d) = self.net.dispatcher() else {
            return;
        };
        let out = d.frame(&mut self.host, info.dt);
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
}
