//! The server-side mod host.
//!
//! * [`store`] (module `modset-store`): scans the installed mods, resolves the host's picks and freezes the
//!   result into a [`mod_manifest::ModSet`] with the wire view the protocol carries.
//! * [`registry`]: the mod code linked into the server binary; creates a game's instances from a frozen set.
//! * [`instance`]: the mods of one game behind the D7 checks (size, rate, panic isolation, output filter).
//! * [`limits`]: counts repeated violations of a seat (the disconnect signal).
//!
//! Specs: `specs/modding/SPEC-modset-store.md`, `SPEC-mod-host-server.md` in the workspace repository.

#![forbid(unsafe_code)]

pub mod instance;
pub mod limits;
pub mod registry;
pub mod store;

#[cfg(test)]
mod tests;

pub use instance::{
    CallEnv, ClientOutcome, DropReason, GameMods, LogLine, ModInstance, ModOutput, ModStatus,
    PANIC_ISOLATION,
};
pub use limits::{MAX_STRIKES, STRIKE_WINDOW_MS, Strikes};
pub use registry::{HostRegistry, InstantiateError};
pub use store::{
    FreezeError, FrozenSet, MANIFEST_FILE, ModStore, ScanProblem, hash_mod_dir, modset_hash64,
    set_view,
};
