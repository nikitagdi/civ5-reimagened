//! Granny scene to a `C5MD` v2 model.
//!
//! Unlike the B1 converter ([`super::convert`]) this keeps what a B2 model needs:
//! - every mesh, with the node states of the gr2 (`granny::mesh::state_mask`: a ship's intact and
//!   damaged hulls), nothing dropped;
//! - one [`MaterialDesc`] per material, taken from the FGX blocks ([`super::material_desc`]);
//! - parts: one per gr2 `Model`, with its name and `InitialPlacement`;
//! - rigid meshes: a mesh on an animated bone keeps `bind`, one on a bone no clip moves is baked
//!   into the model with the rest skin matrix;
//! - packed little-endian vertex rows (position f32x3, normal oct16, UV f16, joints u8x4 or u16x4,
//!   weights unorm8), meshes of one material merged ([`super::merge`]).
//!
//! # Several skeletons (catalogs)
//! A landmark, city or wonder gr2 holds many `Model`s, each with a skeleton of its own (one
//! herd member, one building, one variant). With [`BakeInput::multi`] the skeletons of the models
//! are joined into one: the bones of the model `k` follow those of the models before it, a bone
//! keeps its parent shifted by the same amount, and a mesh's joints and rigid bone are shifted
//! likewise, so a clip names bones by their number in the one skeleton ([`skeleton_map`]).
//!
//! Axes, winding and texture-V flip are those of B1 (see [`super::Basis`]); joints are numbers of
//! skeleton bones (the `BoneBindings` table of the Granny mesh is resolved here).
//!
//! SIMPLIFIED(SIMP-030): normals oct16 and UVs f16 (f32 from [`asset_format::UV_F16_LIMIT`]
//! up), tangents are not kept.

use std::collections::BTreeSet;

use asset_format::{
    MaterialDesc, MeshArrays, MeshV2, ModelBlobV2, Part, STATE_INTACT, STATES_ALL, Skeleton,
    Transform, quantize_weights,
};
use glam::{Mat4, Vec3};
use granny::Scene;
use granny::skeleton::{Skeleton as GrannySkeleton, trs};

use super::ModelError;
use super::basis::Basis;
use super::convert::{
    SOURCE_WINDING_CCW, UV_FLIP_V, convert_skeleton_any, group_ranges, joint_table_with,
    normalize_quat,
};
use super::merge::{MergeScope, RawMesh, merge_meshes};

/// What [`bake_model`] reads.
pub struct BakeInput<'a> {
    pub scene: &'a Scene,
    /// `.fxsxml` name without extension (`ModelBlobV2::source`).
    pub source: &'a str,
    /// The material of every Granny material of the file; `None` where it has no FGX block.
    pub descs: &'a [Option<MaterialDesc>],
    /// Bones a clip of the model moves: rigid meshes on them keep `bind`.
    pub animated_bones: &'a BTreeSet<u16>,
    pub scope: MergeScope,
    pub basis: Basis,
    /// Join the skeletons of all models (catalogs); else the one skeleton of the unit model is
    /// used for every mesh.
    pub multi: bool,
}

/// What the bake counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BakeStats {
    /// Meshes (material groups of the Granny meshes) before the merge, and after.
    pub meshes_before: usize,
    pub meshes_after: usize,
    pub triangles: usize,
    /// Rigid meshes kept on a bone (`bind`), and baked into the model.
    pub bound_meshes: usize,
    pub baked_rigid: usize,
    /// Merged meshes with f32 UVs.
    pub uv_f32_meshes: usize,
    /// Parts whose `InitialPlacement` is not the identity.
    pub placed_parts: usize,
    /// Meshes by their node states ([`state_class`]): every state, the intact hull, a damaged hull,
    /// other masks.
    pub damage_meshes: [usize; 4],
}

#[derive(Debug)]
pub struct BakedModel {
    pub model: ModelBlobV2,
    pub warnings: Vec<String>,
    pub stats: BakeStats,
}

fn near_identity(m: &Mat4) -> bool {
    m.abs_diff_eq(Mat4::IDENTITY, 1e-4)
}

fn is_identity_transform(t: &Transform) -> bool {
    let n = |a: f32, b: f32| (a - b).abs() < 1e-4;
    t.translation.iter().all(|&c| n(c, 0.0))
        && t.scale.iter().all(|&c| n(c, 1.0))
        && n(t.rotation[3].abs(), 1.0)
}

/// The class of a node state mask for the counts: 0 every state, 1 the intact state among others
/// (a ship's intact hull), 2 a damaged hull (not intact, light damage), 3 any other mask.
pub fn state_class(states: u8) -> usize {
    if states == STATES_ALL {
        0
    } else if states & STATE_INTACT != 0 {
        1
    } else if states & 0b10 != 0 {
        2
    } else {
        3
    }
}

/// Meshes of one gr2 mesh object, one per material group, as raw arrays.
struct GroupMesh {
    mesh: RawMesh,
    /// Granny material index of the group.
    granny_material: usize,
    /// Triangle count (for the stats).
    triangles: usize,
}

