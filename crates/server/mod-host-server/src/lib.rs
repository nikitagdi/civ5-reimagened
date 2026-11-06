//! The server-side mod host.
//!
//! * [`store`] (module `modset-store`): scans the installed mods, resolves the host's picks and freezes the
//!   result into a [`mod_manifest::ModSet`] with the wire view the protocol carries.
//!
//! Spec: `specs/modding/SPEC-modset-store.md` in the workspace repository.

#![forbid(unsafe_code)]

pub mod store;

pub use store::{
    FreezeError, FrozenSet, MANIFEST_FILE, ModStore, ScanProblem, hash_mod_dir, modset_hash64,
    set_view,
};
