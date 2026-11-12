//! UI v2 actions: what the player wants, said by name.
//!
//! A skin (markup, script, hotkey) or the map sends a [`WireIntent`]: a name from the catalogue of the model
//! schema and typed arguments. [`UiActions::apply`] checks its form ([`Intent::from_wire`]), checks that it makes
//! sense in the model the player sees, and carries it out: it changes the local state kept in the `ui` root,
//! asks the [`OrderSink`] to send an order, or refuses with a toast. The module never touches the game or the
//! replica; what the player sees changes only through the model.
//!
//! The `ui` root is written by this module alone (as `Owner::Actions`). [`UiActions`] keeps the typed value
//! ([`UiActions::ui`]) and writes it to the store whole after every change; the store turns that into one
//! delta per frame.

#![forbid(unsafe_code)]

mod intent;
mod modintent;
mod pending;
mod reconcile;
pub mod route;
mod settings;
mod sink;
mod state;
mod ui;
mod validate;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use ui2_model::generated::{RefuseWhy, ToastKind, Ui};
use ui2_model::wire::{ModIntent, Origin, WireIntent};
use ui2_model::{Key, ModelStore, Schema};

pub use intent::{Intent, IntentError};
pub use modintent::MAX_MOD_INTENTS;
pub use pending::ActionEvent;
pub use settings::{MemorySettings, SettingsNs, SettingsStore};
pub use sink::{CameraReq, OrderSink, RecordingSink, Refused, Sent};
pub use state::{SkinId, StateError, StateNs};
pub use ui::{MAX_TOASTS, TOAST_MS, UiState};

use route::Ctx;
use settings::Settings;
use state::{Keep, Leaf};

/// The most distinct `(origin, intent)` pairs remembered for the log-once rule.
const MAX_LOGGED: usize = 512;

/// An intent rejected for its form: a mistake of the skin, not of the player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormError {
    pub origin: Origin,
    pub intent: Arc<str>,
    pub error: IntentError,
}

/// The actions module of one game session.
pub struct UiActions {
    schema: &'static Schema,
    state: UiState,
    /// Form errors already reported, by origin (for markup the node) and intent name.
    logged: HashSet<(Origin, Arc<str>)>,
    form_errors: Vec<FormError>,
    settings: Settings,
    /// state.* of skins and mods that are not mounted now, to be brought back.
    saved: HashMap<Keep, Vec<Leaf>>,
    active_skin: Option<SkinId>,
    /// The last time the host gave (settings are written at most once a second by it).
    clock_ms: u64,
    /// The ids of the mods, by `ModIdx`.
    mods: Vec<Arc<str>>,
    /// Intents of mods waiting for the dispatcher.
    mod_out: Vec<ModIntent>,
}

impl UiActions {
    /// A module for the model `schema`, keeping skin and mod settings in `device`. Nothing is written to a
    /// model store until the first [`apply`](Self::apply) or [`publish`](Self::publish).
    #[must_use]
    pub fn new(schema: &'static Schema, device: Box<dyn SettingsStore>) -> UiActions {
        UiActions {
            schema,
            state: UiState::new(),
            logged: HashSet::new(),
            form_errors: Vec::new(),
            settings: Settings::new(device),
            saved: HashMap::new(),
            active_skin: None,
            clock_ms: 0,
            mods: Vec::new(),
            mod_out: Vec::new(),
        }
    }

    /// The `ui` root as this module holds it: what the source needs to read (the selection, the windows).
    #[must_use]
    pub fn ui(&self) -> &Ui {
        self.state.ui()
    }

    /// Takes a whole `ui` root (the sample mode's `ui` part: an open window, a focus) as the module's own state.
    ///
    /// # Errors
    /// The value is not a `ui` root of the schema; nothing changed.
    pub fn adopt_ui(
        &mut self,
        value: ui2_model::Value,
        store: &mut ModelStore,
    ) -> Result<(), ui2_model::StoreError> {
        self.publish(store);
        store
            .writer(ui2_model::Owner::Actions)
            .set_value("ui", value)?;
        self.state.resync(store);
        Ok(())
    }

    /// Write the `ui` root to the store if it changed (or was never written). The other entry points do this
    /// themselves; call it once at start so the root exists before the first frame.
    pub fn publish(&mut self, store: &mut ModelStore) {
        if let Err(e) = self.state.flush(store) {
            tracing::error!("ui2-actions: the store refused the ui root: {e}");
        }
    }

