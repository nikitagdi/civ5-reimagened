//! Material assignment: which `.fxsxml` textures each Granny material, and so each mesh group,
//! is drawn with.
//!
//! # Evidence in the files
//! A mesh group names a Granny material through the mesh's `MaterialBindings`. In the packed
//! Civ5 gr2 files every string of a material is stripped to its hash ([`granny::name_hash`]):
//! the material `Name` (the 3ds Max material name), each map's `Usage` and the map texture's
//! `FromFileName` (the exporter's absolute source path). The `.fxsxml` lists the textures as one
//! flat list. Three facts connect them, strongest first:
//!
//! 1. **Source path** ([`Evidence::SourcePath`]). The source file name is the `.dds` name the
//!    fxsxml lists, but the directory is unknown. CRC-32 can be run backwards over a known
//!    suffix ([`granny::name_hash_prefix`]), so for a map hash `h` and a candidate file `f`,
//!    `prefix(h, "\" + f)` is the hash of the directory `f` would sit in. Two maps whose
//!    candidates give the same directory hash confirm both candidates (a 32-bit match), as long
//!    as the two maps have different hashes and the two names have different lengths: CRC is
//!    affine, so names of equal length always agree in pairs and prove nothing. Upper and lower
//!    case of the names are tried.
//! 2. **Material name** ([`Evidence::MaterialName`]). Artists often named the Max material after
//!    the texture (`Catapult` for `Catapult_DIFF.dds`); the name hash is compared with the
//!    texture's stem and its stem without the role suffix.
//! 3. **Pairing and elimination.** The specular map is the `_SREF` with the diffuse's base name
//!    ([`Evidence::PairedWithDiffuse`]); a slot with one candidate takes it
//!    ([`Evidence::Only`]); the last unresolved material takes the last unused texture
//!    ([`Evidence::Elimination`], reported as inferred).
//!
//! Whatever is still open gets a deterministic pick ([`Evidence::Guess`]) and a report entry:
//! the candidate whose base name equals the mesh file stem, then the longest common prefix with
//! it, then not a damage-state texture, then fxsxml order.
//!
//! Observed on the B1 units and cities: the map at index 0 is the diffuse map; its usage is
//! `Diffuse` (hash `9ee31012`) on city building materials and an unknown label (`6e9c5ea0`) on
//! unit materials. Two-material ships and the Great Admiral carry an intact and a damage-state
//! material (`*_DMG_*`, `*_DAMG_*`, `*_Dam_*`) bound to separate meshes; they are listed as
//! [`NoteKind::DamageState`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use granny::material::{Material as GrannyMaterial, NameRef};
use serde::{Deserialize, Serialize};

use crate::art::{FxsTexture, TextureSlot};

/// Why a texture was chosen for a material slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Evidence {
    /// The map's source path ends in this file name (reverse CRC, confirmed by a second map).
    SourcePath,
    /// The material name equals the texture's base name.
    MaterialName,
    /// `_SREF` with the base name of the chosen diffuse.
    PairedWithDiffuse,
    /// The only texture of its role in the fxsxml.
    Only,
    /// The only texture left once the other materials took theirs.
    Elimination,
    /// Nothing in the files decides; deterministic tie-break.
    Guess,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureChoice {
    /// File name as written in the fxsxml.
    pub file: String,
    pub evidence: Evidence,
}

/// The textures of one Granny material.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlannedMaterial {
    pub diffuse: Option<TextureChoice>,
    pub specular: Option<TextureChoice>,
}

/// What a report entry is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoteKind {
    /// Several candidates and no evidence: the pick is a guess.
    Ambiguous,
    /// Chosen by elimination, not by evidence on the material itself.
    Inferred,
    /// The fxsxml has no texture of this role.
    Missing,
    /// The material draws with a damage-state texture next to an intact material.
    DamageState,
}

impl fmt::Display for NoteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ambiguous => "ambiguous",
            Self::Inferred => "inferred",
            Self::Missing => "missing",
            Self::DamageState => "damage-state",
        })
    }
}

/// One line of the material report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialNote {
    /// The fxsxml (flat lower-case name).
    pub model: String,
    /// Granny material index (0 when the gr2 has no materials).
    pub material: usize,
    /// `diffuse` or `specular`.
    pub slot: String,
    pub kind: NoteKind,
    pub chosen: Option<String>,
    pub candidates: Vec<String>,
}

