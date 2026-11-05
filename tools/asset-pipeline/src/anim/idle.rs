//! The clips of one model for its `an:<model>` pack: which clips of the fxsxml are packed, and
//! the reference frames of each (the Granny curves sampled on the 30 Hz grid, in engine
//! convention) that [`super::resample`] reduces to nodes.
//!
//! Which clips:
//! - the idle family, event codes 1000, 1020, 1040, 2000, 2020, 2040; a model without any event
//!   code takes the clips with "idle" in their name (code 0);
//! - fortify and sleep 1500-1580, worker 1910-1960, combat readiness 1600-1640 and 1280 (packed
//!   for later, not played in B2);
//! - features (`FeatureModels`) 100 and 101; effect models (`fxm_*`) 1000 only.

use std::collections::BTreeMap;

use asset_format::{AnimEvent, AnimPack, ClipRef, Transform};
use granny::animation::TransformTrack;
use granny::animation::{Animation, normalize};
use granny::bind::bind;
use granny::skeleton::Skeleton as GrannySkeleton;
use granny::{GrannyFile, Scene};

use super::resample::{Deviation, RefClip, RefTrack, Tolerance, pack_clip};
use crate::art::FxsAnim;
use crate::game::GameSource;
use crate::models::basis::Basis;

/// Event codes of the idle family.
pub const IDLE_CODES: [u32; 6] = [1000, 1020, 1040, 2000, 2020, 2040];

/// What a model is, which decides the clips beyond the idle family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Unit,
    /// Everything that is not a unit or an effect (`FeatureModels`, landmarks, cities, wonders,
    /// roads, path overlays): 100 and 101 as well.
    Feature,
    /// `EffectData` `fxm_*`: only 1000.
    Effect,
}

/// The family an event code belongs to, for the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Idle,
    Fortify,
    Combat,
    Work,
    Feature,
    /// No event code, "idle" in the name.
    ByName,
}

impl Family {
    pub fn label(self) -> &'static str {
        match self {
            Family::Idle => "idle",
            Family::Fortify => "fortify",
            Family::Combat => "combat",
            Family::Work => "work",
            Family::Feature => "feature",
            Family::ByName => "idle-by-name",
        }
    }
}

/// The family of `ec` for a model of `kind`, `None` when the clip is not packed.
pub fn family(kind: ModelKind, ec: u32) -> Option<Family> {
    match kind {
        ModelKind::Effect => (ec == 1000).then_some(Family::Idle),
        ModelKind::Feature | ModelKind::Unit => {
            if IDLE_CODES.contains(&ec) {
                Some(Family::Idle)
            } else if (1500..=1580).contains(&ec) {
                Some(Family::Fortify)
            } else if (1600..=1640).contains(&ec) || ec == 1280 {
                Some(Family::Combat)
            } else if (1910..=1960).contains(&ec) {
                Some(Family::Work)
            } else if kind == ModelKind::Feature && (ec == 100 || ec == 101) {
                Some(Family::Feature)
            } else {
                None
            }
        }
    }
}

/// One gr2 clip that is packed, with the codes it answers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    /// Index in the fxsxml animation list (the first entry of the file).
    pub anim: usize,
    /// Packed codes, sorted; `[0]` for a clip taken by its name.
    pub codes: Vec<u32>,
}

