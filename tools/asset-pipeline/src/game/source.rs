//! Virtual file system over a Civ5 install: FPK packs plus loose files, with DLC overlays.
//!
//! Override order, as the exe looks files up (RW-9b/9c, `civ5-re/build/b2/rw-9bc-answers.txt`),
//! lowest to highest priority; the lookup is by bare file name:
//! 1. the root manager: `Resource/Common` packs < `Resource/DX9` packs < loose `Resource/Overrides`
//!    (replaces archives) < loose `Assets/` (except `DLC`), where by RW-9d (`rw-9d-answer.txt`) only
//!    `*.xml` and `*.ggxml` replace an archived copy and every other loose type loses to it
//!    (`UnitFormations.xml` stays the loose copy);
//! 2. the content packages under `Assets/DLC`, sorted by path case-insensitively (`DLC_01`..
//!    `DLC_07`, `DLC_Deluxe`, `DLC_SP_Maps*`, `Expansion`, `Expansion2`, `LEKMOD_v34.15`,
//!    `Shared`), each with the packs of its `<Common><Paks>` list (they are in `Resource/DX9`
//!    but belong to the package, not to the root), then its folder scanned loose.
//!
//! A later package beats every earlier one and the root; inside a package a loose file beats its
//! packs. `Tablet`, `UI_bc1` and the other folders are not read: low-quality UI variants and
//! mods that the index leaves out on purpose (their activation on desktop is not proven).
//! Lekmod is read because it is an allowed asset source (decision D15 of build B1).
//!
//! Keys are lower-case file names without directories, because FPK names are flat;
//! loose files from different folders therefore collide (e.g. per-language voice-overs)
//! and the winner is the last one in sorted path order.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use super::fpk::{self, FpkError};

/// Official Firaxis folders under `Assets/DLC`; everything else is skipped.
/// `Tablet` and `UI_bc1` are deliberately absent: they are low-quality UI variants of
/// base assets and would shadow the desktop versions.
const DLC_ALLOW: &[&str] = &[
    "expansion",
    "expansion2",
    "dlc_01",
    "dlc_02",
    "dlc_03",
    "dlc_04",
    "dlc_05",
    "dlc_06",
    "dlc_07",
    "dlc_deluxe",
    "dlc_sp_maps",
    "dlc_sp_maps_2",
    "dlc_sp_maps_3",
    "shared",
];

/// Lekmod folder under `Assets/DLC` that the docs ask for (decision D15 of build B1). Matched
/// case-insensitively; [`is_lekmod_dir`] also accepts the other spellings of the same version.
pub const LEKMOD_DIR: &str = "LEKMOD_v34.15";

/// Whether a folder under `Assets/DLC` holds Lekmod v34.15, by its name: [`LEKMOD_DIR`], the
/// spellings of the same version that upstream's install guide and zips produce (`LEKMODv34_15`,
/// `LEKMOD v34.15`, `lekmod-v34-15`), or the repository's `LEKMOD` folder copied under its own
/// name. Any case; separators are ignored. Other versions (`LEKMOD_v34.16`) are not accepted.
pub fn is_lekmod_dir(name: &str) -> bool {
    let key: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    matches!(key.as_str(), "lekmodv3415" | "lekmod3415" | "lekmod")
}

/// The one Lekmod folder the index reads among the folder names of `Assets/DLC` (sorted): the
/// exact [`LEKMOD_DIR`] if present, else the first name [`is_lekmod_dir`] accepts.
pub fn pick_lekmod_dir<'a>(names: impl IntoIterator<Item = &'a str> + Clone) -> Option<&'a str> {
    names
        .clone()
        .into_iter()
        .find(|n| n.eq_ignore_ascii_case(LEKMOD_DIR))
        .or_else(|| names.into_iter().find(|n| is_lekmod_dir(n)))
}

/// Pack directories under `Resource/` in override order. `DX9_Low` is not listed on purpose: its
/// textures are the reduced variants of the low preset (`tx:<name>@low`, AB-D12), a separate layer
/// that only [`GameSource::lookup_low`] sees.
const PACK_DIRS: &[&str] = &["Common", "DX9"];

/// The low-quality packs, read as a layer of their own.
const LOW_PACK_DIR: &str = "DX9_Low";

#[derive(Debug, thiserror::Error)]
pub enum GameError {
    #[error("{0} is not a Civ5 install (no Resource/ directory)")]
    NotGameRoot(PathBuf),
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("pack {path}: {source}")]
    Pack {
        path: PathBuf,
        #[source]
        source: FpkError,
    },
    #[error("entry {name:?} is outside the pack index")]
    BadPackIndex { name: String },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> GameError + '_ {
    move |source| GameError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Where a file comes from: a tag for reports and picks. The override order is the order the
/// packages are read in (see the module comment), not the order of these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// The Lekmod package folder.
    Lekmod,
    Base,
    Expansion,
    Expansion2,
    Dlc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Packed { pack: u32, offset: u32, size: u32 },
    Loose { path: PathBuf, size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Lower-case file name, the lookup key.
    pub name: String,
    pub layer: Layer,
    /// Position of its source in the override order: 0 for the root, then 1, 2, ... for the content
    /// packages in the order they are read. Files of a higher rank beat those of a lower one.
    pub rank: u32,
    pub origin: Origin,
}

