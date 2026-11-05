//! The UI file system: layers by `*.Civ5Pkg` priority, lookup by bare file name
//!.
//!
//! A layer is the root (`Assets/` without `DLC`, plus the packs no package names) or one package
//! folder with the packs it lists. Layers are ordered by priority; a later layer hides an
//! earlier one. Inside a layer a loose file hides a pack entry, except in the root, where only
//! `.xml` and `.ggxml` do (RW-9d, as `GameSource`). Keys are lower-case file names without
//! directories.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use super::pkg::{self, Activation, Package, PkgError, norm_rel};
use crate::game::fpk::{self, FpkEntry, FpkError};

/// Folders whose files are not part of the UI file system: scenario folders carry UI files of
/// the same names (`TopPanel.lua`) that apply only inside the scenario.
const SKIP_DIRS: &[&str] = &["scenarios"];

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("{0} is not a Civ5 install (no Assets/ directory)")]
    NotGameRoot(PathBuf),
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Pkg(#[from] PkgError),
    #[error("pack {path}: {source}")]
    Pack {
        path: PathBuf,
        #[source]
        source: FpkError,
    },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> VfsError + '_ {
    move |source| VfsError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Settings of one VFS build.
#[derive(Debug, Clone)]
pub struct UiVfsConfig {
    pub game: PathBuf,
    pub activation: Activation,
    /// Priority of a package that has no `<Priority>`.
    pub default_priority: i32,
    /// Priority by package stem or folder name (lower case); wins over the file.
    pub priority_overrides: BTreeMap<String, i32>,
    /// Index the `Override` folder of a package.
    pub include_override: bool,
    /// Priority of the loose files that sit in `Assets/DLC` outside any package.
    pub orphan_priority: i32,
}

impl UiVfsConfig {
    pub fn new(game: impl Into<PathBuf>, set: &str) -> Option<Self> {
        Some(Self {
            game: game.into(),
            activation: Activation::up_to(set)?,
            default_priority: 0,
            priority_overrides: BTreeMap::new(),
            include_override: false,
            orphan_priority: 250,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LayerKind {
    Root,
    /// Loose files in `Assets/DLC` outside any package folder.
    Orphan,
    Package,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiLayer {
    /// `base` for the root, else the folder name (`Expansion2`, `UI_bc1`).
    pub name: String,
    pub kind: LayerKind,
    pub dir: PathBuf,
    pub priority: i32,
    /// Stems of the active `.Civ5Pkg` files of the folder.
    pub packages: Vec<String>,
    /// The `<Priority>` was missing and the default applied.
    pub priority_defaulted: bool,
    /// Pack file names of the layer in override order.
    pub packs: Vec<String>,
    /// `Style` and `Color` files of its active skins, as written.
    pub style_files: Vec<String>,
    pub color_files: Vec<String>,
    /// `Directory` entries of its active skins (lower case, `/`).
    pub skin_dirs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Packed { pack: u32, offset: u32, size: u32 },
    Loose { path: PathBuf, size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiEntry {
    /// Lower-case file name.
    pub name: String,
    /// Index into [`UiVfs::layers`].
    pub layer: usize,
    /// Path inside the layer, `/` separated, original case; `pack.fpk` for a packed entry.
    pub rel: String,
    pub origin: Origin,
    /// Order of discovery inside the layer; the later one hides the earlier one.
    seq: u64,
}

impl UiEntry {
    pub fn size(&self) -> u64 {
        match &self.origin {
            Origin::Packed { size, .. } => u64::from(*size),
            Origin::Loose { size, .. } => *size,
        }
    }

    pub fn is_packed(&self) -> bool {
        matches!(self.origin, Origin::Packed { .. })
    }

    /// Lower-case extension without the dot.
    pub fn extension(&self) -> &str {
        ext_of(&self.name)
    }
}

pub fn ext_of(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, e)| e)
}

/// A file of an `Override` folder that was left out of the index (or put in, with the flag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideFile {
    pub layer: usize,
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPackage {
    pub stem: String,
    pub dir: PathBuf,
    pub reason: String,
}

#[derive(Debug)]
pub struct UiVfs {
    pub layers: Vec<UiLayer>,
    packs: Vec<PathBuf>,
    /// Name to copies, ascending precedence: the last one is visible.
    entries: BTreeMap<String, Vec<UiEntry>>,
    pub override_files: Vec<OverrideFile>,
    pub skipped_packages: Vec<SkippedPackage>,
    /// Packs a package lists that the install lacks: (package stem, pack).
    pub missing_packs: Vec<(String, String)>,
    pub pack_errors: Vec<(PathBuf, String)>,
    pub package_errors: Vec<String>,
    /// Files under `Scenarios` folders, which are left out (a scenario brings its own UI).
    pub skipped_scenario_files: usize,
    pub config: UiVfsConfig,
}

/// Folder groups of the active packages: dir, packages in it.
struct Group<'a> {
    dir: PathBuf,
    pkgs: Vec<&'a Package>,
}

/// A part of a package folder that is a layer of its own: the packages of one priority and the top-level
/// directories whose highest listing package has that priority. EUI is three packages in one folder (`EUI_0`,
/// `EUI_1`, `EUI_2`: 50, 150, 250); `Art0` (blank textures that keep the base game quiet) belongs to the first only
/// and must not hide the textures of the expansions (100, 200), which a layer of 250 would do.
struct Sub<'a> {
    priority: i32,
    /// No package of the part names a `<Priority>`.
    defaulted: bool,
    pkgs: Vec<&'a Package>,
    /// Top-level directory names (lower case) of the part.
    dirs: BTreeSet<String>,
}

/// The parts of a group, the highest priority first (the folder's own layer: the files that no directory claims
/// belong to it). A group whose directories are all claimed at the top priority is one part.
fn sublayers<'a>(g: &Group<'a>, config: &UiVfsConfig) -> Vec<Sub<'a>> {
    let own: Vec<(i32, bool)> = g
        .pkgs
        .iter()
        .map(|p| {
            let ov = config
                .priority_overrides
                .get(&p.stem.to_ascii_lowercase())
                .or_else(|| {
                    let dn = g.dir.file_name()?.to_string_lossy().to_ascii_lowercase();
                    config.priority_overrides.get(&dn)
                })
                .copied();
            match ov.or(p.info.priority) {
                Some(v) => (v, true),
                None => (config.default_priority, false),
            }
        })
        .collect();
    let top = own
        .iter()
        .map(|o| o.0)
        .max()
        .unwrap_or(config.default_priority);
    let mut dir_prio: BTreeMap<String, i32> = BTreeMap::new();
    for (p, (v, _)) in g.pkgs.iter().zip(&own) {
        for d in p.info.active_directories(&config.activation) {
            let first = d.split('/').next().unwrap_or("").to_owned();
            if !first.is_empty() {
                let e = dir_prio.entry(first).or_insert(*v);
                *e = (*e).max(*v);
            }
        }
    }
    let mut prios: BTreeSet<i32> = dir_prio.values().copied().collect();
    prios.insert(top);
    prios
        .into_iter()
        .rev()
        .map(|prio| {
            // The folder's own part lists every package; a lower part those of its priority.
            let idx: Vec<usize> = (0..g.pkgs.len())
                .filter(|i| prio == top || own[*i].0 == prio)
                .collect();
            Sub {
                priority: prio,
                defaulted: !idx.iter().any(|i| own[*i].1),
                pkgs: idx.iter().map(|i| g.pkgs[*i]).collect(),
                dirs: dir_prio
                    .iter()
                    .filter(|(_, v)| **v == prio)
                    .map(|(d, _)| d.clone())
                    .collect(),
            }
        })
        .collect()
}

impl UiVfs {
    pub fn open(config: UiVfsConfig) -> Result<Self, VfsError> {
        let assets = config.game.join("Assets");
        if !assets.is_dir() {
            return Err(VfsError::NotGameRoot(config.game.clone()));
        }
        let dlc = assets.join("DLC");
        let (packages, bad) = if dlc.is_dir() {
            pkg::find_packages(&dlc)?
        } else {
            (Vec::new(), Vec::new())
        };
        let mut vfs = UiVfs {
            layers: Vec::new(),
            packs: Vec::new(),
            entries: BTreeMap::new(),
            override_files: Vec::new(),
            skipped_packages: Vec::new(),
            missing_packs: Vec::new(),
            pack_errors: Vec::new(),
            package_errors: bad.iter().map(ToString::to_string).collect(),
            skipped_scenario_files: 0,
            config,
        };

        // Active packages, grouped by folder.
        let mut groups: Vec<Group<'_>> = Vec::new();
        for p in &packages {
            if !p.info.is_active(&vfs.config.activation) {
                vfs.skipped_packages.push(SkippedPackage {
                    stem: p.stem.clone(),
                    dir: p.dir.clone(),
                    reason: "no UISkin of the package is active for the chosen set and platform"
                        .to_owned(),
                });
                continue;
            }
            match groups.iter_mut().find(|g| g.dir == p.dir) {
                Some(g) => g.pkgs.push(p),
                None => groups.push(Group {
                    dir: p.dir.clone(),
                    pkgs: vec![p],
                }),
            }
        }

        // Layers, ordered by (priority, kind, folder name).
        let mut layers = vec![UiLayer {
            name: "base".to_owned(),
            kind: LayerKind::Root,
            dir: assets.clone(),
            priority: 0,
            packages: Vec::new(),
            priority_defaulted: false,
            packs: Vec::new(),
            style_files: Vec::new(),
            color_files: Vec::new(),
            skin_dirs: Vec::new(),
        }];
        let mut group_of_layer: BTreeMap<(PathBuf, i32), usize> = BTreeMap::new();
        for g in &groups {
            let subs = sublayers(g, &vfs.config);
            for (i, sub) in subs.iter().enumerate() {
                let mut name = g
                    .dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if i > 0 {
                    // The directories of the packages of lower priority (EUI's `Art0`) are a layer of their own.
                    name = format!("{name}@{}", sub.priority);
                }
                let mut style_files = Vec::new();
                let mut color_files = Vec::new();
                if i == 0 {
                    for p in &sub.pkgs {
                        let (s, c) = p.info.active_style_files(&vfs.config.activation);
                        style_files.extend(s);
                        color_files.extend(c);
                    }
                }
                layers.push(UiLayer {
                    name,
                    kind: LayerKind::Package,
                    dir: g.dir.clone(),
                    priority: sub.priority,
                    packages: sub.pkgs.iter().map(|p| p.stem.clone()).collect(),
                    priority_defaulted: sub.defaulted,
                    packs: Vec::new(),
                    style_files,
                    color_files,
                    skin_dirs: sub.dirs.iter().cloned().collect(),
                });
            }
        }
        let has_orphans = dlc.is_dir()
            && fs::read_dir(&dlc)
                .map_err(io_err(&dlc))?
                .filter_map(Result::ok)
                .any(|e| e.file_type().is_ok_and(|t| t.is_file()));
        if has_orphans {
            layers.push(UiLayer {
                name: "DLC".to_owned(),
                kind: LayerKind::Orphan,
                dir: dlc.clone(),
                priority: vfs.config.orphan_priority,
                packages: Vec::new(),
                priority_defaulted: false,
                packs: Vec::new(),
                style_files: Vec::new(),
                color_files: Vec::new(),
                skin_dirs: Vec::new(),
            });
        }
        layers.sort_by_key(|l| (l.priority, l.kind, l.name.to_ascii_lowercase()));
        for (i, l) in layers.iter().enumerate() {
            group_of_layer.insert((l.dir.clone(), l.priority), i);
        }
        vfs.layers = layers;

        // Folders that are layers of their own are not part of the layer around them.
        let layer_dirs: Vec<PathBuf> = vfs
            .layers
            .iter()
            .filter(|l| l.kind == LayerKind::Package)
            .map(|l| l.dir.clone())
            .collect();

        // Packs any package names (active or not) are not root packs.
        let claimed: BTreeSet<String> = packages
            .iter()
            .flat_map(|p| p.info.paks.iter().map(|n| n.to_ascii_lowercase()))
            .collect();

        let mut seq: u64 = 1;
        let mut ctx = Scan {
            vfs: &mut vfs,
            seq: &mut seq,
        };
        // Root: packs, then loose.
        for dir in ["Common", "DX9"] {
            ctx.add_root_packs(&dir_of(&config_game(&ctx), "Resource", dir), &claimed)?;
        }
        let overrides_dir = config_game(&ctx).join("Resource").join("Overrides");
        if overrides_dir.is_dir() {
            ctx.add_loose(0, &overrides_dir, &[], &layer_dirs, None, Rule::Plain)?;
        }
        ctx.add_loose(0, &assets, &[], &layer_dirs, Some(&dlc), Rule::RootAssets)?;

        // Package layers: packs they list, then the folder.
        for g in &groups {
            let subs = sublayers(g, &ctx.vfs.config);
            let top = group_of_layer[&(g.dir.clone(), subs[0].priority)];
            let mut excluded = BTreeSet::new();
            let mut kept = BTreeSet::new();
            let mut packs: Vec<String> = Vec::new();
            for p in &g.pkgs {
                excluded.extend(p.info.excluded_paths(&ctx.vfs.config.activation));
                kept.extend(p.info.active_paths(&ctx.vfs.config.activation));
                for name in p.info.active_paks(&ctx.vfs.config.activation) {
                    if !packs.iter().any(|x| x.eq_ignore_ascii_case(&name)) {
                        packs.push(name);
                    }
                }
            }
            let excluded: Vec<String> = excluded.difference(&kept).cloned().collect();
            let stem = g.pkgs[0].stem.clone();
            for name in packs {
                let found = ["DX9", "Common"].iter().find_map(|d| {
                    let f = config_game(&ctx).join("Resource").join(d).join(&name);
                    f.is_file().then_some(f)
                });
                match found {
                    Some(f) => ctx.add_pack(top, &f)?,
                    None => ctx.vfs.missing_packs.push((stem.clone(), name)),
                }
            }
            // The top layer holds the folder but for the directories of the lower ones; a lower one holds
            // only its own directories.
            let all_top: Vec<String> = sorted_dir(&g.dir)?
                .iter()
                .filter_map(|p| p.file_name())
                .map(|n| norm_rel(&n.to_string_lossy()))
                .collect();
            for (i, sub) in subs.iter().enumerate() {
                let layer = group_of_layer[&(g.dir.clone(), sub.priority)];
                let mut ex = excluded.clone();
                if i == 0 {
                    ex.extend(subs[1..].iter().flat_map(|s| s.dirs.iter().cloned()));
                } else {
                    ex.extend(all_top.iter().filter(|n| !sub.dirs.contains(*n)).cloned());
                }
                ctx.add_loose(
                    layer,
                    &g.dir,
                    &ex,
                    &layer_dirs,
                    None,
                    Rule::Package {
                        include_override: ctx.vfs.config.include_override,
                    },
                )?;
            }
        }
        if has_orphans {
            let layer = group_of_layer[&(dlc.clone(), ctx.vfs.config.orphan_priority)];
            ctx.add_orphans(layer, &dlc)?;
        }

        // Order the copies of each name: ascending (layer, discovery sequence).
        for copies in vfs.entries.values_mut() {
            copies.sort_by_key(|e| (e.layer, e.seq));
        }
        Ok(vfs)
    }

    /// The copies of `name`, hidden ones first, the visible one last.
    pub fn copies(&self, name: &str) -> &[UiEntry] {
        self.entries
            .get(&name.to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }

    /// The visible copy of a bare file name, any case.
    pub fn lookup(&self, name: &str) -> Option<&UiEntry> {
        self.copies(name).last()
    }

    /// The visible copy of a path as written in XML, Lua or the DB: `assets\UI\Art\X.dds`,
    /// `Assets/UI/x.lua`, or a bare name. Directories are ignored (Civ5 semantics).
    pub fn find_path(&self, path: &str) -> Option<&UiEntry> {
        self.lookup(bare_name(path))
    }

    /// `include("IconSupport")`: the first of `exts` that exists, in the order given.
    pub fn find_stem(&self, stem: &str, exts: &[&str]) -> Option<&UiEntry> {
        let stem = bare_name(stem);
        // `include("Name.lua")`: the extension is already there.
        if exts.iter().any(|e| {
            stem.to_ascii_lowercase()
                .ends_with(&format!(".{}", e.trim_start_matches('.')))
        }) {
            return self.lookup(stem);
        }
        exts.iter()
            .find_map(|e| self.lookup(&format!("{stem}.{}", e.trim_start_matches('.'))))
    }

    /// `include("Dir\%w+Popup.lua", true)`: the visible Lua files whose path (with `\`) the Lua pattern finds.
    pub fn find_wild(&self, pattern: &str) -> Vec<&UiEntry> {
        let pat = pattern.replace('/', "\\");
        let last = pat.rsplit('\\').next().unwrap_or(&pat).to_owned();
        let has_dirs = pat.contains('\\');
        self.iter_visible()
            .filter(|e| e.extension() == "lua")
            .filter(|e| {
                let full = e.rel.replace('/', "\\");
                if super::refs::lua_pattern_matches(&full, &pat) {
                    return true;
                }
                // SIMPLIFIED(SIMP-exe-74): as the client's include: a Lua file of another directory with no XML twin (a layout, not a context) counts.
                has_dirs
                    && self
                        .lookup(&format!("{}.xml", e.name.trim_end_matches(".lua")))
                        .is_none()
                    && super::refs::lua_pattern_matches(
                        e.rel.rsplit('/').next().unwrap_or(&e.rel),
                        &last,
                    )
            })
            .collect()
    }

    pub fn layer(&self, entry: &UiEntry) -> &UiLayer {
        &self.layers[entry.layer]
    }

    /// Visible entries, sorted by name.
    pub fn iter_visible(&self) -> impl Iterator<Item = &UiEntry> {
        self.entries.values().filter_map(|c| c.last())
    }

    /// Every name with all its copies.
    pub fn iter_names(&self) -> impl Iterator<Item = (&String, &Vec<UiEntry>)> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The file or pack an entry comes from.
    pub fn origin_path(&self, e: &UiEntry) -> Option<PathBuf> {
        match &e.origin {
            Origin::Loose { path, .. } => Some(path.clone()),
            Origin::Packed { pack, .. } => self.packs.get(*pack as usize).cloned(),
        }
    }

    pub fn read(&self, e: &UiEntry) -> Result<Vec<u8>, VfsError> {
        match &e.origin {
            Origin::Loose { path, .. } => fs::read(path).map_err(io_err(path)),
            Origin::Packed { pack, offset, size } => {
                let path = &self.packs[*pack as usize];
                let mut f = File::open(path).map_err(io_err(path))?;
                let fe = FpkEntry {
                    name: e.name.clone(),
                    size: *size,
                    offset: *offset,
                };
                fpk::read_payload(&mut f, &fe).map_err(|source| VfsError::Pack {
                    path: path.clone(),
                    source,
                })
            }
        }
    }

    /// Whether two copies hold the same bytes. Sizes first; equal sizes are read.
    pub fn same_bytes(&self, a: &UiEntry, b: &UiEntry) -> Result<bool, VfsError> {
        if a.size() != b.size() {
            return Ok(false);
        }
        Ok(self.read(a)? == self.read(b)?)
    }
}

/// Last path segment, lower case.
pub fn bare_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path).trim()
}

fn config_game(ctx: &Scan<'_>) -> PathBuf {
    ctx.vfs.config.game.clone()
}

fn dir_of(game: &Path, a: &str, b: &str) -> PathBuf {
    game.join(a).join(b)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    /// A loose file hides everything below it.
    Plain,
    /// RW-9d: a loose file that is not `.xml`/`.ggxml` loses to a packed copy of the root.
    RootAssets,
    Package {
        include_override: bool,
    },
}

struct Scan<'a> {
    vfs: &'a mut UiVfs,
    seq: &'a mut u64,
}

