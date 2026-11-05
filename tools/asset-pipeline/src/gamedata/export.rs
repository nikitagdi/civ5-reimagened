//! The Lua and index part of the game-data package, and the folder export for tests
//!.

use std::fs;
use std::path::{Path, PathBuf};

use asset_format::{LogicalId, Variant, VariantFormat};

use super::lua_closure::{self, Closure, ClosureSpec};
use super::lua_override::{self, Applied, LuaOverride};
use super::vfs_index::{INDEX_FILE, INDEX_ID, IndexMap, VfsIndex, lua_id};
use super::{
    GAMEPLAY_DB_FILE, GAMEPLAY_DB_ID, GameDataError, LOC_DB_FILE, LOC_EN_US_ID, Lock, PREFETCH,
    cut_en_us, io_err, verify_file,
};
use super::{civ5map, map_scripts};
use crate::store::{BlobStore, ManifestBuilder};

/// Names of the files in the exported folder.
pub const EXPORT_GAMEPLAY_DB: &str = "db/gameplay.db";
pub const EXPORT_LOC_DB: &str = "db/loc-en_us.db";

/// The closure with its bytes and the index made from it.
pub struct Package {
    pub closure: Closure,
    pub index: VfsIndex,
    pub index_json: Vec<u8>,
    /// (normalised path, bytes), in path order.
    pub lua: Vec<(String, Vec<u8>)>,
    /// Closure and civhost-log disagreements; the build goes on.
    pub warnings: Vec<String>,
}

impl Package {
    pub fn lua_bytes(&self) -> u64 {
        self.lua.iter().map(|(_, b)| b.len() as u64).sum()
    }
}

/// Reads the closure of the b1 pack from `game` and makes the index. Writes nothing.
pub fn prepare(
    game: &Path,
    lock: &Lock,
    civhost_log: Option<&Path>,
) -> Result<Package, GameDataError> {
    let spec = ClosureSpec::lekmod_b1(game)?;
    prepare_with(game, lock, &spec, civhost_log)
}

pub fn prepare_with(
    game: &Path,
    lock: &Lock,
    spec: &ClosureSpec,
    civhost_log: Option<&Path>,
) -> Result<Package, GameDataError> {
    let closure = lua_closure::compute(game, spec)?;
    package_of(game, lock, closure, civhost_log)
}

/// Reads the files of `closure` and makes the index.
fn package_of(
    game: &Path,
    lock: &Lock,
    closure: Closure,
    civhost_log: Option<&Path>,
) -> Result<Package, GameDataError> {
    let dlc = lua_closure::find_lekmod_dlc(game).unwrap_or_default();
    // Every accepted spelling of the v34.15 folder (`LEKMODv34_15`, ...) records the same version.
    let lekmod = if crate::game::source::is_lekmod_dir(&dlc) {
        "v34.15".to_owned()
    } else {
        dlc.split_once('_')
            .map_or(dlc.as_str(), |(_, v)| v)
            .to_owned()
    };
    let mut lua = Vec::with_capacity(closure.files.len());
    for (path, f) in &closure.files {
        let bytes = fs::read(&f.abs).map_err(io_err(&f.abs))?;
        if bytes.len() as u64 != f.size {
            return Err(GameDataError::Invalid(format!(
                "{} changed while it was being read",
                f.abs.display()
            )));
        }
        lua.push((path.clone(), bytes));
    }
    let index = VfsIndex::from_closure(&closure, lock, &lekmod);
    index.check()?;
    let index_json = index.to_json()?;
    let mut warnings = closure.warnings.clone();
    if let Some(log) = civhost_log {
        let text = fs::read_to_string(log).map_err(io_err(log))?;
        warnings.extend(lua_closure::check_civhost_log(&text, &closure));
    }
    Ok(Package {
        closure,
        index,
        index_json,
        lua,
        warnings,
    })
}

/// The load group of the client's replica: both databases, the VFS index and the Lua files of the
/// gameplay contexts. Map scripts are the server's; the replica loads games, it does not make maps. The client asks
/// for it by name when the original UI is on, so it is a `Map`-class group with a trigger that no preset sets.
pub const REPLICA_GROUP: &str = "gc:replica";
/// The trigger of [`REPLICA_GROUP`] (asked for by name by the original-UI path).
pub const REPLICA_TRIGGER: &str = "original-ui";

