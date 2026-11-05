//! The model builder the B2 model sets share (`b2-units`, `b2-landmarks`, `b2-city`, `b2-misc`):
//! a list of `.fxsxml` names in, `C5MD` v2 blobs with their FGX
//! materials, the textures of the materials as the original's bytes, the clip packs and the idle
//! state machines in the store and the manifest.
//!
//! A set decides which models it builds, how their meshes merge ([`MergeScope`]) and what kind
//! of clips they carry ([`ModelKind`]); what the models mean (the art table records, the csv) is
//! the set's own. Nothing is dropped and no stand-in is drawn (AB-D14): a model that cannot be
//! built is in [`BuiltModels::failed`] and the set fails after it has written its reports.
//!
//! # Materials
//! The FGX of every material is read ([`crate::models::material_desc`]); texture paths are the
//! explained ones. A shader-set name nobody knows fails the model. An unknown constant-buffer or
//! parameter name (`#xxxxxxxx`, the shader catalog does not have it yet) is listed in the report and
//! does not fail it.
//!
//! SIMPLIFIED(SIMP-061): a texture that a material names but the install does not ship (3 files:
//! `Parrott_cannon_DIFF`, `SteamfighterProp_DIFF`, `SteamfighterProp_SREF`; plus the SREF of
//! `comparsa_tambor`, whose path nothing explains) is replaced by a
//! neutral 4x4 texture under the id of the missing file (mid grey for a diffuse, black for a
//! specular map), listed in the report as `stub`; a real file in the install replaces it by itself.
//! The world models (R4.1 to R4.3) have the same case for a path that no file name of the install
//! explains (a lookup texture of the exporter's machine: a colour-correction ramp, a height map of
//! a decal, a fog colour key): the stand-in is `unresolved_<hash>.dds`, one per path hash, and the
//! model's warnings and the `stub` entries of the report name it. It is not an extension of the
//! simplification to a unit: units keep failing on such a path.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use asset_format::{
    IdleFsm, LogicalId, MaterialDesc, ModelBlobV2, ParamSet, PrefetchGroup, TexKind, TexParam,
    TexSource, TextureBlob, TextureFormat, Variant, VariantFormat,
};
use granny::curve::CurveKind;
use granny::{GrannyFile, Scene};
use rayon::prelude::*;

use super::{an_id, fsm_id, tx_id};
use crate::anim::fsm::{ddtxml_animation_items, parse_ddtxml, parse_fsmxml};
use crate::anim::idle::{
    JoinedSkeleton, ModelKind, ModelPack, PackSetup, build_pack_joined, clip_tracks,
};
use crate::anim::resample::Tolerance;
use crate::art::{
    ArtWarning, FileRole, FxsTexture, ResolvedMember, TextureSlot, resolve_fxsxml_text,
};
use crate::game::GameSource;
use crate::models::ModelError;
use crate::models::bake_v2::{BakeInput, BakeStats, bake_model, primary_skeleton, skeleton_map};
use crate::models::basis::Basis;
use crate::models::fgx::{NameDict, NameRole, Unresolved};
use crate::models::material_desc::{
    Gr2Materials, ModelFiles, PathResolver, build_descs, load_shader_catalog, path_hashes,
};
use crate::models::merge::MergeScope;
use crate::report::ReportEntry;
use crate::store::{BlobStore, ManifestBuilder};
use crate::textures::{dds_passthrough, variant_format};
use crate::world::art_table::md_id;

/// Settings of a build.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `catalog.json` of `shader-re`: names for the FGX blocks.
    pub shader_catalog: Option<PathBuf>,
    /// Tolerance of the packed clips (SIMP-029).
    pub clip_tolerance: Tolerance,
}

/// One model to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelJob {
    /// Flat lower-case `.fxsxml` name.
    pub flat: String,
    /// Which clips beyond the idle family the pack holds.
    pub kind: ModelKind,
    /// How the meshes merge: only inside a gr2 `Model` (a layout handler places the parts) or over
    /// the whole file.
    pub scope: MergeScope,
    /// The asset list as text, for a model that has no `.fxsxml` of its own (`<Asset>` with the
    /// `Mesh` and `Texture` lines); `None` reads `flat` from the install.
    pub xml: Option<String>,
    /// The texture the game binds as the diffuse of every material, whatever the gr2 names. The
    /// trees: `ForestVis` loads `<stem>.dds` by name, and `forest_asia_2.gr2` names
    /// `forest_europe.dds` (golden S2: the draws bind `forest_asia_2.dds`, byte for byte).
    pub diffuse: Option<String>,
}

/// An fxsxml with its files read.
pub struct Loaded {
    pub flat: String,
    pub resolved: ResolvedMember,
    /// The gr2 with the mesh.
    pub mesh_name: String,
    mesh: Vec<u8>,
    mats: Gr2Materials,
    /// Problems that did not stop the load.
    pub warnings: Vec<String>,
}

pub fn load_model(game: &GameSource, flat: &str, xml: Option<&str>) -> Result<Loaded, String> {
    let resolved =
        resolve_fxsxml_text(game, flat, flat, 1.0, None, xml).map_err(|e| e.to_string())?;
    let mut warnings = Vec::new();
    for w in &resolved.warnings {
        match w {
            // A clip the install does not ship is dropped from the model (R2.2a lists them).
            ArtWarning::MissingFile {
                role: FileRole::Animation,
                name,
            } => warnings.push(format!("animation {name} is not in the install")),
            // The fxsxml is a hint; what a material names decides.
            ArtWarning::MissingFile {
                role: FileRole::Texture,
                name,
            } => warnings.push(format!(
                "texture {name} of the fxsxml is not in the install"
            )),
            ArtWarning::MissingFile {
                role: FileRole::TimedTrigger | FileRole::AnimGraph | FileRole::DecisionTree,
                name,
            } => warnings.push(format!("{name} of the fxsxml is not in the install")),
            other => return Err(other.to_string()),
        }
    }
    let [mesh_name] = resolved.meshes() else {
        return Err(format!(
            "expected one mesh, found {}",
            resolved.meshes().len()
        ));
    };
    let mesh_name = mesh_name.clone();
    let mesh = game
        .read_by_name(&mesh_name)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("{mesh_name} is missing from the install"))?;
    let mats = Gr2Materials::read(&mesh).map_err(|e| format!("{mesh_name}: {e}"))?;
    Ok(Loaded {
        flat: flat.to_owned(),
        resolved,
        mesh_name,
        mesh,
        mats,
        warnings,
    })
}

