//! Typed-layer checks on real Civ5 files. Runs only with `CIV5_ROOT` set; use `--release`.
//!
//! Reads `Resource/Common/*.fpk` directly (a minimal directory reader, since `granny` cannot
//! depend on `asset-pipeline`) and loose Firaxis `.gr2` files under `Assets`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use granny::bind::{self, Method};
use granny::check::{FileStats, check_file};
use granny::skeleton::mat_mul;
use granny::{GrannyFile, Scene};

fn civ5_root() -> Option<PathBuf> {
    let root = std::env::var_os("CIV5_ROOT").map(PathBuf::from);
    if root.is_none() {
        println!("skipped: CIV5_ROOT not set");
    }
    root
}

fn is_mod(p: &Path) -> bool {
    p.to_string_lossy().to_ascii_lowercase().contains("lekmod")
}

fn walk_dir(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk_dir(&p, ext, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) && !is_mod(&p) {
            out.push(p);
        }
    }
}

/// FPK directory: `(name, offset, size)`; layout as documented in `asset-pipeline`'s `fpk.rs`.
fn fpk_entries(data: &[u8]) -> Vec<(String, usize, usize)> {
    let u32_at = |p: usize| -> Option<usize> {
        Some(u32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?) as usize)
    };
    let mut out = Vec::new();
    if data.get(..10) != Some(b"\x06\x00\x00\x00FPK_\x00\x00") {
        return out;
    }
    let (Some(count), mut pos) = (u32_at(10), 14usize) else {
        return out;
    };
    for _ in 0..count {
        let Some(len) = u32_at(pos) else { break };
        pos += 4;
        let Some(name) = data.get(pos..pos + len) else {
            break;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        pos += len;
        pos += (6 - pos % 4) % 4 + 8;
        let (Some(size), Some(offset)) = (u32_at(pos), u32_at(pos + 4)) else {
            break;
        };
        pos += 8;
        out.push((name, offset, size));
    }
    out
}

fn common_packs(root: &Path) -> Vec<PathBuf> {
    let mut packs = Vec::new();
    walk_dir(&root.join("Resource").join("Common"), "fpk", &mut packs);
    packs
}

/// `name -> bytes` for gr2 entries of all Common packs whose lower-case name satisfies `want`.
fn pack_files(root: &Path, want: impl Fn(&str) -> bool) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for pack in common_packs(root) {
        let data = std::fs::read(&pack).expect("read pack");
        for (name, offset, size) in fpk_entries(&data) {
            let lower = name.to_ascii_lowercase();
            if lower.ends_with(".gr2")
                && want(&lower)
                && let Some(b) = data.get(offset..offset + size)
            {
                out.insert(lower, b.to_vec());
            }
        }
    }
    out
}

fn identity_error(m: &[f32; 16]) -> f32 {
    (0..16)
        .map(|i| {
            let want = if i % 5 == 0 { 1.0 } else { 0.0 };
            (m[i] - want).abs()
        })
        .fold(0.0, f32::max)
}

