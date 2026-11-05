//! Materials of B2 models: texture paths of the FGX block, the shader catalog names and the
//! model-set walk that checks every unit model.
//!
//! # Texture paths
//! `ParamDataT` holds the hash of the exporter's full source path
//! (`D:\projects\ArtDev\Civilization5\Units\Shared\Textures\Unit_Irradiance.dds`). The file name
//! is known from the `.fxsxml` of the model; the directory is not. [`PathResolver`] works with
//! hashes of directories:
//! 1. directories seen in clear in the named gr2 files (`MiscData`, `EffectData`, DLC) are seeded;
//! 2. for a path hash `h` and a candidate file `f`, `name_hash_prefix(h, "\" + f)` is the hash of
//!    the directory `f` would sit in; when it is a known directory, `f` is the file;
//! 3. two paths of one model whose candidates give the same directory hash confirm each other
//!    (a 32-bit match; names of equal length prove nothing, as CRC is affine), and that
//!    directory becomes known to every other model ([`PathResolver::learn`], a first pass over
//!    all models, then [`PathResolver::resolve`]);
//! 4. a model's own folder is guessed as text: the roots seen in clear (`...\Units`) times the
//!    folder names the files suggest (`Archer_DIFF.dds` -> `Archer`) times the usual sub-folders
//!    (`Maps`, `Textures`). A guess counts when its hash equals the directory hash a candidate
//!    file asks for (32 bits checked per file), and the directory becomes known like in 3.
//!
//! Candidates are the file names of the `.fxsxml` in their spelling, lower case and upper case.
//! Install-wide names (lower case only) are tried against known directories as a last resort.
//! A path nothing explains stays `TexSource::Unresolved(hash)`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::{LogicalId, MaterialDesc, TexSource};
use granny::extended::{FgxMaterial, read_fgx_materials};
use granny::material::{Material as GrannyMaterial, NameRef, read_materials};
use granny::{GrannyFile, name_hash, name_hash_prefix};
use rayon::prelude::*;

use super::ModelError;
use super::fgx::{NameDict, NameRole, TexturePaths, Unresolved, fgx_to_desc};
use super::materials::{Evidence, MaterialPlan, TextureChoice, plan_materials};
use crate::art::{ArtDefines, ArtWarning, FileRole, FxsTexture, TextureSlot, resolve_fxsxml};
use crate::game::GameSource;

/// `tx:<lower-case file name>`.
pub fn texture_id(file: &str) -> LogicalId {
    LogicalId::new(format!("tx:{}", file.to_ascii_lowercase()))
}

fn file_part(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// How a path hash was explained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathEvidence {
    /// The path is just the file name.
    Bare,
    /// The directory is known from a named file or an earlier confirmation.
    KnownDir,
    /// Confirmed by a second path of the same model with the same directory.
    PairedDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// File name as spelled by the candidate.
    pub file: String,
    pub evidence: PathEvidence,
}

/// Spellings tried for a candidate file name: as written, lower, upper.
fn spellings(file: &str) -> Vec<String> {
    let mut v = vec![file.to_owned()];
    for s in [file.to_ascii_lowercase(), file.to_ascii_uppercase()] {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

/// Most tokens of a file stem whose case combinations are all tried.
const MAX_CASE_TOKENS: usize = 7;

/// Spellings of an install file name, which is lower case. The first three are the classic ones:
/// as is, each `_`-word capitalised (`Unit_Irradiance.dds`), upper case. With `mixed`, every
/// combination of lower / Capitalised / UPPER per `_`-word of the stem follows
/// (`Caravel_Dam_DIFF.dds`: artists mix the case word by word), with the extension as is or upper
/// case. A 32-bit match among that many spellings of every file of the install would often be
/// chance, so `mixed` is only asked for files named like the model (see
/// [`PathResolver::resolve`]). Stems with more than [`MAX_CASE_TOKENS`] words keep the classic
/// three.
fn install_spellings(file: &str, mixed: bool) -> Vec<String> {
    let title: String = file
        .split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("_");
    let mut v = vec![file.to_owned()];
    for s in [title, file.to_ascii_uppercase()] {
        if !v.contains(&s) {
            v.push(s);
        }
    }
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file, ""));
    // Words: the `_`-words of the stem, with the damage tags split off (`lightdmg` is written
    // `LightDMG`), each cased on its own. `joined[i]` says word i follows word i - 1 without `_`.
    let mut words: Vec<&str> = Vec::new();
    let mut joined: Vec<bool> = Vec::new();
    for tok in stem.split('_') {
        let cut = ["damage", "damg", "dmg", "dam"]
            .iter()
            .find(|t| tok.len() > t.len() && tok.ends_with(**t))
            .map(|t| tok.len() - t.len())
            .or_else(|| {
                ["heavy", "light"]
                    .iter()
                    .find(|t| tok.len() > t.len() && tok.starts_with(**t))
                    .map(|t| t.len())
            });
        match cut {
            Some(k) => {
                words.push(&tok[..k]);
                joined.push(false);
                words.push(&tok[k..]);
                joined.push(true);
            }
            None => {
                words.push(tok);
                joined.push(false);
            }
        }
    }
    if !mixed || words.len() > MAX_CASE_TOKENS {
        return v;
    }
    let case = |w: &str, k: usize| -> String {
        match k {
            0 => w.to_ascii_lowercase(),
            1 => {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase())
                    .unwrap_or_default()
            }
            _ => w.to_ascii_uppercase(),
        }
    };
    let exts: &[String] = &[ext.to_owned(), ext.to_ascii_uppercase()];
    let mut seen: BTreeSet<String> = v.iter().cloned().collect();
    let combos = 3usize.pow(words.len() as u32);
    for n in 0..combos {
        let mut rest = n;
        let mut stem = String::new();
        for (i, w) in words.iter().enumerate() {
            let k = rest % 3;
            rest /= 3;
            if i > 0 && !joined[i] {
                stem.push('_');
            }
            stem.push_str(&case(w, k));
        }
        for e in exts {
            let name = if ext.is_empty() {
                stem.clone()
            } else {
                format!("{stem}.{e}")
            };
            if seen.insert(name.clone()) {
                v.push(name);
            }
        }
    }
    v
}

