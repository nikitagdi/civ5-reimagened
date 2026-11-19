//! The hotkeys the player binds in the options window of UI v2.
//!
//! A row is one key that something binds, by its default key: the `data-hotkey` keys of the running skin (the
//! package's static hotkeys, named after their part) and the game's own command keys (the unit actions the HUD
//! binds through `data-bind-hotkey`, from the game menu screen's `hotkeys`). Binding a row to another key is a
//! remap in front of the skin's runtime ([`Remap`]): a press of the new key reaches the runtime as the default key,
//! and a press of the default key of a row that moved does not reach it. Keys are written
//! `[Ctrl+][Alt+][Shift+]<KeyboardEvent.code>`, as the runtime's `data-hotkey` and the HUD's catalogue write them.
//!
//! The bindings that differ from the defaults are stored in the classic client's options store, one key
//! `civ5.options.UI2Hotkeys` (`F6=KeyT,Ctrl+KeyM=KeyQ`): it is kept on the device and follows the player's account
//! like the other options (the skin settings of UI v2 are not kept yet).
//!
//! Enter, Escape, Tab and Space are fixed: they also confirm, close and move the focus, which a remap would take from
//! every window. A key can belong to one row only: a binding that would share a key is refused and the clash is shown.
//!
//! SIMPLIFIED(SIMP-392): a key is remapped for everything that binds it (all rows of one default key move
//! together); the hotkey labels the skin draws (`data-hotkey-label`) still show the default key; keys the world
//! takes (camera, the map) are not checked for clashes.

use std::collections::BTreeMap;
use std::sync::Arc;

use ui2_model::generated::{GameMenu, RefuseWhy, UiHotkey};
use ui2_model::{KeyedList, Text};

use crate::UiActions;
use crate::settings::SettingsStore;

/// The key of the bindings in the options store (after `civ5.options.`).
pub const STORE_KEY: &str = "UI2Hotkeys";
/// The longest stored text (a value of the account store, `protocol::account::MAX_VALUE_CHARS`).
pub const MAX_STORED: usize = 256;
/// The keys no binding may move or take.
pub const FIXED: &[&str] = &["Enter", "NumpadEnter", "Escape", "Tab", "Space"];
/// Codes of the modifier keys: a press of one alone waits for the key.
const MODIFIERS: &[&str] = &[
    "ShiftLeft",
    "ShiftRight",
    "ControlLeft",
    "ControlRight",
    "AltLeft",
    "AltRight",
    "MetaLeft",
    "MetaRight",
    "OSLeft",
    "OSRight",
];

/// What becomes of a key press in front of the skin's runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Remap {
    /// It goes on as it is.
    Same,
    /// It goes on as this key (a binding moved here).
    To(String),
    /// Its binding moved away: the runtime does not get it (the world does).
    Drop,
}

/// The `KeyboardEvent.code` of a key.
fn code_of(chord: &str) -> &str {
    chord.rsplit('+').next().unwrap_or(chord)
}

fn is_fixed(chord: &str) -> bool {
    FIXED.contains(&code_of(chord))
}

/// A key as the runtime reads one: modifiers in the order Ctrl, Alt, Shift, then a code of letters and digits.
#[must_use]
pub fn valid(chord: &str) -> bool {
    let mut parts: Vec<&str> = chord.split('+').collect();
    let Some(code) = parts.pop() else {
        return false;
    };
    if code.is_empty()
        || !code.bytes().all(|c| c.is_ascii_alphanumeric())
        || MODIFIERS.contains(&code)
    {
        return false;
    }
    let mut last = 0;
    for p in parts {
        let rank = match p {
            "Ctrl" => 1,
            "Alt" => 2,
            "Shift" => 3,
            _ => return false,
        };
        if rank <= last {
            return false;
        }
        last = rank;
    }
    true
}

