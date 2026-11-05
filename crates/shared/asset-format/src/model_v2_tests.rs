//! Tests of the `C5MD` v2 types in `model.rs`.

use glam::Mat4;

use super::*;

fn angle(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d: f32 = (0..3).map(|k| a[k] * b[k]).sum();
    d.clamp(-1.0, 1.0).acos()
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    v.map(|c| c / l)
}

fn arrays(skinned: bool) -> MeshArrays {
    let n = 5;
    MeshArrays {
        positions: (0..n)
            .map(|i| [i as f32 * 0.37 - 1.0, 12345.678 - i as f32, 1e-3 * i as f32])
            .collect(),
        normals: (0..n)
            .map(|i| {
                unit([
                    0.3 + i as f32 * 0.4,
                    -0.5 + i as f32 * 0.1,
                    0.8 - i as f32 * 0.35,
                ])
            })
            .collect(),
        uvs: (0..n)
            .map(|i| [i as f32 * 0.19 - 0.3, 1.0 - i as f32 * 0.23])
            .collect(),
        joints: if skinned {
            (0..n).map(|i| [i as u16 % 3, 1, 0, 2]).collect()
        } else {
            vec![]
        },
        weights: if skinned {
            vec![[100, 100, 55, 0]; n]
        } else {
            vec![]
        },
        indices: vec![0, 1, 2, 2, 1, 3, 3, 4, 0],
    }
}

fn bone(name: &str, parent: Option<u16>) -> Bone {
    Bone {
        name: name.into(),
        parent,
        rest: Transform::IDENTITY,
        inverse_bind: Mat4::IDENTITY.to_cols_array(),
    }
}

fn model_with(mesh: MeshV2) -> ModelBlobV2 {
    ModelBlobV2 {
        source: "test".into(),
        skeleton: Skeleton {
            bones: vec![bone("a", None), bone("b", Some(0)), bone("c", Some(0))],
        },
        parts: vec![Part {
            name: "m".into(),
            first_mesh: 0,
            mesh_count: 1,
            placement: Transform::IDENTITY,
        }],
        meshes: vec![mesh],
        materials: vec![MaterialDesc::default()],
        clips: vec![ClipRef {
            name: "IdleA".into(),
            ec: 1000,
            index: 0,
            events: vec![AnimEvent {
                time: 0.0,
                code: 1000,
            }],
            legacy_blob: None,
        }],
        anim_pack: Some(LogicalId::new("an:test")),
        fsm: Some(LogicalId::new("fsm:1_omni_state_01")),
        state_clips: vec![StateClip {
            state: "Constructed".into(),
            clip: ClipKey::Ec(1000),
            repeat: 2,
            pause: 0.5,
        }],
        effects: vec![],
        bounds: [0.0; 6],
    }
}

#[test]
fn quantization_bounds() {
    let a = arrays(true);
    let m = MeshV2::pack("m", 0, 0, &a, false, MeshV2::uvs_need_f32(&a.uvs)).unwrap();
    assert_eq!(m.stride(), Some(12 + 4 + 4 + 4 + 4));
    // Positions are exact.
    assert_eq!(m.positions().unwrap(), a.positions);
    // Normals within 1e-3 rad.
    for (got, want) in m.normals().unwrap().into_iter().zip(&a.normals) {
        assert!(angle(got, *want) <= 1e-3, "{got:?} {want:?}");
    }
    // UVs within 1/2048.
    for (got, want) in m.uvs().unwrap().into_iter().zip(&a.uvs) {
        for k in 0..2 {
            assert!((got[k] - want[k]).abs() <= 1.0 / 2048.0, "{got:?} {want:?}");
        }
    }
    assert_eq!(m.joints().unwrap()[1], [1, 1, 0, 2]);
    assert_eq!(m.weights().unwrap()[0], [100, 100, 55, 0]);
    assert_eq!(m.indices.to_u32(), a.indices);
    assert!(!m.indices.wide);
}