/// Bones that a clip of the model moves (a track with any curve that is not the identity), over
/// every animation of the fxsxml. Only read when the model has a rigid mesh on a bone. With
/// `multi` the bones are numbers in the joined skeleton of the catalog.
fn animated_bones_of(
    game: &GameSource,
    l: &Loaded,
    scene: &Scene,
    multi: bool,
) -> Result<BTreeSet<u16>, String> {
    let needs = scene
        .meshes
        .iter()
        .any(|m| !m.is_skinned() && m.bone_bindings.len() == 1);
    let mut out = BTreeSet::new();
    if !needs {
        return Ok(out);
    }
    let joined = if multi {
        joined_skeletons(scene)?
    } else {
        // The skeleton the blob is built on (the skinned model's), not the first of the file.
        let (_, si) = primary_skeleton(scene).map_err(|e| e.to_string())?;
        vec![(&scene.skeletons[si], 0u16)]
    };
    let joined: Vec<JoinedSkeleton<'_>> = joined
        .into_iter()
        .map(|(skeleton, offset)| JoinedSkeleton { skeleton, offset })
        .collect();
    for a in l.resolved.animations() {
        let Some(bytes) = game.read_by_name(&a.file).map_err(|e| e.to_string())? else {
            continue;
        };
        let file = GrannyFile::parse(&bytes).map_err(|e| format!("{}: {e}", a.file))?;
        let clip = Scene::read(&file).map_err(|e| format!("{}: {e}", a.file))?;
        for anim in &clip.animations {
            let tracks = if multi {
                // A catalog clip names a track group after a model; one that matches nothing
                // moves nothing here.
                clip_tracks(anim, &joined).unwrap_or_default()
            } else {
                // The unit model has one skeleton and every track group binds to it.
                let skel = joined[0].skeleton;
                anim.track_groups
                    .iter()
                    .flat_map(|group| {
                        let binding = granny::bind::bind(skel, group);
                        group
                            .transform_tracks
                            .iter()
                            .zip(binding.track_to_bone)
                            .filter_map(|(t, b)| b.map(|b| (b, t)))
                            .collect::<Vec<_>>()
                    })
                    .collect()
            };
            for (bone, track) in tracks {
                let moves = [
                    track.position.kind,
                    track.orientation.kind,
                    track.scale_shear.kind,
                ]
                .iter()
                .any(|k| *k != CurveKind::Identity);
                if moves && let Ok(b) = u16::try_from(bone) {
                    out.insert(b);
                }
            }
        }
    }
    Ok(out)
}

/// Why a model has no clip of event code 1000, from what the install gives: no clip anywhere (the
/// model is static in the original too), clip files the install does not ship, or clips whose
/// codes the idle selection does not take (a pipeline gap, the line says which codes).
fn no_idle_evidence(game: &GameSource, l: &Loaded, b: &Built) -> String {
    let anims = l.resolved.animations();
    let mesh = if b.mesh_anims == 0 {
        "the mesh gr2 has no animation either"
    } else {
        "the mesh gr2 has its own animations (not read)"
    };
    if anims.is_empty() {
        let source = if game.lookup(&l.flat).is_some() {
            "the fxsxml names no Animation"
        } else {
            "no fxsxml of this name in the install (a mesh-only model)"
        };
        return format!("static in the original: {source}; {mesh}");
    }
    let shipped: Vec<&crate::art::FxsAnim> = anims
        .iter()
        .filter(|a| game.lookup(&a.file).is_some())
        .collect();
    if shipped.is_empty() {
        let files: Vec<&str> = anims.iter().map(|a| a.file.as_str()).collect();
        return format!(
            "static in the original: the install ships none of the clips the fxsxml names ({})",
            files.join(", ")
        );
    }
    let codes: BTreeSet<u32> = shipped
        .iter()
        .flat_map(|a| a.event_codes.iter().copied())
        .collect();
    let packed: Vec<String> = b
        .pack
        .iter()
        .flat_map(|p| p.clips.iter().map(|c| c.ec.to_string()))
        .collect();
    format!(
        "no clip of code 1000: the fxsxml clips have codes {codes:?}, packed [{}]",
        packed.join(", ")
    )
}

/// The skeletons the models of a catalog use, each with the number of its first bone in the
/// joined skeleton ([`skeleton_map`]).
fn joined_skeletons(scene: &Scene) -> Result<Vec<(&granny::skeleton::Skeleton, u16)>, String> {
    let map =
        skeleton_map(scene, &Basis::CIV5_TO_ENGINE, &mut Vec::new()).map_err(|e| e.to_string())?;
    Ok(map
        .offsets
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.map(|o| (&scene.skeletons[i], o)))
        .collect())
}

/// `1_omni_state_01` for `1_Omni_State_01.fsmxml`.
fn fsm_stem(file: &str) -> String {
    file.rsplit_once('.')
        .map_or(file, |(s, _)| s)
        .to_ascii_lowercase()
}