/// The members of [`REPLICA_GROUP`] for `pkg`.
pub fn replica_members(pkg: &Package) -> Vec<LogicalId> {
    let mut m = vec![
        LogicalId::new(GAMEPLAY_DB_ID),
        LogicalId::new(LOC_EN_US_ID),
        LogicalId::new(INDEX_ID),
    ];
    m.extend(
        pkg.closure
            .context_files
            .iter()
            .map(|p| LogicalId::new(lua_id(p))),
    );
    m
}

/// What `add_to_manifest` stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaSummary {
    pub files: usize,
    pub lua_bytes: u64,
    pub index_bytes: u64,
}

/// Adds `lua:<path>` for every file and `data:vfs-index`; the index is checked against the
/// manifest being built (both DBs must already be in it).
pub fn add_to_manifest(
    pkg: &Package,
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
) -> Result<LuaSummary, GameDataError> {
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
    let mut lua_bytes = 0;
    for (path, bytes) in &pkg.lua {
        lua_bytes += add(&lua_id(path), bytes)?;
    }
    let index_bytes = add(INDEX_ID, &pkg.index_json)?;
    pkg.index.check_against(manifest.manifest())?;
    Ok(LuaSummary {
        files: pkg.lua.len(),
        lua_bytes,
        index_bytes,
    })
}

/// What `export` wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSummary {
    pub out: PathBuf,
    pub files: usize,
    pub lua_files: usize,
    pub total_bytes: u64,
    pub gameplay_bytes: u64,
    pub loc_bytes: u64,
    pub loc_rows: u64,
}

/// Empties `out` (absent, empty or an earlier export) and makes `out/db`.
fn reset_out(out: &Path) -> Result<(), GameDataError> {
    if out.exists() {
        let empty = fs::read_dir(out).map_err(io_err(out))?.next().is_none();
        if !empty && !out.join(INDEX_FILE).is_file() {
            return Err(GameDataError::Invalid(format!(
                "{} is not empty and holds no {INDEX_FILE}: refusing to replace it",
                out.display()
            )));
        }
        fs::remove_dir_all(out).map_err(io_err(out))?;
    }
    fs::create_dir_all(out.join("db")).map_err(io_err(out))
}

/// Writes the package as a folder: `vfs-index.json`, `db/gameplay.db`, `db/loc-en_us.db` and the
/// Lua files by the paths of the index. The snapshot is checked against `lock` first. `out` must
/// not exist, be empty or hold an earlier export (it is replaced).
pub fn export(
    game: &Path,
    snapshot: &Path,
    lock_path: &Path,
    out: &Path,
    civhost_log: Option<&Path>,
) -> Result<(ExportSummary, Vec<String>), GameDataError> {
    let lock = Lock::for_snapshot(lock_path, snapshot)?;
    let gameplay_src = snapshot.join(GAMEPLAY_DB_FILE);
    let loc_src = snapshot.join(LOC_DB_FILE);
    verify_file(&gameplay_src, &lock.gameplay_db_sha256, &lock)?;
    verify_file(&loc_src, &lock.loc_sha256, &lock)?;
    let pkg = prepare(game, &lock, civhost_log)?;

    reset_out(out)?;

    let gameplay = fs::read(&gameplay_src).map_err(io_err(&gameplay_src))?;
    if super::sha256_hex(&gameplay) != lock.gameplay_db_sha256 {
        return Err(GameDataError::HashMismatch {
            path: gameplay_src.display().to_string(),
            expected: lock.gameplay_db_sha256.clone(),
            actual: super::sha256_hex(&gameplay),
            lock: lock.path.clone(),
        });
    }
    let write = |rel: &str, bytes: &[u8]| -> Result<(), GameDataError> {
        let p = out.join(rel);
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        fs::write(&p, bytes).map_err(io_err(&p))
    };
    write(EXPORT_GAMEPLAY_DB, &gameplay)?;
    let loc_dest = out.join(EXPORT_LOC_DB);
    let loc_rows = cut_en_us(&loc_src, &loc_dest)?;
    let loc_bytes = fs::metadata(&loc_dest).map_err(io_err(&loc_dest))?.len();
    write(INDEX_FILE, &pkg.index_json)?;
    for (path, bytes) in &pkg.lua {
        write(path, bytes)?;
    }
    let summary = ExportSummary {
        out: out.to_owned(),
        files: pkg.lua.len() + 3,
        lua_files: pkg.lua.len(),
        total_bytes: pkg.lua_bytes()
            + pkg.index_json.len() as u64
            + gameplay.len() as u64
            + loc_bytes,
        gameplay_bytes: gameplay.len() as u64,
        loc_bytes,
        loc_rows,
    };
    Ok((summary, pkg.warnings))
}

