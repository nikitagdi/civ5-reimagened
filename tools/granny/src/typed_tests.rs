//! Typed layer tests on a synthetic gr2: skeleton, skinned mesh, model and animation.

use crate::bind::{self, Method};
use crate::tests::*;
use crate::{GrannyError, GrannyFile, Ptr, Scene};

#[derive(Clone, Copy, Default)]
struct Opts {
    bad_index: bool,
    bad_parent: bool,
}

fn transform(w: &mut Writer, flags: u32, pos: [f32; 3], q: [f32; 4], m: [f32; 9]) {
    w.u32(2, flags);
    for f in pos.iter().chain(&q).chain(&m) {
        w.bytes(2, &f.to_le_bytes());
    }
}

fn floats(w: &mut Writer, v: &[f32]) {
    for f in v {
        w.bytes(2, &f.to_le_bytes());
    }
}

const IDENT3: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
const ID_ROOT: u32 = 0x1111;
const ID_CHILD: u32 = 0x2222;
const ID_STRAY: u32 = 0x9999;
const ID_MATERIAL: u32 = 0x3333;
const ID_USAGE: u32 = 0x4444;
const ID_TEX_FILE: u32 = 0x5555;

/// One skeleton (2 bones with ID-only names), one skinned mesh with one material, one model and
/// one animation with three tracks (IDs child, root, stray).
fn typed_file(o: Opts) -> Vec<u8> {
    let mut w = Writer::new(3);
    let tex_t = w.typedef(1, &[def(STRING, "FromFileName")]);
    let submat_t = w.typedef(1, &[def(REFERENCE, "Texture").child(tex_t)]);
    let map_t = w.typedef(
        1,
        &[def(STRING, "Usage"), def(REFERENCE, "Map").child(submat_t)],
    );
    let mat_t = w.typedef(
        1,
        &[def(STRING, "Name"), def(REF_TO_ARRAY, "Maps").child(map_t)],
    );
    let bone_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(INT32, "ParentIndex"),
            def(TRANSFORM, "Transform"),
            def(REAL32, "InverseWorldTransform").width(16),
            def(REAL32, "LODError"),
        ],
    );
    let skel_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(REF_TO_ARRAY, "Bones").child(bone_t),
        ],
    );
    let vtx_t = w.typedef(
        1,
        &[
            def(REAL32, "Position").width(3),
            def(REAL32, "Normal").width(3),
            def(REAL32, "TextureCoordinates0").width(2),
            def(UINT8, "BoneWeights").width(4),
            def(UINT8, "BoneIndices").width(4),
            def(REAL32, "VendorSpecificAttribute").hashed(),
            def(REAL16, "TextureCoordinates1").width(2),
        ],
    );
    let vd_t = w.typedef(1, &[def(REF_TO_ARRAY, "Vertices").child(vtx_t)]);
    let grp_t = w.typedef(
        1,
        &[
            def(INT32, "MaterialIndex"),
            def(INT32, "TriFirst"),
            def(INT32, "TriCount"),
        ],
    );
    let idx32_t = w.typedef(1, &[def(INT32, "Int32")]);
    let idx16_t = w.typedef(1, &[def(UINT16, "Int16")]);
    let topo_t = w.typedef(
        1,
        &[
            def(REF_TO_ARRAY, "Groups").child(grp_t),
            def(REF_TO_ARRAY, "Indices").child(idx32_t),
            def(REF_TO_ARRAY, "Indices16").child(idx16_t),
        ],
    );
    let mb_t = w.typedef(1, &[def(REFERENCE, "Material").child(mat_t)]);
    let bb_t = w.typedef(1, &[def(STRING, "BoneName")]);
    let mesh_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(REFERENCE, "PrimaryVertexData").child(vd_t),
            def(REFERENCE, "PrimaryTopology").child(topo_t),
            def(REF_TO_ARRAY, "MaterialBindings").child(mb_t),
            def(REF_TO_ARRAY, "BoneBindings").child(bb_t),
        ],
    );
    let meshbind_t = w.typedef(1, &[def(REFERENCE, "Mesh").child(mesh_t)]);
    let model_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(REFERENCE, "Skeleton").child(skel_t),
            def(TRANSFORM, "InitialPlacement"),
            def(REF_TO_ARRAY, "MeshBindings").child(meshbind_t),
        ],
    );
    let hdr_t = w.typedef(1, &[def(UINT8, "Format"), def(UINT8, "Degree")]);
    let d4c_t = w.typedef(
        1,
        &[
            def(INLINE, "CurveDataHeader_D4Constant32f").child(hdr_t),
            def(INT16, "Padding"),
            def(REAL32, "Controls").width(4),
        ],
    );
    let u8_t = w.typedef(1, &[def(UINT8, "UInt8")]);
    let d3k8_t = w.typedef(
        1,
        &[
            def(INLINE, "CurveDataHeader_D3K8uC8u").child(hdr_t),
            def(UINT16, "OneOverKnotScaleTrunc"),
            def(REAL32, "ControlScales").width(3),
            def(REAL32, "ControlOffsets").width(3),
            def(REF_TO_ARRAY, "KnotsControls").child(u8_t),
        ],
    );
    let ident_t = w.typedef(
        1,
        &[
            def(INLINE, "CurveDataHeader_DaIdentity").child(hdr_t),
            def(INT16, "Dimension"),
        ],
    );
    let curve_t = w.typedef(1, &[def(VARIANT, "CurveData")]);
    let track_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(UINT32, "Flags"),
            def(INLINE, "OrientationCurve").child(curve_t),
            def(INLINE, "PositionCurve").child(curve_t),
            def(INLINE, "ScaleShearCurve").child(curve_t),
        ],
    );
    let group_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(REF_TO_ARRAY, "TransformTracks").child(track_t),
            def(TRANSFORM, "InitialPlacement"),
            def(INT32, "AccumulationFlags"),
            def(REAL32, "LoopTranslation").width(3),
        ],
    );
    let anim_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(REAL32, "Duration"),
            def(REAL32, "TimeStep"),
            def(REAL32, "Oversampling"),
            def(ARRAY_OF_REFS, "TrackGroups").child(group_t),
        ],
    );
    let root_t = w.typedef(
        1,
        &[
            def(ARRAY_OF_REFS, "Materials").child(mat_t),
            def(ARRAY_OF_REFS, "Skeletons").child(skel_t),
            def(ARRAY_OF_REFS, "Meshes").child(mesh_t),
            def(ARRAY_OF_REFS, "Models").child(model_t),
            def(ARRAY_OF_REFS, "Animations").child(anim_t),
        ],
    );

    // Material: stripped strings, IDs in the pointer slots; one map with a texture.
    let texture = w.u32(2, ID_TEX_FILE);
    let submat = w.ptr(2, Some(texture));
    let maps = w.u32(2, ID_USAGE);
    w.ptr(2, Some(submat));
    let material = w.u32(2, ID_MATERIAL);
    w.u32(2, 1);
    w.ptr(2, Some(maps));

    // Skeleton: names are IDs written into the pointer slot without a fixup.
    let bones = w.pos(2);
    w.u32(2, ID_ROOT);
    w.u32(2, -1i32 as u32);
    transform(
        &mut w,
        7,
        [1.0, 2.0, 3.0],
        [0.0, 0.0, 0.0, 1.0],
        [2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 2.0],
    );
    floats(
        &mut w,
        &[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, -2.0, -3.0, 1.0,
        ],
    );
    floats(&mut w, &[0.5]);
    w.u32(2, ID_CHILD);
    w.u32(2, if o.bad_parent { 9 } else { 0 });
    let h = std::f32::consts::FRAC_1_SQRT_2;
    transform(
        &mut w,
        7,
        [0.0, 5.0, 0.0],
        [0.0, 0.0, h, h],
        [1.0, 0.0, 0.5, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    );
    floats(&mut w, &[0.0; 16]);
    floats(&mut w, &[0.0]);
    let skel_name = w.cstr(2, "skel");
    let skeleton = w.ptr(2, Some(skel_name));
    w.u32(2, 2);
    w.ptr(2, Some(bones));

    // Vertices: pos3, normal3, uv0 (2), weights u8x4, indices u8x4, unknown f32, uv1 half x2.
    let verts = w.pos(2);
    let vertex =
        |w: &mut Writer, p: [f32; 3], uv: [f32; 2], wt: [u8; 4], ix: [u8; 4], h1: [u16; 2]| {
            floats(w, &p);
            floats(w, &[0.0, 0.0, 1.0]);
            floats(w, &uv);
            w.bytes(2, &wt);
            w.bytes(2, &ix);
            floats(w, &[99.0]);
            for h in h1 {
                w.bytes(2, &h.to_le_bytes());
            }
        };
    vertex(
        &mut w,
        [0.0, 0.0, 0.0],
        [0.0, 0.0],
        [255, 0, 0, 0],
        [0, 0, 0, 0],
        [0, 0],
    );
    vertex(
        &mut w,
        [1.0, 0.0, 0.0],
        [1.0, 0.0],
        [128, 127, 0, 0],
        [0, 1, 0, 0],
        [0x3C00, 0x4000],
    );
    vertex(
        &mut w,
        [0.0, 1.0, 0.0],
        [0.0, 1.0],
        [0, 255, 0, 0],
        [1, 0, 0, 0],
        [0, 0],
    );
    let vd = w.u32(2, 3);
    w.ptr(2, Some(verts));

    let groups = w.pos(2);
    for v in [0u32, 0, 1] {
        w.u32(2, v);
    }
    let idx = w.pos(2);
    let last: u16 = if o.bad_index { 5 } else { 2 };
    for v in [0u16, 1, last, 0] {
        w.bytes(2, &v.to_le_bytes());
    }
    let topo = w.u32(2, 1);
    w.ptr(2, Some(groups));
    w.u32(2, 0);
    w.ptr(2, None);
    w.u32(2, 3);
    w.ptr(2, Some(idx));

    let mat_binding = w.ptr(2, Some(material));
    let bone_binding = w.u32(2, ID_CHILD);

    let mesh_name = w.cstr(2, "mesh0");
    let mesh = w.ptr(2, Some(mesh_name));
    w.ptr(2, Some(vd));
    w.ptr(2, Some(topo));
    w.u32(2, 1);
    w.ptr(2, Some(mat_binding));
    w.u32(2, 1);
    w.ptr(2, Some(bone_binding));

    let mesh_binding = w.ptr(2, Some(mesh));
    let model = w.ptr(2, None);
    w.ptr(2, Some(skeleton));
    transform(&mut w, 0, [0.0; 3], [0.0, 0.0, 0.0, 1.0], IDENT3);
    w.u32(2, 1);
    w.ptr(2, Some(mesh_binding));

    // Curves.
    let orient = w.bytes(2, &[5, 0, 0, 0]);
    floats(&mut w, &[0.0, 0.0, 0.0, 1.0]);
    let pos_knots = w.bytes(2, &[0, 100, 0, 0, 0, 10, 20, 30]);
    let position = w.bytes(2, &[11, 1]);
    w.bytes(2, &0x42C8u16.to_le_bytes());
    floats(&mut w, &[0.1, 0.1, 0.1, 1.0, 2.0, 3.0]);
    w.u32(2, 8);
    w.ptr(2, Some(pos_knots));
    let identity = w.bytes(2, &[2, 0, 9, 0]);
    let track = |w: &mut Writer, id: u32| {
        let t = w.u32(2, id);
        w.u32(2, 0);
        for (ty, data) in [(d4c_t, orient), (d3k8_t, position), (ident_t, identity)] {
            w.ptr(2, Some(ty));
            w.ptr(2, Some(data));
        }
        t
    };
    let tracks = track(&mut w, ID_CHILD);
    track(&mut w, ID_ROOT);
    track(&mut w, ID_STRAY);
    let group_name = w.cstr(2, "group");
    let group = w.ptr(2, Some(group_name));
    w.u32(2, 3);
    w.ptr(2, Some(tracks));
    transform(&mut w, 0, [0.0; 3], [0.0, 0.0, 0.0, 1.0], IDENT3);
    w.u32(2, 0);
    floats(&mut w, &[1.0, 2.0, 3.0]);
    let group_ref = w.ptr(2, Some(group));
    let anim_name = w.cstr(2, "anim");
    let anim = w.ptr(2, Some(anim_name));
    floats(&mut w, &[1.0, 0.1, 2.0]);
    w.u32(2, 1);
    w.ptr(2, Some(group_ref));

    // Reference tables and root.
    let one = |w: &mut Writer, target: Ptr| {
        let p = w.pos(2);
        w.ptr(2, Some(target));
        p
    };
    let t_mat = one(&mut w, material);
    let t_skel = one(&mut w, skeleton);
    let t_mesh = one(&mut w, mesh);
    let t_model = one(&mut w, model);
    let t_anim = one(&mut w, anim);
    let root = w.pos(0);
    for t in [t_mat, t_skel, t_mesh, t_model, t_anim] {
        w.u32(0, 1);
        w.ptr(0, Some(t));
    }
    w.build(root_t, root)
}