impl Entry {
    pub fn size(&self) -> u64 {
        match &self.origin {
            Origin::Packed { size, .. } => u64::from(*size),
            Origin::Loose { size, .. } => *size,
        }
    }

    /// Lower-case extension without the dot; empty if none.
    pub fn extension(&self) -> &str {
        extension_of(&self.name)
    }
}

fn extension_of(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, ext)| ext)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ExtStat {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Excluded {
    pub dir: String,
    pub reason: &'static str,
}

/// Index statistics collected while opening; counts include shadowed entries.
#[derive(Debug, Default, Clone)]
pub struct ScanStats {
    pub packs: usize,
    /// Packs whose directory failed to parse (skipped, so the report can show them).
    pub pack_errors: Vec<(PathBuf, String)>,
    pub packed: BTreeMap<String, ExtStat>,
    pub loose: BTreeMap<String, ExtStat>,
    pub shadowed: usize,
    pub excluded: Vec<Excluded>,
}

#[derive(Debug)]
pub struct GameSource {
    root: PathBuf,
    packs: Vec<PathBuf>,
    entries: BTreeMap<String, Entry>,
    shadowed: Vec<Entry>,
    /// The `DX9_Low` packs; not part of `entries`, not counted in the statistics.
    low: BTreeMap<String, Entry>,
    stats: ScanStats,
    /// `rank` of the entries being added.
    rank: u32,
    /// Name of the Lekmod folder under `Assets/DLC` that was read, as it is on disk.
    lekmod: Option<String>,
}

impl GameSource {
    /// Indexes the install at `root`. Only reads directories; payloads stay on disk.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, GameError> {
        let root = root.into();
        if !root.join("Resource").is_dir() {
            return Err(GameError::NotGameRoot(root));
        }
        let mut src = GameSource {
            root,
            packs: Vec::new(),
            entries: BTreeMap::new(),
            shadowed: Vec::new(),
            low: BTreeMap::new(),
            stats: ScanStats::default(),
            rank: 0,
            lekmod: None,
        };

        // Ascending priority: later inserts shadow earlier ones (the exe searches the last created
        // manager first, RW-9b/9c).
        let assets = src.root.join("Assets");
        let packages = if assets.join("DLC").is_dir() {
            src.find_packages(&assets.join("DLC"))?
        } else {
            Vec::new()
        };
        // The root manager, created first and searched last: Common packs < DX9 packs < loose
        // `Resource/Overrides` < loose `Assets`; the extension decides in `Assets` (RW-9d).
        // Packs a content package lists in its `<Common><Paks>` belong to that package.
        let claimed: BTreeSet<String> = packages
            .iter()
            .flat_map(|p| p.paks.iter().map(|n| n.to_ascii_lowercase()))
            .collect();
        for dir in PACK_DIRS {
            src.add_packs(&src.root.join("Resource").join(dir), &claimed)?;
        }
        // `Resource/Overrides` loose files replace archived ones (mode ADD_REPLACE_ARCHIVED, RW-9d).
        let overrides = src.root.join("Resource").join("Overrides");
        if overrides.is_dir() {
            src.add_loose(&overrides, Layer::Base, None, RootRule::ReplaceArchived)?;
        }
        if assets.is_dir() {
            src.add_loose(&assets, Layer::Base, Some("dlc"), RootRule::Assets)?;
        } else {
            tracing::warn!(path = %assets.display(), "no Assets/ directory, loose files skipped");
        }
        // Content packages in the order of their sorted paths; each has its listed packs, then its
        // folder scanned loose (a loose file beats a pack of the same package). A package always
        // beats the root and every package before it.
        for (i, p) in packages.into_iter().enumerate() {
            src.rank = i as u32 + 1;
            for name in &p.paks {
                let found = ["DX9", "Common"].iter().find_map(|d| {
                    let f = src.root.join("Resource").join(d).join(name);
                    f.is_file().then_some(f)
                });
                match found {
                    Some(f) => src.add_pak(f, p.layer)?,
                    None => {
                        tracing::warn!(pack = %name, "pack listed by a package is not in the install")
                    }
                }
            }
            src.add_loose(&p.dir, p.layer, None, RootRule::Package)?;
        }
        src.stats.packs = src.packs.len();
        src.stats.shadowed = src.shadowed.len();
        src.add_low_packs()?;
        Ok(src)
    }

    /// The copy of `name` in the `DX9_Low` packs (the low preset's variant), if there is one.
    pub fn lookup_low(&self, name: &str) -> Option<&Entry> {
        self.low.get(&name.to_ascii_lowercase())
    }

    /// Every copy of `name` the layers hold: the visible one first, then the ones it hides, in the
    /// order they were hidden. The `DX9_Low` variants are not among them.
    pub fn copies(&self, name: &str) -> Vec<&Entry> {
        let name = name.to_ascii_lowercase();
        self.entries
            .get(&name)
            .into_iter()
            .chain(self.shadowed.iter().filter(|e| e.name == name))
            .collect()
    }

    fn add_low_packs(&mut self) -> Result<(), GameError> {
        let dir = self.root.join("Resource").join(LOW_PACK_DIR);
        if !dir.is_dir() {
            return Ok(());
        }
        let mut files = sorted_dir(&dir)?
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fpk")) && p.is_file())
            .collect::<Vec<_>>();
        files.sort();
        for path in files {
            let entries = match fpk::open_directory(&path) {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(pack = %path.display(), %err, "low pack skipped");
                    continue;
                }
            };
            let pack = self.packs.len() as u32;
            self.packs.push(path);
            for e in entries {
                let name = e.name.to_ascii_lowercase();
                self.low.insert(
                    name.clone(),
                    Entry {
                        name,
                        layer: Layer::Base,
                        rank: 0,
                        origin: Origin::Packed {
                            pack,
                            offset: e.offset,
                            size: e.size,
                        },
                    },
                );
            }
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The Lekmod folder under `Assets/DLC` that the index read (its name on disk), or `None`
    /// when the install has none that [`is_lekmod_dir`] accepts.
    pub fn lekmod_dir(&self) -> Option<&str> {
        self.lekmod.as_deref()
    }

    pub fn stats(&self) -> &ScanStats {
        &self.stats
    }

    /// Entries hidden by a higher-priority one, in the order they were shadowed.
    pub fn shadowed(&self) -> &[Entry] {
        &self.shadowed
    }

    /// Number of distinct names visible after overrides.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Case-insensitive lookup by file name.
    pub fn lookup(&self, name: &str) -> Option<&Entry> {
        self.entries.get(&name.to_ascii_lowercase())
    }

    /// The file an entry comes from: the pack for a packed entry, the file itself for a loose one.
    pub fn origin_path(&self, entry: &Entry) -> Option<PathBuf> {
        match &entry.origin {
            Origin::Loose { path, .. } => Some(path.clone()),
            Origin::Packed { pack, .. } => self.packs.get(*pack as usize).cloned(),
        }
    }

    /// Every visible entry, sorted by name.
    pub fn iter_all(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// Visible entries with the given extension (any case, no dot), sorted by name.
    pub fn iter_ext<'a>(&'a self, ext: &'a str) -> impl Iterator<Item = &'a Entry> {
        self.entries
            .values()
            .filter(move |e| e.extension().eq_ignore_ascii_case(ext))
    }

    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>, GameError> {
        match &entry.origin {
            Origin::Loose { path, .. } => fs::read(path).map_err(io_err(path)),
            Origin::Packed { pack, offset, size } => {
                let path =
                    self.packs
                        .get(*pack as usize)
                        .ok_or_else(|| GameError::BadPackIndex {
                            name: entry.name.clone(),
                        })?;
                let mut file = File::open(path).map_err(io_err(path))?;
                let e = fpk::FpkEntry {
                    name: entry.name.clone(),
                    size: *size,
                    offset: *offset,
                };
                fpk::read_payload(&mut file, &e).map_err(|source| GameError::Pack {
                    path: path.clone(),
                    source,
                })
            }
        }
    }

    pub fn read_by_name(&self, name: &str) -> Result<Option<Vec<u8>>, GameError> {
        self.lookup(name).map(|e| self.read(e)).transpose()
    }

    fn count(&mut self, entry: &Entry) {
        let (stat, bytes) = match entry.origin {
            Origin::Packed { .. } => (&mut self.stats.packed, entry.size()),
            Origin::Loose { .. } => (&mut self.stats.loose, entry.size()),
        };
        let s = stat.entry(entry.extension().to_string()).or_default();
        s.count += 1;
        s.bytes += bytes;
    }

    fn insert(&mut self, entry: Entry) {
        self.count(&entry);
        if let Some(old) = self.entries.insert(entry.name.clone(), entry) {
            self.shadowed.push(old);
        }
    }

    /// Adds the `.fpk` files of `dir`, except those in `skip` (lower-case names).
    fn add_packs(&mut self, dir: &Path, skip: &BTreeSet<String>) -> Result<(), GameError> {
        if !dir.is_dir() {
            tracing::warn!(path = %dir.display(), "pack directory missing");
            return Ok(());
        }
        let mut files = sorted_dir(dir)?
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fpk")) && p.is_file())
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| skip.contains(&n.to_string_lossy().to_ascii_lowercase()))
            })
            .collect::<Vec<_>>();
        files.sort();
        for path in files {
            self.add_pak(path, Layer::Base)?;
        }
        Ok(())
    }

    /// Adds the entries of one pack; a pack that does not parse is skipped and reported.
    fn add_pak(&mut self, path: PathBuf, layer: Layer) -> Result<(), GameError> {
        let dir_entries = match fpk::open_directory(&path) {
            Ok(e) => e,
            Err(err) => {
                tracing::warn!(pack = %path.display(), %err, "pack skipped");
                self.stats.pack_errors.push((path, err.to_string()));
                return Ok(());
            }
        };
        let pack = self.packs.len() as u32;
        self.packs.push(path);
        for e in dir_entries {
            self.insert(Entry {
                name: e.name.to_ascii_lowercase(),
                layer,
                rank: self.rank,
                origin: Origin::Packed {
                    pack,
                    offset: e.offset,
                    size: e.size,
                },
            });
        }
        Ok(())
    }

    /// The content packages under `Assets/DLC` that the index reads, in the order the exe creates
    /// them: by path, case-insensitive (`FindPackages` sorts the `.Civ5Pkg` paths, so the folders
    /// go `DLC_01`..`DLC_07`, `DLC_Deluxe`, `DLC_SP_Maps*`, `Expansion`, `Expansion2`, `LEKMOD_*`,
    /// `Shared`, ...). Folders outside the allow-list are reported and skipped; of the Lekmod
    /// folders only the one [`pick_lekmod_dir`] chooses is read.
    fn find_packages(&mut self, dlc_root: &Path) -> Result<Vec<Package>, GameError> {
        let mut dirs = sorted_dir(dlc_root)?;
        dirs.retain(|p| p.is_dir());
        dirs.sort_by_key(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default()
        });
        let names: Vec<String> = dirs
            .iter()
            .map(|d| {
                d.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        self.lekmod = pick_lekmod_dir(names.iter().map(String::as_str)).map(str::to_owned);
        let mut out = Vec::new();
        for (dir, name) in dirs.into_iter().zip(names) {
            let lower = name.to_ascii_lowercase();
            let layer = if self.lekmod.as_deref() == Some(name.as_str()) {
                Layer::Lekmod
            } else if DLC_ALLOW.contains(&lower.as_str()) {
                match lower.as_str() {
                    "expansion" => Layer::Expansion,
                    "expansion2" => Layer::Expansion2,
                    _ => Layer::Dlc,
                }
            } else {
                let reason = if is_lekmod_dir(&name) {
                    "a second Lekmod v34.15 folder (only one is read)"
                } else if lower.starts_with("lekmod") {
                    "third-party mod (not Lekmod v34.15)"
                } else {
                    "not in the official allow-list"
                };
                tracing::warn!(dir = %name, reason, "excluded from the index");
                self.stats.excluded.push(Excluded { dir: name, reason });
                continue;
            };
            let mut paks = Vec::new();
            for f in sorted_dir(&dir)? {
                if f.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("civ5pkg"))
                {
                    let text = fs::read_to_string(&f).map_err(io_err(&f))?;
                    paks.extend(common_paks(&text));
                }
            }
            out.push(Package { dir, layer, paks });
        }
        Ok(out)
    }

    /// Adds every file under `dir`; `skip_top` names a top-level folder to leave out.
    fn add_loose(
        &mut self,
        dir: &Path,
        layer: Layer,
        skip_top: Option<&str>,
        rule: RootRule,
    ) -> Result<(), GameError> {
        let mut files = Vec::new();
        walk(dir, skip_top, &mut files)?;
        for (path, size) in files {
            let Some(name) = path.file_name() else {
                continue;
            };
            let entry = Entry {
                name: name.to_string_lossy().to_ascii_lowercase(),
                layer,
                rank: self.rank,
                origin: Origin::Loose { path, size },
            };
            // RW-9d: in the root catalog a loose file of `Assets` that is not `.xml`/`.ggxml` does
            // not replace an archived copy (the archive stays, the loose file is hidden).
            let archive_stays = rule == RootRule::Assets
                && !matches!(extension_of(&entry.name), "xml" | "ggxml")
                && self
                    .entries
                    .get(&entry.name)
                    .is_some_and(|old| matches!(old.origin, Origin::Packed { .. }));
            if archive_stays {
                self.count(&entry);
                self.shadowed.push(entry);
            } else {
                self.insert(entry);
            }
        }
        Ok(())
    }
}

