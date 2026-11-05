//! Set `b1-units`: static unit models and the records
//! `unit/<UNIT_TYPE>` of the art table.
//!
//! The snapshot decides what is built: every unit of the B1 eras ([`crate::db::b1_units`]),
//! its `ArtDefine_UnitInfos` row, the members of that row and the style variants (`_ASIA`, ...)
//! of units with a cultural variation. The files come from the game's virtual file system.
//!
//! # Models
//! One blob per distinct `.fxsxml` named by `ArtDefine_UnitMemberInfos.Model`, id
//! `model:static_<fxsxml stem>`. Members that share an fxsxml share the blob and differ only in
//! the `scale` / `z_offset` of their [`Figure`]. The id differs from the animated Warrior set's
//! `model:<member id>`, so both can be in one manifest. The model is
//! [`LoadedMember::convert_static`]: frame 0 of the idle clip baked in, damage-state hulls
//! dropped; the animated blobs are not written (SIMP-010).
//!
//! # Placeholders
//! SIMPLIFIED(SIMP-011): a unit art whose chain is broken (no `ArtDefine_UnitInfos` row, no
//! member, a member without a model row) or whose model cannot be built (missing file, texture
//! that does not convert, conversion error) gets `placeholder = true` and no figures: the client
//! draws a marker for the whole squad. The reason goes to the report; no other unit model is
//! ever substituted.
//!
//! # Rules not recovered from the game
//! - A missing `Scale` is 1.0 and a missing `ZOffset` is 0.0 (SIMPLIFIED: the engine default
//!   was not read).
//! - A missing `Formation` is `None`.
//! - Era variants (`UnitArtInfoEraVariation`) are not built; the base art is used for every era
//!   and the unit goes to the report.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::UnitArt as UnitArtRecord;
use asset_format::{
    ContentHash, Figure, LogicalId, PrefetchGroup, UnitRecord, UnitVariant, Variant, VariantFormat,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::{TextureOptions, convert_texture, load_art_table, store_art_table};
use crate::art::flat_name;
use crate::db::{
    ArtInfoRow, DbError, MemberRow, SnapDb, UnitRow, art_info, b1_art_infos, b1_units, members,
    open_snapshot, strategic_view, style_variants,
};
use crate::game::GameSource;
use crate::models::ModelError;
use crate::models::materials::{MaterialNote, NoteKind};
use crate::models::set::{load_fxsxml, texture_id};
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder, StoreStats};

pub const SET_NAME: &str = "b1-units";

/// Bump when the static model bytes change for the same inputs. The animated `model` step of the
/// Warrior set has its own version (`MODEL_STEP_VERSION`) and is not affected.
pub const STATIC_STEP_VERSION: u32 = 1;

/// Models and their textures are fetched on demand when a unit appears.
const PREFETCH: PrefetchGroup = PrefetchGroup(0);

/// Id of the static model of an fxsxml (`Assets/Units/Foo/Bar.fxsxml` or `bar.fxsxml`).
pub fn static_model_id(fxsxml: &str) -> LogicalId {
    let flat = flat_name(fxsxml);
    let stem = flat.strip_suffix(".fxsxml").unwrap_or(&flat);
    LogicalId::new(format!("model:static_{stem}"))
}

// ---------------------------------------------------------------- snapshot input

/// The art chain of one `ArtDefine_UnitInfos` type as the snapshot has it.
#[derive(Debug, Clone)]
pub struct ArtChain {
    /// `None`: the type has no `ArtDefine_UnitInfos` row.
    pub info: Option<ArtInfoRow>,
    /// Members in table order, or why the chain is broken.
    pub members: Result<Vec<MemberRow>, String>,
    /// `ArtDefine_StrategicView.Asset` of the type.
    pub sv_file: Option<String>,
}

/// Everything the set reads from the snapshot.
#[derive(Debug, Clone, Default)]
pub struct UnitsInput {
    pub units: Vec<UnitRow>,
    /// Base art info to `(suffix, variant art info)`, for units with a cultural variation.
    pub variants: BTreeMap<String, Vec<(String, String)>>,
    pub arts: BTreeMap<String, ArtChain>,
}