/// The clips to pack, in the order of the fxsxml; entries naming one gr2 file are one clip.
/// `tree_codes` are the event codes the model's decision tree plays (`.ddtxml`): a herd's tree
/// picks one of 1000..1003 per landmark (`Cow`, `Bison`, `ANC_Polder`), so those are packed too.
pub fn select_clips(anims: &[FxsAnim], kind: ModelKind, tree_codes: &[u32]) -> Vec<Selected> {
    let any_code = anims.iter().any(|a| !a.event_codes.is_empty());
    let mut by_file: BTreeMap<String, Selected> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, a) in anims.iter().enumerate() {
        let codes: Vec<u32> = if any_code {
            a.event_codes
                .iter()
                .copied()
                .filter(|&c| family(kind, c).is_some() || tree_codes.contains(&c))
                .collect()
        } else if kind != ModelKind::Effect && a.label().to_ascii_lowercase().contains("idle") {
            vec![0]
        } else {
            Vec::new()
        };
        if codes.is_empty() {
            continue;
        }
        let key = a.file.to_ascii_lowercase();
        let e = by_file.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            Selected {
                anim: i,
                codes: Vec::new(),
            }
        });
        e.codes.extend(codes);
    }
    order
        .into_iter()
        .filter_map(|k| by_file.remove(&k))
        .map(|mut s| {
            s.codes.sort_unstable();
            s.codes.dedup();
            s
        })
        .collect()
}

// ---------------------------------------------------------------- reference frames

/// A skeleton of a catalog model inside the joined skeleton of the blob.
#[derive(Debug, Clone, Copy)]
pub struct JoinedSkeleton<'a> {
    pub skeleton: &'a GrannySkeleton,
    /// The number of its first bone in the blob's skeleton.
    pub offset: u16,
}

