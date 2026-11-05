//! Whole-file validation used by `civ5-assets probe --kind gr2` and the game-file tests:
//! parses everything typed, samples every curve and checks that values are sane.

use crate::animation::Animation;
use crate::curve::format;
use crate::skeleton::Skeleton;
use crate::{GrannyError, GrannyFile, Scene};

/// Counts gathered from one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileStats {
    pub skeletons: usize,
    pub bones: usize,
    pub meshes: usize,
    pub skinned_meshes: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub models: usize,
    pub animations: usize,
    pub tracks: usize,
    /// Curves per format code (all three curves of every track).
    pub curve_formats: [usize; format::COUNT],
    /// Vertex members that were not decoded (names), e.g. unknown hashed attributes.
    pub skipped_members: Vec<String>,
}

impl FileStats {
    pub fn add(&mut self, o: &FileStats) {
        self.skeletons += o.skeletons;
        self.bones += o.bones;
        self.meshes += o.meshes;
        self.skinned_meshes += o.skinned_meshes;
        self.vertices += o.vertices;
        self.triangles += o.triangles;
        self.models += o.models;
        self.animations += o.animations;
        self.tracks += o.tracks;
        for (a, b) in self.curve_formats.iter_mut().zip(&o.curve_formats) {
            *a += b;
        }
        for m in &o.skipped_members {
            if !self.skipped_members.contains(m) {
                self.skipped_members.push(m.clone());
            }
        }
    }
}

/// Track IDs and the pose at t=0 of an animation's first track group, enough to check the
/// binding against a skeleton from another file without keeping curves in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimSummary {
    pub track_ids: Vec<Option<u32>>,
    pub track_names: Vec<Option<String>>,
    /// Translation and rotation at t=0 for each track.
    pub start_pose: Vec<([f32; 3], [f32; 4])>,
}

/// A fully decoded and validated file.
#[derive(Debug, Clone)]
pub struct Checked {
    pub stats: FileStats,
    /// First skeleton of a file that also has a mesh (a character model).
    pub model_skeleton: Option<Skeleton>,
    /// Summary of the first animation, if any.
    pub anim: Option<AnimSummary>,
}

/// Times (as fractions of the duration) at which each track is sampled.
const SAMPLE_FRACTIONS: [f32; 7] = [0.0, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0];
/// Allowed deviation of an evaluated (un-normalised) quaternion's length from 1.
const QUAT_NORM_TOLERANCE: f32 = 0.5;

fn invalid(msg: String) -> GrannyError {
    GrannyError::Invalid(msg)
}

fn check_animation(a: &Animation, stats: &mut FileStats) -> Result<(), GrannyError> {
    for g in &a.track_groups {
        for (ti, t) in g.transform_tracks.iter().enumerate() {
            stats.tracks += 1;
            for c in [&t.position, &t.orientation, &t.scale_shear] {
                stats.curve_formats[usize::from(c.format).min(format::COUNT - 1)] += 1;
                if !c.is_finite() {
                    return Err(invalid(format!(
                        "track {ti}: non-finite {} data",
                        crate::curve::format_name(c.format)
                    )));
                }
                if c.knots.windows(2).any(|w| w[1] < w[0] - 1e-3) {
                    return Err(invalid(format!(
                        "track {ti}: decreasing knots in {}",
                        crate::curve::format_name(c.format)
                    )));
                }
            }
            for f in SAMPLE_FRACTIONS {
                let time = f * a.duration;
                let p = t.sample_raw(time);
                let finite = p
                    .translation
                    .iter()
                    .chain(&p.rotation)
                    .chain(&p.scale_shear)
                    .all(|x| x.is_finite());
                if !finite {
                    return Err(invalid(format!("track {ti}: non-finite sample at {time}")));
                }
                let n = p.rotation.iter().map(|x| x * x).sum::<f32>().sqrt();
                if (n - 1.0).abs() > QUAT_NORM_TOLERANCE {
                    return Err(invalid(format!(
                        "track {ti}: quaternion length {n:.3} at {time}"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Parses `data` as a gr2 file and validates its typed contents.
pub fn check_file(data: &[u8]) -> Result<Checked, GrannyError> {
    let file = GrannyFile::parse(data)?;
    file.walk()?;
    let scene = Scene::read(&file)?;
    let mut stats = FileStats {
        skeletons: scene.skeletons.len(),
        bones: scene.skeletons.iter().map(|s| s.bones.len()).sum(),
        meshes: scene.meshes.len(),
        models: scene.models.len(),
        animations: scene.animations.len(),
        ..FileStats::default()
    };
    for m in &scene.meshes {
        stats.skinned_meshes += usize::from(m.is_skinned());
        stats.vertices += m.vertex_count();
        stats.triangles += m.triangle_count();
        stats.add(&FileStats {
            skipped_members: m.skipped_members.clone(),
            ..FileStats::default()
        });
        let all = m
            .positions
            .iter()
            .flatten()
            .chain(m.normals.iter().flatten().flatten())
            // Secondary UVs of some leader meshes hold NaNs in the source; they are not used.
            .chain(m.uv.first().into_iter().flatten().flatten());
        if all.into_iter().any(|x| !x.is_finite()) {
            return Err(invalid("non-finite vertex data".into()));
        }
    }
    for a in &scene.animations {
        check_animation(a, &mut stats)?;
    }
    let anim = scene.animations.first().and_then(|a| {
        let g = a.track_groups.first()?;
        Some(AnimSummary {
            track_ids: g.transform_tracks.iter().map(|t| t.name_id).collect(),
            track_names: g.transform_tracks.iter().map(|t| t.name.clone()).collect(),
            start_pose: g
                .transform_tracks
                .iter()
                .map(|t| {
                    let p = t.sample(0.0);
                    (p.translation, p.rotation)
                })
                .collect(),
        })
    });
    let model_skeleton = if scene.meshes.is_empty() {
        None
    } else {
        scene.skeletons.into_iter().next()
    };
    Ok(Checked {
        stats,
        model_skeleton,
        anim,
    })
}
