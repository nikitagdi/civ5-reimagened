//! Static (rigid) models: one pose of a skinned model baked into its vertices.
//!
//! B1 draws units without animations. The static model is the converted model with frame 0 of
//! the idle clip (the clip whose event list has [`IDLE_EVENT_CODE`], `Warrior_IdleA`) applied to
//! the vertices: positions and normals go through the same skinning the client does
//! (`palette = world * inverse_bind`, `sum(weight * palette[joint])`, normal as a direction
//! through the same matrix, renormalized), then the skeleton, joints, weights and animations are
//! dropped.
//!
//! SIMPLIFIED(SIMP-010): units are shown in the pose of the first idle frame, not animated.
//!
//! Ships and the Great Admiral also contain a damage-state hull: full size, overlapping the intact
//! one, and not hidden by frame 0. [`bake_static`] drops the meshes bound to the materials it is
//! told are damage-state ones (see [`super::set::LoadedMember::damage_materials`]) and the
//! materials nothing uses afterwards.

use asset_format::{
    AnimRef, AnimationBlob, LogicalId, Mesh, ModelBlob, Skeleton, oct_decode, oct_encode,
};
use glam::{Mat4, Vec3, Vec4};

use super::ModelError;

/// Event code of the idle clip in the unit fxsxml files (`Warrior_IdleA`: `ec="1000"`).
pub const IDLE_EVENT_CODE: u32 = 1000;

/// The clip a static unit is posed from: the first animation listed with [`IDLE_EVENT_CODE`] whose
/// clip is in `clips`.
pub fn idle_clip<'a>(
    model: &'a ModelBlob,
    clips: &'a [(LogicalId, AnimationBlob)],
) -> Option<(&'a AnimRef, &'a AnimationBlob)> {
    model
        .animations
        .iter()
        .filter(|a| a.events.iter().any(|e| e.code == IDLE_EVENT_CODE))
        .find_map(|a| {
            clips
                .iter()
                .find(|(id, _)| *id == a.blob)
                .map(|(_, c)| (a, c))
        })
}

/// Skinning palette (`world * inverse_bind`) of `skeleton` at `t` seconds of `clip`; the rest pose
/// without a clip.
pub fn skin_palette(skeleton: &Skeleton, clip: Option<&AnimationBlob>, t: f32) -> Vec<Mat4> {
    let n = skeleton.bones.len();
    let rest = skeleton.rest_pose();
    let mut local = rest.clone();
    if let Some(c) = clip {
        c.sample(t, false, &rest, &mut local);
    }
    let mut world = vec![Mat4::IDENTITY; n];
    skeleton.world_matrices(&local, &mut world);
    let mut skin = vec![Mat4::IDENTITY; n];
    skeleton.skin_matrices(&world, &mut skin);
    skin
}

fn bake_mesh(mesh: &Mesh, skin: &[Mat4], index: usize) -> Result<Mesh, ModelError> {
    if mesh.joints.is_empty() {
        return Ok(mesh.clone());
    }
    let mut out = Mesh {
        positions: Vec::with_capacity(mesh.positions.len()),
        normals: Vec::with_capacity(mesh.normals.len()),
        joints: Vec::new(),
        weights: Vec::new(),
        ..mesh.clone()
    };
    for (i, (p, n)) in mesh.positions.iter().zip(&mesh.normals).enumerate() {
        let mut m = Mat4::ZERO;
        for k in 0..4 {
            let w = f32::from(mesh.weights[i][k]) / 255.0;
            let joint = usize::from(mesh.joints[i][k]);
            let bone = skin.get(joint).ok_or_else(|| {
                ModelError::invalid(format!(
                    "mesh {index}: vertex {i} uses joint {joint}, the skeleton has {}",
                    skin.len()
                ))
            })?;
            m += *bone * w;
        }
        out.positions
            .push((m * Vec4::new(p[0], p[1], p[2], 1.0)).truncate().to_array());
        let n = Vec3::from_array(oct_decode(*n));
        let n = (m * n.extend(0.0)).truncate();
        out.normals.push(oct_encode(n.to_array()));
    }
    Ok(out)
}

