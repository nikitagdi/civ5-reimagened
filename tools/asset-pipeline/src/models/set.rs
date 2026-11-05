//! Asset sets: unit members converted into blobs and a manifest, with an incremental cache.
//!
//! Cache steps (all keyed by content hashes of the input files, a step version and parameters,
//! so a second run reads and hashes the inputs but converts and writes nothing):
//! - `texture`: one DDS file to its Rgba8 / Bc7 / Astc4x4 blobs.
//! - `model`: fxsxml + mesh gr2 + every animation gr2 to the model blob and animation blobs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

use asset_format::{
    AnimEvent, AnimRef, AnimationBlob, ContentHash, LogicalId, Material, ModelBlob, PrefetchGroup,
    TextureBlob, TextureFormat, Variant, VariantFormat,
};
use granny::material::read_materials;
use granny::{GrannyFile, Scene};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::ModelError;
use super::bake::{StaticModel, bake_static};
use super::basis::Basis;
use super::convert::{ModelMaterials, convert_animation, convert_model};
use super::materials::{MaterialNote, MaterialPlan, NoteKind, plan_materials};
use crate::art::{ArtDefines, ArtWarning, FileRole, ResolvedMember, resolve_fxsxml};
use crate::game::GameSource;
use crate::store::{BlobStore, BuildCache, CacheKey, ManifestBuilder, StoreStats};
use crate::textures::{ColorSpace, color_space_for, convert_dds};

/// Bump when the converted bytes change for the same inputs (invalidates the build cache).
pub const MODEL_STEP_VERSION: u32 = 2;
pub const TEXTURE_STEP_VERSION: u32 = 1;

/// Everything in a set is fetched up front.
const PREFETCH: PrefetchGroup = PrefetchGroup(1);

/// Engine world units per Civ5 world unit, before the member's `fScale`. Engine world units are
/// Civ5 units, so the total scale of a unit model is `fScale * ENGINE_UNITS_PER_CIV5_UNIT`.
pub const ENGINE_UNITS_PER_CIV5_UNIT: f32 = 1.0;

/// A named list of unit member art ids that are built together.
#[derive(Debug)]
pub struct SetDef {
    pub name: &'static str,
    pub members: &'static [&'static str],
}

/// Add sets here; `build --set <name>` picks one.
pub const SETS: &[SetDef] = &[SetDef {
    name: "warrior",
    members: &["ART_DEF_UNIT_MEMBER_WARRIOR"],
}];

pub fn find_set(name: &str) -> Option<&'static SetDef> {
    SETS.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

/// Composite sets: a name that expands to member sets.
/// `game-data` is built by `crate::gamedata`, not from a [`SetDef`]; see [`GAME_DATA`].
pub const COMPOSITES: &[(&str, &[&str])] = &[
    (
        "b1",
        &[
            "b1-terrain",
            "b1-trees",
            "b1-cities",
            "b1-sv",
            "b1-units",
            "b1-table",
            "game-data",
        ],
    ),
    // `--set b2`. The groups are made after the last of these.
    (
        "b2",
        &[
            "b2-light",
            "b2-terrain",
            "b2-water",
            "b2-units",
            "b2-landmarks",
            "b2-city",
            "b2-misc",
            "b2-fx",
            "client-data",
        ],
    ),
];

/// Name of the set built by `crate::gamedata`. [`resolve_sets`] skips it; the caller runs it
/// after the unit sets (see [`expand_names`]).
pub const GAME_DATA: &str = "game-data";

/// Sets built by `crate::sets` (textures and art table records), not from a [`SetDef`].
pub const TABLE_SETS: &[&str] = &[
    crate::sets::terrain::SET_NAME,
    crate::sets::trees::SET_NAME,
    crate::sets::cities::SET_NAME,
    crate::sets::sv::SET_NAME,
    crate::sets::units::SET_NAME,
    crate::sets::table::SET_NAME,
    crate::sets::b2_light::SET_NAME,
    crate::sets::b2_units::SET_NAME,
    crate::sets::b2_landmarks::SET_NAME,
    crate::sets::b2_city::SET_NAME,
    crate::sets::b2_misc::SET_NAME,
    crate::sets::b2_terrain::SET_NAME,
    crate::sets::b2_water::SET_NAME,
    crate::sets::b2_fx::SET_NAME,
    crate::sets::client_data::SET_NAME,
];