/// How loose files meet the entries already in the catalog (RW-9d, `civ5-re/build/b2/rw-9d-answer.txt`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootRule {
    /// Loose `Assets` of the root: `.xml`/`.ggxml` replace an archive, other types lose to it.
    Assets,
    /// `Resource/Overrides`: replaces archived copies.
    ReplaceArchived,
    /// A content package folder: a loose file always wins.
    Package,
}

/// A content package that is read: its folder, its layer tag and the packs of its `<Common>` list.
struct Package {
    dir: PathBuf,
    layer: Layer,
    paks: Vec<String>,
}

/// The `<File>` names of `<Common><Paks>` in the text of a `.Civ5Pkg`.
fn common_paks(text: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once("<Common>") else {
        return Vec::new();
    };
    let section = rest.split_once("</Common>").map_or(rest, |(s, _)| s);
    let mut out = Vec::new();
    let mut tail = section;
    while let Some((_, after)) = tail.split_once("<File>") {
        let Some((name, next)) = after.split_once("</File>") else {
            break;
        };
        out.push(name.trim().to_owned());
        tail = next;
    }
    out
}

fn sorted_dir(dir: &Path) -> Result<Vec<PathBuf>, GameError> {
    let mut v = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err(dir))?;
    v.sort();
    Ok(v)
}

