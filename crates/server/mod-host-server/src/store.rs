//! `modset-store`: the installed mods of a server, and the freezing of a game's mod set.
//!
//! Layout on disk: `<root>/<id>/mod.toml` plus the mod's files. The directory name must equal the manifest id. A
//! game's set is resolved once, when the game is created, from the host's picks (`GameSettings.mods`); the result is
//! frozen (versions, content hashes, load order) and from then on only compared, never re-resolved.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use mod_api::MOD_API_VERSION;
use mod_manifest::{
    ClientPolicy, ContentHash, Installed, LockError, Manifest, ModId, ModSet, ModsetHash, Pick,
    ResolveError, SyncClass, VersionReq, Warning, parse_lock, resolve, write_lock,
};
use protocol::{
    InstalledModView, MAX_MOD_PROBLEM_CHARS, MAX_MODS, ModPick, ModProblem, ModSetView, ModSync,
    ModView, xxh64,
};
use thiserror::Error;

use crate::assets::{PackInfo, scan_pack};
use crate::registry::InstantiateError;

/// The manifest file name inside a mod directory.
pub const MANIFEST_FILE: &str = "mod.toml";
/// A manifest larger than this is rejected before parsing.
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
/// Hashing refuses a mod directory with more files than this (a runaway tree, not a real mod).
const MAX_FILES: usize = 100_000;
/// Hashing refuses a directory tree deeper than this.
const MAX_DEPTH: usize = 32;

const HASH_MAGIC: &[u8; 8] = b"C5MODCNT";
const LANE_SEEDS: [u64; 4] = [
    0x6D6F_6463_6F6E_0001,
    0x6D6F_6463_6F6E_0002,
    0x6D6F_6463_6F6E_0003,
    0x6D6F_6463_6F6E_0004,
];

/// A directory of the mods root that is not usable as a mod, with the reason. Never fatal for the other mods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanProblem {
    /// The directory name under the mods root (empty for the root itself).
    pub dir: String,
    pub message: String,
}

#[derive(Debug, Clone)]
struct Entry {
    id: ModId,
    dir: PathBuf,
    pack: Option<PackInfo>,
}

/// The mods installed on this server, as found by [`ModStore::scan`].
#[derive(Debug, Clone, Default)]
pub struct ModStore {
    entries: Vec<Entry>,
    installed: Vec<Installed>,
    problems: Vec<ScanProblem>,
}

/// A resolved and frozen mod set of one game, and what the resolver noticed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrozenSet {
    pub set: ModSet,
    pub warnings: Vec<String>,
}

impl FrozenSet {
    /// The picks that reproduce this set exactly: every mod of the set (dependencies included), in load order,
    /// pinned to its version (`=1.2.3`). A game stores these in `GameSettings.mods` after the freeze, so the
    /// settings in `meta.json`, `SETG` and the recording header name the resolved versions.
    pub fn exact_picks(&self) -> Vec<ModPick> {
        self.set
            .mods()
            .iter()
            .map(|m| ModPick {
                id: m.id.as_str().to_string(),
                req: format!("={}", m.version),
            })
            .collect()
    }

    /// The `mods.lock` text of the set: versions, load order, content hashes and the set hash. This is what a
    /// game keeps on disk (`meta.json`, the `MODS` section of a save) and checks again when it comes back.
    pub fn lock_text(&self) -> String {
        write_lock(&self.set)
    }

    /// The wire view of the set, as the server's own plan (every mod is `available`).
    pub fn view(&self) -> ModSetView {
        set_view(&self.set)
    }
}

/// Why a game's picks cannot be turned into a set. Each variant reads as a sentence for a person.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FreezeError {
    #[error("{0} mods were picked, the most is {MAX_MODS}")]
    TooManyPicks(usize),
    #[error("pick {index} (`{id}`): {message}")]
    BadPick {
        index: usize,
        id: String,
        message: String,
    },
    #[error("mod `{0}` is picked twice")]
    DuplicatePick(String),
    #[error("{0}")]
    Resolve(#[from] ResolveError),
    #[error("the resolved set has {0} mods, the most is {MAX_MODS}")]
    SetTooLarge(usize),
}

impl FreezeError {
    /// The error as the wire's list of problems (`ModPlan::Err`, `RejectReason::ModMismatch`); the text is cut to
    /// `MAX_MOD_PROBLEM_CHARS` characters.
    pub fn problems(&self) -> Vec<ModProblem> {
        vec![ModProblem {
            text: clamp_chars(&self.to_string(), MAX_MOD_PROBLEM_CHARS),
        }]
    }
}

fn clamp_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

impl ModStore {
    /// An empty store: no mod is installed.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A store over ready-made candidates (tests, and hosts that build `Installed` themselves).
    pub fn from_installed(installed: Vec<Installed>) -> Self {
        Self {
            entries: Vec::new(),
            installed,
            problems: Vec::new(),
        }
    }