#[test]
fn reads_skeleton_with_id_only_names() {
    let data = typed_file(Opts::default());
    let f = GrannyFile::parse(&data).unwrap();
    let scene = Scene::read(&f).unwrap();
    let sk = &scene.skeletons[0];
    assert_eq!(sk.name.as_deref(), Some("skel"));
    assert_eq!(sk.bones.len(), 2);
    assert_eq!(
        (
            sk.bones[0].name.clone(),
            sk.bones[0].name_id,
            sk.bones[0].parent
        ),
        (None, Some(ID_ROOT), None)
    );
    assert_eq!(sk.bones[1].parent, Some(0));
    assert_eq!(sk.bones[0].lod_error, 0.5);
    assert_eq!(sk.find_bone_by_id(ID_CHILD), Some(1));
    assert_eq!(sk.find_bone_by_id(ID_STRAY), None);
    assert!(!sk.has_names());

    let t0 = sk.bones[0].local_trs();
    assert_eq!(
        (t0.translation, t0.scale, t0.shear_free),
        ([1.0, 2.0, 3.0], [2.0; 3], true)
    );
    assert!(!sk.bones[1].local_trs().shear_free);

    // Rest world of the child: parent translation + parent scale * local translation (0, 5, 0).
    let world = sk.rest_world();
    assert_eq!(world.len(), 2);
    assert!((world[1][12] - 1.0).abs() < 1e-5, "{:?}", world[1]);
    assert!((world[1][13] - 12.0).abs() < 1e-5, "{:?}", world[1]);
}