impl fmt::Display for MaterialNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "material {}: {} material {} {}: {}",
            self.kind,
            self.model,
            self.material,
            self.slot,
            self.chosen.as_deref().unwrap_or("(none)")
        )?;
        if !self.candidates.is_empty() {
            write!(f, " (candidates: {})", self.candidates.join(", "))?;
        }
        Ok(())
    }
}

/// Textures per Granny material plus the deduplicated engine material list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialPlan {
    /// One entry per Granny material (one entry if the gr2 has none).
    pub granny: Vec<PlannedMaterial>,
    /// Distinct `(diffuse, specular)` file pairs in first-use order: the engine materials.
    pub materials: Vec<(Option<String>, Option<String>)>,
    /// Engine material index per Granny material.
    pub by_granny: Vec<u16>,
    pub notes: Vec<MaterialNote>,
}

impl MaterialPlan {
    /// Every texture file the engine materials use, in first-use order, without repeats.
    pub fn texture_files(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        self.materials
            .iter()
            .flat_map(|(d, s)| d.iter().chain(s))
            .filter(|f| seen.insert(f.to_ascii_lowercase()))
            .map(String::as_str)
            .collect()
    }
}

/// Damage-state texture: a name token such as `DMG`, `DAMG`, `Dam`, `HeavyDMG`.
pub fn is_damage_texture(file: &str) -> bool {
    let lower = file.to_ascii_lowercase();
    let stem = lower.rsplit_once('.').map_or(lower.as_str(), |(s, _)| s);
    stem.split('_')
        .any(|t| matches!(t, "dmg" | "damg" | "dam" | "damage" | "damaged") || t.ends_with("dmg"))
}

/// Original-case stem and stem without the role suffix (`Catapult_DIFF.dds` -> `Catapult_DIFF`,
/// `Catapult`).
fn base_names(t: &FxsTexture) -> Vec<String> {
    let stem = t.file.rsplit_once('.').map_or(t.file.as_str(), |(s, _)| s);
    let mut out = vec![stem.to_owned()];
    if let Some((base, role)) = stem.rsplit_once('_')
        && TextureSlot::classify(&format!("x_{role}.dds")) == t.slot
        && t.slot != TextureSlot::Other
    {
        out.push(base.to_owned());
    }
    out
}

fn name_matches(name: &NameRef, candidate: &str) -> bool {
    match (&name.text, name.id) {
        (Some(text), _) => text.eq_ignore_ascii_case(candidate),
        (None, Some(id)) => [
            candidate.to_owned(),
            candidate.to_ascii_lowercase(),
            candidate.to_ascii_uppercase(),
        ]
        .iter()
        .any(|c| granny::name_hash(c) == id),
        (None, None) => false,
    }
}

