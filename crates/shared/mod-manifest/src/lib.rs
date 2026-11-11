//! Mod manifest (`mod.toml`), a semver subset, the deterministic dependency resolver, the frozen `ModSet` with its
//! hash, and the `mods.lock` text format. MIT-licensed; no I/O (inputs are `&str`).
//!
//! Spec: `specs/modding/SPEC-mod-manifest.md` in the workspace repository.

#![forbid(unsafe_code)]

mod error;
mod hash;
mod lock;
mod manifest;
mod modset;
mod reader;
mod resolve;
mod semver;
mod ui;

pub use error::{ConflictError, LockError, ManifestError, ModSetError, ResolveError, SemverError};
pub use lock::{LOCK_VERSION, parse_lock, write_lock};
pub use manifest::{
    CAP_NET_CHANNEL, CHANNEL_PAYLOAD_CEILING, Channel, ClientPolicy, Content, CoreSection,
    DEFAULT_C2S_MAX, DEFAULT_RATE_PER_S, DEFAULT_S2C_MAX, DEFAULT_STATE_MAX, LuaUiEntry, Manifest,
    ModId, Order, Part, RustEntry, RustSection, STATE_MAX_CEILING, SyncClass, TICK_HZ_MAX,
};
pub use modset::{ContentHash, ModSet, ModsetHash, ResolvedMod};
pub use resolve::{Installed, Pick, Resolution, STEP_LIMIT, Warning, resolve};
pub use semver::{MAX_COMPONENT, Version, VersionReq};
pub use ui::{
    UI_PANELS_MAX, UI_READS_MAX, UI_SCRIPTS_MAX, UiConflict, UiFallback, UiMode, UiPanel,
    UiSection, ui_conflicts,
};
