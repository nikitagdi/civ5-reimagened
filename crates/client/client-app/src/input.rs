//! Translates winit events into `platform::InputEvent`.

use platform::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, NamedKey, PhysicalPosition, PhysicalSize,
    PointerEvent, PointerId, PointerKind, PointerPhase, WheelDelta, WheelEvent,
};
use winit::event::{ElementState, MouseScrollDelta, TouchPhase, WindowEvent};
use winit::keyboard::{self, PhysicalKey};

/// Hold a key press until the next event (web only): winit's web backend sends the
/// `ModifiersChanged` of a key event right after that `KeyboardInput`, so Shift+Enter would arrive
/// as Enter whenever the Shift press itself was not seen first (synthetic input, Shift pressed
/// before the canvas had the focus). Natively `ModifiersChanged` comes first.
const HOLD_KEY_PRESSES: bool = cfg!(target_arch = "wasm32");

/// winit omits the cursor position on `MouseInput` and modifiers on pointer events, so track them.
#[derive(Debug, Default)]
pub(crate) struct InputTranslator {
    cursor: PhysicalPosition,
    modifiers: Modifiers,
    /// A key press waiting for the `ModifiersChanged` that may follow it ([`HOLD_KEY_PRESSES`]).
    pending: Option<KeyEvent>,
}

impl InputTranslator {
    /// The input events of a winit event, in order (none for non-input events).
    pub(crate) fn translate(&mut self, event: &WindowEvent) -> Vec<InputEvent> {
        let modifiers_changed = matches!(event, WindowEvent::ModifiersChanged(_));
        let ev = self.translate_one(event);
        self.accept(ev, modifiers_changed, HOLD_KEY_PRESSES)
    }

    /// The held key press, if any (end of an event batch).
    pub(crate) fn flush(&mut self) -> Option<InputEvent> {
        self.pending.take().map(InputEvent::Key)
    }

    /// Orders `ev` after a held key press: a `ModifiersChanged` (`modifiers_changed`) first gives
    /// its modifiers to the held press; any other event releases it unchanged. With `hold` a new
    /// key press is held.
    fn accept(
        &mut self,
        ev: Option<InputEvent>,
        modifiers_changed: bool,
        hold: bool,
    ) -> Vec<InputEvent> {
        let mut out = Vec::new();
        if let Some(mut k) = self.pending.take() {
            if modifiers_changed {
                k.modifiers = self.modifiers;
            }
            out.push(InputEvent::Key(k));
        }
        match ev {
            Some(InputEvent::Key(k)) if hold && k.pressed => self.pending = Some(k),
            Some(e) => out.push(e),
            None => {}
        }
        out
    }

    /// `None` for non-input events.
    fn translate_one(&mut self, event: &WindowEvent) -> Option<InputEvent> {
        match event {
            WindowEvent::Resized(size) => Some(InputEvent::Resized(PhysicalSize {
                width: size.width,
                height: size.height,
            })),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                Some(InputEvent::ScaleFactor(*scale_factor))
            }
            WindowEvent::Focused(f) => Some(InputEvent::Focused(*f)),
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.modifiers = Modifiers {
                    shift: s.shift_key(),
                    ctrl: s.control_key(),
                    alt: s.alt_key(),
                    meta: s.super_key(),
                };
                None
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = PhysicalPosition {
                    x: position.x,
                    y: position.y,
                };
                Some(self.mouse(PointerPhase::Move, None))
            }
            WindowEvent::CursorLeft { .. } => Some(self.mouse(PointerPhase::Left, None)),
            WindowEvent::MouseInput { state, button, .. } => {
                let phase = match state {
                    ElementState::Pressed => PointerPhase::Down,
                    ElementState::Released => PointerPhase::Up,
                };
                Some(self.mouse(phase, Some(map_button(*button))))
            }
            WindowEvent::MouseWheel { delta, .. } => Some(InputEvent::Wheel(WheelEvent {
                delta: match delta {
                    MouseScrollDelta::LineDelta(x, y) => WheelDelta::Lines { x: *x, y: *y },
                    MouseScrollDelta::PixelDelta(p) => WheelDelta::Pixels { x: p.x, y: p.y },
                },
                position: self.cursor,
                modifiers: self.modifiers,
            })),
            WindowEvent::Touch(t) => Some(InputEvent::Pointer(PointerEvent {
                // Shift by 1: id 0 is the mouse.
                id: PointerId(t.id.wrapping_add(1)),
                kind: PointerKind::Touch,
                phase: match t.phase {
                    TouchPhase::Started => PointerPhase::Down,
                    TouchPhase::Moved => PointerPhase::Move,
                    TouchPhase::Ended => PointerPhase::Up,
                    TouchPhase::Cancelled => PointerPhase::Cancel,
                },
                position: PhysicalPosition {
                    x: t.location.x,
                    y: t.location.y,
                },
                button: None,
                modifiers: self.modifiers,
            })),
            WindowEvent::KeyboardInput { event, .. } => Some(InputEvent::Key(KeyEvent {
                key: {
                    let key = map_key(&event.logical_key);
                    self.track_modifier_key(&key, event.state == ElementState::Pressed);
                    key
                },
                code: match event.physical_key {
                    // KeyCode names match DOM `KeyboardEvent.code`.
                    PhysicalKey::Code(c) => Some(format!("{c:?}")),
                    PhysicalKey::Unidentified(_) => None,
                },
                pressed: event.state == ElementState::Pressed,
                repeat: event.repeat,
                modifiers: self.modifiers,
            })),
            _ => None,
        }
    }

    /// A modifier key itself changes the held modifiers at once. winit's web backend sends
    /// `ModifiersChanged` only after the `KeyboardInput` that carried the change, so a Shift
    /// whose own event had no shift flag (synthetic input, Shift pressed before the canvas had
    /// the focus) would make the next Enter arrive without Shift.
    fn track_modifier_key(&mut self, key: &Key, pressed: bool) {
        match key {
            Key::Named(NamedKey::Shift) => self.modifiers.shift = pressed,
            Key::Named(NamedKey::Control) => self.modifiers.ctrl = pressed,
            Key::Named(NamedKey::Alt) => self.modifiers.alt = pressed,
            Key::Named(NamedKey::Meta) => self.modifiers.meta = pressed,
            _ => {}
        }
    }

    fn mouse(&self, phase: PointerPhase, button: Option<MouseButton>) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId::MOUSE,
            kind: PointerKind::Mouse,
            phase,
            position: self.cursor,
            button,
            modifiers: self.modifiers,
        })
    }
}

