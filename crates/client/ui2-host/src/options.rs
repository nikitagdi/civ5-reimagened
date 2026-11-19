//! The client's options and the player's hotkeys in UI v2's host.
//!
//! The options window edits the classic client's options (`ui2_actions::options`): the application gives the
//! running UI the page's store ([`V2Ui::set_client_store`], over [`KvSettings`]), takes the volume knobs the window
//! changed ([`V2Ui::take_volume_changes`]) for its audio buses, and the frame ends the turn by itself when the
//! multiplayer auto end turn is on ([`crate::frame`]).
//!
//! The hotkeys: the running skin's `data-hotkey` keys are the actions module's rows ([`skin_hotkeys`]); a key press
//! goes through [`V2Ui::handle_input`] here first: while a row waits for a key the press is its new key, and with the
//! player's bindings in force a press of a bound key reaches the runtime as the key the skin binds
//! (`ui2_actions::Remap`). A focused text field takes its keys as they are.

use std::rc::Rc;

use platform::{InputEvent, Key, KeyEvent, KeyValueStore, Modifiers};
use ui2_actions::{Remap, SettingsStore};
use ui2_package::{Package, hotkey_mods};
use ui2_runtime::InputOutcome;

use crate::v2::V2Ui;

/// The page's key-value store as the actions module's settings store.
pub struct KvSettings(pub Rc<dyn KeyValueStore>);

impl SettingsStore for KvSettings {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key)
    }

    fn set(&mut self, key: &str, value: &str) {
        self.0.set(key, value);
    }

    fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }
}

/// `[Ctrl+][Alt+][Shift+]<code>` of a key and its modifier bits (`ui2_package::hotkey_mods`).
fn chord(code: &str, mods: u8) -> String {
    let mut out = String::new();
    for (bit, name) in [
        (hotkey_mods::CTRL, "Ctrl+"),
        (hotkey_mods::ALT, "Alt+"),
        (hotkey_mods::SHIFT, "Shift+"),
    ] {
        if mods & bit != 0 {
            out.push_str(name);
        }
    }
    out.push_str(code);
    out
}

/// The key of a key event as the skin's hotkeys write it; `None` when the platform did not report the code.
#[must_use]
pub fn chord_of(k: &KeyEvent) -> Option<String> {
    let code = k.code.as_deref()?;
    let m = k.modifiers;
    let mods = (if m.ctrl { hotkey_mods::CTRL } else { 0 })
        | (if m.alt { hotkey_mods::ALT } else { 0 })
        | (if m.shift { hotkey_mods::SHIFT } else { 0 });
    Some(chord(code, mods))
}

/// The event of the key `to` for a press or release `k` of another one.
fn as_key(to: &str, k: &KeyEvent) -> KeyEvent {
    let mut parts: Vec<&str> = to.split('+').collect();
    let code = parts.pop().unwrap_or(to);
    let has = |m: &str| parts.contains(&m);
    let key = match code
        .strip_prefix("Key")
        .or_else(|| code.strip_prefix("Digit"))
    {
        Some(c) if c.len() == 1 => Key::Character(c.to_ascii_lowercase()),
        _ => Key::Unidentified,
    };
    KeyEvent {
        key,
        code: Some(code.to_string()),
        pressed: k.pressed,
        repeat: k.repeat,
        modifiers: Modifiers {
            ctrl: has("Ctrl"),
            alt: has("Alt"),
            shift: has("Shift"),
            meta: k.modifiers.meta,
        },
    }
}

/// The skin's `data-hotkey` keys with the part of their node (`None`: the node has no `data-part`), in the
/// package's order.
#[must_use]
pub fn skin_hotkeys(pkg: &Package) -> Vec<(String, Option<String>)> {
    pkg.hotkeys()
        .iter()
        .filter_map(|h| {
            let code = pkg.str(h.code)?;
            let part = pkg
                .template(h.node.0)
                .and_then(|t| t.nodes.get(h.node.1 as usize))
                .and_then(|n| n.part)
                .and_then(|p| pkg.str(p))
                .map(str::to_string);
            Some((chord(code, h.mods), part))
        })
        .collect()
}

impl V2Ui {
    /// The store of the client's options (the classic client's `civ5.options.*`, which the account follows) and
    /// the volume of a knob never saved (0 to 1). Without it the options live in memory for the session.
    pub fn set_client_store(&mut self, store: Box<dyn SettingsStore>, volume_default: f32) {
        self.actions.set_client_store(store, volume_default);
        self.actions.set_skin_hotkeys(skin_hotkeys(&self.skin.pkg));
    }

    /// The volume knobs the options window changed since the last call: `(knob, 0..=1)` with the classic client's
    /// knob names (`MUSIC`, `SFX`, `SPEECH`, `AMBIENCE`).
    pub fn take_volume_changes(&mut self) -> Vec<(&'static str, f32)> {
        self.actions.take_volume_changes()
    }

    /// A key event before the runtime: the capture of a new key, then the player's bindings. `None`: the runtime
    /// takes the event as it is.
    pub(crate) fn key_in(&mut self, k: &KeyEvent, now_ms: f64) -> Option<InputOutcome> {
        let chord = chord_of(k)?;
        if self.actions.capturing_key() {
            if k.pressed && !k.repeat {
                self.actions.capture_key(&chord);
            }
            return Some(InputOutcome::Consumed);
        }
        if self.text_focused {
            return None;
        }
        match self.actions.remap_key(&chord) {
            Remap::Same => None,
            Remap::Drop => Some(InputOutcome::PassToWorld),
            Remap::To(to) => {
                let ev = InputEvent::Key(as_key(&to, k));
                Some(self.skin.runtime.handle_input(&ev, now_ms))
            }
        }
    }
}
