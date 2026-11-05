//! What `world-anim` reads of a model: the clips (`ec` and index in the
//! `C5A2` pack), the skeleton, the automaton `C5FM`, the decision rows of a landmark. The formats are
//! `asset-format`'s (`C5MD` v2, `C5A2`, `C5FM`); nothing here has a format of its own, only the
//! types `render` builds from them.
//!
//! A model without clips is not an error: it has a `ModelAnim` with no clips and `attach` gives
//! `None` for it. A clip that points outside the pack, or a track that points outside the skeleton,
//! is an error and the whole model is refused (a half-animated model is worse than a still one).

use std::sync::Arc;

use asset_format::{AnimPack, FormatError, IdleFsm, ModelBlobV2, Skeleton, StateClip};

use super::graph::{GraphNote, MachineDef};

// ---- event codes (`ec`) of the .fxsxml and the actions of the automaton (option 4) ----
//
// The files: `Assets/DLC/.../Unit/*.fxsxml` (the `ec` of a clip), `1_omni_state_01.fsmxml` (the
// `action` of a state).

/// IdleA.
pub const EC_IDLE_A: u32 = 1000;
/// IdleA to IdleB.
pub const EC_IDLE_A_TO_B: u32 = 1020;
/// FidgetA.
pub const EC_FIDGET_A: u32 = 1040;
/// IdleB.
pub const EC_IDLE_B: u32 = 2000;
/// IdleB to IdleA.
pub const EC_IDLE_B_TO_A: u32 = 2020;
/// FidgetB.
pub const EC_FIDGET_B: u32 = 2040;
/// Fortify and sleep (the first clip of the family 1500..=1580).
pub const EC_FORTIFY: u32 = 1500;
/// The loop of the fortify family: 1520, 1540, 1560, 1580.
pub const EC_FORTIFY_LOOP: [u32; 4] = [1520, 1540, 1560, 1580];
/// The work of a worker: 1910..=1960.
pub const EC_WORK_FIRST: u32 = 1910;
pub const EC_WORK_LAST: u32 = 1960;
/// The feature clips: idle and worked (`FeatureModels`).
pub const EC_FEATURE_IDLE: u32 = 100;
pub const EC_FEATURE_WORKED: u32 = 101;

/// Codes that are never requested in B2 (BUILD 5, R-D7): attack, death, run, victory, capture. The
/// automaton must never enter a state whose action is one of them.
pub const EC_NEVER: [u32; 6] = [1100, 1200, 1400, 1800, 2100, 1195];
/// Combat readiness (1600..=1640 and 1280): packed for the next build, not played in B2.
pub fn is_combat_ready(ec: u32) -> bool {
    (1600..=1640).contains(&ec) || ec == 1280
}

/// Samples of the codes that are never played, for the tests: the never-requested ones and a
/// few of combat readiness.
#[cfg(test)]
pub const EC_NEVER_SAMPLES: [u32; 10] =
    [1100, 1200, 1400, 1800, 2100, 1195, 1600, 1620, 1640, 1280];

/// Frames a second of the baked clips (WA-D2).
pub const BAKE_FPS: f32 = 30.0;

#[derive(Debug, thiserror::Error)]
pub enum AnimDataError {
    #[error("blob: {0}")]
    Format(#[from] FormatError),
    #[error("model `{model}`: clip `{clip}` is number {index} of a pack with {len} clips")]
    ClipIndex {
        model: String,
        clip: String,
        index: u16,
        len: usize,
    },
    #[error("model `{model}`: the clips use {span} bones, the skeleton has {bones}")]
    BoneSpan {
        model: String,
        span: usize,
        bones: usize,
    },
    #[error("model `{model}`: has clips, and no `C5A2` pack")]
    NoPack { model: String },
}

/// One clip of the model as `render` keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipEntry {
    pub name: String,
    /// Event code; 0 when the fxsxml gives none (the name says "idle").
    pub ec: u32,
    /// Index in the pack.
    pub index: u16,
    /// Seconds.
    pub duration: f32,
}