impl Scan<'_> {
    fn next(&mut self) -> u64 {
        let v = *self.seq;
        *self.seq += 1;
        v
    }

    fn insert(&mut self, mut e: UiEntry, seq: u64) {
        e.seq = seq;
        self.vfs.entries.entry(e.name.clone()).or_default().push(e);
    }

    fn add_pack(&mut self, layer: usize, path: &Path) -> Result<(), VfsError> {
        let entries = match fpk::open_directory(path) {
            Ok(e) => e,
            Err(err) => {
                self.vfs
                    .pack_errors
                    .push((path.to_path_buf(), err.to_string()));
                return Ok(());
            }
        };
        let pack = self.vfs.packs.len() as u32;
        self.vfs.packs.push(path.to_path_buf());
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.vfs.layers[layer].packs.push(file_name.clone());
        for e in entries {
            let seq = self.next();
            self.insert(
                UiEntry {
                    name: e.name.to_ascii_lowercase(),
                    layer,
                    rel: file_name.clone(),
                    origin: Origin::Packed {
                        pack,
                        offset: e.offset,
                        size: e.size,
                    },
                    seq: 0,
                },
                seq,
            );
        }
        Ok(())
    }

    fn add_root_packs(&mut self, dir: &Path, claimed: &BTreeSet<String>) -> Result<(), VfsError> {
        if !dir.is_dir() {
            return Ok(());
        }
        let mut files: Vec<PathBuf> = sorted_dir(dir)?
            .into_iter()
            .filter(|p| p.is_file() && has_ext(p, "fpk"))
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| claimed.contains(&n.to_string_lossy().to_ascii_lowercase()))
            })
            .collect();
        files.sort();
        for f in files {
            self.add_pack(0, &f)?;
        }
        Ok(())
    }

    /// Loose files of `dir` into `layer`. `excluded` are paths relative to `dir` (lower case) that
    /// leave the folder; `layer_dirs` are folders of other layers; `skip_top` a folder to leave out.
    fn add_loose(
        &mut self,
        layer: usize,
        dir: &Path,
        excluded: &[String],
        layer_dirs: &[PathBuf],
        skip_top: Option<&Path>,
        rule: Rule,
    ) -> Result<(), VfsError> {
        let mut files = Vec::new();
        walk(
            dir,
            dir,
            excluded,
            layer_dirs,
            skip_top,
            &mut files,
            &mut self.vfs.skipped_scenario_files,
        )?;
        for (path, rel, size) in files {
            let Some(name) = path
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
            else {
                continue;
            };
            let in_override = matches!(rule, Rule::Package { .. })
                && rel
                    .split('/')
                    .next()
                    .is_some_and(|d| d.eq_ignore_ascii_case("override"))
                && rel.contains('/');
            if in_override {
                self.vfs.override_files.push(OverrideFile {
                    layer,
                    name: name.clone(),
                    path: path.clone(),
                    size,
                });
                if !matches!(
                    rule,
                    Rule::Package {
                        include_override: true
                    }
                ) {
                    continue;
                }
            }
            let packed_in_root = rule == Rule::RootAssets
                && !matches!(ext_of(&name), "xml" | "ggxml")
                && self
                    .vfs
                    .entries
                    .get(&name)
                    .is_some_and(|c| c.iter().any(|e| e.layer == layer && e.is_packed()));
            let seq = if packed_in_root { 0 } else { self.next() };
            self.insert(
                UiEntry {
                    name,
                    layer,
                    rel,
                    origin: Origin::Loose { path, size },
                    seq: 0,
                },
                seq,
            );
        }
        Ok(())
    }

    fn add_orphans(&mut self, layer: usize, dlc: &Path) -> Result<(), VfsError> {
        for p in sorted_dir(dlc)? {
            let meta = fs::symlink_metadata(&p).map_err(io_err(&p))?;
            if !meta.is_file() {
                continue;
            }
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let seq = self.next();
            self.insert(
                UiEntry {
                    name: name.to_ascii_lowercase(),
                    layer,
                    rel: name,
                    origin: Origin::Loose {
                        path: p,
                        size: meta.len(),
                    },
                    seq: 0,
                },
                seq,
            );
        }
        Ok(())
    }
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn sorted_dir(dir: &Path) -> Result<Vec<PathBuf>, VfsError> {
    let mut v = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err(dir))?;
    v.sort();
    Ok(v)
}

