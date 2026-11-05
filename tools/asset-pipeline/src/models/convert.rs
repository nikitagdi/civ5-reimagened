//! Granny scene to engine blobs: skeleton, skinned meshes and animation tracks.

use std::cell::Cell;
use std::collections::BTreeMap;

use asset_format::{
    AnimationBlob, Bone, Indices, Material, Mesh, ModelBlob, Skeleton, Transform, f32_to_f16_bits,
    oct_encode, quantize_weights,
};
use granny::animation::{Animation, Pose};
use granny::bind::bind;
use granny::skeleton::Skeleton as GrannySkeleton;
use granny::{Scene, mesh::Mesh as GrannyMesh};

use super::ModelError;
use super::basis::Basis;
use super::resample::{SAMPLE_RATE, Sample, resample_track};

/// Granny/Max UVs have V up; wgpu and glTF textures have V down.
pub const UV_FLIP_V: bool = true;

/// Source triangles are counter-clockwise seen from outside (checked against the vertex normals
/// by a test on the real Warrior). `false` would reverse every triangle.
pub const SOURCE_WINDING_CCW: bool = true;

/// Off-diagonal scale/shear terms above this are reported (they are dropped).
const SHEAR_WARN: f32 = 1e-3;

/// Engine materials of a model and the engine material of each Granny material (the index a
/// mesh group reaches through its mesh's `MaterialBindings`). See [`super::materials`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelMaterials {
    pub materials: Vec<Material>,
    /// Indexed by Granny material; empty means every group uses material 0.
    pub by_granny: Vec<u16>,
}

impl ModelMaterials {
    /// One material for every group.
    pub fn single(material: Material) -> Self {
        Self {
            materials: vec![material],
            by_granny: Vec::new(),
        }
    }

    /// Engine material of a group with `MaterialIndex` `group_material` of `mesh`; `None` if the
    /// binding is missing (the caller falls back to 0 and warns).
    fn of_group(&self, mesh: &GrannyMesh, group_material: u32) -> Option<u16> {
        if self.by_granny.is_empty() {
            return Some(0);
        }
        let granny = mesh
            .material_bindings
            .get(group_material as usize)
            .copied()
            .flatten()?;
        self.by_granny
            .get(granny)
            .copied()
            .filter(|&m| usize::from(m) < self.materials.len())
    }
}

#[derive(Debug)]
pub struct ConvertedModel {
    /// Animations are left empty; the caller fills them from the art definition.
    pub model: ModelBlob,
    /// Index of the Granny skeleton in the scene, for binding animation tracks.
    pub skeleton: usize,
    pub warnings: Vec<String>,
}

fn bone_name(b: &granny::skeleton::Bone, index: usize) -> String {
    match (&b.name, b.name_id) {
        (Some(n), _) => n.clone(),
        (None, Some(id)) => format!("bone_{id:08x}"),
        (None, None) => format!("bone_{index}"),
    }
}

/// The rest transform in engine convention, plus whether scale/shear was a pure diagonal.
fn rest_transform(b: &granny::skeleton::Bone, basis: &Basis) -> (Transform, bool) {
    let trs = b.local_trs();
    let t = Transform {
        translation: trs.translation,
        rotation: normalize_quat(trs.rotation),
        scale: trs.scale,
    };
    (basis.transform(&t), trs.shear_free)
}