#[test]
fn rejects_bad_parent_index() {
    let data = typed_file(Opts {
        bad_parent: true,
        ..Opts::default()
    });
    let f = GrannyFile::parse(&data).unwrap();
    assert!(matches!(Scene::read(&f), Err(GrannyError::Invalid(m)) if m.contains("parent")));
}

#[test]
fn decodes_mesh_vertices_indices_groups_and_bindings() {
    let data = typed_file(Opts::default());
    let f = GrannyFile::parse(&data).unwrap();
    let scene = Scene::read(&f).unwrap();
    let m = &scene.meshes[0];
    assert_eq!(m.name.as_deref(), Some("mesh0"));
    assert_eq!(m.vertex_count(), 3);
    assert_eq!(
        m.positions,
        [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
    );
    assert_eq!(m.normals.as_ref().unwrap()[1], [0.0, 0.0, 1.0]);
    assert!(m.tangents.is_none());
    assert_eq!(m.uv.len(), 2);
    assert_eq!(m.uv[0][2], [0.0, 1.0]);
    assert_eq!(m.uv[1][1], [1.0, 2.0], "half-float UVs");
    assert_eq!(m.bone_weights.as_ref().unwrap()[1], [128, 127, 0, 0]);
    assert_eq!(m.bone_indices.as_ref().unwrap()[2], [1, 0, 0, 0]);
    assert!(m.is_skinned());
    assert_eq!(m.max_bone_index(), Some(1));
    assert_eq!(m.indices, [0, 1, 2]);
    assert_eq!(m.triangle_count(), 1);
    assert_eq!(m.groups.len(), 1);
    assert_eq!((m.groups[0].material_index, m.groups[0].tri_count), (0, 1));
    assert_eq!(m.material_bindings, [Some(0)]);
    assert_eq!(m.skipped_members.len(), 1);
    assert!(
        m.skipped_members[0].starts_with('#'),
        "{:?}",
        m.skipped_members
    );

    // The binding carries only an ID; the model's skeleton resolves it.
    assert_eq!(m.bone_bindings.len(), 1);
    assert_eq!(m.bone_bindings[0].name_id, Some(ID_CHILD));
    assert_eq!(m.bone_bindings[0].bone, Some(1));

    let model = &scene.models[0];
    assert_eq!((model.skeleton, &model.meshes[..]), (Some(0), &[0][..]));
}

#[test]
fn reads_materials_with_stripped_names() {
    let data = typed_file(Opts::default());
    let f = GrannyFile::parse(&data).unwrap();
    let scene = Scene::read(&f).unwrap();
    assert_eq!(scene.materials.len(), 1);
    let m = &scene.materials[0];
    assert_eq!((m.name.text.clone(), m.name.id), (None, Some(ID_MATERIAL)));
    // The type has no `Texture` member of its own.
    assert!(m.texture_file.is_empty());
    assert_eq!(m.maps.len(), 1);
    assert_eq!(m.maps[0].usage.id, Some(ID_USAGE));
    assert_eq!(m.maps[0].texture_file.id, Some(ID_TEX_FILE));
    assert!(m.maps[0].texture_file.text.is_none());
}

#[test]
fn rejects_out_of_range_triangle_index() {
    let data = typed_file(Opts {
        bad_index: true,
        ..Opts::default()
    });
    let f = GrannyFile::parse(&data).unwrap();
    assert!(
        matches!(Scene::read(&f), Err(GrannyError::Invalid(m)) if m.contains("index out of range"))
    );
}

#[test]
fn reads_animation_and_binds_tracks_by_id() {
    let data = typed_file(Opts::default());
    let f = GrannyFile::parse(&data).unwrap();
    let scene = Scene::read(&f).unwrap();
    let a = &scene.animations[0];
    assert_eq!((a.duration, a.time_step, a.oversampling), (1.0, 0.1, 2.0));
    assert_eq!(a.track_count(), 3);
    let g = &a.track_groups[0];
    assert_eq!(g.loop_translation, [1.0, 2.0, 3.0]);
    let t = &g.transform_tracks[0];
    assert_eq!(t.name_id, Some(ID_CHILD));
    // Position is a linear D3K8uC8u curve from (1,2,3) to (2,4,6) over one second.
    let p = t.sample(0.5);
    for (got, want) in p.translation.iter().zip([1.5, 3.0, 4.5]) {
        assert!((got - want).abs() < 1e-5, "{p:?}");
    }
    assert_eq!(p.rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(p.scale_shear, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    // Clamped outside the domain.
    assert_eq!(t.sample(-3.0).translation, [1.0, 2.0, 3.0]);
    assert_eq!(t.sample(9.0).translation, [2.0, 4.0, 6.0]);

    let sk = &scene.skeletons[0];
    let b = bind::bind(sk, g);
    assert_eq!(b.method, Method::Id);
    assert_eq!(b.track_to_bone, [Some(1), Some(0), None]);
    assert!(!b.is_complete());
    assert_eq!(b.unbound(), 1);
    assert_eq!(b.bone_to_track(2), [Some(1), Some(0)]);
    let errors = bind::pose_errors(sk, g, &b, 0.0);
    assert_eq!(errors.len(), 2);
}
