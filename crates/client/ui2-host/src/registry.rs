//! The skin index of the web build (`ui2/skins.json`) and the checks a skin's file passes before it is used.
//!
//! The index is a JSON array of `{id, file, hash, title_key, themes, api, default}`. The application downloads
//! `ui2/<file>`, hands the bytes to [`SkinRegistry::open`], which reads the package and checks it against the
//! index (hash, id) and against this client's model schema. The registry also remembers skins that failed during
//! this session and picks the fallback after a failure, so a broken skin is never retried in a loop.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::Deserialize;
use ui2_model::Schema;
use ui2_package::{Package, PackageHash};

use crate::error::{LoadFailure, SkinLoadError};

/// Shortest hash prefix the index may carry (the file name's 16 hex digits).
const MIN_HASH_HEX: usize = 16;

/// The script API version a skin is written for: `major.minor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ApiVersion {
    pub major: u16,
    pub minor: u16,
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// One skin of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkinEntry {
    /// The skin id (`engraved`): the `<skin>` of `?ui=v2:<skin>`.
    pub id: String,
    /// The package file name inside `ui2/`.
    pub file: String,
    /// The blake3 of the file, lowercase hex: all 64 digits or a prefix of at least 16.
    pub hash: String,
    /// The game text key of the skin's name (settings list).
    pub title_key: String,
    /// The theme names, in manifest order (theme index 0 first).
    pub themes: Vec<String>,
    /// The script API the skin is written for.
    pub api: ApiVersion,
    /// The skin `?ui=v2` starts with.
    pub default: bool,
}

impl SkinEntry {
    /// The path of the package relative to the web root (`ui2/<file>`).
    pub fn path(&self) -> String {
        format!("ui2/{}", self.file)
    }
}

/// Why an index cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistryError {
    /// Not JSON, or not an array of entries.
    #[error("skin index: {0}")]
    Json(String),
    /// An entry with a bad value.
    #[error("skin index entry {index}: {reason}")]
    Entry { index: usize, reason: String },
    /// Two entries with one id.
    #[error("skin index: skin {0} is listed twice")]
    Duplicate(String),
    /// More than one entry says `default`.
    #[error("skin index: both {0} and {1} are marked default")]
    SeveralDefaults(String, String),
}

/// What the application falls back to after a skin failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fallback {
    /// Another skin of the index (the default one).
    Skin(String),
    /// The classic UI.
    Classic,
}

/// The skins of the build.
#[derive(Debug, Clone, Default)]
pub struct SkinRegistry {
    entries: Vec<SkinEntry>,
    /// Skins that failed in this session.
    broken: BTreeSet<String>,
}

/// The entry as written in JSON; unknown fields are ignored (a newer build may add some).
#[derive(Deserialize)]
struct RawEntry {
    id: String,
    file: String,
    hash: String,
    #[serde(default)]
    title_key: String,
    #[serde(default)]
    themes: Vec<String>,
    api: RawApi,
    #[serde(default)]
    default: bool,
}

/// `"1.0"` or `[1, 0]`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawApi {
    Text(String),
    Pair([u16; 2]),
}

fn parse_api(raw: RawApi) -> Option<ApiVersion> {
    match raw {
        RawApi::Pair([major, minor]) => Some(ApiVersion { major, minor }),
        RawApi::Text(s) => {
            let (major, minor) = s.split_once('.')?;
            Some(ApiVersion {
                major: major.parse().ok()?,
                minor: minor.parse().ok()?,
            })
        }
    }
}

fn check_entry(raw: RawEntry) -> Result<SkinEntry, String> {
    let id_ok = !raw.id.is_empty()
        && raw
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !id_ok {
        return Err(format!(
            "id {:?} must be lowercase letters, digits, - or _",
            raw.id
        ));
    }
    // The file is fetched relative to `ui2/`: a plain file name, nothing that climbs or changes origin.
    let file_ok = raw.file.ends_with(".ui2")
        && raw.file.len() > ".ui2".len()
        && !raw.file.contains(['/', '\\', ':', '?', '#'])
        && !raw.file.starts_with('.');
    if !file_ok {
        return Err(format!(
            "file {:?} must be a plain .ui2 file name",
            raw.file
        ));
    }
    let hash_ok = (MIN_HASH_HEX..=64).contains(&raw.hash.len())
        && raw
            .hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !hash_ok {
        return Err(format!(
            "hash {:?} must be 16 to 64 lowercase hex digits",
            raw.hash
        ));
    }
    let api = parse_api(raw.api).ok_or_else(|| "api must be \"major.minor\"".to_string())?;
    Ok(SkinEntry {
        id: raw.id,
        file: raw.file,
        hash: raw.hash,
        title_key: raw.title_key,
        themes: raw.themes,
        api,
        default: raw.default,
    })
}

/// The full blake3 of a package as lowercase hex.
pub fn hash_hex(h: &PackageHash) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(64);
    for b in h.0 {
        s.push(char::from(HEX[usize::from(b >> 4)]));
        s.push(char::from(HEX[usize::from(b & 15)]));
    }
    s
}