/// Depth-first, sorted. Skips excluded relative paths, other layers' folders, and `skip_top`.
fn walk(
    root: &Path,
    dir: &Path,
    excluded: &[String],
    layer_dirs: &[PathBuf],
    skip_top: Option<&Path>,
    out: &mut Vec<(PathBuf, String, u64)>,
    skipped: &mut usize,
) -> Result<(), VfsError> {
    for path in sorted_dir(dir)? {
        let meta = fs::symlink_metadata(&path).map_err(io_err(&path))?;
        let rel = path
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let rel_norm = norm_rel(&rel);
        let is_excluded = excluded
            .iter()
            .any(|x| rel_norm == *x || rel_norm.starts_with(&format!("{x}/")));
        if is_excluded {
            continue;
        }
        if meta.is_dir() {
            if skip_top.is_some_and(|s| s == path) || (path != *root && layer_dirs.contains(&path))
            {
                continue;
            }
            if path
                .file_name()
                .is_some_and(|n| SKIP_DIRS.iter().any(|s| n.eq_ignore_ascii_case(s)))
            {
                let mut gone = Vec::new();
                walk(&path, &path, &[], &[], None, &mut gone, &mut 0)?;
                *skipped += gone.len();
                continue;
            }
            walk(root, &path, excluded, layer_dirs, None, out, skipped)?;
        } else if meta.is_file() {
            out.push((path, rel, meta.len()));
        }
    }
    Ok(())
}

