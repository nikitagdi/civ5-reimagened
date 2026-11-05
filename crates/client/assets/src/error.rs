//! Errors for the `assets` crate.

use platform::BlobError;

use crate::{ContentHash, LogicalId, ParseHashError, VfsOverlayId};

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    #[error("asset {0:?} not found in manifest")]
    UnknownAsset(LogicalId),
    #[error("no variant of asset {0:?} matches the device profile")]
    NoVariant(LogicalId),
    #[error("VFS overlay {0:?} is not registered")]
    UnknownOverlay(VfsOverlayId),
    #[error("blob {0} not found in any source")]
    BlobNotFound(ContentHash),
    #[error("blob content does not match hash {0}")]
    HashMismatch(ContentHash),
    #[error("blob {0} is corrupt: the body does not match its hash, also after a second download")]
    Corrupt(ContentHash),
    #[error("network error: {0}")]
    Network(String),
    #[error("blob store error: {0}")]
    Store(#[from] BlobError),
    #[error(transparent)]
    Hash(#[from] ParseHashError),
    #[error("server has no manifest (HTTP {0})")]
    ManifestUnavailable(u16),
    #[error("manifest pointer is not a valid hash: {0}")]
    ManifestPointer(ParseHashError),
    #[error("manifest is not loaded")]
    ManifestNotLoaded,
    #[error("invalid manifest: {0}")]
    ManifestFormat(#[from] asset_format::FormatError),
    #[error("request handle is unknown or already consumed")]
    UnknownHandle,
}
