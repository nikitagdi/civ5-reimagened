//! Model pipeline tests. The synthetic GLB test always runs; the Warrior ones need `CIV5_ROOT`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{
    AnimationBlob, ContentHash, Indices, LogicalId, Manifest, Material, Mesh, ModelBlob,
    PlatformProfile, TextureBlob, TextureFormat, Transform, VariantFormat, f16_bits_to_f32,
    oct_decode,
};
use asset_pipeline::game::GameSource;
use asset_pipeline::models::convert::winding_agreement;
use asset_pipeline::models::set::{self, BuildSummary, ENGINE_UNITS_PER_CIV5_UNIT};
use asset_pipeline::models::{self, build_set};

const WARRIOR: &str = "ART_DEF_UNIT_MEMBER_WARRIOR";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-models-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn real_source() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(&root).expect("scan failed"))
}

fn build_warrior(src: &GameSource, out: &Path) -> BuildSummary {
    build_set(src, set::find_set("warrior").unwrap(), out).unwrap()
}

fn read_manifest(out: &Path) -> (ContentHash, Manifest) {
    let text = std::fs::read_to_string(out.join("manifest")).unwrap();
    let hash: ContentHash = text.trim().parse().unwrap();
    let bytes = std::fs::read(out.join(hash.to_string())).unwrap();
    (hash, Manifest::decode(&bytes).unwrap())
}

fn blob(out: &Path, m: &Manifest, id: &str, profile: &PlatformProfile) -> Vec<u8> {
    let v = m
        .resolve(&LogicalId::new(id), profile)
        .unwrap_or_else(|| panic!("{id} not in the manifest"));
    let bytes = std::fs::read(out.join(v.hash.to_string())).unwrap();
    assert_eq!(bytes.len() as u64, v.size, "{id}: manifest size");
    assert_eq!(*blake3::hash(&bytes).as_bytes(), v.hash.0, "{id}: hash");
    bytes
}

// --- tiny column-major matrix helpers (independent of asset-format's math types) ---

type M4 = [f32; 16];

fn mul(a: &M4, b: &M4) -> M4 {
    let mut m = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            m[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    m
}

fn trs(t: &Transform) -> M4 {
    let [x, y, z, w] = t.rotation;
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut m = [0.0; 16];
    for row in 0..3 {
        for col in 0..3 {
            m[col * 4 + row] = r[row][col] * t.scale[col];
        }
        m[12 + row] = t.translation[row];
    }
    m[15] = 1.0;
    m
}

fn apply(m: &M4, p: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * p[0] + m[4 + r] * p[1] + m[8 + r] * p[2] + m[12 + r])
}

/// `world * inverse_bind` per bone for the local pose `local`.
fn skin_matrices(model: &ModelBlob, local: &[Transform]) -> Vec<M4> {
    let mut world: Vec<M4> = Vec::new();
    for (i, b) in model.skeleton.bones.iter().enumerate() {
        let m = trs(&local[i]);
        let w = match b.parent {
            Some(p) => mul(&world[usize::from(p)], &m),
            None => m,
        };
        world.push(w);
    }
    world
        .iter()
        .zip(&model.skeleton.bones)
        .map(|(w, b)| mul(w, &b.inverse_bind))
        .collect()
}

fn skinned_positions(mesh: &Mesh, skin: &[M4]) -> Vec<[f32; 3]> {
    mesh.positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut out = [0.0f32; 3];
            for k in 0..4 {
                let w = f32::from(mesh.weights[i][k]) / 255.0;
                let q = apply(&skin[usize::from(mesh.joints[i][k])], *p);
                for c in 0..3 {
                    out[c] += w * q[c];
                }
            }
            out
        })
        .collect()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

// --- W4 ---