/// Knowledge about the directories of texture paths.
#[derive(Debug, Clone, Default)]
pub struct PathResolver {
    dirs: BTreeSet<u32>,
    /// Clear-text directories that end in `Units`: where the model folders sit.
    roots: BTreeSet<String>,
    /// File names of the clear-text paths, in their spelling.
    names: BTreeSet<String>,
}

/// Roots known even when the named files show none of them (spellings seen in the files:
/// `ArtDev` / `artdev`, `Civilization5`, `Civilzation5_DLC_Expansion`, `Civilization5_XP2`).
const BASE_ROOTS: [&str; 6] = [
    r"D:\projects\ArtDev\Civilization5\Units",
    r"D:\projects\artdev\Civilization5\Units",
    r"D:\projects\artdev\Civilzation5_DLC_Expansion\Units",
    r"D:\projects\ArtDev\Civilzation5_DLC_Expansion\Units",
    r"D:\projects\artdev\Civilization5_XP2\Units",
    r"D:\projects\ArtDev\Civilization5_XP2\Units",
];

/// Sub-folders of a model folder.
const SUB_DIRS: [&str; 7] = [
    "",
    r"\Maps",
    r"\maps",
    r"\Textures",
    r"\textures",
    r"\Map",
    r"\Tex",
];

impl PathResolver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds everything `other` knows.
    pub fn merge(&mut self, other: &PathResolver) {
        self.dirs.extend(&other.dirs);
        self.roots.extend(other.roots.iter().cloned());
        self.names.extend(other.names.iter().cloned());
    }

    /// Number of known directory hashes.
    pub fn dir_count(&self) -> usize {
        self.dirs.len()
    }

    /// Remembers the directory of a path spelled out in a named file.
    pub fn seed_path(&mut self, path: &str) {
        if let Some((dir, _)) = path.rsplit_once('\\') {
            self.dirs.insert(name_hash(dir));
            let mut end = 0;
            for part in dir.split('\\') {
                end += part.len();
                if part.eq_ignore_ascii_case("units") {
                    self.roots.insert(dir[..end].to_owned());
                }
                end += 1;
            }
        }
    }

    /// Folder names the files of a model suggest: every `_`-prefix of each file stem and hint
    /// (`Archer_02_DIFF.dds` -> `Archer`, `Archer_02`, `Archer_02_DIFF`), as written and lower case.
    fn folder_names(candidates: &[String], hints: &[String]) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let stems = candidates
            .iter()
            .map(|c| c.rsplit_once('.').map_or(c.as_str(), |(s, _)| s))
            .chain(hints.iter().map(String::as_str));
        for stem in stems {
            let parts: Vec<&str> = stem.split('_').collect();
            for k in 1..=parts.len() {
                let name = parts[..k].join("_");
                out.insert(name.to_ascii_lowercase());
                out.insert(name);
            }
        }
        out
    }

    /// Hashes of the guessed model folders.
    fn guessed_dirs(&self, candidates: &[String], hints: &[String]) -> BTreeSet<u32> {
        let roots = BASE_ROOTS
            .iter()
            .copied()
            .chain(self.roots.iter().map(String::as_str));
        let names = Self::folder_names(candidates, hints);
        let mut out = BTreeSet::new();
        for root in roots {
            for name in &names {
                for sub in SUB_DIRS {
                    out.insert(name_hash(&format!("{root}\\{name}{sub}")));
                }
            }
        }
        out
    }

    /// Seeds the directories of every readable path of one gr2: material maps and FGX paths.
    pub fn seed_gr2(&mut self, fgx: &[Option<FgxMaterial>], materials: &[GrannyMaterial]) {
        for m in materials {
            for n in m
                .maps
                .iter()
                .map(|m| &m.texture_file)
                .chain([&m.texture_file])
            {
                if let Some(t) = &n.text {
                    self.seed_path(t);
                }
            }
        }
        for m in fgx.iter().flatten() {
            for s in m.present_sets() {
                for n in &s.data_t {
                    if let Some(t) = &n.text {
                        self.seed_path(t);
                    }
                }
            }
        }
    }

    /// Learns the directories this model's paths confirm: by pairs, and by guessed folders.
    /// `hints` are more folder-name seeds (the fxsxml and mesh file stems).
    pub fn learn(&mut self, hashes: &BTreeSet<u32>, candidates: &[String], hints: &[String]) {
        let (_, dirs) = pair_confirm(hashes, candidates);
        self.dirs.extend(dirs);
        let guesses = self.guessed_dirs(candidates, hints);
        for &h in hashes {
            for c in candidates {
                for s in spellings(c) {
                    let d = name_hash_prefix(h, format!("\\{s}").as_bytes());
                    if guesses.contains(&d) {
                        self.dirs.insert(d);
                    }
                }
            }
        }
    }

    /// Explains each hash it can. `extra` are install-wide file names tried against known
    /// directories only.
    pub fn resolve(
        &self,
        hashes: &BTreeSet<u32>,
        candidates: &[String],
        extra: &[String],
    ) -> BTreeMap<u32, ResolvedPath> {
        let mut out: BTreeMap<u32, ResolvedPath> = BTreeMap::new();
        let mut dirs = self.dirs.clone();
        let by_dirs = |dirs: &BTreeSet<u32>, h: u32| -> Option<ResolvedPath> {
            let mut found: BTreeMap<String, ResolvedPath> = BTreeMap::new();
            for c in candidates {
                for s in spellings(c) {
                    let evidence = if name_hash(&s) == h {
                        PathEvidence::Bare
                    } else if dirs.contains(&name_hash_prefix(h, format!("\\{s}").as_bytes())) {
                        PathEvidence::KnownDir
                    } else {
                        continue;
                    };
                    found.entry(c.to_ascii_lowercase()).or_insert(ResolvedPath {
                        file: c.clone(),
                        evidence,
                    });
                }
            }
            // Two different files for one hash would be a collision: trust neither.
            (found.len() == 1).then(|| found.into_values().next().expect("one"))
        };
        for &h in hashes {
            if let Some(r) = by_dirs(&dirs, h) {
                out.insert(h, r);
            }
        }
        let open: BTreeSet<u32> = hashes
            .iter()
            .copied()
            .filter(|h| !out.contains_key(h))
            .collect();
        let (paired, new_dirs) = pair_confirm(&open, candidates);
        for (h, file) in paired {
            out.insert(
                h,
                ResolvedPath {
                    file,
                    evidence: PathEvidence::PairedDir,
                },
            );
        }
        dirs.extend(new_dirs);
        let firsts: BTreeSet<String> = candidates.iter().filter_map(|c| first_word(c)).collect();
        for &h in &open {
            if out.contains_key(&h) {
                continue;
            }
            if let Some(r) = by_dirs(&dirs, h) {
                out.insert(h, r);
                continue;
            }
            let mut hits: BTreeSet<&str> = BTreeSet::new();
            for (i, f) in extra.iter().chain(&self.names).enumerate() {
                // Files named like the model (`caravel_dam_diff.dds` for `Caravel_DIFF.dds`) may
                // be spelled with any case per word; the rest only the usual three ways.
                let mixed = first_word(f).is_some_and(|w| firsts.contains(&w));
                // An install-wide file that is not named like the model is a 32-bit chance hit
                // (`esp_spy_rigginelection22-index.dds` for a SREF). Shared engine files
                // (`unit_irradiance.dds`) have plain names: no digit, no dash in the stem.
                let plain = !f
                    .split('.')
                    .next()
                    .unwrap_or(f)
                    .contains(|c: char| c.is_ascii_digit() || c == '-');
                if i < extra.len() && !mixed && !plain && !self.names.contains(f) {
                    continue;
                }
                for s in install_spellings(f, mixed) {
                    if dirs.contains(&name_hash_prefix(h, format!("\\{s}").as_bytes())) {
                        hits.insert(f.as_str());
                    }
                }
            }
            if let [one] = hits.iter().collect::<Vec<_>>()[..] {
                out.insert(
                    h,
                    ResolvedPath {
                        file: (*one).to_owned(),
                        evidence: PathEvidence::KnownDir,
                    },
                );
            }
        }
        out
    }
}