impl SkinRegistry {
    /// Parses `ui2/skins.json`.
    pub fn parse(json: &str) -> Result<SkinRegistry, RegistryError> {
        let raw: Vec<serde_json::Value> =
            serde_json::from_str(json).map_err(|e| RegistryError::Json(e.to_string()))?;
        let mut entries: Vec<SkinEntry> = Vec::with_capacity(raw.len());
        for (index, v) in raw.into_iter().enumerate() {
            let raw: RawEntry = serde_json::from_value(v).map_err(|e| RegistryError::Entry {
                index,
                reason: e.to_string(),
            })?;
            let e = check_entry(raw).map_err(|reason| RegistryError::Entry { index, reason })?;
            if entries.iter().any(|o| o.id == e.id) {
                return Err(RegistryError::Duplicate(e.id));
            }
            if let Some(d) = entries.iter().find(|o| o.default).filter(|_| e.default) {
                return Err(RegistryError::SeveralDefaults(d.id.clone(), e.id));
            }
            entries.push(e);
        }
        Ok(SkinRegistry {
            entries,
            broken: BTreeSet::new(),
        })
    }

    /// The skins, in index order.
    pub fn entries(&self) -> &[SkinEntry] {
        &self.entries
    }

    /// A skin by id.
    pub fn get(&self, id: &str) -> Option<&SkinEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// The default skin, if the index marks one.
    pub fn default_skin(&self) -> Option<&SkinEntry> {
        self.entries.iter().find(|e| e.default)
    }

    /// The skin to start for a choice: `Some(id)` for `v2:<id>`, `None` for plain `v2`. An unknown or broken skin
    /// gives the default skin (unless it is broken too); `None` means the classic UI.
    pub fn resolve(&self, want: Option<&str>) -> Option<&SkinEntry> {
        let usable = |e: &&SkinEntry| !self.broken.contains(&e.id);
        want.and_then(|id| self.get(id))
            .filter(usable)
            .or_else(|| self.default_skin().filter(usable))
    }

    /// Remembers that a skin failed: it is not chosen again in this session.
    pub fn mark_broken(&mut self, id: &str) {
        self.broken.insert(id.to_string());
    }

    /// Whether a skin failed in this session.
    pub fn is_broken(&self, id: &str) -> bool {
        self.broken.contains(id)
    }

    /// Marks a failed skin broken and says what to fall back to: the default skin if it is not the one that failed
    /// (nor broken before), otherwise the classic UI.
    pub fn fail(&mut self, failed: &str) -> Fallback {
        self.mark_broken(failed);
        match self.resolve(None) {
            Some(e) => Fallback::Skin(e.id.clone()),
            None => Fallback::Classic,
        }
    }

    /// Reads the downloaded file of a skin and checks it: a readable package, the hash of the index, the id of the
    /// index, and model paths and intents this client has. The result is ready for [`V2Ui::new`](crate::V2Ui::new).
    pub fn open(&self, id: &str, bytes: Arc<[u8]>) -> Result<Arc<Package>, SkinLoadError> {
        let entry = self
            .get(id)
            .ok_or_else(|| SkinLoadError::new(id, LoadFailure::UnknownSkin))?;
        entry.check_api()?;
        let fail = |f| SkinLoadError::new(id, f);
        let pkg = Package::read(bytes).map_err(|e| fail(LoadFailure::Package(e)))?;
        let file = hash_hex(&pkg.hash());
        if !file.starts_with(&entry.hash) {
            return Err(fail(LoadFailure::HashMismatch {
                index: entry.hash.clone(),
                file,
            }));
        }
        if pkg.manifest().id != entry.id {
            return Err(fail(LoadFailure::IdMismatch {
                index: entry.id.clone(),
                package: pkg.manifest().id.clone(),
            }));
        }
        check_package(&pkg).map_err(fail)?;
        Ok(Arc::new(pkg))
    }
}

/// The script API version this client offers to skins.
pub const SCRIPT_API: ApiVersion = ApiVersion { major: 1, minor: 0 };

impl SkinEntry {
    /// The index entry's script API against this client's ([`SCRIPT_API`], the same major version): a skin of
    /// another major version is refused before it is downloaded.
    pub fn check_api(&self) -> Result<(), SkinLoadError> {
        if self.api.major == SCRIPT_API.major {
            return Ok(());
        }
        Err(SkinLoadError::new(
            self.id.as_str(),
            LoadFailure::Incompatible(ui2_package::PackageError::Incompatible {
                need: format!("script API {} (the client has {SCRIPT_API})", self.api),
            }),
        ))
    }
}

/// The checks of a package against this client that do not need an index: versions (profile, model, script
/// API), model paths and intents.
pub fn check_package(pkg: &Package) -> Result<(), LoadFailure> {
    pkg.check_compat(&ui2_package::Versions {
        profile: ui2_package::PROFILE_VERSION,
        model: ui2_model::r#gen::SCHEMA_VERSION,
        api: (SCRIPT_API.major, SCRIPT_API.minor),
    })
    .map_err(LoadFailure::Incompatible)?;
    pkg.check_paths(Schema::v1())
        .map_err(LoadFailure::Incompatible)
}