#[test]
fn build_twice_gives_identical_manifest_and_writes_nothing_the_second_time() {
    let Some(src) = real_source() else { return };
    let tmp = TempDir::new();
    let first = build_warrior(&src, tmp.path());
    let (hash1, m1) = read_manifest(tmp.path());
    assert_eq!(hash1, first.manifest);
    assert!(first.stats.blobs_written > 30, "{:?}", first.stats);
    assert_eq!(first.stats.cache_misses, 3, "2 textures + 1 model step");
    assert_eq!(first.stats.manifest_writes, 1);

    let second = build_warrior(&src, tmp.path());
    let (hash2, m2) = read_manifest(tmp.path());
    assert_eq!(hash1, hash2, "manifest hash must not change");
    assert_eq!(m1, m2);
    assert_eq!(second.stats.blobs_written, 0, "{:?}", second.stats);
    assert_eq!(second.stats.bytes_written, 0);
    assert_eq!(second.stats.manifest_writes, 0);
    assert_eq!(second.stats.cache_hits, 3);
    assert_eq!(second.stats.cache_misses, 0);
    println!(
        "first {:.2}s {:?}\nsecond {:.2}s {:?}",
        first.seconds, first.stats, second.seconds, second.stats
    );

    // A fresh directory must reproduce the same blobs (determinism, not just caching).
    let other = TempDir::new();
    let third = build_warrior(&src, other.path());
    assert_eq!(third.manifest, hash1);
}

#[test]
fn deleting_a_blob_invalidates_its_cache_entry() {
    let Some(src) = real_source() else { return };
    let tmp = TempDir::new();
    let first = build_warrior(&src, tmp.path());
    let (_, m) = read_manifest(tmp.path());
    let model = m
        .variants(&LogicalId::new("model:art_def_unit_member_warrior"))
        .first()
        .unwrap()
        .hash;
    std::fs::remove_file(tmp.path().join(model.to_string())).unwrap();
    let again = build_warrior(&src, tmp.path());
    assert_eq!(again.manifest, first.manifest);
    assert_eq!(again.stats.cache_misses, 1);
    assert!(tmp.path().join(model.to_string()).is_file());
}

// --- model contents ---