impl ClipEntry {
    /// Baked frames of the clip: `ceil(duration * 30) + 1` (4.3); at least 2, so that a frame has a
    /// next one.
    pub fn frames(&self) -> u32 {
        frames_of(self.duration)
    }
}

/// The most frames a baked clip gets. The bone texture holds 512 x 2048 slots (the limit of WebGL2);
/// the idle clips of the install run up to 47 s with 100+ bones, so at 30 frames a second 20 of them
/// fill the texture and the next clips never bake (a late-game map with all its unit kinds on screen
/// did not settle). A longer clip is baked at a lower rate; the frames are still blended.
pub const MAX_BAKE_FRAMES: u32 = 160;

/// The bake rates, from the finest. `(frames a second, most frames of a clip)`. The cache bakes at
/// the finest while the texture has room and goes down a tier as it fills (and when a clip finds no
/// room at its tier), so that every clip in use fits: a tier halves, then thirds, the slots.
pub const BAKE_TIERS: [(f32, u32); 3] = [(BAKE_FPS, MAX_BAKE_FRAMES), (15.0, 80), (10.0, 48)];

/// The coarsest tier.
pub const LAST_TIER: u8 = (BAKE_TIERS.len() - 1) as u8;

/// Frames a second a clip of `duration` seconds is baked at in `tier`.
pub fn bake_fps_tier(duration: f32, tier: u8) -> f32 {
    let (fps, max_frames) = BAKE_TIERS[usize::from(tier.min(LAST_TIER))];
    if !duration.is_finite() || duration <= 0.0 {
        return fps;
    }
    fps.min((max_frames - 1) as f32 / duration)
}

/// `ceil(duration * fps) + 1` in `tier`, never fewer than 2.
pub fn frames_tier(duration: f32, tier: u8) -> u32 {
    if !duration.is_finite() || duration <= 0.0 {
        return 2;
    }
    ((duration * bake_fps_tier(duration, tier)).ceil() as u32)
        .saturating_add(1)
        .max(2)
}

/// Frames a second a clip of `duration` seconds is baked at: [`BAKE_FPS`], or less if that would be
/// more than [`MAX_BAKE_FRAMES`] frames.
pub fn bake_fps_of(duration: f32) -> f32 {
    bake_fps_tier(duration, 0)
}

/// `ceil(duration * fps) + 1` ([`bake_fps_of`]; 30 for a clip up to about 5 s), never fewer than 2.
pub fn frames_of(duration: f32) -> u32 {
    frames_tier(duration, 0)
}

/// The clips of one model, in the order of the blob.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClipTable {
    entries: Vec<ClipEntry>,
}