#[test]
fn large_uvs_use_f32_and_stay_exact() {
    let mut a = arrays(false);
    a.uvs[2] = [3.123_456_7, -2.5];
    assert!(MeshV2::uvs_need_f32(&a.uvs));
    let m = MeshV2::pack("m", 0, 0, &a, false, true).unwrap();
    assert_eq!(
        m.offset_of(Semantic::Texcoord(0)).unwrap().1,
        AttrFormat::F32x2
    );
    assert_eq!(m.uvs().unwrap()[2], [3.123_456_7, -2.5]);
    // Below the limit f16 is within the bound over the whole range.
    let mut worst = 0.0f32;
    for i in 0..4000 {
        let u = -1.999 + i as f32 * 0.000_999;
        let back = crate::f16_bits_to_f32(crate::f32_to_f16_bits(u));
        worst = worst.max((back - u).abs());
    }
    assert!(worst <= 1.0 / 2048.0, "{worst}");
}

#[test]
fn wide_indices_and_wide_joints() {
    let mut a = arrays(true);
    a.indices = vec![0, 1, 70000];
    let m = MeshV2::pack("m", 0, 0, &a, true, false).unwrap();
    assert!(m.indices.wide);
    assert_eq!(m.indices.to_u32(), [0, 1, 70000]);
    assert_eq!(
        m.offset_of(Semantic::BlendIndices0).unwrap().1,
        AttrFormat::U16x4
    );
    // A joint above 255 does not fit the narrow layout.
    a.joints[0][0] = 300;
    assert!(MeshV2::pack("m", 0, 0, &a, false, false).is_err());
    assert!(MeshV2::pack("m", 0, 0, &a, true, false).is_ok());
}

#[test]
fn roundtrip_is_deterministic() {
    let a = arrays(true);
    let mut mesh = MeshV2::pack("hull", 0, 0, &a, false, false).unwrap();
    mesh.states = 0b1110;
    let mut m = model_with(mesh);
    m.bounds = m.compute_bounds();
    assert!(m.bounds[3] >= m.bounds[0]);
    let bytes = m.encode();
    assert_eq!(&bytes[..4], b"C5MD");
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 3);
    assert_eq!(ModelBlobV2::decode(&bytes).unwrap(), m);
    assert_eq!(m.encode(), bytes);
    assert_eq!(m.triangle_count(), 3);
}

/// A version 2 blob carries damage labels where version 3 has state masks: they are read as masks.
#[test]
fn version_2_damage_labels_read_as_state_masks() {
    let a = arrays(true);
    let mut m = model_with(MeshV2::pack("hull", 0, 0, &a, false, false).unwrap());
    for (label, states) in [
        (0u8, STATES_ALL),
        (1, STATE_INTACT),
        (2, 0b1110),
        (3, 0b1100),
    ] {
        m.meshes[0].states = label;
        let mut bytes = m.encode();
        bytes[4..6].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            ModelBlobV2::decode(&bytes).unwrap().meshes[0].states,
            states,
            "label {label}"
        );
    }
}

#[test]
fn rigid_mesh_with_bind() {
    let a = arrays(false);
    let mut mesh = MeshV2::pack("prop", 0, 0, &a, false, false).unwrap();
    mesh.bind = Some(2);
    let m = model_with(mesh);
    assert!(ModelBlobV2::decode(&m.encode()).is_ok());
    let mut bad = m.clone();
    bad.meshes[0].bind = Some(3);
    assert!(ModelBlobV2::decode(&bad.encode()).is_err());
}

