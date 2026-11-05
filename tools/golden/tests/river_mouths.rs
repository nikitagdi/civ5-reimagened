//! The mouths of the rivers of S2 against the capture (R5.7 follow-up): `RiverVisSystem::TessellateMesh`
//! puts the first pair of a river that ends at a mouth at the height the river sim gave the end of the
//! river, which is the height of the sea floor of the field. S2 ev 1032 has those pairs at z 7.9096 (five
//! rivers) and 8.047 / 8.053 (one), not at the sea level 10.12.
//!
//! Needs the cache of the scene (`golden world S2`) and the assets of the install; without them the test
//! says so and passes.

use std::path::PathBuf;

use golden::terrain_check::{build_terrain, terrain_basis};
use golden::wv3;
use render::world::terrain::TerrainFrame;
use render::world::water::river::mesh;
use render::world::water::river::stage::RiverShared;

/// The first pair of every river of S2 ev 1032 that ends at a mouth: the left vertex `x, y` and the
/// height `z` (both vertices of the pair have the same `z` up to 0.006).
const CAPTURE_MOUTHS: [([f32; 2], f32); 6] = [
    ([-404.547, 554.353], 7.9096),
    ([-42.715, -451.017], 7.9096),
    ([428.234, 278.683], 8.0469),
    ([590.218, -658.925], 7.9096),
    ([588.771, -446.246], 7.9096),
    ([481.045, 485.117], 7.9096),
];

fn workspace() -> PathBuf {
    golden::captures::default_cache()
        .map(|c| c.join(".."))
        .unwrap_or_default()
}

#[test]
fn the_mouths_of_s2_are_at_the_height_of_the_capture() {
    let ws = workspace();
    let basis_file = ws.join("cache/golden/S2/mapbasis.wv3");
    let assets = ws.join("assets-out/b2");
    if !basis_file.is_file() || !assets.join("manifest").is_file() {
        eprintln!("skipped: no cache/golden/S2/mapbasis.wv3 or assets-out/b2");
        return;
    }
    let mb = wv3::decode_basis(&std::fs::read(&basis_file).unwrap()).unwrap();
    let basis = terrain_basis(&mb).unwrap();
    let frame = TerrainFrame::new(
        u32::from(basis.width),
        u32::from(basis.height),
        basis.wrap_x,
    );
    let basis_wrap = basis.wrap_x;
    let plain = build_terrain(&assets, basis.clone()).unwrap();
    let mut terrain = build_terrain(&assets, basis).unwrap();
    let shared = RiverShared::default();
    terrain.install_stage(shared.stage());
    let sim = shared.sim().expect("the Rivers stage ran");
    let m = mesh::build(&sim, &frame, 2);
    // The first pair of every river that ends at a mouth.
    let mut starts = Vec::new();
    let mut at = 0usize;
    for (i, left) in sim.left_banks.iter().enumerate() {
        let n = left.points.len();
        if n < 4 {
            continue;
        }
        let pairs = (((n - 2) as f32 + 1e-4) / 0.1) as usize + 1;
        if sim.rivers[i].ends_at_mouth {
            starts.push(at);
        }
        at += 2 * pairs;
    }
    eprintln!(
        "vertices {} (capture 2772), mouths {}",
        m.vertices.len(),
        starts.len()
    );
    let mut worst = 0.0f32;
    for &s in &starts {
        let v = m.vertices[s].position;
        let near = CAPTURE_MOUTHS
            .iter()
            .min_by(|a, b| {
                let d = |c: &([f32; 2], f32)| (c.0[0] - v[0]).hypot(c.0[1] - v[1]);
                d(a).total_cmp(&d(b))
            })
            .unwrap();
        let dxy = (near.0[0] - v[0]).hypot(near.0[1] - v[1]);
        // The original's own height texel there, and ours.
        let half = frame.half_world();
        let scale = frame.terrain_scale();
        let (u, vv) = (
            ((v[0] + half.x) / scale.x).round() as i32,
            ((v[1] + half.y) / scale.y).round() as i32,
        );
        let cap = golden::terrain_check::capture_a8_at(&ws.join("cache"), "S2", basis_wrap, u, vv)
            .unwrap();
        let (mut sum, mut n) = (0i32, 0);
        for dv in -12..=12 {
            for du in -12..=12 {
                if let Some(c) = golden::terrain_check::capture_a8_at(
                    &ws.join("cache"),
                    "S2",
                    basis_wrap,
                    u + du,
                    vv + dv,
                )
                .unwrap()
                {
                    let o = i32::from(plain.sampler().unwrap().raw_height(u + du, vv + dv) >> 8);
                    sum += (o - i32::from(c)).abs();
                    n += 1;
                }
            }
        }
        eprintln!(
            "  mean |ours - capture| over 25x25 texels: {:.3} ({n})",
            sum as f32 / n.max(1) as f32
        );
        eprintln!(
            "  texel ({u}, {vv}): capture byte {cap:?}, ours {:?}",
            (-3..=3)
                .flat_map(|d| [plain.sampler().unwrap().raw_height(u + d, vv) >> 8])
                .collect::<Vec<_>>()
        );
        eprintln!(
            "mouth at ({:.1}, {:.1}) z {:.4}; capture ({:.1}, {:.1}) z {:.4}; dxy {:.2} dz {:+.4}",
            v[0],
            v[1],
            v[2],
            near.0[0],
            near.0[1],
            near.1,
            dxy,
            v[2] - near.1
        );
        // Our rivers wiggle by our own stream of `rand` (SIMP-040), so a mouth may lie a few units from
        // the one of the capture; the sea floor is flat there, so the heights still agree (to 0.0016 where
        // the place is the same, 0.14 where the sea floor of the other mouth is deeper).
        assert!(
            dxy < 10.0,
            "no mouth of the capture near ({}, {})",
            v[0],
            v[1]
        );
        worst = worst.max((v[2] - near.1).abs());
    }
    assert_eq!(m.vertices.len(), 2772);
    // Before the field was asked, every mouth was at the level of the water: 2.46 too high.
    assert!(worst < 0.2, "a mouth is {worst} off the capture");
}
