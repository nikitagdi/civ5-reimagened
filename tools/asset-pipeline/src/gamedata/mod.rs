//! game-data: the game DB, the en_US localization, the Lua closure of the map script and the
//! gameplay contexts, and the VFS index, as content-addressed blobs
//!; `export` writes the same as a folder.
//!
//! SIMPLIFIED(SIMP-016): the DB is a snapshot taken by the real game, not built from the XML.
//!
//! The reference snapshot (the one the project was tested with) is pinned by sha256 in `gamedata.lock`. The library
//! treats a snapshot that does not match as an error; `civ5-assets` turns that into a warning unless a lock file is
//! named on purpose (`--gamedata-lock`) or `CIV5_GAMEDATA_STRICT=1` is set ([`allow_unpinned_snapshot`]): a snapshot
//! taken from another install (another Lekmod build, other DLC) then goes on with its own hashes, which the export
//! writes into its `gamedata.lock`. `civ5-assets lock-gamedata` writes a lock for a snapshot.

pub mod civ5map;
pub mod db;
mod export;
mod loc;
pub mod lua_closure;
pub mod lua_override;
pub mod map_scripts;
mod sha256;
pub mod vfs_index;

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use asset_format::{LogicalId, PrefetchGroup, Variant, VariantFormat};

use crate::store::{BlobStore, ManifestBuilder, StoreError};

pub use db::Db;
pub use export::{
    EXPORT_GAMEPLAY_DB, EXPORT_LOC_DB, ExportSummary, LOCK_FILE, LuaSummary, MANIFEST_FILE,
    Package, REPLICA_GROUP, REPLICA_TRIGGER, ServerOptions, ServerSummary, check_seal, export,
    export_server, prepare, replica_members, seal,
};
pub use loc::{LOC_TABLE, cut_en_us};
pub use lua_closure::{Closure, ClosureSpec, KNOWN_MISSING};
pub use sha256::{Sha256, sha256_hex};
pub use vfs_index::{INDEX_FILE, INDEX_ID, VfsIndex};

/// Name of the composite member that selects this module in `build --set`.
pub const SET_NAME: &str = "game-data";
pub const GAMEPLAY_DB_ID: &str = "data:gameplay-db";
pub const LOC_EN_US_ID: &str = "data:loc-en_us";
/// Snapshot file names. The gameplay DB is the Debug one: it has `MapScripts` and the
/// `Map_Sizes` rows the Core DB lacks.
pub const GAMEPLAY_DB_FILE: &str = "Civ5DebugDatabase.db";
pub const LOC_DB_FILE: &str = "Localization-Merged.db";
/// Environment fallback for `--db`.
pub const SNAPSHOT_ENV: &str = "CIV5_DB_SNAPSHOT";

/// A snapshot whose hashes differ from the lock goes on with a warning (see the module text).
static ALLOW_UNPINNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Lets a snapshot that does not match the lock go on (with a warning and its own hashes).
pub fn allow_unpinned_snapshot(allow: bool) {
    ALLOW_UNPINNED.store(allow, std::sync::atomic::Ordering::Relaxed);
}

/// The text of a lock file pinning the two snapshot files of `snapshot`.
pub fn lock_text_for(snapshot: &Path) -> Result<String, GameDataError> {
    let g = hash_file(&snapshot.join(GAMEPLAY_DB_FILE))?;
    let l = hash_file(&snapshot.join(LOC_DB_FILE))?;
    Ok(format!(
        "# sha256 of a DB snapshot (hashes only, no game data), written by `civ5-assets lock-gamedata`.\n\
         gameplay_db_sha256 = {g}  # {GAMEPLAY_DB_FILE}\n\
         loc_sha256         = {l}  # {LOC_DB_FILE}\n"
    ))
}

/// Default lock file: next to the crate manifest (hashes only, no game data).
pub fn default_lock_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("gamedata.lock")
}