/// First `_`-word of a file name, lower case (`Caravel_DIFF.dds` -> `caravel`).
fn first_word(file: &str) -> Option<String> {
    let w = file.split(['_', '.']).next()?;
    (w.len() >= 3).then(|| w.to_ascii_lowercase())
}

/// Path hashes confirmed by a pair, and the directory hashes that confirmed them.
fn pair_confirm(
    hashes: &BTreeSet<u32>,
    candidates: &[String],
) -> (BTreeMap<u32, String>, BTreeSet<u32>) {
    // Directory hash -> (path hash, candidate index, suffix length, spelling).
    let mut by_dir: BTreeMap<u32, Vec<(u32, usize, usize)>> = BTreeMap::new();
    for &h in hashes {
        for (ci, c) in candidates.iter().enumerate() {
            for s in spellings(c) {
                let suffix = format!("\\{s}");
                let dir = name_hash_prefix(h, suffix.as_bytes());
                by_dir.entry(dir).or_default().push((h, ci, suffix.len()));
            }
        }
    }
    let mut confirmed: BTreeMap<u32, BTreeSet<usize>> = BTreeMap::new();
    let mut dirs = BTreeSet::new();
    for (dir, group) in &by_dir {
        for (i, &(ha, ca, la)) in group.iter().enumerate() {
            for &(hb, cb, lb) in &group[i + 1..] {
                if ha != hb && la != lb {
                    confirmed.entry(ha).or_default().insert(ca);
                    confirmed.entry(hb).or_default().insert(cb);
                    dirs.insert(*dir);
                }
            }
        }
    }
    let files = confirmed
        .into_iter()
        .filter_map(|(h, set)| {
            let names: BTreeSet<String> = set
                .iter()
                .map(|&c| candidates[c].to_ascii_lowercase())
                .collect();
            (names.len() == 1).then(|| (h, candidates[*set.first().expect("non-empty")].clone()))
        })
        .collect();
    (files, dirs)
}

/// The path hashes a gr2's FGX blocks refer to (non-empty, stripped).
pub fn path_hashes(fgx: &[Option<FgxMaterial>]) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for m in fgx.iter().flatten() {
        for s in m.present_sets() {
            for n in &s.data_t {
                if let (None, Some(id)) = (&n.text, n.id)
                    && id != name_hash("")
                {
                    out.insert(id);
                }
            }
        }
    }
    out
}

