//! Pointer lock: the mouse stops moving the cursor and reports relative motion instead
//! ([`crate::InputEvent::PointerDelta`]). Needed by first-person cameras (client-hooks, I2).
//!
//! This module is pure state and logic. The window system sits behind [`CursorGrab`] (winit in
//! `client-app`), and the browser's own view of the lock behind [`PointerLockProbe`] (`platform-web`).
//!
//! Policy (documented in `SPEC-client-hooks.md`):
//! - the lock is asked for in a user gesture (a click or a key press) on the web, or it is refused;
//! - [`PointerLockMode::Locked`] first; where the platform has no such mode (X11, some Windows set-ups)
//!   [`PointerLockMode::Confined`] follows: the cursor stays in the window and the relative motion comes
//!   from the raw mouse device, so the camera still turns without limit;
//! - the cursor is hidden while the lock is engaged and shown again on release;
//! - without a request nothing here does anything, and no event is made.

use crate::input::InputEvent;

/// How the window system holds the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerLockMode {
    /// The cursor does not move at all (browsers: Pointer Lock API; Windows, macOS: pinned).
    Locked,
    /// The cursor is kept inside the window. The fallback where `Locked` is not supported.
    Confined,
}

/// Why the window system did not grab the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabError {
    /// This mode does not exist on this platform.
    Unsupported,
    /// The system refused (no focus, no user gesture, ...).
    Refused,
}

/// The window system's side of the lock: winit's `set_cursor_grab` and `set_cursor_visible`.
pub trait CursorGrab {
    /// Grabs the cursor in `mode`; `None` lets it go.
    fn grab(&mut self, mode: Option<PointerLockMode>) -> Result<(), GrabError>;
    fn set_cursor_visible(&mut self, visible: bool);
}

/// What the page reports about the lock (web only): the browser can end it without asking us (Esc).
pub trait PointerLockProbe {
    /// The browser holds a pointer lock on the game canvas now.
    fn locked(&self) -> bool;
}

/// Where the lock stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerLockStatus {
    /// Nothing asked, or let go.
    Released,
    /// Asked and granted in this mode.
    Engaged(PointerLockMode),
    /// Asked, and no mode worked. Stays so until the next request.
    Denied,
}

/// The lock of one window.
#[derive(Debug)]
pub struct PointerLock {
    status: PointerLockStatus,
    /// The last `engaged()` an event was made for.
    reported: bool,
    /// Web: the probe saw the browser hold the lock since the request. Before that the lock is still
    /// being granted (a promise in the browser) and a missing lock is not a loss.
    probe_seen: bool,
}

impl Default for PointerLock {
    fn default() -> Self {
        Self::new()
    }
}

impl PointerLock {
    pub const fn new() -> Self {
        Self {
            status: PointerLockStatus::Released,
            reported: false,
            probe_seen: false,
        }
    }

    pub fn status(&self) -> PointerLockStatus {
        self.status
    }

    /// The cursor is held: deltas flow and the cursor is hidden.
    pub fn engaged(&self) -> bool {
        matches!(self.status, PointerLockStatus::Engaged(_))
    }

    /// Asks for the lock: `Locked`, then `Confined`. Asking again while engaged changes nothing.
    pub fn request(&mut self, grab: &mut dyn CursorGrab) -> PointerLockStatus {
        if self.engaged() {
            return self.status;
        }
        self.probe_seen = false;
        for mode in [PointerLockMode::Locked, PointerLockMode::Confined] {
            match grab.grab(Some(mode)) {
                Ok(()) => {
                    grab.set_cursor_visible(false);
                    self.status = PointerLockStatus::Engaged(mode);
                    return self.status;
                }
                Err(GrabError::Unsupported | GrabError::Refused) => {}
            }
        }
        self.status = PointerLockStatus::Denied;
        self.status
    }

    /// Lets the cursor go and shows it. Harmless when nothing is held.
    pub fn release(&mut self, grab: &mut dyn CursorGrab) {
        if self.status == PointerLockStatus::Released {
            return;
        }
        // Errors on release cannot be acted on: the cursor is ours to show either way.
        let _ = grab.grab(None);
        grab.set_cursor_visible(true);
        self.status = PointerLockStatus::Released;
        self.probe_seen = false;
    }

    /// The browser ended the lock (Esc) or granted it late: follows the probe. Call once a frame, web only.
    /// Returns true if the lock was lost and has been let go here.
    pub fn follow_probe(
        &mut self,
        probe: &dyn PointerLockProbe,
        grab: &mut dyn CursorGrab,
    ) -> bool {
        if !self.engaged() {
            return false;
        }
        if probe.locked() {
            self.probe_seen = true;
            return false;
        }
        if self.probe_seen {
            self.release(grab);
            return true;
        }
        false
    }

    /// The window lost the focus: the system lets the cursor go by itself; so does the state here.
    pub fn focus_lost(&mut self, grab: &mut dyn CursorGrab) {
        self.release(grab);
    }

    /// The event for a change of [`PointerLock::engaged`] since the last call, if any. Never made without
    /// a request.
    pub fn take_change(&mut self) -> Option<InputEvent> {
        let now = self.engaged();
        (now != self.reported).then(|| {
            self.reported = now;
            InputEvent::PointerLockChanged(now)
        })
    }