/// Everything the game-data step wants to say; each variant names the file or table concerned.
#[derive(Debug, thiserror::Error)]
pub enum GameDataError {
    #[error(
        "snapshot file {path} has sha256 {actual}, but {lock} pins {expected}; this is not the \
         pinned snapshot (to adopt a new one, update the lock on purpose)"
    )]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
        lock: String,
    },
    #[error("snapshot file {path} is missing (expected in the --db directory)")]
    MissingSnapshotFile { path: String },
    #[error("lock file {path}: {msg}")]
    Lock { path: String, msg: String },
    #[error("i/o error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("sqlite {op} on {path}: {msg}")]
    Sqlite {
        path: String,
        op: String,
        msg: String,
    },
    #[error("path {path} is not valid for SQLite (not UTF-8 or contains NUL)")]
    BadPath { path: String },
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Store(#[from] StoreError),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> GameDataError + '_ {
    move |source| GameDataError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Pinned sha256 sums of the two snapshot files (`gamedata.lock`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub gameplay_db_sha256: String,
    pub loc_sha256: String,
    /// Where it was read from, for error messages.
    pub path: String,
}

impl Lock {
    /// Lines `key = <64 hex>`; `#` starts a comment. Both keys are required, unknown keys are errors.
    pub fn parse(text: &str, path: &str) -> Result<Self, GameDataError> {
        let bad = |msg: String| GameDataError::Lock {
            path: path.to_owned(),
            msg,
        };
        let (mut gameplay, mut loc) = (None, None);
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| bad(format!("line {}: expected `key = sha256`", n + 1)))?;
            let (key, value) = (key.trim(), value.trim().to_ascii_lowercase());
            // Keys of the lock an export writes next to its files (state of the Lua replacement).
            if key == "lekmod" || key.starts_with("lua_override") {
                continue;
            }
            if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad(format!(
                    "line {}: {key} is not a 64-digit hex sha256",
                    n + 1
                )));
            }
            let slot = match key {
                "gameplay_db_sha256" => &mut gameplay,
                "loc_sha256" => &mut loc,
                other => return Err(bad(format!("line {}: unknown key {other:?}", n + 1))),
            };
            if slot.replace(value).is_some() {
                return Err(bad(format!("line {}: {key} given twice", n + 1)));
            }
        }
        Ok(Self {
            gameplay_db_sha256: gameplay
                .ok_or_else(|| bad("gameplay_db_sha256 is missing".into()))?,
            loc_sha256: loc.ok_or_else(|| bad("loc_sha256 is missing".into()))?,
            path: path.to_owned(),
        })
    }

    pub fn read(path: &Path) -> Result<Self, GameDataError> {
        let text = std::fs::read_to_string(path).map_err(io_err(path))?;
        Self::parse(&text, &path.display().to_string())
    }

    /// The lock to check `snapshot` with: the one at `path`, or, when the snapshot differs from it and
    /// [`allow_unpinned_snapshot`] is on, the snapshot's own hashes (with a warning).
    pub fn for_snapshot(path: &Path, snapshot: &Path) -> Result<Self, GameDataError> {
        let lock = Self::read(path)?;
        if !ALLOW_UNPINNED.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(lock);
        }
        let g = hash_file(&snapshot.join(GAMEPLAY_DB_FILE))?;
        let l = hash_file(&snapshot.join(LOC_DB_FILE))?;
        if g == lock.gameplay_db_sha256 && l == lock.loc_sha256 {
            return Ok(lock);
        }
        tracing::warn!(
            "the DB snapshot in {} is not the reference snapshot pinned by {} (another game version, Lekmod build or \
             DLC set): going on with its own hashes; the result is untested. Pin it with `civ5-assets lock-gamedata`, \
             or set CIV5_GAMEDATA_STRICT=1 to make this an error",
            snapshot.display(),
            lock.path
        );
        Ok(Self {
            gameplay_db_sha256: g,
            loc_sha256: l,
            path: format!("{} (unpinned)", snapshot.display()),
        })
    }
}

/// Streams `path` through sha256; returns the hex sum.
pub fn hash_file(path: &Path) -> Result<String, GameDataError> {
    let mut f = File::open(path).map_err(|e| {
        if e.kind() == io::ErrorKind::NotFound {
            GameDataError::MissingSnapshotFile {
                path: path.display().to_string(),
            }
        } else {
            io_err(path)(e)
        }
    })?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            return Ok(h.finalize_hex());
        }
        h.update(&buf[..n]);
    }
}

/// Fails with a `HashMismatch` naming the file and the pinned sum unless `path` matches `expected`.
pub fn verify_file(path: &Path, expected: &str, lock: &Lock) -> Result<(), GameDataError> {
    let actual = hash_file(path)?;
    if actual != expected {
        return Err(GameDataError::HashMismatch {
            path: path.display().to_string(),
            expected: expected.to_owned(),
            actual,
            lock: lock.path.clone(),
        });
    }
    Ok(())
}

/// What `build_db_blobs` stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbSummary {
    pub gameplay_bytes: u64,
    pub loc_bytes: u64,
    pub loc_rows: u64,
}