pub fn is_table_set(name: &str) -> bool {
    TABLE_SETS.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// `--set` names with composites expanded, in order, repeats removed (case-insensitive).
pub fn expand_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        let members: Vec<&str> = match COMPOSITES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some((_, members)) => members.to_vec(),
            None => vec![name.as_str()],
        };
        for m in members {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(m)) {
                out.push(m.to_owned());
            }
        }
    }
    out
}

/// Composite sets read the game tables from the DB snapshot, so they need `--db`.
pub fn is_composite(name: &str) -> bool {
    COMPOSITES.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// Expands `--set` names (composites included) to concrete sets, in order, without repeats.
/// A name that is neither a known set nor a composite, and a composite member that is not
/// implemented yet, are errors; nothing is skipped silently.
pub fn resolve_sets(names: &[String]) -> Result<Vec<&'static SetDef>, String> {
    let mut out: Vec<&'static SetDef> = Vec::new();
    for name in names {
        let members: Vec<&str> = match COMPOSITES
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some((_, members)) => members.to_vec(),
            None => vec![name.as_str()],
        };
        for m in members {
            if m.eq_ignore_ascii_case(GAME_DATA) || is_table_set(m) {
                continue;
            }
            let Some(def) = find_set(m) else {
                let known: Vec<_> = SETS
                    .iter()
                    .map(|s| s.name)
                    .chain(TABLE_SETS.iter().copied())
                    .chain(COMPOSITES.iter().map(|c| c.0))
                    .collect();
                return Err(if is_composite(name) {
                    format!("set {m:?} (part of {name:?}) is not implemented yet")
                } else {
                    format!("unknown set {m:?}; known sets: {}", known.join(", "))
                });
            };
            if !out.iter().any(|d| std::ptr::eq(*d, def)) {
                out.push(def);
            }
        }
    }
    Ok(out)
}

pub fn texture_id(file: &str) -> LogicalId {
    LogicalId::new(format!("tex:{}", file.to_ascii_lowercase()))
}

pub fn model_id(member: &str) -> LogicalId {
    LogicalId::new(format!("model:{}", member.to_ascii_lowercase()))
}

pub fn anim_id(file: &str) -> LogicalId {
    LogicalId::new(format!("anim:{}", file.to_ascii_lowercase()))
}

/// A unit member with all of its input files read.
pub struct LoadedMember {
    pub resolved: ResolvedMember,
    /// Textures per material, with the report entries of every guess.
    pub plan: MaterialPlan,
    /// Problems that did not stop the load (dropped clips).
    pub load_warnings: Vec<String>,
    fxsxml: Vec<u8>,
    mesh: Vec<u8>,
    /// Parallel to `resolved.animations()`.
    anims: Vec<Vec<u8>>,
}

fn read_required(game: &GameSource, name: &str) -> Result<Vec<u8>, ModelError> {
    game.read_by_name(name)?
        .ok_or_else(|| ModelError::invalid(format!("{name} is missing from the install")))
}

fn hash_of(bytes: &[u8]) -> ContentHash {
    ContentHash(*blake3::hash(bytes).as_bytes())
}

pub fn load_member(
    game: &GameSource,
    defs: &ArtDefines,
    member_id: &str,
) -> Result<LoadedMember, ModelError> {
    load_resolved(game, defs.resolve_member(game, member_id)?)
}

/// Loads a model named by its fxsxml (`ArtDefine_UnitMemberInfos.Model` from the DB snapshot,
/// or a city building set) under the id `id`; `scale` is the member's `Scale` (1 for cities).
pub fn load_fxsxml(
    game: &GameSource,
    id: &str,
    fxsxml: &str,
    scale: f32,
) -> Result<LoadedMember, ModelError> {
    load_resolved(game, resolve_fxsxml(game, id, fxsxml, scale, None)?)
}

