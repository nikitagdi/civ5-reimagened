//! The client's options as the options window of UI v2 edits them: the classic client's `civ5.options.<Key>` values
//! of the page's key-value store (the classic `OptionsManager` keeps them there and the account sync carries them to
//! the server), the hotkeys of the skin and the game ([`crate::hotkeys`]), drafts until the player accepts.
//!
//! The catalogue is the classic options menu's game, interface and audio options with the defaults of the classic
//! client (`exe_api::options::OPTIONS` and `exe_api::m4_wog`). An option takes effect here when UI v2 does what the
//! classic client does with it: the four volume knobs drive the audio buses
//! ([`UiActions::take_volume_changes`]), "multiplayer auto end turn" ends the turn ([`crate::auto_end`]). Every other
//! option is shown with the reason it does not apply and cannot be changed here; its stored value is left alone.
//!
//! SIMPLIFIED(SIMP-392): of the classic options, UI v2 applies the volumes and the multiplayer auto end turn; the
//! options the classic client's Lua screens read (automatic unit cycle, tile recommendations, civilian yields, reward
//! popups, citizen warning, basic help, policy information, score lists, tooltip delays) are shown as not available
//! in this interface, and the ones no client applies (worker automation, autosaves, mail turns, map inertia and zoom,
//! quick selection advance, single player auto end) say why.

use std::collections::BTreeMap;
use std::sync::Arc;

use ui2_model::generated::{GameMenu, RefuseWhy, ToastKind, UiOption};
use ui2_model::{KeyedList, ModelStore, Text};

use crate::UiActions;
use crate::hotkeys::Hotkeys;
use crate::intent::Intent;
use crate::settings::{MemorySettings, SettingsStore};

/// The prefix of the classic client's options in the store (`exe_api::options::KEY_PREFIX`).
pub const KEY_PREFIX: &str = "civ5.options.";
/// The volume knobs of the classic options menu, in the order of `exe_api::m4_wog::KNOBS`.
pub const KNOBS: [&str; 4] = ["MUSIC", "SFX", "SPEECH", "AMBIENCE"];

/// How an option is edited and stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptKind {
    /// A switch, stored `1` / `0`.
    Toggle,
    /// A whole number, stored as decimal text.
    Number { min: i32, max: i32, step: i32 },
    /// A volume knob: shown in percent, stored as a fraction from 0 to 1 (`Volume.<KNOB>`).
    Volume,
}

/// One option of the catalogue.
#[derive(Clone, Copy, Debug)]
pub struct OptDef {
    /// The key in the store without [`KEY_PREFIX`].
    pub key: &'static str,
    /// The tab of the window: `general` or `interface`.
    pub tab: &'static str,
    pub kind: OptKind,
    /// The classic client's default, as a value of this module (a volume: the page's default replaces it).
    pub default: i32,
    /// Why UI v2 does not apply it: the last part of the skin's text key `options.why.<why>`. `None`: it applies.
    pub why: Option<&'static str>,
}

const fn t(
    key: &'static str,
    tab: &'static str,
    default: i32,
    why: Option<&'static str>,
) -> OptDef {
    OptDef {
        key,
        tab,
        kind: OptKind::Toggle,
        default,
        why,
    }
}

const fn n(
    key: &'static str,
    tab: &'static str,
    default: i32,
    max: i32,
    step: i32,
    why: &'static str,
) -> OptDef {
    OptDef {
        key,
        tab,
        kind: OptKind::Number { min: 0, max, step },
        default,
        why: Some(why),
    }
}

const fn v(key: &'static str) -> OptDef {
    OptDef {
        key,
        tab: "general",
        kind: OptKind::Volume,
        default: 100,
        why: None,
    }
}

/// Not in this interface yet (the classic client's Lua screens read it).
const UI: Option<&str> = Some("interface");

