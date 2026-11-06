//! Hooks of mods into the app (client-hooks I1, I2, C3): the registration point of the client side of mods.
//!
//! The app asks the hooks first about every input event. A hook may take the event, ask for the pointer lock and
//! raise or lower the suppression of the original UI. Only values cross this border (D2): the hooks never see
//! the app. With no hook registered nothing here does anything: `claim_input` returns "not taken" at once, no
//! state changes, and the app behaves as it did without this module.

use platform::InputEvent;

/// What a hook asks of the app while it handles an event. Requests are values; the app carries them out.
#[derive(Debug, Default)]
pub struct HookCtl {
    pointer_lock: Option<bool>,
    suppress_ui: Option<bool>,
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
}

/// The client side of a mod, as far as the input goes. More hooks (frame, camera) join it with the
/// tasks that need them.
pub trait ModHook {
    /// Called first for every input event, before the performance panel, egui, the original UI and the map.
    /// Returns true if the event is taken: it goes no further and no other hook sees it.
    ///
    /// Keys: read `KeyEvent::code` (the physical key, `KeyW`), not `key`, so the layout and Shift do not matter.
    fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool;
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

    /// Asks the hooks about `event`, in order, up to the first that takes it.
    pub fn claim_input(&mut self, event: &InputEvent) -> Claim {
        let mut claim = Claim::default();
        for hook in &mut self.hooks {
            let mut ctl = HookCtl::default();
            let taken = hook.claim_input(event, &mut ctl);
            if let Some(on) = ctl.suppress_ui {
                self.ui_suppressed = on;
            }
            if ctl.pointer_lock.is_some() {
                claim.pointer_lock = ctl.pointer_lock;
            }
            if taken {
                claim.taken = true;
                break;
            }
        }
        claim
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