pub fn read_input(db: &SnapDb) -> Result<UnitsInput, DbError> {
    let units = b1_units(db)?;
    let infos = b1_art_infos(db, &units)?;
    let mut variants = BTreeMap::new();
    for u in units.iter().filter(|u| u.cultural_variation) {
        if !variants.contains_key(&u.art_info) {
            variants.insert(u.art_info.clone(), style_variants(db, &u.art_info)?);
        }
    }
    let mut arts = BTreeMap::new();
    for ai in infos {
        let chain_members = members(db, &ai).map_err(|e| e.to_string());
        arts.insert(
            ai.clone(),
            ArtChain {
                info: art_info(db, &ai)?,
                members: chain_members,
                sv_file: strategic_view(db, &ai)?,
            },
        );
    }
    Ok(UnitsInput {
        units,
        variants,
        arts,
    })
}

/// Reads the input from a snapshot directory.
pub fn load_input(snapshot: &Path) -> Result<UnitsInput, ModelError> {
    let wrap = |e: DbError| ModelError::invalid(format!("reading {}: {e}", snapshot.display()));
    read_input(&open_snapshot(snapshot).map_err(wrap)?).map_err(wrap)
}

/// Flat fxsxml name to the art infos whose members use it.
pub fn model_users(input: &UnitsInput) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (ai, chain) in &input.arts {
        if let Ok(ms) = &chain.members {
            for m in ms {
                out.entry(flat_name(&m.model))
                    .or_default()
                    .insert(ai.clone());
            }
        }
    }
    out
}

// ---------------------------------------------------------------- records

/// Result of [`assemble`].
#[derive(Debug, Default)]
pub struct Assembly {
    pub records: BTreeMap<String, UnitRecord>,
    /// `placeholder`, `placeholder-variant`, `missing-icon`, `era-variation`.
    pub entries: Vec<ReportEntry>,
    /// Unit types whose base art is a placeholder.
    pub placeholder_units: usize,
    /// Variant arts that are placeholders.
    pub placeholder_variants: usize,
    pub variant_arts: usize,
}

/// Which unit types use which art info (their own and, as variants, the style ones).
fn art_users(input: &UnitsInput) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut out: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for u in &input.units {
        out.entry(&u.art_info).or_default().insert(&u.unit_type);
        if u.cultural_variation {
            for (_, v) in input.variants.get(&u.art_info).into_iter().flatten() {
                out.entry(v).or_default().insert(&u.unit_type);
            }
        }
    }
    out
}

/// The art of one art info, with its placeholder reason and missing-icon reason.
/// `failed` maps flat fxsxml names that did not build to the reason; `has_icon` tells whether a
/// strategic-view texture file is in the manifest.
fn art_of(
    art: &str,
    input: &UnitsInput,
    failed: &BTreeMap<String, String>,
    has_icon: &dyn Fn(&str) -> bool,
) -> (UnitArtRecord, Option<String>, Option<String>) {
    let mut out = UnitArtRecord {
        art_info: art.to_owned(),
        figures: Vec::new(),
        formation: None,
        sv_icon: None,
        placeholder: false,
    };
    let Some(chain) = input.arts.get(art) else {
        out.placeholder = true;
        return (out, Some("not read from the snapshot".into()), None);
    };
    out.formation = chain.info.as_ref().and_then(|i| i.formation.clone());
    let icon_reason = match &chain.sv_file {
        None => Some("no ArtDefine_StrategicView row".to_owned()),
        Some(f) if has_icon(f) => {
            out.sv_icon = Some(texture_id(f));
            None
        }
        Some(f) => Some(format!(
            "{f} is not in the manifest (not in the install, or set b1-sv was not built)"
        )),
    };
    let reason = if chain.info.is_none() {
        Some("no ArtDefine_UnitInfos row".to_owned())
    } else {
        match &chain.members {
            Err(e) => Some(format!("broken art chain: {e}")),
            Ok(ms) if ms.is_empty() => Some("no ArtDefine_UnitInfoMemberInfos rows".to_owned()),
            Ok(ms) => {
                let bad: Vec<String> = ms
                    .iter()
                    .filter_map(|m| {
                        failed
                            .get(&flat_name(&m.model))
                            .map(|r| format!("member {} model {}: {r}", m.member_type, m.model))
                    })
                    .collect();
                if bad.is_empty() {
                    out.figures = ms
                        .iter()
                        .map(|m| Figure {
                            model: static_model_id(&m.model),
                            count: m.count,
                            scale: m.scale.unwrap_or(1.0),
                            z_offset: m.z_offset.unwrap_or(0.0),
                        })
                        .collect();
                    None
                } else {
                    Some(bad.join("; "))
                }
            }
        }
    };
    out.placeholder = reason.is_some();
    (out, reason, icon_reason)
}

