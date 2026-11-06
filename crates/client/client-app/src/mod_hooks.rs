//! Hooks of mods into the app (client-hooks I1, I2, C3): the registration point of the client side of mods.
//!
//! The app asks the hooks first about every input event. A hook may take the event, ask for the pointer lock and
//! raise or lower the suppression of the original UI. Only values cross this border (D2): the hooks never see
//! the app. With no hook registered nothing here does anything: `claim_input` returns "not taken" at once, no
//! state changes, and the app behaves as it did without this module.

use platform::InputEvent;
use render::ModCamera;
use render::world::terrain::TerrainSampler;

/// What a hook asks of the app while it handles an event or a frame. Requests are values; the app carries
/// them out.
#[derive(Default)]
pub struct HookCtl {
    pointer_lock: Option<bool>,
    suppress_ui: Option<bool>,
    /// `Some(None)`: the hook gives the camera back; `None`: it did not ask.
    camera: Option<Option<Box<dyn ModCamera>>>,
}

impl HookCtl {
    /// Takes the pointer (`true`) or gives it back (`false`). On the web this works only inside the handling of
    /// a click or a key press, so ask while claiming such an event. See `platform::PointerLock` for the modes.
    pub fn request_pointer_lock(&mut self, on: bool) {
        self.pointer_lock = Some(on);
    }

    /// Hides the original UI and stops its input and its frame while a mod owns the screen (C3). The mod
    /// lowers it again when it leaves.
    pub fn suppress_original_ui(&mut self, on: bool) {
        self.suppress_ui = Some(on);
    }

    /// Takes the screen's camera (C2, `Some`) or gives it back to the strategic one (`None`). The camera stays
    /// until a hook gives it back, so a hook that moves it every frame sets a new one every frame (it is a small
    /// value). While one is set the app moves the strategic camera to its `terrain_focus` every frame, so the
    /// labels, the minimap, the picking and the world sound keep looking at the place the mod is.
    pub fn set_camera(&mut self, camera: Option<Box<dyn ModCamera>>) {
        self.camera = Some(camera);
    }
}

/// What a hook sees of the frame, as values.
pub struct FrameInfo<'a> {
    /// Seconds since the last frame.
    pub dt: f32,
    /// The clock of the app, milliseconds.
    pub now_ms: u64,
    /// Where the strategic camera looks (our X and Z): a camera of a mod starts there.
    pub focus: glam::Vec2,
    /// The heights of the map, once there is one: where the ground is under an eye.
    pub terrain: Option<&'a TerrainSampler>,
}

/// The client side of a mod, as far as the input goes. More hooks (frame, camera) join it with the
/// tasks that need them.
pub trait ModHook {
    /// Called first for every input event, before the performance panel, egui, the original UI and the map.
    /// Returns true if the event is taken: it goes no further and no other hook sees it.
    ///
    /// Keys: read `KeyEvent::code` (the physical key, `KeyW`), not `key`, so the layout and Shift do not matter.
    fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool;

    /// Called once a frame, before the camera is chosen (nothing is called with no hook registered). The place
    /// to move a camera of the mod and hand it over with `HookCtl::set_camera`. A pointer lock asked here
    /// is carried out, but on the web a lock needs the gesture of an input event: ask it from `claim_input`.
    fn frame(&mut self, _info: &FrameInfo<'_>, _ctl: &mut HookCtl) {}

    /// The mods this hook carries put themselves into the config of the network core (`ModHello`, T35). Called once,
    /// before the core is made.
    fn configure_net(&self, _cfg: &mut client_core::NetConfig) {}

    /// An event of the network core: the set of the game (`ModSet`), a message of a mod's server half (`ModMsg`),
    /// `Attached` and `Detached`. Hooks that carry no mods ignore it.
    fn on_net_event(&mut self, _ev: &client_core::ClientEvent, _ctl: &mut HookCtl) {}

    /// Sends what the mods queued since the last call (`ClientMsg::Mod`). Called every frame while a game runs.
    fn flush_net(&mut self, _core: &mut client_core::ClientCore) {}
}

/// What the app does after the hooks saw an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Claim {
    /// A hook took the event.
    pub taken: bool,
    /// Take (`Some(true)`) or give back (`Some(false)`) the pointer, as the hooks asked last.
    pub pointer_lock: Option<bool>,
}

/// The registered hooks and the state they raise. Empty by default: that is the app without mods.
#[derive(Default)]
pub struct ModHooks {
    hooks: Vec<Box<dyn ModHook>>,
    ui_suppressed: bool,
    /// The camera a hook holds in place of the strategic one (C2); `None`: the strategic camera.
    camera: Option<Box<dyn ModCamera>>,
}