/// `model` with the pose at `t` seconds of `clip` (rest pose if `None`) applied to its skinned
/// meshes; no skeleton, no joints and weights, no animations. Rigid meshes are kept as they are.
pub fn bake_pose(
    model: &ModelBlob,
    clip: Option<&AnimationBlob>,
    t: f32,
) -> Result<ModelBlob, ModelError> {
    let skin = skin_palette(&model.skeleton, clip, t);
    let meshes = model
        .meshes
        .iter()
        .enumerate()
        .map(|(i, m)| bake_mesh(m, &skin, i))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ModelBlob {
        skeleton: Skeleton::default(),
        meshes,
        materials: model.materials.clone(),
        animations: Vec::new(),
    })
}

/// A mesh removed from the static model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedMesh {
    /// Index in the animated model's mesh list.
    pub mesh: usize,
    /// Index in the animated model's material list.
    pub material: u16,
    pub diffuse: Option<LogicalId>,
    pub triangles: usize,
}

#[derive(Debug)]
pub struct StaticModel {
    pub model: ModelBlob,
    /// Name of the clip frame 0 of which was baked; `None`: no idle clip, bind pose baked.
    pub clip: Option<String>,
    pub dropped: Vec<DroppedMesh>,
    pub warnings: Vec<String>,
}