/// The short form of a key: `Ctrl+T` for `Ctrl+KeyT`, `1` for `Digit1`.
#[must_use]
pub fn label(chord: &str) -> String {
    let code = code_of(chord);
    let short = code
        .strip_prefix("Key")
        .or_else(|| code.strip_prefix("Digit"))
        .or_else(|| code.strip_prefix("Arrow"))
        .filter(|s| !s.is_empty())
        .unwrap_or(code);
    match chord.rfind('+') {
        Some(at) => format!("{}+{short}", &chord[..at]),
        None => short.to_string(),
    }
}

#[derive(Default)]
pub(crate) struct Hotkeys {
    /// The skin's `data-hotkey` keys and their parts.
    skin: Vec<(String, Option<String>)>,
    /// The game's command keys and their names, as the game menu screen last gave them.
    game: Vec<(String, Text)>,
    /// The bindings in force: default key to the player's key (only those that differ).
    committed: BTreeMap<String, String>,
    /// The window's bindings.
    draft: BTreeMap<String, String>,
    /// The row that waits for a key press.
    pub(crate) capture: Option<String>,
    /// The last key refused because another row has it (or it is fixed).
    pub(crate) clash: Option<String>,
}

impl Hotkeys {
    /// The bindings stored; a pair that is not two keys (or moves a fixed key) is left out.
    pub(crate) fn load(store: &dyn SettingsStore) -> Hotkeys {
        let mut committed = BTreeMap::new();
        if let Some(text) = store.get(&format!("{}{STORE_KEY}", crate::options::KEY_PREFIX)) {
            for pair in text.split(',') {
                if let Some((from, to)) = pair.split_once('=')
                    && valid(from)
                    && valid(to)
                    && !is_fixed(from)
                    && !is_fixed(to)
                    && from != to
                    && !committed.values().any(|v| v == to)
                {
                    committed.insert(from.to_string(), to.to_string());
                }
            }
        }
        Hotkeys {
            draft: committed.clone(),
            committed,
            ..Hotkeys::default()
        }
    }

    pub(crate) fn set_skin(&mut self, keys: Vec<(String, Option<String>)>) {
        self.skin = keys;
    }

    /// Every row's default key, skin first.
    fn ids(&self) -> impl Iterator<Item = &str> {
        self.skin
            .iter()
            .map(|(k, _)| k.as_str())
            .chain(self.game.iter().map(|(k, _)| k.as_str()))
    }

    fn effective<'a>(&'a self, id: &'a str) -> &'a str {
        self.draft.get(id).map_or(id, String::as_str)
    }

    /// The rows of `ui.hotkeys`: one per default key, the skin's first.
    pub(crate) fn rows(&self) -> KeyedList<UiHotkey> {
        let mut rows: Vec<UiHotkey> = Vec::new();
        let mut at: BTreeMap<&str, usize> = BTreeMap::new();
        let skin = self.skin.iter().map(|(k, part)| {
            let name = match part {
                Some(p) => Text::key(&format!("hotkeys.{p}"), &[]),
                None => Text::key("hotkeys.other", &[]),
            };
            (k.as_str(), "skin", name)
        });
        let game = self
            .game
            .iter()
            .map(|(k, name)| (k.as_str(), "game", name.clone()));
        for (id, group, name) in skin.chain(game) {
            if let Some(&i) = at.get(id) {
                rows[i].more += 1;
                continue;
            }
            at.insert(id, rows.len());
            let key = self.effective(id);
            rows.push(UiHotkey {
                id: Arc::from(id),
                key: Arc::from(key),
                label: Arc::from(label(key).as_str()),
                group: Arc::from(group),
                name,
                more: 0,
                fixed: is_fixed(id),
            });
        }
        // One row per default key.
        KeyedList::from_vec(rows).unwrap_or_default()
    }

    /// The window opens: the game's command keys of the game menu screen, the drafts are the bindings in force.
    pub(crate) fn edit(&mut self, menu: Option<&GameMenu>) {
        if let Some(menu) = menu {
            self.game = menu
                .hotkeys
                .iter()
                .flat_map(|g| g.actions.iter())
                .filter(|a| !a.hidden)
                .filter_map(|a| {
                    let key = a.default.as_deref().filter(|k| valid(k))?;
                    Some((key.to_string(), a.name.clone()))
                })
                .collect();
        }
        self.cancel();
    }