/// The options of the window, in the order it shows them.
pub const CATALOGUE: &[OptDef] = &[
    t("MultiplayerAutoEndTurn", "general", 0, None),
    t(
        "SinglePlayerAutoEndTurn",
        "general",
        0,
        Some("single_player"),
    ),
    t("AutoUnitCycle", "general", 1, UI),
    t("QuickSelectionAdvance", "general", 1, Some("client")),
    t("AutoWorkersDontReplace", "general", 0, Some("server")),
    t(
        "AutoWorkersDontRemoveFeatures",
        "general",
        0,
        Some("server"),
    ),
    t("NoRewardPopups", "general", 0, UI),
    t("NoTileRecommendations", "general", 0, UI),
    t("CivilianYields", "general", 0, UI),
    n("TurnsBetweenAutosave", "general", 1, 100, 1, "autosave"),
    n("NumAutosavesKept", "general", 10, 100, 1, "autosave"),
    t("TurnNotifyEmail", "general", 0, Some("email")),
    v("Volume.MUSIC"),
    v("Volume.SFX"),
    v("Volume.SPEECH"),
    v("Volume.AMBIENCE"),
    t("PolicyInfo", "interface", 0, UI),
    t("ScoreList", "interface", 0, UI),
    t("MPScoreList", "interface", 1, UI),
    t("NoCitizenWarning", "interface", 0, UI),
    t("NoBasicHelp", "interface", 0, UI),
    n("Tooltip1Seconds", "interface", 150, 5000, 50, "interface"),
    n("Tooltip2Seconds", "interface", 500, 5000, 50, "interface"),
    t("EnableMapInertia", "interface", 1, Some("camera")),
    t("StraightZoom", "interface", 0, Some("camera")),
];

/// The option of a key.
#[must_use]
pub fn def(key: &str) -> Option<&'static OptDef> {
    CATALOGUE.iter().find(|d| d.key == key)
}

/// The value of an option's store text; `None` for text that is not one.
#[must_use]
pub fn parse(kind: OptKind, text: &str) -> Option<i32> {
    let text = text.trim();
    match kind {
        OptKind::Toggle => {
            let n: f64 = text.parse().ok()?;
            Some(i32::from(n != 0.0))
        }
        OptKind::Number { min, max, .. } => {
            let n: f64 = text.parse().ok()?;
            Some((n.round() as i32).clamp(min, max))
        }
        OptKind::Volume => {
            let x: f32 = text.parse().ok()?;
            Some((x.clamp(0.0, 1.0) * 100.0).round() as i32)
        }
    }
}

/// The store text of a value, as the classic client writes it.
#[must_use]
pub fn text_of(kind: OptKind, value: i32) -> String {
    match kind {
        OptKind::Toggle => u8::from(value != 0).to_string(),
        OptKind::Number { .. } => value.to_string(),
        OptKind::Volume => (value.clamp(0, 100) as f32 / 100.0).to_string(),
    }
}

fn clamp(kind: OptKind, value: i32) -> i32 {
    match kind {
        OptKind::Toggle => i32::from(value != 0),
        OptKind::Number { min, max, .. } => value.clamp(min, max),
        OptKind::Volume => value.clamp(0, 100),
    }
}

/// The options of the session: the store, the drafts of the window and the hotkeys.
pub(crate) struct ClientOptions {
    store: Box<dyn SettingsStore>,
    /// The volume of a knob never saved, in percent (the page's default, as the classic client's).
    volume_default: i32,
    /// The window's drafts, by key; empty while it is not being edited.
    drafts: BTreeMap<&'static str, i32>,
    pub(crate) hotkeys: Hotkeys,
    /// Volumes changed since the host last took them (a draft is heard at once, as the classic slider is).
    volumes: Vec<(&'static str, f32)>,
}

impl Default for ClientOptions {
    fn default() -> Self {
        ClientOptions::new(Box::new(MemorySettings::default()), 1.0)
    }
}

impl ClientOptions {
    pub(crate) fn new(store: Box<dyn SettingsStore>, volume_default: f32) -> ClientOptions {
        let hotkeys = Hotkeys::load(&*store);
        ClientOptions {
            store,
            volume_default: (volume_default.clamp(0.0, 1.0) * 100.0).round() as i32,
            drafts: BTreeMap::new(),
            hotkeys,
            volumes: Vec::new(),
        }
    }

    fn default_of(&self, d: &OptDef) -> i32 {
        match d.kind {
            OptKind::Volume => self.volume_default,
            _ => d.default,
        }
    }