impl ClipTable {
    pub fn new(entries: Vec<ClipEntry>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[ClipEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The first clip with event code `ec` (0 is "none" and finds nothing).
    pub fn by_ec(&self, ec: u32) -> Option<&ClipEntry> {
        if ec == 0 {
            return None;
        }
        self.entries.iter().find(|c| c.ec == ec)
    }

    /// Every clip with event code `ec`, in order (`ec` 1000..=1003 of the animals).
    pub fn all_ec(&self, ec: u32) -> impl Iterator<Item = &ClipEntry> {
        self.entries.iter().filter(move |c| ec != 0 && c.ec == ec)
    }

    pub fn by_index(&self, index: u16) -> Option<&ClipEntry> {
        self.entries.iter().find(|c| c.index == index)
    }

    /// The clip that plays when nothing else is asked: `ec` 1000; for a model without codes the
    /// first clip with "idle" in its name; for a model that has neither, its first clip.
    pub fn idle(&self) -> Option<&ClipEntry> {
        self.by_ec(EC_IDLE_A)
            .or_else(|| {
                self.entries
                    .iter()
                    .find(|c| c.name.to_ascii_lowercase().contains("idle"))
            })
            .or_else(|| self.entries.first())
    }

    /// True if the model has a clip that is an idle one by code or by name.
    pub fn has_idle(&self) -> bool {
        self.by_ec(EC_IDLE_A).is_some()
            || self
                .entries
                .iter()
                .any(|c| c.name.to_ascii_lowercase().contains("idle"))
    }

    /// Sum of the baked slots of all clips for a skeleton of `bones`.
    pub fn slots(&self, bones: u32) -> u64 {
        self.entries
            .iter()
            .map(|c| u64::from(c.frames()) * u64::from(bones))
            .sum()
    }
}

/// Everything `world-anim` needs of one model.
#[derive(Debug, Clone)]
pub struct ModelAnim {
    /// `.fxsxml` name (the `source` of the blob).
    pub source: String,
    pub skeleton: Arc<Skeleton>,
    pub pack: Arc<AnimPack>,
    pub clips: ClipTable,
    /// The automaton with the program of each state; `None` for a model with no `fsm`.
    pub machine: Option<MachineDef>,
    /// Decision rows of a landmark (`.ddtxml`).
    pub state_clips: Vec<StateClip>,
    /// What the building of the programs found worth a line in the report.
    pub notes: Vec<GraphNote>,
    /// Clips of a model converted from v1 (they live in separate `C5AN` blobs, not in a pack): not
    /// read, counted for the report.
    pub legacy_clips: usize,
}

impl ModelAnim {
    /// From the decoded blobs. `pack` is the `an:` blob of `model.anim_pack`, `fsm` the `fsm:` blob
    /// of `model.fsm`; a missing one is `None`.
    pub fn new(
        model: &ModelBlobV2,
        pack: Option<AnimPack>,
        fsm: Option<&IdleFsm>,
    ) -> Result<Self, AnimDataError> {
        let pack = pack.unwrap_or_default();
        let bones = model.skeleton.bones.len();
        let mut entries = Vec::with_capacity(model.clips.len());
        let mut legacy = 0;
        for c in &model.clips {
            if c.legacy_blob.is_some() {
                legacy += 1;
                continue;
            }
            let Some(clip) = pack.clips.get(usize::from(c.index)) else {
                if model.anim_pack.is_none() {
                    return Err(AnimDataError::NoPack {
                        model: model.source.clone(),
                    });
                }
                return Err(AnimDataError::ClipIndex {
                    model: model.source.clone(),
                    clip: c.name.clone(),
                    index: c.index,
                    len: pack.clips.len(),
                });
            };
            entries.push(ClipEntry {
                name: c.name.clone(),
                ec: c.ec,
                index: c.index,
                duration: clip.duration,
            });
        }
        let span = pack.bone_span();
        if span > bones {
            return Err(AnimDataError::BoneSpan {
                model: model.source.clone(),
                span,
                bones,
            });
        }
        let clips = ClipTable::new(entries);
        let mut notes = Vec::new();
        let machine = fsm.map(|f| MachineDef::build(f, &clips, &mut notes));
        Ok(Self {
            source: model.source.clone(),
            skeleton: Arc::new(model.skeleton.clone()),
            pack: Arc::new(pack),
            clips,
            machine,
            state_clips: model.state_clips.clone(),
            notes,
            legacy_clips: legacy,
        })
    }

    /// From the bytes of the three blobs (`AssetLoader` gives bytes, 4.1).
    pub fn from_bytes(
        model: &[u8],
        pack: Option<&[u8]>,
        fsm: Option<&[u8]>,
    ) -> Result<Self, AnimDataError> {
        let model = ModelBlobV2::decode(model)?;
        let pack = pack.map(AnimPack::decode).transpose()?;
        let fsm = fsm.map(IdleFsm::decode).transpose()?;
        Self::new(&model, pack, fsm.as_ref())
    }

    /// Bones of the skeleton: the size of one baked frame in slots.
    pub fn bones(&self) -> u32 {
        self.skeleton.bones.len() as u32
    }

    /// True if there is anything to play.
    pub fn animated(&self) -> bool {
        !self.clips.is_empty()
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Synthetic blobs for the tests of the module: a two-bone model with the clips of an idle
    //! family, a pack that moves bone 1, a machine of the shape of `1_omni_state_01`.

    use asset_format::{
        Bone, Channel, Clip, ClipRef, FsmCondition, FsmState, IdleFsm, Skeleton, TrackKeys,
        Transform, Transition,
    };
    use glam::Quat;

    use super::*;

    pub fn skeleton(bones: usize) -> Skeleton {
        Skeleton {
            bones: (0..bones)
                .map(|i| Bone {
                    name: format!("b{i}"),
                    parent: i.checked_sub(1).map(|p| p as u16),
                    rest: Transform {
                        translation: [0.0, 1.0, 0.0],
                        ..Transform::IDENTITY
                    },
                    // The inverse of the rest world matrix of a chain that climbs 1 a bone.
                    inverse_bind: glam::Mat4::from_translation(glam::Vec3::new(
                        0.0,
                        -((i + 1) as f32),
                        0.0,
                    ))
                    .to_cols_array(),
                })
                .collect(),
        }
    }

    fn pack_quats(v: &[Quat]) -> Vec<u8> {
        v.iter()
            .flat_map(|q| {
                asset_format::quat_pack(q.to_array())
                    .into_iter()
                    .flat_map(i16::to_le_bytes)
            })
            .collect()
    }

    fn pack_times(v: &[u16]) -> Vec<u8> {
        v.iter().flat_map(|t| t.to_le_bytes()).collect()
    }

    /// A clip of `duration` seconds that swings the bones `1..` about Z by `angle` degrees and
    /// back.
    pub fn swing_clip(name: &str, duration: f32, bones: usize, angle: f32) -> Clip {
        let a = Quat::from_rotation_z(angle.to_radians());
        let track = |bone: u16| TrackKeys {
            bone,
            rot: Channel::Keys {
                count: 3,
                times: pack_times(&[0, 32768, 65535]),
                values: pack_quats(&[Quat::IDENTITY, a, Quat::IDENTITY]),
            },
            pos: Channel::Rest,
            scale: Channel::Rest,
        };
        Clip {
            name: name.into(),
            duration,
            tracks: (1..bones as u16).map(track).collect(),
        }
    }

    /// The clips of an idle family on a skeleton of `bones`.
    pub fn idle_pack(bones: usize) -> (AnimPack, Vec<ClipRef>) {
        let list = [
            ("Unit_IdleA", 1000, 2.0, 20.0),
            ("Unit_IdleA_to_B", 1020, 0.5, 10.0),
            ("Unit_FidgetA", 1040, 1.0, 30.0),
            ("Unit_IdleB", 2000, 2.5, 15.0),
            ("Unit_IdleB_to_A", 2020, 0.5, 10.0),
            ("Unit_FidgetB", 2040, 1.2, 25.0),
        ];
        let mut pack = AnimPack::default();
        let mut refs = Vec::new();
        for (i, (name, ec, dur, angle)) in list.into_iter().enumerate() {
            pack.clips.push(swing_clip(name, dur, bones, angle));
            refs.push(ClipRef {
                name: name.into(),
                ec,
                index: i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        (pack, refs)
    }

    /// A model with the idle family and, when `with_fsm`, a machine named `1_omni_state_01` shaped
    /// like the real one: a chooser that goes to IDLE with 0.4 and to FIDGETS with 0.6, and the
    /// fortify and sleep states.
    pub fn model(name: &str, bones: usize) -> (ModelBlobV2, AnimPack) {
        let (pack, clips) = idle_pack(bones);
        let m = ModelBlobV2 {
            source: name.into(),
            skeleton: skeleton(bones),
            clips,
            anim_pack: Some(asset_format::LogicalId::new(format!("an:{name}"))),
            fsm: Some(asset_format::LogicalId::new("fsm:1_omni_state_01")),
            ..ModelBlobV2::default()
        };
        (m, pack)
    }

    pub fn omni_machine() -> IdleFsm {
        let state = |name: &str, action: u32, asset: &str, next: Vec<(u16, f32)>| FsmState {
            name: name.into(),
            action,
            asset: asset.into(),
            next,
            transitions: vec![Transition {
                to: 0,
                conditions: vec![],
            }],
        };
        IdleFsm {
            name: "1_omni_state_01".into(),
            default: 0,
            states: vec![
                state("IDLE_CHOOSER_DUMMY", 0, "", vec![(1, 0.4), (2, 0.6)]),
                state("IDLE_LOOP", 1000, "IDLE_GRAPH.dge", vec![]),
                state("FIDGETS_LOOP", 1000, "FIDGETS_GRAPH.dge", vec![]),
                state("FORTIFY_LOOP", 1500, "FORTIFY_GRAPH.dge", vec![]),
                state("SLEEP_LOOP", 1500, "FORTIFY_GRAPH.dge", vec![]),
                state(
                    "OFFSET_LOOP",
                    1000,
                    "IDLE_OFFSET_CONTINUE_TRUE_GRAPH.dge",
                    vec![],
                ),
                state("MYSTERY", 1234, "MYSTERY_GRAPH.dge", vec![]),
                state("ENTER_FORTIFY", 0, "ENTER_FORTIFY.dge", vec![]),
            ],
        }
    }

    /// The same states with the transitions and conditions of a `C5FM` v2: the chooser, idle that
    /// leaves by a roll after a time, a fidget that ends by its clip and may flip IdleA/IdleB, and
    /// fortify entered by the request 1500 through ENTER_FORTIFY (left when its clip is done, as
    /// in `1_omni_state_01`) and left by any other request.
    pub fn omni_machine_v2() -> IdleFsm {
        let to_if = |to: u16, conditions: Vec<FsmCondition>| Transition { to, conditions };
        let asked = |method: u8, value: i32| FsmCondition::CheckPathOption {
            method,
            path: 4,
            value,
        };
        // The methods of FCompareValues: 1 equal, 6 not equal.
        let mut m = omni_machine();
        for s in &mut m.states {
            s.transitions.clear();
        }
        let fortify = |m: &mut IdleFsm, from: usize| {
            m.states[from]
                .transitions
                .insert(0, to_if(7, vec![asked(1, 1500)]));
        };
        m.states[0].transitions = vec![to_if(7, vec![asked(1, 1500)])];
        m.states[7].transitions = vec![to_if(
            3,
            vec![FsmCondition::EcComplete {
                ec: 1500,
                value: None,
            }],
        )];
        fortify(&mut m, 1);
        fortify(&mut m, 2);
        m.states[1].transitions.push(to_if(
            0,
            vec![
                FsmCondition::CheckStateTime {
                    method: 5,
                    value: 2.0,
                },
                FsmCondition::RandomRoll {
                    value: 0.02,
                    invert: false,
                },
                asked(1, 1000),
            ],
        ));
        m.states[2].transitions.push(to_if(
            1,
            vec![
                FsmCondition::EcComplete {
                    ec: 1040,
                    value: Some(0.98),
                },
                FsmCondition::EventRandomPathOption {
                    path: 0,
                    value: 0,
                    use_weights: true,
                    options: vec![(0, 0.7), (1, 0.3)],
                },
            ],
        ));
        for from in [3usize, 4] {
            m.states[from].transitions = vec![to_if(1, vec![asked(6, 1500)])];
        }
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::ClipRef;

    #[test]
    fn frames_are_ceil_duration_30_plus_one() {
        // A long clip is baked at a lower rate, at most `MAX_BAKE_FRAMES` frames.
        assert_eq!(bake_fps_of(2.0), BAKE_FPS);
        assert!(frames_of(47.0) <= MAX_BAKE_FRAMES + 1);
        assert!(bake_fps_of(47.0) < BAKE_FPS);
        assert_eq!(frames_of(2.0), 61);
        assert_eq!(frames_of(2.01), 62);
        assert_eq!(frames_of(0.0), 2);
        assert_eq!(frames_of(f32::NAN), 2);
        assert_eq!(frames_of(-1.0), 2);
    }

    #[test]
    fn a_model_reads_its_clips_by_code() {
        let (m, pack) = fixtures::model("warrior", 3);
        let a = ModelAnim::new(&m, Some(pack), None).unwrap();
        assert_eq!(a.clips.len(), 6);
        assert_eq!(a.clips.by_ec(EC_IDLE_A).unwrap().name, "Unit_IdleA");
        assert_eq!(a.clips.by_ec(EC_FIDGET_B).unwrap().index, 5);
        assert!(a.clips.by_ec(0).is_none());
        assert!(a.clips.by_ec(1500).is_none());
        assert_eq!(a.clips.idle().unwrap().ec, EC_IDLE_A);
        assert!(a.animated() && a.machine.is_none());
        assert_eq!(a.bones(), 3);
    }

    #[test]
    fn a_model_without_codes_idles_by_name() {
        let (mut m, pack) = fixtures::model("whale", 2);
        for c in &mut m.clips {
            c.ec = 0;
        }
        m.clips.swap(0, 3);
        let a = ModelAnim::new(&m, Some(pack), None).unwrap();
        // The first clip with "idle" in the name, in the order of the blob.
        assert_eq!(a.clips.idle().unwrap().name, "Unit_IdleB");
        assert!(a.clips.has_idle());
    }

    #[test]
    fn a_model_without_clips_is_still_a_model() {
        let m = ModelBlobV2 {
            source: "wall".into(),
            skeleton: fixtures::skeleton(1),
            ..ModelBlobV2::default()
        };
        let a = ModelAnim::new(&m, None, None).unwrap();
        assert!(!a.animated());
        assert!(a.clips.idle().is_none());
        assert!(!a.clips.has_idle());
    }

    #[test]
    fn a_clip_outside_the_pack_or_the_skeleton_refuses_the_model() {
        let (mut m, pack) = fixtures::model("x", 3);
        m.clips[0].index = 9;
        assert!(matches!(
            ModelAnim::new(&m, Some(pack.clone()), None),
            Err(AnimDataError::ClipIndex {
                index: 9,
                len: 6,
                ..
            })
        ));
        m.anim_pack = None;
        assert!(matches!(
            ModelAnim::new(&m, None, None),
            Err(AnimDataError::NoPack { .. })
        ));
        // Tracks of bone 2 on a skeleton of two.
        let (m, pack) = fixtures::model("y", 3);
        let mut small = m.clone();
        small.skeleton = fixtures::skeleton(2);
        assert!(matches!(
            ModelAnim::new(&small, Some(pack), None),
            Err(AnimDataError::BoneSpan {
                span: 3,
                bones: 2,
                ..
            })
        ));
    }

    #[test]
    fn clips_of_a_v1_model_are_counted_not_read() {
        let (mut m, pack) = fixtures::model("old", 2);
        m.clips.push(ClipRef {
            name: "old_idle".into(),
            ec: 1000,
            index: 0,
            events: vec![],
            legacy_blob: Some(asset_format::LogicalId::new("anim.idle")),
        });
        let a = ModelAnim::new(&m, Some(pack), None).unwrap();
        assert_eq!((a.clips.len(), a.legacy_clips), (6, 1));
    }

    #[test]
    fn the_three_blobs_are_read_from_bytes() {
        let (m, pack) = fixtures::model("warrior", 3);
        let fsm = fixtures::omni_machine();
        let a =
            ModelAnim::from_bytes(&m.encode(), Some(&pack.encode()), Some(&fsm.encode())).unwrap();
        assert_eq!(a.machine.as_ref().unwrap().states.len(), 8);
        assert!(ModelAnim::from_bytes(&m.encode()[..20], None, None).is_err());
        assert!(ModelAnim::from_bytes(&m.encode(), Some(b"nope"), None).is_err());
    }

    #[test]
    fn the_never_requested_codes_are_not_idle_or_fortify() {
        for ec in EC_NEVER {
            assert!(
                ![EC_IDLE_A, EC_IDLE_B, EC_FORTIFY].contains(&ec) && !(1910..=1960).contains(&ec)
            );
        }
    }
}