#[test]
fn warrior_model_is_complete_upright_and_rests_on_its_bind_pose() {
    let Some(src) = real_source() else { return };
    let tmp = TempDir::new();
    build_warrior(&src, tmp.path());
    let (_, m) = read_manifest(tmp.path());
    let profile = PlatformProfile::uncompressed_only();
    let model = ModelBlob::decode(&blob(
        tmp.path(),
        &m,
        "model:art_def_unit_member_warrior",
        &profile,
    ))
    .unwrap();

    let bones = model.skeleton.bones.len();
    assert_eq!(bones, 23);
    assert_eq!(model.meshes.len(), 1);
    let mesh = &model.meshes[0];
    assert_eq!(mesh.positions.len(), 863);
    assert_eq!(mesh.indices.len() / 3, 674);
    assert!(matches!(mesh.indices, Indices::U16(_)));
    assert!(
        mesh.joints
            .iter()
            .flatten()
            .all(|&j| usize::from(j) < bones)
    );
    assert!(
        mesh.weights
            .iter()
            .all(|w| w.iter().map(|&x| u32::from(x)).sum::<u32>() == 255)
    );
    // The install lists 31 clips in warrior.fxsxml.
    assert_eq!(model.animations.len(), 31);
    assert!(
        model
            .animations
            .iter()
            .any(|a| a.name == "Warrior_IdleA" && a.events.iter().any(|e| e.code == 1000))
    );
    let attack_city = model
        .animations
        .iter()
        .find(|a| a.name == "Warrior_Attack_City")
        .unwrap();
    assert_eq!(
        attack_city
            .events
            .iter()
            .map(|e| e.code)
            .collect::<Vec<_>>(),
        [1160, 1180]
    );
    assert_eq!(
        model.materials,
        [Material {
            diffuse: Some(LogicalId::new("tex:warrior_diff.dds")),
            specular: Some(LogicalId::new("tex:warrior_sref.dds")),
        }]
    );
    assert_eq!(mesh.material, 0);

    // Engine axes: Y up, feet on the ground, taller than wide, faces point outward.
    let (lo, hi) = bounds(&mesh.positions);
    println!("bounds {lo:?} .. {hi:?}");
    assert!(lo[1].abs() < 2.0, "feet at y = {}", lo[1]);
    assert!(hi[1] > 100.0 && hi[1] < 140.0, "height {}", hi[1]);
    assert!(hi[1] - lo[1] > hi[2] - lo[2], "taller than deep");
    assert!(winding_agreement(mesh) > 0.99);
    let n0 = oct_decode(mesh.normals[0]);
    assert!((n0.iter().map(|c| c * c).sum::<f32>() - 1.0).abs() < 1e-3);
    // V flip: all UVs of this model lie in the unit square.
    for uv in &mesh.uvs {
        let (u, v) = (f16_bits_to_f32(uv[0]), f16_bits_to_f32(uv[1]));
        assert!(
            (-0.01..=1.01).contains(&u) && (-0.01..=1.01).contains(&v),
            "{u} {v}"
        );
    }

    // Bind pose: world(rest) * inverse_bind is identity for every bone.
    let rest = model.skeleton.rest_pose();
    let skin = skin_matrices(&model, &rest);
    let identity = trs(&Transform::IDENTITY);
    for (i, s) in skin.iter().enumerate() {
        for (a, b) in s.iter().zip(&identity) {
            assert!((a - b).abs() < 5e-3, "bone {i}: {s:?}");
        }
    }
    let at_rest = skinned_positions(mesh, &skin);
    for (a, b) in at_rest.iter().zip(&mesh.positions) {
        assert!(dist(*a, *b) < 0.05);
    }

    // IdleA at t = 0 is a stance, not the bind pose: the torso stays close, arm and club move.
    let idle = model
        .animations
        .iter()
        .find(|a| a.name == "Warrior_IdleA")
        .unwrap();
    let clip = AnimationBlob::decode(&blob(tmp.path(), &m, idle.blob.as_str(), &profile)).unwrap();
    assert_eq!(clip.sample_rate, 30.0);
    assert!(
        clip.duration > 0.5 && clip.duration < 20.0,
        "{}",
        clip.duration
    );
    let mut pose = rest.clone();
    clip.sample(0.0, false, &rest, &mut pose);
    let skinned = skinned_positions(mesh, &skin_matrices(&model, &pose));
    let mut moved: Vec<f32> = skinned
        .iter()
        .zip(&mesh.positions)
        .map(|(a, b)| dist(*a, *b))
        .collect();
    moved.sort_by(f32::total_cmp);
    let pct = |p: usize| moved[moved.len() * p / 100];
    println!(
        "idle t=0 displacement from bind: p10 {:.1} p50 {:.1} p90 {:.1} max {:.1}",
        pct(10),
        pct(50),
        pct(90),
        moved.last().unwrap()
    );
    assert!(pct(50) < 15.0, "median displacement {}", pct(50));
    assert!(
        *moved.last().unwrap() < 150.0,
        "the arm and club swing furthest"
    );

    // Every clip, sampled across its length, stays finite and near the model.
    let mut total_bytes = 0;
    for a in &model.animations {
        let bytes = blob(tmp.path(), &m, a.blob.as_str(), &profile);
        total_bytes += bytes.len();
        let clip = AnimationBlob::decode(&bytes).unwrap();
        let mut pose = rest.clone();
        for step in 0..=8 {
            let t = clip.duration * step as f32 / 8.0;
            clip.sample(t, false, &rest, &mut pose);
            let s = skinned_positions(mesh, &skin_matrices(&model, &pose));
            let (plo, phi) = bounds(&s);
            for k in 0..3 {
                assert!(
                    plo[k] > -400.0 && phi[k] < 400.0,
                    "{} t={t}: {plo:?} {phi:?}",
                    a.name
                );
            }
        }
    }
    println!("animation blobs: {total_bytes} bytes in total");
}

#[test]
fn warrior_textures_have_three_variants_with_full_mip_chains() {
    let Some(src) = real_source() else { return };
    let tmp = TempDir::new();
    build_warrior(&src, tmp.path());
    let (_, m) = read_manifest(tmp.path());
    for (name, srgb) in [
        ("tex:warrior_diff.dds", true),
        ("tex:warrior_sref.dds", false),
    ] {
        let variants = m.variants(&LogicalId::new(name));
        let formats: Vec<_> = variants.iter().map(|v| v.format).collect();
        assert_eq!(
            formats,
            [
                VariantFormat::Uncompressed,
                VariantFormat::Bc,
                VariantFormat::Astc
            ],
            "{name}"
        );
        for (v, want) in variants.iter().zip([
            TextureFormat::Rgba8,
            TextureFormat::Bc7,
            TextureFormat::Astc4x4,
        ]) {
            assert_eq!(v.prefetch.0, 1);
            let bytes = std::fs::read(tmp.path().join(v.hash.to_string())).unwrap();
            let t = TextureBlob::decode(&bytes).unwrap();
            assert_eq!((t.format, t.srgb), (want, srgb), "{name}");
            let full = t.width.max(t.height).ilog2() as usize + 1;
            assert_eq!(t.mips.len(), full, "{name} {want:?}: full mip chain");
            println!("{name} {want:?}: {}x{} {} bytes", t.width, t.height, v.size);
        }
    }
    assert!(
        m.variants(&LogicalId::new("model:art_def_unit_member_warrior"))
            .iter()
            .all(|v| v.prefetch.0 == 1 && v.format == VariantFormat::Generic)
    );
}