    pub(crate) fn start_capture(&mut self, id: &str) {
        self.clash = None;
        self.capture = (!id.is_empty() && !is_fixed(id) && self.ids().any(|i| i == id))
            .then(|| id.to_string());
    }

    /// A key press while a row waits: it becomes the row's key, unless it is fixed or another row has it. Escape
    /// stops waiting; a modifier alone waits on. `false`: no row waits.
    pub(crate) fn capture_key(&mut self, chord: &str) -> bool {
        let Some(id) = self.capture.clone() else {
            return false;
        };
        if MODIFIERS.contains(&code_of(chord)) {
            return true;
        }
        self.capture = None;
        if chord == "Escape" {
            return true;
        }
        if !valid(chord)
            || is_fixed(chord)
            || self.ids().any(|o| o != id && self.effective(o) == chord)
        {
            self.clash = Some(chord.to_string());
            return true;
        }
        self.clash = None;
        if chord == id {
            self.draft.remove(&id);
        } else {
            self.draft.insert(id, chord.to_string());
        }
        true
    }

    /// A row gets its default key back, unless another row has it now.
    pub(crate) fn clear(&mut self, id: &str) -> Result<(), RefuseWhy> {
        if !self.draft.contains_key(id) {
            return Ok(());
        }
        if self.ids().any(|o| o != id && self.effective(o) == id) {
            self.clash = Some(id.to_string());
            return Ok(());
        }
        self.clash = None;
        self.draft.remove(id);
        Ok(())
    }

    pub(crate) fn reset(&mut self) {
        self.draft.clear();
        self.capture = None;
        self.clash = None;
    }

    pub(crate) fn cancel(&mut self) {
        self.draft = self.committed.clone();
        self.capture = None;
        self.clash = None;
    }

    pub(crate) fn commit(&mut self) {
        self.committed = self.draft.clone();
        self.capture = None;
        self.clash = None;
    }

    /// The store text of the drafts: `Some(None)` when every key is its default, `None` when they do not fit.
    pub(crate) fn encode_draft(&self) -> Option<Option<String>> {
        if self.draft.is_empty() {
            return Some(None);
        }
        let text = self
            .draft
            .iter()
            .map(|(from, to)| format!("{from}={to}"))
            .collect::<Vec<_>>()
            .join(",");
        (text.len() <= MAX_STORED).then_some(Some(text))
    }

    pub(crate) fn remap(&self, chord: &str) -> Remap {
        if let Some((from, _)) = self.committed.iter().find(|(_, to)| *to == chord) {
            return Remap::To(from.clone());
        }
        if self.committed.contains_key(chord) {
            return Remap::Drop;
        }
        Remap::Same
    }
}

impl UiActions {
    /// A row of the hotkeys waits for a key press ([`capture_key`](Self::capture_key) takes the next one).
    #[must_use]
    pub fn capturing_key(&self) -> bool {
        self.client.hotkeys.capture.is_some()
    }

    /// A key press (`[Ctrl+][Alt+][Shift+]<code>`) while a row waits for one: it becomes the row's key in the
    /// window's drafts, or is refused as a clash. `false`: no row waits, the press is not taken.
    pub fn capture_key(&mut self, chord: &str) -> bool {
        let taken = self.client.hotkeys.capture_key(chord);
        if taken {
            self.show_options();
        }
        taken
    }

    /// What a key press becomes with the player's bindings in force.
    #[must_use]
    pub fn remap_key(&self, chord: &str) -> Remap {
        self.client.hotkeys.remap(chord)
    }

    /// The bindings in force that differ from the defaults: default key to the player's key.
    #[must_use]
    pub fn hotkey_bindings(&self) -> Vec<(String, String)> {
        self.client
            .hotkeys
            .committed
            .iter()
            .map(|(a, b)| (a.clone(), b.clone()))
            .collect()
    }
}
