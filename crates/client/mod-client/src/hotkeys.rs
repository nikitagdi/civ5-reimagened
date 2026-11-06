//! Key bindings of mods (C4): the table, and the policy against the game's own keys.
//!
//! **Policy.** The game binds nearly every key (the original's action table, the unit orders, END TURN, the camera, the
//! chat and the performance panel). A mod must never take one of them away without saying so:
//!
//! - a chord on a key the game binds, with no Ctrl and no Alt (so bare or with Shift only), is **refused** at
//!   registration (`CtxError::Rejected` naming the key). The mod keeps the key it had no right to;
//! - a chord with Ctrl or Alt on such a key is **allowed with a notice** (the game binds a few such chords and the table
//!   cannot be listed here); the host logs it (`take_notices`). The game still comes first: see Dispatch;
//! - a key the game does not bind (`ContextMenu`, `F13`, `IntlBackslash` and so on) is free;
//! - two bindings never share a chord; a mod's `id` is unique within the mod; a modifier key alone cannot be bound.
//!
//! **Dispatch.** A key press reaches [`HotkeyTable::matches`] only after the game's UI has declined it: no mod's
//! `claim_input` took it, the egui screens and the Lua UI did not use it (so a typed letter in a text box, or a key
//! the game's action table carried out, never fires a mod), and the map's own keys did not take it. A matched press
//! does not reach the game's map either; a repeat of the held key is swallowed without firing again.

use crate::ctx::{CtxError, Hotkeys};
use crate::types::HotkeySpec;

/// How the game uses a chord.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyUse {
    /// Nothing in the game binds the key.
    Free,
    /// The game binds the key; with Ctrl or Alt it may or may not bind this exact chord.
    Maybe,
    /// The game binds the chord (the key with no Ctrl and no Alt), or the key cannot be bound at all.
    Taken,
}

/// DOM `KeyboardEvent.code` values of the keys the game's tables bind (`exe-api::hotkeys::kb_of_vk` and the
/// interim controls).
const GAME_CODES: &[&str] = &[
    "KeyA",
    "KeyB",
    "KeyC",
    "KeyD",
    "KeyE",
    "KeyF",
    "KeyG",
    "KeyH",
    "KeyI",
    "KeyJ",
    "KeyK",
    "KeyL",
    "KeyM",
    "KeyN",
    "KeyO",
    "KeyP",
    "KeyQ",
    "KeyR",
    "KeyS",
    "KeyT",
    "KeyU",
    "KeyV",
    "KeyW",
    "KeyX",
    "KeyY",
    "KeyZ",
    "Digit0",
    "Digit1",
    "Digit2",
    "Digit3",
    "Digit4",
    "Digit5",
    "Digit6",
    "Digit7",
    "Digit8",
    "Digit9",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Semicolon",
    "Quote",
    "Backquote",
    "Backslash",
    "Comma",
    "Period",
    "Slash",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "Enter",
    "Escape",
    "Space",
    "Backspace",
    "Tab",
    "CapsLock",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Insert",
    "Delete",
    "Pause",
    "ScrollLock",
    "NumLock",
    "Numpad0",
    "Numpad1",
    "Numpad2",
    "Numpad3",
    "Numpad4",
    "Numpad5",
    "Numpad6",
    "Numpad7",
    "Numpad8",
    "Numpad9",
    "NumpadAdd",
    "NumpadSubtract",
    "NumpadMultiply",
    "NumpadDivide",
    "NumpadDecimal",
];

const MODIFIER_CODES: &[&str] = &[
    "ShiftLeft",
    "ShiftRight",
    "ControlLeft",
    "ControlRight",
    "AltLeft",
    "AltRight",
    "MetaLeft",
    "MetaRight",
];

/// The policy above for a chord.
pub fn game_key(spec: &HotkeySpec) -> KeyUse {
    if MODIFIER_CODES.contains(&spec.code.as_str()) {
        return KeyUse::Taken;
    }
    if !GAME_CODES.contains(&spec.code.as_str()) {
        return KeyUse::Free;
    }
    if spec.ctrl || spec.alt {
        KeyUse::Maybe
    } else {
        KeyUse::Taken
    }
}

/// The bindings of all mods of a game. Implements [`Hotkeys`] for the mod the host selected with
/// [`select`](Self::select) (the same way the other services follow `ClientHost::ctx`).
#[derive(Debug, Default)]
pub struct HotkeyTable {
    cur: u16,
    bound: Vec<(u16, HotkeySpec)>,
    notices: Vec<String>,
}

impl HotkeyTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The mod the next `register`/`unregister` are about.
    pub fn select(&mut self, mod_idx: u16) {
        self.cur = mod_idx;
    }

    pub fn len(&self) -> usize {
        self.bound.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bound.is_empty()
    }

    /// Drops every binding of a mod (it was detached).
    pub fn remove_mod(&mut self, mod_idx: u16) {
        self.bound.retain(|(m, _)| *m != mod_idx);
    }

    /// Drops everything (the game ended).
    pub fn clear(&mut self) {
        self.bound.clear();
    }

    /// Lines for the log about chords that were allowed but may hide a game key.
    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    /// The binding that fits this exact chord: `(mod_idx, id)`.
    pub fn matches(&self, code: &str, ctrl: bool, shift: bool, alt: bool) -> Option<(u16, &str)> {
        self.bound
            .iter()
            .find(|(_, h)| h.code == code && h.ctrl == ctrl && h.shift == shift && h.alt == alt)
            .map(|(m, h)| (*m, h.id.as_str()))
    }

    /// The bindings, for a key-binding list: `(mod_idx, spec)`.
    pub fn list(&self) -> impl Iterator<Item = &(u16, HotkeySpec)> {
        self.bound.iter()
    }
}