// --- W5 ---

fn assert_glb_shape(glb: &[u8], joints: usize, animations: usize) -> gltf::Document {
    let (doc, buffers, images) = gltf::import_slice(glb).expect("glTF must load");
    assert_eq!(doc.skins().count(), 1);
    let skin = doc.skins().next().unwrap();
    assert_eq!(skin.joints().count(), joints);
    assert_eq!(doc.animations().count(), animations);
    assert_eq!(doc.meshes().count(), 1);
    assert_eq!(images.len(), 1);
    // Inverse bind matrices accessor: one mat4 per joint.
    let reader = skin.reader(|b| Some(&buffers[b.index()]));
    assert_eq!(reader.read_inverse_bind_matrices().unwrap().count(), joints);
    // Every primitive has skinning attributes and valid indices.
    for p in doc.meshes().next().unwrap().primitives() {
        let r = p.reader(|b| Some(&buffers[b.index()]));
        let n = r.read_positions().unwrap().count();
        assert_eq!(r.read_joints(0).unwrap().into_u16().count(), n);
        assert_eq!(r.read_weights(0).unwrap().into_f32().count(), n);
        assert!(
            r.read_indices()
                .unwrap()
                .into_u32()
                .all(|i| (i as usize) < n)
        );
    }
    doc
}

fn synthetic_model() -> (ModelBlob, Vec<(String, AnimationBlob)>, TextureBlob) {
    use asset_format::{Bone, Skeleton, Track, quat_pack};
    let bone = |name: &str, parent: Option<u16>, t: [f32; 3]| Bone {
        name: name.into(),
        parent,
        rest: Transform {
            translation: t,
            ..Transform::IDENTITY
        },
        inverse_bind: {
            let mut m = trs(&Transform::IDENTITY);
            m[12] = -t[0];
            m[13] = -t[1];
            m[14] = -t[2];
            m
        },
    };
    let model = ModelBlob {
        skeleton: Skeleton {
            bones: vec![
                bone("a", None, [0.0; 3]),
                bone("b", Some(0), [0.0, 1.0, 0.0]),
            ],
        },
        meshes: vec![Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 2.0, 0.0]],
            normals: vec![[0, 0]; 3],
            uvs: vec![[0, 0], [0x3c00, 0], [0, 0x3c00]],
            joints: vec![[0, 1, 0, 0]; 3],
            weights: vec![[128, 127, 0, 0]; 3],
            indices: Indices::U16(vec![0, 1, 2]),
            material: 0,
        }],
        materials: vec![Material {
            diffuse: Some(LogicalId::new("tex:x.dds")),
            specular: None,
        }],
        animations: Vec::new(),
    };
    let clip = |n: &str| {
        (
            n.to_owned(),
            AnimationBlob {
                duration: 2.0 / 30.0,
                sample_rate: 30.0,
                tracks: vec![
                    Track {
                        bone: 1,
                        rotations: vec![quat_pack([0.0, 0.0, 0.0, 1.0]); 3],
                        translations: vec![[0.0, 1.0, 0.0], [0.0, 1.5, 0.0], [0.0, 2.0, 0.0]],
                        scales: vec![[1.0; 3]],
                    },
                    Track {
                        bone: 0,
                        rotations: vec![quat_pack([
                            0.0,
                            0.0,
                            std::f32::consts::FRAC_1_SQRT_2,
                            std::f32::consts::FRAC_1_SQRT_2,
                        ])],
                        translations: vec![],
                        scales: vec![],
                    },
                ],
            },
        )
    };
    let pixels: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 3) as u8).collect();
    let tex = TextureBlob {
        format: TextureFormat::Rgba8,
        dim: asset_format::TexDim::D2,
        srgb: true,
        width: 4,
        height: 4,
        depth: 1,
        mips: vec![pixels],
    };
    (model, vec![clip("one"), clip("two")], tex)
}