// ---------------------------------------------------------------------------------------------
// Server package
// ---------------------------------------------------------------------------------------------

/// Pins of the export: snapshot hashes and the state of the Lua replacement.
pub const LOCK_FILE: &str = "gamedata.lock";
/// `sha256sum` list of every other file of the export; written last.
pub const MANIFEST_FILE: &str = "manifest.sha256";

/// What the server export does besides the b1 export.
#[derive(Debug, Clone, Default)]
pub struct ServerOptions<'a> {
    pub civhost_log: Option<&'a Path>,
    /// Lua files read from elsewhere (FIX-0004 first, unless it is switched off).
    pub overrides: Vec<LuaOverride>,
    /// FIX-0004 is among `overrides`; written to the lock as `lua_override = on`.
    pub fix_0004: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSummary {
    pub export: ExportSummary,
    pub map_scripts: usize,
    pub maps: usize,
    pub map_bytes: u64,
    pub applied: Vec<Applied>,
    /// Files listed in `manifest.sha256`.
    pub sealed: usize,
}

/// Writes the server package: the b1 export for all map scripts of the DB with the include
/// closure of the world-builder entry, the `.Civ5Map` files under `maps/` (index section
/// `civ5_maps`), the Lua replacements, `gamedata.lock` and, last, `manifest.sha256`.
pub fn export_server(
    game: &Path,
    snapshot: &Path,
    lock_path: &Path,
    out: &Path,
    opts: &ServerOptions<'_>,
) -> Result<(ServerSummary, Vec<String>), GameDataError> {
    let lock = Lock::for_snapshot(lock_path, snapshot)?;
    let gameplay_src = snapshot.join(GAMEPLAY_DB_FILE);
    let loc_src = snapshot.join(LOC_DB_FILE);
    verify_file(&gameplay_src, &lock.gameplay_db_sha256, &lock)?;
    verify_file(&loc_src, &lock.loc_sha256, &lock)?;

    let scripts = map_scripts::read(&gameplay_src)?;
    let spec = ClosureSpec::lekmod_b2(game, scripts)?;
    let mut closure = lua_closure::compute(game, &spec)?;
    let applied = lua_override::apply(&mut closure, &opts.overrides)?;
    let maps = civ5map::find(game)?;
    let mut pkg = package_of(game, &lock, closure, opts.civhost_log)?;
    pkg.index.civ5_maps = maps
        .iter()
        .map(|m| IndexMap {
            path: m.data_path(),
            size: m.size,
        })
        .collect();
    pkg.index.check()?;
    pkg.index_json = pkg.index.to_json()?;

    reset_out(out)?;
    let gameplay = fs::read(&gameplay_src).map_err(io_err(&gameplay_src))?;
    if super::sha256_hex(&gameplay) != lock.gameplay_db_sha256 {
        return Err(GameDataError::HashMismatch {
            path: gameplay_src.display().to_string(),
            expected: lock.gameplay_db_sha256.clone(),
            actual: super::sha256_hex(&gameplay),
            lock: lock.path.clone(),
        });
    }
    let write = |rel: &str, bytes: &[u8]| -> Result<(), GameDataError> {
        let p = out.join(rel);
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(io_err(dir))?;
        }
        fs::write(&p, bytes).map_err(io_err(&p))
    };
    write(EXPORT_GAMEPLAY_DB, &gameplay)?;
    let loc_dest = out.join(EXPORT_LOC_DB);
    let loc_rows = cut_en_us(&loc_src, &loc_dest)?;
    let loc_bytes = fs::metadata(&loc_dest).map_err(io_err(&loc_dest))?.len();
    write(INDEX_FILE, &pkg.index_json)?;
    for (path, bytes) in &pkg.lua {
        write(path, bytes)?;
    }
    let mut map_bytes = 0;
    for m in &maps {
        let bytes = fs::read(&m.abs).map_err(io_err(&m.abs))?;
        map_bytes += bytes.len() as u64;
        write(&m.data_path(), &bytes)?;
    }
    write(
        LOCK_FILE,
        lock_text(&lock, &pkg.index.snapshot.lekmod, opts, &applied).as_bytes(),
    )?;
    let sealed = seal(out)?;

    let summary = ServerSummary {
        export: ExportSummary {
            out: out.to_owned(),
            files: pkg.lua.len() + maps.len() + 5,
            lua_files: pkg.lua.len(),
            total_bytes: pkg.lua_bytes()
                + pkg.index_json.len() as u64
                + gameplay.len() as u64
                + loc_bytes
                + map_bytes,
            gameplay_bytes: gameplay.len() as u64,
            loc_bytes,
            loc_rows,
        },
        map_scripts: pkg.index.map_scripts.len(),
        maps: maps.len(),
        map_bytes,
        applied,
        sealed,
    };
    Ok((summary, pkg.warnings))
}