    /// The stored value of an option (its default when nothing or nothing readable is stored).
    pub(crate) fn stored(&self, d: &OptDef) -> i32 {
        self.store
            .get(&format!("{KEY_PREFIX}{}", d.key))
            .and_then(|t| parse(d.kind, &t))
            .unwrap_or_else(|| self.default_of(d))
    }

    /// The value in force (stored) of a key of the catalogue.
    pub(crate) fn committed(&self, key: &str) -> Option<i32> {
        def(key).map(|d| self.stored(d))
    }

    fn now(&self, d: &OptDef) -> i32 {
        self.drafts
            .get(d.key)
            .copied()
            .unwrap_or_else(|| self.stored(d))
    }

    /// The rows of `ui.options`: the drafts while the window edits them, else the stored values.
    pub(crate) fn rows(&self) -> KeyedList<UiOption> {
        let rows = CATALOGUE
            .iter()
            .map(|d| {
                let (min, max, step, toggle) = match d.kind {
                    OptKind::Toggle => (0, 1, 1, true),
                    OptKind::Number { min, max, step } => (min, max, step, false),
                    OptKind::Volume => (0, 100, 5, false),
                };
                UiOption {
                    key: Arc::from(d.key),
                    tab: Arc::from(d.tab),
                    name: Text::key(&format!("options.{}", d.key), &[]),
                    toggle,
                    value: self.now(d),
                    min,
                    max,
                    step,
                    available: d.why.is_none(),
                    why: d.why.map(|w| Text::key(&format!("options.why.{w}"), &[])),
                }
            })
            .collect();
        // The keys of the catalogue are distinct.
        KeyedList::from_vec(rows).unwrap_or_default()
    }

    /// The window opens: the drafts are the stored values.
    pub(crate) fn edit(&mut self, game_menu: Option<&GameMenu>) {
        // Drafts the window left (it was closed without an answer) are dropped, their volumes too.
        self.cancel();
        self.hotkeys.edit(game_menu);
    }

    /// Change a draft. `Err`: the option is not in the catalogue or does not apply here.
    pub(crate) fn set(&mut self, key: &str, value: i32) -> Result<(), RefuseWhy> {
        let d = def(key).ok_or(RefuseWhy::Invalid)?;
        if d.why.is_some() {
            return Err(RefuseWhy::OptionUnavailable);
        }
        let value = clamp(d.kind, value);
        self.drafts.insert(d.key, value);
        if d.kind == OptKind::Volume {
            self.hear(d.key, value);
        }
        Ok(())
    }

    fn hear(&mut self, key: &'static str, percent: i32) {
        let knob = key.strip_prefix("Volume.").unwrap_or(key);
        if let Some(k) = KNOBS.iter().find(|k| **k == knob) {
            self.volumes.retain(|(o, _)| o != k);
            self.volumes.push((k, percent as f32 / 100.0));
        }
    }

    /// Store the drafts that differ from the stored values and the hotkeys. `Err`: the hotkeys do not fit the store
    /// (nothing was stored).
    pub(crate) fn accept(&mut self) -> Result<(), RefuseWhy> {
        let keys = self.hotkeys.encode_draft().ok_or(RefuseWhy::TooBig)?;
        for (key, value) in std::mem::take(&mut self.drafts) {
            // What is not stored is the default: a draft equal to it is not written.
            if let Some(d) = def(key)
                && self.stored(d) != value
            {
                self.store
                    .set(&format!("{KEY_PREFIX}{key}"), &text_of(d.kind, value));
            }
        }
        let store_key = format!("{KEY_PREFIX}{}", crate::hotkeys::STORE_KEY);
        match keys {
            Some(text) => self.store.set(&store_key, &text),
            None => {
                if self.store.get(&store_key).is_some() {
                    self.store.remove(&store_key);
                }
            }
        }
        self.hotkeys.commit();
        Ok(())
    }

    /// Drop the drafts; the volumes go back to the stored ones.
    pub(crate) fn cancel(&mut self) {
        let drafts = std::mem::take(&mut self.drafts);
        for key in drafts.keys() {
            if let Some(d) = def(key)
                && d.kind == OptKind::Volume
            {
                let stored = self.stored(d);
                self.hear(d.key, stored);
            }
        }
        self.hotkeys.cancel();
    }