fn file_part(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn common_prefix(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// Texture indices confirmed for each `(material, map)` by the source-path evidence.
fn source_path_matches(
    textures: &[FxsTexture],
    materials: &[GrannyMaterial],
) -> BTreeMap<(usize, usize), usize> {
    struct Entry {
        at: (usize, usize),
        hash: u32,
    }
    let mut confirmed: BTreeMap<(usize, usize), BTreeSet<usize>> = BTreeMap::new();
    let mut entries = Vec::new();
    for (mi, m) in materials.iter().enumerate() {
        for (k, map) in m.maps.iter().enumerate() {
            let at = (mi, k);
            match (&map.texture_file.text, map.texture_file.id) {
                (Some(path), _) => {
                    let file = file_part(path);
                    if let Some(t) = textures
                        .iter()
                        .position(|t| t.file.eq_ignore_ascii_case(file))
                    {
                        confirmed.entry(at).or_default().insert(t);
                    }
                }
                (None, Some(hash)) => entries.push(Entry { at, hash }),
                (None, None) => {}
            }
        }
    }
    // Directory hash -> (entry, texture, name length) for every candidate spelling.
    let mut dirs: BTreeMap<u32, Vec<(usize, usize, usize)>> = BTreeMap::new();
    for (ei, e) in entries.iter().enumerate() {
        for (ti, t) in textures.iter().enumerate() {
            if granny::name_hash(&t.file) == e.hash {
                confirmed.entry(e.at).or_default().insert(ti);
            }
            let spellings: BTreeSet<String> = [t.file.clone(), t.file.to_ascii_lowercase()]
                .into_iter()
                .collect();
            for s in spellings {
                let suffix = format!("\\{s}");
                let dir = granny::name_hash_prefix(e.hash, suffix.as_bytes());
                dirs.entry(dir).or_default().push((ei, ti, suffix.len()));
            }
        }
    }
    for group in dirs.values() {
        for (i, &(ea, ta, la)) in group.iter().enumerate() {
            for &(eb, tb, lb) in &group[i + 1..] {
                if entries[ea].hash != entries[eb].hash && la != lb {
                    confirmed.entry(entries[ea].at).or_default().insert(ta);
                    confirmed.entry(entries[eb].at).or_default().insert(tb);
                }
            }
        }
    }
    // A map confirmed as two different files would be a hash collision: trust neither.
    confirmed
        .into_iter()
        .filter_map(|(at, set)| {
            let names: BTreeSet<String> = set
                .iter()
                .map(|&t| textures[t].file.to_ascii_lowercase())
                .collect();
            (names.len() == 1).then(|| (at, *set.first().expect("non-empty")))
        })
        .collect()
}

/// Ranks candidates for a guess: base name equal to `target`, longest common prefix with it,
/// not a damage texture, fxsxml order.
fn rank(textures: &[FxsTexture], candidates: &mut [usize], target: &str) {
    candidates.sort_by_key(|&t| {
        let key = &textures[t].material_key;
        (
            key != target,
            std::cmp::Reverse(common_prefix(key, target)),
            is_damage_texture(&textures[t].file),
            t,
        )
    });
}

/// Assigns textures to the Granny `materials` of the mesh file `mesh_file` (any case), from the
/// textures of the fxsxml `model`. See the module docs for the rules.
pub fn plan_materials(
    model: &str,
    mesh_file: &str,
    textures: &[FxsTexture],
    materials: &[GrannyMaterial],
) -> MaterialPlan {
    // Distinct textures (the fxsxml repeats shared lighting maps), first occurrence wins.
    let mut seen = BTreeSet::new();
    let textures: Vec<FxsTexture> = textures
        .iter()
        .filter(|t| seen.insert(t.file.to_ascii_lowercase()))
        .cloned()
        .collect();
    let lower = mesh_file.to_ascii_lowercase();
    let mesh_stem = lower.rsplit_once('.').map_or(lower.as_str(), |(s, _)| s);
    let of_slot = |slot: TextureSlot| -> Vec<usize> {
        (0..textures.len())
            .filter(|&t| textures[t].slot == slot)
            .collect()
    };
    let diffuse_all = of_slot(TextureSlot::Diffuse);
    let spec_all = of_slot(TextureSlot::SpecRef);
    // A gr2 without materials still gets one material from the fxsxml.
    let virtual_one = [GrannyMaterial::default()];
    let gmats: &[GrannyMaterial] = if materials.is_empty() {
        &virtual_one
    } else {
        materials
    };
    let n = gmats.len();
    let by_map = source_path_matches(&textures, materials);
    let confirmed_of = |mi: usize, slot: TextureSlot| -> BTreeSet<usize> {
        by_map
            .iter()
            .filter(|((m, _), t)| *m == mi && textures[**t].slot == slot)
            .map(|(_, &t)| t)
            .collect()
    };
    let mut notes = Vec::new();
    let mut note = |mi: usize, slot: &str, kind, chosen: Option<usize>, cands: &[usize]| {
        notes.push(MaterialNote {
            model: model.to_owned(),
            material: mi,
            slot: slot.into(),
            kind,
            chosen: chosen.map(|t| textures[t].file.clone()),
            candidates: cands.iter().map(|&t| textures[t].file.clone()).collect(),
        });
    };

    // --- diffuse: evidence on the material itself ---
    let mut diffuse: Vec<Option<(usize, Evidence)>> = vec![None; n];
    for (mi, m) in gmats.iter().enumerate() {
        let confirmed: Vec<usize> = confirmed_of(mi, TextureSlot::Diffuse).into_iter().collect();
        if let [one] = confirmed[..] {
            diffuse[mi] = Some((one, Evidence::SourcePath));
            continue;
        }
        if confirmed.len() > 1 {
            let mut c = confirmed.clone();
            rank(&textures, &mut c, mesh_stem);
            diffuse[mi] = Some((c[0], Evidence::Guess));
            note(mi, "diffuse", NoteKind::Ambiguous, Some(c[0]), &c);
            continue;
        }
        // Map 0 is the diffuse map; a confirmed file there with no role token (decals: `_D`).
        if let Some(&t) = by_map.get(&(mi, 0))
            && textures[t].slot == TextureSlot::Other
        {
            diffuse[mi] = Some((t, Evidence::SourcePath));
            continue;
        }
        let named: BTreeSet<usize> = diffuse_all
            .iter()
            .copied()
            .filter(|&t| {
                base_names(&textures[t])
                    .iter()
                    .any(|b| name_matches(&m.name, b))
            })
            .collect();
        let named: Vec<usize> = named.into_iter().collect();
        match named[..] {
            [one] => diffuse[mi] = Some((one, Evidence::MaterialName)),
            [] => {}
            _ => {
                let mut c = named.clone();
                rank(&textures, &mut c, mesh_stem);
                diffuse[mi] = Some((c[0], Evidence::Guess));
                note(mi, "diffuse", NoteKind::Ambiguous, Some(c[0]), &c);
            }
        }
    }

    // --- diffuse: the rest ---
    let used: BTreeSet<usize> = diffuse.iter().flatten().map(|d| d.0).collect();
    let mut remaining: Vec<usize> = diffuse_all
        .iter()
        .copied()
        .filter(|t| !used.contains(t))
        .collect();
    let open: Vec<usize> = (0..n).filter(|&mi| diffuse[mi].is_none()).collect();
    if n == 1 && diffuse_all.len() == 1 && open == [0] {
        diffuse[0] = Some((diffuse_all[0], Evidence::Only));
    } else if open.len() == 1 && remaining.len() == 1 {
        let mi = open[0];
        diffuse[mi] = Some((remaining[0], Evidence::Elimination));
        note(
            mi,
            "diffuse",
            NoteKind::Inferred,
            Some(remaining[0]),
            &remaining,
        );
    } else {
        for &mi in &open {
            let mut c = if remaining.is_empty() {
                diffuse_all.clone()
            } else {
                remaining.clone()
            };
            if c.is_empty() {
                note(mi, "diffuse", NoteKind::Missing, None, &[]);
                continue;
            }
            rank(&textures, &mut c, mesh_stem);
            diffuse[mi] = Some((c[0], Evidence::Guess));
            note(mi, "diffuse", NoteKind::Ambiguous, Some(c[0]), &c);
            remaining.retain(|&t| t != c[0]);
        }
    }

    // --- specular: evidence, then pairing with the diffuse ---
    let mut specular: Vec<Option<(usize, Evidence)>> = vec![None; n];
    for mi in 0..n {
        let confirmed: Vec<usize> = confirmed_of(mi, TextureSlot::SpecRef).into_iter().collect();
        if let [one] = confirmed[..] {
            specular[mi] = Some((one, Evidence::SourcePath));
            continue;
        }
        let Some((d, _)) = diffuse[mi] else { continue };
        let key = &textures[d].material_key;
        let paired: Vec<usize> = spec_all
            .iter()
            .copied()
            .filter(|&t| textures[t].material_key == *key)
            .collect();
        if let [one] = paired[..] {
            specular[mi] = Some((one, Evidence::PairedWithDiffuse));
        }
    }
    let used: BTreeSet<usize> = specular.iter().flatten().map(|s| s.0).collect();
    let unused: Vec<usize> = spec_all
        .iter()
        .copied()
        .filter(|t| !used.contains(t))
        .collect();
    for mi in 0..n {
        if specular[mi].is_some() {
            continue;
        }
        let mut c = if unused.is_empty() {
            spec_all.clone()
        } else {
            unused.clone()
        };
        if c.is_empty() {
            continue; // No `_SREF` at all: draw without a specular map.
        }
        let target = diffuse[mi].map_or(mesh_stem.to_owned(), |(d, _)| {
            textures[d].material_key.clone()
        });
        if let [one] = c[..] {
            let ev = if spec_all.len() == 1 {
                Evidence::Only
            } else {
                Evidence::Elimination
            };
            specular[mi] = Some((one, ev));
            if ev == Evidence::Elimination {
                note(mi, "specular", NoteKind::Inferred, Some(one), &c);
            }
            continue;
        }
        rank(&textures, &mut c, &target);
        specular[mi] = Some((c[0], Evidence::Guess));
        note(mi, "specular", NoteKind::Ambiguous, Some(c[0]), &c);
    }

    // --- damage-state materials next to intact ones ---
    let damaged: Vec<bool> = diffuse
        .iter()
        .map(|d| d.is_some_and(|(t, _)| is_damage_texture(&textures[t].file)))
        .collect();
    if damaged.contains(&false) {
        for mi in (0..n).filter(|&mi| damaged[mi]) {
            let d = diffuse[mi].map(|x| x.0);
            note(mi, "diffuse", NoteKind::DamageState, d, &[]);
        }
    }

    let choice = |c: Option<(usize, Evidence)>| {
        c.map(|(t, evidence)| TextureChoice {
            file: textures[t].file.clone(),
            evidence,
        })
    };
    let granny: Vec<PlannedMaterial> = (0..n)
        .map(|mi| PlannedMaterial {
            diffuse: choice(diffuse[mi]),
            specular: choice(specular[mi]),
        })
        .collect();
    let mut engine: Vec<(Option<String>, Option<String>)> = Vec::new();
    let mut by_granny = Vec::with_capacity(n);
    for p in &granny {
        let pair = (
            p.diffuse.as_ref().map(|c| c.file.clone()),
            p.specular.as_ref().map(|c| c.file.clone()),
        );
        let lower = |s: &Option<String>| s.as_ref().map(|x| x.to_ascii_lowercase());
        let i = match engine
            .iter()
            .position(|e| lower(&e.0) == lower(&pair.0) && lower(&e.1) == lower(&pair.1))
        {
            Some(i) => i,
            None => {
                engine.push(pair);
                engine.len() - 1
            }
        };
        by_granny.push(u16::try_from(i).unwrap_or(u16::MAX));
    }
    MaterialPlan {
        granny,
        materials: engine,
        by_granny: if materials.is_empty() {
            Vec::new()
        } else {
            by_granny
        },
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::art::parse_fxsxml;
    use granny::material::MaterialMap;

    fn textures(files: &[&str]) -> Vec<FxsTexture> {
        let xml: String = files
            .iter()
            .map(|f| format!(r#"<Texture file="{f}"/>"#))
            .collect();
        parse_fxsxml(&format!("<Asset>{xml}</Asset>"))
            .unwrap()
            .textures
    }

    fn id(s: &str) -> NameRef {
        NameRef {
            text: None,
            id: Some(granny::name_hash(s)),
        }
    }

    /// A stripped material whose maps come from `files` in `dir` (full source paths).
    fn material(name: &str, dir: &str, files: &[&str]) -> GrannyMaterial {
        GrannyMaterial {
            name: id(name),
            texture_file: NameRef::default(),
            maps: files
                .iter()
                .map(|f| MaterialMap {
                    usage: id("Diffuse"),
                    texture_file: id(&format!("{dir}\\{f}")),
                })
                .collect(),
        }
    }

    const ENV: [&str; 3] = [
        "Unit_Irradiance.dds",
        "Unit_Environment_Dull.dds",
        "Unit_Environment_Sharp.dds",
    ];

    fn files(p: &MaterialPlan) -> Vec<(Option<&str>, Option<&str>)> {
        p.materials
            .iter()
            .map(|(d, s)| (d.as_deref(), s.as_deref()))
            .collect()
    }

    #[test]
    fn source_paths_pick_each_materials_pair() {
        let tex = textures(
            &[
                &ENV[..],
                &[
                    "CatapultOperator_DIFF.dds",
                    "CatapultOperator_SREF.dds",
                    "Catapult_DIFF.dds",
                    "Catapult_SREF.dds",
                ],
            ]
            .concat(),
        );
        let dir = r"C:\Art\Units\Catapult";
        let lib = r"C:\Art\Shared";
        // Material 0 is the crew, material 1 the machine; names give no hint.
        let mats = [
            material(
                "m0",
                dir,
                &["CatapultOperator_DIFF.dds", "CatapultOperator_SREF.dds"],
            ),
            material("m1", dir, &["Catapult_DIFF.dds", "Catapult_SREF.dds"]),
        ];
        let mut mats = mats.to_vec();
        for m in &mut mats {
            m.maps.push(MaterialMap {
                usage: id("x"),
                texture_file: id(&format!("{lib}\\{}", ENV[0])),
            });
        }
        let p = plan_materials("catapult.fxsxml", "Catapult.gr2", &tex, &mats);
        assert_eq!(
            files(&p),
            [
                (
                    Some("CatapultOperator_DIFF.dds"),
                    Some("CatapultOperator_SREF.dds")
                ),
                (Some("Catapult_DIFF.dds"), Some("Catapult_SREF.dds")),
            ]
        );
        assert_eq!(p.by_granny, [0, 1]);
        assert_eq!(
            p.granny[0].diffuse.as_ref().unwrap().evidence,
            Evidence::SourcePath
        );
        assert!(p.notes.is_empty(), "{:?}", p.notes);
        assert_eq!(
            p.texture_files(),
            [
                "CatapultOperator_DIFF.dds",
                "CatapultOperator_SREF.dds",
                "Catapult_DIFF.dds",
                "Catapult_SREF.dds"
            ]
        );
    }

    #[test]
    fn swapped_order_follows_the_paths_not_the_fxsxml() {
        let tex = textures(&["Khan_flag_DIFF.dds", "Khan_DIFF.dds"]);
        let mats = [
            material("a", r"D:\k", &["Khan_DIFF.dds"]),
            material("b", r"D:\k", &["Khan_flag_DIFF.dds"]),
        ];
        let p = plan_materials("k.fxsxml", "k.gr2", &tex, &mats);
        assert_eq!(
            files(&p),
            [
                (Some("Khan_DIFF.dds"), None),
                (Some("Khan_flag_DIFF.dds"), None)
            ]
        );
    }

    #[test]
    fn equal_length_names_prove_nothing_and_fall_back_to_names() {
        // Same length, different directories: no source-path evidence; the names decide.
        let tex = textures(&["AAAA_DIFF.dds", "BBBB_DIFF.dds"]);
        let mats = [
            material("BBBB", r"X:\1", &["BBBB_DIFF.dds"]),
            material("AAAA", r"Y:\2", &["AAAA_DIFF.dds"]),
        ];
        let p = plan_materials("m.fxsxml", "m.gr2", &tex, &mats);
        assert_eq!(
            files(&p),
            [(Some("BBBB_DIFF.dds"), None), (Some("AAAA_DIFF.dds"), None)]
        );
        assert_eq!(
            p.granny[0].diffuse.as_ref().unwrap().evidence,
            Evidence::MaterialName
        );
    }

    #[test]
    fn last_open_material_takes_the_last_texture_and_is_reported() {
        // Material 0 is proven by its base/height/specular maps; material 1 has nothing.
        let tex = textures(&[
            "Euro_Anc_City_DIFF.dds",
            "Euro_Anc_City_SREF.dds",
            "CityBase_Diff_anc.dds",
            "CityBase_HT.dds",
            "CityBase_SREF.dds",
        ]);
        let mats = [
            material(
                "x",
                r"C:\city",
                &[
                    "CityBase_Diff_anc.dds",
                    "CityBase_HT.dds",
                    "CityBase_SREF.dds",
                ],
            ),
            material("y", r"C:\other", &["Euro_Anc_City_DIFF.dds"]),
        ];
        let p = plan_materials("euro_anc_city.fxsxml", "Euro_Anc_City.gr2", &tex, &mats);
        assert_eq!(
            files(&p),
            [
                (Some("CityBase_Diff_anc.dds"), Some("CityBase_SREF.dds")),
                (
                    Some("Euro_Anc_City_DIFF.dds"),
                    Some("Euro_Anc_City_SREF.dds")
                ),
            ]
        );
        assert_eq!(p.notes.len(), 1, "{:?}", p.notes);
        assert_eq!(
            (p.notes[0].kind, p.notes[0].material),
            (NoteKind::Inferred, 1)
        );
        assert!(p.notes[0].to_string().contains("Euro_Anc_City_DIFF.dds"));
    }

    #[test]
    fn one_material_with_two_diffuse_maps_is_an_ambiguous_guess() {
        let tex = textures(&[
            "U_Roman_Ballista_Bolt_DIFF.dds",
            "U_Roman_Ballista_Bolt_SREF.dds",
            "U_Roman_Ballista_DIFF.dds",
            "U_Roman_Ballista_SREF.dds",
        ]);
        let mats = [material(
            "?",
            r"C:\b",
            &["U_Roman_Ballista_DIFF.dds", "U_Roman_Ballista_SREF.dds"],
        )];
        let p = plan_materials(
            "u_roman_ballista.fxsxml",
            "U_Roman_Ballista.gr2",
            &tex,
            &mats,
        );
        // The base name equal to the mesh stem wins; the SREF pairs with it.
        assert_eq!(
            files(&p),
            [(
                Some("U_Roman_Ballista_DIFF.dds"),
                Some("U_Roman_Ballista_SREF.dds")
            )]
        );
        assert_eq!(p.notes.len(), 1);
        assert_eq!(p.notes[0].kind, NoteKind::Ambiguous);
        assert_eq!(p.notes[0].candidates.len(), 2);
        // Deterministic: the same input gives the same plan.
        assert_eq!(
            p,
            plan_materials(
                "u_roman_ballista.fxsxml",
                "U_Roman_Ballista.gr2",
                &tex,
                &mats
            )
        );
    }

    #[test]
    fn single_texture_pair_needs_no_evidence() {
        let tex = textures(&[&ENV[..], &["Warrior_SREF.dds", "Warrior_DIFF.dds"]].concat());
        // Stripped material that matches nothing, and a gr2 with no materials at all.
        let mats = [material("zzz", r"C:\w", &["Other.dds"])];
        for m in [&mats[..], &[]] {
            let p = plan_materials("warrior.fxsxml", "Warrior.gr2", &tex, m);
            assert_eq!(
                files(&p),
                [(Some("Warrior_DIFF.dds"), Some("Warrior_SREF.dds"))]
            );
            assert!(p.notes.is_empty(), "{:?}", p.notes);
            assert_eq!(p.by_granny.len(), m.len());
        }
    }

    #[test]
    fn damage_materials_are_noted_and_identical_materials_merge() {
        let tex = textures(&[
            "galleass_DIFF.dds",
            "galleass_DMG_DIFF.dds",
            "galleass_SREF.dds",
        ]);
        let dir = r"C:\g";
        let mats = [
            material("a", dir, &["galleass_DMG_DIFF.dds", "galleass_SREF.dds"]),
            material("b", dir, &["galleass_DIFF.dds", "galleass_SREF.dds"]),
            material("c", dir, &["galleass_DIFF.dds", "galleass_SREF.dds"]),
        ];
        let p = plan_materials("galleass.fxsxml", "Galleass.gr2", &tex, &mats);
        assert_eq!(p.materials.len(), 2);
        assert_eq!(p.by_granny, [0, 1, 1]);
        assert_eq!(p.materials[0].1.as_deref(), Some("galleass_SREF.dds"));
        let kinds: Vec<_> = p.notes.iter().map(|n| (n.kind, n.material)).collect();
        assert_eq!(kinds, [(NoteKind::DamageState, 0)]);
        assert!(is_damage_texture("Trireme_DIFF_HeavyDMG.dds"));
        assert!(is_damage_texture("Vik_Longboat_Dam_DIFF.dds"));
        assert!(!is_damage_texture("Great_Admiral_REN_DIFF.dds"));
    }

    #[test]
    fn no_diffuse_texture_is_reported_missing() {
        let tex = textures(&ENV);
        let p = plan_materials("x.fxsxml", "x.gr2", &tex, &[]);
        assert_eq!(files(&p), [(None, None)]);
        assert_eq!(p.notes[0].kind, NoteKind::Missing);
    }
}