    /// Scans `root` for `<id>/mod.toml`. A missing `root` means no mods. A broken directory becomes a
    /// [`ScanProblem`] and the rest still load. Directory order is by name, so the result is deterministic.
    pub fn scan(root: &Path) -> Self {
        let mut store = Self::default();
        let rd = match fs::read_dir(root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return store,
            Err(e) => {
                store.problems.push(ScanProblem {
                    dir: String::new(),
                    message: format!("cannot read the mods directory: {e}"),
                });
                return store;
            }
        };
        let mut dirs: Vec<(String, PathBuf)> = Vec::new();
        for item in rd {
            match item {
                Ok(item) => {
                    // Plain files and symlinks next to the mod directories are not mods.
                    if item.file_type().is_ok_and(|t| t.is_dir()) {
                        dirs.push((item.file_name().to_string_lossy().into_owned(), item.path()));
                    }
                }
                Err(e) => store.problems.push(ScanProblem {
                    dir: String::new(),
                    message: format!("cannot list the mods directory: {e}"),
                }),
            }
        }
        dirs.sort();
        for (name, path) in dirs {
            match load_mod(&name, &path) {
                Ok((installed, pack)) => {
                    store.entries.push(Entry {
                        id: installed.manifest.id.clone(),
                        dir: path,
                        pack,
                    });
                    store.installed.push(installed);
                }
                Err(message) => store.problems.push(ScanProblem { dir: name, message }),
            }
        }
        store
    }

    /// The installed candidates, in directory-name order.
    pub fn installed(&self) -> &[Installed] {
        &self.installed
    }

    /// Directories that were skipped, with the reason.
    pub fn problems(&self) -> &[ScanProblem] {
        &self.problems
    }

    /// The root directory of an installed mod (`None` for stores made by [`from_installed`](Self::from_installed)).
    pub fn dir_of(&self, id: &ModId) -> Option<&Path> {
        self.entries
            .iter()
            .find(|e| &e.id == id)
            .map(|e| e.dir.as_path())
    }

    /// The asset pack of an installed mod, if it has one (`<id>/<id>.pack`, found at scan time).
    pub fn pack_of(&self, id: &ModId) -> Option<&PackInfo> {
        self.entries
            .iter()
            .find(|e| &e.id == id)
            .and_then(|e| e.pack.as_ref())
    }

    /// The pack with this SHA-256 (64 hex digits, any case), for `GET /mods/{hash}`. Unknown: `None`.
    pub fn pack_by_hash(&self, hex: &str) -> Option<&PackInfo> {
        let hex = hex.to_ascii_lowercase();
        self.entries
            .iter()
            .filter_map(|e| e.pack.as_ref())
            .find(|p| p.hex() == hex)
    }

    /// The wire view of a set with the mods' pack hashes filled in (`ModView.pack`).
    pub fn set_view(&self, set: &ModSet) -> ModSetView {
        set_view_with(set, |id| self.pack_of(id).map(PackInfo::hex))
    }

    /// The lobby's list of installed mods (`LobbyChoices.mods`).
    pub fn lobby_views(&self) -> Vec<InstalledModView> {
        self.installed
            .iter()
            .take(MAX_MODS)
            .map(|i| {
                let m = &i.manifest;
                InstalledModView {
                    id: m.id.as_str().to_string(),
                    version: m.version.to_string(),
                    title: m.name.clone(),
                    sync: sync_to_wire(m.sync),
                    client_required: m.client_policy() == ClientPolicy::Required,
                }
            })
            .collect()
    }

    /// Whether every mod of a frozen `set` is installed here with the same version and content hash.
    pub fn check_frozen(&self, set: &ModSet) -> Result<(), InstantiateError> {
        for rm in set.mods() {
            let installed = self
                .installed
                .iter()
                .find(|i| i.manifest.id == rm.id && i.manifest.version == rm.version)
                .ok_or_else(|| InstantiateError::NotInstalled {
                    id: rm.id.clone(),
                    version: rm.version,
                })?;
            if installed.content_hash != rm.content_hash {
                return Err(InstantiateError::Changed {
                    id: rm.id.clone(),
                    version: rm.version,
                });
            }
        }
        Ok(())
    }

    /// A stored game's mods back: parses the stored `mods.lock` text and checks it against the installed mods.
    /// `lock` is `None` for a game without mods; if its settings still list mods, that is an inconsistency
    /// (`NoLock`). `picks_in_settings` is `GameSettings.mods.len()`.
    pub fn restore_set(
        &self,
        lock: Option<&str>,
        picks_in_settings: usize,
    ) -> Result<ModSet, RestoreError> {
        let Some(text) = lock else {
            return if picks_in_settings == 0 {
                Ok(ModSet::default())
            } else {
                Err(RestoreError::NoLock)
            };
        };
        let set = parse_lock(text).map_err(|e: LockError| RestoreError::BadLock(e.to_string()))?;
        self.check_frozen(&set)?;
        Ok(set)
    }