/// Prefetch group of the data blobs: fetched up front, like the rest of the b1 sets.
const PREFETCH: PrefetchGroup = PrefetchGroup(1);

/// Verifies both snapshot files in `snapshot` against `lock` and adds `data:gameplay-db` (a byte
/// copy of the DB) and `data:loc-en_us` (the en_US cut) to `store` and `manifest`.
/// Nothing is stored if either hash does not match.
pub fn build_db_blobs(
    snapshot: &Path,
    lock: &Lock,
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
) -> Result<DbSummary, GameDataError> {
    let gameplay_path = snapshot.join(GAMEPLAY_DB_FILE);
    let loc_path = snapshot.join(LOC_DB_FILE);
    verify_file(&gameplay_path, &lock.gameplay_db_sha256, lock)?;
    verify_file(&loc_path, &lock.loc_sha256, lock)?;

    let gameplay = std::fs::read(&gameplay_path).map_err(io_err(&gameplay_path))?;
    // The file could change between the hash and the read; re-check what is actually stored.
    if sha256_hex(&gameplay) != lock.gameplay_db_sha256 {
        return Err(GameDataError::HashMismatch {
            path: gameplay_path.display().to_string(),
            expected: lock.gameplay_db_sha256.clone(),
            actual: sha256_hex(&gameplay),
            lock: lock.path.clone(),
        });
    }

    let tmp = store
        .dir()
        .join(format!(".gamedata-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(io_err(&tmp))?;
    let cut = tmp.join("loc-en_us.db");
    let result = cut_en_us(&loc_path, &cut).and_then(|rows| {
        let bytes = std::fs::read(&cut).map_err(io_err(&cut))?;
        Ok((rows, bytes))
    });
    let _ = std::fs::remove_dir_all(&tmp);
    let (loc_rows, loc) = result?;

    let mut add = |id: &str, bytes: &[u8]| -> Result<u64, GameDataError> {
        let (hash, size) = store.put(bytes)?;
        manifest.add(
            LogicalId::new(id),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PREFETCH,
            },
        );
        Ok(size)
    };
    let gameplay_bytes = add(GAMEPLAY_DB_ID, &gameplay)?;
    let loc_bytes = add(LOC_EN_US_ID, &loc)?;
    Ok(DbSummary {
        gameplay_bytes,
        loc_bytes,
        loc_rows,
    })
}

/// The `game-data` build step: reads `lock_path`, adds the DB blobs to the manifest in `out`
/// (keeping what earlier builds put there) and rewrites the manifest pointer.
pub fn build(snapshot: &Path, lock_path: &Path, out: &Path) -> Result<DbSummary, GameDataError> {
    let lock = Lock::for_snapshot(lock_path, snapshot)?;
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let summary = build_db_blobs(snapshot, &lock, &store, &mut manifest)?;
    manifest.finish(&store)?;
    Ok(summary)
}

/// What `build_package` stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSummary {
    pub db: DbSummary,
    pub lua: LuaSummary,
    /// Disagreements between the closure and the civhost log or the index roots; not errors.
    pub warnings: Vec<String>,
}

/// The whole `game-data` step: the DB blobs, the Lua closure under `game`, `data:vfs-index`. The
/// closure and the snapshot hashes are checked before anything is stored. Entries of earlier
/// builds stay in the manifest.
pub fn build_package(
    game: &Path,
    snapshot: &Path,
    lock_path: &Path,
    out: &Path,
    civhost_log: Option<&Path>,
) -> Result<PackageSummary, GameDataError> {
    let lock = Lock::for_snapshot(lock_path, snapshot)?;
    let pkg = prepare(game, &lock, civhost_log)?;
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let db = build_db_blobs(snapshot, &lock, &store, &mut manifest)?;
    let lua = export::add_to_manifest(&pkg, &store, &mut manifest)?;
    // The group of the client's replica; the group table is read from the manifest and written back.
    crate::world::groups::publish(
        &store,
        &mut manifest,
        vec![crate::world::groups::GroupSpec::map(
            export::REPLICA_GROUP,
            asset_format::Trigger::Preset(export::REPLICA_TRIGGER.to_owned()),
            export::replica_members(&pkg),
        )],
    )
    .map_err(|e| GameDataError::Invalid(format!("group {}: {e}", export::REPLICA_GROUP)))?;
    manifest.finish(&store)?;
    Ok(PackageSummary {
        db,
        lua,
        warnings: pkg.warnings,
    })
}