/// Every missing file is fatal except an animation clip: the clip is dropped with a load warning
/// (`U_Assyrian_Siege_Tower_Charge_Attack.gr2` is listed but not shipped), so the model still
/// builds with the clips that exist.
fn load_resolved(
    game: &GameSource,
    mut resolved: ResolvedMember,
) -> Result<LoadedMember, ModelError> {
    let id = resolved.id.clone();
    let mut load_warnings = Vec::new();
    let mut missing_clips = BTreeSet::new();
    let fatal: Vec<String> = resolved
        .warnings
        .iter()
        .filter(|w| match w {
            ArtWarning::MissingFile {
                role: FileRole::Animation,
                name,
            } => {
                load_warnings.push(format!("{id}: animation {name} is missing, clip dropped"));
                missing_clips.insert(name.to_ascii_lowercase());
                false
            }
            ArtWarning::MissingFile { .. } => true,
            ArtWarning::BadFxsxml { .. } => false,
        })
        .map(|w| w.to_string())
        .collect();
    if !fatal.is_empty() {
        return Err(ModelError::invalid(format!("{id}: {}", fatal.join("; "))));
    }
    resolved
        .model
        .animations
        .retain(|a| !missing_clips.contains(&a.file.to_ascii_lowercase()));
    let mesh_name = match resolved.meshes() {
        [one] => one.clone(),
        other => {
            return Err(ModelError::invalid(format!(
                "{id}: expected one mesh, found {}",
                other.len()
            )));
        }
    };
    let fxsxml = read_required(game, &resolved.fxsxml)?;
    let mesh = read_required(game, &mesh_name)?;
    let anims = resolved
        .animations()
        .iter()
        .map(|a| read_required(game, &a.file))
        .collect::<Result<Vec<_>, _>>()?;
    let granny_materials = GrannyFile::parse(&mesh)
        .and_then(|f| read_materials(&f))
        .map_err(|source| ModelError::Granny {
            what: mesh_name.clone(),
            source,
        })?;
    let plan = plan_materials(
        &resolved.fxsxml,
        &mesh_name,
        &resolved.model.textures,
        &granny_materials,
    );
    Ok(LoadedMember {
        resolved,
        plan,
        load_warnings,
        fxsxml,
        mesh,
        anims,
    })
}

/// Output of converting one member.
pub struct MemberBlobs {
    pub model: ModelBlob,
    /// Unique clips in first-use order, by logical id.
    pub clips: Vec<(LogicalId, AnimationBlob)>,
    pub warnings: Vec<String>,
}

impl LoadedMember {
    /// Diffuse file of the first material.
    pub fn diffuse_file(&self) -> Option<&str> {
        self.plan.materials.first().and_then(|m| m.0.as_deref())
    }

    /// Specular file of the first material.
    pub fn spec_file(&self) -> Option<&str> {
        self.plan.materials.first().and_then(|m| m.1.as_deref())
    }

    /// Texture files the materials use, in material order, DIFF before SREF, without repeats.
    pub fn texture_files(&self) -> Vec<&str> {
        self.plan.texture_files()
    }

    /// Engine materials, in the order mesh groups index them.
    pub fn model_materials(&self) -> ModelMaterials {
        ModelMaterials {
            materials: self
                .plan
                .materials
                .iter()
                .map(|(d, s)| Material {
                    diffuse: d.as_deref().map(texture_id),
                    specular: s.as_deref().map(texture_id),
                })
                .collect(),
            by_granny: self.plan.by_granny.clone(),
        }
    }

    pub(crate) fn input_hashes(&self) -> Vec<ContentHash> {
        [&self.fxsxml, &self.mesh]
            .into_iter()
            .chain(&self.anims)
            .map(|b| hash_of(b))
            .collect()
    }

    pub(crate) fn step_params(&self) -> Vec<u8> {
        let mut p = self.resolved.id.to_ascii_lowercase().into_bytes();
        for (d, s) in &self.plan.materials {
            for f in [d, s] {
                p.push(0);
                p.extend_from_slice(f.as_deref().unwrap_or("").to_ascii_lowercase().as_bytes());
            }
        }
        for m in &self.plan.by_granny {
            p.extend_from_slice(&m.to_le_bytes());
        }
        p
    }

