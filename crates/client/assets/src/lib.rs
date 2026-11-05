//! Client assets (architecture §5.7, §6.5): VFS, manifest, blob sources, streaming queue.
//!
//! Fetches are pollable ([`FetchOp`]), not `async`, to stay single-threaded on the web (D8).
//! This crate does not hash content; verifying bytes against a [`ContentHash`] is the caller's job.

mod error;
mod groups;
mod http;
mod loader;
mod source;
mod streamer;
mod ui_vfs;
mod vfs;

pub use asset_format::{
    ContentHash, GROUP_TABLE_ID, GroupTable, LogicalId, Manifest, ParseHashError, PlatformProfile,
    PrefetchGroup, PriorityClass, Variant, VariantFormat,
};
pub use error::AssetError;
pub use groups::{GroupError, GroupScheduler, NeedOutcome};
pub use http::{HttpSource, verify_stats};
pub use loader::{AssetHandle, AssetLoader, AssetPoll, ManifestStatus};
pub use source::{BlobSource, FetchOp, Layered, StoreSource};
pub use streamer::{Priority, Streamer};
pub use ui_vfs::{UiFiles, decode_texts};
pub use vfs::{Vfs, VfsOverlayId, normalize_path};