    /// Carry out the intents of one frame, in order. `intents` is left empty. `store` is read for what the
    /// player sees and written (the `ui` root) for what changes; orders go to `sink`. `now_ms` is the host's
    /// clock.
    pub fn apply(
        &mut self,
        intents: &mut Vec<WireIntent>,
        store: &mut ModelStore,
        sink: &mut dyn OrderSink,
        now_ms: u64,
    ) {
        self.tick(now_ms);
        self.state.expire(now_ms);
        for wire in std::mem::take(intents) {
            if wire.name.starts_with("mods.") {
                if let Err(error) = self.mod_intent(&wire) {
                    self.form_error(&wire, error);
                }
                continue;
            }
            if matches!(wire.origin, Origin::Mod(_)) {
                let error = IntentError::ModCatalog {
                    intent: wire.name.clone(),
                };
                self.form_error(&wire, error);
                continue;
            }
            match Intent::from_wire(self.schema, &wire) {
                Ok(intent) => self.run(&intent, store, sink, now_ms),
                Err(error) => self.form_error(&wire, error),
            }
        }
        self.publish(store);
    }

    /// Tell the module the time (the host's clock, in milliseconds) and write the settings that are due: they
    /// reach the store at most once a second. [`apply`](Self::apply) and [`reconcile`](Self::reconcile) do
    /// this themselves; a host that does neither for a while calls it.
    pub fn tick(&mut self, now_ms: u64) {
        self.clock_ms = now_ms;
        self.settings.flush(now_ms);
    }

    /// The settings of a skin or mod as `(key, JSON text)` pairs, by key: what the script worker is started
    /// with.
    pub fn settings_snapshot(&mut self, ns: &SettingsNs) -> Vec<(String, String)> {
        self.settings.snapshot(ns)
    }

    /// Set a setting to a JSON text, or remove it (`None`). The new value is visible at once; it reaches the
    /// store with the next write that is due (see [`tick`](Self::tick)).
    ///
    /// # Errors
    /// The key or the value breaks a limit (key of 1 to 64 characters of `A-Z a-z 0-9 . _ -`; value JSON of at
    /// most 4096 bytes; at most 64 keys), or the id of the skin or mod cannot be part of a store key. The
    /// change is dropped and logged.
    pub fn settings_set(
        &mut self,
        ns: &SettingsNs,
        key: &str,
        json: Option<&str>,
    ) -> Result<(), StateError> {
        self.settings.set(ns, key, json, self.clock_ms)
    }

    /// The notifications the player dismissed and the source must leave out.
    #[must_use]
    pub fn dismissed(&self) -> &BTreeSet<Key> {
        &self.state.dismissed
    }

    /// The form errors found since the last call, each `(origin, intent)` once. They are also logged.
    pub fn take_form_errors(&mut self) -> Vec<FormError> {
        std::mem::take(&mut self.form_errors)
    }

    fn run(&mut self, intent: &Intent, store: &ModelStore, sink: &mut dyn OrderSink, now_ms: u64) {
        let mut ctx = Ctx {
            ui: &mut self.state,
            model: store,
            sink,
            now_ms,
        };
        match intent.name() {
            "window.open" | "window.close" | "window.toggle" => {
                route::window::run(&mut ctx, intent);
            }
            "unit.select" | "unit.select_next" | "unit.deselect" | "unit.action" | "unit.move"
            | "unit.range_attack" => route::unit::run(&mut ctx, intent),
            "map.click" => route::map::click(&mut ctx, intent),
            "production.choose" | "production.replace" | "production.push" | "production.pop"
            | "production.move" | "production.buy" => route::production::run(&mut ctx, intent),
            "research.choose" => route::research::run(&mut ctx, intent),
            "chat.send" => route::chat::send(&mut ctx, intent),
            "note.dismiss" | "note.activate" => route::notes::run(&mut ctx, intent),
            "turn.primary" | "turn.end" | "turn.cancel" | "screen.open" => {
                route::turn::run(&mut ctx, intent);
            }
            "camera.to_plot" | "camera.to_city" | "camera.to_unit" => {
                route::camera::run(&mut ctx, intent);
            }
            other => {
                // Every intent of the catalogue has a route; a new one that has none is a mistake of this
                // module, and the player is told it did nothing.
                tracing::error!("ui2-actions: intent `{other}` has no route");
                ctx.ui
                    .toast(ToastKind::Refused, RefuseWhy::Other, None, now_ms);
            }
        }
    }

    fn form_error(&mut self, wire: &WireIntent, error: IntentError) {
        if self.logged.len() >= MAX_LOGGED {
            self.logged.clear();
        }
        if !self.logged.insert((wire.origin, wire.name.clone())) {
            return;
        }
        tracing::warn!("ui2-actions: bad intent from {:?}: {error}", wire.origin);
        self.form_errors.push(FormError {
            origin: wire.origin,
            intent: wire.name.clone(),
            error,
        });
    }
}
