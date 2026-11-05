//! Original binaries: lookup, pinned sha256 check, cache directory (SR-D6).
//!
//! The binaries come from your own install of Civ5 1.0.3.279: `--bin <id>=<path>`, `CIV5_BIN_DIR` (a folder with
//! the files, or with `linux_1.0.3.279/` and `win_1.0.3.279/` folders), or the game folder `CIV5_ROOT` (the
//! Windows exes sit in its root; the Linux build's `Civ5XP` in the root of the Linux install). A file is accepted
//! only when its sha256 equals the pin; anything else is an error naming both sums.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::sha256::sha256_hex;

/// The three binaries the shaders live in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BinId {
    /// `Civ5XP`, ELF i386 (Aspyr Linux build).
    Linux,
    /// `CivilizationV.exe`, Direct3D 9.
    Dx9,
    /// `CivilizationV_DX11.exe`, Direct3D 11.
    Dx11,
}

impl BinId {
    pub const ALL: [BinId; 3] = [BinId::Linux, BinId::Dx9, BinId::Dx11];

    /// Short name used by `--bin <name>=<path>` and in `blocks.json`.
    pub fn name(self) -> &'static str {
        match self {
            BinId::Linux => "linux",
            BinId::Dx9 => "dx9",
            BinId::Dx11 => "dx11",
        }
    }

    pub fn from_name(s: &str) -> Option<BinId> {
        BinId::ALL.into_iter().find(|b| b.name() == s)
    }

    pub fn pin(self) -> &'static Pin {
        &PINS[self as usize]
    }
}

impl fmt::Display for BinId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A pinned binary: its folder under `CIV5_BIN_DIR`, its name, size and sha256.
#[derive(Debug)]
pub struct Pin {
    pub id: BinId,
    /// Folder under `CIV5_BIN_DIR` (one per platform build).
    pub dir: &'static str,
    pub file: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

/// Indexed by `BinId as usize`. Civ5 1.0.3.279 (the last Steam build).
pub const PINS: [Pin; 3] = [
    Pin {
        id: BinId::Linux,
        dir: "linux_1.0.3.279",
        file: "Civ5XP",
        size: 36_536_148,
        sha256: "cc06b647821ec5e7cca3c397f6b0d4726f0106cdd67bcf074d494bea2607a8ca",
    },
    Pin {
        id: BinId::Dx9,
        dir: "win_1.0.3.279",
        file: "CivilizationV.exe",
        size: 10_602_040,
        sha256: "f113b5aa47fb6b410fc364e195d9f262f00c49db525f145f32ffd8a1f156bc75",
    },
    Pin {
        id: BinId::Dx11,
        dir: "win_1.0.3.279",
        file: "CivilizationV_DX11.exe",
        size: 16_982_584,
        sha256: "482a43cc52bf7045946c2fcba7cd82dd1ff83b826ef649e01126c865e8eae0a4",
    },
];

#[derive(Debug, thiserror::Error)]
pub enum BinError {
    #[error(
        "{id}: binary not found (looked for {file}). Tried:\n{tried}\nPass --bin {id}=<path>, set CIV5_BIN_DIR to a folder with the files, or CIV5_ROOT to the game folder",
        id = .id, file = .id.pin().file, tried = fmt_paths(.tried)
    )]
    NotFound { id: BinId, tried: Vec<PathBuf> },
    #[error(
        "{id}: {path} is not the pinned build (Civ5 1.0.3.279): sha256 {actual}, expected {expected}. Only the pinned binaries are supported",
        path = .path.display()
    )]
    WrongVersion {
        id: BinId,
        path: PathBuf,
        expected: &'static str,
        actual: String,
    },
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("no cache folder: {0} has no parent; pass --cache <dir> or set CIV5_SHADER_RE_CACHE")]
    NoWorkspace(PathBuf),
}