/// Text of the export's `gamedata.lock`: the pinned snapshot sums and the Lua replacement state.
/// `Lock::parse` reads it back (it skips the keys it does not use).
fn lock_text(lock: &Lock, lekmod: &str, opts: &ServerOptions<'_>, applied: &[Applied]) -> String {
    let mut s = String::from("# gamedata.lock of this export (civ5-assets export-gamedata)\n");
    s.push_str(&format!(
        "gameplay_db_sha256 = {}\n",
        lock.gameplay_db_sha256
    ));
    s.push_str(&format!("loc_sha256 = {}\n", lock.loc_sha256));
    s.push_str(&format!("lekmod = {lekmod}\n"));
    s.push_str(&format!(
        "lua_override = {}\n",
        if opts.fix_0004 { "on" } else { "off" }
    ));
    for a in applied {
        s.push_str(&format!("lua_override_file = {}\n", a.path));
        s.push_str(&format!("lua_override_sha256 = {}\n", a.replacement_sha256));
        s.push_str(&format!(
            "lua_override_installed_sha256 = {}\n",
            a.installed_sha256
        ));
    }
    s
}

/// Paths (from `dir`, `/`) of all files under `dir` except the seal itself, in byte order.
fn sealed_paths(dir: &Path) -> Result<Vec<String>, GameDataError> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), GameDataError> {
        for e in fs::read_dir(dir).map_err(io_err(dir))? {
            let e = e.map_err(io_err(dir))?;
            let name = e.file_name().to_string_lossy().into_owned();
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if e.file_type().map_err(io_err(&e.path()))?.is_dir() {
                walk(&e.path(), &child, out)?;
            } else {
                out.push(child);
            }
        }
        Ok(())
    }
    let mut v = Vec::new();
    walk(dir, "", &mut v)?;
    v.retain(|p| p != MANIFEST_FILE);
    v.sort();
    Ok(v)
}

fn seal_text(dir: &Path) -> Result<(String, usize), GameDataError> {
    let paths = sealed_paths(dir)?;
    let mut text = String::new();
    for p in &paths {
        text.push_str(&format!("{}  {p}\n", super::hash_file(&dir.join(p))?));
    }
    Ok((text, paths.len()))
}

/// Writes `manifest.sha256` (`sha256sum` format, sorted by path) for every file under `dir`;
/// returns the number of files. Run again after a file is added (`map-scripts.json`).
pub fn seal(dir: &Path) -> Result<usize, GameDataError> {
    let (text, n) = seal_text(dir)?;
    let p = dir.join(MANIFEST_FILE);
    fs::write(&p, text).map_err(io_err(&p))?;
    Ok(n)
}

/// Compares `manifest.sha256` with the files; the error lists changed, missing and unlisted files.
pub fn check_seal(dir: &Path) -> Result<usize, GameDataError> {
    let p = dir.join(MANIFEST_FILE);
    let have = fs::read_to_string(&p).map_err(io_err(&p))?;
    let (want, n) = seal_text(dir)?;
    if have == want {
        return Ok(n);
    }
    let parse = |t: &str| -> std::collections::BTreeMap<String, String> {
        t.lines()
            .filter_map(|l| l.split_once("  "))
            .map(|(h, f)| (f.to_owned(), h.to_owned()))
            .collect()
    };
    let (have, want) = (parse(&have), parse(&want));
    let mut problems = Vec::new();
    for (f, h) in &have {
        match want.get(f) {
            None => problems.push(format!("{f}: listed but missing")),
            Some(w) if w != h => problems.push(format!("{f}: changed")),
            _ => {}
        }
    }
    for f in want.keys().filter(|f| !have.contains_key(*f)) {
        problems.push(format!("{f}: not listed"));
    }
    Err(GameDataError::Invalid(format!(
        "{MANIFEST_FILE} does not match {}:\n  {}",
        dir.display(),
        problems.join("\n  ")
    )))
}
