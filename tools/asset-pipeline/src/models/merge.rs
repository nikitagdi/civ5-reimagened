//! Merging the meshes of a v2 model ("Слияние").
//!
//! Meshes with the same material, the same rigid bone and the same node states are joined into
//! one mesh: one draw call per material instead of one per Granny mesh group. Triangles and
//! vertices are only concatenated (indices shifted), nothing is re-sorted or re-quantized, so the
//! merged model draws exactly what the parts drew.
//!
//! The scope is the `Model` of the gr2 ([`MergeScope::Part`]) for catalogs whose parts a layout
//! handler places one by one, or the whole file ([`MergeScope::Model`]).

use asset_format::MeshArrays;

/// A mesh before packing: unquantized arrays and the labels that decide what merges.
#[derive(Debug, Clone, PartialEq)]
pub struct RawMesh {
    pub name: String,
    /// Index of the part (gr2 `Model`) the mesh belongs to.
    pub part: u16,
    /// Index into the model's material list.
    pub material: u16,
    pub arrays: MeshArrays,
    /// Rigid mesh on an animated bone.
    pub bind: Option<u16>,
    /// The node states it is drawn in (`MeshV2::states`).
    pub states: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeScope {
    /// Only meshes of one part merge; the parts stay.
    Part,
    /// All meshes of the model merge; the caller collapses the parts to one.
    Model,
}

type Key = (u16, u16, Option<u16>, u8);

fn key(m: &RawMesh, scope: MergeScope) -> Key {
    let part = match scope {
        MergeScope::Part => m.part,
        MergeScope::Model => 0,
    };
    (part, m.material, m.bind, m.states)
}

/// Joins `a` and `b`: vertices of `b` follow those of `a`, indices of `b` are shifted. Both must
/// be skinned or both rigid.
fn append(a: &mut RawMesh, b: RawMesh) {
    let shift = a.arrays.positions.len() as u32;
    a.arrays.positions.extend(b.arrays.positions);
    a.arrays.normals.extend(b.arrays.normals);
    a.arrays.uvs.extend(b.arrays.uvs);
    a.arrays.joints.extend(b.arrays.joints);
    a.arrays.weights.extend(b.arrays.weights);
    a.arrays
        .indices
        .extend(b.arrays.indices.into_iter().map(|i| i + shift));
}

/// Merges meshes with the same key, in the order of their first member. Under
/// [`MergeScope::Part`] the result is ordered by part first (a part's meshes stay together);
/// under [`MergeScope::Model`] the part of every result is 0.
///
/// A skinned and a rigid mesh of one key cannot join (their layouts differ): skinned-ness is part
/// of the key.
pub fn merge_meshes(meshes: Vec<RawMesh>, scope: MergeScope) -> Vec<RawMesh> {
    let mut out: Vec<(Key, bool, RawMesh, usize)> = Vec::new();
    for mut m in meshes {
        let k = key(&m, scope);
        let skinned = !m.arrays.joints.is_empty();
        if scope == MergeScope::Model {
            m.part = 0;
        }
        match out
            .iter_mut()
            .find(|(ok, os, _, _)| *ok == k && *os == skinned)
        {
            Some((_, _, first, n)) => {
                *n += 1;
                append(first, m);
            }
            None => out.push((k, skinned, m, 1)),
        }
    }
    let mut merged: Vec<RawMesh> = out
        .into_iter()
        .map(|(_, _, mut m, n)| {
            if n > 1 {
                m.name = format!("{}+{}", m.name, n - 1);
            }
            m
        })
        .collect();
    // Stable: first-member order inside a part.
    merged.sort_by_key(|m| m.part);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(
        name: &str,
        part: u16,
        material: u16,
        bind: Option<u16>,
        states: u8,
        skinned: bool,
        x: f32,
    ) -> RawMesh {
        RawMesh {
            name: name.into(),
            part,
            material,
            arrays: MeshArrays {
                positions: vec![[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                uvs: vec![[x, 0.0]; 3],
                joints: if skinned {
                    vec![[1, 0, 0, 0]; 3]
                } else {
                    vec![]
                },
                weights: if skinned {
                    vec![[255, 0, 0, 0]; 3]
                } else {
                    vec![]
                },
                indices: vec![0, 1, 2],
            },
            bind,
            states,
        }
    }

    fn triangles(ms: &[RawMesh]) -> usize {
        ms.iter().map(|m| m.arrays.indices.len() / 3).sum()
    }

    #[test]
    fn same_material_merges_and_keeps_every_triangle() {
        let input = vec![
            tri("a", 0, 0, None, 0, true, 0.0),
            tri("b", 0, 1, None, 0, true, 10.0),
            tri("c", 0, 0, None, 0, true, 20.0),
        ];
        let n_tri = triangles(&input);
        let out = merge_meshes(input, MergeScope::Part);
        assert_eq!(out.len(), 2);
        assert_eq!(triangles(&out), n_tri);
        // First-member order; the second mesh of material 0 follows the first one.
        assert_eq!(out[0].material, 0);
        assert_eq!(out[0].name, "a+1");
        assert_eq!(out[0].arrays.positions.len(), 6);
        assert_eq!(out[0].arrays.indices, [0, 1, 2, 3, 4, 5]);
        assert_eq!(out[0].arrays.positions[3], [20.0, 0.0, 0.0]);
        assert_eq!(out[0].arrays.joints.len(), 6);
        assert_eq!(out[1].name, "b");
        // Each triangle still points at its own vertices.
        for m in &out {
            for t in m.arrays.indices.chunks(3) {
                assert!(t.iter().all(|&i| (i as usize) < m.arrays.positions.len()));
            }
        }
    }

    #[test]
    fn bind_damage_and_skinning_keep_meshes_apart() {
        let input = vec![
            tri("hull", 0, 0, None, 1, true, 0.0),
            tri("hull_dmg", 0, 0, None, 2, true, 0.0),
            tri("prop_a", 0, 0, Some(3), 0, false, 0.0),
            tri("prop_b", 0, 0, Some(4), 0, false, 0.0),
            tri("prop_c", 0, 0, Some(3), 0, false, 5.0),
            tri("rigid", 0, 0, None, 1, false, 0.0),
        ];
        let out = merge_meshes(input, MergeScope::Model);
        // hull(1), hull_dmg(2), prop on 3 (two merged), prop on 4, rigid hull (not skinned).
        assert_eq!(out.len(), 5);
        let dmg: Vec<u8> = out.iter().map(|m| m.states).collect();
        assert_eq!(dmg, [1, 2, 0, 0, 1]);
        let merged = out.iter().find(|m| m.bind == Some(3)).unwrap();
        assert_eq!(merged.arrays.positions.len(), 6);
    }

    #[test]
    fn parts_stay_apart_under_part_scope() {
        let input = vec![
            tri("a0", 0, 0, None, 0, false, 0.0),
            tri("b0", 1, 0, None, 0, false, 0.0),
            tri("a1", 0, 0, None, 0, false, 3.0),
        ];
        let parts = merge_meshes(input.clone(), MergeScope::Part);
        assert_eq!(parts.len(), 2);
        assert_eq!((parts[0].part, parts[1].part), (0, 1));
        assert_eq!(parts[0].arrays.positions.len(), 6);
        let whole = merge_meshes(input, MergeScope::Model);
        assert_eq!(whole.len(), 1);
        assert_eq!(whole[0].part, 0);
        assert_eq!(triangles(&whole), 3);
    }

    #[test]
    fn nothing_to_merge_is_a_no_op() {
        assert!(merge_meshes(Vec::new(), MergeScope::Part).is_empty());
        let one = vec![tri("a", 0, 0, None, 0, true, 0.0)];
        assert_eq!(merge_meshes(one.clone(), MergeScope::Model), one);
    }
}