/// The records `unit/<UNIT_TYPE>` and the report entries of the input. Pure: `failed` and
/// `has_icon` are what the build found out about files.
pub fn assemble(
    input: &UnitsInput,
    failed: &BTreeMap<String, String>,
    has_icon: &dyn Fn(&str) -> bool,
) -> Assembly {
    let users = art_users(input);
    let base_arts: BTreeSet<&str> = input.units.iter().map(|u| u.art_info.as_str()).collect();
    let mut arts: BTreeMap<&str, (UnitArtRecord, Option<String>, Option<String>)> = BTreeMap::new();
    for name in users.keys() {
        arts.insert(name, art_of(name, input, failed, has_icon));
    }

    let mut a = Assembly::default();
    for (name, (_, placeholder, icon)) in &arts {
        let units: Vec<&str> = users[name].iter().copied().collect();
        let is_base = base_arts.contains(name);
        if let Some(r) = placeholder {
            let kind = if is_base {
                "placeholder"
            } else {
                "placeholder-variant"
            };
            a.entries.push(ReportEntry::new(
                kind,
                *name,
                format!("{r} (units: {})", units.join(", ")),
            ));
        }
        if let Some(r) = icon {
            a.entries.push(ReportEntry::new(
                "missing-icon",
                *name,
                format!("{r} (units: {})", units.join(", ")),
            ));
        }
    }
    for u in &input.units {
        let base = arts[u.art_info.as_str()].0.clone();
        let mut variants = Vec::new();
        if u.cultural_variation {
            for (suffix, v) in input.variants.get(&u.art_info).into_iter().flatten() {
                variants.push(UnitVariant {
                    suffix: suffix.clone(),
                    art: arts[v.as_str()].0.clone(),
                });
            }
        }
        a.variant_arts += variants.len();
        a.placeholder_variants += variants.iter().filter(|v| v.art.placeholder).count();
        a.placeholder_units += usize::from(base.placeholder);
        if u.era_variation {
            a.entries.push(ReportEntry::new(
                "era-variation",
                u.unit_type.clone(),
                "UnitArtInfoEraVariation=1: the art of the owner era is not built, the base art is used",
            ));
        }
        a.records
            .insert(u.unit_type.clone(), UnitRecord { base, variants });
    }
    a
}

// ---------------------------------------------------------------- models

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedInfo {
    pub mesh: u32,
    pub material: u16,
    pub diffuse: Option<String>,
    pub triangles: u32,
}

/// Cached output of converting one static model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Built {
    pub model: (ContentHash, u64),
    /// Lowercase texture files of the kept materials, DIFF and SREF.
    pub textures: Vec<String>,
    pub clip: Option<String>,
    pub dropped: Vec<DroppedInfo>,
    pub warnings: Vec<String>,
    pub meshes: u32,
    pub triangles: u32,
}

/// One model after loading and converting.
struct Processed {
    flat: String,
    outcome: Result<Built, String>,
    notes: Vec<MaterialNote>,
    load_warnings: Vec<String>,
}

