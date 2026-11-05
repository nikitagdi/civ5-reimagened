//! Replacement of Lua files of the install in the server package.
//!
//! FIX-0004 (`Lekmod_policies.lua`: the Consulates handler must not remove itself after the first
//! player) is on by default: the file of the Lekmod repository goes in place of the installed one.
//! `--no-lua-override` turns it off (regression against the installed game). The state is written
//! to `gamedata.lock` of the export.

use std::fs;
use std::path::{Path, PathBuf};

use super::lua_closure::Closure;
use super::{GameDataError, sha256_hex};

/// File replaced by FIX-0004.
pub const FIX_0004_FILE: &str = "Lekmod_policies.lua";
/// Where FIX-0004's file is in the Lekmod repository.
pub const FIX_0004_REPO_PATH: &str = "LEKMOD/Lua/Lekmod_policies.lua";
/// Environment fallback for `--lekmod-repo`.
pub const LEKMOD_REPO_ENV: &str = "LEKMOD_REPO";

/// One replacement: the closure file with this name is read from `source` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaOverride {
    /// File name, any case (`Lekmod_policies.lua`).
    pub file: String,
    pub source: PathBuf,
}

/// What `apply` did to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// Normalised path in the index.
    pub path: String,
    pub source: PathBuf,
    pub installed_sha256: String,
    pub replacement_sha256: String,
    pub size: u64,
}

impl Applied {
    /// The replacement is byte-identical to the installed file.
    pub fn is_noop(&self) -> bool {
        self.installed_sha256 == self.replacement_sha256
    }
}

fn invalid(m: String) -> GameDataError {
    GameDataError::Invalid(m)
}

/// `FILE=PATH` of `--lua-override`.
pub fn parse_arg(arg: &str) -> Result<LuaOverride, GameDataError> {
    let (file, source) = arg
        .split_once('=')
        .filter(|(f, s)| !f.trim().is_empty() && !s.trim().is_empty())
        .ok_or_else(|| invalid(format!("--lua-override {arg:?}: expected FILE=PATH")))?;
    if file.contains(['/', '\\']) {
        return Err(invalid(format!(
            "--lua-override {arg:?}: FILE is a file name without folders"
        )));
    }
    Ok(LuaOverride {
        file: file.trim().to_owned(),
        source: PathBuf::from(source.trim()),
    })
}

/// The Lekmod repository: `--lekmod-repo`, then `LEKMOD_REPO`, then `LEKMOD_ROOT` (the gamecore checkout of
/// `cargo xtask gamecore`), then a `Lekmod` folder next to an ancestor of this crate.
pub fn find_lekmod_repo(flag: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = flag {
        return Some(p.to_owned());
    }
    for var in [LEKMOD_REPO_ENV, "LEKMOD_ROOT"] {
        if let Some(p) = std::env::var_os(var) {
            return Some(PathBuf::from(p));
        }
    }
    let start = Path::new(env!("CARGO_MANIFEST_DIR"));
    start
        .ancestors()
        .map(|a| a.join("Lekmod"))
        .find(|c| c.join(FIX_0004_REPO_PATH).is_file())
}

/// The default FIX-0004 override.
pub fn fix_0004(repo: Option<&Path>) -> Result<LuaOverride, GameDataError> {
    let repo = find_lekmod_repo(repo).ok_or_else(|| {
        invalid(format!(
            "FIX-0004 is on by default but the Lekmod repository is not found: pass --lekmod-repo <dir> \
             or set {LEKMOD_REPO_ENV} (or --no-lua-override to export the installed file)"
        ))
    })?;
    let source = repo.join(FIX_0004_REPO_PATH);
    if !source.is_file() {
        return Err(invalid(format!(
            "FIX-0004 file {} is not found (Lekmod repository {})",
            source.display(),
            repo.display()
        )));
    }
    Ok(LuaOverride {
        file: FIX_0004_FILE.to_owned(),
        source,
    })
}

/// Points the matching file of `closure` at the replacement (its size becomes the replacement's).
/// Each override must match exactly one file of the closure.
pub fn apply(
    closure: &mut Closure,
    overrides: &[LuaOverride],
) -> Result<Vec<Applied>, GameDataError> {
    let mut done = Vec::new();
    for ov in overrides {
        let want = ov.file.to_lowercase();
        let hits: Vec<String> = closure
            .files
            .keys()
            .filter(|p| p.rsplit('/').next() == Some(want.as_str()))
            .cloned()
            .collect();
        let [path] = hits.as_slice() else {
            return Err(invalid(format!(
                "lua override {}: {} file(s) of the closure have this name (expected 1)",
                ov.file,
                hits.len()
            )));
        };
        let bytes = fs::read(&ov.source).map_err(|source| GameDataError::Io {
            path: ov.source.display().to_string(),
            source,
        })?;
        let f = closure
            .files
            .get_mut(path)
            .ok_or_else(|| invalid(format!("lua override {}: file vanished", ov.file)))?;
        let installed = fs::read(&f.abs).map_err(|source| GameDataError::Io {
            path: f.abs.display().to_string(),
            source,
        })?;
        f.abs = ov.source.clone();
        f.size = bytes.len() as u64;
        done.push(Applied {
            path: path.clone(),
            source: ov.source.clone(),
            installed_sha256: sha256_hex(&installed),
            replacement_sha256: sha256_hex(&bytes),
            size: f.size,
        });
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_is_file_equals_path() {
        let o = parse_arg("Lekmod_policies.lua=C:\\x\\y.lua").unwrap();
        assert_eq!(o.file, "Lekmod_policies.lua");
        assert_eq!(o.source, PathBuf::from("C:\\x\\y.lua"));
        assert!(parse_arg("nofile").is_err());
        assert!(parse_arg("a/b.lua=x").is_err());
        assert!(parse_arg("=x").is_err());
    }
}