#[test]
fn hostile_blobs_are_errors() {
    let a = arrays(true);
    let good = model_with(MeshV2::pack("m", 0, 0, &a, false, false).unwrap());
    assert!(ModelBlobV2::decode(&good.encode()).is_ok());
    let mut cases: Vec<(&str, ModelBlobV2)> = Vec::new();
    let mut m = good.clone();
    m.meshes[0].vertices.pop();
    cases.push(("short vertices", m));
    let mut m = good.clone();
    m.meshes[0].vertex_count = u32::MAX;
    cases.push(("huge vertex count", m));
    let mut m = good.clone();
    m.meshes[0].indices.data.truncate(7);
    cases.push(("partial triangle", m));
    let mut m = good.clone();
    m.meshes[0].indices = PackedIndices::from_u32(&[0, 1, 9]);
    cases.push(("index out of range", m));
    let mut m = good.clone();
    m.meshes[0].material = 1;
    cases.push(("material out of range", m));
    let mut m = good.clone();
    m.meshes[0].part = 1;
    cases.push(("part out of range", m));
    let mut m = good.clone();
    m.meshes[0].codec = crate::Codec::Zstd;
    cases.push(("codec", m));
    let mut m = good.clone();
    m.skeleton.bones.truncate(1);
    cases.push(("joint out of range", m));
    let mut m = good.clone();
    m.parts[0].mesh_count = 5;
    cases.push(("part past the meshes", m));
    let mut m = good.clone();
    m.parts[0].mesh_count = 0;
    cases.push(("mesh outside its part", m));
    let mut m = good.clone();
    m.bounds[2] = f32::NAN;
    cases.push(("bounds", m));
    let mut m = good.clone();
    m.meshes[0].layout.push(VertexAttr {
        semantic: Semantic::Color(0),
        format: AttrFormat::Snorm16x4,
    });
    cases.push(("layout longer than the bytes", m));
    for (what, m) in cases {
        assert!(
            matches!(
                ModelBlobV2::decode(&m.encode()),
                Err(FormatError::Invalid(_))
            ),
            "{what}"
        );
    }
    let bytes = good.encode();
    for cut in [0, 3, 6, bytes.len() / 2, bytes.len() - 1] {
        assert!(ModelBlobV2::decode(&bytes[..cut]).is_err(), "cut {cut}");
    }
    let mut bad = bytes.clone();
    bad[4] = 9;
    assert!(matches!(
        ModelBlobV2::decode(&bad),
        Err(FormatError::UnsupportedVersion { .. })
    ));
    // An empty layout with a claimed vertex count is an error, not an allocation.
    let mut huge = good.clone();
    huge.meshes[0].layout.clear();
    assert!(ModelBlobV2::decode(&huge.encode()).is_err());
}

#[test]
fn v1_is_read_and_converted() {
    let v1 = super::tests::sample_model();
    let m = ModelBlobV2::decode(&v1.encode()).unwrap();
    assert_eq!(m.parts.len(), 1);
    assert_eq!(m.parts[0].mesh_count as usize, v1.meshes.len());
    assert_eq!(m.meshes.len(), 2);
    // Geometry survives byte for byte.
    assert_eq!(m.meshes[0].positions().unwrap(), v1.meshes[0].positions);
    assert_eq!(m.meshes[0].joints().unwrap()[0], [0, 1, 0, 0]);
    assert_eq!(m.meshes[0].weights().unwrap()[0], [200, 55, 0, 0]);
    assert!(!m.meshes[1].has(Semantic::BlendIndices0));
    assert_eq!(m.meshes[1].indices.to_u32(), [0, 1, 2, 2, 1, 3]);
    assert_eq!(m.meshes[1].material, 1);
    // Materials keep their textures, clips keep their blob.
    let files: Vec<_> = m.materials[0]
        .texture_files()
        .map(LogicalId::as_str)
        .collect();
    assert_eq!(files, ["tex.diffuse"]);
    assert_eq!(m.clips[0].legacy_blob, Some(LogicalId::new("anim.idle")));
    assert_eq!(m.clips[0].events[0].code, 7);
    assert_eq!(m.skeleton, v1.skeleton);
}

/// A v2 model back to a rigid v1 one: the part's placement is applied, the uvs and normals keep
/// their values, the diffuse is the base sampler.
#[test]
fn v2_becomes_a_static_v1_model() {
    let v1 = super::tests::sample_model();
    let mut m = ModelBlobV2::decode(&v1.encode()).unwrap();
    m.parts[0].placement.translation = [1.0, 2.0, 3.0];
    m.materials[0].sets[0].textures[0].name = "BaseSampler".into();
    let back = m.to_static_v1().unwrap();
    assert_eq!(back.meshes.len(), v1.meshes.len());
    for (a, b) in back.meshes.iter().zip(&v1.meshes) {
        for (p, q) in a.positions.iter().zip(&b.positions) {
            assert_eq!(*p, [q[0] + 1.0, q[1] + 2.0, q[2] + 3.0]);
        }
        assert_eq!(a.uvs, b.uvs);
        for (n, o) in a.normals.iter().zip(&b.normals) {
            assert!(angle(crate::oct_decode(*n), crate::oct_decode(*o)) < 1e-3);
        }
        assert!(a.joints.is_empty() && a.weights.is_empty());
        assert_eq!(a.material, b.material);
    }
    assert_eq!(
        back.materials[0].diffuse,
        Some(LogicalId::new("tex.diffuse"))
    );
    assert!(back.animations.is_empty());
}