fn process_model(
    game: &GameSource,
    cache: &BuildCache<'_>,
    store: &BlobStore,
    flat: &str,
) -> Processed {
    let id = static_model_id(flat);
    let loaded = match load_fxsxml(game, id.as_str(), flat, 1.0) {
        Ok(l) => l,
        Err(e) => {
            return Processed {
                flat: flat.to_owned(),
                outcome: Err(e.to_string()),
                notes: Vec::new(),
                load_warnings: Vec::new(),
            };
        }
    };
    let key = CacheKey::new(
        "model-static",
        STATIC_STEP_VERSION,
        &loaded.input_hashes(),
        &loaded.step_params(),
    );
    let outcome = cache
        .get_or_compute(
            &key,
            |b: &Built| vec![b.model.0],
            || {
                let (stat, _animated) = loaded.convert_static()?;
                let (hash, size) = store.put(&stat.model.encode())?;
                let mut textures: Vec<String> = Vec::new();
                for m in &stat.model.materials {
                    for t in [&m.diffuse, &m.specular].into_iter().flatten() {
                        let file = t.as_str().trim_start_matches("tex:").to_owned();
                        if !textures.contains(&file) {
                            textures.push(file);
                        }
                    }
                }
                Ok::<_, ModelError>(Built {
                    model: (hash, size),
                    textures,
                    clip: stat.clip,
                    dropped: stat
                        .dropped
                        .iter()
                        .map(|d| DroppedInfo {
                            mesh: d.mesh as u32,
                            material: d.material,
                            diffuse: d.diffuse.as_ref().map(|i| i.as_str().to_owned()),
                            triangles: d.triangles as u32,
                        })
                        .collect(),
                    warnings: stat.warnings,
                    meshes: stat.model.meshes.len() as u32,
                    triangles: stat
                        .model
                        .meshes
                        .iter()
                        .map(|m| m.indices.len() as u32 / 3)
                        .sum(),
                })
            },
        )
        .map_err(|e| e.to_string());
    Processed {
        flat: flat.to_owned(),
        outcome,
        notes: loaded.plan.notes.clone(),
        load_warnings: loaded.load_warnings.clone(),
    }
}

/// Report entries of one built model: material guesses (B8), dropped meshes (B7), warnings.
fn model_entries(p: &Processed, built: &Built) -> Vec<ReportEntry> {
    let mut out = Vec::new();
    for n in &p.notes {
        let kind = match n.kind {
            NoteKind::Ambiguous => "material-ambiguous",
            NoteKind::Inferred => "material-inferred",
            NoteKind::Missing => "material-missing",
            NoteKind::DamageState => continue, // listed as dropped-mesh below
        };
        out.push(ReportEntry::new(kind, p.flat.clone(), n.to_string()));
    }
    for d in &built.dropped {
        out.push(ReportEntry::new(
            "dropped-mesh",
            p.flat.clone(),
            format!(
                "mesh {} ({} triangles, material {}, {}): damage-state hull, SIMP-010",
                d.mesh,
                d.triangles,
                d.material,
                d.diffuse.as_deref().unwrap_or("no diffuse")
            ),
        ));
    }
    for w in p.load_warnings.iter().chain(&built.warnings) {
        out.push(ReportEntry::new("model-warning", p.flat.clone(), w.clone()));
    }
    out
}

// ---------------------------------------------------------------- build

#[derive(Debug)]
pub struct UnitsSummary {
    pub units: usize,
    pub placeholder_units: usize,
    pub placeholder_variants: usize,
    pub variant_arts: usize,
    pub models: usize,
    pub failed_models: usize,
    pub textures: usize,
    pub meshes: u64,
    pub triangles: u64,
    pub dropped_meshes: usize,
    pub model_bytes: u64,
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub table_entries: usize,
    pub table_bytes: u64,
    pub report: SetReport,
    pub stats: StoreStats,
    pub seconds: f64,
}

impl UnitsSummary {
    pub fn total_bytes(&self) -> u64 {
        self.model_bytes + self.texture_bytes.values().sum::<u64>() + self.table_bytes
    }
}

type Converted = Vec<(VariantFormat, ContentHash, u64)>;