/// [`TexturePaths`] over the explained hashes of one material of a model.
struct ModelPaths<'a> {
    resolved: &'a BTreeMap<u32, ResolvedPath>,
    exists: &'a dyn Fn(&str) -> bool,
    /// File the model's planned material gives a sampler when the path itself is unexplained.
    infer: &'a dyn Fn(&str) -> Option<Inferred>,
    inferred: RefCell<Vec<(String, Inferred)>>,
    /// Files the plan names that the install does not ship, by sampler.
    missing: RefCell<Vec<(String, String)>>,
    /// The Granny material carries texture maps of its own: an FGX sampler with an empty path
    /// takes its file from the model's fxsxml by role (the Great Prophet, the WW1 infantry, the
    /// marine: every FGX path empty, `Diffuse` / `BuildingSREF` samplers, the files in the maps).
    granny_maps: bool,
}

impl TexturePaths for ModelPaths<'_> {
    fn empty(&self, sampler: &str) -> Option<TexSource> {
        if !self.granny_maps {
            return None;
        }
        let i = (self.infer)(sampler).filter(|i| (self.exists)(&i.file))?;
        let src = TexSource::File(texture_id(&i.file));
        self.inferred.borrow_mut().push((sampler.to_owned(), i));
        Some(src)
    }

    fn source(&self, sampler: &str, path: &NameRef) -> TexSource {
        let (file, hash) = match (&path.text, path.id) {
            (Some(t), _) => (Some(file_part(t).to_owned()), name_hash(t)),
            (None, Some(id)) => (self.resolved.get(&id).map(|r| r.file.clone()), id),
            (None, None) => return TexSource::Engine,
        };
        match file {
            Some(f) if (self.exists)(&f) => TexSource::File(texture_id(&f)),
            Some(_) => TexSource::Unresolved(hash),
            None => match (self.infer)(sampler) {
                Some(i) if (self.exists)(&i.file) => {
                    let src = TexSource::File(texture_id(&i.file));
                    self.inferred.borrow_mut().push((sampler.to_owned(), i));
                    src
                }
                Some(i) => {
                    self.missing.borrow_mut().push((sampler.to_owned(), i.file));
                    TexSource::Unresolved(hash)
                }
                None => TexSource::Unresolved(hash),
            },
        }
    }
}

/// A texture chosen from the model's fxsxml by the role of its sampler, not by its path
/// (the B1 material planner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inferred {
    pub file: String,
    /// `Elimination` or `Guess` evidence: the files do not decide.
    pub weak: bool,
}

/// What each sampler of a unit material is for, by its CTAB name.
fn planned_file(
    plan: &MaterialPlan,
    textures: &[FxsTexture],
    material: usize,
    sampler: &str,
) -> Option<Inferred> {
    let choice = |c: &Option<TextureChoice>| {
        c.as_ref().map(|c| Inferred {
            file: c.file.clone(),
            weak: matches!(c.evidence, Evidence::Elimination | Evidence::Guess),
        })
    };
    let planned = plan.granny.get(material)?;
    match sampler.to_ascii_lowercase().as_str() {
        "basetexturemap" | "diffuse" => choice(&planned.diffuse),
        // The sampler of a tree model (`forest_europe.dds` has no role token): the one texture
        // of the fxsxml that is not a lighting map.
        "basesampler" => choice(&planned.diffuse).or_else(|| {
            let mut it = textures.iter().filter(|t| t.slot == TextureSlot::Other);
            match (it.next(), it.next()) {
                (Some(t), None) => Some(Inferred {
                    file: t.file.clone(),
                    weak: true,
                }),
                _ => None,
            }
        }),
        // The height map of a ground decal: the one texture of the fxsxml named `..._H`.
        "heighttexturemap" => {
            let mut it = textures.iter().filter(|t| {
                let l = t.file.to_ascii_lowercase();
                let stem = l.rsplit_once('.').map_or(l.as_str(), |(s, _)| s);
                stem.ends_with("_h")
            });
            match (it.next(), it.next()) {
                (Some(t), None) => Some(Inferred {
                    file: t.file.clone(),
                    weak: true,
                }),
                _ => None,
            }
        }
        "srefmap" | "buildingsref" => choice(&planned.specular),
        "irradiancecubemap" | "irradiance" => {
            let mut it = textures
                .iter()
                .filter(|t| t.slot == TextureSlot::Irradiance);
            match (it.next(), it.next()) {
                (Some(t), None) => Some(Inferred {
                    file: t.file.clone(),
                    weak: false,
                }),
                _ => None,
            }
        }
        _ => None,
    }
}

/// A texture path that no file of the install explains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedTexture {
    pub material: usize,
    pub sampler: String,
    pub hash: u32,
    /// The file the path names when it is known but the install does not have it.
    pub missing_file: Option<String>,
}

/// A texture picked by sampler role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredTexture {
    pub material: usize,
    pub sampler: String,
    pub file: String,
    pub weak: bool,
}

/// The materials of one gr2.
#[derive(Debug, Clone, Default)]
pub struct ModelMaterials {
    /// One per Granny material; `None` where the material has no FGX block.
    pub descs: Vec<Option<MaterialDesc>>,
    pub unresolved: Vec<Unresolved>,
    pub unresolved_textures: Vec<UnresolvedTexture>,
    /// Textures chosen by sampler role where the path stayed unexplained.
    pub inferred: Vec<InferredTexture>,
    /// Paths explained, by kind of evidence.
    pub evidence: BTreeMap<PathEvidence, usize>,
}

/// Both lists of a gr2, parsed once.
pub struct Gr2Materials {
    pub fgx: Vec<Option<FgxMaterial>>,
    pub granny: Vec<GrannyMaterial>,
}

impl Gr2Materials {
    pub fn read(bytes: &[u8]) -> Result<Self, granny::GrannyError> {
        let file = GrannyFile::parse(bytes)?;
        Ok(Self {
            fgx: read_fgx_materials(&file)?,
            granny: read_materials(&file)?,
        })
    }
}

