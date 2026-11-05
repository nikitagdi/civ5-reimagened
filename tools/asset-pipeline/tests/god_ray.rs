//! The god ray model (`FXM_God_Ray.fxsxml`, item 31) through the model builder of `b2-misc`.
//! Needs `CIV5_ROOT` (the install); skipped without it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::PrefetchGroup;
use asset_pipeline::game::GameSource;
use asset_pipeline::models::merge::MergeScope;
use asset_pipeline::sets::b2_misc::{self, GOD_RAY_MODEL};
use asset_pipeline::sets::b2_models::{ModelJob, Options, build_models};
use asset_pipeline::store::{BlobStore, ManifestBuilder};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-god-ray-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_god_ray_builds_as_an_fx_funnel_of_about_357_units() {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return;
    };
    let game = GameSource::open(&root).expect("scan failed");
    assert!(
        game.lookup(GOD_RAY_MODEL).is_some(),
        "the install has no {GOD_RAY_MODEL}"
    );
    let p = b2_misc::plan(&|n: &str| game.lookup(n).is_some()).unwrap();
    assert!(p.god_ray);
    let job: ModelJob = b2_misc::model_jobs(&p)
        .into_iter()
        .find(|j| j.flat == GOD_RAY_MODEL)
        .unwrap();
    assert_eq!(job.scope, MergeScope::Model);

    let out = TempDir::new();
    let store = BlobStore::open(&out.0).unwrap();
    let mut manifest = ManifestBuilder::open(&store).unwrap();
    let built = build_models(
        &game,
        &[job],
        &store,
        &mut manifest,
        &Options::default(),
        PrefetchGroup(0),
    )
    .unwrap();
    assert!(built.failed.is_empty(), "{:?}", built.failed);
    let (_, b) = &built.good[0];
    let m = &b.model;
    println!("bounds {:?}", m.bounds);
    println!(
        "materials {:?}",
        m.materials.iter().map(|x| &x.package).collect::<Vec<_>>()
    );
    println!("textures {:?}", built.texture_files);
    println!(
        "meshes {} skinned {:?} bind {:?}",
        m.meshes.len(),
        m.meshes.iter().map(|x| x.vertex_count).collect::<Vec<_>>(),
        m.meshes.iter().map(|x| x.bind).collect::<Vec<_>>()
    );
    assert_eq!(m.materials[0].package, "FXShader");
    // The funnel is about 357 units tall, whichever axis is up in the baked basis, and 81 across.
    let [x0, y0, z0, x1, y1, z1] = m.bounds;
    let ext = [x1 - x0, y1 - y0, z1 - z0];
    let tall = ext.iter().cloned().fold(0.0f32, f32::max);
    assert!((300.0..420.0).contains(&tall), "{ext:?}");
    // Up is Y in our basis.
    assert!(ext[1] > ext[0] && ext[1] > ext[2], "{ext:?}");
    // All six textures of the three layers ship (re-b11 3.1): base_01/02, UV_01/02/03 and alpha_01.
    for t in [
        "fx_god_ray_base_01.dds",
        "fx_god_ray_base_02.dds",
        "fx_god_ray_uv_01.dds",
        "fx_god_ray_uv_02.dds",
        "fx_god_ray_uv_03.dds",
        "fx_god_ray_alpha_01.dds",
    ] {
        assert!(built.texture_files.contains(t), "{t}");
    }
    // The layers read the textures re-b11 names: layer 0 base_01/UV_01, layer 1 base_02/UV_03 (the
    // crossing), layer 2 base_02/UV_02, all on alpha_01.
    let want = [
        ("BaseTextureMap0", "fx_god_ray_base_01.dds"),
        ("UVScrollingMap0", "fx_god_ray_uv_01.dds"),
        ("AlphaLookupMap0", "fx_god_ray_alpha_01.dds"),
        ("BaseTextureMap1", "fx_god_ray_base_02.dds"),
        ("UVScrollingMap1", "fx_god_ray_uv_03.dds"),
        ("AlphaLookupMap1", "fx_god_ray_alpha_01.dds"),
        ("BaseTextureMap2", "fx_god_ray_base_02.dds"),
        ("UVScrollingMap2", "fx_god_ray_uv_02.dds"),
        ("AlphaLookupMap2", "fx_god_ray_alpha_01.dds"),
    ];
    let bound: Vec<(String, String)> = m.materials[0]
        .sets
        .iter()
        .flat_map(|s| &s.textures)
        .map(|t| (t.name.clone(), format!("{:?}", t.src)))
        .collect();
    println!("layer textures {bound:?}");
    for (slot, file) in want {
        let (_, src) = bound
            .iter()
            .find(|(n, _)| n == slot)
            .unwrap_or_else(|| panic!("{slot} missing: {bound:?}"));
        assert!(
            src.to_lowercase().contains(file),
            "{slot}: {src} is not {file}"
        );
    }
    // Winding: the faces of the funnel, seen from outside, are counter-clockwise in our basis (Y up)
    // for the large majority, which is what the renderer's `Ccw` front face with back culling needs.
    let mesh = &m.meshes[0];
    let stride = mesh.stride().unwrap();
    let pos = |i: usize| -> [f32; 3] {
        let o = i * stride;
        let f = |k: usize| {
            f32::from_le_bytes(mesh.vertices[o + 4 * k..o + 4 * k + 4].try_into().unwrap())
        };
        [f(0), f(1), f(2)]
    };
    let n = mesh.vertex_count as usize;
    let (mut cx, mut cz) = (0.0f32, 0.0f32);
    for i in 0..n {
        let p = pos(i);
        cx += p[0] / n as f32;
        cz += p[2] / n as f32;
    }
    let idx: Vec<usize> = if mesh.indices.wide {
        mesh.indices
            .data
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()) as usize)
            .collect()
    } else {
        mesh.indices
            .data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes(c.try_into().unwrap()) as usize)
            .collect()
    };
    let (mut out, mut inn) = (0usize, 0usize);
    for t in idx.chunks_exact(3) {
        let (a, b, c) = (pos(t[0]), pos(t[1]), pos(t[2]));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let nrm = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let r = [
            (a[0] + b[0] + c[0]) / 3.0 - cx,
            (a[2] + b[2] + c[2]) / 3.0 - cz,
        ];
        let d = nrm[0] * r[0] + nrm[2] * r[1];
        if d > 0.0 {
            out += 1;
        } else if d < 0.0 {
            inn += 1;
        }
    }
    println!("winding: outward-ccw {out}, inward {inn}");
    assert!(
        out > 8 * inn || inn > 8 * out,
        "mixed winding: {out} vs {inn}"
    );
}