    /// Every draft (that can change here) its default, and every hotkey its default key.
    pub(crate) fn reset(&mut self) {
        for d in CATALOGUE.iter().filter(|d| d.why.is_none()) {
            let value = self.default_of(d);
            self.drafts.insert(d.key, value);
            if d.kind == OptKind::Volume {
                self.hear(d.key, value);
            }
        }
        self.hotkeys.reset();
    }

    pub(crate) fn take_volumes(&mut self) -> Vec<(&'static str, f32)> {
        std::mem::take(&mut self.volumes)
    }
}

impl UiActions {
    /// Where the client's options live (the page's store; the app wraps the classic client's account-synced store)
    /// and the volume of a knob never saved (0 to 1, the page's default). The hotkeys are read again from it.
    pub fn set_client_store(&mut self, store: Box<dyn SettingsStore>, volume_default: f32) {
        self.client = ClientOptions::new(store, volume_default);
        self.client
            .hotkeys
            .set_skin(std::mem::take(&mut self.skin_keys));
        self.show_options();
    }

    /// The value in force of a client option (`MultiplayerAutoEndTurn`, `Volume.MUSIC` in percent ...): `None` for
    /// a key that is not in the catalogue.
    #[must_use]
    pub fn client_option(&self, key: &str) -> Option<i32> {
        self.client.committed(key)
    }

    /// The volume knobs the options window changed since the last call: `(knob, 0..=1)`, knobs as the classic
    /// client names them (`MUSIC`, `SFX`, `SPEECH`, `AMBIENCE`). A draft is heard at once and a cancel brings the
    /// stored volume back, as the classic menu's sliders do.
    pub fn take_volume_changes(&mut self) -> Vec<(&'static str, f32)> {
        self.client.take_volumes()
    }

    /// The `data-hotkey` keys of the running skin: `(key, part)` pairs, the key as `[Ctrl+][Alt+][Shift+]<code>`.
    pub fn set_skin_hotkeys(&mut self, keys: Vec<(String, Option<String>)>) {
        self.skin_keys.clone_from(&keys);
        self.client.hotkeys.set_skin(keys);
        self.show_options();
    }

    /// Write the rows of the options window to the `ui` root.
    pub(crate) fn show_options(&mut self) {
        let rows = self.client.rows();
        let keys = self.client.hotkeys.rows();
        let capture = self.client.hotkeys.capture.as_deref().map(Arc::from);
        let clash = self.client.hotkeys.clash.as_deref().map(Arc::from);
        let ui = self.state.ui();
        if ui.options != rows
            || ui.hotkeys != keys
            || ui.hotkey_capture != capture
            || ui.hotkey_clash != clash
        {
            let ui = self.state.edit();
            ui.options = rows;
            ui.hotkeys = keys;
            ui.hotkey_capture = capture;
            ui.hotkey_clash = clash;
        }
    }

    /// `options.*` and `hotkey.*`.
    pub(crate) fn options_intent(&mut self, intent: &Intent, store: &ModelStore, now_ms: u64) {
        let refused = match intent.name() {
            "options.edit" => {
                let menu = store.get::<Option<GameMenu>>().and_then(Option::as_ref);
                self.client.edit(menu);
                Ok(())
            }
            "options.set" => match (intent.text("key"), intent.int("value")) {
                (Some(key), Some(value)) => self.client.set(key, value),
                _ => Err(RefuseWhy::Invalid),
            },
            "options.accept" => self.client.accept(),
            "options.cancel" => {
                self.client.cancel();
                Ok(())
            }
            "options.reset" => {
                self.client.reset();
                Ok(())
            }
            "hotkey.capture" => {
                self.client
                    .hotkeys
                    .start_capture(intent.text("id").unwrap_or(""));
                Ok(())
            }
            "hotkey.clear" => self.client.hotkeys.clear(intent.text("id").unwrap_or("")),
            _ => Ok(()),
        };
        if let Err(why) = refused {
            self.state.toast(ToastKind::Refused, why, None, now_ms);
        }
        self.show_options();
    }
}