/// The ground decal of a landmark (`LandmarkShader`) whose base path no file explains: its colour is
/// the `_D` file beside its `_H` height map (`Barbarian_Camp2_Decal_D` beside `_Decal_H`), when the
/// fxsxml lists one. The plan of the Granny material guesses the diffuse of the model there
/// (`Barbarian_Huts_DIFF`, the atlas of the huts), which drawn flat on the ground is an atlas under
/// the camp. Every decal of the install pairs its `_D` and `_H` that way (wheat, camps, farms).
fn decal_base_from_height(
    desc: &mut MaterialDesc,
    textures: &[FxsTexture],
    inferred: &RefCell<Vec<(String, Inferred)>>,
    exists: &dyn Fn(&str) -> bool,
) {
    const BASE: &str = "BaseTextureMap";
    const HEIGHT: &str = "HeightTextureMap";
    if desc.package != "LandmarkShader" {
        return;
    }
    let mut list = inferred.borrow_mut();
    let Some(at) = list.iter().position(|(s, _)| s == BASE) else {
        return;
    };
    let height =
        desc.sets
            .iter()
            .flat_map(|s| &s.textures)
            .find_map(|t| match (&t.src, t.name == HEIGHT) {
                (TexSource::File(id), true) => {
                    Some(id.as_str().trim_start_matches("tx:").to_owned())
                }
                _ => None,
            });
    let Some(stem) = height.as_deref().and_then(|h| h.strip_suffix("_h.dds")) else {
        return;
    };
    let want = format!("{stem}_d.dds");
    let Some(file) = textures
        .iter()
        .map(|t| t.file.as_str())
        .find(|f| f.eq_ignore_ascii_case(&want))
        .filter(|f| exists(f))
    else {
        return;
    };
    for t in desc.sets.iter_mut().flat_map(|s| s.textures.iter_mut()) {
        if t.name == BASE {
            t.src = TexSource::File(texture_id(file));
        }
    }
    list[at].1 = Inferred {
        file: file.to_owned(),
        weak: true,
    };
}

/// The fxsxml side of a model.
pub struct ModelFiles<'a> {
    /// Flat fxsxml name and mesh file name (plan report keys).
    pub fxsxml: &'a str,
    pub mesh: &'a str,
    pub textures: &'a [FxsTexture],
}

/// Builds the [`MaterialDesc`] of every material of a gr2. `files` is the model's fxsxml;
/// `exists` tells whether the install has a texture file.
pub fn build_descs(
    mats: &Gr2Materials,
    files: &ModelFiles<'_>,
    resolver: &PathResolver,
    extra: &[String],
    dict: &NameDict,
    exists: &dyn Fn(&str) -> bool,
) -> Result<ModelMaterials, ModelError> {
    let mut candidates: Vec<String> = files.textures.iter().map(|t| t.file.clone()).collect();
    // The SREF a model shows beside its diffuse file is often absent from its fxsxml.
    let siblings: Vec<String> = candidates
        .iter()
        .filter(|c| c.to_ascii_lowercase().ends_with("_diff.dds"))
        .map(|c| format!("{}_sref.dds", &c[..c.len() - "_diff.dds".len()]))
        .filter(|c| !candidates.iter().any(|o| o.eq_ignore_ascii_case(c)))
        .collect();
    candidates.extend(siblings);
    let hashes = path_hashes(&mats.fgx);
    let resolved = resolver.resolve(&hashes, &candidates, extra);
    let plan = plan_materials(files.fxsxml, files.mesh, files.textures, &mats.granny);
    // `read_materials` skips null entries, so names and plans pair up only when the counts agree.
    let paired = mats.granny.len() == mats.fgx.len();
    let mut out = ModelMaterials::default();
    for r in resolved.values() {
        *out.evidence.entry(r.evidence).or_default() += 1;
    }
    for (i, slot) in mats.fgx.iter().enumerate() {
        let Some(fgx) = slot else {
            out.descs.push(None);
            continue;
        };
        let source_name = paired
            .then(|| mats.granny[i].name.text.clone())
            .flatten()
            .filter(|n| !n.is_empty());
        let infer = |sampler: &str| {
            if paired {
                planned_file(&plan, files.textures, i, sampler)
            } else {
                None
            }
        };
        let paths = ModelPaths {
            resolved: &resolved,
            exists,
            infer: &infer,
            inferred: RefCell::new(Vec::new()),
            missing: RefCell::new(Vec::new()),
            granny_maps: paired && !mats.granny[i].maps.is_empty(),
        };
        let mut c = fgx_to_desc(fgx, source_name, dict, &paths)?;
        decal_base_from_height(&mut c.desc, files.textures, &paths.inferred, exists);
        let missing = paths.missing.take();
        out.inferred.extend(
            paths
                .inferred
                .into_inner()
                .into_iter()
                .map(|(sampler, inf)| InferredTexture {
                    material: i,
                    sampler,
                    file: inf.file,
                    weak: inf.weak,
                }),
        );
        for set in &c.desc.sets {
            for t in &set.textures {
                if let TexSource::Unresolved(hash) = t.src {
                    out.unresolved_textures.push(UnresolvedTexture {
                        material: i,
                        sampler: t.name.clone(),
                        hash,
                        missing_file: resolved.get(&hash).map(|r| r.file.clone()).or_else(|| {
                            missing
                                .iter()
                                .find(|(sampler, _)| *sampler == t.name)
                                .map(|(_, f)| f.clone())
                        }),
                    });
                }
            }
        }
        out.unresolved.extend(c.unresolved);
        out.descs.push(Some(c.desc));
    }
    Ok(out)
}

// ---------------------------------------------------------------- shader catalog

