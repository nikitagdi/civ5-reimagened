//! UI v2 host side: the skins of the build and one running UI v2.
//!
//! - [`SkinRegistry`]: the index `ui2/skins.json`, the checks of a downloaded package against it (hash, id, model
//!   paths), and the fallback after a failure (the default skin, else the classic UI; a failed skin is not tried
//!   again in the session).
//! - [`V2Ui`]: the view model store, the replica source, and the skin's runtime, text engine and scene, with the
//!   frame in the host's order. It does not own the replica (the game session does and lends a view per frame)
//!   and touches no GPU: the application gives [`V2Ui::scene_mut`] to the renderer after each frame.
//! - [`NetSink`]: the actions module's order sink over the network client, for one frame ([`HostNet`]).
//!
//! Load failures are values ([`SkinLoadError`]), never panics, so the application can fall back.

#![forbid(unsafe_code)]

pub mod error;
pub mod registry;
pub mod sink;
pub mod v2;

pub use error::{LoadFailure, SkinLoadError};
pub use registry::{ApiVersion, Fallback, RegistryError, SkinEntry, SkinRegistry, hash_hex};
pub use sink::{HostNet, NetSink, Offline, refused};
pub use ui2_runtime::{Env, Services};
pub use ui2_source::ReplicaView;
pub use v2::{HostClock, LoadStats, RESOLVE_BUDGET_MS, V2Frame, V2FrameReport, V2Init, V2Ui};
