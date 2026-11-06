//! The registry of mod code linked into this server binary, and the creation of a game's instances.
//!
//! The distribution build links the server halves of its mods and calls each crate's
//! `pub fn register(r: &mut ServerRegistry)` (see `mod-api`). A game's frozen set (`ModSet`) is then matched
//! against the installed mods and these factories.

use mod_api::{Caps, CapsError, Limiter, ServerRegistry};
use mod_manifest::{ModId, ModSet, Part, Version};
use thiserror::Error;

use crate::instance::{GameMods, ModInstance};
use crate::store::ModStore;

/// Why a game's mods cannot be instantiated. Each variant is a sentence for a person (it reaches the lobby and
/// the log, S4).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InstantiateError {
    #[error("mod `{id}` {version} is not installed on this server")]
    NotInstalled { id: ModId, version: Version },
    #[error(
        "mod `{id}` {version} changed since the game was created (the content on disk differs)"
    )]
    Changed { id: ModId, version: Version },
    #[error("mod `{id}` has a server part, but this server build does not contain its code")]
    NoCode { id: ModId },
    #[error("mod `{id}`: {source}")]
    Caps {
        id: ModId,
        #[source]
        source: CapsError,
    },
}

/// The server halves of the mods this binary contains.
#[derive(Default)]
pub struct HostRegistry {
    inner: ServerRegistry,
}

impl HostRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets a mod crate register itself: `reg.register(hello_server::register)`.
    pub fn register(&mut self, f: impl FnOnce(&mut ServerRegistry)) -> &mut Self {
        f(&mut self.inner);
        self
    }

    /// Registration problems (bad or duplicate ids). The host must look at this after registering.
    pub fn problems(&self) -> &[mod_api::RegistryError] {
        self.inner.problems()
    }

    /// Whether code for the mod is linked in.
    pub fn has(&self, id: &ModId) -> bool {
        self.inner.ids().any(|i| i == id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &ModId> {
        self.inner.ids()
    }

    /// The instances of a game, one per mod with a server half, in load order. Every mod of `set` must be
    /// installed in `store` with the same version and content hash; a mod with a `server-rs` part must have code
    /// here. An empty set gives empty [`GameMods`] without touching the store.
    pub fn instantiate(
        &self,
        store: &ModStore,
        set: &ModSet,
    ) -> Result<GameMods, InstantiateError> {
        store.check_frozen(set)?;
        let mut slots = Vec::with_capacity(set.len());
        for rm in set.mods() {
            let installed = store
                .installed()
                .iter()
                .find(|i| i.manifest.id == rm.id && i.manifest.version == rm.version)
                .ok_or_else(|| InstantiateError::NotInstalled {
                    id: rm.id.clone(),
                    version: rm.version,
                })?;
            let manifest = &installed.manifest;
            if !manifest.has_part(Part::ServerRs) {
                slots.push(None);
                continue;
            }
            let module = self
                .inner
                .create(&rm.id)
                .ok_or_else(|| InstantiateError::NoCode { id: rm.id.clone() })?;
            let caps = Caps::from_manifest(manifest).map_err(|source| InstantiateError::Caps {
                id: rm.id.clone(),
                source,
            })?;
            slots.push(Some(ModInstance::new(
                rm.id.clone(),
                rm.load_idx,
                caps,
                Limiter::from_manifest(manifest),
                module,
            )));
        }
        Ok(GameMods::new(slots))
    }
}