/// Names for [`NameDict`] from `catalog.json` of `shader-re`.
///
/// Every string of the file up to 128 bytes is taken, as a value or as an object key: CTAB
/// constant and sampler names, package names and `shader_sets` keys sit at places that depend on
/// the schema version, and a name that no FGX hash asks for does no harm.
pub fn load_shader_catalog(path: &Path) -> Result<NameDict, ModelError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ModelError::invalid(format!("shader catalog {}: {e}", path.display())))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| ModelError::invalid(format!("shader catalog {}: {e}", path.display())))?;
    let mut dict = NameDict::builtin();
    collect_strings(&value, &mut dict);
    Ok(dict)
}

fn collect_strings(v: &serde_json::Value, dict: &mut NameDict) {
    use serde_json::Value;
    match v {
        Value::String(s) if s.len() <= 128 => dict.add(s),
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, dict)),
        Value::Object(o) => {
            for (k, x) in o {
                if k.len() <= 128 {
                    dict.add(k);
                }
                collect_strings(x, dict);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- unit model walk

/// Result of checking the FGX of every unit model.
#[derive(Debug, Default)]
pub struct FgxReport {
    pub models: usize,
    pub materials: usize,
    /// Materials without an FGX block.
    pub without_fgx: usize,
    pub shader_sets: BTreeMap<String, usize>,
    /// Unknown names with the models that use them.
    pub unresolved_names: BTreeMap<Unresolved, BTreeSet<String>>,
    /// `model: material N sampler (hash)`: nothing explains the path.
    pub unresolved_textures: Vec<String>,
    /// The path is explained, but the install does not ship the file.
    pub missing_textures: Vec<String>,
    pub evidence: BTreeMap<PathEvidence, usize>,
    /// Texture paths no hash explained, filled by the sampler role: the material planner had
    /// evidence (`SourcePath`, `MaterialName`, pairing, the only candidate) ...
    pub inferred_strong: usize,
    /// ... or only elimination / a guess (listed one by one).
    pub inferred_weak: Vec<String>,
    pub known_dirs: usize,
    pub dict_extra: usize,
    /// Models that could not be checked at all.
    pub errors: Vec<String>,
}

impl FgxReport {
    /// Problems that fail the build: textures nothing explains, models that did not load. Unknown
    /// names count too when `names_are_errors` (a shader catalog was given).
    pub fn failures(&self, names_are_errors: bool) -> Vec<String> {
        let mut v = self.errors.clone();
        v.extend(self.unresolved_textures.iter().cloned());
        if names_are_errors {
            // A fallback material name is only a label (the shader set and the
            // parameters are asked for); it is listed but does not fail the check.
            v.extend(
                self.unresolved_names
                    .iter()
                    .filter(|(n, _)| n.role != NameRole::Fallback)
                    .map(|(n, models)| {
                        format!(
                            "unresolved name {n} (e.g. {})",
                            models.iter().next().unwrap()
                        )
                    }),
            );
        }
        v
    }

    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!(
            "fgx: {} model(s), {} material(s) ({} without FGX), {} directory hash(es), {} catalog name(s)",
            self.models, self.materials, self.without_fgx, self.known_dirs, self.dict_extra
        )];
        v.push(format!(
            "fgx: shader sets {}",
            self.shader_sets
                .iter()
                .map(|(k, n)| format!("{k} x{n}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        v.push(format!(
            "fgx: texture paths by evidence {:?}; by sampler role: {} with evidence, {} weak",
            self.evidence,
            self.inferred_strong,
            self.inferred_weak.len()
        ));
        v.extend(
            self.inferred_weak
                .iter()
                .map(|w| format!("fgx: weak texture {w}")),
        );
        let required = self
            .unresolved_names
            .keys()
            .filter(|n| n.role != NameRole::Fallback)
            .count();
        v.push(format!(
            "fgx: {} unresolved required name(s) (shader set, block, param; {} fallback label(s) more), {} unresolved texture(s), {} texture(s) missing from the install, {} error(s)",
            required,
            self.unresolved_names.len() - required,
            self.unresolved_textures.len(),
            self.missing_textures.len(),
            self.errors.len()
        ));
        v.extend(
            self.missing_textures
                .iter()
                .map(|t| format!("fgx: missing texture file {t}")),
        );
        for (n, models) in &self.unresolved_names {
            v.push(format!(
                "fgx: unresolved {n} ({} model(s), e.g. {})",
                models.len(),
                models.iter().next().map_or("", String::as_str)
            ));
        }
        v.extend(
            self.unresolved_textures
                .iter()
                .map(|t| format!("fgx: unresolved texture {t}")),
        );
        v.extend(self.errors.iter().map(|e| format!("fgx: error {e}")));
        v
    }
}

/// One unit model to check.
struct UnitModel {
    fxsxml: String,
    mesh: String,
    mats: Gr2Materials,
    textures: Vec<FxsTexture>,
    /// Texture file names, and the file stems of the fxsxml and the mesh.
    candidates: Vec<String>,
    hints: Vec<String>,
}

/// Checks the FGX of the models of every unit art member in the install: all names readable,
/// every texture path explained by a file of the install.
pub fn check_unit_models(game: &GameSource, dict: &NameDict) -> Result<FgxReport, ModelError> {
    let mut report = FgxReport {
        dict_extra: dict.len(),
        ..FgxReport::default()
    };
    let defs = ArtDefines::load(game)?;

    // Directories spelled out in clear anywhere in the install.
    let gr2: Vec<_> = game.iter_ext("gr2").collect();
    let seeds: Vec<PathResolver> = gr2
        .par_iter()
        .filter_map(|e| {
            let bytes = game.read(e).ok()?;
            let m = Gr2Materials::read(&bytes).ok()?;
            let mut r = PathResolver::new();
            r.seed_gr2(&m.fgx, &m.granny);
            (r.dir_count() > 0).then_some(r)
        })
        .collect();
    let mut resolver = PathResolver::new();
    for s in &seeds {
        resolver.merge(s);
    }

    // The distinct models of the members, in order.
    let mut fxsxmls: Vec<String> = defs
        .members
        .values()
        .map(|m| m.granny_fxsxml.clone())
        .collect();
    fxsxmls.sort();
    fxsxmls.dedup();
    let mut models = Vec::new();
    for fx in &fxsxmls {
        let r = resolve_fxsxml(game, fx, fx, 1.0, None)?;
        let missing: Vec<String> = r
            .warnings
            .iter()
            .filter(|w| {
                matches!(
                    w,
                    ArtWarning::MissingFile {
                        role: FileRole::Fxsxml | FileRole::Mesh,
                        ..
                    } | ArtWarning::BadFxsxml { .. }
                )
            })
            .map(ToString::to_string)
            .collect();
        let [mesh] = r.meshes() else {
            report.errors.push(format!(
                "{fx}: expected one mesh, found {}",
                r.meshes().len()
            ));
            continue;
        };
        if !missing.is_empty() {
            report.errors.push(format!("{fx}: {}", missing.join("; ")));
            continue;
        }
        let parsed = game
            .read_by_name(mesh)
            .map_err(ModelError::from)
            .and_then(|b| {
                b.ok_or_else(|| ModelError::invalid("mesh missing".to_owned()))
                    .and_then(|b| {
                        Gr2Materials::read(&b).map_err(|source| ModelError::Granny {
                            what: mesh.clone(),
                            source,
                        })
                    })
            });
        match parsed {
            Ok(mats) => models.push(UnitModel {
                fxsxml: fx.clone(),
                mesh: mesh.clone(),
                mats,
                candidates: r.model.textures.iter().map(|t| t.file.clone()).collect(),
                textures: r.model.textures.clone(),
                hints: [fx.as_str(), mesh.as_str()]
                    .iter()
                    .map(|n| n.rsplit_once('.').map_or(*n, |(s, _)| s).to_owned())
                    .collect(),
            }),
            Err(e) => report.errors.push(format!("{fx}: {e}")),
        }
    }

    // Pass 1: directories confirmed by pairs, shared by all models.
    for m in &models {
        resolver.learn(&path_hashes(&m.mats.fgx), &m.candidates, &m.hints);
    }
    report.known_dirs = resolver.dir_count();

    // Pass 2: the install's texture names (lower case) for what pairs left open.
    let extra: Vec<String> = game.iter_ext("dds").map(|e| e.name.clone()).collect();
    let exists = |f: &str| game.lookup(f).is_some();
    let built: Vec<_> = models
        .par_iter()
        .map(|m| {
            let files = ModelFiles {
                fxsxml: &m.fxsxml,
                mesh: &m.mesh,
                textures: &m.textures,
            };
            (
                &m.fxsxml,
                build_descs(&m.mats, &files, &resolver, &extra, dict, &exists),
            )
        })
        .collect();
    report.models = models.len();
    for (fx, result) in built {
        let mm = match result {
            Ok(mm) => mm,
            Err(e) => {
                report.errors.push(format!("{fx}: {e}"));
                continue;
            }
        };
        for d in &mm.descs {
            match d {
                Some(d) => {
                    report.materials += 1;
                    *report.shader_sets.entry(d.package.clone()).or_default() += 1;
                }
                None => {
                    report.materials += 1;
                    report.without_fgx += 1;
                }
            }
        }
        for u in mm.unresolved {
            report
                .unresolved_names
                .entry(u)
                .or_default()
                .insert(fx.clone());
        }
        for t in mm.unresolved_textures {
            match t.missing_file {
                Some(file) => report.missing_textures.push(format!(
                    "{fx}: material {} sampler {} -> {file}",
                    t.material, t.sampler
                )),
                None => report.unresolved_textures.push(format!(
                    "{fx}: material {} sampler {} (path #{:08x})",
                    t.material, t.sampler, t.hash
                )),
            }
        }
        for i in mm.inferred {
            if i.weak {
                report.inferred_weak.push(format!(
                    "{fx}: material {} sampler {} -> {}",
                    i.material, i.sampler, i.file
                ));
            } else {
                report.inferred_strong += 1;
            }
        }
        for (k, n) in mm.evidence {
            *report.evidence.entry(k).or_default() += n;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(path: &str) -> u32 {
        name_hash(path)
    }

    #[test]
    fn a_guessed_decal_base_is_the_d_file_beside_its_height_map() {
        use asset_format::{ParamSet, TexKind, TexParam};
        let tex = |name: &str, file: &str| TexParam {
            name: name.into(),
            kind: TexKind::Tex2D,
            src: TexSource::File(texture_id(file)),
            semantic: -1,
            reg: 0,
            usage: 0,
            flags: 8,
            active: true,
        };
        let camp = |package: &str| MaterialDesc {
            package: package.into(),
            sets: vec![ParamSet {
                blocks: vec![],
                textures: vec![
                    tex("BaseTextureMap", "Barbarian_Huts_DIFF.dds"),
                    tex("HeightTextureMap", "Barbarian_Camp2_Decal_H.dds"),
                ],
                constants: vec![],
            }],
            ..MaterialDesc::default()
        };
        let files: Vec<FxsTexture> = [
            "Barbarian_Camp2_Decal_D.dds",
            "Barbarian_Camp2_Decal_H.dds",
            "Barbarian_Huts_DIFF.dds",
        ]
        .iter()
        .map(|f| FxsTexture {
            file: (*f).into(),
            slot: TextureSlot::Other,
            material_key: String::new(),
        })
        .collect();
        let guessed = || {
            RefCell::new(vec![(
                "BaseTextureMap".to_owned(),
                Inferred {
                    file: "Barbarian_Huts_DIFF.dds".into(),
                    weak: true,
                },
            )])
        };
        let base = |d: &MaterialDesc| d.texture("BaseTextureMap").unwrap().src.clone();
        let mut d = camp("LandmarkShader");
        decal_base_from_height(&mut d, &files, &guessed(), &|_| true);
        assert_eq!(
            base(&d),
            TexSource::File(texture_id("Barbarian_Camp2_Decal_D.dds"))
        );
        // A base the path names stays; so does every other package.
        let mut d = camp("LandmarkShader");
        decal_base_from_height(&mut d, &files, &RefCell::new(Vec::new()), &|_| true);
        assert_eq!(
            base(&d),
            TexSource::File(texture_id("Barbarian_Huts_DIFF.dds"))
        );
        let mut d = camp("BuildingShader");
        decal_base_from_height(&mut d, &files, &guessed(), &|_| true);
        assert_eq!(
            base(&d),
            TexSource::File(texture_id("Barbarian_Huts_DIFF.dds"))
        );
    }

    #[test]
    fn known_directory_explains_a_hash() {
        let mut r = PathResolver::new();
        r.seed_path(r"D:\art\Units\Shared\Textures\Black_SREF.dds");
        let h = hash(r"D:\art\Units\Shared\Textures\Unit_Irradiance.dds");
        let cands = vec!["Unit_Irradiance.dds".to_owned(), "Other.dds".to_owned()];
        let got = r.resolve(&BTreeSet::from([h]), &cands, &[]);
        assert_eq!(got[&h].file, "Unit_Irradiance.dds");
        assert_eq!(got[&h].evidence, PathEvidence::KnownDir);
    }

    #[test]
    fn a_bare_file_name_is_a_path() {
        let r = PathResolver::new();
        let h = hash("Tex.dds");
        let got = r.resolve(&BTreeSet::from([h]), &["Tex.dds".into()], &[]);
        assert_eq!(got[&h].evidence, PathEvidence::Bare);
    }

    #[test]
    fn two_paths_in_one_directory_confirm_each_other_and_teach_the_directory() {
        let dir = r"D:\art\Units\Catapult\Textures";
        let (a, b) = (
            hash(&format!(r"{dir}\Catapult_DIFF.dds")),
            hash(&format!(r"{dir}\Catapult_Crew_SREF.dds")),
        );
        let cands = vec![
            "Catapult_DIFF.dds".to_owned(),
            "Catapult_Crew_SREF.dds".to_owned(),
            "Unrelated_DIFF.dds".to_owned(),
        ];
        let mut r = PathResolver::new();
        let got = r.resolve(&BTreeSet::from([a, b]), &cands, &[]);
        assert_eq!(got[&a].file, "Catapult_DIFF.dds");
        assert_eq!(got[&b].file, "Catapult_Crew_SREF.dds");
        assert_eq!(got[&a].evidence, PathEvidence::PairedDir);

        // Another model with only one path from the same directory: unknown alone, known after
        // the first model taught the directory.
        let c = hash(&format!(r"{dir}\Siege_DIFF.dds"));
        let other = vec!["Siege_DIFF.dds".to_owned()];
        assert!(r.resolve(&BTreeSet::from([c]), &other, &[]).is_empty());
        r.learn(&BTreeSet::from([a, b]), &cands, &[]);
        let got = r.resolve(&BTreeSet::from([c]), &other, &[]);
        assert_eq!(got[&c].evidence, PathEvidence::KnownDir);
    }

    #[test]
    fn equal_length_names_prove_nothing() {
        let (a, b) = (hash(r"X:\1\AAAA_DIFF.dds"), hash(r"Y:\2\BBBB_DIFF.dds"));
        let cands = vec!["AAAA_DIFF.dds".to_owned(), "BBBB_DIFF.dds".to_owned()];
        let got = PathResolver::new().resolve(&BTreeSet::from([a, b]), &cands, &[]);
        assert!(got.is_empty());
    }

    #[test]
    fn install_names_help_only_with_a_known_directory() {
        let mut r = PathResolver::new();
        r.seed_path(r"D:\art\Shared\x.dds");
        let h = hash(r"D:\art\Shared\lonely.dds");
        let extra = vec!["lonely.dds".to_owned(), "other.dds".to_owned()];
        let got = r.resolve(&BTreeSet::from([h]), &[], &extra);
        assert_eq!(got[&h].file, "lonely.dds");
        assert!(
            PathResolver::new()
                .resolve(&BTreeSet::from([h]), &[], &extra)
                .is_empty()
        );
    }

    #[test]
    fn catalog_names_are_collected_from_values_and_keys() {
        let dir = std::env::temp_dir().join(format!("fgx-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.json");
        std::fs::write(
            &path,
            r#"{"schema":1,"blocks":[{"ctab":[{"name":"fWaterHeight"}]}],
               "shader_sets":{"UnitShader_Skinned":{"pkg":"UnitShader"}}}"#,
        )
        .unwrap();
        let dict = load_shader_catalog(&path).unwrap();
        let n = NameRef {
            text: None,
            id: Some(name_hash("UnitShader_Skinned")),
        };
        assert_eq!(dict.resolve(&n).as_deref(), Some("UnitShader_Skinned"));
        let n = NameRef {
            text: None,
            id: Some(name_hash("fWaterHeight")),
        };
        assert_eq!(dict.resolve(&n).as_deref(), Some("fWaterHeight"));
        assert!(load_shader_catalog(&dir.join("missing.json")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