fn fmt_paths(p: &[PathBuf]) -> String {
    p.iter()
        .map(|p| format!("  {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where to look, in this order.
#[derive(Clone, Debug, Default)]
pub struct Search {
    /// `--bin <id>=<path>`.
    pub explicit: HashMap<BinId, PathBuf>,
    /// `CIV5_BIN_DIR`: either a folder holding the files, or one with the per-build folders of [`Pin::dir`].
    pub bin_dir: Option<PathBuf>,
    /// `CIV5_ROOT`: the game folder (the binaries of that platform build sit in its root).
    pub start: Option<PathBuf>,
}

impl Search {
    /// Environment: `CIV5_BIN_DIR`, then the game folder `CIV5_ROOT`.
    pub fn from_env() -> Search {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Search {
            explicit: HashMap::new(),
            bin_dir: var("CIV5_BIN_DIR"),
            start: var("CIV5_ROOT"),
        }
    }

    fn candidates(&self, id: BinId) -> Vec<PathBuf> {
        let pin = id.pin();
        let mut out = Vec::new();
        if let Some(p) = self.explicit.get(&id) {
            out.push(p.clone());
            return out;
        }
        if let Some(d) = &self.bin_dir {
            out.push(d.join(pin.file));
            out.push(d.join(pin.dir).join(pin.file));
        }
        if let Some(game) = &self.start {
            out.push(game.join(pin.file));
        }
        out
    }
}

/// A binary that passed the pin check.
#[derive(Clone, Debug)]
pub struct FoundBin {
    pub id: BinId,
    pub path: PathBuf,
    pub sha256: String,
}

impl FoundBin {
    /// First 12 hex digits of the sha256: the cache folder name (SR-D6).
    pub fn key(&self) -> &str {
        &self.sha256[..12]
    }

    pub fn read(&self) -> Result<Vec<u8>, BinError> {
        std::fs::read(&self.path).map_err(|source| BinError::Io {
            path: self.path.clone(),
            source,
        })
    }
}

/// Finds `id`; the first existing candidate is the one that is checked.
pub fn find(id: BinId, search: &Search) -> Result<FoundBin, BinError> {
    let candidates = search.candidates(id);
    for path in &candidates {
        if !path.is_file() {
            continue;
        }
        let data = std::fs::read(path).map_err(|source| BinError::Io {
            path: path.clone(),
            source,
        })?;
        return check_pin(id, path, &data);
    }
    Err(BinError::NotFound {
        id,
        tried: candidates,
    })
}

/// The pin check on bytes already in memory (also used by the tests).
pub fn check_pin(id: BinId, path: &Path, data: &[u8]) -> Result<FoundBin, BinError> {
    let pin = id.pin();
    let actual = sha256_hex(data);
    if actual != pin.sha256 {
        return Err(BinError::WrongVersion {
            id,
            path: path.to_path_buf(),
            expected: pin.sha256,
            actual,
        });
    }
    Ok(FoundBin {
        id,
        path: path.to_path_buf(),
        sha256: actual,
    })
}

/// The folder that holds the repository (`tools/shader-re` is two levels below the repository root).
pub fn workspace_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().nth(3).map(Path::to_path_buf)
}

/// Cache root: `--cache`, else `CIV5_SHADER_RE_CACHE`, else `cache/shader-re` next to the repository (SR-D6).
pub fn cache_root(explicit: Option<&Path>) -> Result<PathBuf, BinError> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("CIV5_SHADER_RE_CACHE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let start = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let ws = workspace_root(&start).ok_or(BinError::NoWorkspace(start))?;
    Ok(ws.join("cache").join("shader-re"))
}

/// `<cache root>/<sha256-12>/` for one binary.
pub fn bin_cache_dir(root: &Path, bin: &FoundBin) -> PathBuf {
    root.join(bin.key())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pins_are_well_formed() {
        for (i, p) in PINS.iter().enumerate() {
            assert_eq!(p.id as usize, i);
            assert_eq!(p.sha256.len(), 64);
            assert!(p.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert_eq!(BinId::from_name("dx11"), Some(BinId::Dx11));
        assert_eq!(BinId::from_name("x"), None);
    }

    #[test]
    fn foreign_build_is_a_clear_error() {
        let e = check_pin(BinId::Linux, Path::new("Civ5XP"), b"some other build").unwrap_err();
        let msg = e.to_string();
        assert!(matches!(e, BinError::WrongVersion { .. }));
        assert!(msg.contains("not the pinned build"), "{msg}");
        assert!(msg.contains(BinId::Linux.pin().sha256), "{msg}");
    }

    #[test]
    fn missing_binaries_list_what_was_tried() {
        let dir = std::env::temp_dir().join("shader-re-no-such-dir-xyz");
        let search = Search {
            explicit: HashMap::new(),
            bin_dir: Some(dir),
            start: None,
        };
        let e = find(BinId::Dx9, &search).unwrap_err();
        let msg = e.to_string();
        assert!(matches!(e, BinError::NotFound { .. }));
        assert!(msg.contains("CivilizationV.exe"), "{msg}");
        assert!(msg.contains("CIV5_BIN_DIR"), "{msg}");
    }

    #[test]
    fn explicit_path_wins_and_is_checked() {
        let dir = std::env::temp_dir().join(format!("shader-re-bins-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("fake.exe");
        std::fs::write(&f, b"not the game").unwrap();
        let mut search = Search::default();
        search.explicit.insert(BinId::Dx9, f);
        let e = find(BinId::Dx9, &search).unwrap_err();
        assert!(matches!(e, BinError::WrongVersion { .. }), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_dir_is_the_sha256_prefix() {
        let b = FoundBin {
            id: BinId::Linux,
            path: PathBuf::from("x"),
            sha256: BinId::Linux.pin().sha256.to_string(),
        };
        assert_eq!(b.key(), "cc06b647821e");
        assert_eq!(
            bin_cache_dir(Path::new("c"), &b),
            Path::new("c").join("cc06b647821e")
        );
    }
}