pub(super) fn normalize_quat(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if n.is_finite() && n > 1e-6 {
        q.map(|c| c / n)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

pub fn convert_skeleton(
    skel: &GrannySkeleton,
    basis: &Basis,
    warnings: &mut Vec<String>,
) -> Result<Skeleton, ModelError> {
    let skeleton = convert_skeleton_any(skel, basis, warnings)?;
    if skeleton.bones.len() > 255 {
        return Err(ModelError::invalid(format!(
            "skeleton has {} bones, joints are u8",
            skeleton.bones.len()
        )));
    }
    Ok(skeleton)
}

/// [`convert_skeleton`] without the u8 joint limit (the v2 models use `U16x4` joints for big
/// skeletons).
pub fn convert_skeleton_any(
    skel: &GrannySkeleton,
    basis: &Basis,
    warnings: &mut Vec<String>,
) -> Result<Skeleton, ModelError> {
    let mut bones = Vec::with_capacity(skel.bones.len());
    for (i, b) in skel.bones.iter().enumerate() {
        let parent = match b.parent {
            None => None,
            Some(p) if p < i => Some(u16::try_from(p).map_err(|_| {
                ModelError::invalid(format!("bone {i}: parent index {p} does not fit u16"))
            })?),
            Some(p) => {
                return Err(ModelError::invalid(format!(
                    "bone {i}: parent {p} does not precede it"
                )));
            }
        };
        let (rest, shear_free) = rest_transform(b, basis);
        if !shear_free {
            warnings.push(format!(
                "bone {i} ({}): rest scale/shear has off-diagonal terms (dropped)",
                bone_name(b, i)
            ));
        }
        bones.push(Bone {
            name: bone_name(b, i),
            parent,
            rest,
            inverse_bind: basis.matrix(b.inverse_world),
        });
    }
    Ok(Skeleton { bones })
}

/// Skinned vertex joints for `mesh`: vertex bone indices go through `BoneBindings`.
///
/// A binding whose name ID is shared by several skeleton bones is ambiguous for the reader
/// (`Skeleton::find_bone_by_id` wants a unique match). The Great Artist (Flute) skeleton has two
/// bones with the same name and the same rest and inverse-bind transforms; skinning to either is
/// the same, so the first is used and the file gets a warning. Bones that differ stay an error.
pub(super) fn joint_table(
    mesh: &GrannyMesh,
    skeleton: &GrannySkeleton,
    warnings: &mut Vec<String>,
) -> Result<Vec<u16>, ModelError> {
    joint_table_with(mesh, skeleton, warnings, false)
}

/// [`joint_table`]; with `root_fallback` a binding that matches no bone (the exporter's helper
/// dummy) follows the root bone, with a warning (B2 world models).
pub(super) fn joint_table_with(
    mesh: &GrannyMesh,
    skeleton: &GrannySkeleton,
    warnings: &mut Vec<String>,
    root_fallback: bool,
) -> Result<Vec<u16>, ModelError> {
    let bones = skeleton.bones.len();
    mesh.bone_bindings
        .iter()
        .enumerate()
        .map(|(i, b)| match b.bone {
            Some(bone) if bone < bones => u16::try_from(bone)
                .map_err(|_| ModelError::invalid(format!("bone index {bone} does not fit u16"))),
            _ => {
                let same: Vec<usize> = b
                    .name_id
                    .map(|id| {
                        skeleton
                            .bones
                            .iter()
                            .enumerate()
                            .filter(|(_, x)| x.name_id == Some(id))
                            .map(|(k, _)| k)
                            .collect()
                    })
                    .unwrap_or_default();
                let twins = same.len() > 1
                    && same.iter().all(|&k| {
                        skeleton.bones[k].inverse_world == skeleton.bones[same[0]].inverse_world
                            && skeleton.bones[k].local == skeleton.bones[same[0]].local
                    });
                if twins {
                    warnings.push(format!(
                        "mesh {:?}: bone binding {i} names bones {same:?} alike with equal transforms; using bone {}",
                        mesh.name, same[0]
                    ));
                    u16::try_from(same[0]).map_err(|_| {
                        ModelError::invalid(format!("bone index {} does not fit u16", same[0]))
                    })
                } else if let Some(root) = root_fallback
                    .then(|| skeleton.bones.iter().position(|x| x.parent.is_none()))
                    .flatten()
                {
                    // The exporter bound the mesh to a helper (a `Point01` dummy) that the
                    // skeleton does not carry: the mesh follows the root bone.
                    warnings.push(format!(
                        "mesh {:?}: bone binding {i} ({}) matches no skeleton bone; using the root bone {root}",
                        mesh.name,
                        b.name.as_deref().unwrap_or("?")
                    ));
                    u16::try_from(root).map_err(|_| {
                        ModelError::invalid(format!("bone index {root} does not fit u16"))
                    })
                } else {
                    Err(ModelError::invalid(format!(
                        "mesh {:?}: bone binding {i} matches no skeleton bone",
                        mesh.name
                    )))
                }
            }
        })
        .collect()
}

fn pick<T: Copy>(v: &[T], keep: &[u32]) -> Vec<T> {
    keep.iter().map(|&i| v[i as usize]).collect()
}

pub(super) struct Range {
    /// Triangle indices (into the mesh vertex list) of this group.
    pub(super) indices: Vec<u32>,
    pub(super) material: u32,
}

pub(super) fn group_ranges(mesh: &GrannyMesh) -> Vec<Range> {
    if mesh.groups.is_empty() {
        return vec![Range {
            indices: mesh.indices.clone(),
            material: 0,
        }];
    }
    mesh.groups
        .iter()
        .map(|g| {
            let from = g.tri_first as usize * 3;
            let to = from + g.tri_count as usize * 3;
            Range {
                indices: mesh.indices[from..to].to_vec(),
                material: g.material_index,
            }
        })
        .collect()
}

/// Applies a Granny transform (`T * R * S`, column vectors) to rigid mesh data in source axes.
struct Placement {
    m: [f32; 16],
    /// Inverse-transpose of the linear part, rows, for normals.
    normal: [[f32; 3]; 3],
}

impl Placement {
    fn new(t: &granny::Transform) -> Option<Self> {
        if is_identity(t) {
            return None;
        }
        let m = granny::skeleton::transform_matrix(t);
        let a = |r: usize, c: usize| m[c * 4 + r];
        let det = a(0, 0) * (a(1, 1) * a(2, 2) - a(1, 2) * a(2, 1))
            - a(0, 1) * (a(1, 0) * a(2, 2) - a(1, 2) * a(2, 0))
            + a(0, 2) * (a(1, 0) * a(2, 1) - a(1, 1) * a(2, 0));
        let inv = if det.abs() > 1e-12 { 1.0 / det } else { 0.0 };
        // Inverse-transpose = cofactor matrix / det.
        let cof = |r: usize, c: usize| {
            let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
            let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
            (a(r1, c1) * a(r2, c2) - a(r1, c2) * a(r2, c1)) * inv
        };
        let normal = std::array::from_fn(|r| std::array::from_fn(|c| cof(r, c)));
        Some(Self { m, normal })
    }

    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.m;
        std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
    }

    fn normal(&self, n: [f32; 3]) -> [f32; 3] {
        let v: [f32; 3] = std::array::from_fn(|r| (0..3).map(|c| self.normal[r][c] * n[c]).sum());
        let len = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if len > 1e-12 { v.map(|x| x / len) } else { n }
    }
}

/// One engine mesh per material group; vertices are shared only when there is a single group.
/// `placement` (rigid meshes only) moves the mesh within the model.
fn convert_mesh(
    mesh: &GrannyMesh,
    skeleton: &GrannySkeleton,
    basis: &Basis,
    materials: &ModelMaterials,
    placement: Option<&Placement>,
    warnings: &mut Vec<String>,
) -> Result<Vec<Mesh>, ModelError> {
    if mesh.positions.is_empty() || mesh.indices.is_empty() {
        return Ok(Vec::new());
    }
    let n = mesh.positions.len();
    let skinned = mesh.is_skinned();
    let joint_map = if skinned {
        joint_table(mesh, skeleton, warnings)?
    } else {
        Vec::new()
    };
    if mesh.normals.is_none() {
        warnings.push(format!("mesh {:?}: no normals, using +Y", mesh.name));
    }
    let uv0 = mesh.uv.first();
    if uv0.is_none() {
        warnings.push(format!("mesh {:?}: no texture coordinates", mesh.name));
    }
    let flip = basis.flips_handedness() ^ !SOURCE_WINDING_CCW;
    let mut bad_weights = 0usize;

    // Per-vertex data for the whole granny mesh, in engine convention.
    let place = if skinned { None } else { placement };
    let positions: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|&p| basis.point(place.map_or(p, |t| t.point(p))))
        .collect();
    let normals: Vec<[i16; 2]> = (0..n)
        .map(|i| {
            let nrm = mesh
                .normals
                .as_ref()
                .and_then(|v| v.get(i))
                .map_or([0.0, 1.0, 0.0], |&n| {
                    basis.point(place.map_or(n, |t| t.normal(n)))
                });
            oct_encode(nrm)
        })
        .collect();
    let uvs: Vec<[u16; 2]> = (0..n)
        .map(|i| {
            let [u, v] = uv0.and_then(|t| t.get(i)).copied().unwrap_or([0.0, 0.0]);
            let v = if UV_FLIP_V { 1.0 - v } else { v };
            [f32_to_f16_bits(u), f32_to_f16_bits(v)]
        })
        .collect();
    let (joints, weights): (Vec<[u8; 4]>, Vec<[u8; 4]>) = if skinned {
        let (bi, bw) = (
            mesh.bone_indices.as_ref().expect("skinned"),
            mesh.bone_weights.as_ref().expect("skinned"),
        );
        (0..n)
            .map(|i| {
                let (idx, w) = (bi[i], bw[i]);
                let mut j = [0u8; 4];
                for k in 0..4 {
                    if w[k] > 0 {
                        // Safe: weights only index real binding entries.
                        // The skeleton has at most 255 bones here (`convert_skeleton`).
                        j[k] = joint_map.get(usize::from(idx[k])).copied().unwrap_or(0) as u8;
                        if usize::from(idx[k]) >= joint_map.len() {
                            bad_weights += 1;
                        }
                    }
                }
                (j, quantize_weights(w.map(|x| f32::from(x) / 255.0)))
            })
            .unzip()
    } else {
        (Vec::new(), Vec::new())
    };
    if bad_weights > 0 {
        warnings.push(format!(
            "mesh {:?}: {bad_weights} weighted influences point outside the bone bindings",
            mesh.name
        ));
    }

    let ranges = group_ranges(mesh);
    let single = ranges.len() == 1;
    let mut out = Vec::new();
    for r in ranges {
        if r.indices.is_empty() {
            continue;
        }
        let material = materials.of_group(mesh, r.material).unwrap_or_else(|| {
            warnings.push(format!(
                "mesh {:?}: group material {} has no material binding, using material 0",
                mesh.name, r.material
            ));
            0
        });
        // Remap to the vertices this group uses, keeping source order.
        let (map, keep): (BTreeMap<u32, u32>, Vec<u32>) = if single {
            (
                (0..n as u32).map(|i| (i, i)).collect(),
                (0..n as u32).collect(),
            )
        } else {
            let mut used: Vec<u32> = r.indices.clone();
            used.sort_unstable();
            used.dedup();
            (
                used.iter()
                    .enumerate()
                    .map(|(k, &v)| (v, k as u32))
                    .collect(),
                used,
            )
        };
        let mut idx: Vec<u32> = r.indices.iter().map(|i| map[i]).collect();
        if flip {
            for t in idx.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
        let indices = if keep.len() <= usize::from(u16::MAX) + 1 {
            Indices::U16(idx.iter().map(|&i| i as u16).collect())
        } else {
            Indices::U32(idx)
        };
        out.push(Mesh {
            positions: pick(&positions, &keep),
            normals: pick(&normals, &keep),
            uvs: pick(&uvs, &keep),
            joints: if skinned {
                pick(&joints, &keep)
            } else {
                Vec::new()
            },
            weights: if skinned {
                pick(&weights, &keep)
            } else {
                Vec::new()
            },
            indices,
            material,
        });
    }
    Ok(out)
}

/// Skeleton and meshes of a scene. Each mesh group gets its engine material from `materials`.
///
/// Unit files have one model: its skeleton and meshes are converted, a non-identity
/// `InitialPlacement` is ignored with a warning. Files with several models (city building sets:
/// one model per building, each with a base quad and a building mesh) are converted whole: the
/// skeleton is the first skinned model's (else the first model's), every other model must be
/// rigid, and each rigid model is moved by its `InitialPlacement`, so the pieces keep the layout
/// of the source file.
pub fn convert_model(
    scene: &Scene,
    basis: &Basis,
    materials: &ModelMaterials,
) -> Result<ConvertedModel, ModelError> {
    let with_skeleton: Vec<(usize, usize)> = scene
        .models
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.skeleton.map(|s| (i, s)))
        .collect();
    let skinned = |i: usize| {
        scene.models[i]
            .meshes
            .iter()
            .any(|&mi| scene.meshes[mi].is_skinned())
    };
    let &(primary, skel_index) = with_skeleton
        .iter()
        .find(|(i, _)| skinned(*i))
        .or(with_skeleton.first())
        .ok_or_else(|| ModelError::invalid("gr2 has no model with a skeleton"))?;
    let skel = &scene.skeletons[skel_index];
    let mut warnings = Vec::new();
    let skeleton = convert_skeleton(skel, basis, &mut warnings)?;
    let several = scene.models.len() > 1;
    if let Some(other) = (0..scene.models.len()).find(|&i| i != primary && skinned(i)) {
        return Err(ModelError::invalid(format!(
            "models {primary} and {other} both have skinned meshes"
        )));
    }
    let order: Vec<usize> = if several {
        (0..scene.models.len()).collect()
    } else {
        vec![primary]
    };
    let mut meshes = Vec::new();
    for i in order {
        let model = &scene.models[i];
        let placement = Placement::new(&model.initial_placement);
        if placement.is_some() && (!several || skinned(i)) {
            warnings.push(format!(
                "model {i}: InitialPlacement is not identity (ignored)"
            ));
        }
        for &mi in &model.meshes {
            meshes.extend(convert_mesh(
                &scene.meshes[mi],
                skel,
                basis,
                materials,
                placement.as_ref().filter(|_| several),
                &mut warnings,
            )?);
        }
    }
    Ok(ConvertedModel {
        model: ModelBlob {
            skeleton,
            meshes,
            materials: materials.materials.clone(),
            animations: Vec::new(),
        },
        skeleton: skel_index,
        warnings,
    })
}