/// The static model of `model`: frame 0 of its idle clip baked in, meshes drawn with a material
/// in `damage_materials` (indices into `model.materials`) dropped, materials left unused removed
/// (meshes are renumbered).
pub fn bake_static(
    model: &ModelBlob,
    clips: &[(LogicalId, AnimationBlob)],
    damage_materials: &[u16],
) -> Result<StaticModel, ModelError> {
    let mut warnings = Vec::new();
    let idle = idle_clip(model, clips);
    if idle.is_none() && !model.skeleton.bones.is_empty() {
        warnings.push(format!(
            "no animation with event {IDLE_EVENT_CODE}; the bind pose is baked"
        ));
    }
    let mut baked = bake_pose(model, idle.map(|(_, c)| c), 0.0)?;

    // SIMPLIFIED(SIMP-010): the original switches between the intact and the damage-state hull by
    // the unit's damage; a static unit shows the intact one.
    let mut dropped = Vec::new();
    let all = std::mem::take(&mut baked.meshes);
    for (i, mesh) in all.into_iter().enumerate() {
        if damage_materials.contains(&mesh.material) {
            dropped.push(DroppedMesh {
                mesh: i,
                material: mesh.material,
                diffuse: baked
                    .materials
                    .get(usize::from(mesh.material))
                    .and_then(|m| m.diffuse.clone()),
                triangles: mesh.indices.len() / 3,
            });
        } else {
            baked.meshes.push(mesh);
        }
    }

    // Keep only the materials that meshes still draw with, in their old order.
    let mut used = vec![false; baked.materials.len()];
    for m in &baked.meshes {
        if let Some(u) = used.get_mut(usize::from(m.material)) {
            *u = true;
        }
    }
    let mut renumber = vec![0u16; used.len()];
    let mut kept = Vec::new();
    for (old, material) in baked.materials.iter().enumerate() {
        if used[old] {
            renumber[old] = kept.len() as u16;
            kept.push(material.clone());
        }
    }
    for m in &mut baked.meshes {
        m.material = renumber[usize::from(m.material)];
    }
    baked.materials = kept;

    Ok(StaticModel {
        model: baked,
        clip: idle.map(|(a, _)| a.name.clone()),
        dropped,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{AnimEvent, Bone, Indices, Material, Track, Transform, quat_pack};

    fn bone(name: &str, parent: Option<u16>, t: [f32; 3]) -> Bone {
        Bone {
            name: name.into(),
            parent,
            rest: Transform {
                translation: t,
                ..Transform::IDENTITY
            },
            inverse_bind: Mat4::from_translation(-Vec3::from_array(t)).to_cols_array(),
        }
    }

    fn mesh(material: u16, skinned: bool) -> Mesh {
        let n = 3;
        Mesh {
            positions: vec![[0.0, 0.5, 0.0], [1.0, 1.0, 0.0], [0.0, 2.0, 0.5]],
            normals: vec![
                oct_encode([1.0, 0.0, 0.0]),
                oct_encode([0.0, 1.0, 0.0]),
                [0, 0],
            ],
            uvs: vec![[0, 0]; n],
            joints: if skinned {
                vec![[0, 1, 0, 0]; n]
            } else {
                vec![]
            },
            weights: if skinned {
                vec![[64, 191, 0, 0]; n]
            } else {
                vec![]
            },
            indices: Indices::U16(vec![0, 1, 2]),
            material,
        }
    }

    fn model() -> (ModelBlob, Vec<(LogicalId, AnimationBlob)>) {
        let rot = |z: f32| {
            let (s, c) = (z / 2.0).sin_cos();
            quat_pack([0.0, 0.0, s, c])
        };
        let clip = AnimationBlob {
            duration: 1.0,
            sample_rate: 30.0,
            tracks: vec![
                Track {
                    bone: 0,
                    rotations: vec![rot(0.7), rot(1.4)],
                    translations: vec![],
                    scales: vec![],
                },
                Track {
                    bone: 1,
                    rotations: vec![rot(-0.4)],
                    translations: vec![[0.0, 1.5, 0.2]],
                    scales: vec![[1.0, 1.2, 1.0]],
                },
            ],
        };
        let material = |d: &str| Material {
            diffuse: Some(LogicalId::new(d)),
            specular: None,
        };
        let model = ModelBlob {
            skeleton: Skeleton {
                bones: vec![
                    bone("a", None, [0.0; 3]),
                    bone("b", Some(0), [0.0, 1.0, 0.0]),
                ],
            },
            meshes: vec![mesh(0, true), mesh(1, true), mesh(2, false)],
            materials: vec![
                material("tex:ship_diff.dds"),
                material("tex:ship_dmg_diff.dds"),
                material("tex:flag.dds"),
            ],
            animations: vec![
                AnimRef {
                    name: "Walk".into(),
                    blob: LogicalId::new("anim:walk"),
                    events: vec![AnimEvent {
                        time: 0.0,
                        code: 1010,
                    }],
                },
                AnimRef {
                    name: "IdleA".into(),
                    blob: LogicalId::new("anim:idlea"),
                    events: vec![AnimEvent {
                        time: 0.0,
                        code: IDLE_EVENT_CODE,
                    }],
                },
            ],
        };
        (model, vec![(LogicalId::new("anim:idlea"), clip)])
    }

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    /// Reference: the pose of the animation path, computed by hand without the baking code.
    fn reference(model: &ModelBlob, clip: &AnimationBlob, v: usize) -> ([f32; 3], [f32; 3]) {
        let rest = model.skeleton.rest_pose();
        let mut local = rest.clone();
        clip.sample(0.0, false, &rest, &mut local);
        let w0 = local[0].to_mat4();
        let w1 = w0 * local[1].to_mat4();
        let skin = [
            w0 * Mat4::from_cols_array(&model.skeleton.bones[0].inverse_bind),
            w1 * Mat4::from_cols_array(&model.skeleton.bones[1].inverse_bind),
        ];
        let m = &model.meshes[0];
        let p = Vec3::from_array(m.positions[v]);
        let n = Vec3::from_array(oct_decode(m.normals[v]));
        let (wa, wb) = (64.0 / 255.0, 191.0 / 255.0);
        let pos = skin[0].transform_point3(p) * wa + skin[1].transform_point3(p) * wb;
        let nrm =
            (skin[0].transform_vector3(n) * wa + skin[1].transform_vector3(n) * wb).normalize();
        (pos.to_array(), nrm.to_array())
    }

    #[test]
    fn baked_vertices_equal_the_animated_pose() {
        let (model, clips) = model();
        let clip = &clips[0].1;
        let baked = bake_pose(&model, Some(clip), 0.0).unwrap();
        assert!(baked.skeleton.bones.is_empty() && baked.animations.is_empty());
        let moved = baked.meshes[0].positions[1] != model.meshes[0].positions[1];
        assert!(moved, "the pose must differ from the bind pose");
        for v in 0..2 {
            let (p, n) = reference(&model, clip, v);
            assert!(close(baked.meshes[0].positions[v], p), "{v}");
            let got = oct_decode(baked.meshes[0].normals[v]);
            assert!(
                got.iter().zip(&n).all(|(a, b)| (a - b).abs() < 2e-3),
                "{v}: {got:?} {n:?}"
            );
        }
        assert!(
            baked
                .meshes
                .iter()
                .all(|m| m.joints.is_empty() && m.weights.is_empty())
        );
        assert!(ModelBlob::decode(&baked.encode()).is_ok());
    }

    #[test]
    fn without_a_clip_the_bind_pose_is_baked() {
        let (model, _) = model();
        let baked = bake_pose(&model, None, 0.0).unwrap();
        for (a, b) in baked.meshes[0]
            .positions
            .iter()
            .zip(&model.meshes[0].positions)
        {
            assert!(close(*a, *b));
        }
    }

    #[test]
    fn rigid_meshes_are_kept_as_they_are() {
        let (model, clips) = model();
        let baked = bake_pose(&model, Some(&clips[0].1), 0.0).unwrap();
        assert_eq!(baked.meshes[2], model.meshes[2]);
    }

    #[test]
    fn idle_clip_is_found_by_its_event_code() {
        let (model, clips) = model();
        assert_eq!(idle_clip(&model, &clips).unwrap().0.name, "IdleA");
        assert!(idle_clip(&model, &[]).is_none());
    }

    #[test]
    fn damage_state_meshes_are_dropped_and_materials_renumbered() {
        let (model, clips) = model();
        let s = bake_static(&model, &clips, &[1]).unwrap();
        assert_eq!(s.clip.as_deref(), Some("IdleA"));
        assert!(s.warnings.is_empty());
        assert_eq!(s.dropped.len(), 1);
        assert_eq!(s.dropped[0].mesh, 1);
        assert_eq!(s.dropped[0].triangles, 1);
        assert_eq!(
            s.dropped[0].diffuse,
            Some(LogicalId::new("tex:ship_dmg_diff.dds"))
        );
        assert_eq!(s.model.meshes.len(), 2);
        assert_eq!(s.model.materials.len(), 2);
        assert!(
            s.model
                .materials
                .iter()
                .all(|m| m.diffuse != Some(LogicalId::new("tex:ship_dmg_diff.dds")))
        );
        let ids: Vec<u16> = s.model.meshes.iter().map(|m| m.material).collect();
        assert_eq!(ids, [0, 1]);
        assert_eq!(
            s.model.materials[1].diffuse,
            Some(LogicalId::new("tex:flag.dds"))
        );
        assert!(ModelBlob::decode(&s.model.encode()).is_ok());
    }

    #[test]
    fn a_model_without_damage_materials_keeps_every_mesh() {
        let (model, clips) = model();
        let s = bake_static(&model, &clips, &[]).unwrap();
        assert!(s.dropped.is_empty());
        assert_eq!(s.model.meshes.len(), 3);
        assert_eq!(s.model.materials, model.materials);
    }

    #[test]
    fn no_idle_clip_warns_and_bakes_the_bind_pose() {
        let (model, _) = model();
        let s = bake_static(&model, &[], &[]).unwrap();
        assert!(s.clip.is_none());
        assert_eq!(s.warnings.len(), 1);
    }

    #[test]
    fn a_joint_outside_the_skeleton_is_an_error() {
        let (mut model, _) = model();
        model.meshes[0].joints[0][0] = 9;
        assert!(bake_pose(&model, None, 0.0).is_err());
    }
}
