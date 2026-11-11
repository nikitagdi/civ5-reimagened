//! The keyboard, in order of precedence:
//!
//! 1. a focused text field takes every key but Esc and Tab (the host's text bridge edits it);
//! 2. Tab and Shift+Tab move the focus ([`focus`](super::focus));
//! 3. Enter and Space click the focused node;
//! 4. Esc closes the top visible screen: `window.close` with the screen's window, origin `Hotkey`;
//! 5. `data-hotkey` of the package: the key clicks its node (origin `Hotkey`) while the node is shown and not
//!    disabled; the topmost layer wins (`window`, `hud`, `map`; never `tooltip`) and a visible screen blocks
//!    the layers below the `window` layer;
//! 6. anything else is the world's.
//!
//! Only presses act; a repeat is taken when its key would act but does nothing. Releases go to the world,
//! except while a text field has the focus.

use platform::{Key, KeyEvent, NamedKey};
use ui2_model::Value as MValue;
use ui2_model::wire::{Origin, WindowId, WireIntent};
use ui2_package::profile::action::Event;
use ui2_package::{Layer, hotkey_mods};

use crate::Runtime;
use crate::arena::NodeId;
use crate::input::InputOutcome;
use crate::input::hit;

/// The rank of a layer for hotkeys: higher wins; `None` never takes keys.
fn rank(l: Layer) -> Option<u8> {
    match l {
        Layer::Map => Some(0),
        Layer::Hud => Some(1),
        Layer::Window => Some(2),
        Layer::Tooltip => None,
    }
}

impl Runtime {
    pub(crate) fn on_key(&mut self, k: &KeyEvent) -> InputOutcome {
        let named = match &k.key {
            Key::Named(n) => Some(*n),
            Key::Character(c) if c == " " => Some(NamedKey::Space),
            _ => None,
        };
        if self.text_focus().is_some() && !matches!(named, Some(NamedKey::Escape | NamedKey::Tab)) {
            return InputOutcome::Consumed;
        }
        if !k.pressed {
            return InputOutcome::PassToWorld;
        }
        if named == Some(NamedKey::Tab) && self.tab(k.modifiers.shift) {
            return InputOutcome::Consumed;
        }
        match named {
            Some(NamedKey::Enter | NamedKey::Space) => {
                if let Some(f) = self.focused() {
                    if !k.repeat {
                        self.dispatch(f, Event::Click, false);
                    }
                    return InputOutcome::Consumed;
                }
            }
            Some(NamedKey::Escape) => {
                if let Some(close) = self.close_top_screen() {
                    if !k.repeat {
                        self.output.intents.push(close);
                    }
                    return InputOutcome::Consumed;
                }
            }
            _ => {}
        }
        if let Some(node) = self.hotkey_target(k) {
            if !k.repeat {
                self.dispatch(node, Event::Click, true);
            }
            return InputOutcome::Consumed;
        }
        InputOutcome::PassToWorld
    }

    /// The intent that closes the top visible screen, when it is a window of the skin.
    fn close_top_screen(&self) -> Option<WireIntent> {
        let top = *self.visible_screens().last()?;
        let tmpl = self.arena.get(top)?.tmpl();
        let screens = &self.input.screens;
        let at = screens.binary_search_by(|(r, _)| r.cmp(&tmpl)).ok()?;
        let id = &screens[at].1;
        WindowId::from_name(id)?;
        Some(WireIntent {
            name: "window.close".into(),
            args: vec![("window".into(), MValue::Str(id.clone()))],
            origin: Origin::Hotkey,
        })
    }

    /// The layer a node is in (the HUD outside layers).
    fn layer_of(&self, id: NodeId) -> Layer {
        let mut at = Some(id);
        while let Some(a) = at {
            if let Some(l) = hit::layer_kind(&self.pkg, &self.arena, a) {
                return l;
            }
            at = self.arena.get(a).and_then(|n| n.parent());
        }
        Layer::Hud
    }

    /// The node a key press clicks, if a `data-hotkey` of the package matches it and its node can take it.
    fn hotkey_target(&self, k: &KeyEvent) -> Option<NodeId> {
        let code = k.code.as_deref()?;
        let m = k.modifiers;
        let mods = (if m.ctrl { hotkey_mods::CTRL } else { 0 })
            | (if m.alt { hotkey_mods::ALT } else { 0 })
            | (if m.shift { hotkey_mods::SHIFT } else { 0 });
        let refs: Vec<_> = self
            .pkg
            .hotkeys()
            .iter()
            .filter(|h| h.mods == mods && self.pkg.str(h.code) == Some(code))
            .map(|h| h.node)
            .collect();
        if refs.is_empty() {
            return None;
        }
        let modal = !self.visible_screens().is_empty();
        let geo = self.geo();
        let mut best: Option<(u8, NodeId)> = None;
        for id in self.arena.preorder() {
            let Some(n) = self.arena.get(id) else {
                continue;
            };
            if !refs.contains(&n.tmpl()) || self.disabled(id) || !geo.shown(id) {
                continue;
            }
            let layer = self.layer_of(id);
            let Some(r) = rank(layer) else {
                continue;
            };
            if modal && layer != Layer::Window {
                continue;
            }
            if best.is_none_or(|(b, _)| r > b) {
                best = Some((r, id));
            }
        }
        best.map(|(_, id)| id)
    }
}