/// Reads and compiles one `.fsmxml` (the copy of the install that wins, Lekmod's over the base).
fn read_fsm(game: &GameSource, file: &str) -> Result<IdleFsm, String> {
    let bytes = game
        .read_by_name(file)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "is not in the install".to_owned())?;
    parse_fsmxml(&fsm_stem(file), &String::from_utf8_lossy(&bytes))
}

/// A texture a material names that the install does not ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Substitute {
    /// Lower-case file name.
    pub file: String,
    /// Diffuse-like or specular-like by the name.
    pub specular: bool,
}

/// Makes `id` the diffuse of every material ([`ModelJob::diffuse`]): the sampler by the names
/// `to_static_v1` reads, else the first file texture, else a slot of its own.
fn bind_diffuse(descs: &mut [Option<MaterialDesc>], id: &LogicalId) {
    for desc in descs.iter_mut().flatten() {
        let named = |n: &str| {
            matches!(
                n.to_ascii_lowercase().as_str(),
                "basetexturemap" | "basesampler" | "diffuse"
            )
        };
        let has_named = desc
            .sets
            .iter()
            .flat_map(|s| &s.textures)
            .any(|t| named(&t.name));
        let mut first = true;
        for tex in desc.sets.iter_mut().flat_map(|s| s.textures.iter_mut()) {
            let file = matches!(tex.src, TexSource::File(_));
            if (has_named && named(&tex.name)) || (!has_named && file && first) {
                tex.src = TexSource::File(id.clone());
            }
            first &= has_named || !file;
        }
        // No slot to take it: `forest_generic.gr2` has a `SimpleShader` material whose samplers
        // are all engine ones with unreadable names. The diffuse is given its own slot, the
        // name `to_static_v1` reads.
        if !has_named && first {
            desc.sets.push(ParamSet {
                textures: vec![TexParam {
                    name: "Diffuse".to_owned(),
                    kind: TexKind::Tex2D,
                    src: TexSource::File(id.clone()),
                    semantic: -1,
                    reg: 0,
                    usage: 0,
                    flags: 0,
                    active: true,
                }],
                ..ParamSet::default()
            });
        }
    }
}

/// The neutral stand-in: 4x4 RGBA8, mid grey (diffuse) or black (specular map), alpha 255.
// SIMPLIFIED(SIMP-061): see the module comment.
pub fn neutral_texture(specular: bool) -> TextureBlob {
    let px: [u8; 4] = if specular {
        [0, 0, 0, 255]
    } else {
        [128, 128, 128, 255]
    };
    let level: Vec<u8> = px.iter().copied().cycle().take(4 * 4 * 4).collect();
    TextureBlob::new_2d(TextureFormat::Rgba8, false, 4, 4, vec![level])
}

/// A weak guess gave a material the intact diffuse of the model because nothing explained its
/// path. When the fxsxml names exactly one damage-state texture that no material uses and exactly
/// one diffuse is weak, that texture is the damaged hull's (`Ironclad_DMG.dds`; its folder is the
/// one nothing confirms). The step is by elimination and is listed in the report.
fn damage_elimination(
    descs: &mut [Option<MaterialDesc>],
    inferred: &[crate::models::material_desc::InferredTexture],
    textures: &[FxsTexture],
    notes: &mut Vec<String>,
) {
    let is_diffuse = |n: &str| {
        matches!(
            n.to_ascii_lowercase().as_str(),
            "basetexturemap" | "diffuse"
        )
    };
    let weak: Vec<_> = inferred
        .iter()
        .filter(|i| i.weak && is_diffuse(&i.sampler))
        .collect();
    let used: BTreeSet<String> = descs
        .iter()
        .flatten()
        .flat_map(MaterialDesc::texture_files)
        .map(|id| id.as_str().trim_start_matches("tx:").to_owned())
        .collect();
    let open: Vec<&FxsTexture> = textures
        .iter()
        .filter(|t| {
            crate::models::materials::is_damage_texture(&t.file)
                && !used.contains(&t.file.to_ascii_lowercase())
                && matches!(
                    t.slot,
                    TextureSlot::Diffuse | TextureSlot::Other | TextureSlot::Mask
                )
        })
        .collect();
    let ([w], [t]) = (weak.as_slice(), open.as_slice()) else {
        return;
    };
    let Some(Some(desc)) = descs.get_mut(w.material) else {
        return;
    };
    for set in &mut desc.sets {
        for tex in &mut set.textures {
            if tex.name == w.sampler {
                tex.src = TexSource::File(tx_id(&t.file));
            }
        }
    }
    notes.push(format!(
        "material {} sampler {}: {} instead of the guess {} (the only damage-state texture of the fxsxml that no material uses)",
        w.material, w.sampler, t.file, w.file
    ));
}

/// A built model before it is stored.
pub struct Built {
    pub model: ModelBlobV2,
    /// The `an:` pack of the model and what it holds.
    pub pack: Option<ModelPack>,
    pub stats: BakeStats,
    pub warnings: Vec<String>,
    /// FGX names nobody knows: `(role, hash)`.
    pub unresolved: Vec<Unresolved>,
    /// Weak guesses of texture files by sampler role (material, sampler, file).
    pub weak: Vec<String>,
    /// Textures chosen by elimination (see [`damage_elimination`]).
    pub notes: Vec<String>,
    pub substitutes: Vec<Substitute>,
    /// Texture files named by the fxsxml and used by no material.
    pub unused_textures: Vec<String>,
    /// The decision tree of the fxsxml and the number of items in its animation layer
    /// ([`ddtxml_animation_items`]; zero: a static tree).
    pub tree: Option<(String, usize)>,
    /// Animations stored in the mesh gr2 itself.
    pub mesh_anims: usize,
}