/// Depth-first walk in sorted order, so the result doesn't depend on the file system.
fn walk(
    dir: &Path,
    skip_top: Option<&str>,
    out: &mut Vec<(PathBuf, u64)>,
) -> Result<(), GameError> {
    for path in sorted_dir(dir)? {
        let meta = fs::symlink_metadata(&path).map_err(io_err(&path))?;
        if meta.is_dir() {
            let skipped = skip_top.is_some_and(|s| {
                path.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(s))
            });
            if !skipped {
                walk(&path, None, out)?;
            }
        } else if meta.is_file() {
            out.push((path, meta.len()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::fpk::testutil::build_fpk;
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "civ5-assets-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            TempTree(dir)
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fake_install() -> TempTree {
        let t = TempTree::new();
        t.write(
            "Resource/Common/m.fpk",
            &build_fpk(&[("Shared.TXT", b"pack-common"), ("only_pack.gr2", b"gr2")]),
        );
        t.write(
            "Resource/DX9/t.fpk",
            &build_fpk(&[("shared.txt", b"pack-dx9"), ("tex.dds", b"dds")]),
        );
        t.write(
            "Resource/DX9_Low/low.fpk",
            &build_fpk(&[("tex.dds", b"LOW"), ("low_only.dds", b"x")]),
        );
        t.write("Assets/Gameplay/shared.txt", b"loose-base");
        // (a loose .txt does not beat the archive in the root, RW-9d; the packages above it decide)
        t.write("Assets/DLC/Expansion/shared.txt", b"exp1");
        t.write("Assets/DLC/Expansion2/shared.txt", b"exp2");
        t.write("Assets/DLC/Expansion2/ui/bnw_only.lua", b"lua");
        t.write("Assets/DLC/DLC_01/Deep/Dir/dlc.xml", b"dlc1");
        t.write("Assets/DLC/Shared/dlc.xml", b"dlc-shared");
        t.write("Assets/DLC/LEKMOD_v34.15/shared.txt", b"MOD");
        t.write("Assets/DLC/LEKMOD_v34.15/lekmod_only.lua", b"LEKMOD-ONLY");
        t.write("Assets/DLC/LEKMOD_v34.16/lekmod_34_16.dat", b"refused");
        t.write("Assets/DLC/OtherMod/other_mod.dat", b"refused");
        t.write("Assets/DLC/Tablet/shared.txt", b"TABLET");
        t
    }

    fn text(src: &GameSource, name: &str) -> String {
        String::from_utf8(src.read_by_name(name).unwrap().unwrap()).unwrap()
    }

    #[test]
    fn override_order_base_pack_loose_expansions_dlc() {
        let t = fake_install();
        let src = GameSource::open(&t.0).unwrap();

        // Lekmod is a package after Expansion2.
        assert_eq!(text(&src, "shared.txt"), "MOD");
        assert_eq!(src.lookup("SHARED.txt").unwrap().layer, Layer::Lekmod);
        // DX9 beats Common within the base layer.
        assert_eq!(text(&src, "tex.dds"), "dds");
        // DLC layer beats everything below; `Shared` sorts after `DLC_01`.
        assert_eq!(text(&src, "dlc.xml"), "dlc-shared");
        assert_eq!(text(&src, "only_pack.gr2"), "gr2");
        assert_eq!(text(&src, "bnw_only.lua"), "lua");
    }

    // ---- one test per rule of RW-9b/9c ----

    const PKG_EXP2: &str = "<Civ5Package><Gameplay><File>x.xml</File></Gameplay><Common><Paks>\
         <File>Exp2Units.fpk</File></Paks></Common></Civ5Package>";

    #[test]
    fn rule_1_the_lookup_is_by_bare_file_name_wherever_the_file_sits() {
        let t = TempTree::new();
        t.write(
            "Resource/Common/m.fpk",
            &build_fpk(&[("Name.DDS", b"pack")]),
        );
        t.write("Assets/DLC/Shared/a/b/c/name.dds", b"deep-loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "NAME.dds"), "deep-loose");
    }

    #[test]
    fn rule_2_in_the_root_dx9_beats_common_and_a_loose_non_xml_file_loses_to_both() {
        let t = TempTree::new();
        t.write(
            "Resource/Common/m.fpk",
            &build_fpk(&[
                ("a.txt", b"common"),
                ("b.txt", b"common"),
                ("e.txt", b"common"),
            ]),
        );
        t.write(
            "Resource/DX9/d.fpk",
            &build_fpk(&[("b.txt", b"dx9"), ("d.txt", b"dx9"), ("e.txt", b"dx9")]),
        );
        t.write("Assets/Overlays/a.txt", b"loose");
        t.write("Assets/Overlays/b.txt", b"loose");
        t.write("Assets/Overlays/c.txt", b"loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "a.txt"), "common", "RW-9d: the archive stays");
        assert_eq!(text(&src, "b.txt"), "dx9");
        assert_eq!(text(&src, "d.txt"), "dx9");
        assert_eq!(text(&src, "e.txt"), "dx9");
        assert_eq!(
            text(&src, "c.txt"),
            "loose",
            "a loose file nothing else has"
        );
        // The loose copy is hidden, not lost.
        assert_eq!(src.copies("a.txt").len(), 2);
    }

    #[test]
    fn rule_2b_a_loose_xml_or_ggxml_replaces_the_archived_copy() {
        let t = TempTree::new();
        t.write(
            "Resource/Common/m.fpk",
            &build_fpk(&[
                ("UnitFormations.xml", b"packed"),
                ("x.ggxml", b"packed"),
                ("y.dds", b"packed"),
            ]),
        );
        t.write("Assets/Units/UnitFormations.xml", b"loose");
        t.write("Assets/Misc/x.ggxml", b"loose");
        t.write("Assets/Misc/y.dds", b"loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "unitformations.xml"), "loose");
        assert_eq!(text(&src, "x.ggxml"), "loose");
        assert_eq!(text(&src, "y.dds"), "packed");
    }

    #[test]
    fn rule_2c_resource_overrides_loose_files_beat_archives_of_any_type() {
        let t = TempTree::new();
        t.write(
            "Resource/DX9/d.fpk",
            &build_fpk(&[("o.dds", b"packed"), ("p.csv", b"packed")]),
        );
        t.write("Resource/Overrides/o.dds", b"override");
        t.write("Assets/Misc/p.csv", b"assets-loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "o.dds"), "override");
        assert_eq!(text(&src, "p.csv"), "packed");
    }

    #[test]
    fn rule_2d_loose_over_loose_in_the_root_takes_the_later_scan() {
        let t = TempTree::new();
        t.write("Resource/Overrides/q.dds", b"override");
        t.write("Assets/Misc/q.dds", b"assets");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "q.dds"), "assets");
    }

    #[test]
    fn rule_2e_a_package_loose_file_of_any_type_beats_a_root_archive() {
        let t = TempTree::new();
        t.write("Resource/DX9/d.fpk", &build_fpk(&[("r.dds", b"packed")]));
        t.write("Assets/DLC/Expansion/r.dds", b"pkg-loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "r.dds"), "pkg-loose");
    }

    #[test]
    fn rule_3_a_package_beats_the_root_loose_or_packed() {
        let t = TempTree::new();
        t.write("Resource/DX9/d.fpk", &build_fpk(&[("p.txt", b"root-pak")]));
        t.write("Assets/l.txt", b"root-loose");
        t.write("Assets/DLC/Shared/p.txt", b"pkg-loose");
        t.write("Assets/DLC/Shared/l.txt", b"pkg-loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "p.txt"), "pkg-loose");
        assert_eq!(text(&src, "l.txt"), "pkg-loose");
    }

    #[test]
    fn rule_4_packages_are_ordered_by_path_ignoring_case_and_the_later_wins() {
        let t = TempTree::new();
        fs::create_dir_all(t.0.join("Resource")).unwrap();
        // Sorted: DLC_01 < dlc_Deluxe < DLC_SP_Maps < DLC_SP_Maps_2 < Expansion < Expansion2 <
        // LEKMOD_v34.15 < Shared.
        for (dir, w) in [
            ("DLC_01", "dlc01"),
            ("dlc_Deluxe", "deluxe"),
            ("DLC_SP_Maps", "maps"),
            ("DLC_SP_Maps_2", "maps2"),
            ("Expansion", "exp1"),
            ("Expansion2", "exp2"),
            ("LEKMOD_v34.15", "lekmod"),
            ("Shared", "shared"),
        ] {
            t.write(&format!("Assets/DLC/{dir}/order.txt"), w.as_bytes());
        }
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "order.txt"), "shared");
        // Remove from the top: every step gives the next one down.
        for (dir, want) in [
            ("Shared", "lekmod"),
            ("LEKMOD_v34.15", "exp2"),
            ("Expansion2", "exp1"),
            ("Expansion", "maps2"),
            ("DLC_SP_Maps_2", "maps"),
            ("DLC_SP_Maps", "deluxe"),
            ("dlc_Deluxe", "dlc01"),
        ] {
            fs::remove_file(t.0.join(format!("Assets/DLC/{dir}/order.txt"))).unwrap();
            let src = GameSource::open(&t.0).unwrap();
            assert_eq!(text(&src, "order.txt"), want, "after removing {dir}");
        }
    }

    #[test]
    fn rule_5_the_packs_a_package_lists_belong_to_it_and_beat_the_root_and_earlier_packages() {
        let t = TempTree::new();
        // The root has UnitsTextures.fpk; Expansion1 and Expansion2 list their own packs, which
        // sit in Resource/DX9 like the root's.
        t.write(
            "Resource/DX9/Units.fpk",
            &build_fpk(&[("irr.dds", b"root-4k")]),
        );
        t.write(
            "Resource/DX9/Exp1Units.fpk",
            &build_fpk(&[("irr.dds", b"exp1")]),
        );
        t.write(
            "Resource/DX9/Exp2Units.fpk",
            &build_fpk(&[("irr.dds", b"exp2-24k")]),
        );
        t.write(
            "Assets/DLC/Expansion/Expansion1.Civ5Pkg",
            b"<Common><Paks><File>Exp1Units.fpk</File></Paks></Common>",
        );
        t.write(
            "Assets/DLC/Expansion2/Expansion2.Civ5Pkg",
            PKG_EXP2.as_bytes(),
        );
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "irr.dds"), "exp2-24k");
        assert_eq!(src.lookup("irr.dds").unwrap().layer, Layer::Expansion2);
        // The other copies are hidden, not lost.
        let hidden: Vec<Layer> = src.shadowed().iter().map(|e| e.layer).collect();
        assert!(hidden.contains(&Layer::Base) && hidden.contains(&Layer::Expansion));

        // Without the Common list the same file would be a root pack: DX9 packs by name, the
        // later name wins, so the claim by the package is what decides.
        let t2 = TempTree::new();
        t2.write(
            "Resource/DX9/Units.fpk",
            &build_fpk(&[("irr.dds", b"root-4k")]),
        );
        t2.write(
            "Resource/DX9/Exp2Units.fpk",
            &build_fpk(&[("irr.dds", b"exp2-24k")]),
        );
        t2.write(
            "Assets/DLC/Expansion2/Expansion2.Civ5Pkg",
            b"<Common><Paks></Paks></Common>",
        );
        let src = GameSource::open(&t2.0).unwrap();
        assert_ne!(src.lookup("irr.dds").unwrap().layer, Layer::Expansion2);
    }

    #[test]
    fn rule_6_inside_a_package_a_loose_file_beats_its_packs() {
        let t = TempTree::new();
        t.write(
            "Resource/DX9/Exp2Units.fpk",
            &build_fpk(&[("f.txt", b"pak")]),
        );
        t.write(
            "Assets/DLC/Expansion2/Expansion2.Civ5Pkg",
            PKG_EXP2.as_bytes(),
        );
        t.write("Assets/DLC/Expansion2/f.txt", b"loose");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "f.txt"), "loose");
        assert!(matches!(
            src.copies("f.txt")[1].origin,
            Origin::Packed { .. }
        ));
    }

    #[test]
    fn rule_7_terraindef_the_loose_shared_copy_beats_the_root_pak() {
        let t = TempTree::new();
        t.write(
            "Resource/Common/TerrainModels.fpk",
            &build_fpk(&[("terraindef.csv", b"packed")]),
        );
        t.write("Assets/DLC/Shared/Terrain/TerrainDef.csv", b"loose-shared");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "TerrainDef.csv"), "loose-shared");
        assert_eq!(src.lookup("terraindef.csv").unwrap().layer, Layer::Dlc);
    }

    #[test]
    fn rule_8_lekmod_is_a_package_between_expansion2_and_shared() {
        let t = TempTree::new();
        fs::create_dir_all(t.0.join("Resource")).unwrap();
        t.write("Assets/Overlays/Highlights.xml", b"base-15");
        t.write("Assets/DLC/Expansion2/highlights.xml", b"exp2-copy");
        t.write(
            "Assets/DLC/LEKMOD_v34.15/Lua/UI/Highlights.xml",
            b"lekmod-34",
        );
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "highlights.xml"), "lekmod-34");
        assert_eq!(src.lookup("highlights.xml").unwrap().layer, Layer::Lekmod);
        // The copies it hides are still listed, so the report can show them.
        assert_eq!(src.copies("highlights.xml").len(), 3);
        assert_eq!(src.stats().shadowed, src.shadowed().len());

        t.write("Assets/DLC/Shared/Highlights.xml", b"shared");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(text(&src, "highlights.xml"), "shared");
    }

    #[test]
    fn lekmod_folder_names_of_the_same_version_are_accepted() {
        for ok in [
            "LEKMOD_v34.15",
            "lekmod_V34.15",
            "LEKMODv34_15",
            "LEKMOD v34.15",
            "LEKMOD-v34-15",
            "LEKMOD",
        ] {
            assert!(is_lekmod_dir(ok), "{ok}");
        }
        for no in [
            "LEKMOD_v34.16",
            "LEKMODv34_1",
            "LEKMOD_v33.15",
            "LEKMOD_old",
            "Lekmap v4_5",
            "UI_bc1",
        ] {
            assert!(!is_lekmod_dir(no), "{no}");
        }
        // The exact name wins over another spelling; otherwise the first in sorted order.
        assert_eq!(
            pick_lekmod_dir(["LEKMOD", "LEKMODv34_15", "lekmod_v34.15"]),
            Some("lekmod_v34.15")
        );
        assert_eq!(
            pick_lekmod_dir(["LEKMODv34_15", "LEKMOD_v34.16"]),
            Some("LEKMODv34_15")
        );
        assert_eq!(pick_lekmod_dir(["LEKMOD_v34.16", "Shared"]), None);
    }

    #[test]
    fn a_differently_named_lekmod_folder_is_read_and_reported() {
        let t = TempTree::new();
        fs::create_dir_all(t.0.join("Resource")).unwrap();
        t.write("Assets/DLC/Expansion2/order.txt", b"exp2");
        t.write("Assets/DLC/LEKMODv34_15/order.txt", b"lekmod");
        t.write("Assets/DLC/LEKMOD_v34.16/v16.txt", b"refused");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(src.lekmod_dir(), Some("LEKMODv34_15"));
        assert_eq!(text(&src, "order.txt"), "lekmod");
        assert_eq!(src.lookup("order.txt").unwrap().layer, Layer::Lekmod);
        assert!(src.lookup("v16.txt").is_none());

        // No Lekmod folder at all: the index opens (other tools need it) and says so.
        let t2 = TempTree::new();
        fs::create_dir_all(t2.0.join("Resource")).unwrap();
        t2.write("Assets/DLC/LEKMOD_v34.16/v16.txt", b"refused");
        let src = GameSource::open(&t2.0).unwrap();
        assert_eq!(src.lekmod_dir(), None);
        assert_eq!(src.stats().excluded[0].dir, "LEKMOD_v34.16");
    }

    #[test]
    fn common_paks_are_read_from_the_package_text() {
        assert_eq!(common_paks(PKG_EXP2), ["Exp2Units.fpk"]);
        assert!(common_paks("<Gameplay><File>x</File></Gameplay>").is_empty());
        assert_eq!(
            common_paks(
                "<Common><Paks><File> A.fpk </File><File>B.fpk</File></Paks></Common><X><File>no</File></X>"
            ),
            ["A.fpk", "B.fpk"]
        );
    }

    #[test]
    fn low_quality_packs_and_other_mods_are_excluded() {
        let t = fake_install();
        let src = GameSource::open(&t.0).unwrap();

        assert!(src.lookup("low_only.dds").is_none());
        assert_eq!(src.stats().packs, 2);
        let excluded: Vec<_> = src
            .stats()
            .excluded
            .iter()
            .map(|e| e.dir.as_str())
            .collect();
        assert_eq!(excluded, ["LEKMOD_v34.16", "OtherMod", "Tablet"]);
        // Other mod folders, including other Lekmod versions, are not read.
        assert!(src.lookup("lekmod_34_16.dat").is_none());
        assert!(src.lookup("other_mod.dat").is_none());
    }

    #[test]
    fn low_packs_are_a_layer_of_their_own() {
        let t = fake_install();
        let src = GameSource::open(&t.0).unwrap();
        let low = src.lookup_low("TEX.dds").unwrap();
        assert_eq!(src.read(low).unwrap(), b"LOW");
        assert!(src.lookup_low("low_only.dds").is_some());
        assert!(src.lookup_low("only_pack.gr2").is_none());
        // The visible layers and their statistics do not see it.
        assert_eq!(text(&src, "tex.dds"), "dds");
        assert!(src.lookup("low_only.dds").is_none());
        assert_eq!(src.stats().packs, 2);
        assert_eq!(src.stats().packed["dds"].count, 1);
    }

    #[test]
    fn copies_lists_the_visible_file_first_then_the_hidden_ones() {
        let t = fake_install();
        let src = GameSource::open(&t.0).unwrap();
        let layers: Vec<Layer> = src.copies("SHARED.txt").iter().map(|e| e.layer).collect();
        assert_eq!(layers[0], Layer::Lekmod);
        assert_eq!(
            layers.len(),
            6,
            "the visible one and the five it hides: {layers:?}"
        );
        assert!(layers.contains(&Layer::Lekmod) && layers.contains(&Layer::Expansion));
        assert!(src.copies("nothing.txt").is_empty());
        assert_eq!(src.copies("tex.dds").len(), 1);
    }

    #[test]
    fn shadowed_entries_are_recorded_and_stats_count_them() {
        let t = fake_install();
        let src = GameSource::open(&t.0).unwrap();
        // shared.txt: lekmod, common, dx9, loose base, exp1 are shadowed; dlc.xml: dlc_01.
        assert_eq!(src.shadowed().len(), 6);
        assert_eq!(src.stats().shadowed, 6);
        assert!(src.shadowed().iter().any(|e| e.layer == Layer::Expansion));

        let st = src.stats();
        assert_eq!(st.packed["txt"].count, 2);
        assert_eq!(st.packed["dds"].count, 1);
        assert_eq!(st.loose["txt"].count, 4);
        assert_eq!(st.loose["xml"].bytes, 4 + 10);
    }

    #[test]
    fn iterates_by_extension_in_name_order() {
        let t = fake_install();
        let src = GameSource::open(&t.0).unwrap();
        let names: Vec<_> = src.iter_ext("TXT").map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["shared.txt"]);
        assert_eq!(src.iter_ext("gr2").count(), 1);
    }

    #[test]
    fn broken_pack_is_reported_not_fatal() {
        let t = fake_install();
        t.write("Resource/Common/broken.fpk", b"not a pack at all");
        let src = GameSource::open(&t.0).unwrap();
        assert_eq!(src.stats().pack_errors.len(), 1);
        assert_eq!(src.stats().packs, 2);
    }

    #[test]
    fn rejects_non_install() {
        let t = TempTree::new();
        assert!(matches!(
            GameSource::open(&t.0),
            Err(GameError::NotGameRoot(_))
        ));
    }
}