    /// A relative motion of the mouse device as an event: only while the lock is engaged, and not a zero one.
    pub fn delta_event(&self, dx: f64, dy: f64) -> Option<InputEvent> {
        (self.engaged() && (dx != 0.0 || dy != 0.0)).then_some(InputEvent::PointerDelta { dx, dy })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        /// Modes the system accepts.
        accepts: Vec<PointerLockMode>,
        held: Option<PointerLockMode>,
        visible: Option<bool>,
        asked: Vec<Option<PointerLockMode>>,
    }

    impl CursorGrab for Fake {
        fn grab(&mut self, mode: Option<PointerLockMode>) -> Result<(), GrabError> {
            self.asked.push(mode);
            match mode {
                None => {
                    self.held = None;
                    Ok(())
                }
                Some(m) if self.accepts.contains(&m) => {
                    self.held = Some(m);
                    Ok(())
                }
                Some(_) => Err(GrabError::Unsupported),
            }
        }
        fn set_cursor_visible(&mut self, visible: bool) {
            self.visible = Some(visible);
        }
    }

    struct Probe(std::cell::Cell<bool>);
    impl PointerLockProbe for Probe {
        fn locked(&self) -> bool {
            self.0.get()
        }
    }

    #[test]
    fn nothing_asked_makes_no_event_and_no_delta() {
        let mut l = PointerLock::new();
        assert_eq!(l.status(), PointerLockStatus::Released);
        assert!(l.take_change().is_none());
        assert!(
            l.delta_event(3.0, 4.0).is_none(),
            "a free mouse makes no deltas"
        );
    }

    #[test]
    fn locked_is_tried_first_and_hides_the_cursor() {
        let mut g = Fake {
            accepts: vec![PointerLockMode::Locked, PointerLockMode::Confined],
            ..Fake::default()
        };
        let mut l = PointerLock::new();
        assert_eq!(
            l.request(&mut g),
            PointerLockStatus::Engaged(PointerLockMode::Locked)
        );
        assert_eq!(g.asked, [Some(PointerLockMode::Locked)]);
        assert_eq!(g.visible, Some(false));
        assert_eq!(l.take_change(), Some(InputEvent::PointerLockChanged(true)));
        assert!(l.take_change().is_none(), "one event for one change");
        assert_eq!(
            l.delta_event(1.5, -2.0),
            Some(InputEvent::PointerDelta { dx: 1.5, dy: -2.0 })
        );
        assert!(l.delta_event(0.0, 0.0).is_none());
    }

    #[test]
    fn confined_is_the_fallback_where_locked_is_unsupported() {
        let mut g = Fake {
            accepts: vec![PointerLockMode::Confined],
            ..Fake::default()
        };
        let mut l = PointerLock::new();
        assert_eq!(
            l.request(&mut g),
            PointerLockStatus::Engaged(PointerLockMode::Confined)
        );
        assert_eq!(
            g.asked,
            [
                Some(PointerLockMode::Locked),
                Some(PointerLockMode::Confined)
            ]
        );
        assert!(
            l.delta_event(1.0, 0.0).is_some(),
            "the raw device still turns the camera"
        );
    }

    #[test]
    fn a_refusal_of_both_modes_is_denied_and_hands_out_nothing() {
        let mut g = Fake::default();
        let mut l = PointerLock::new();
        assert_eq!(l.request(&mut g), PointerLockStatus::Denied);
        assert!(l.take_change().is_none());
        assert!(l.delta_event(1.0, 1.0).is_none());
        assert_eq!(g.visible, None, "the cursor stays as it was");
        // A later request may work.
        g.accepts.push(PointerLockMode::Locked);
        assert!(matches!(l.request(&mut g), PointerLockStatus::Engaged(_)));
    }

    #[test]
    fn release_shows_the_cursor_and_reports_the_change() {
        let mut g = Fake {
            accepts: vec![PointerLockMode::Locked],
            ..Fake::default()
        };
        let mut l = PointerLock::new();
        l.request(&mut g);
        l.take_change();
        l.release(&mut g);
        assert_eq!(g.held, None);
        assert_eq!(g.visible, Some(true));
        assert_eq!(l.take_change(), Some(InputEvent::PointerLockChanged(false)));
        // Releasing again is harmless.
        let asked = g.asked.len();
        l.release(&mut g);
        assert_eq!(g.asked.len(), asked);
    }

    #[test]
    fn the_probe_ends_the_lock_only_after_the_browser_granted_it() {
        let mut g = Fake {
            accepts: vec![PointerLockMode::Locked],
            ..Fake::default()
        };
        let probe = Probe(std::cell::Cell::new(false));
        let mut l = PointerLock::new();
        l.request(&mut g);
        // Still being granted: not a loss.
        assert!(!l.follow_probe(&probe, &mut g));
        assert!(l.engaged());
        probe.0.set(true);
        assert!(!l.follow_probe(&probe, &mut g));
        // Esc: the browser lets go.
        probe.0.set(false);
        assert!(l.follow_probe(&probe, &mut g));
        assert!(!l.engaged());
        assert_eq!(g.visible, Some(true));
        assert!(!l.follow_probe(&probe, &mut g), "nothing more to lose");
    }

    #[test]
    fn focus_loss_releases() {
        let mut g = Fake {
            accepts: vec![PointerLockMode::Locked],
            ..Fake::default()
        };
        let mut l = PointerLock::new();
        l.request(&mut g);
        l.focus_lost(&mut g);
        assert!(!l.engaged());
    }
}