impl Hotkeys for HotkeyTable {
    fn register(&mut self, spec: HotkeySpec) -> Result<(), CtxError> {
        let m = self.cur;
        if spec.id.is_empty() || spec.code.is_empty() {
            return Err(CtxError::Rejected("a hotkey needs an id and a key".into()));
        }
        if self.bound.iter().any(|(o, h)| *o == m && h.id == spec.id) {
            return Err(CtxError::Rejected(format!(
                "hotkey id `{}` is already registered",
                spec.id
            )));
        }
        let use_ = game_key(&spec);
        if use_ == KeyUse::Taken {
            return Err(CtxError::Rejected(format!(
                "`{}` is a key of the game{}; bind it with Ctrl or Alt, or use a key the game leaves free",
                spec.code,
                if spec.shift {
                    " (Shift does not change that)"
                } else {
                    ""
                }
            )));
        }
        if let Some((_, other)) = self.matches(&spec.code, spec.ctrl, spec.shift, spec.alt) {
            return Err(CtxError::Rejected(format!(
                "`{}` is already bound to `{other}`",
                spec.code
            )));
        }
        if use_ == KeyUse::Maybe {
            self.notices.push(format!(
                "mod hotkey `{}` is bound to {}{}{}, a key the game uses; it fires only when the game did not use the press",
                spec.id,
                if spec.ctrl { "Ctrl+" } else { "" },
                if spec.alt { "Alt+" } else { "" },
                spec.code
            ));
        }
        self.bound.push((m, spec));
        Ok(())
    }

    fn unregister(&mut self, id: &str) {
        let m = self.cur;
        self.bound.retain(|(o, h)| !(*o == m && h.id == id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str, code: &str, ctrl: bool, shift: bool, alt: bool) -> HotkeySpec {
        HotkeySpec {
            id: id.into(),
            code: code.into(),
            ctrl,
            shift,
            alt,
            label: id.into(),
        }
    }

    #[test]
    fn a_bare_or_shifted_game_key_is_refused() {
        let mut t = HotkeyTable::new();
        for (code, shift) in [
            ("KeyW", false),
            ("KeyH", true),
            ("Enter", false),
            ("F3", false),
        ] {
            let e = t
                .register(spec("x", code, false, shift, false))
                .unwrap_err();
            assert!(
                matches!(e, CtxError::Rejected(m) if m.contains(code)),
                "{code}"
            );
        }
        assert!(t.is_empty());
    }

    #[test]
    fn a_ctrl_or_alt_chord_on_a_game_key_is_allowed_and_noticed() {
        let mut t = HotkeyTable::new();
        t.register(spec("a", "KeyH", true, true, false)).unwrap();
        t.register(spec("b", "F5", false, false, true)).unwrap();
        let n = t.take_notices();
        assert_eq!(n.len(), 2);
        assert!(n[0].contains("Ctrl+KeyH"), "{n:?}");
        assert!(t.take_notices().is_empty());
    }

    #[test]
    fn a_key_the_game_leaves_free_binds_bare() {
        let mut t = HotkeyTable::new();
        t.register(spec("menu", "ContextMenu", false, false, false))
            .unwrap();
        t.register(spec("f13", "F13", false, true, false)).unwrap();
        assert!(t.take_notices().is_empty());
        assert_eq!(t.matches("F13", false, true, false), Some((0, "f13")));
        assert_eq!(
            t.matches("F13", false, false, false),
            None,
            "modifiers are exact"
        );
    }

    #[test]
    fn modifier_keys_alone_cannot_be_bound() {
        let mut t = HotkeyTable::new();
        assert!(
            t.register(spec("m", "ShiftLeft", false, false, false))
                .is_err()
        );
        assert!(
            t.register(spec("m", "ControlRight", true, false, false))
                .is_err()
        );
    }

    #[test]
    fn chords_and_ids_do_not_repeat() {
        let mut t = HotkeyTable::new();
        t.select(1);
        t.register(spec("go", "F13", false, false, false)).unwrap();
        assert!(
            t.register(spec("go", "F14", false, false, false)).is_err(),
            "id twice"
        );
        t.select(2);
        assert!(
            t.register(spec("other", "F13", false, false, false))
                .is_err(),
            "the chord is taken by another mod"
        );
        // The same id in another mod is fine.
        t.register(spec("go", "F14", false, false, false)).unwrap();
        assert_eq!(t.matches("F14", false, false, false), Some((2, "go")));
    }

    #[test]
    fn unregister_and_remove_mod_free_the_chord() {
        let mut t = HotkeyTable::new();
        t.select(1);
        t.register(spec("go", "F13", false, false, false)).unwrap();
        t.unregister("go");
        assert!(t.is_empty());
        t.register(spec("go", "F13", false, false, false)).unwrap();
        t.select(2);
        t.register(spec("up", "F14", false, false, false)).unwrap();
        t.remove_mod(1);
        assert_eq!(t.len(), 1);
        assert_eq!(t.matches("F13", false, false, false), None);
        t.clear();
        assert!(t.is_empty());
    }

    #[test]
    fn empty_ids_and_keys_are_refused() {
        let mut t = HotkeyTable::new();
        assert!(t.register(spec("", "F13", false, false, false)).is_err());
        assert!(t.register(spec("x", "", false, false, false)).is_err());
    }
}