pub fn texture_files_of(descs: &[MaterialDesc]) -> BTreeSet<String> {
    descs
        .iter()
        .flat_map(MaterialDesc::texture_files)
        .map(|id| id.as_str().trim_start_matches("tx:").to_owned())
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn build_model(
    game: &GameSource,
    job: &ModelJob,
    l: &Loaded,
    resolver: &PathResolver,
    extra: &[String],
    dict: &NameDict,
    clip_tolerance: &Tolerance,
    fsms: &BTreeMap<String, Result<IdleFsm, String>>,
) -> Result<Built, String> {
    let exists = |f: &str| game.lookup(f).is_some();
    let files = ModelFiles {
        fxsxml: &l.flat,
        mesh: &l.mesh_name,
        textures: &l.resolved.model.textures,
    };
    let mm =
        build_descs(&l.mats, &files, resolver, extra, dict, &exists).map_err(|e| e.to_string())?;

    // A shader set nobody knows is an error; an unknown block or parameter name is a report line.
    if let Some(u) = mm.unresolved.iter().find(|u| u.role == NameRole::ShaderSet) {
        return Err(format!("the shader set of a material is unknown: {u}"));
    }
    // A path nothing explains is an error; one that names a file the install lacks is replaced.
    let mut substitutes: Vec<Substitute> = Vec::new();
    let mut pre_warnings: Vec<String> = Vec::new();
    let mut descs = mm.descs.clone();
    for t in &mm.unresolved_textures {
        // The specular reference of `comparsa_tambor` has a path nothing in the install
        // explains (its fxsxml lists the files of another model): it is a stub like a missing file.
        let guess = (t.sampler == "SREFMap")
            .then(|| format!("{}_sref.dds", l.flat.trim_end_matches(".fxsxml")));
        // A path nothing explains (a shared lookup texture of the exporter's machine that no
        // file name of the install matches) gets one neutral stand-in per path hash; the
        // report lists it, so the gap stays visible.
        let unexplained = format!("unresolved_{:08x}.dds", t.hash);
        let file = match t.missing_file.as_ref().or(guess.as_ref()) {
            Some(f) => f,
            None if job.kind == ModelKind::Unit => {
                return Err(format!(
                    "material {} sampler {}: nothing explains the texture path #{:08x}",
                    t.material, t.sampler, t.hash
                ));
            }
            None => {
                pre_warnings.push(format!(
                    "material {} sampler {}: nothing explains the texture path #{:08x}; neutral stand-in {unexplained}",
                    t.material, t.sampler, t.hash
                ));
                &unexplained
            }
        };
        let lower = file.to_ascii_lowercase();
        let desc = descs
            .get_mut(t.material)
            .and_then(Option::as_mut)
            .ok_or_else(|| format!("material {} has no FGX block", t.material))?;
        for set in &mut desc.sets {
            for tex in &mut set.textures {
                if tex.name == t.sampler && tex.src == TexSource::Unresolved(t.hash) {
                    tex.src = TexSource::File(tx_id(&lower));
                }
            }
        }
        if !substitutes.iter().any(|s| s.file == lower) {
            substitutes.push(Substitute {
                specular: TextureSlot::classify(&lower) == TextureSlot::SpecRef,
                file: lower,
            });
        }
    }

    let mut notes: Vec<String> = Vec::new();
    damage_elimination(
        &mut descs,
        &mm.inferred,
        &l.resolved.model.textures,
        &mut notes,
    );
    if let Some(d) = &job.diffuse {
        bind_diffuse(&mut descs, &tx_id(&d.to_ascii_lowercase()));
    }

    let scene = {
        let f = GrannyFile::parse(&l.mesh).map_err(|e| format!("{}: {e}", l.mesh_name))?;
        Scene::read(&f).map_err(|e| format!("{}: {e}", l.mesh_name))?
    };
    let multi = job.kind == ModelKind::Feature;
    let animated = animated_bones_of(game, l, &scene, multi)?;
    let stem = l.flat.strip_suffix(".fxsxml").unwrap_or(&l.flat);
    let bake = |scope: MergeScope| {
        bake_model(&BakeInput {
            scene: &scene,
            source: stem,
            descs: &descs,
            animated_bones: &animated,
            scope,
            basis: Basis::CIV5_TO_ENGINE,
            multi,
        })
    };
    // A skinned mesh in a part with a placement cannot merge over the whole model; such a model
    // keeps its parts (the placement stays a part's).
    let baked = match bake(job.scope) {
        Err(e)
            if job.scope == MergeScope::Model
                && e.to_string().contains("cannot merge into the whole model") =>
        {
            bake(MergeScope::Part).map_err(|e| e.to_string())?
        }
        r => r.map_err(|e| e.to_string())?,
    };

    // The clips: the idle family and what the fxsxml gives beyond it (anim::idle).
    let mut model = baked.model;
    let mut warnings = baked.warnings;
    warnings.extend(pre_warnings);
    let b = model.bounds;
    let size = ((b[3] - b[0]).powi(2) + (b[4] - b[1]).powi(2) + (b[5] - b[2]).powi(2)).sqrt();
    let kind = job.kind;
    let rest = model.skeleton.rest_pose();
    let joined: Vec<JoinedSkeleton<'_>> = if multi {
        joined_skeletons(&scene)?
            .into_iter()
            .map(|(skeleton, offset)| JoinedSkeleton { skeleton, offset })
            .collect()
    } else {
        Vec::new()
    };
    let skeleton = if multi {
        joined
            .first()
            .map(|j| j.skeleton)
            .ok_or_else(|| "no skeleton".to_owned())?
    } else {
        let (_, skel_index) = primary_skeleton(&scene).map_err(|e| e.to_string())?;
        &scene.skeletons[skel_index]
    };
    // The decision tree of a landmark first: the codes it plays are packed with the idle family.
    let mut tree: Option<(String, usize)> = None;
    if let Some(file) = l.resolved.model.decision_trees.first() {
        let bytes = game
            .read_by_name(file)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("{file} is not in the install"))?;
        let text = String::from_utf8_lossy(&bytes);
        model.state_clips = parse_ddtxml(&text).map_err(|e| format!("{file}: {e}"))?;
        let items = ddtxml_animation_items(&text).map_err(|e| format!("{file}: {e}"))?;
        tree = Some((file.clone(), items));
    }
    let tree_codes: Vec<u32> = model
        .state_clips
        .iter()
        .filter_map(|r| match r.clip {
            asset_format::ClipKey::Ec(ec) => Some(ec),
            asset_format::ClipKey::Name(_) => None,
        })
        .collect();
    let pack = build_pack_joined(
        game,
        l.resolved.animations(),
        &PackSetup {
            kind,
            skeleton,
            rest: &rest,
            size,
            basis: &Basis::CIV5_TO_ENGINE,
            tolerance: clip_tolerance,
            tree_codes: &tree_codes,
        },
        &joined,
    )?;
    if let Some(p) = &pack {
        model.clips = p.clips.clone();
        model.anim_pack = Some(an_id(stem));
    }

    // The idle state machine (its file is parsed once, `fsms`) and the decision tree of a landmark.
    let machines = &l.resolved.model.state_machines;
    if let Some(first) = machines.first() {
        match fsms.get(&first.to_ascii_lowercase()) {
            Some(Ok(_)) => model.fsm = Some(fsm_id(&fsm_stem(first))),
            Some(Err(e)) => return Err(format!("{first}: {e}")),
            None => return Err(format!("{first}: not read")),
        }
        if machines.len() > 1 {
            warnings.push(format!(
                "{} state machines in the fxsxml; only {first} is used",
                machines.len()
            ));
        }
    }

    let used: BTreeSet<String> = texture_files_of(&model.materials);
    let unused_textures = l
        .resolved
        .model
        .textures
        .iter()
        .filter(|t: &&FxsTexture| {
            matches!(t.slot, TextureSlot::Diffuse | TextureSlot::SpecRef)
                && !used.contains(&t.file.to_ascii_lowercase())
        })
        .map(|t| t.file.clone())
        .collect();
    Ok(Built {
        model,
        pack,
        stats: baked.stats,
        warnings,
        unresolved: mm.unresolved,
        weak: mm
            .inferred
            .iter()
            .filter(|i| i.weak)
            .map(|i| {
                format!(
                    "material {} sampler {} -> {}",
                    i.material, i.sampler, i.file
                )
            })
            .collect(),
        substitutes,
        notes,
        unused_textures,
        tree,
        mesh_anims: scene.animations.len(),
    })
}