impl ModHooks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a hook. Hooks are asked in the order they were registered.
    pub fn register(&mut self, hook: Box<dyn ModHook>) {
        self.hooks.push(hook);
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    /// The original UI is hidden and gets neither input nor frames.
    pub fn ui_suppressed(&self) -> bool {
        self.ui_suppressed
    }

    /// The camera of a mod (C2): the app shows it in place of the strategic one while it is `Some`.
    pub fn camera(&self) -> Option<&dyn ModCamera> {
        self.camera.as_deref()
    }

    /// Takes in what a hook asked; the pointer lock is left to the caller.
    fn absorb(&mut self, ctl: HookCtl) -> Option<bool> {
        if let Some(on) = ctl.suppress_ui {
            self.ui_suppressed = on;
        }
        if let Some(camera) = ctl.camera {
            self.camera = camera;
        }
        ctl.pointer_lock
    }

    /// Asks the hooks about `event`, in order, up to the first that takes it.
    pub fn claim_input(&mut self, event: &InputEvent) -> Claim {
        let mut claim = Claim::default();
        for i in 0..self.hooks.len() {
            let mut ctl = HookCtl::default();
            let taken = self.hooks[i].claim_input(event, &mut ctl);
            if let Some(on) = self.absorb(ctl) {
                claim.pointer_lock = Some(on);
            }
            if taken {
                claim.taken = true;
                break;
            }
        }
        claim
    }

    /// Lets the mods put themselves into the config of the network core (`ModHello`).
    pub fn configure_net(&self, cfg: &mut client_core::NetConfig) {
        for h in &self.hooks {
            h.configure_net(cfg);
        }
    }

    /// Gives an event of the network core to every hook, in order. Returns the pointer lock they asked last.
    pub fn net_event(&mut self, ev: &client_core::ClientEvent) -> Option<bool> {
        let mut lock = None;
        for i in 0..self.hooks.len() {
            let mut ctl = HookCtl::default();
            self.hooks[i].on_net_event(ev, &mut ctl);
            if let Some(on) = self.absorb(ctl) {
                lock = Some(on);
            }
        }
        lock
    }

    /// Lets every hook send what its mods queued.
    pub fn flush_net(&mut self, core: &mut client_core::ClientCore) {
        for h in &mut self.hooks {
            h.flush_net(core);
        }
    }

    /// Lets every hook see the frame, in order. Returns the pointer lock they asked last, if any.
    pub fn frame(&mut self, info: &FrameInfo<'_>) -> Option<bool> {
        let mut lock = None;
        for i in 0..self.hooks.len() {
            let mut ctl = HookCtl::default();
            self.hooks[i].frame(info, &mut ctl);
            if let Some(on) = self.absorb(ctl) {
                lock = Some(on);
            }
        }
        lock
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{
        Key, KeyEvent, Modifiers, NamedKey, PhysicalPosition, PointerEvent, PointerId, PointerKind,
        PointerPhase,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    fn events() -> Vec<InputEvent> {
        vec![
            InputEvent::Pointer(PointerEvent {
                id: PointerId::MOUSE,
                kind: PointerKind::Mouse,
                phase: PointerPhase::Down,
                position: PhysicalPosition { x: 4.0, y: 5.0 },
                button: Some(platform::MouseButton::Left),
                modifiers: Modifiers::default(),
            }),
            InputEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::F3),
                code: Some("F3".into()),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            }),
            InputEvent::Text("a".into()),
            InputEvent::Focused(false),
            InputEvent::PointerDelta { dx: 1.0, dy: 2.0 },
            InputEvent::PointerLockChanged(true),
        ]
    }

    #[test]
    fn empty_hooks_take_nothing_ask_nothing_and_change_nothing() {
        let mut hooks = ModHooks::default();
        assert!(hooks.is_empty());
        for ev in events() {
            assert_eq!(hooks.claim_input(&ev), Claim::default(), "{ev:?}");
        }
        assert!(!hooks.ui_suppressed());
    }

    /// Takes the keys `KeyW` and `Escape` (by code), records what it saw.
    struct Walker {
        seen: Rc<RefCell<Vec<String>>>,
    }

    impl ModHook for Walker {
        fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool {
            match event {
                InputEvent::Key(k) if k.code.as_deref() == Some("KeyW") => {
                    self.seen.borrow_mut().push("w".into());
                    true
                }
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    ctl.request_pointer_lock(true);
                    ctl.suppress_original_ui(true);
                    true
                }
                InputEvent::PointerLockChanged(false) => {
                    ctl.suppress_original_ui(false);
                    false
                }
                _ => false,
            }
        }
    }

    fn key(code: &str, key: Key) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            code: Some(code.into()),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn a_hook_takes_by_key_code_whatever_the_layout() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut hooks = ModHooks::new();
        hooks.register(Box::new(Walker {
            seen: Rc::clone(&seen),
        }));
        // A Cyrillic layout types "ц" on the W key; the code is still KeyW.
        let w = hooks.claim_input(&key("KeyW", Key::Character("ц".into())));
        assert!(w.taken);
        assert!(
            !hooks
                .claim_input(&key("KeyA", Key::Character("a".into())))
                .taken
        );
        assert_eq!(*seen.borrow(), ["w"]);
    }

    #[test]
    fn a_click_can_take_the_pointer_and_hide_the_ui_until_the_lock_ends() {
        let mut hooks = ModHooks::new();
        hooks.register(Box::new(Walker {
            seen: Rc::default(),
        }));
        let click = events().remove(0);
        let c = hooks.claim_input(&click);
        assert_eq!(
            c,
            Claim {
                taken: true,
                pointer_lock: Some(true)
            }
        );
        assert!(hooks.ui_suppressed());
        // Esc ended the lock in the browser: the hook gives the UI back.
        let lost = hooks.claim_input(&InputEvent::PointerLockChanged(false));
        assert!(!lost.taken);
        assert!(!hooks.ui_suppressed());
    }

    /// Holds a first-person camera from the first click, lets it go on Escape, moves it on every frame.
    struct Looker {
        at: f32,
    }

    impl ModHook for Looker {
        fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool {
            match event {
                InputEvent::Pointer(p) if p.phase == PointerPhase::Down => {
                    ctl.set_camera(Some(Box::new(render::FirstPerson::new(glam::Vec3::new(
                        self.at, 6.5, 0.0,
                    )))));
                    true
                }
                InputEvent::Key(k) if k.code.as_deref() == Some("Escape") => {
                    ctl.set_camera(None);
                    true
                }
                _ => false,
            }
        }

        fn frame(&mut self, info: &FrameInfo<'_>, ctl: &mut HookCtl) {
            if info.dt > 0.0 {
                self.at += 10.0 * info.dt;
                let mut cam = render::FirstPerson::new(glam::Vec3::new(self.at, 6.5, 0.0));
                cam.far = 2000.0;
                ctl.set_camera(Some(Box::new(cam)));
            }
        }
    }

    fn info(dt: f32) -> FrameInfo<'static> {
        FrameInfo {
            dt,
            now_ms: 0,
            focus: glam::Vec2::ZERO,
            terrain: None,
        }
    }

    #[test]
    fn a_hook_holds_the_camera_until_it_gives_it_back() {
        let mut hooks = ModHooks::new();
        assert!(hooks.camera().is_none());
        // No hooks: a frame asks for nothing and changes nothing.
        assert_eq!(hooks.frame(&info(0.016)), None);
        assert!(hooks.camera().is_none());
        hooks.register(Box::new(Looker { at: 1.0 }));
        // Registering a hook does not take the screen; the camera is the hook's to ask for.
        assert_eq!(hooks.frame(&info(0.0)), None);
        assert!(hooks.camera().is_none());
        hooks.claim_input(&events().remove(0));
        let cam = hooks.camera().expect("the click took the camera");
        assert_eq!(cam.eye().x, 1.0);
        assert_eq!(cam.far(), 3000.0);
        // Each frame the hook hands over the camera moved on; the focus follows it.
        hooks.frame(&info(0.5));
        let cam = hooks.camera().expect("held");
        assert_eq!(cam.eye().x, 6.0);
        assert_eq!(cam.far(), 2000.0);
        assert_eq!(cam.terrain_focus(), glam::Vec2::new(6.0, 0.0));
        assert_eq!(cam.hints().terrain_focus, Some(cam.terrain_focus()));
        // Esc gives the strategic camera back.
        let esc = hooks.claim_input(&key("Escape", Key::Named(NamedKey::Escape)));
        assert!(esc.taken);
        assert!(hooks.camera().is_none());
    }

    #[test]
    fn the_first_hook_that_takes_an_event_ends_the_chain() {
        struct Take(bool, Rc<RefCell<u32>>);
        impl ModHook for Take {
            fn claim_input(&mut self, _e: &InputEvent, _c: &mut HookCtl) -> bool {
                *self.1.borrow_mut() += 1;
                self.0
            }
        }
        let calls = Rc::new(RefCell::new(0));
        let mut hooks = ModHooks::new();
        hooks.register(Box::new(Take(false, Rc::clone(&calls))));
        hooks.register(Box::new(Take(true, Rc::clone(&calls))));
        hooks.register(Box::new(Take(true, Rc::clone(&calls))));
        assert!(hooks.claim_input(&InputEvent::Focused(true)).taken);
        assert_eq!(*calls.borrow(), 2, "the third hook is not asked");
    }
}
