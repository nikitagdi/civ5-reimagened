//! Mod asset packs: the manifest of a pack, its byte codec, sha256, and verification against `assets.lock`.
//! MIT-licensed; no I/O, the caller hands in bytes. Spec: `specs/modding/SPEC-mod-assets.md` in the workspace
//! repository.
//!
//! A pack carries a mod's textures, sounds and raw files under logical ids. The server serves it by hash
//! (`/mods/{hash}`), the client loads and checks it. `load` is the whole client-side check in one call:
//!
//! ```
//! use mod_assets::{AssetKind, AssetsLock, NewFile, Pack, PackError, Source, Sha256, load};
//! use mod_manifest::ModId;
//!
//! let src = Source { name: "game.jar".into(), sha256: Sha256::of(b"jar") };
//! let pack = Pack::build(
//!     ModId::new("hello").unwrap(),
//!     vec![src],
//!     vec![NewFile { id: "textures/dirt".into(), kind: AssetKind::TextureRgba8 { width: 1, height: 1, mips: 1 }, bytes: vec![9; 4] }],
//! ).unwrap();
//! let bytes = pack.to_bytes();
//! let lock = AssetsLock::of_pack(&pack);
//!
//! let loaded = load(Some(&bytes), Some(&pack.hash()), Some(&lock)).unwrap();
//! assert_eq!(loaded.get("textures/dirt"), Some(&[9u8; 4][..]));
//! assert_eq!(load(None, None, None), Err(PackError::Missing));
//! ```

#![forbid(unsafe_code)]

mod error;
mod lock;
mod pack;
mod sha256;

pub use error::{LockProblem, PackError, VerifyError};
pub use lock::{AssetsLock, verify_against_lock};
pub use pack::{
    AssetKind, MAX_FILES, MAX_ID_LEN, MAX_MANIFEST_BYTES, MAX_MIPS, MAX_TEXTURE_SIDE, NewFile,
    PACK_MAGIC, Pack, PackFile, PackManifest, Source, valid_asset_id,
};
pub use sha256::{Hasher, ParseSha256Error, Sha256};

/// Opens a pack the way a host does. `bytes` is `None` when no pack could be found: that is
/// [`PackError::Missing`], the signal for an optional mod to disable itself and a required one to fail with a clear
/// message. With `expected_hash` (from the mod set view) the whole blob is checked first; with `lock` the pack
/// must match `assets.lock` exactly.
pub fn load(
    bytes: Option<&[u8]>,
    expected_hash: Option<&Sha256>,
    lock: Option<&AssetsLock>,
) -> Result<Pack, PackError> {
    let bytes = bytes.ok_or(PackError::Missing)?;
    let pack = match expected_hash {
        Some(h) => Pack::parse_expecting(bytes, h)?,
        None => Pack::parse(bytes)?,
    };
    if let Some(lock) = lock {
        verify_against_lock(&pack, lock)?;
    }
    Ok(pack)
}