#[test]
fn synthetic_glb_loads_and_keeps_pixels() {
    let (model, clips, tex) = synthetic_model();
    let textures = [(LogicalId::new("tex:x.dds"), tex.clone())];
    let glb = models::write_glb(&model, &clips, &textures, 0.5).unwrap();
    let doc = assert_glb_shape(&glb, 2, 2);
    let (_, _, images) = gltf::import_slice(&glb).unwrap();
    assert_eq!((images[0].width, images[0].height), (4, 4));
    assert_eq!(images[0].pixels, tex.mips[0]);
    // The armature node carries the scale.
    let armature = doc.nodes().find(|n| n.name() == Some("armature")).unwrap();
    assert_eq!(armature.transform().decomposed().2, [0.5; 3]);
    // Per clip: rotation, translation and scale of bone 1 plus the rotation of bone 0.
    let anim = doc.animations().next().unwrap();
    assert_eq!(anim.channels().count(), 4);
}

#[test]
fn warrior_glb_loads_with_one_skin_and_all_animations() {
    let Some(src) = real_source() else { return };
    let started = std::time::Instant::now();
    let glb = set::export_member_glb(&src, WARRIOR).unwrap();
    println!(
        "glb: {} bytes in {:.2}s",
        glb.len(),
        started.elapsed().as_secs_f64()
    );
    let doc = assert_glb_shape(&glb, 23, 31);
    let (_, _, images) = gltf::import_slice(&glb).unwrap();
    assert!(images[0].width >= 128);
    // Armature scale = fScale * engine units per Civ5 unit.
    let armature = doc.nodes().find(|n| n.name() == Some("armature")).unwrap();
    let s = armature.transform().decomposed().2[0];
    assert!((s - 0.14 * ENGINE_UNITS_PER_CIV5_UNIT).abs() < 1e-6, "{s}");
}

// --- B7: static (baked) models ---

fn apply_dir(m: &M4, d: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|r| m[r] * d[0] + m[4 + r] * d[1] + m[8 + r] * d[2])
}

/// Normals through the same weighted palette, renormalized (what the client shader does).
fn skinned_normals(mesh: &Mesh, skin: &[M4]) -> Vec<[f32; 3]> {
    (0..mesh.normals.len())
        .map(|i| {
            let n = oct_decode(mesh.normals[i]);
            let mut out = [0.0f32; 3];
            for k in 0..4 {
                let w = f32::from(mesh.weights[i][k]) / 255.0;
                let q = apply_dir(&skin[usize::from(mesh.joints[i][k])], n);
                for c in 0..3 {
                    out[c] += w * q[c];
                }
            }
            let len = out.iter().map(|x| x * x).sum::<f32>().sqrt();
            out.map(|x| x / len)
        })
        .collect()
}