/// The tracks of `anim` with the bone each drives, in the order of the file. One skeleton: the
/// tracks of the track group that binds most of its tracks to it, by name (a bone with two
/// tracks takes the last). Several (a catalog): every track group with tracks goes to the skeleton that has its
/// name (the groups of a catalog clip are named after the models), a group of no skeleton is
/// left out.
pub fn clip_tracks<'a>(
    anim: &'a Animation,
    joined: &[JoinedSkeleton<'_>],
) -> Result<Vec<(usize, &'a TransformTrack)>, String> {
    let mut out: Vec<(usize, &TransformTrack)> = Vec::new();
    if let [one] = joined {
        // A clip of a gr2 with two skeletons (`EuroFemale18`: a one-bone `WORLD_center`
        // skeleton first, the 23-bone body second) has one track group per skeleton, in the same
        // order; the group of the skinned skeleton is the one that binds most tracks to it, not
        // the first. The first group with tracks only when none binds (its bones are left out).
        let mut best: Option<(usize, &granny::animation::TrackGroup, granny::bind::Binding)> = None;
        for g in anim
            .track_groups
            .iter()
            .filter(|g| !g.transform_tracks.is_empty())
        {
            let b = bind(one.skeleton, g);
            let n = b.track_to_bone.iter().flatten().count();
            if best.as_ref().is_none_or(|(m, _, _)| n > *m) {
                best = Some((n, g, b));
            }
        }
        let (_, group, binding) =
            best.ok_or_else(|| "the animation has no transform tracks".to_owned())?;
        for (t, b) in binding.track_to_bone.iter().enumerate() {
            if let Some(b) = b {
                out.push((*b + usize::from(one.offset), &group.transform_tracks[t]));
            }
        }
        return Ok(out);
    }
    for group in anim
        .track_groups
        .iter()
        .filter(|g| !g.transform_tracks.is_empty())
    {
        let by_name = group.name.as_deref().and_then(|name| {
            joined.iter().find(|j| {
                j.skeleton
                    .name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
        });
        // A group whose name matches no skeleton goes to the skeleton that binds most of its
        // tracks by bone name (when it binds any).
        let Some(j) = by_name.or_else(|| {
            joined
                .iter()
                .map(|j| {
                    let n = bind(j.skeleton, group)
                        .track_to_bone
                        .iter()
                        .flatten()
                        .count();
                    (n, j)
                })
                .filter(|(n, _)| *n > 0)
                .max_by_key(|(n, _)| *n)
                .map(|(_, j)| j)
        }) else {
            continue;
        };
        let binding = bind(j.skeleton, group);
        for (t, b) in binding.track_to_bone.iter().enumerate() {
            if let Some(b) = b {
                out.push((*b + usize::from(j.offset), &group.transform_tracks[t]));
            }
        }
    }
    if out.is_empty() {
        return Err("no track group of the animation matches a skeleton of the model".to_owned());
    }
    Ok(out)
}

/// The reference frames of `anim`: every track bound to a bone of `skel`, sampled by the Granny
/// curves at the 30 Hz grid and put in engine convention. A bone with two tracks takes the last.
pub fn reference_clip(
    name: &str,
    anim: &Animation,
    skel: &GrannySkeleton,
    basis: &Basis,
) -> Result<RefClip, String> {
    reference_clip_joined(
        name,
        anim,
        &[JoinedSkeleton {
            skeleton: skel,
            offset: 0,
        }],
        basis,
    )
}

/// [`reference_clip`] for a model whose skeletons are joined (see [`clip_tracks`]); bones are
/// numbers in the joined skeleton.
pub fn reference_clip_joined(
    name: &str,
    anim: &Animation,
    joined: &[JoinedSkeleton<'_>],
    basis: &Basis,
) -> Result<RefClip, String> {
    let mut by_bone: BTreeMap<usize, &TransformTrack> = BTreeMap::new();
    for (bone, track) in clip_tracks(anim, joined)? {
        by_bone.insert(bone, track);
    }
    let mut clip = RefClip {
        name: name.to_owned(),
        duration: if anim.duration.is_finite() && anim.duration > 0.0 {
            anim.duration
        } else {
            0.0
        },
        tracks: Vec::new(),
    };
    let frames = clip.frames();
    for (&bone, &track) in &by_bone {
        let bone = u16::try_from(bone).map_err(|_| format!("bone {bone} does not fit u16"))?;
        let mut r = RefTrack {
            bone,
            rot: Vec::with_capacity(frames),
            pos: Vec::with_capacity(frames),
            scale: Vec::with_capacity(frames),
        };
        for i in 0..frames {
            let p = track.sample(clip.frame_time(i));
            if !p.translation.iter().all(|c| c.is_finite())
                || !p.rotation.iter().all(|c| c.is_finite())
                || !p.scale_shear.iter().all(|c| c.is_finite())
            {
                return Err(format!(
                    "bone {bone}: the curves give a value that is not finite"
                ));
            }
            r.rot.push(basis.rotation(normalize(p.rotation)));
            r.pos.push(basis.point(p.translation));
            r.scale
                .push(basis.scale([p.scale_shear[0], p.scale_shear[4], p.scale_shear[8]]));
        }
        clip.tracks.push(r);
    }
    Ok(clip)
}

// ---------------------------------------------------------------- the pack of a model

/// What the pack of one model holds, for the report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackStats {
    pub clips: usize,
    pub tracks: usize,
    pub keys: usize,
    /// Keys by channel: rotation, translation, scale.
    pub channel_keys: [usize; 3],
    /// Reference frames over all clips (what 30 Hz keys would be per channel).
    pub frames: usize,
    pub max_dev: Deviation,
    /// Clips by family of their codes.
    pub families: BTreeMap<Family, usize>,
    /// Keys by the first family of each clip.
    pub family_keys: BTreeMap<Family, usize>,
    pub has_idle_1000: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ModelPack {
    pub pack: AnimPack,
    pub clips: Vec<ClipRef>,
    /// The gr2 file of each clip of the pack, lower case.
    pub files: Vec<String>,
    pub stats: PackStats,
    pub warnings: Vec<String>,
}

/// What the clips of a model are packed against.
pub struct PackSetup<'a> {
    pub kind: ModelKind,
    /// The skeleton of the mesh gr2 (the clips bind to its bones).
    pub skeleton: &'a GrannySkeleton,
    /// Engine-convention rest pose of `skeleton`.
    pub rest: &'a [Transform],
    /// Model size: the diagonal of its box (what the translation tolerance is a percent of).
    pub size: f32,
    pub basis: &'a Basis,
    pub tolerance: &'a Tolerance,
    /// Event codes the decision tree of the model plays ([`select_clips`]); empty without one.
    pub tree_codes: &'a [u32],
}

/// Packs the selected clips of a model. A clip the install does not ship is skipped (the fxsxml
/// loader already warned); one that does not parse is an error.
pub fn build_pack(
    game: &GameSource,
    anims: &[FxsAnim],
    setup: &PackSetup<'_>,
) -> Result<Option<ModelPack>, String> {
    build_pack_joined(game, anims, setup, &[])
}

/// [`build_pack`] for a catalog model: `joined` lists the skeletons of its models with their
/// offsets in the blob's skeleton (empty: the one `setup.skeleton`).
pub fn build_pack_joined(
    game: &GameSource,
    anims: &[FxsAnim],
    setup: &PackSetup<'_>,
    joined: &[JoinedSkeleton<'_>],
) -> Result<Option<ModelPack>, String> {
    let PackSetup {
        kind,
        skeleton: skel,
        rest,
        size,
        basis,
        tolerance: tol,
        tree_codes,
    } = *setup;
    let selected = select_clips(anims, kind, tree_codes);
    let mut out = ModelPack::default();
    // Clips none of whose tracks finds a bone of the model (they pack as the rest pose).
    let mut unbound: Vec<String> = Vec::new();
    for s in &selected {
        let a = &anims[s.anim];
        let Some(bytes) = game.read_by_name(&a.file).map_err(|e| e.to_string())? else {
            continue;
        };
        let file = GrannyFile::parse(&bytes).map_err(|e| format!("{}: {e}", a.file))?;
        let scene = Scene::read(&file).map_err(|e| format!("{}: {e}", a.file))?;
        let Some(anim) = scene.animations.iter().find(|n| {
            n.track_groups
                .iter()
                .any(|g| !g.transform_tracks.is_empty())
        }) else {
            return Err(format!("{}: no animation with tracks", a.file));
        };
        let reference = if joined.is_empty() {
            reference_clip(a.label(), anim, skel, basis)
        } else {
            reference_clip_joined(a.label(), anim, joined, basis)
        }
        .map_err(|e| format!("{}: {e}", a.file))?;
        if reference.tracks.is_empty() {
            unbound.push(a.label().to_owned());
        }
        let (clip, dev) = pack_clip(&reference, rest, size, tol);
        if !dev.within(tol) {
            out.warnings.push(format!(
                "clip {}: {:.3} deg / {:.3}% / {:.3}% off the reference, above the tolerance",
                a.label(),
                dev.rot_deg,
                dev.pos_percent,
                dev.scale_percent
            ));
        }
        let index = u16::try_from(out.pack.clips.len())
            .map_err(|_| "more than 65535 clips in one model".to_owned())?;
        out.stats.tracks += clip.tracks.len();
        out.stats.keys += clip.key_count();
        for t in &clip.tracks {
            out.stats.channel_keys[0] += t.rot.key_count();
            out.stats.channel_keys[1] += t.pos.key_count();
            out.stats.channel_keys[2] += t.scale.key_count();
        }
        out.stats.frames += reference.frames() * reference.tracks.len() * 3;
        out.stats.max_dev.max_with(&dev);
        let mut families: Vec<Family> = Vec::new();
        for &ec in &s.codes {
            let f = if ec == 0 {
                Family::ByName
            } else {
                family(kind, ec).unwrap_or(Family::Idle)
            };
            if !families.contains(&f) {
                families.push(f);
            }
            out.stats.has_idle_1000 |= ec == 1000;
            out.clips.push(ClipRef {
                name: a.label().to_owned(),
                ec,
                index,
                events: Vec::<AnimEvent>::new(),
                legacy_blob: None,
            });
        }
        if let Some(f) = families.first() {
            *out.stats.family_keys.entry(*f).or_default() += clip.key_count();
        }
        for f in families {
            *out.stats.families.entry(f).or_default() += 1;
        }
        out.files.push(a.file.to_ascii_lowercase());
        out.pack.clips.push(clip);
        out.stats.clips += 1;
    }
    if !unbound.is_empty() {
        out.warnings.push(format!(
            "{} clip(s) bind no track to a bone of the model (they are the rest pose): {}",
            unbound.len(),
            unbound.join(", ")
        ));
    }
    Ok((!out.pack.clips.is_empty()).then_some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anim(file: &str, ecs: &[u32]) -> FxsAnim {
        FxsAnim {
            file: file.into(),
            event_codes: ecs.to_vec(),
            name: None,
            layer: None,
        }
    }

    #[test]
    fn the_families_of_event_codes() {
        use ModelKind::*;
        assert_eq!(family(Unit, 1000), Some(Family::Idle));
        assert_eq!(family(Unit, 2040), Some(Family::Idle));
        assert_eq!(family(Unit, 1500), Some(Family::Fortify));
        assert_eq!(family(Unit, 1580), Some(Family::Fortify));
        assert_eq!(family(Unit, 1600), Some(Family::Combat));
        assert_eq!(family(Unit, 1280), Some(Family::Combat));
        assert_eq!(family(Unit, 1960), Some(Family::Work));
        assert_eq!(family(Unit, 100), None);
        assert_eq!(family(Feature, 101), Some(Family::Feature));
        assert_eq!(family(Effect, 1000), Some(Family::Idle));
        assert_eq!(family(Effect, 1020), None);
        assert_eq!(family(Unit, 1700), None);
    }

    #[test]
    fn selection_keeps_file_order_and_merges_a_file_named_twice() {
        let anims = vec![
            anim("A_Idle.gr2", &[1000]),
            anim("A_Run.gr2", &[1200]),
            anim("A_Fortify.gr2", &[1500]),
            anim("a_idle.GR2", &[1020, 3000]),
            anim("A_Ready.gr2", &[1600, 1280]),
        ];
        let s = select_clips(&anims, ModelKind::Unit, &[]);
        assert_eq!(
            s,
            vec![
                Selected {
                    anim: 0,
                    codes: vec![1000, 1020]
                },
                Selected {
                    anim: 2,
                    codes: vec![1500]
                },
                Selected {
                    anim: 4,
                    codes: vec![1280, 1600]
                },
            ]
        );
    }

    #[test]
    fn a_herd_packs_the_variants_its_decision_tree_plays() {
        let anims = vec![
            anim("Cow_Idle1000.gr2", &[1000]),
            anim("Cow_Idle1001.gr2", &[1001]),
            anim("Cow_Idle1003.gr2", &[1003]),
            anim("Cow_Other.gr2", &[1700]),
        ];
        let codes = |s: Vec<Selected>| s.into_iter().flat_map(|s| s.codes).collect::<Vec<_>>();
        assert_eq!(codes(select_clips(&anims, ModelKind::Feature, &[])), [1000]);
        assert_eq!(
            codes(select_clips(
                &anims,
                ModelKind::Feature,
                &[1000, 1001, 1002, 1003]
            )),
            [1000, 1001, 1003]
        );
    }

    #[test]
    fn a_model_without_codes_takes_idle_by_name() {
        let anims = vec![anim("Wolf_Walk.gr2", &[]), anim("Wolf_IdleA.gr2", &[])];
        let s = select_clips(&anims, ModelKind::Unit, &[]);
        assert_eq!(
            s,
            vec![Selected {
                anim: 1,
                codes: vec![0]
            }]
        );
        // With even one code the names do not count.
        let anims = vec![anim("Wolf_Walk.gr2", &[1200]), anim("Wolf_IdleA.gr2", &[])];
        assert!(select_clips(&anims, ModelKind::Unit, &[]).is_empty());
    }
}