fn map_button(b: winit::event::MouseButton) -> MouseButton {
    use winit::event::MouseButton as W;
    match b {
        W::Left => MouseButton::Left,
        W::Right => MouseButton::Right,
        W::Middle => MouseButton::Middle,
        W::Back => MouseButton::Back,
        W::Forward => MouseButton::Forward,
        W::Other(n) => MouseButton::Other(n),
    }
}

fn map_key(k: &keyboard::Key) -> Key {
    use keyboard::NamedKey as W;
    match k {
        keyboard::Key::Character(s) => Key::Character(s.to_string()),
        keyboard::Key::Named(n) => {
            let named = match n {
                W::Enter => NamedKey::Enter,
                W::Escape => NamedKey::Escape,
                W::Tab => NamedKey::Tab,
                W::Backspace => NamedKey::Backspace,
                W::Delete => NamedKey::Delete,
                W::Insert => NamedKey::Insert,
                W::Space => NamedKey::Space,
                W::ArrowUp => NamedKey::ArrowUp,
                W::ArrowDown => NamedKey::ArrowDown,
                W::ArrowLeft => NamedKey::ArrowLeft,
                W::ArrowRight => NamedKey::ArrowRight,
                W::Home => NamedKey::Home,
                W::End => NamedKey::End,
                W::PageUp => NamedKey::PageUp,
                W::PageDown => NamedKey::PageDown,
                W::Shift => NamedKey::Shift,
                W::Control => NamedKey::Control,
                W::Alt => NamedKey::Alt,
                W::Super | W::Meta => NamedKey::Meta,
                W::F1 => NamedKey::F1,
                W::F2 => NamedKey::F2,
                W::F3 => NamedKey::F3,
                W::F4 => NamedKey::F4,
                W::F5 => NamedKey::F5,
                W::F6 => NamedKey::F6,
                W::F7 => NamedKey::F7,
                W::F8 => NamedKey::F8,
                W::F9 => NamedKey::F9,
                W::F10 => NamedKey::F10,
                W::F11 => NamedKey::F11,
                W::F12 => NamedKey::F12,
                _ => return Key::Unidentified,
            };
            Key::Named(named)
        }
        _ => Key::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_key_press_takes_the_modifiers_that_follow_it() {
        let mut t = InputTranslator::default();
        let enter = |pressed| {
            Some(InputEvent::Key(KeyEvent {
                key: Key::Named(NamedKey::Enter),
                code: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::default(),
            }))
        };
        // Enter, then winit's ModifiersChanged(Shift): one Shift+Enter.
        assert!(t.accept(enter(true), false, true).is_empty(), "held");
        t.modifiers.shift = true;
        let out = t.accept(None, true, true);
        assert!(matches!(&out[..], [InputEvent::Key(k)] if k.pressed && k.modifiers.shift));
        // Another event releases a held press unchanged, before itself.
        t.modifiers.shift = false;
        t.accept(enter(true), false, true);
        let out = t.accept(enter(false), false, true);
        assert!(matches!(&out[..], [InputEvent::Key(a), InputEvent::Key(b)]
            if a.pressed && !b.pressed && !a.modifiers.shift));
        // Natively nothing is held; the end of a batch flushes.
        assert_eq!(t.accept(enter(true), false, false).len(), 1);
        t.accept(enter(true), false, true);
        assert!(t.flush().is_some() && t.flush().is_none());
    }

    #[test]
    fn modifier_keys_set_the_modifiers_before_the_next_key() {
        let mut t = InputTranslator::default();
        t.track_modifier_key(&Key::Named(NamedKey::Shift), true);
        assert!(
            t.modifiers.shift,
            "Shift+Enter carries Shift even before ModifiersChanged"
        );
        t.track_modifier_key(&Key::Named(NamedKey::Enter), true);
        assert!(t.modifiers.shift);
        t.track_modifier_key(&Key::Named(NamedKey::Shift), false);
        t.track_modifier_key(&Key::Named(NamedKey::Control), true);
        assert!(!t.modifiers.shift && t.modifiers.ctrl);
    }
}
