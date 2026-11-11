//! Intents of mods: `mods.<id>.<name>`. They are not in the catalogue of the schema; the module only checks that
//! they come from the mod they name and passes them on, unchanged, to the mod's Rust half.

use std::sync::Arc;

use ui2_model::wire::{ModIdx, ModIntent, Origin, WireIntent};

use crate::UiActions;
use crate::intent::IntentError;

/// The most mod intents kept between two [`UiActions::take_mod_intents`] calls; later ones are dropped.
pub const MAX_MOD_INTENTS: usize = 256;

/// The prefix of the intents of mods.
const PREFIX: &str = "mods.";

/// A mod id as the manifest allows it: `[a-z][a-z0-9-]{0,31}`.
fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && id.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// An intent name inside a mod: `[a-z][a-z0-9_]*`.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl UiActions {
    /// Tell the module which mod each [`ModIdx`] is (the ids of the mod table of the host, in index order), so
    /// it can tell whose intent an origin is. Call it when the mod set of the game is known.
    pub fn set_mods(&mut self, ids: Vec<Arc<str>>) {
        self.mods = ids;
    }

    /// The intents `mods.<id>.<name>` of the frames since the last call, in order, for the mod dispatcher.
    pub fn take_mod_intents(&mut self) -> Vec<ModIntent> {
        std::mem::take(&mut self.mod_out)
    }

    /// Check an intent with the prefix `mods.` and queue it for the mod it names.
    pub(crate) fn mod_intent(&mut self, wire: &WireIntent) -> Result<(), IntentError> {
        let bad = |why: &str| IntentError::ModIntent {
            intent: wire.name.clone(),
            why: why.to_string(),
        };
        let rest = wire.name.strip_prefix(PREFIX).unwrap_or_default();
        let (id, name) = rest
            .split_once('.')
            .filter(|(id, name)| valid_id(id) && valid_name(name))
            .ok_or_else(|| bad("expected `mods.<id>.<name>`"))?;
        let Origin::Mod(ModIdx(idx)) = wire.origin else {
            return Err(bad("only the mod it names may send it"));
        };
        if self.mods.get(usize::from(idx)).map(|m| &**m) != Some(id) {
            return Err(bad("only the mod it names may send it"));
        }
        if self.mod_out.len() >= MAX_MOD_INTENTS {
            tracing::warn!(
                "ui2-actions: too many mod intents in one frame, `{}` dropped",
                wire.name
            );
            return Ok(());
        }
        self.mod_out.push(ModIntent {
            mod_id: Arc::from(id),
            name: Arc::from(name),
            args: wire.args.clone(),
        });
        Ok(())
    }
}
