//! The cost of the particles on the CPU against the frame budget (perf-huge: 30 FPS, so 33 ms a
//! frame for everything): a map of burning cities and a beam, simulated and expanded per frame.

use std::time::Instant;

use asset_format::fx_flag as flag;
use glam::{Mat4, Vec3};

use super::draw::{CameraAxes, build};
use super::sim::tests::{effect, props};
use super::sim::{MAX_PARTICLES, Sim};

#[test]
fn a_full_pool_steps_and_expands_within_a_few_milliseconds() {
    let mut sim = Sim::default();
    // 60 burning cities (the d3 fire: rate 64, life about 0.5 s) and smoke columns of 2 s.
    for i in 0..60u64 {
        let mut p = props();
        p.rate = 64.0;
        p.life = [0.5, 2.0];
        p.speed = [5.0, 30.0];
        p.volume_kind = 2;
        p.volume = [10.0, 5.0, 10.0, 11.5];
        p.gravity = 20.0;
        p.flags = if i % 2 == 0 {
            flag::VELOCITY_ALIGNED
        } else {
            0
        };
        let at = Mat4::from_translation(Vec3::new(i as f32 * 64.0, 0.0, 0.0));
        sim.spawn(i, effect(p), at, 1.0, None);
    }
    let cam = CameraAxes {
        right: Vec3::X,
        up: Vec3::Y,
        forward: -Vec3::Z,
    };
    for _ in 0..90 {
        sim.step(1.0 / 30.0);
    }
    assert!(
        sim.total() <= MAX_PARTICLES,
        "the pool caps: {}",
        sim.total()
    );
    let started = Instant::now();
    let frames = 30;
    let mut quads = 0;
    for _ in 0..frames {
        sim.step(1.0 / 30.0);
        let (v, _) = build(&sim, cam, &|_| true);
        quads = v.len() / 4;
    }
    let per_frame = started.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
    eprintln!(
        "{} particles, {quads} quads, {per_frame:.2} ms a frame",
        sim.total()
    );
    // A debug build is several times slower than the release the budget is for.
    let limit = if cfg!(debug_assertions) { 40.0 } else { 4.0 };
    assert!(per_frame < limit, "{per_frame:.2} ms a frame");
}