fn is_identity(t: &granny::Transform) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    let q = normalize_quat(t.orientation);
    t.position.iter().all(|&p| near(p, 0.0))
        && near(q[3].abs(), 1.0)
        && (0..3).all(|r| (0..3).all(|c| near(t.scale_shear[r][c], f32::from(u8::from(r == c)))))
}

#[derive(Debug)]
pub struct ConvertedAnimation {
    pub blob: AnimationBlob,
    pub warnings: Vec<String>,
}

fn sample_of(p: &Pose, basis: &Basis, shear: &Cell<bool>) -> Sample {
    let s = &p.scale_shear;
    let off = [s[1], s[2], s[3], s[5], s[6], s[7]];
    if off.iter().any(|x| x.abs() > SHEAR_WARN) {
        shear.set(true);
    }
    Sample {
        translation: basis.point(p.translation),
        rotation: basis.rotation(normalize_quat(p.rotation)),
        scale: basis.scale([s[0], s[4], s[8]]),
    }
}

/// Resamples every track of `anim` at 30 Hz onto the bones of `skel` (tracks bind by name ID).
/// `rest` is the engine-convention rest pose of the same skeleton.
pub fn convert_animation(
    anim: &Animation,
    skel: &GrannySkeleton,
    rest: &[Transform],
    basis: &Basis,
) -> Result<ConvertedAnimation, ModelError> {
    let mut warnings = Vec::new();
    let group = anim
        .track_groups
        .iter()
        .find(|g| !g.transform_tracks.is_empty())
        .ok_or_else(|| ModelError::invalid("animation has no transform tracks"))?;
    if anim
        .track_groups
        .iter()
        .filter(|g| !g.transform_tracks.is_empty())
        .count()
        > 1
    {
        warnings.push("several track groups; using the first".into());
    }
    let binding = bind(skel, group);
    if binding.unbound() > 0 {
        warnings.push(format!(
            "{} of {} tracks match no bone (dropped)",
            binding.unbound(),
            binding.track_to_bone.len()
        ));
    }
    // Tracks in bone order, so equal inputs give identical bytes.
    let mut by_bone: BTreeMap<usize, usize> = BTreeMap::new();
    for (t, b) in binding.track_to_bone.iter().enumerate() {
        if let Some(b) = b
            && by_bone.insert(*b, t).is_some()
        {
            warnings.push(format!("bone {b} has several tracks; the last one wins"));
        }
    }
    let duration = anim.duration;
    let shear = Cell::new(false);
    let mut tracks = Vec::new();
    for (&bone, &t) in &by_bone {
        let track = &group.transform_tracks[t];
        let bone_id = u16::try_from(bone)
            .map_err(|_| ModelError::invalid(format!("bone index {bone} does not fit u16")))?;
        let Some(rest) = rest.get(bone) else {
            continue;
        };
        if let Some(tr) = resample_track(bone_id, duration, SAMPLE_RATE, rest, |time| {
            sample_of(&track.sample(time), basis, &shear)
        }) {
            tracks.push(tr);
        }
    }
    if shear.get() {
        warnings.push("scale/shear off-diagonal terms present (dropped)".into());
    }
    Ok(ConvertedAnimation {
        blob: AnimationBlob {
            duration,
            sample_rate: SAMPLE_RATE,
            tracks,
        },
        warnings,
    })
}