#[test]
fn warrior_model_and_animations_decode_and_bind() {
    let Some(root) = civ5_root() else { return };
    let files = pack_files(&root, |n| {
        n == "warrior.gr2"
            || (n.starts_with("warrior_") && !n.starts_with("warrior_v") && n != "warrior_.gr2")
    });
    let model = files.get("warrior.gr2").expect("warrior.gr2 not found");
    let file = GrannyFile::parse(model).unwrap();
    let scene = Scene::read(&file).unwrap();

    assert_eq!(scene.meshes.len(), 1, "Warrior has one mesh");
    assert_eq!(scene.models.len(), 1);
    assert_eq!(scene.skeletons.len(), 1);
    let mesh = &scene.meshes[0];
    let skel = &scene.skeletons[0];
    assert!(mesh.is_skinned());
    assert!(mesh.vertex_count() > 0 && mesh.triangle_count() > 0);
    assert!(!mesh.groups.is_empty());
    println!(
        "Warrior: {} bones, {} bone bindings, {} vertices, {} triangles, {} groups, {} material bindings",
        skel.bones.len(),
        mesh.bone_bindings.len(),
        mesh.vertex_count(),
        mesh.triangle_count(),
        mesh.groups.len(),
        mesh.material_bindings.len()
    );
    assert!(skel.bones.len() <= 128, "WebGL2 palette limit");

    // Every vertex bone index selects a binding, and each binding resolves to a bone.
    let max = usize::from(mesh.max_bone_index().unwrap());
    assert!(max < mesh.bone_bindings.len());
    assert!(mesh.bone_bindings.iter().all(|b| b.bone.is_some()));
    let mut used: Vec<_> = mesh.bone_bindings.iter().filter_map(|b| b.bone).collect();
    used.sort_unstable();
    used.dedup();
    assert_eq!(
        used.len(),
        mesh.bone_bindings.len(),
        "bindings are distinct bones"
    );

    // The transform convention is right if rest_world * inverse_world is the identity.
    let world = skel.rest_world();
    for (i, (w, b)) in world.iter().zip(&skel.bones).enumerate() {
        let e = identity_error(&mat_mul(&b.inverse_world, w));
        assert!(e < 0.05, "bone {i}: inverse_world * rest_world off by {e}");
    }

    let anims: Vec<_> = files.iter().filter(|(n, _)| *n != "warrior.gr2").collect();
    assert!(
        anims.len() >= 30,
        "found {} Warrior animations",
        anims.len()
    );
    let mut durations = Vec::new();
    for (name, bytes) in &anims {
        let f = GrannyFile::parse(bytes).unwrap();
        let s = Scene::read(&f).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(s.animations.len(), 1, "{name}");
        let a = &s.animations[0];
        durations.push((name.trim_end_matches(".gr2").to_string(), a.duration));
        let g = &a.track_groups[0];
        let b = bind::bind(skel, g);
        assert_eq!(b.method, Method::Id, "{name}");
        assert!(b.is_complete(), "{name}: {:?}", b.track_to_bone);
        assert_eq!(g.transform_tracks.len(), skel.bones.len(), "{name}");
        let errors = bind::pose_errors(skel, g, &b, 0.0);
        let close = errors.iter().filter(|(dp, _)| *dp < 1.0).count();
        assert!(
            close * 10 >= errors.len() * 8,
            "{name}: only {close}/{} tracks start near the rest translation",
            errors.len()
        );
        check_file(bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
    println!("Warrior animations ({}):", durations.len());
    for (n, d) in &durations {
        println!("  {n}: {d:.3}s");
    }
}

/// W2: at least 99.5 % of all gr2 files decode, sample and validate without errors.
#[test]
fn whole_game_typed_decode_ratio() {
    let Some(root) = civ5_root() else { return };
    let mut loose = Vec::new();
    walk_dir(&root.join("Assets"), "gr2", &mut loose);
    let packs = common_packs(&root);
    let next = Mutex::new((packs.into_iter(), loose.into_iter()));
    let stats = Mutex::new((
        0usize,
        0usize,
        FileStats::default(),
        BTreeMap::<String, usize>::new(),
    ));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let (pack, file) = {
                        let mut n = next.lock().unwrap();
                        match n.0.next() {
                            Some(p) => (Some(p), None),
                            None => (None, n.1.next()),
                        }
                    };
                    let mut work: Vec<(String, Vec<u8>)> = Vec::new();
                    if let Some(p) = pack {
                        let data = std::fs::read(&p).expect("read pack");
                        for (name, off, size) in fpk_entries(&data) {
                            if name.to_ascii_lowercase().ends_with(".gr2")
                                && let Some(b) = data.get(off..off + size)
                            {
                                work.push((name, b.to_vec()));
                            }
                        }
                    } else if let Some(p) = file {
                        let data = std::fs::read(&p).expect("read gr2");
                        work.push((p.display().to_string(), data));
                    } else {
                        break;
                    }
                    for (name, bytes) in work {
                        let r = std::panic::catch_unwind(|| check_file(&bytes));
                        let mut st = stats.lock().unwrap();
                        st.0 += 1;
                        match r {
                            Ok(Ok(c)) => {
                                st.1 += 1;
                                st.2.add(&c.stats);
                            }
                            Ok(Err(e)) => {
                                let key = format!("{e} (e.g. {name})");
                                *st.3.entry(key).or_default() += 1;
                            }
                            Err(_) => panic!("panic while decoding {name}"),
                        }
                    }
                }
            });
        }
    });
    let (total, ok, agg, errors) = stats.into_inner().unwrap();
    let ratio = ok as f64 / total.max(1) as f64;
    println!(
        "gr2 files: {total}, decoded: {ok} ({:.3}%); {} meshes, {} skeletons, {} animations, {} tracks",
        ratio * 100.0,
        agg.meshes,
        agg.skeletons,
        agg.animations,
        agg.tracks
    );
    println!("curve formats: {:?}", agg.curve_formats);
    for (e, n) in &errors {
        println!("  x{n}: {e}");
    }
    assert!(total > 0);
    assert!(ratio >= 0.995, "typed decode ratio {ratio:.4} below 99.5%");
}