// ---------------------------------------------------------------- the rows and counts

/// One row of the per-model csv of a set (`units.csv`, `landmarks.csv`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvRow {
    /// `unit` or `member` (units), `model` (other sets).
    pub kind: &'static str,
    pub id: String,
    pub art_info: String,
    pub model: String,
    pub bones: usize,
    pub meshes: usize,
    pub triangles: usize,
    pub materials: usize,
    /// Meshes per damage label 0..=3.
    pub damage: [usize; 4],
    pub textures: usize,
    /// Clips of the pack of the model, and the bytes of the pack.
    pub clips: usize,
    pub anim_bytes: u64,
    pub status: String,
}

/// What the clip packs hold (R2.2a).
#[derive(Debug, Default)]
pub struct AnimSummary {
    /// Models with a pack.
    pub packs: usize,
    pub clips: usize,
    /// Clip entries (`ClipRef`) by family of the event code.
    pub by_family: BTreeMap<&'static str, usize>,
    /// Keys by the first family of each clip.
    pub keys_by_family: BTreeMap<&'static str, usize>,
    pub tracks: usize,
    pub keys: usize,
    /// Keys by channel: rotation, translation, scale.
    pub channel_keys: [usize; 3],
    /// Keys the clips would have at 30 Hz, for scale.
    pub frames: usize,
    pub bytes: u64,
    /// Models without a clip of event code 1000, with the evidence ([`no_idle_evidence`]).
    pub no_idle: Vec<(String, String)>,
    /// Distinct gr2 files over all packs.
    pub distinct_files: BTreeSet<String>,
    pub max_dev: crate::anim::resample::Deviation,
    pub fsm: FsmSummary,
}

/// What the state machines hold (R2.2b).
#[derive(Debug, Default)]
pub struct FsmSummary {
    /// Files compiled and stored.
    pub machines: usize,
    /// Files that did not compile, with the reason.
    pub failed: BTreeMap<String, String>,
    pub states: usize,
    pub transitions: usize,
    pub conditions: usize,
    /// States by action family: idle 1000, fortify 1500, combat 1280 and 1600-1640, work
    /// 1910-1960.
    pub action_states: BTreeMap<&'static str, usize>,
    pub bytes: u64,
    /// Models with `state_clips` and the rows over all of them.
    pub models_with_state_clips: usize,
    pub state_clip_rows: usize,
}

/// What [`build_models`] made.
pub struct BuiltModels {
    /// The models that were built, with the files they were built from.
    pub good: Vec<(Loaded, Built)>,
    /// Flat name to the reason it could not be built.
    pub failed: BTreeMap<String, String>,
    /// One row per model (built or failed), by flat name; `id` and `art_info` are the set's.
    pub rows: BTreeMap<String, CsvRow>,
    pub entries: Vec<ReportEntry>,
    /// Texture files (lower case) the materials name.
    pub texture_files: BTreeSet<String>,
    /// Ids of the neutral stand-in textures (SIMP-061).
    pub stub_ids: Vec<String>,
    pub texture_bytes: BTreeMap<VariantFormat, u64>,
    pub model_bytes: u64,
    pub meshes: u64,
    pub meshes_before_merge: u64,
    pub triangles: u64,
    pub materials: u64,
    pub damage_models: usize,
    pub anims: AnimSummary,
}