/// Fraction of triangles whose winding agrees with the average of their vertex normals.
/// Near 1.0 means counter-clockwise seen from outside.
pub fn winding_agreement(mesh: &Mesh) -> f32 {
    let idx: Vec<usize> = match &mesh.indices {
        Indices::U16(v) => v.iter().map(|&i| usize::from(i)).collect(),
        Indices::U32(v) => v.iter().map(|&i| i as usize).collect(),
    };
    let (mut agree, mut total) = (0usize, 0usize);
    for t in idx.chunks_exact(3) {
        let p: Vec<[f32; 3]> = t.iter().map(|&i| mesh.positions[i]).collect();
        let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
        let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
        let face = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let mut avg = [0.0f32; 3];
        for &i in t {
            let n = asset_format::oct_decode(mesh.normals[i]);
            for k in 0..3 {
                avg[k] += n[k];
            }
        }
        let dot: f32 = (0..3).map(|k| face[k] * avg[k]).sum();
        if dot != 0.0 {
            total += 1;
            agree += usize::from(dot > 0.0);
        }
    }
    if total == 0 {
        1.0
    } else {
        agree as f32 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use granny::mesh::{BoneBinding, Group};
    use granny::skeleton::Bone as GBone;

    fn gtransform(position: [f32; 3]) -> granny::Transform {
        granny::Transform {
            flags: 0,
            position,
            orientation: [0.0, 0.0, 0.0, 1.0],
            scale_shear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    fn gbone(id: u32, parent: Option<usize>, z: f32) -> GBone {
        let mut inverse_world = [0.0f32; 16];
        for i in 0..4 {
            inverse_world[i * 5] = 1.0;
        }
        inverse_world[14] = -z;
        GBone {
            name: None,
            name_id: Some(id),
            parent,
            local: gtransform([0.0, 0.0, z]),
            inverse_world,
            lod_error: 0.0,
        }
    }

    /// Z-up quad at z = 2 facing +Z, skinned to two bones through a swapped binding table.
    fn scene(groups: Vec<Group>) -> Scene {
        let positions = vec![
            [-1.0, -1.0, 2.0],
            [1.0, -1.0, 2.0],
            [1.0, 1.0, 2.0],
            [-1.0, 1.0, 2.0],
        ];
        let binding = |bone| BoneBinding {
            name: None,
            name_id: None,
            bone,
        };
        let mesh = GrannyMesh {
            positions,
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
            uv: vec![vec![[0.0, 0.25], [1.0, 0.25], [1.0, 1.0], [0.0, 1.0]]],
            bone_indices: Some(vec![[0, 1, 0, 0]; 4]),
            bone_weights: Some(vec![[128, 127, 0, 0]; 4]),
            indices: vec![0, 1, 2, 0, 2, 3],
            groups,
            bone_bindings: vec![binding(Some(1)), binding(Some(0))],
            ..GrannyMesh::default()
        };
        Scene {
            skeletons: vec![GrannySkeleton {
                name: None,
                bones: vec![gbone(10, None, 0.0), gbone(11, Some(0), 1.0)],
            }],
            meshes: vec![mesh],
            models: vec![granny::Model {
                name: None,
                skeleton: Some(0),
                initial_placement: gtransform([0.0; 3]),
                meshes: vec![0],
            }],
            animations: Vec::new(),
            materials: Vec::new(),
        }
    }

    fn one() -> ModelMaterials {
        ModelMaterials::single(Material::default())
    }

    fn all_group() -> Vec<Group> {
        vec![Group {
            material_index: 0,
            tri_first: 0,
            tri_count: 2,
        }]
    }

    #[test]
    fn converts_a_skinned_mesh_into_engine_axes() {
        let c = convert_model(&scene(all_group()), &Basis::CIV5_TO_ENGINE, &one()).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        let m = &c.model;
        assert_eq!(m.skeleton.bones.len(), 2);
        assert_eq!(m.skeleton.bones[1].parent, Some(0));
        // Source Z (up) becomes engine Y.
        assert_eq!(m.skeleton.bones[1].rest.translation, [0.0, 1.0, 0.0]);
        assert_eq!(m.skeleton.bones[1].inverse_bind[13], -1.0);
        let mesh = &m.meshes[0];
        assert_eq!(mesh.positions[0], [-1.0, 2.0, 1.0]);
        let n = asset_format::oct_decode(mesh.normals[0]);
        assert!((n[1] - 1.0).abs() < 1e-3, "{n:?}");
        // Binding table order is swapped: vertex slot 0 -> binding 0 -> bone 1.
        assert_eq!(mesh.joints[0], [1, 0, 0, 0]);
        assert_eq!(mesh.weights[0], [128, 127, 0, 0]);
        // V is flipped for the top-left texture origin.
        assert_eq!(
            asset_format::f16_bits_to_f32(mesh.uvs[0][1]),
            0.75,
            "v = 1 - 0.25"
        );
        assert!(winding_agreement(mesh) > 0.99);
        assert!(ModelBlob::decode(&m.encode()).is_ok());
    }

    #[test]
    fn mirrored_basis_reverses_winding_to_stay_outward() {
        let mirror = Basis::new([[-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).unwrap();
        let c = convert_model(&scene(all_group()), &mirror, &one()).unwrap();
        let mesh = &c.model.meshes[0];
        assert!(winding_agreement(mesh) > 0.99);
        // The same data with a proper rotation keeps source order.
        let p = convert_model(&scene(all_group()), &Basis::CIV5_TO_ENGINE, &one()).unwrap();
        let (Indices::U16(a), Indices::U16(b)) = (&mesh.indices, &p.model.meshes[0].indices) else {
            panic!("expected u16 indices");
        };
        assert_eq!(&a[..3], &[b[0], b[2], b[1]]);
    }

    #[test]
    fn groups_become_meshes_with_compacted_vertices() {
        let groups = vec![
            Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 1,
            },
            Group {
                material_index: 1,
                tri_first: 1,
                tri_count: 1,
            },
        ];
        let c = convert_model(&scene(groups), &Basis::CIV5_TO_ENGINE, &one()).unwrap();
        assert_eq!(c.model.meshes.len(), 2);
        for mesh in &c.model.meshes {
            assert_eq!(mesh.positions.len(), 3);
            assert_eq!(mesh.indices.len(), 3);
            assert_eq!(mesh.material, 0);
        }
        // Second group uses source vertices 0, 2, 3.
        assert_eq!(c.model.meshes[1].positions[1], [1.0, 2.0, -1.0]);
    }

    #[test]
    fn bone_binding_without_a_bone_is_an_error() {
        let mut s = scene(all_group());
        s.meshes[0].bone_bindings[0].bone = None;
        assert!(convert_model(&s, &Basis::CIV5_TO_ENGINE, &one()).is_err());
    }

    #[test]
    fn parent_after_child_is_an_error() {
        let mut s = scene(all_group());
        s.skeletons[0].bones[0].parent = Some(1);
        assert!(convert_model(&s, &Basis::CIV5_TO_ENGINE, &one()).is_err());
    }

    fn mat(diffuse: &str) -> Material {
        Material {
            diffuse: Some(asset_format::LogicalId::new(diffuse)),
            specular: None,
        }
    }

    fn two_groups() -> Vec<Group> {
        vec![
            Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 1,
            },
            Group {
                material_index: 1,
                tri_first: 1,
                tri_count: 1,
            },
        ]
    }

    #[test]
    fn groups_reach_their_material_through_the_mesh_bindings() {
        let mut s = scene(two_groups());
        // Group 0 -> binding 0 -> Granny material 2; group 1 -> binding 1 -> Granny material 0.
        s.meshes[0].material_bindings = vec![Some(2), Some(0)];
        let materials = ModelMaterials {
            materials: vec![mat("tex:a"), mat("tex:b")],
            by_granny: vec![1, 1, 0],
        };
        let c = convert_model(&s, &Basis::CIV5_TO_ENGINE, &materials).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        let used: Vec<u16> = c.model.meshes.iter().map(|m| m.material).collect();
        assert_eq!(used, [0, 1]);
        assert_eq!(c.model.materials, materials.materials);
        assert!(ModelBlob::decode(&c.model.encode()).is_ok());
    }

    #[test]
    fn group_without_a_binding_falls_back_to_material_0_with_a_warning() {
        let mut s = scene(two_groups());
        s.meshes[0].material_bindings = vec![Some(1)];
        let materials = ModelMaterials {
            materials: vec![mat("tex:a"), mat("tex:b")],
            by_granny: vec![0, 1],
        };
        let c = convert_model(&s, &Basis::CIV5_TO_ENGINE, &materials).unwrap();
        let used: Vec<u16> = c.model.meshes.iter().map(|m| m.material).collect();
        assert_eq!(used, [1, 0]);
        assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
        assert!(c.warnings[0].contains("no material binding"));
    }

    /// Two rigid one-triangle models, like the pieces of a city building set.
    fn rigid_set() -> Scene {
        let tri = |z: f32| GrannyMesh {
            positions: vec![[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]],
            normals: Some(vec![[1.0, 0.0, 0.0]; 3]),
            uv: vec![vec![[0.0, 0.0]; 3]],
            indices: vec![0, 1, 2],
            groups: vec![Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 1,
            }],
            material_bindings: vec![Some(0)],
            ..GrannyMesh::default()
        };
        let skel = || GrannySkeleton {
            name: None,
            bones: vec![gbone(1, None, 0.0)],
        };
        let mut moved = gtransform([100.0, 0.0, 0.0]);
        // 90 degrees about Z: source +X becomes +Y.
        let h = std::f32::consts::FRAC_1_SQRT_2;
        moved.orientation = [0.0, 0.0, h, h];
        Scene {
            skeletons: vec![skel(), skel()],
            meshes: vec![tri(0.0), tri(5.0)],
            models: vec![
                granny::Model {
                    name: None,
                    skeleton: Some(0),
                    initial_placement: gtransform([0.0; 3]),
                    meshes: vec![0],
                },
                granny::Model {
                    name: None,
                    skeleton: Some(1),
                    initial_placement: moved,
                    meshes: vec![1],
                },
            ],
            animations: Vec::new(),
            materials: Vec::new(),
        }
    }

    #[test]
    fn every_rigid_model_is_converted_at_its_initial_placement() {
        let c = convert_model(&rigid_set(), &Basis::CIV5_TO_ENGINE, &one()).unwrap();
        assert!(c.warnings.is_empty(), "{:?}", c.warnings);
        assert_eq!(c.model.meshes.len(), 2);
        assert!(c.model.meshes.iter().all(|m| m.joints.is_empty()));
        // Model 0 stays put: source (1, 0, 0) -> engine (1, 0, 0).
        assert_eq!(c.model.meshes[0].positions[1], [1.0, 0.0, 0.0]);
        // Model 1: rotated to source (0, 1, 5), moved by 100 on X -> engine (100, 5, -1).
        let p = c.model.meshes[1].positions[1];
        for (a, b) in p.iter().zip([100.0, 5.0, -1.0]) {
            assert!((a - b).abs() < 1e-4, "{p:?}");
        }
        // The normal turns with it: source +X -> +Y -> engine -Z.
        let n = asset_format::oct_decode(c.model.meshes[1].normals[0]);
        assert!((n[2] + 1.0).abs() < 1e-3, "{n:?}");
    }

    #[test]
    fn skinned_meshes_in_two_models_are_an_error() {
        let mut s = scene(all_group());
        s.models.push(s.models[0].clone());
        assert!(convert_model(&s, &Basis::CIV5_TO_ENGINE, &one()).is_err());
    }

    /// Bindings as the reader makes them: `bone` is set only for a unique name ID.
    fn binding_mesh(ids: &[u32], skel: &GrannySkeleton) -> GrannyMesh {
        GrannyMesh {
            bone_bindings: ids
                .iter()
                .map(|&id| BoneBinding {
                    name: None,
                    name_id: Some(id),
                    bone: skel.find_bone_by_id(id),
                })
                .collect(),
            ..GrannyMesh::default()
        }
    }

    fn skeleton_of(bones: Vec<GBone>) -> GrannySkeleton {
        GrannySkeleton { name: None, bones }
    }

    #[test]
    fn a_binding_to_two_identical_bones_uses_the_first_and_warns() {
        // Bones 1 and 2 share name id 7 and every transform (Great Artist Flute).
        let skel = skeleton_of(vec![
            gbone(5, None, 0.0),
            gbone(7, Some(0), 1.0),
            gbone(7, Some(0), 1.0),
        ]);
        let mut warnings = Vec::new();
        let joints = joint_table(&binding_mesh(&[5, 7], &skel), &skel, &mut warnings).unwrap();
        assert_eq!(joints, [0, 1]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("[1, 2]"), "{warnings:?}");
    }

    #[test]
    fn a_binding_to_bones_that_differ_or_to_no_bone_is_an_error() {
        let differ = skeleton_of(vec![
            gbone(5, None, 0.0),
            gbone(7, Some(0), 1.0),
            gbone(7, Some(0), 2.0),
        ]);
        let mut w = Vec::new();
        assert!(joint_table(&binding_mesh(&[7], &differ), &differ, &mut w).is_err());
        let same = skeleton_of(vec![gbone(5, None, 0.0)]);
        assert!(joint_table(&binding_mesh(&[99], &same), &same, &mut w).is_err());
        assert!(w.is_empty());
    }
}