    /// Engine materials (indices into [`Self::model_materials`]) whose diffuse is a damage-state
    /// texture next to an intact one (B8 `NoteKind::DamageState`).
    pub fn damage_materials(&self) -> Vec<u16> {
        let mut out: Vec<u16> = self
            .plan
            .notes
            .iter()
            .filter(|n| n.kind == NoteKind::DamageState)
            .filter_map(|n| {
                if self.plan.by_granny.is_empty() {
                    Some(0)
                } else {
                    self.plan.by_granny.get(n.material).copied()
                }
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The static model of this member for B1: frame 0 of the idle clip baked into the vertices,
    /// damage-state hulls dropped (see [`super::bake`]). The animated blobs come back too.
    pub fn convert_static(&self) -> Result<(StaticModel, MemberBlobs), ModelError> {
        let blobs = self.convert()?;
        let mut stat = bake_static(&blobs.model, &blobs.clips, &self.damage_materials())?;
        let id = &self.resolved.id;
        stat.warnings = stat.warnings.iter().map(|w| format!("{id}: {w}")).collect();
        Ok((stat, blobs))
    }

    pub fn convert(&self) -> Result<MemberBlobs, ModelError> {
        let basis = Basis::CIV5_TO_ENGINE;
        let granny_err = |what: &str| {
            let what = what.to_owned();
            move |source| ModelError::Granny { what, source }
        };
        let mesh_name = &self.resolved.meshes()[0];
        let file = GrannyFile::parse(&self.mesh).map_err(granny_err(mesh_name))?;
        let scene = Scene::read(&file).map_err(granny_err(mesh_name))?;
        let converted = convert_model(&scene, &basis, &self.model_materials())?;
        let mut warnings: Vec<String> = converted
            .warnings
            .iter()
            .map(|w| format!("{mesh_name}: {w}"))
            .collect();
        let mut model = converted.model;
        let skel = &scene.skeletons[converted.skeleton];
        let rest = model.skeleton.rest_pose();

        let anims = self.resolved.animations();
        let results: Vec<Result<_, ModelError>> = anims
            .par_iter()
            .zip(&self.anims)
            .map(|(a, bytes)| {
                let file = GrannyFile::parse(bytes).map_err(granny_err(&a.file))?;
                let scene = Scene::read(&file).map_err(granny_err(&a.file))?;
                let anim = scene
                    .animations
                    .first()
                    .ok_or_else(|| ModelError::invalid(format!("{}: no animation", a.file)))?;
                let mut c = convert_animation(anim, skel, &rest, &basis)
                    .map_err(|e| ModelError::invalid(format!("{}: {e}", a.file)))?;
                if scene.animations.len() > 1 {
                    c.warnings
                        .push("several animations in one file; using the first".into());
                }
                Ok(c)
            })
            .collect();

        let mut clips: Vec<(LogicalId, AnimationBlob)> = Vec::new();
        for (a, result) in anims.iter().zip(results) {
            let c = result?;
            warnings.extend(c.warnings.iter().map(|w| format!("{}: {w}", a.file)));
            let id = anim_id(&a.file);
            if !clips.iter().any(|(i, _)| *i == id) {
                clips.push((id.clone(), c.blob));
            }
            model.animations.push(AnimRef {
                name: a
                    .file
                    .rsplit_once('.')
                    .map_or(a.file.as_str(), |(s, _)| s)
                    .to_owned(),
                blob: id,
                // fxsxml has no per-event timing: all codes sit at clip start.
                events: a
                    .event_codes
                    .iter()
                    .map(|&code| AnimEvent { time: 0.0, code })
                    .collect(),
            });
        }
        Ok(MemberBlobs {
            model,
            clips,
            warnings,
        })
    }
}

/// Color space of a texture file, from its name.
fn color_space(file: &str) -> ColorSpace {
    color_space_for(file)
}

#[derive(Serialize, Deserialize)]
struct TextureStepOut {
    variants: Vec<(VariantFormat, ContentHash, u64)>,
}

#[derive(Serialize, Deserialize)]
struct ModelStepOut {
    model: (ContentHash, u64),
    clips: Vec<(String, ContentHash, u64)>,
}

/// What `build_set` produced.
#[derive(Debug)]
pub struct BuildSummary {
    pub manifest: ContentHash,
    pub members: usize,
    pub textures: usize,
    pub animations: usize,
    pub stats: StoreStats,
    /// Includes one line per material note.
    pub warnings: Vec<String>,
    /// Material guesses, inferences and damage-state materials of every member (they are also
    /// in `warnings`, so the CLI prints them on every build, cached or not).
    pub material_notes: Vec<MaterialNote>,
    pub seconds: f64,
}

const TEXTURE_FORMATS: [(TextureFormat, VariantFormat); 3] = [
    (TextureFormat::Rgba8, VariantFormat::Uncompressed),
    (TextureFormat::Bc7, VariantFormat::Bc),
    (TextureFormat::Astc4x4, VariantFormat::Astc),
];

fn texture_step(
    game: &GameSource,
    cache: &BuildCache<'_>,
    store: &BlobStore,
    file: &str,
) -> Result<TextureStepOut, ModelError> {
    let bytes = read_required(game, file)?;
    let space = color_space(file);
    let key = CacheKey::new(
        "texture",
        TEXTURE_STEP_VERSION,
        &[hash_of(&bytes)],
        &[u8::from(space == ColorSpace::Srgb)],
    );
    cache.get_or_compute(
        &key,
        |out: &TextureStepOut| out.variants.iter().map(|v| v.1).collect(),
        || {
            let formats = TEXTURE_FORMATS.map(|f| f.0);
            let blobs =
                convert_dds(&bytes, space, &formats).map_err(|source| ModelError::Texture {
                    name: file.to_owned(),
                    source,
                })?;
            let mut variants = Vec::new();
            for (blob, (_, variant)) in blobs.iter().zip(TEXTURE_FORMATS) {
                let (hash, size) = store.put(&blob.encode())?;
                variants.push((variant, hash, size));
            }
            Ok(TextureStepOut { variants })
        },
    )
}

fn model_step(
    cache: &BuildCache<'_>,
    store: &BlobStore,
    m: &LoadedMember,
    warnings: &mut Vec<String>,
) -> Result<ModelStepOut, ModelError> {
    let key = CacheKey::new(
        "model",
        MODEL_STEP_VERSION,
        &m.input_hashes(),
        &m.step_params(),
    );
    let mut fresh_warnings = Vec::new();
    let out = cache.get_or_compute(
        &key,
        |o: &ModelStepOut| {
            std::iter::once(o.model.0)
                .chain(o.clips.iter().map(|c| c.1))
                .collect()
        },
        || {
            let blobs = m.convert()?;
            fresh_warnings = blobs.warnings;
            let model = store.put(&blobs.model.encode())?;
            let mut clips = Vec::new();
            for (id, clip) in &blobs.clips {
                let (hash, size) = store.put(&clip.encode())?;
                clips.push((id.as_str().to_owned(), hash, size));
            }
            Ok::<_, ModelError>(ModelStepOut { model, clips })
        },
    )?;
    warnings.extend(fresh_warnings);
    Ok(out)
}

/// Builds `def` into `out`: blobs, build cache and the `manifest` pointer file.
pub fn build_set(game: &GameSource, def: &SetDef, out: &Path) -> Result<BuildSummary, ModelError> {
    build_sets(game, &[def], out)
}

/// Builds several sets with one `ManifestBuilder`. The builder starts from the manifest already
/// in `out`, so earlier builds are kept and the manifest is never overwritten by a smaller one.
pub fn build_sets(
    game: &GameSource,
    sets: &[&SetDef],
    out: &Path,
) -> Result<BuildSummary, ModelError> {
    let started = Instant::now();
    let defs = ArtDefines::load(game)?;
    let store = BlobStore::open(out)?;
    let cache = BuildCache::open(&store)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let mut warnings = Vec::new();

    // A member listed in several sets is built once.
    let mut ids: Vec<&str> = Vec::new();
    for id in sets.iter().flat_map(|s| s.members.iter().copied()) {
        if !ids.iter().any(|seen| seen.eq_ignore_ascii_case(id)) {
            ids.push(id);
        }
    }
    let members: Vec<LoadedMember> = ids
        .iter()
        .map(|id| load_member(game, &defs, id))
        .collect::<Result<_, _>>()?;
    let material_notes: Vec<MaterialNote> = members
        .iter()
        .flat_map(|m| m.plan.notes.iter().cloned())
        .collect();
    warnings.extend(members.iter().flat_map(|m| m.load_warnings.iter().cloned()));
    warnings.extend(material_notes.iter().map(|n| n.to_string()));

    // Textures shared between members are converted once.
    let mut texture_files: BTreeMap<String, &str> = BTreeMap::new();
    for m in &members {
        for f in m.texture_files() {
            texture_files.entry(f.to_ascii_lowercase()).or_insert(f);
        }
    }
    let files: Vec<&str> = texture_files.values().copied().collect();
    let textures: Vec<Result<TextureStepOut, ModelError>> = files
        .par_iter()
        .map(|f| texture_step(game, &cache, &store, f))
        .collect();
    for (file, result) in files.iter().zip(textures) {
        for (format, hash, size) in result?.variants {
            manifest.add(
                texture_id(file),
                Variant {
                    format,
                    hash,
                    size,
                    prefetch: PREFETCH,
                },
            );
        }
    }

    let mut animations = 0;
    for m in &members {
        let out = model_step(&cache, &store, m, &mut warnings)?;
        let generic = |hash, size| Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: PREFETCH,
        };
        manifest.add(model_id(&m.resolved.id), generic(out.model.0, out.model.1));
        for (id, hash, size) in out.clips {
            manifest.add(LogicalId::new(id), generic(hash, size));
            animations += 1;
        }
        tracing::info!(
            member = %m.resolved.id,
            f_scale = m.resolved.scale,
            engine_scale = m.resolved.scale * ENGINE_UNITS_PER_CIV5_UNIT,
            "built"
        );
    }

    let hash = manifest.finish(&store)?;
    Ok(BuildSummary {
        manifest: hash,
        members: members.len(),
        textures: texture_files.len(),
        animations,
        stats: store.stats(),
        warnings,
        material_notes,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// Texture blob of `file` as plain RGBA8, for the glTF export.
pub fn rgba_texture(game: &GameSource, file: &str) -> Result<TextureBlob, ModelError> {
    let bytes = read_required(game, file)?;
    let mut blobs =
        convert_dds(&bytes, color_space(file), &[TextureFormat::Rgba8]).map_err(|source| {
            ModelError::Texture {
                name: file.to_owned(),
                source,
            }
        })?;
    blobs
        .pop()
        .ok_or_else(|| ModelError::invalid("texture conversion returned nothing"))
}

/// GLB bytes for one unit member (`model_id` as in the art defines, any case).
pub fn export_member_glb(game: &GameSource, member_id: &str) -> Result<Vec<u8>, ModelError> {
    let defs = ArtDefines::load(game)?;
    let loaded = load_member(game, &defs, member_id)?;
    loaded_glb(game, &loaded)
}

/// GLB bytes for a model loaded by [`load_fxsxml`] or [`load_member`], with the diffuse texture
/// of every material.
pub fn loaded_glb(game: &GameSource, loaded: &LoadedMember) -> Result<Vec<u8>, ModelError> {
    let blobs = loaded.convert()?;
    let mut diffuse: Vec<(LogicalId, TextureBlob)> = Vec::new();
    for f in loaded.plan.materials.iter().filter_map(|m| m.0.as_deref()) {
        let id = texture_id(f);
        if !diffuse.iter().any(|(i, _)| *i == id) {
            diffuse.push((id, rgba_texture(game, f)?));
        }
    }
    let names: BTreeMap<&str, &str> = blobs
        .model
        .animations
        .iter()
        .map(|a| (a.blob.as_str(), a.name.as_str()))
        .collect();
    let clips: Vec<(String, AnimationBlob)> = blobs
        .clips
        .iter()
        .map(|(id, blob)| {
            let name = names.get(id.as_str()).copied().unwrap_or(id.as_str());
            (name.to_owned(), blob.clone())
        })
        .collect();
    let scale = loaded.resolved.scale * ENGINE_UNITS_PER_CIV5_UNIT;
    super::glb::write_glb(&blobs.model, &clips, &diffuse, scale)
}

#[cfg(test)]
mod set_resolution_tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn several_sets_resolve_in_order_without_repeats() {
        let d = resolve_sets(&names(&["warrior", "WARRIOR"])).unwrap();
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn composite_b1_resolves_to_no_unit_sets() {
        assert!(resolve_sets(&names(&["b1"])).unwrap().is_empty());
    }

    #[test]
    fn b1_table_is_a_table_set() {
        assert!(is_table_set("B1-Table"));
    }

    #[test]
    fn b1_terrain_is_a_table_set_not_a_unit_set() {
        assert!(resolve_sets(&names(&["b1-terrain"])).unwrap().is_empty());
        assert_eq!(
            resolve_sets(&names(&["B1-Terrain", "warrior"]))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn b1_trees_and_b1_cities_are_table_sets() {
        for n in ["b1-trees", "B1-Cities"] {
            assert!(is_table_set(n));
            assert!(resolve_sets(&names(&[n])).unwrap().is_empty());
        }
    }

    #[test]
    fn b1_sv_is_a_table_set() {
        assert!(is_table_set("B1-SV"));
        assert!(resolve_sets(&names(&["b1-sv"])).unwrap().is_empty());
    }

    #[test]
    fn game_data_is_not_a_unit_set_and_expands_from_b1() {
        assert!(resolve_sets(&names(&["game-data"])).unwrap().is_empty());
        let e = expand_names(&names(&["warrior", "b1", "GAME-DATA"]));
        assert_eq!(e.first().map(String::as_str), Some("warrior"));
        assert_eq!(e.iter().filter(|n| *n == "game-data").count(), 1);
    }

    #[test]
    fn unknown_set_is_an_error() {
        let e = resolve_sets(&names(&["nope"])).unwrap_err();
        assert!(e.contains("unknown set"), "{e}");
    }
}
