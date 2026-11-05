//! Binding animation tracks to skeleton bones.
//!
//! Granny binds by track name. Packed Civ5 files strip every name string but leave a 32-bit ID
//! (a name hash) in the `Name` slot of bones, tracks and mesh bone bindings, so the same
//! lookup works with IDs. Track order is unrelated to bone order (it looks like hash order).

use crate::animation::TrackGroup;
use crate::skeleton::Skeleton;

/// How tracks were matched to bones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Name,
    Id,
}

/// Result of binding one track group to a skeleton.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    /// Bone for each transform track; `None` when the track matches no bone.
    pub track_to_bone: Vec<Option<usize>>,
    pub method: Method,
}

impl Binding {
    /// True if every track found a distinct bone.
    pub fn is_complete(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        self.track_to_bone
            .iter()
            .all(|b| b.is_some_and(|b| seen.insert(b)))
    }

    /// Number of tracks that found no bone.
    pub fn unbound(&self) -> usize {
        self.track_to_bone.iter().filter(|b| b.is_none()).count()
    }

    /// Track for each bone (last wins on duplicates).
    pub fn bone_to_track(&self, bones: usize) -> Vec<Option<usize>> {
        let mut v = vec![None; bones];
        for (t, b) in self.track_to_bone.iter().enumerate() {
            if let Some(slot) = b.and_then(|b| v.get_mut(b)) {
                *slot = Some(t);
            }
        }
        v
    }
}

/// Binds by track name, or by ID if the tracks carry no names. Names and IDs mix across files
/// (a loose mod mesh with names, the stripped clips of the base game with IDs), so a name also
/// matches a bone that only has the ID of that name, and an ID a bone whose name hashes to it.
pub fn bind(skel: &Skeleton, group: &TrackGroup) -> Binding {
    let named = group.transform_tracks.iter().any(|t| t.name.is_some());
    let track_to_bone = group
        .transform_tracks
        .iter()
        .map(|t| match (&t.name, t.name_id) {
            (Some(n), _) => skel
                .find_bone(n)
                .or_else(|| skel.find_bone_by_id(crate::names::name_hash(n))),
            (None, Some(id)) => skel.find_bone_by_id(id),
            (None, None) => None,
        })
        .collect();
    Binding {
        track_to_bone,
        method: if named { Method::Name } else { Method::Id },
    }
}

/// How well a binding reproduces the rest pose at `t`: per bound track, the distance between
/// the sampled translation and the bone's rest translation, and the rotation angle (radians)
/// between sampled and rest orientation.
pub fn pose_errors(
    skel: &Skeleton,
    group: &TrackGroup,
    binding: &Binding,
    t: f32,
) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    for (track, bone) in group.transform_tracks.iter().zip(&binding.track_to_bone) {
        let Some(bone) = bone.and_then(|b| skel.bones.get(b)) else {
            continue;
        };
        let p = track.sample(t);
        let dp = (0..3)
            .map(|k| (p.translation[k] - bone.local.position[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        let dot: f32 = (0..4)
            .map(|k| p.rotation[k] * bone.local.orientation[k])
            .sum::<f32>()
            .abs()
            .min(1.0);
        out.push((dp, 2.0 * dot.acos()));
    }
    out
}
