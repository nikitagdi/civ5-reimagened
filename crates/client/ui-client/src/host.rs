//! `UiHost` of the client: textures from the store, the clock, the cursor.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use audio::{ButtonEvent, ButtonFlags};
use ui::{TextureInfo, UiHost};

use crate::store::UiStore;

/// What the host counts and remembers for the layer to read back (the host itself is boxed inside `UiCore`).
#[derive(Default)]
pub struct HostCells {
    /// Seconds since the start of the program, set by the layer each frame.
    pub now: Cell<f64>,
    pub cursor: Cell<i32>,
    /// Button sounds asked for, ever.
    pub sounds: Cell<u64>,
    /// Button sounds asked for and not yet taken by the app (`UiLayer::take_button_sounds`).
    pub pending_sounds: RefCell<Vec<(ButtonEvent, ButtonFlags)>>,
    /// World point (the Lua world: x east, y north, z up) to UI pixels; the app sets it each frame (M2).
    pub projector: RefCell<Option<Projector>>,
}

/// See [HostCells::projector].
pub type Projector = Rc<dyn Fn([f32; 3]) -> Option<[f32; 2]>>;

/// More than this many untaken sounds are dropped (an app with no audio never drains the queue).
pub const MAX_PENDING_SOUNDS: usize = 64;

pub struct ClientHost {
    store: Rc<UiStore>,
    cells: Rc<HostCells>,
}

impl ClientHost {
    pub fn new(store: Rc<UiStore>, cells: Rc<HostCells>) -> ClientHost {
        ClientHost { store, cells }
    }
}

impl UiHost for ClientHost {
    fn texture(&mut self, name: &str) -> Option<TextureInfo> {
        self.store.texture_info(name)
    }

    fn button_sound(&mut self, event: ButtonEvent, flags: ButtonFlags) {
        // SIMPLIFIED(SIMP-uicl-1): the layer only queues the button sounds; it plays nothing itself. The app drains
        // the queue into `audio::UiAudio::on_button`, which applies the rules (`ButtonSoundRules`, SIMP-uiau-1).
        // client-app plays the queue through Web Audio (`client-app/ui_sound.rs`).
        self.cells.sounds.set(self.cells.sounds.get() + 1);
        let mut q = self.cells.pending_sounds.borrow_mut();
        if q.len() < MAX_PENDING_SOUNDS {
            q.push((event, flags));
        }
    }

    fn set_cursor(&mut self, cursor: i32) {
        self.cells.cursor.set(cursor);
    }

    fn project(&mut self, world: [f32; 3]) -> Option<[f32; 2]> {
        let p = self.cells.projector.borrow().clone()?;
        p(world)
    }

    fn now(&self) -> f64 {
        self.cells.now.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::ControlKind;

    fn flags() -> ButtonFlags {
        ButtonFlags {
            no_default_sound: false,
            disabled: false,
            kind: ControlKind::Button,
        }
    }

    #[test]
    fn button_sounds_are_queued_counted_and_capped() {
        let cells = Rc::new(HostCells::default());
        let mut host = ClientHost::new(
            UiStore::new(Default::default(), Vec::new()).unwrap(),
            cells.clone(),
        );
        host.button_sound(ButtonEvent::Click, flags());
        host.button_sound(ButtonEvent::MouseOver, flags());
        assert_eq!(cells.sounds.get(), 2);
        let taken = cells.pending_sounds.take();
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0].0, ButtonEvent::Click);
        for _ in 0..MAX_PENDING_SOUNDS + 10 {
            host.button_sound(ButtonEvent::Click, flags());
        }
        assert_eq!(cells.pending_sounds.borrow().len(), MAX_PENDING_SOUNDS);
        assert_eq!(cells.sounds.get(), 2 + MAX_PENDING_SOUNDS as u64 + 10);
    }
}