#[test]
fn warrior_static_model_equals_the_idle_pose_of_the_animation_path() {
    let Some(src) = real_source() else { return };
    let defs = asset_pipeline::art::ArtDefines::load(&src).unwrap();
    let loaded = set::load_member(&src, &defs, WARRIOR).unwrap();
    let (stat, blobs) = loaded.convert_static().unwrap();
    assert!(stat.warnings.is_empty(), "{:?}", stat.warnings);
    assert!(
        stat.dropped.is_empty(),
        "the Warrior has no damage-state hull"
    );
    assert_eq!(stat.clip.as_deref(), Some("Warrior_IdleA"));
    let s = &stat.model;
    assert!(s.skeleton.bones.is_empty() && s.animations.is_empty());
    assert!(
        s.meshes
            .iter()
            .all(|m| m.joints.is_empty() && m.weights.is_empty())
    );
    assert_eq!(s.materials, blobs.model.materials);
    assert!(ModelBlob::decode(&s.encode()).is_ok());

    // The reference pose: IdleA sampled at 0 through independent matrix code.
    let model = &blobs.model;
    let (_, clip) = models::bake::idle_clip(model, &blobs.clips).unwrap();
    let rest = model.skeleton.rest_pose();
    let mut pose = rest.clone();
    clip.sample(0.0, false, &rest, &mut pose);
    let skin = skin_matrices(model, &pose);
    assert_eq!(s.meshes.len(), model.meshes.len());
    let (mut worst_p, mut worst_n) = (0.0f32, 0.0f32);
    for (baked, orig) in s.meshes.iter().zip(&model.meshes) {
        assert_eq!(baked.indices, orig.indices);
        assert_eq!(baked.uvs, orig.uvs);
        for (a, b) in baked.positions.iter().zip(skinned_positions(orig, &skin)) {
            worst_p = worst_p.max(dist(*a, b));
        }
        for (a, b) in baked.normals.iter().zip(skinned_normals(orig, &skin)) {
            worst_n = worst_n.max(dist(oct_decode(*a), b));
        }
    }
    println!("baked vs animated: worst position error {worst_p:e}, worst normal error {worst_n:e}");
    assert!(worst_p < 0.01, "position error {worst_p}");
    assert!(worst_n < 5e-3, "normal error {worst_n}");
    // The pose is not the bind pose.
    let moved = s.meshes[0]
        .positions
        .iter()
        .zip(&model.meshes[0].positions)
        .any(|(a, b)| dist(*a, *b) > 1.0);
    assert!(moved, "idle frame 0 must differ from the bind pose");
}

/// R5.11 (answers-b update 9): which node shows in which state comes from the model file. The
/// trireme: two damaged pieces (states 1..7), two meshes in every state, the intact hull (state 0)
/// and a damaged hull for the three damage levels; the battleship: the damaged hull and the intact
/// one. Needs the install (`CIV5_ROOT`).
#[test]
fn the_ships_carry_the_node_states_of_their_hulls() {
    let Some(src) = real_source() else { return };
    for (file, expect) in [
        ("trireme.gr2", vec![0xfe, 0xfe, 0xff, 0xff, 0x01, 0x0e]),
        ("battleship.gr2", vec![0xfe, 0x01]),
    ] {
        let bytes = src.read_by_name(file).unwrap().expect(file);
        let gr2 = granny::GrannyFile::parse(&bytes).unwrap();
        let scene = granny::Scene::read(&gr2).unwrap();
        let states: Vec<u8> = scene.meshes.iter().map(|m| m.state_mask).collect();
        assert_eq!(states, expect, "{file}");
    }
}

/// Prints which models lose which meshes. Needs the install (`CIV5_ROOT`).
#[test]
fn every_model_with_a_damage_state_hull_loses_exactly_those_meshes() {
    let Some(src) = real_source() else { return };
    let defs = asset_pipeline::art::ArtDefines::load(&src).unwrap();
    let (mut checked, mut failed) = (0usize, 0usize);
    for id in defs.members.values().map(|m| m.id.clone()) {
        let Ok(loaded) = set::load_member(&src, &defs, &id) else {
            failed += 1;
            continue;
        };
        let damaged = loaded.damage_materials();
        if damaged.is_empty() {
            continue;
        }
        let (stat, blobs) = loaded
            .convert_static()
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        checked += 1;
        let list: Vec<String> = stat
            .dropped
            .iter()
            .map(|d| {
                format!(
                    "mesh {} ({} tris, {})",
                    d.mesh,
                    d.triangles,
                    d.diffuse.as_ref().map_or("-", |x| x.as_str())
                )
            })
            .collect();
        println!(
            "DAMAGE {id}: {} of {} meshes dropped: {}",
            stat.dropped.len(),
            blobs.model.meshes.len(),
            list.join("; ")
        );
        assert!(
            !stat.dropped.is_empty(),
            "{id}: no mesh uses a damage-state material"
        );
        assert!(!stat.model.meshes.is_empty(), "{id}: nothing left");
        assert_eq!(
            stat.model.meshes.len() + stat.dropped.len(),
            blobs.model.meshes.len()
        );
        for m in &stat.model.materials {
            let d = m.diffuse.as_ref().map_or("", |x| x.as_str());
            assert!(
                !asset_pipeline::models::materials::is_damage_texture(d),
                "{id}: {d} left in"
            );
        }
        assert!(ModelBlob::decode(&stat.model.encode()).is_ok(), "{id}");
    }
    println!("models with damage-state hulls: {checked}; members not loadable: {failed}");
}