#[allow(clippy::too_many_arguments)]
fn convert_mesh_groups(
    mesh: &granny::mesh::Mesh,
    mesh_index: usize,
    part: u16,
    skeleton: &GrannySkeleton,
    bone_offset: u16,
    basis: &Basis,
    out_skeleton: &Skeleton,
    animated: &BTreeSet<u16>,
    placement: &Transform,
    stats: &mut BakeStats,
    warnings: &mut Vec<String>,
) -> Result<Vec<GroupMesh>, ModelError> {
    if mesh.positions.is_empty() || mesh.indices.is_empty() {
        return Ok(Vec::new());
    }
    let label = mesh
        .name
        .clone()
        .unwrap_or_else(|| format!("mesh{mesh_index}"));
    let n = mesh.positions.len();
    let skinned = mesh.is_skinned();
    let joint_map = if skinned {
        let mut t = joint_table_with(mesh, skeleton, warnings, true)?;
        for j in &mut t {
            *j = j
                .checked_add(bone_offset)
                .ok_or_else(|| ModelError::invalid("a joint number does not fit u16"))?;
        }
        t
    } else {
        Vec::new()
    };
    if mesh.normals.is_none() {
        warnings.push(format!("mesh {label:?}: no normals, using +Y"));
    }
    let uv0 = mesh.uv.first();
    if uv0.is_none() {
        warnings.push(format!("mesh {label:?}: no texture coordinates"));
    }

    // A rigid mesh follows its bone: bound on an animated one, baked on a still one.
    let rigid_bone = if skinned {
        None
    } else {
        match mesh.bone_bindings.as_slice() {
            [] => None,
            // A model with no skeleton bone keeps its mesh where it is.
            [_] if skeleton.bones.is_empty() => {
                warnings.push(format!(
                    "mesh {label:?}: bound to a bone but the skeleton has none, placed unbound"
                ));
                None
            }
            [b] => {
                let bone = b
                    .bone
                    .and_then(|k| u16::try_from(k).ok())
                    .filter(|&k| usize::from(k) < skeleton.bones.len())
                    .map(|k| k + bone_offset)
                    .filter(|&k| usize::from(k) < out_skeleton.bones.len());
                if bone.is_none() {
                    // The exporter bound the mesh to a helper bone that is not in the
                    // skeleton; the mesh keeps the place its vertices give it.
                    warnings.push(format!(
                        "mesh {label:?}: its bone binding matches no skeleton bone, placed unbound"
                    ));
                }
                bone
            }
            many => {
                return Err(ModelError::invalid(format!(
                    "mesh {label:?}: a rigid mesh with {} bone bindings",
                    many.len()
                )));
            }
        }
    };
    let (bind, rest_skin) = match rigid_bone {
        Some(b) if animated.contains(&b) => (Some(b), None),
        Some(b) => {
            let rest = out_skeleton.rest_pose();
            let mut world = vec![Mat4::IDENTITY; rest.len()];
            out_skeleton.world_matrices(&rest, &mut world);
            let mut skin = vec![Mat4::IDENTITY; rest.len()];
            out_skeleton.skin_matrices(&world, &mut skin);
            let m = skin[usize::from(b)];
            // The exporter of a pile of rigid objects (a deposit of stone, an orchard) gives each its
            // own model with the place of the object as `InitialPlacement`, its vertices around the
            // origin, and a bone whose `InverseWorld` is the inverse of that place: the skin of the
            // still bone then undoes the placement and every object stands at the origin. The
            // game puts the objects at their places (golden S1a), so a skin that only cancels the
            // placement is not applied.
            let cancels =
                !is_identity_transform(placement) && near_identity(&(placement.to_mat4() * m));
            (None, (!near_identity(&m) && !cancels).then_some(m))
        }
        None => (None, None),
    };
    if bind.is_some() {
        stats.bound_meshes += 1;
    } else if rigid_bone.is_some() {
        stats.baked_rigid += 1;
    }

    let mut positions: Vec<[f32; 3]> = mesh.positions.iter().map(|&p| basis.point(p)).collect();
    let mut normals: Vec<[f32; 3]> = (0..n)
        .map(|i| {
            mesh.normals
                .as_ref()
                .and_then(|v| v.get(i))
                .map_or([0.0, 1.0, 0.0], |&nrm| basis.point(nrm))
        })
        .collect();
    if let Some(m) = rest_skin {
        for p in &mut positions {
            *p = m.transform_point3(Vec3::from_array(*p)).to_array();
        }
        for nrm in &mut normals {
            *nrm = m
                .transform_vector3(Vec3::from_array(*nrm))
                .normalize_or_zero()
                .to_array();
        }
    }
    let uvs: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let [u, v] = uv0.and_then(|t| t.get(i)).copied().unwrap_or([0.0, 0.0]);
            [u, if UV_FLIP_V { 1.0 - v } else { v }]
        })
        .collect();
    let mut bad_weights = 0usize;
    let (joints, weights): (Vec<[u16; 4]>, Vec<[u8; 4]>) = if skinned {
        let (bi, bw) = (
            mesh.bone_indices.as_ref().expect("skinned"),
            mesh.bone_weights.as_ref().expect("skinned"),
        );
        (0..n)
            .map(|i| {
                let (idx, w) = (bi[i], bw[i]);
                let mut j = [0u16; 4];
                for k in 0..4 {
                    if w[k] > 0 {
                        j[k] = joint_map.get(usize::from(idx[k])).copied().unwrap_or(0);
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
            "mesh {label:?}: {bad_weights} weighted influences point outside the bone bindings"
        ));
    }

    let flip = basis.flips_handedness() ^ !SOURCE_WINDING_CCW;
    let ranges = group_ranges(mesh);
    let several = ranges.len() > 1;
    let mut out = Vec::new();
    for (gi, r) in ranges.into_iter().enumerate() {
        if r.indices.is_empty() {
            continue;
        }
        let granny_material = mesh
            .material_bindings
            .get(r.material as usize)
            .copied()
            .flatten()
            .ok_or_else(|| {
                ModelError::invalid(format!(
                    "mesh {label:?}: group material {} has no material binding",
                    r.material
                ))
            })?;
        // Only the vertices this group uses, in source order.
        let mut used: Vec<u32> = r.indices.clone();
        used.sort_unstable();
        used.dedup();
        let mut remap = vec![u32::MAX; n];
        for (k, &v) in used.iter().enumerate() {
            remap[v as usize] = k as u32;
        }
        let mut indices: Vec<u32> = r.indices.iter().map(|&i| remap[i as usize]).collect();
        if flip {
            for t in indices.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
        let pick = |v: &[[f32; 3]]| used.iter().map(|&i| v[i as usize]).collect::<Vec<_>>();
        let arrays = MeshArrays {
            positions: pick(&positions),
            normals: pick(&normals),
            uvs: used.iter().map(|&i| uvs[i as usize]).collect(),
            joints: if skinned {
                used.iter().map(|&i| joints[i as usize]).collect()
            } else {
                Vec::new()
            },
            weights: if skinned {
                used.iter().map(|&i| weights[i as usize]).collect()
            } else {
                Vec::new()
            },
            indices,
        };
        let triangles = arrays.indices.len() / 3;
        out.push(GroupMesh {
            mesh: RawMesh {
                name: if several {
                    format!("{label}.{gi}")
                } else {
                    label.clone()
                },
                part,
                material: 0,
                arrays,
                bind,
                states: mesh.state_mask,
            },
            granny_material,
            triangles,
        });
    }
    Ok(out)
}

/// The skeletons of a catalog gr2 joined into one ([`BakeInput::multi`]).
#[derive(Debug, Clone, Default)]
pub struct SkeletonMap {
    pub skeleton: Skeleton,
    /// Per Granny skeleton of the scene: the number of its first bone in `skeleton`, `None` when
    /// no model uses it.
    pub offsets: Vec<Option<u16>>,
}

/// Joins the skeletons that models use, in model order (a skeleton two models share is added once).
pub fn skeleton_map(
    scene: &Scene,
    basis: &Basis,
    warnings: &mut Vec<String>,
) -> Result<SkeletonMap, ModelError> {
    let mut out = SkeletonMap {
        skeleton: Skeleton::default(),
        offsets: vec![None; scene.skeletons.len()],
    };
    for model in &scene.models {
        let Some(si) = model.skeleton else { continue };
        if out.offsets[si].is_some() {
            continue;
        }
        let offset = u16::try_from(out.skeleton.bones.len())
            .map_err(|_| ModelError::invalid("more than 65535 bones in the joined skeletons"))?;
        let one = convert_skeleton_any(&scene.skeletons[si], basis, warnings)?;
        for mut b in one.bones {
            b.parent = b.parent.map(|p| p + offset);
            out.skeleton.bones.push(b);
        }
        out.offsets[si] = Some(offset);
    }
    if out.skeleton.bones.len() > usize::from(u16::MAX) {
        return Err(ModelError::invalid(
            "more than 65535 bones in the joined skeletons",
        ));
    }
    Ok(out)
}

/// A part's placement applied to its meshes (the model scope has one part, so the placement is
/// baked into the vertices). A skinned mesh cannot take it.
fn bake_placement(groups: &mut [GroupMesh], placement: &Transform) -> Result<(), ModelError> {
    let m = placement.to_mat4();
    let normal = glam::Mat3::from_mat4(m).inverse().transpose();
    let flip = m.determinant() < 0.0;
    for g in groups {
        let a = &mut g.mesh.arrays;
        if !a.joints.is_empty() {
            return Err(ModelError::invalid(format!(
                "mesh {:?}: a skinned mesh of a part with a placement cannot merge into the whole model",
                g.mesh.name
            )));
        }
        for p in &mut a.positions {
            *p = m.transform_point3(Vec3::from_array(*p)).to_array();
        }
        for n in &mut a.normals {
            *n = (normal * Vec3::from_array(*n))
                .normalize_or_zero()
                .to_array();
        }
        if flip {
            for t in a.indices.chunks_exact_mut(3) {
                t.swap(1, 2);
            }
        }
    }
    Ok(())
}

/// The model of the gr2 that carries the skeleton (the one with skinned meshes if any) and the
/// index of its skeleton.
pub fn primary_skeleton(scene: &Scene) -> Result<(usize, usize), ModelError> {
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
    if let Some(other) = (0..scene.models.len()).find(|&i| i != primary && skinned(i)) {
        return Err(ModelError::invalid(format!(
            "models {primary} and {other} both have skinned meshes"
        )));
    }
    Ok((primary, skel_index))
}

/// Builds the v2 model of `input.scene`. Errors when a mesh has no material, a material has no
/// FGX block, or the skeleton and meshes do not fit (no stand-ins, AB-D14).
pub fn bake_model(input: &BakeInput<'_>) -> Result<BakedModel, ModelError> {
    let scene = input.scene;
    let basis = &input.basis;
    let mut warnings = Vec::new();
    // The skeleton of the blob and, per model, the Granny skeleton its meshes bind to and the
    // number of its first bone in the blob's skeleton.
    let empty = GrannySkeleton {
        name: None,
        bones: Vec::new(),
    };
    let (skeleton, binds): (Skeleton, Vec<(&GrannySkeleton, u16)>) = if input.multi {
        let map = skeleton_map(scene, basis, &mut warnings)?;
        let binds = scene
            .models
            .iter()
            .map(|m| match m.skeleton {
                Some(si) => (&scene.skeletons[si], map.offsets[si].unwrap_or(0)),
                None => (&empty, 0),
            })
            .collect();
        (map.skeleton, binds)
    } else {
        let (_, skel_index) = primary_skeleton(scene)?;
        let skel = &scene.skeletons[skel_index];
        let skeleton = convert_skeleton_any(skel, basis, &mut warnings)?;
        (skeleton, vec![(skel, 0); scene.models.len()])
    };
    if skeleton.bones.is_empty() {
        return Err(ModelError::invalid("gr2 has no model with a skeleton"));
    }
    let wide_joints = skeleton.bones.len() > 256;

    let mut stats = BakeStats::default();
    let mut parts: Vec<Part> = Vec::new();
    let mut groups: Vec<GroupMesh> = Vec::new();
    for (i, model) in scene.models.iter().enumerate() {
        let part = u16::try_from(i)
            .map_err(|_| ModelError::invalid("a gr2 with more than 65535 models"))?;
        let t = trs(&model.initial_placement);
        let placement = basis.transform(&Transform {
            translation: t.translation,
            rotation: normalize_quat(t.rotation),
            scale: t.scale,
        });
        if !is_identity_transform(&placement) {
            stats.placed_parts += 1;
        }
        parts.push(Part {
            name: model.name.clone().unwrap_or_else(|| format!("model{i}")),
            first_mesh: 0,
            mesh_count: 0,
            placement,
        });
        let first_group = groups.len();
        let (bind_skel, bone_offset) = binds[i];
        for &mi in &model.meshes {
            groups.extend(convert_mesh_groups(
                &scene.meshes[mi],
                mi,
                part,
                bind_skel,
                bone_offset,
                basis,
                &skeleton,
                input.animated_bones,
                &placement,
                &mut stats,
                &mut warnings,
            )?);
        }
        // The model scope has one part: the placement goes into the vertices.
        if input.scope == MergeScope::Model && !is_identity_transform(&placement) {
            bake_placement(&mut groups[first_group..], &placement)?;
        }
    }
    if parts.is_empty() {
        return Err(ModelError::invalid("gr2 has no models"));
    }
    stats.meshes_before = groups.len();
    stats.triangles = groups.iter().map(|g| g.triangles).sum();

    // Materials in first-use order; Granny materials with the same description join.
    let mut materials: Vec<MaterialDesc> = Vec::new();
    let mut by_granny: Vec<(usize, u16)> = Vec::new();
    for g in &mut groups {
        let final_index = match by_granny.iter().find(|(k, _)| *k == g.granny_material) {
            Some((_, f)) => *f,
            None => {
                let desc = input
                    .descs
                    .get(g.granny_material)
                    .and_then(Option::as_ref)
                    .ok_or_else(|| {
                        ModelError::invalid(format!(
                            "mesh {:?}: material {} has no FGX block",
                            g.mesh.name, g.granny_material
                        ))
                    })?;
                let same = |m: &MaterialDesc| {
                    let mut a = m.clone();
                    let mut b = desc.clone();
                    a.source_name = None;
                    b.source_name = None;
                    a == b
                };
                let f = match materials.iter().position(same) {
                    Some(f) => f as u16,
                    None => {
                        materials.push(desc.clone());
                        u16::try_from(materials.len() - 1)
                            .map_err(|_| ModelError::invalid("more than 65535 materials"))?
                    }
                };
                by_granny.push((g.granny_material, f));
                f
            }
        };
        g.mesh.material = final_index;
    }
    let raws: Vec<RawMesh> = groups.into_iter().map(|g| g.mesh).collect();
    let merged = merge_meshes(raws, input.scope);
    stats.meshes_after = merged.len();

    let mut meshes: Vec<MeshV2> = Vec::with_capacity(merged.len());
    for m in &merged {
        let uv_f32 = MeshV2::uvs_need_f32(&m.arrays.uvs);
        stats.uv_f32_meshes += usize::from(uv_f32);
        stats.damage_meshes[state_class(m.states)] += 1;
        let mut mesh = MeshV2::pack(
            m.name.clone(),
            m.part,
            m.material,
            &m.arrays,
            wide_joints,
            uv_f32,
        )
        .map_err(|e| ModelError::invalid(format!("mesh {:?}: {e}", m.name)))?;
        mesh.bind = m.bind;
        mesh.states = m.states;
        meshes.push(mesh);
    }

    // Parts: the merged meshes are in part order; under the model scope there is one part.
    if input.scope == MergeScope::Model {
        parts = vec![Part {
            name: input.source.to_owned(),
            first_mesh: 0,
            mesh_count: u16::try_from(meshes.len())
                .map_err(|_| ModelError::invalid("more than 65535 meshes"))?,
            placement: Transform::IDENTITY,
        }];
    } else {
        for p in &mut parts {
            p.mesh_count = 0;
        }
        for (i, m) in meshes.iter().enumerate() {
            let p = &mut parts[usize::from(m.part)];
            if p.mesh_count == 0 {
                p.first_mesh =
                    u16::try_from(i).map_err(|_| ModelError::invalid("more than 65535 meshes"))?;
            }
            p.mesh_count += 1;
        }
    }

    let mut model = ModelBlobV2 {
        source: input.source.to_owned(),
        skeleton,
        parts,
        meshes,
        materials,
        ..ModelBlobV2::default()
    };
    model.bounds = model.compute_bounds();
    model
        .validate()
        .map_err(|e| ModelError::invalid(format!("{}: {e}", input.source)))?;
    Ok(BakedModel {
        model,
        warnings,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{
        LogicalId, ParamSet, STATE_INTACT, STATES_ALL, TexKind, TexParam, TexSource,
    };
    use granny::mesh::{BoneBinding, Group, Mesh as GrannyMesh};
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

    fn desc(diffuse: &str) -> MaterialDesc {
        MaterialDesc {
            package: "UnitShader_Skinned".into(),
            sets: vec![ParamSet {
                blocks: vec![],
                textures: vec![TexParam {
                    name: "BaseTextureMap".into(),
                    kind: TexKind::Tex2D,
                    src: TexSource::File(LogicalId::new(format!("tx:{diffuse}"))),
                    semantic: 0,
                    reg: 0,
                    usage: 0,
                    flags: 0,
                    active: true,
                }],
                constants: vec![],
            }],
            ..MaterialDesc::default()
        }
    }

    /// A Z-up ship: a skinned hull (two groups: intact and damaged hull), a skinned flag and a
    /// rigid prop on bone 1. Bones: 0 root, 1 at z = 1.
    fn ship() -> Scene {
        let quad = |x: f32| {
            vec![
                [x, -1.0, 2.0],
                [x + 1.0, -1.0, 2.0],
                [x + 1.0, 1.0, 2.0],
                [x, 1.0, 2.0],
            ]
        };
        let binding = |bone| BoneBinding {
            name: None,
            name_id: None,
            bone,
        };
        let mut hull_pos = quad(0.0);
        hull_pos.extend(quad(0.0)); // the damaged hull overlaps the intact one
        let hull = GrannyMesh {
            name: Some("hull".into()),
            positions: hull_pos,
            normals: Some(vec![[0.0, 0.0, 1.0]; 8]),
            uv: vec![vec![[0.0, 0.25]; 8]],
            bone_indices: Some(vec![[0, 0, 0, 0]; 8]),
            bone_weights: Some(vec![[255, 0, 0, 0]; 8]),
            indices: vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7],
            groups: vec![
                Group {
                    material_index: 0,
                    tri_first: 0,
                    tri_count: 2,
                },
                Group {
                    material_index: 1,
                    tri_first: 2,
                    tri_count: 2,
                },
            ],
            material_bindings: vec![Some(0), Some(1)],
            bone_bindings: vec![binding(Some(1))],
            ..GrannyMesh::default()
        };
        let flag = GrannyMesh {
            name: Some("flag".into()),
            positions: quad(3.0),
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
            uv: vec![vec![[0.0, 3.5], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]],
            bone_indices: Some(vec![[0, 0, 0, 0]; 4]),
            bone_weights: Some(vec![[255, 0, 0, 0]; 4]),
            indices: vec![0, 1, 2, 0, 2, 3],
            groups: vec![Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 2,
            }],
            material_bindings: vec![Some(2)],
            bone_bindings: vec![binding(Some(0))],
            ..GrannyMesh::default()
        };
        let prop = GrannyMesh {
            name: Some("prop".into()),
            positions: quad(6.0),
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
            uv: vec![vec![[0.0, 0.0]; 4]],
            indices: vec![0, 1, 2, 0, 2, 3],
            groups: vec![Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 2,
            }],
            material_bindings: vec![Some(2)],
            bone_bindings: vec![binding(Some(1))],
            ..GrannyMesh::default()
        };
        Scene {
            skeletons: vec![GrannySkeleton {
                name: None,
                bones: vec![gbone(10, None, 0.0), gbone(11, Some(0), 1.0)],
            }],
            meshes: vec![hull, flag, prop],
            models: vec![granny::Model {
                name: Some("Ship".into()),
                skeleton: Some(0),
                initial_placement: gtransform([0.0; 3]),
                meshes: vec![0, 1, 2],
            }],
            animations: Vec::new(),
            materials: Vec::new(),
        }
    }

    fn descs() -> Vec<Option<MaterialDesc>> {
        vec![
            Some(desc("ship_diff.dds")),
            Some(desc("ship_dmg_diff.dds")),
            Some(desc("flag_diff.dds")),
        ]
    }

    fn bake(animated: &[u16], scope: MergeScope) -> BakedModel {
        let scene = ship();
        let animated: BTreeSet<u16> = animated.iter().copied().collect();
        let descs = descs();
        bake_model(&BakeInput {
            scene: &scene,
            source: "ship",
            descs: &descs,
            animated_bones: &animated,
            scope,
            basis: Basis::CIV5_TO_ENGINE,
            multi: false,
        })
        .unwrap()
    }

    #[test]
    fn keeps_every_mesh_with_the_node_states_of_the_gr2() {
        let b = bake(&[], MergeScope::Part);
        assert!(b.warnings.is_empty(), "{:?}", b.warnings);
        // 4 groups (hull x2, flag, prop). The prop is rigid (no joints), so it does not merge
        // with the skinned flag although the material is the same.
        assert_eq!(b.stats.meshes_before, 4);
        assert_eq!(b.stats.meshes_after, 4);
        assert_eq!(b.stats.baked_rigid, 1);
        assert_eq!(b.stats.triangles, 8);
        assert_eq!(b.model.triangle_count(), 8);
        assert!(b.model.meshes.iter().all(|m| m.states == STATES_ALL));
        assert_eq!(b.stats.damage_meshes, [4, 0, 0, 0]);
        // Materials: three distinct descriptions.
        assert_eq!(b.model.materials.len(), 3);
        assert_eq!(b.model.parts.len(), 1);
        assert_eq!(b.model.parts[0].name, "Ship");
        assert_eq!(b.model.parts[0].mesh_count as usize, b.model.meshes.len());
        assert!(ModelBlobV2::decode(&b.model.encode()).is_ok());
    }

    #[test]
    fn a_rigid_mesh_on_an_animated_bone_keeps_bind_and_a_still_one_is_baked() {
        let still = bake(&[], MergeScope::Part);
        let moving = bake(&[1], MergeScope::Part);
        assert_eq!(moving.stats.bound_meshes, 1);
        let bound: Vec<_> = moving
            .model
            .meshes
            .iter()
            .filter(|m| m.bind == Some(1))
            .collect();
        assert_eq!(bound.len(), 1);
        assert!(!bound[0].has(asset_format::Semantic::BlendIndices0));
        assert_eq!(still.stats.bound_meshes, 0);
        assert!(still.model.meshes.iter().all(|m| m.bind.is_none()));
        // The bone sits at its bind pose, so baking changes nothing: same vertices as bound.
        let p = bound[0].positions().unwrap();
        assert_eq!(p[0], [6.0, 2.0, 1.0]);
        let prop = still
            .model
            .meshes
            .iter()
            .find(|m| m.material == 2 && !m.has(asset_format::Semantic::BlendIndices0))
            .unwrap();
        assert_eq!(prop.positions().unwrap(), p);
    }

    #[test]
    fn quantization_and_axes_match_b1() {
        let b = bake(&[], MergeScope::Part);
        let flag = b
            .model
            .meshes
            .iter()
            .find(|m| m.material == 2 && m.has(asset_format::Semantic::BlendIndices0))
            .unwrap();
        // Source Z up becomes engine Y.
        assert_eq!(flag.positions().unwrap()[0], [3.0, 2.0, 1.0]);
        // A V of 3.5 flips to -2.5: outside the f16 range of the bound, so f32 is used.
        assert_eq!(
            flag.offset_of(asset_format::Semantic::Texcoord(0))
                .unwrap()
                .1,
            asset_format::AttrFormat::F32x2
        );
        assert_eq!(flag.uvs().unwrap()[0], [0.0, -2.5]);
        assert_eq!(b.stats.uv_f32_meshes, 1);
        let hull = &b.model.meshes[0];
        assert_eq!(hull.uvs().unwrap()[0], [0.0, 0.75]);
        // Joint numbers are skeleton bones: the binding table maps 0 -> bone 1 for the hull.
        assert_eq!(hull.joints().unwrap()[0], [1, 0, 0, 0]);
        assert_eq!(hull.weights().unwrap()[0], [255, 0, 0, 0]);
        assert!(b.stats.uv_f32_meshes <= 1);
    }

    /// Two rigid rocks around their own origin, each a model whose `InitialPlacement` is its place and
    /// whose still bone has the inverse of that place as `InverseWorld` (the stone deposit).
    fn rocks() -> Scene {
        let rock = |name: &str, at: [f32; 3]| {
            let mut inverse_world = [0.0f32; 16];
            for i in 0..4 {
                inverse_world[i * 5] = 1.0;
            }
            inverse_world[12] = -at[0];
            inverse_world[13] = -at[1];
            GBone {
                name: Some(name.into()),
                name_id: None,
                parent: None,
                local: gtransform([0.0; 3]),
                inverse_world,
                lod_error: 0.0,
            }
        };
        let mesh = |name: &str| GrannyMesh {
            name: Some(name.into()),
            positions: vec![[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
            uv: vec![vec![[0.0, 0.0]; 3]],
            indices: vec![0, 1, 2],
            groups: vec![Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 1,
            }],
            material_bindings: vec![Some(0)],
            bone_bindings: vec![BoneBinding {
                name: None,
                name_id: None,
                bone: Some(0),
            }],
            ..GrannyMesh::default()
        };
        let model = |name: &str, skeleton: usize, mesh: usize, at: [f32; 3]| granny::Model {
            name: Some(name.into()),
            skeleton: Some(skeleton),
            initial_placement: gtransform(at),
            meshes: vec![mesh],
        };
        Scene {
            skeletons: vec![
                GrannySkeleton {
                    name: None,
                    bones: vec![rock("a", [10.0, 0.0, 0.0])],
                },
                GrannySkeleton {
                    name: None,
                    bones: vec![rock("b", [-10.0, 0.0, 0.0])],
                },
            ],
            meshes: vec![mesh("a"), mesh("b")],
            models: vec![
                model("a", 0, 0, [10.0, 0.0, 0.0]),
                model("b", 1, 1, [-10.0, 0.0, 0.0]),
            ],
            animations: Vec::new(),
            materials: Vec::new(),
        }
    }

    #[test]
    fn a_still_bone_that_only_undoes_the_placement_leaves_the_objects_at_their_places() {
        let scene = rocks();
        let descs = vec![Some(desc("rock_diff.dds"))];
        let animated = BTreeSet::new();
        let b = bake_model(&BakeInput {
            scene: &scene,
            source: "rocks",
            descs: &descs,
            animated_bones: &animated,
            scope: MergeScope::Model,
            basis: Basis::CIV5_TO_ENGINE,
            multi: true,
        })
        .unwrap();
        // One merged mesh; the rocks stand 20 apart along X (the basis keeps X).
        assert_eq!(b.model.meshes.len(), 1);
        let xs: Vec<f32> = b.model.meshes[0]
            .positions()
            .unwrap()
            .iter()
            .map(|p| p[0])
            .collect();
        let (lo, hi) = xs
            .iter()
            .fold((f32::MAX, f32::MIN), |(l, h), &x| (l.min(x), h.max(x)));
        assert!(
            (lo + 11.0).abs() < 1e-2 && (hi - 11.0).abs() < 1e-2,
            "{lo} {hi}"
        );
    }

    #[test]
    fn whole_model_scope_collapses_the_parts() {
        let b = bake(&[], MergeScope::Model);
        assert_eq!(b.model.parts.len(), 1);
        assert_eq!(b.model.parts[0].name, "ship");
        assert_eq!(b.model.triangle_count(), 8);
    }

    #[test]
    fn missing_fgx_or_binding_is_an_error() {
        let scene = ship();
        let animated = BTreeSet::new();
        let mut d = descs();
        d[1] = None;
        let err = bake_model(&BakeInput {
            scene: &scene,
            source: "ship",
            descs: &d,
            animated_bones: &animated,
            scope: MergeScope::Part,
            basis: Basis::CIV5_TO_ENGINE,
            multi: false,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("no FGX block"), "{err}");

        let mut scene = ship();
        scene.meshes[1].material_bindings = vec![None];
        let d = descs();
        let err = bake_model(&BakeInput {
            scene: &scene,
            source: "ship",
            descs: &d,
            animated_bones: &animated,
            scope: MergeScope::Part,
            basis: Basis::CIV5_TO_ENGINE,
            multi: false,
        })
        .unwrap_err()
        .to_string();
        assert!(err.contains("no material binding"), "{err}");
    }

    #[test]
    fn a_bone_binding_without_a_bone_is_placed_on_the_root_or_unbound_with_a_warning() {
        let mut scene = ship();
        scene.meshes[2].bone_bindings[0].bone = None;
        let d = descs();
        let animated = BTreeSet::new();
        let b = bake_model(&BakeInput {
            scene: &scene,
            source: "ship",
            descs: &d,
            animated_bones: &animated,
            scope: MergeScope::Part,
            basis: Basis::CIV5_TO_ENGINE,
            multi: false,
        })
        .unwrap();
        assert!(
            b.warnings
                .iter()
                .any(|w| w.contains("matches no skeleton bone")),
            "{:?}",
            b.warnings
        );
    }

    #[test]
    fn the_state_mask_of_a_granny_mesh_goes_to_its_meshes() {
        let mut scene = ship();
        scene.meshes[0].state_mask = 0b1110;
        scene.meshes[1].state_mask = STATE_INTACT;
        let d = vec![
            Some(desc("a_diff.dds")),
            Some(desc("b_diff.dds")),
            Some(desc("c_diff.dds")),
        ];
        let animated = BTreeSet::new();
        let b = bake_model(&BakeInput {
            scene: &scene,
            source: "x",
            descs: &d,
            animated_bones: &animated,
            scope: MergeScope::Part,
            basis: Basis::CIV5_TO_ENGINE,
            multi: false,
        })
        .unwrap();
        let states: Vec<u8> = b.model.meshes.iter().map(|m| m.states).collect();
        // Both groups of the hull (one gr2 mesh), the flag, the prop.
        assert_eq!(
            states.iter().filter(|&&s| s == 0b1110).count(),
            2,
            "{states:?}"
        );
        assert_eq!(
            states.iter().filter(|&&s| s == STATE_INTACT).count(),
            1,
            "{states:?}"
        );
        assert_eq!(b.stats.damage_meshes, [1, 1, 2, 0]);
    }

    /// The ship plus a second model with a skeleton of its own (2 bones, the second at z = 2) and
    /// one rigid quad bound to that bone; the model is placed at x = 10.
    fn catalog() -> Scene {
        let mut scene = ship();
        scene.skeletons.push(GrannySkeleton {
            name: None,
            bones: vec![gbone(20, None, 0.0), gbone(21, Some(0), 2.0)],
        });
        let quad = vec![
            [6.0, -1.0, 2.0],
            [7.0, -1.0, 2.0],
            [7.0, 1.0, 2.0],
            [6.0, 1.0, 2.0],
        ];
        scene.meshes.push(GrannyMesh {
            name: Some("second".into()),
            positions: quad,
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
            uv: vec![vec![[0.0, 0.0]; 4]],
            indices: vec![0, 1, 2, 0, 2, 3],
            groups: vec![Group {
                material_index: 0,
                tri_first: 0,
                tri_count: 2,
            }],
            material_bindings: vec![Some(2)],
            bone_bindings: vec![BoneBinding {
                name: None,
                name_id: None,
                bone: Some(1),
            }],
            ..GrannyMesh::default()
        });
        scene.models.push(granny::Model {
            name: Some("Second".into()),
            skeleton: Some(1),
            initial_placement: gtransform([10.0, 0.0, 0.0]),
            meshes: vec![3],
        });
        scene
    }

    fn bake_catalog(animated: &[u16], scope: MergeScope) -> BakedModel {
        let scene = catalog();
        let animated: BTreeSet<u16> = animated.iter().copied().collect();
        let descs = descs();
        bake_model(&BakeInput {
            scene: &scene,
            source: "catalog",
            descs: &descs,
            animated_bones: &animated,
            scope,
            basis: Basis::CIV5_TO_ENGINE,
            multi: true,
        })
        .unwrap()
    }

    #[test]
    fn a_catalog_joins_the_skeletons_and_shifts_the_bones_of_later_models() {
        let b = bake_catalog(&[3], MergeScope::Part);
        // 2 bones of the ship, 2 of the second model; the second model's root is a root.
        assert_eq!(b.model.skeleton.bones.len(), 4);
        assert_eq!(b.model.skeleton.bones[2].parent, None);
        assert_eq!(b.model.skeleton.bones[3].parent, Some(2));
        assert_eq!(b.model.parts.len(), 2);
        assert_eq!(b.model.parts[1].name, "Second");
        assert!(b.stats.placed_parts == 1);
        // Bone 1 of the second skeleton is bone 3 of the joined one; a clip moves it, so the mesh
        // keeps its bind.
        let second = b.model.meshes.iter().find(|m| m.name == "second").unwrap();
        assert_eq!(second.bind, Some(3));
        assert_eq!(second.part, 1);
        // The ship's own meshes are as without the second model.
        let hull = &b.model.meshes[0];
        assert_eq!(hull.joints().unwrap()[0], [1, 0, 0, 0]);
        // Without the clip the bone is still: the mesh is baked and keeps no bind.
        let still = bake_catalog(&[], MergeScope::Part);
        assert_eq!(
            still
                .model
                .meshes
                .iter()
                .find(|m| m.name == "second")
                .unwrap()
                .bind,
            None
        );
    }

    #[test]
    fn the_whole_model_scope_bakes_the_placement_of_a_part_into_its_vertices() {
        let b = bake_catalog(&[], MergeScope::Model);
        assert_eq!(b.model.parts.len(), 1);
        // The second model's quad (x 6..7 in its own space) now sits at x 16..17: it merged with
        // the prop of the same material, which stays at 6..7.
        let xs: Vec<f32> = b
            .model
            .meshes
            .iter()
            .flat_map(|m| m.positions().unwrap())
            .map(|p| p[0])
            .collect();
        assert!(xs.iter().any(|x| (x - 16.0).abs() < 1e-3), "{xs:?}");
        assert!(xs.iter().all(|x| *x < 7.5 || *x > 15.9), "{xs:?}");
        // A model without skeletons joined is the unit model: no bones is an error.
        let mut scene = catalog();
        scene.models.iter_mut().for_each(|m| m.skeleton = None);
        let animated = BTreeSet::new();
        let d = descs();
        assert!(
            bake_model(&BakeInput {
                scene: &scene,
                source: "x",
                descs: &d,
                animated_bones: &animated,
                scope: MergeScope::Part,
                basis: Basis::CIV5_TO_ENGINE,
                multi: true,
            })
            .is_err()
        );
    }
}