    /// Resolves the host's picks against the installed mods and freezes the result. An empty pick list gives the
    /// empty set (a game without mods).
    pub fn freeze(&self, picks: &[ModPick]) -> Result<FrozenSet, FreezeError> {
        if picks.len() > MAX_MODS {
            return Err(FreezeError::TooManyPicks(picks.len()));
        }
        let mut parsed: Vec<Pick> = Vec::with_capacity(picks.len());
        for (index, p) in picks.iter().enumerate() {
            let bad = |message: String| FreezeError::BadPick {
                index,
                id: clamp_chars(&p.id, 64),
                message,
            };
            let id = ModId::new(&p.id).map_err(|m| bad(format!("invalid mod id: {m}")))?;
            let req = p
                .req
                .parse::<VersionReq>()
                .map_err(|e| bad(e.to_string()))?;
            if parsed.iter().any(|q| q.id == id) {
                return Err(FreezeError::DuplicatePick(id.as_str().to_string()));
            }
            parsed.push(Pick { id, req });
        }
        let res = resolve(&parsed, &self.installed, &MOD_API_VERSION)?;
        if res.set.len() > MAX_MODS {
            return Err(FreezeError::SetTooLarge(res.set.len()));
        }
        Ok(FrozenSet {
            set: res.set,
            warnings: res.warnings.iter().map(Warning::to_string).collect(),
        })
    }
}

/// Why a stored game cannot get its mods back (S4). Reads as a sentence for the player.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RestoreError {
    #[error("the stored mod set is damaged: {0}")]
    BadLock(String),
    #[error("{0}")]
    Mod(#[from] InstantiateError),
    #[error("the game uses mods, but no frozen mod set was stored for it")]
    NoLock,
}

/// The wire view of a set (`GameMsg::Mods`), with `available = true` for every mod and no pack hashes.
/// [`ModStore::set_view`] fills the packs in.
pub fn set_view(set: &ModSet) -> ModSetView {
    set_view_with(set, |_| None)
}

/// [`set_view`] with `pack` asking for the pack hash (hex) of a mod.
pub fn set_view_with(set: &ModSet, pack: impl Fn(&ModId) -> Option<String>) -> ModSetView {
    ModSetView {
        modset_hash: modset_hash64(&set.modset_hash()),
        mods: set
            .mods()
            .iter()
            .map(|m| ModView {
                id: m.id.as_str().to_string(),
                version: m.version.to_string(),
                load_idx: m.load_idx,
                sync: sync_to_wire(m.sync),
                client_required: m.client == ClientPolicy::Required,
                available: true,
                pack: pack(&m.id),
            })
            .collect(),
    }
}

/// The 64-bit form of the set hash that the protocol carries: the first eight bytes, little-endian.
pub fn modset_hash64(h: &ModsetHash) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&h.0[..8]);
    u64::from_le_bytes(b)
}

fn sync_to_wire(s: SyncClass) -> ModSync {
    match s {
        SyncClass::Gameplay => ModSync::Gameplay,
        SyncClass::Cosmetic => ModSync::Cosmetic,
        SyncClass::Client => ModSync::Client,
    }
}

fn load_mod(dir_name: &str, dir: &Path) -> Result<(Installed, Option<PackInfo>), String> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let meta = fs::metadata(&manifest_path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => format!("no {MANIFEST_FILE}"),
        _ => format!("cannot read {MANIFEST_FILE}: {e}"),
    })?;
    if meta.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST_FILE} is {} bytes, the most is {MAX_MANIFEST_BYTES}",
            meta.len()
        ));
    }
    let text = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {MANIFEST_FILE}: {e}"))?;
    let manifest = Manifest::parse(&text).map_err(|e| e.to_string())?;
    if manifest.id.as_str() != dir_name {
        return Err(format!(
            "directory name does not match the mod id `{}`",
            manifest.id
        ));
    }
    let content_hash = hash_mod_dir(dir).map_err(|e| format!("cannot hash the content: {e}"))?;
    let pack = scan_pack(dir, &manifest.id)?;
    Ok((
        Installed {
            manifest,
            content_hash,
        },
        pack,
    ))
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, seed) in LANE_SEEDS.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&xxh64(*seed, bytes).to_le_bytes());
    }
    out
}

/// The content hash of a mod directory: every regular file below it (names starting with `.` and symlinks are
/// skipped), by relative path in byte order, each as path, size and a 256-bit digest of its bytes. Independent of
/// the order the file system lists entries in and of timestamps. Not a security boundary (mods are trusted).
pub fn hash_mod_dir(dir: &Path) -> io::Result<ContentHash> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(dir, "", 0, &mut files)?;
    files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut stream = Vec::with_capacity(12 + files.len() * 64);
    stream.extend_from_slice(HASH_MAGIC);
    stream.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (rel, path) in &files {
        let bytes = fs::read(path)?;
        stream.extend_from_slice(&(rel.len() as u32).to_le_bytes());
        stream.extend_from_slice(rel.as_bytes());
        stream.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        stream.extend_from_slice(&digest(&bytes));
    }
    Ok(ContentHash(digest(&stream)))
}

fn collect(
    dir: &Path,
    prefix: &str,
    depth: usize,
    out: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(io::Error::other("directory tree is too deep"));
    }
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let ty = item.file_type()?;
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if ty.is_dir() {
            collect(&item.path(), &rel, depth + 1, out)?;
        } else if ty.is_file() {
            if out.len() >= MAX_FILES {
                return Err(io::Error::other("too many files"));
            }
            out.push((rel, item.path()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