/// One name with several copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collision {
    pub name: String,
    /// `layer` index of the visible copy and its size.
    pub winner: (usize, u64),
    /// Hidden copies, from the one just below the winner down.
    pub losers: Vec<(usize, u64)>,
    /// All copies hold the same bytes; `None` when not compared.
    pub identical: Option<bool>,
}

impl UiVfs {
    /// Names with more than one copy. With `compare`, the winner is compared with each hidden
    /// copy (equal sizes are read).
    pub fn collisions(&self, compare: bool) -> Result<Vec<Collision>, VfsError> {
        let mut out = Vec::new();
        for (name, copies) in &self.entries {
            if copies.len() < 2 {
                continue;
            }
            let win = copies.last().expect("len >= 2");
            let losers: Vec<&UiEntry> = copies[..copies.len() - 1].iter().rev().collect();
            let identical = if compare {
                let mut same = true;
                for l in &losers {
                    if !self.same_bytes(win, l)? {
                        same = false;
                        break;
                    }
                }
                Some(same)
            } else {
                None
            };
            out.push(Collision {
                name: name.clone(),
                winner: (win.layer, win.size()),
                losers: losers.iter().map(|l| (l.layer, l.size())).collect(),
                identical,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::fpk::testutil::build_fpk;
    use std::sync::atomic::{AtomicU32, Ordering};

    pub(crate) struct Tree(pub PathBuf);

    impl Tree {
        pub fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "civ5-ui-vfs-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&dir).unwrap();
            Tree(dir)
        }
        pub fn write(&self, rel: &str, bytes: &[u8]) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn pkg(priority: Option<i32>, set: Option<&str>, paks: &[&str]) -> String {
        let pr = priority.map_or(String::new(), |p| format!("<Priority>{p:03}</Priority>"));
        let skin = set.map_or(String::new(), |s| {
            format!(
                "<UISkin name=\"n\" set=\"{s}\" platform=\"Common\"><Skin><Directory>UI</Directory></Skin></UISkin>"
            )
        });
        let pk: String = paks.iter().map(|p| format!("<File>{p}</File>")).collect();
        format!("<Civ5Package>{pr}{skin}<Common><Paks>{pk}</Paks></Common></Civ5Package>")
    }

    fn text(v: &UiVfs, name: &str) -> String {
        String::from_utf8(v.read(v.lookup(name).unwrap()).unwrap()).unwrap()
    }

    fn open(t: &Tree, set: &str) -> UiVfs {
        UiVfs::open(UiVfsConfig::new(&t.0, set).unwrap()).unwrap()
    }

    /// Base, G&K (100), BNW (200), Shared (no priority), EUI (50/150/250 in one folder), Lekmod 300.
    fn install() -> Tree {
        let t = Tree::new();
        t.write("Assets/UI/Same.lua", b"base");
        t.write("Assets/UI/Only_base.lua", b"ob");
        t.write(
            "Assets/DLC/Expansion/Expansion1.Civ5Pkg",
            pkg(Some(100), Some("Expansion1"), &[]).as_bytes(),
        );
        t.write("Assets/DLC/Expansion/UI/Same.lua", b"gk");
        t.write(
            "Assets/DLC/Expansion2/Expansion2.Civ5Pkg",
            pkg(Some(200), Some("Expansion2"), &["Bnw.fpk"]).as_bytes(),
        );
        t.write("Assets/DLC/Expansion2/UI/Same.lua", b"bnw");
        t.write(
            "Assets/DLC/Shared/Upgrade1.Civ5Pkg",
            pkg(None, None, &[]).as_bytes(),
        );
        t.write("Assets/DLC/Shared/UI/Shared_only.lua", b"sh");
        for (f, p, s) in [
            ("EUI_0", 50, "BaseGame"),
            ("EUI_1", 150, "Expansion1"),
            ("EUI_2", 250, "Expansion2"),
        ] {
            t.write(
                &format!("Assets/DLC/UI_bc1/{f}.Civ5Pkg"),
                pkg(Some(p), Some(s), &[]).as_bytes(),
            );
        }
        t.write("Assets/DLC/UI_bc1/Core/Same.lua", b"eui");
        t.write("Assets/DLC/UI_bc1/Art0/old.dds", b"art0");
        t.write(
            "Assets/DLC/LEKMOD_v34.15/MPModsPack.Civ5Pkg",
            pkg(Some(300), Some("Expansion2"), &[]).as_bytes(),
        );
        t.write("Assets/DLC/LEKMOD_v34.15/Lua/UI/Same.lua", b"lek");
        t.write(
            "Resource/DX9/Bnw.fpk",
            &build_fpk(&[("Packed.dds", b"bnwpack"), ("Same.lua", b"inpack")]),
        );
        t.write(
            "Resource/DX9/Base.fpk",
            &build_fpk(&[("Packed.dds", b"basepack")]),
        );
        t.write(
            "Resource/Common/C.fpk",
            &build_fpk(&[("Packed.dds", b"common")]),
        );
        t
    }

    /// `Directory` entries make the skin: EUI's `Art0` (blank textures of the base game's skin, priority 50) must not hide the
    /// textures of the expansions the way the folder's highest priority (250) would.
    fn pkg_dirs(priority: i32, set: &str, dirs: &[&str]) -> String {
        let d: String = dirs
            .iter()
            .map(|d| format!("<Directory>{d}</Directory>"))
            .collect();
        format!(
            "<Civ5Package><Priority>{priority:03}</Priority><UISkin name=\"n\" set=\"{set}\" platform=\"Common\"><Skin>{d}</Skin></UISkin><Common><Paks></Paks></Common></Civ5Package>"
        )
    }

    #[test]
    fn a_skin_directory_has_the_priority_of_its_own_package() {
        let t = Tree::new();
        t.write(
            "Assets/DLC/Expansion2/Expansion2.Civ5Pkg",
            pkg(Some(200), Some("Expansion2"), &[]).as_bytes(),
        );
        t.write("Assets/DLC/Expansion2/UI/Notice.dds", b"real");
        t.write("Assets/DLC/Expansion2/UI/Plain.dds", b"real2");
        for (f, p, set, dirs) in [
            (
                "EUI_0",
                50,
                "BaseGame",
                &["Art", "Art0", "Art1", "Core"][..],
            ),
            ("EUI_1", 150, "Expansion1", &["Art", "Art1", "Core"][..]),
            ("EUI_2", 250, "Expansion2", &["Art", "Art2", "Core"][..]),
        ] {
            t.write(
                &format!("Assets/DLC/UI_bc1/{f}.Civ5Pkg"),
                pkg_dirs(p, set, dirs).as_bytes(),
            );
        }
        t.write("Assets/DLC/UI_bc1/Art0/Notice.dds", b"blank0");
        t.write("Assets/DLC/UI_bc1/Art1/Plain.dds", b"blank1");
        t.write("Assets/DLC/UI_bc1/Art2/Notice2.dds", b"eui2");
        t.write("Assets/DLC/UI_bc1/Core/Core.lua", b"core");
        let v = open(&t, "expansion2");
        let layers: Vec<(&str, i32)> = v
            .layers
            .iter()
            .map(|l| (l.name.as_str(), l.priority))
            .collect();
        assert_eq!(
            layers,
            [
                ("base", 0),
                ("UI_bc1@50", 50),
                ("UI_bc1@150", 150),
                ("Expansion2", 200),
                ("UI_bc1", 250)
            ]
        );
        // Art0 (50) loses to the real texture of the expansion (200); Art1 (150) too.
        assert_eq!(text(&v, "Notice.dds"), "real");
        assert_eq!(text(&v, "Plain.dds"), "real2");
        // Art2 and Core are the folder's own (250).
        assert_eq!(v.layer(v.lookup("Notice2.dds").unwrap()).name, "UI_bc1");
        assert_eq!(v.layer(v.lookup("Core.lua").unwrap()).name, "UI_bc1");
        // A file is in one layer only.
        assert_eq!(v.copies("core.lua").len(), 1);
        assert_eq!(v.copies("notice.dds").len(), 2);
    }

    #[test]
    fn layers_follow_priority_not_path() {
        let t = install();
        let v = open(&t, "expansion2");
        let order: Vec<(&str, i32)> = v
            .layers
            .iter()
            .map(|l| (l.name.as_str(), l.priority))
            .collect();
        assert_eq!(
            order,
            [
                ("base", 0),
                ("Shared", 0),
                ("Expansion", 100),
                ("Expansion2", 200),
                ("UI_bc1", 250),
                ("LEKMOD_v34.15", 300)
            ]
        );
        assert!(v.layers[1].priority_defaulted);
        // Lekmod 300 beats EUI 250 beats BNW 200 beats G&K 100 beats base.
        assert_eq!(text(&v, "Same.lua"), "lek");
        let c = v.copies("same.lua");
        let names: Vec<&str> = c.iter().map(|e| v.layer(e).name.as_str()).collect();
        assert_eq!(
            names,
            [
                "base",
                "Expansion",
                "Expansion2",
                "Expansion2",
                "UI_bc1",
                "LEKMOD_v34.15"
            ]
        );
    }

    #[test]
    fn a_pack_of_the_package_sits_below_its_loose_files() {
        let t = install();
        let v = open(&t, "expansion2");
        let c = v.copies("same.lua");
        // In Expansion2 the pack copy comes before the loose copy.
        let bnw: Vec<_> = c
            .iter()
            .filter(|e| v.layer(e).name == "Expansion2")
            .collect();
        assert!(bnw[0].is_packed() && !bnw[1].is_packed());
        assert_eq!(v.layers[3].packs, ["Bnw.fpk"]);
    }

    #[test]
    fn root_packs_dx9_beats_common_and_claimed_packs_leave_the_root() {
        let t = install();
        let v = open(&t, "expansion2");
        // `Bnw.fpk` is named by Expansion2 and wins over the root packs.
        assert_eq!(text(&v, "packed.dds"), "bnwpack");
        let layers: Vec<&str> = v
            .copies("packed.dds")
            .iter()
            .map(|e| v.layer(e).name.as_str())
            .collect();
        assert_eq!(layers, ["base", "base", "Expansion2"]);
        // Under the vanilla set Expansion2 is out, and so is its pack.
        let v = open(&t, "basegame");
        assert_eq!(text(&v, "packed.dds"), "basepack");
    }

    #[test]
    fn the_set_decides_which_packages_count() {
        let t = install();
        let v = open(&t, "basegame");
        assert!(
            v.layers
                .iter()
                .all(|l| l.name != "Expansion2" && l.name != "Expansion")
        );
        let eui = v.layers.iter().find(|l| l.name == "UI_bc1").unwrap();
        assert_eq!(eui.priority, 50);
        assert_eq!(eui.packages, ["EUI_0"]);
        assert_eq!(v.skipped_packages.len(), 5);
        let v = open(&t, "expansion1");
        let eui = v.layers.iter().find(|l| l.name == "UI_bc1").unwrap();
        assert_eq!((eui.priority, eui.packages.len()), (150, 2));
    }

    #[test]
    fn one_folder_with_three_packages_is_one_layer_and_one_copy() {
        let t = install();
        let v = open(&t, "expansion2");
        assert_eq!(v.layers.iter().filter(|l| l.name == "UI_bc1").count(), 1);
        assert_eq!(v.copies("old.dds").len(), 1);
    }

    #[test]
    fn root_loose_non_xml_loses_to_the_root_pack() {
        let t = Tree::new();
        t.write(
            "Resource/DX9/a.fpk",
            &build_fpk(&[("x.dds", b"pack"), ("y.xml", b"pack")]),
        );
        t.write("Assets/UI/x.dds", b"loose");
        t.write("Assets/UI/y.xml", b"loose");
        let v = open(&t, "expansion2");
        assert_eq!(text(&v, "x.dds"), "pack");
        assert_eq!(text(&v, "y.xml"), "loose");
    }

    #[test]
    fn lookup_ignores_directories_and_case() {
        let t = install();
        let v = open(&t, "expansion2");
        assert!(v.find_path("assets\\UI\\Art\\SAME.LUA").is_some());
        assert_eq!(v.find_path("Same.lua").unwrap().name, "same.lua");
        assert_eq!(
            v.find_stem("Only_base", &["xml", "lua"]).unwrap().name,
            "only_base.lua"
        );
        assert!(v.find_path("nothing.lua").is_none());
    }

    #[test]
    fn tablet_directories_and_nested_layers_stay_out() {
        let t = Tree::new();
        let p = r#"<Civ5Package><Priority>200</Priority>
          <UISkin name="a" set="Expansion2" platform="Common"><Skin><Directory>UI</Directory></Skin></UISkin>
          <UISkin name="a" set="Expansion2" platform="Tablet"><Skin><Directory>UI_Tablet</Directory></Skin></UISkin></Civ5Package>"#;
        t.write("Assets/DLC/Expansion2/Expansion2.Civ5Pkg", p.as_bytes());
        t.write("Assets/DLC/Expansion2/UI/InGame.xml", b"desktop");
        t.write("Assets/DLC/Expansion2/UI_Tablet/InGame.xml", b"tablet");
        t.write("Assets/DLC/Expansion2/UI_Tablet/Tablet_only.xml", b"t");
        // A package folder inside another one is its own layer.
        t.write(
            "Assets/DLC/Expansion2/Inner/Inner.Civ5Pkg",
            pkg(Some(10), None, &[]).as_bytes(),
        );
        t.write("Assets/DLC/Expansion2/Inner/in.lua", b"inner");
        let v = open(&t, "expansion2");
        assert_eq!(text(&v, "InGame.xml"), "desktop");
        assert!(v.lookup("tablet_only.xml").is_none());
        let in_lua = v.lookup("in.lua").unwrap();
        assert_eq!(v.layer(in_lua).name, "Inner");
    }

    #[test]
    fn override_folder_is_listed_and_indexed_only_on_request() {
        let t = Tree::new();
        t.write("Assets/Gameplay/XML/CIV5Units.xml", b"vanilla");
        t.write(
            "Assets/DLC/LEKMOD/M.Civ5Pkg",
            pkg(Some(300), None, &[]).as_bytes(),
        );
        t.write("Assets/DLC/LEKMOD/Override/CIV5Units.xml", b"");
        t.write("Assets/DLC/LEKMOD/Override/New.xml", b"new");
        t.write("Assets/DLC/LEKMOD/Lua/x.lua", b"l");
        let v = open(&t, "expansion2");
        assert_eq!(text(&v, "civ5units.xml"), "vanilla");
        assert!(v.lookup("new.xml").is_none());
        assert_eq!(v.override_files.len(), 2);
        let mut cfg = UiVfsConfig::new(&t.0, "expansion2").unwrap();
        cfg.include_override = true;
        let v = UiVfs::open(cfg).unwrap();
        assert_eq!(text(&v, "civ5units.xml"), "");
        assert_eq!(text(&v, "new.xml"), "new");
    }

    #[test]
    fn priority_override_and_default_apply() {
        let t = install();
        let mut cfg = UiVfsConfig::new(&t.0, "expansion2").unwrap();
        cfg.priority_overrides.insert("upgrade1".to_owned(), 210);
        let v = UiVfs::open(cfg).unwrap();
        let shared = v.layers.iter().find(|l| l.name == "Shared").unwrap();
        assert_eq!(shared.priority, 210);
        assert!(!shared.priority_defaulted);
        let idx: Vec<&str> = v.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(
            idx,
            [
                "base",
                "Expansion",
                "Expansion2",
                "Shared",
                "UI_bc1",
                "LEKMOD_v34.15"
            ]
        );
    }

    #[test]
    fn collisions_report_winner_losers_and_equality() {
        let t = install();
        t.write("Assets/DLC/Shared/UI/Dup.lua", b"same");
        t.write("Assets/UI/Dup.lua", b"same");
        let v = open(&t, "expansion2");
        let cs = v.collisions(true).unwrap();
        let dup = cs.iter().find(|c| c.name == "dup.lua").unwrap();
        assert_eq!(dup.identical, Some(true));
        assert_eq!(dup.losers.len(), 1);
        let same = cs.iter().find(|c| c.name == "same.lua").unwrap();
        assert_eq!(same.identical, Some(false));
        assert_eq!(v.layers[same.winner.0].name, "LEKMOD_v34.15");
    }

    #[test]
    fn scenario_folders_are_left_out() {
        let t = Tree::new();
        t.write("Assets/UI/TopPanel.lua", b"base");
        t.write(
            "Assets/DLC/DLC_07/C.Civ5Pkg",
            pkg(Some(300), None, &[]).as_bytes(),
        );
        t.write(
            "Assets/DLC/DLC_07/Scenarios/Conquest/UI/TopPanel.lua",
            b"scenario",
        );
        t.write("Assets/DLC/DLC_07/Scenarios/Conquest/UI/Other.lua", b"o");
        let v = open(&t, "expansion2");
        assert_eq!(text(&v, "toppanel.lua"), "base");
        assert!(v.lookup("other.lua").is_none());
        assert_eq!(v.skipped_scenario_files, 2);
    }

    #[test]
    fn orphan_files_in_dlc_get_their_own_layer() {
        let t = Tree::new();
        t.write("Assets/DLC/EUI_text_en_us.xml", b"txt");
        t.write(
            "Assets/DLC/UI_bc1/EUI_2.Civ5Pkg",
            pkg(Some(250), None, &[]).as_bytes(),
        );
        let v = open(&t, "expansion2");
        let e = v.lookup("eui_text_en_us.xml").unwrap();
        assert_eq!(v.layer(e).kind, LayerKind::Orphan);
        assert_eq!(v.layer(e).priority, 250);
    }
}