/// Builds the set into `out`: models, textures and `unit/*` records, added to the manifest and
/// the art table already there. Run after `b1-sv`, which writes the icons the records name.
pub fn build_units(
    game: &GameSource,
    input: &UnitsInput,
    out: &Path,
    opts: TextureOptions,
) -> Result<UnitsSummary, ModelError> {
    let started = Instant::now();
    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut manifest = ManifestBuilder::open(&store)?;

    let users = model_users(input);
    let flats: Vec<&String> = users.keys().collect();
    let processed: Vec<Processed> = flats
        .par_iter()
        .map(|f| process_model(game, &cache, &store, f))
        .collect();

    // Textures shared between models are converted once. A texture that does not convert fails
    // every model that draws with it.
    let mut files: BTreeSet<&str> = BTreeSet::new();
    for p in &processed {
        if let Ok(b) = &p.outcome {
            files.extend(b.textures.iter().map(String::as_str));
        }
    }
    let files: Vec<&str> = files.into_iter().collect();
    let converted: Vec<Result<Converted, String>> = files
        .par_iter()
        .map(|f| {
            let bytes = game
                .read_by_name(f)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{f} is missing from the install"))?;
            convert_texture(&cache, &store, f, &bytes, opts).map_err(|e| e.to_string())
        })
        .collect();
    let textures: BTreeMap<&str, Result<Converted, String>> =
        files.iter().copied().zip(converted).collect();

    let mut failed: BTreeMap<String, String> = BTreeMap::new();
    let mut ok: Vec<(&Processed, &Built)> = Vec::new();
    for p in &processed {
        match &p.outcome {
            Err(e) => {
                failed.insert(p.flat.clone(), e.clone());
            }
            Ok(b) => {
                let bad: Vec<String> = b
                    .textures
                    .iter()
                    .filter_map(|t| match &textures[t.as_str()] {
                        Err(e) => Some(format!("texture {t}: {e}")),
                        Ok(_) => None,
                    })
                    .collect();
                if bad.is_empty() {
                    ok.push((p, b));
                } else {
                    failed.insert(p.flat.clone(), bad.join("; "));
                }
            }
        }
    }

    let mut entries: Vec<ReportEntry> = Vec::new();
    let mut model_bytes = 0;
    let (mut meshes, mut triangles, mut dropped) = (0u64, 0u64, 0usize);
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for (p, b) in &ok {
        manifest.add(
            static_model_id(&p.flat),
            Variant {
                format: VariantFormat::Generic,
                hash: b.model.0,
                size: b.model.1,
                prefetch: PREFETCH,
            },
        );
        model_bytes += b.model.1;
        meshes += u64::from(b.meshes);
        triangles += u64::from(b.triangles);
        dropped += b.dropped.len();
        used.extend(b.textures.iter().map(String::as_str));
        entries.extend(model_entries(p, b));
    }
    let mut texture_bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    for file in &used {
        if let Ok(variants) = &textures[file] {
            for (format, hash, size) in variants {
                *texture_bytes.entry(*format).or_default() += size;
                manifest.add(
                    texture_id(file),
                    Variant {
                        format: *format,
                        hash: *hash,
                        size: *size,
                        prefetch: PREFETCH,
                    },
                );
            }
        }
    }
    for (flat, reason) in &failed {
        entries.push(ReportEntry::new(
            "model-failed",
            flat.clone(),
            reason.clone(),
        ));
    }

    let has_icon = |f: &str| !manifest.manifest().variants(&texture_id(f)).is_empty();
    let a = assemble(input, &failed, &has_icon);
    entries.extend(a.entries.iter().cloned());
    let mut table = load_art_table(&store, &manifest)?;
    for (unit_type, rec) in &a.records {
        table.insert_unit(unit_type, rec.clone());
    }
    let table_bytes = store_art_table(&store, &mut manifest, &table)?;
    manifest.finish(&store)?;

    let mut report = SetReport {
        entries,
        ..SetReport::default()
    };
    for (k, v) in [
        ("units", a.records.len()),
        ("placeholder units", a.placeholder_units),
        ("placeholder variants", a.placeholder_variants),
        ("variant arts", a.variant_arts),
        ("models", ok.len()),
        ("failed models", failed.len()),
        ("textures", used.len()),
        ("meshes", meshes as usize),
        ("triangles", triangles as usize),
        ("dropped meshes", dropped),
    ] {
        report.counts.insert(k.to_owned(), v as u64);
    }
    report.bytes.insert("models".to_owned(), model_bytes);
    for (f, b) in &texture_bytes {
        report.bytes.insert(format!("textures {f:?}"), *b);
    }
    report.bytes.insert("table".to_owned(), table_bytes);
    Ok(UnitsSummary {
        units: a.records.len(),
        placeholder_units: a.placeholder_units,
        placeholder_variants: a.placeholder_variants,
        variant_arts: a.variant_arts,
        models: ok.len(),
        failed_models: failed.len(),
        textures: used.len(),
        meshes,
        triangles,
        dropped_meshes: dropped,
        model_bytes,
        texture_bytes,
        table_entries: table.len(),
        table_bytes,
        report,
        stats: store.stats(),
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_ids_use_the_fxsxml_stem() {
        assert_eq!(
            static_model_id("Assets/Units/Warrior/Warrior_V2.fxsxml").as_str(),
            "model:static_warrior_v2"
        );
        assert_eq!(
            static_model_id("Catapult.fxsxml").as_str(),
            "model:static_catapult"
        );
    }
}