impl BuiltModels {
    /// Stored bytes of everything the builder wrote.
    pub fn total_bytes(&self) -> u64 {
        self.model_bytes
            + self.anims.bytes
            + self.anims.fsm.bytes
            + self.texture_bytes.values().sum::<u64>()
    }
}

/// Builds `jobs` into `store` and `manifest`: models (`md:`), their textures (`tx:`), clip packs
/// (`an:`) and the state machines (`fsm:`) the models use. `group` is the prefetch flag of every
/// blob (0: by the group table, AB-D10). Failed models are listed in the result and the report
/// entries; the caller turns them into an error after writing its own reports.
pub fn build_models(
    game: &GameSource,
    jobs: &[ModelJob],
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    opts: &Options,
    group: PrefetchGroup,
) -> Result<BuiltModels, ModelError> {
    let dict = match &opts.shader_catalog {
        Some(p) => load_shader_catalog(p)?,
        None => NameDict::builtin(),
    };
    let mut entries: Vec<ReportEntry> = Vec::new();
    let t0 = std::time::Instant::now();
    let stage = |what: &str| tracing::info!("models: {what} ({:.1}s)", t0.elapsed().as_secs_f64());

    let job_of: BTreeMap<&str, &ModelJob> = jobs.iter().map(|j| (j.flat.as_str(), j)).collect();
    // 1. Read the files of every model.
    let loaded: Vec<Result<Loaded, String>> = jobs
        .par_iter()
        .map(|j| load_model(game, &j.flat, j.xml.as_deref()))
        .collect();
    let mut failed: BTreeMap<String, String> = BTreeMap::new();
    let mut models: Vec<Loaded> = Vec::new();
    for (j, r) in jobs.iter().zip(loaded) {
        match r {
            Ok(l) => models.push(l),
            Err(e) => {
                failed.insert(j.flat.clone(), e);
            }
        }
    }

    stage("files read");
    // 2. Directories of the texture paths: spelled out in the named gr2 files of the install, then
    //    confirmed by pairs inside one model (material_desc docs).
    let seeds: Vec<PathResolver> = game
        .iter_ext("gr2")
        .collect::<Vec<_>>()
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
    for m in &models {
        let candidates: Vec<String> = m
            .resolved
            .model
            .textures
            .iter()
            .map(|t| t.file.clone())
            .collect();
        let hints: Vec<String> = [m.flat.as_str(), m.mesh_name.as_str()]
            .iter()
            .map(|n| n.rsplit_once('.').map_or(*n, |(s, _)| s).to_owned())
            .collect();
        resolver.learn(&path_hashes(&m.mats.fgx), &candidates, &hints);
    }
    let extra: Vec<String> = game.iter_ext("dds").map(|e| e.name.clone()).collect();

    stage("texture paths learned");
    // 3. The idle state machines (a file is shared by many models), then the models.
    let fsm_files: BTreeSet<String> = models
        .iter()
        .flat_map(|m| m.resolved.model.state_machines.iter())
        .map(|n| n.to_ascii_lowercase())
        .collect();
    let fsms: BTreeMap<String, Result<IdleFsm, String>> = fsm_files
        .par_iter()
        .map(|n| (n.clone(), read_fsm(game, n)))
        .collect();
    let built: Vec<Result<Built, String>> = models
        .par_iter()
        .map(|l| {
            tracing::debug!("model start {}", l.flat);
            let t = std::time::Instant::now();
            let r = build_model(
                game,
                job_of[l.flat.as_str()],
                l,
                &resolver,
                &extra,
                &dict,
                &opts.clip_tolerance,
                &fsms,
            );
            if t.elapsed().as_secs_f64() > 5.0 {
                tracing::info!("model {} took {:.1}s", l.flat, t.elapsed().as_secs_f64());
            }
            r
        })
        .collect();
    let mut ok: Vec<(Loaded, Built)> = Vec::new();
    for (l, r) in models.into_iter().zip(built) {
        match r {
            Ok(b) => ok.push((l, b)),
            Err(e) => {
                failed.insert(l.flat.clone(), e);
            }
        }
    }

    stage("models built");
    // 4. Textures: every file a material names, as the original's blocks.
    let mut files: BTreeSet<String> = BTreeSet::new();
    let mut substitutes: BTreeMap<String, bool> = BTreeMap::new();
    for (_, b) in &ok {
        files.extend(texture_files_of(&b.model.materials));
        for s in &b.substitutes {
            substitutes.insert(s.file.clone(), s.specular);
        }
    }
    let file_list: Vec<&String> = files.iter().collect();
    let blobs: Vec<Result<TextureBlob, String>> = file_list
        .par_iter()
        .map(|f| {
            if let Some(&specular) = substitutes.get(f.as_str())
                && game.lookup(f).is_none()
            {
                return Ok(neutral_texture(specular));
            }
            let bytes = game
                .read_by_name(f)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("{f} is missing from the install"))?;
            dds_passthrough(&bytes).map_err(|e| format!("{f}: {e}"))
        })
        .collect();
    let mut texture_bytes: BTreeMap<VariantFormat, u64> = BTreeMap::new();
    let mut bad_textures: BTreeMap<String, String> = BTreeMap::new();
    let mut stub_ids: Vec<String> = Vec::new();
    for (f, r) in file_list.iter().zip(blobs) {
        match r {
            Ok(blob) => {
                let (hash, size) = store.put(&blob.encode())?;
                let format = variant_format(blob.format);
                *texture_bytes.entry(format).or_default() += size;
                manifest.add(
                    tx_id(f),
                    Variant {
                        format,
                        hash,
                        size,
                        prefetch: group,
                    },
                );
                if substitutes.contains_key(f.as_str()) && game.lookup(f).is_none() {
                    stub_ids.push(format!("tx:{f}"));
                }
            }
            Err(e) => {
                bad_textures.insert((*f).clone(), e);
            }
        }
    }
    // A model whose texture did not convert fails (nothing is substituted for it).
    let mut good: Vec<(Loaded, Built)> = Vec::new();
    for (l, b) in ok {
        let bad: Vec<String> = texture_files_of(&b.model.materials)
            .into_iter()
            .filter_map(|f| bad_textures.get(&f).map(|e| format!("texture {f}: {e}")))
            .collect();
        if bad.is_empty() {
            good.push((l, b));
        } else {
            failed.insert(l.flat.clone(), bad.join("; "));
        }
    }

    stage("textures converted");
    // 5. Store the models and count.
    let (mut meshes, mut meshes_before, mut triangles, mut materials) = (0u64, 0u64, 0u64, 0u64);
    let mut model_bytes = 0u64;
    let mut damage_models = 0usize;
    let mut anims = AnimSummary::default();
    let mut rows: BTreeMap<String, CsvRow> = BTreeMap::new();
    let mut unresolved_names: BTreeMap<Unresolved, BTreeSet<String>> = BTreeMap::new();
    for (l, b) in &good {
        let model = &b.model;
        let (hash, size) = store.put(&model.encode())?;
        manifest.add(
            md_id(&l.flat),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: group,
            },
        );
        model_bytes += size;
        let mut pack_size = 0u64;
        match &b.pack {
            Some(p) => {
                let (hash, size) = store.put(&p.pack.encode())?;
                pack_size = size;
                manifest.add(
                    an_id(l.flat.trim_end_matches(".fxsxml")),
                    Variant {
                        format: VariantFormat::Generic,
                        hash,
                        size,
                        prefetch: group,
                    },
                );
                anims.packs += 1;
                anims.bytes += size;
                anims.clips += p.stats.clips;
                anims.tracks += p.stats.tracks;
                anims.keys += p.stats.keys;
                for (a, b) in anims.channel_keys.iter_mut().zip(p.stats.channel_keys) {
                    *a += b;
                }
                anims.frames += p.stats.frames;
                anims.max_dev.max_with(&p.stats.max_dev);
                anims.distinct_files.extend(p.files.iter().cloned());
                for (f, k) in &p.stats.family_keys {
                    *anims.keys_by_family.entry(f.label()).or_default() += k;
                }
                for c in &p.clips {
                    let f = if c.ec == 0 {
                        "idle-by-name"
                    } else {
                        crate::anim::idle::family(job_of[l.flat.as_str()].kind, c.ec)
                            .unwrap_or(crate::anim::idle::Family::Idle)
                            .label()
                    };
                    *anims.by_family.entry(f).or_default() += 1;
                }
                for w in &p.warnings {
                    entries.push(ReportEntry::new("anim-warning", l.flat.clone(), w.clone()));
                }
                if !p.stats.has_idle_1000 {
                    anims
                        .no_idle
                        .push((l.flat.clone(), no_idle_evidence(game, l, b)));
                }
            }
            None => anims
                .no_idle
                .push((l.flat.clone(), no_idle_evidence(game, l, b))),
        }
        meshes += model.meshes.len() as u64;
        meshes_before += b.stats.meshes_before as u64;
        triangles += model.triangle_count() as u64;
        materials += model.materials.len() as u64;
        damage_models += usize::from(b.stats.damage_meshes[1..].iter().any(|&n| n > 0));
        rows.insert(
            l.flat.clone(),
            CsvRow {
                kind: "member",
                id: String::new(),
                art_info: String::new(),
                model: l.flat.clone(),
                bones: model.skeleton.bones.len(),
                meshes: model.meshes.len(),
                triangles: model.triangle_count(),
                materials: model.materials.len(),
                damage: b.stats.damage_meshes,
                textures: texture_files_of(&model.materials).len(),
                clips: b.pack.as_ref().map_or(0, |p| p.stats.clips),
                anim_bytes: pack_size,
                status: "ok".to_owned(),
            },
        );
        for u in &b.unresolved {
            unresolved_names
                .entry(*u)
                .or_default()
                .insert(l.flat.clone());
        }
        for w in l.warnings.iter().chain(&b.warnings) {
            entries.push(ReportEntry::new("model-warning", l.flat.clone(), w.clone()));
        }
        for w in &b.weak {
            entries.push(ReportEntry::new("texture-weak", l.flat.clone(), w.clone()));
        }
        for n in &b.notes {
            entries.push(ReportEntry::new(
                "texture-elimination",
                l.flat.clone(),
                n.clone(),
            ));
        }
        for t in &b.unused_textures {
            let note = if game.lookup(t).is_some() {
                "named by the fxsxml, used by no material (the file is in the install)"
            } else {
                "named by the fxsxml, used by no material, and not in the install"
            };
            entries.push(ReportEntry::new(
                "texture-unused",
                l.flat.clone(),
                format!("{t}: {note}"),
            ));
        }
        if b.stats.placed_parts > 0 {
            entries.push(ReportEntry::new(
                "placement",
                l.flat.clone(),
                "the gr2 model has an InitialPlacement that is not the identity (kept in Part.placement)",
            ));
        }
    }
    // The state machines the built models use, and the decision trees.
    let used_fsms: BTreeSet<&str> = good
        .iter()
        .filter_map(|(_, b)| b.model.fsm.as_ref())
        .map(|id| id.as_str())
        .collect();
    for (file, r) in &fsms {
        match r {
            Ok(m) if used_fsms.contains(fsm_id(&fsm_stem(file)).as_str()) => {
                let (hash, size) = store.put(&m.encode())?;
                manifest.add(
                    fsm_id(&fsm_stem(file)),
                    Variant {
                        format: VariantFormat::Generic,
                        hash,
                        size,
                        prefetch: group,
                    },
                );
                let f = &mut anims.fsm;
                f.machines += 1;
                f.bytes += size;
                f.states += m.states.len();
                for s in &m.states {
                    f.transitions += s.transitions.len();
                    f.conditions += s
                        .transitions
                        .iter()
                        .map(|t| t.conditions.len())
                        .sum::<usize>();
                    let family = match s.action {
                        1000 => Some("idle 1000"),
                        1500 => Some("fortify 1500"),
                        a if crate::anim::fsm::is_combat_action(a) => Some("combat 1280/1600-1640"),
                        1910..=1960 => Some("work 1910-1960"),
                        _ => None,
                    };
                    if let Some(family) = family {
                        *f.action_states.entry(family).or_default() += 1;
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                anims.fsm.failed.insert(file.clone(), e.clone());
            }
        }
    }
    for (l, b) in &good {
        if !b.model.state_clips.is_empty() {
            anims.fsm.models_with_state_clips += 1;
            anims.fsm.state_clip_rows += b.model.state_clips.len();
        }
        if let Some((tree, items)) = &b.tree
            && b.model.state_clips.is_empty()
        {
            let why = if *items == 0 {
                format!(
                    "static in the original: {tree} has an empty animation layer (only trigger timelines)"
                )
            } else {
                format!(
                    "{tree} has {items} item(s) in its animation layer but gives no animation rows"
                )
            };
            entries.push(ReportEntry::new("state-clips-empty", l.flat.clone(), why));
        }
    }
    for (file, why) in &anims.fsm.failed {
        entries.push(ReportEntry::new("fsm-failed", file.clone(), why.clone()));
    }
    for (u, who) in &unresolved_names {
        entries.push(ReportEntry::new(
            "fgx-name",
            u.to_string(),
            format!(
                "not in any dictionary ({} model(s), e.g. {})",
                who.len(),
                who.iter().next().map_or("", String::as_str)
            ),
        ));
    }
    for (m, why) in &anims.no_idle {
        entries.push(ReportEntry::new("anim-no-idle", m.clone(), why.clone()));
    }
    for id in &stub_ids {
        entries.push(ReportEntry::new(
            "stub",
            id.clone(),
            "named by a material, not in the install: neutral 4x4 texture (SIMP-061)",
        ));
    }
    for (flat, reason) in &failed {
        rows.insert(
            flat.clone(),
            CsvRow {
                kind: "member",
                id: String::new(),
                art_info: String::new(),
                model: flat.clone(),
                bones: 0,
                meshes: 0,
                triangles: 0,
                materials: 0,
                damage: [0; 4],
                textures: 0,
                clips: 0,
                anim_bytes: 0,
                status: format!("FAILED: {reason}"),
            },
        );
        entries.push(ReportEntry::new(
            "model-failed",
            flat.clone(),
            reason.clone(),
        ));
    }

    Ok(BuiltModels {
        good,
        failed,
        rows,
        entries,
        texture_files: files,
        stub_ids,
        texture_bytes,
        model_bytes,
        meshes,
        meshes_before_merge: meshes_before,
        triangles,
        materials,
        damage_models,
        anims,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sampler(name: &str, src: TexSource) -> TexParam {
        TexParam {
            name: name.to_owned(),
            kind: TexKind::Tex2D,
            src,
            semantic: -1,
            reg: 0,
            usage: 0,
            flags: 0,
            active: true,
        }
    }

    fn desc(textures: Vec<TexParam>) -> Option<MaterialDesc> {
        Some(MaterialDesc {
            sets: vec![ParamSet {
                textures,
                ..ParamSet::default()
            }],
            ..MaterialDesc::default()
        })
    }

    #[test]
    fn the_job_diffuse_reaches_every_material_the_client_reads() {
        let id = tx_id("forest_generic.dds");
        let other = tx_id("forest_europe.dds");
        let mut descs = vec![
            // A named sampler takes it.
            desc(vec![
                sampler("BuildingSREF", TexSource::Engine),
                sampler("Diffuse", TexSource::File(other.clone())),
            ]),
            // No named one: the first file texture.
            desc(vec![
                sampler("#11111111", TexSource::File(other.clone())),
                sampler("#22222222", TexSource::File(other.clone())),
            ]),
            // `forest_generic`: only engine samplers with unreadable names.
            desc(vec![
                sampler("#0f13d4e5", TexSource::Engine),
                sampler("EnvironmentMap", TexSource::Engine),
            ]),
            None,
        ];
        bind_diffuse(&mut descs, &id);
        for d in descs.iter().flatten() {
            let v1 = ModelBlobV2 {
                materials: vec![d.clone()],
                ..ModelBlobV2::default()
            }
            .to_static_v1()
            .unwrap();
            assert_eq!(v1.materials[0].diffuse.as_ref(), Some(&id), "{d:?}");
        }
        let second: Vec<_> = descs[1].as_ref().unwrap().texture_files().collect();
        assert_eq!(second, [&id, &other], "only the first file texture");
        assert!(descs[3].is_none());
    }

    #[test]
    fn neutral_textures_are_valid_blobs() {
        for spec in [false, true] {
            let t = neutral_texture(spec);
            assert_eq!((t.width, t.height, t.mips.len()), (4, 4, 1));
            assert_eq!(t.mips[0].len(), 64);
            assert!(!t.srgb);
            assert_eq!(TextureBlob::decode(&t.encode()).unwrap(), t);
        }
        assert_eq!(&neutral_texture(true).mips[0][..4], [0, 0, 0, 255]);
        assert_eq!(&neutral_texture(false).mips[0][..4], [128, 128, 128, 255]);
    }

    #[test]
    fn a_model_id_is_the_flat_stem() {
        assert_eq!(
            md_id("Assets/Units/Warrior/Warrior.fxsxml").as_str(),
            "md:warrior"
        );
    }
}
