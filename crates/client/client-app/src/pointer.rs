//! Pointer lock of the game window (client-hooks, I2): winit behind `platform::CursorGrab`, the device's
//! relative motion as `InputEvent::PointerDelta`, and the lock changes as `InputEvent::PointerLockChanged`.
//!
//! Without a request nothing here makes an event, so the app behaves as it did without this module.

use platform::{
    CursorGrab, GrabError, InputEvent, PointerLock, PointerLockMode, PointerLockProbe,
    PointerLockStatus,
};
use winit::error::ExternalError;
use winit::window::{CursorGrabMode, Window};

/// `Window::set_cursor_grab` and `set_cursor_visible` as a [`CursorGrab`].
struct WinitGrab<'a>(&'a Window);

impl CursorGrab for WinitGrab<'_> {
    fn grab(&mut self, mode: Option<PointerLockMode>) -> Result<(), GrabError> {
        let mode = match mode {
            None => CursorGrabMode::None,
            Some(PointerLockMode::Locked) => CursorGrabMode::Locked,
            Some(PointerLockMode::Confined) => CursorGrabMode::Confined,
        };
        self.0.set_cursor_grab(mode).map_err(|e| match e {
            ExternalError::NotSupported(_) => GrabError::Unsupported,
            _ => GrabError::Refused,
        })
    }

    fn set_cursor_visible(&mut self, visible: bool) {
        self.0.set_cursor_visible(visible);
    }
}

/// The lock of the app's window and what feeds it.
#[derive(Default)]
pub struct PointerService {
    lock: PointerLock,
    probe: Option<Box<dyn PointerLockProbe>>,
}

impl PointerService {
    pub fn new(probe: Option<Box<dyn PointerLockProbe>>) -> Self {
        Self {
            lock: PointerLock::new(),
            probe,
        }
    }

    /// Asks for the lock (`Locked`, else `Confined`). On the web this must run inside the handling of a click or a
    /// key press: the browser refuses the request otherwise, and the probe then sees no lock.
    pub fn request(&mut self, window: &Window) -> PointerLockStatus {
        self.lock.request(&mut WinitGrab(window))
    }

    pub fn release(&mut self, window: &Window) {
        self.lock.release(&mut WinitGrab(window));
    }

    pub fn status(&self) -> PointerLockStatus {
        self.lock.status()
    }

    /// The cursor is held and hidden; the app must not show a cursor of its own.
    pub fn engaged(&self) -> bool {
        self.lock.engaged()
    }

    /// The relative motion of the mouse device (`DeviceEvent::MouseMotion`); `None` unless the lock is engaged.
    pub fn motion(&self, dx: f64, dy: f64) -> Option<InputEvent> {
        self.lock.delta_event(dx, dy)
    }

    /// The window lost the focus: the lock goes with it.
    pub fn focus_lost(&mut self, window: &Window) {
        self.lock.focus_lost(&mut WinitGrab(window));
    }

    /// Once a frame: follows the browser (Esc ends a lock), then the change event if there is one.
    pub fn poll(&mut self, window: &Window) -> Option<InputEvent> {
        if let Some(probe) = self.probe.as_deref() {
            self.lock.follow_probe(probe, &mut WinitGrab(window));
        }
        self.lock.take_change()
    }
}
