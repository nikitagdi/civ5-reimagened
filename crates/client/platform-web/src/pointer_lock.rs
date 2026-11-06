//! `document.pointerLockElement` behind `PointerLockProbe`: the browser ends a pointer lock by itself
//! (Esc), and winit's `set_cursor_grab` reports nothing of that.

use platform::PointerLockProbe;

/// True while the document holds a pointer lock (on any element: the game page has only its canvas).
pub struct WebPointerLockProbe {
    document: web_sys::Document,
}

impl WebPointerLockProbe {
    /// The probe of the page; `None` outside a window.
    pub fn new() -> Option<Self> {
        Some(Self {
            document: web_sys::window()?.document()?,
        })
    }
}

impl PointerLockProbe for WebPointerLockProbe {
    fn locked(&self) -> bool {
        self.document.pointer_lock_element().is_some()
    }
}
