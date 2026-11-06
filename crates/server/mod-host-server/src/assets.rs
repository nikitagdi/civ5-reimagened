//! The asset pack of an installed mod (T32): `<mods root>/<id>/<id>.pack`, the `mod-assets` container the mod's
//! builder writes next to `mod.toml`. The store finds it at scan time and keeps its SHA-256 (the key of
//! `GET /mods/{hash}` and the `pack` field of `ModView`) and its size; the bytes are read per request.
//!
//! The server does not parse the pack (the client does, with the hash it was told): it checks the magic and the
//! size cap, so a stray file is not advertised.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use mod_assets::{Hasher, PACK_MAGIC, Sha256};
use mod_manifest::ModId;

/// The most a served pack may weigh, bytes. A larger file is not advertised and not served.
pub const MAX_PACK_BYTES: u64 = 256 * 1024 * 1024;

/// A pack found next to an installed mod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackInfo {
    /// SHA-256 of the whole file (`Pack::hash`).
    pub hash: Sha256,
    pub path: PathBuf,
    pub size: u64,
}

impl PackInfo {
    /// The 64 lowercase hex digits clients put in the URL.
    pub fn hex(&self) -> String {
        self.hash.to_string()
    }
}

/// The pack file name of a mod: `<id>.pack`.
pub fn pack_file_name(id: &ModId) -> String {
    format!("{}.pack", id.as_str())
}

/// Looks for `<dir>/<id>.pack`. `Ok(None)`: the mod has no pack. `Err`: the file exists but cannot be used
/// (not a regular file, too big, unreadable, not a pack); the sentence is for the operator's log.
///
/// # Errors
/// See above.
pub fn scan_pack(dir: &Path, id: &ModId) -> Result<Option<PackInfo>, String> {
    let name = pack_file_name(id);
    let path = dir.join(&name);
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {name}: {e}")),
    };
    if !meta.is_file() {
        return Err(format!("{name} is not a regular file"));
    }
    if meta.len() > MAX_PACK_BYTES {
        return Err(format!(
            "{name} is {} bytes, the most is {MAX_PACK_BYTES}",
            meta.len()
        ));
    }
    match hash_file(&path).map_err(|e| format!("cannot read {name}: {e}"))? {
        Some((hash, size)) => Ok(Some(PackInfo { hash, path, size })),
        None => Err(format!("{name} is not an asset pack")),
    }
}

/// Streams the file through SHA-256. `None`: it does not start with the pack magic.
fn hash_file(path: &Path) -> io::Result<Option<(Sha256, u64)>> {
    let mut file = File::open(path)?.take(MAX_PACK_BYTES + 1);
    let mut hasher = Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    let mut head = Vec::with_capacity(PACK_MAGIC.len());
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if head.len() < PACK_MAGIC.len() {
            let take = (PACK_MAGIC.len() - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    if total > MAX_PACK_BYTES {
        return Err(io::Error::from(io::ErrorKind::FileTooLarge));
    }
    if head != PACK_MAGIC {
        return Ok(None);
    }
    Ok(Some((hasher.finish(), total)))
}

/// Reads a pack for serving, re-checking the size recorded at scan time. A file that changed since is refused
/// (`InvalidData`): the advertised hash would no longer match.
///
/// # Errors
/// I/O errors, and a size different from `info.size`.
pub fn read_pack(info: &PackInfo) -> io::Result<Vec<u8>> {
    let changed = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "the pack changed since the scan",
        )
    };
    let file = File::open(&info.path)?;
    if file.metadata()?.len() != info.size {
        return Err(changed());
    }
    let mut buf = Vec::with_capacity(usize::try_from(info.size).unwrap_or(0));
    file.take(info.size + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 != info.size {
        return Err(changed());
    }
    Ok(buf)
}
