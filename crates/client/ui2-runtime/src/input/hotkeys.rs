//! The keyboard, in order of precedence:
//!
//! 1. a focused text field takes every key but Esc and Tab (the host's text bridge edits it);
//! 2. Tab and Shift+Tab move the focus ([`focus`](super::focus));
//! 3. a focused control takes its keys ([`controls`](super::controls): arrows, Page Up/Down, Home, End,
//!    Esc of a select); Enter and Space click the focused node;
//! 4. Esc closes the top visible screen: `window.close` with the screen's window, origin `Hotkey`;
//! 5. `data-hotkey` of the package: the key clicks its node (origin `Hotkey`) while the node is shown and not
//!    disabled; the topmost layer wins (`window`, `hud`, `map`; never `tooltip`) and a visible screen blocks
//!    the layers below the `window` layer;
//! 6. anything else is the world's.
//!
//! Only presses act; a repeat is taken when its key would act but does nothing. Releases go to the world,
//! except while a text field has the focus.

use crate::error::{ErrorCode, UiError};
use platform::{Key, KeyEvent, NamedKey};
use std::collections::HashMap;
use ui2_model::Value as MValue;
use ui2_model::wire::{Origin, WindowId, WireIntent};
use ui2_package::profile::action::Event;
use ui2_package::{BindKind, Layer, hotkey_mods};

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
        if k.pressed && self.number_key(k, named) {
            return InputOutcome::Consumed;
        }
        if self.text_focus().is_some() && !matches!(named, Some(NamedKey::Escape | NamedKey::Tab)) {
            return InputOutcome::Consumed;
        }
        if !k.pressed {
            return InputOutcome::PassToWorld;
        }
        if named == Some(NamedKey::Tab) && self.tab(k.modifiers.shift) {
            return InputOutcome::Consumed;
        }
        if let Some(out) = self.control_key(k, named) {
            return out;
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
                if let Some(top) = self.visible_screens().last().copied()
                    && self.has_handler(top, Event::Escape)
                {
                    if !k.repeat {
                        self.dispatch_here(top, Event::Escape);
                    }
                    return InputOutcome::Consumed;
                }
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
        self.input
            .hotkeys
            .iter()
            .find(|(c, m, id)| {
                c == code && *m == mods && !self.disabled(*id) && self.geo().shown(*id)
            })
            .map(|(_, _, id)| *id)
    }

    fn screen_of(&self, id: NodeId) -> Option<NodeId> {
        let mut at = Some(id);
        while let Some(id) = at {
            let n = self.arena.get(id)?;
            if self.input.screens.iter().any(|(t, _)| *t == n.tmpl()) {
                return Some(id);
            }
            at = n.parent();
        }
        None
    }

    pub(crate) fn rebuild_hotkeys(&mut self, store: &ui2_model::ModelStore) {
        let top = self.visible_screens().last().copied();
        // SIMPLIFIED(SIMP-330): rebuild key maps once per frame to include structural and visibility changes.
        let mut winners = HashMap::new();
        let mut entries = Vec::new();
        for id in self.arena.preorder() {
            let Some(n) = self.arena.get(id) else {
                continue;
            };
            if self.disabled(id) || !self.geo().shown(id) {
                continue;
            }
            let Some(layer) = rank(self.layer_of(id)) else {
                continue;
            };
            let screen = self.screen_of(id);
            if top.is_some() && screen != top {
                continue;
            }
            let dynamic = self.binder.hotkeys.get(&id);
            let static_key = self.pkg.hotkeys().iter().find(|h| h.node == n.tmpl());
            let mut chord = dynamic.and_then(|s| parse_hotkey(s));
            if dynamic.is_none()
                && let Some(h) = static_key
            {
                chord = self.pkg.str(h.code).map(|s| (s.to_string(), h.mods));
                if let Some(value) = override_key(&self.pkg, n.tmpl(), store) {
                    chord = value;
                }
            }
            if let Some(value) = dynamic
                && !value.is_empty()
                && chord.is_none()
            {
                self.errors.report(UiError::at(
                    &self.path(id),
                    ErrorCode::Bind,
                    "invalid dynamic hotkey",
                ));
            }
            let Some((code, mods)) = chord else {
                continue;
            };
            if winners.contains_key(&(screen, layer, code.clone(), mods)) {
                self.errors.report(UiError::at(
                    &self.path(id),
                    ErrorCode::Bind,
                    format!("hotkey conflict for {code}, first element wins"),
                ));
                continue;
            }
            winners.insert((screen, layer, code.clone(), mods), id);
            entries.push((layer, code, mods, id));
        }
        entries.sort_by_key(|e| std::cmp::Reverse(e.0));
        self.input.hotkeys = entries
            .into_iter()
            .map(|(_, c, m, id)| (c, m, id))
            .collect();
    }
}

pub(crate) fn parse_hotkey(value: &str) -> Option<(String, u8)> {
    let mut parts: Vec<_> = value.split('+').collect();
    let code = parts.pop()?;
    if !ui2_package::profile::action::is_key_code(code) {
        return None;
    }
    let mut mods = 0;
    let mut last = 0;
    for part in parts {
        let bit = match part {
            "Ctrl" => hotkey_mods::CTRL,
            "Alt" => hotkey_mods::ALT,
            "Shift" => hotkey_mods::SHIFT,
            _ => return None,
        };
        if bit <= last {
            return None;
        }
        last = bit;
        mods |= bit;
    }
    Some((code.to_string(), mods))
}

/// An absent map entry preserves the static chord. An empty string disables it.
pub(crate) fn override_key(
    pkg: &crate::Pkg,
    tmpl: ui2_package::NodeRef,
    store: &ui2_model::ModelStore,
) -> Option<Option<(String, u8)>> {
    let name = pkg
        .bindings_of(tmpl.0, tmpl.1)
        .iter()
        .find_map(|b| match &b.kind {
            BindKind::Event {
                ev: Event::Click,
                actions,
            } => actions.iter().find_map(|a| match a {
                ui2_package::profile::action::Action::Intent { name, .. }
                | ui2_package::profile::action::Action::Call { name, .. } => Some(name),
                _ => None,
            }),
            _ => None,
        })?;
    let field = store.schema().resolve(&format!("ui.keys.{name}")).ok()?;
    match store.value(field.id, &[])? {
        ui2_model::Value::Str(s) => Some(parse_hotkey(s)),
        _ => None,
    }
}
