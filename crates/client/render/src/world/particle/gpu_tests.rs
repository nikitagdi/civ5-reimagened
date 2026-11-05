//! Whole-frame GPU tests of the `Particle` system: the "look here" beam, played from an `fx:`
//! effect handed to the renderer, lights the pixels over its cell. A test that finds no adapter
//! prints why it skips and returns.

use asset_format::{
    FxEffect, FxFrame, FxIndex, FxVisEffect, LogicalId, TexDim, TextureBlob, TextureFormat,
    fx_flag2 as flag2,
};
use draw_list::UiDrawList;
use glam::Vec3;

use super::LOOK_AT_EFFECT;
use super::sim::tests::{effect, props};
use crate::frame::FrameSource;
use crate::game_camera::GameCamera;
use crate::gpu_test::block_on;
use crate::hexgrid::cell_center;
use crate::renderer::{BackendPref, Renderer};
use crate::scene::Scene;
use crate::terrain::{Fog, Relief, TerrainCell, TerrainKind, TerrainMesh, TerrainSnapshot};

const SIZE: (u32, u32) = (512, 288);
const MAP: (u32, u32) = (16, 10);
const CELL: (i32, i32) = (8, 5);

fn world(mut r: Renderer) -> Renderer {
    let cells = (0..MAP.0 * MAP.1)
        .map(|_| TerrainCell {
            relief: Relief::Flat,
            kind: TerrainKind::Grass,
            fog: Fog::Visible,
            ..TerrainCell::default()
        })
        .collect();
    let snapshot = TerrainSnapshot::new(MAP.0, MAP.1, true, cells).unwrap();
    let mesh = TerrainMesh::build(&snapshot);
    r.set_terrain(&snapshot, &mesh);
    r.set_entity_art_unavailable();
    r.set_animation_time(0.0);
    r.set_time(0.0);
    r
}

fn white_beam() -> FxEffect {
    let mut p = props();
    p.alpha = 1; // additive
    p.burst = 1;
    p.rate = 0.0;
    p.life = [10.0, 10.0];
    p.size_min = [60.0; 3];
    p.size_max = [60.0; 3];
    p.colors = [[1.0; 4]; 4];
    p.tint = [1.0; 4];
    p.flags2 = flag2::UNIFORM_SIZE;
    let mut fx = (*effect(p)).clone();
    fx.emitters[0].frames = vec![FxFrame {
        texture: Some(LogicalId::new("tx:white")),
        uv: [0.0, 0.0, 1.0, 1.0],
    }];
    fx
}

fn white_texture() -> TextureBlob {
    TextureBlob {
        format: TextureFormat::Rgba8,
        dim: TexDim::D2,
        srgb: false,
        width: 4,
        height: 4,
        depth: 1,
        mips: vec![vec![255; 4 * 4 * 4]],
    }
}

/// The pixel over the middle of `CELL` before and with the beam; `None` without an adapter.
fn beam_pixels(name: &str, pref: BackendPref) -> Option<([u8; 4], [u8; 4])> {
    let Ok(r) = block_on(Renderer::new_headless(SIZE, pref)) else {
        eprintln!("SKIP {name}: no adapter");
        return None;
    };
    let mut r = world(r);
    let camera = GameCamera::new(cell_center(CELL.0, CELL.1));
    let scene = Scene::test(0.0);
    let ui = UiDrawList::default();
    let aspect = SIZE.0 as f32 / SIZE.1 as f32;
    let c = cell_center(CELL.0, CELL.1);
    let p = camera
        .view_proj(aspect)
        .project_point3(Vec3::new(c.x, 0.0, c.y));
    let at = (
        ((p.x * 0.5 + 0.5) * SIZE.0 as f32) as u32,
        ((0.5 - p.y * 0.5) * SIZE.1 as f32) as u32,
    );
    let shot = |r: &mut Renderer, t: f32| {
        r.set_animation_time(t);
        let readback = r.read_frame(FrameSource::World);
        r.render(&scene, &camera, &ui, None).unwrap();
        readback.wait().unwrap()
    };

    let before = shot(&mut r, 0.0);

    let fx = LogicalId::new("fx:t");
    let particles = r.particles_mut();
    particles.set_index(FxIndex {
        effects: vec![FxVisEffect {
            name: LOOK_AT_EFFECT.into(),
            translation: [0.0; 3],
            rotation: [90.0, 0.0, 0.0],
            scale: 1.0,
            duration: 5.0,
            fow_mask: 3,
            particles: vec![fx.clone()],
        }],
    });
    particles.add_effect(fx, white_beam());
    particles.add_texture(LogicalId::new("tx:white"), white_texture());
    particles.look_at(CELL.0, CELL.1);
    let mut with = shot(&mut r, 0.05);
    for i in 2..=5 {
        with = shot(&mut r, 0.05 * i as f32);
    }
    let live = r.particles_mut().particles();
    assert!(live > 0, "{name}: the beam has particles ({live})");
    // The quad is centred on the ground point and 60 units wide: the lower half sinks into the
    // terrain (depth test), so take the pixel of the column over the cell that changed most.
    let px = |im: &crate::frame::Image, y: u32| {
        let p = im.pixel(at.0, y);
        [p[0], p[1], p[2], p[3]]
    };
    let gain = |y: u32| -> i32 {
        let (a, b) = (px(&before, y), px(&with, y));
        (0..3).map(|i| i32::from(b[i]) - i32::from(a[i])).sum()
    };
    let y = (0..SIZE.1).max_by_key(|&y| gain(y)).unwrap();
    Some((px(&before, y), px(&with, y)))
}

fn assert_lit(name: &str, before: [u8; 4], with: [u8; 4]) {
    let all = (0..3).all(|i| with[i] > before[i] || with[i] == 255);
    let any = (0..3).any(|i| with[i] > before[i]);
    assert!(
        all && any,
        "{name}: the beam lights its cell: before {before:?}, with {with:?}"
    );
}

#[test]
fn a_look_at_beam_lights_its_cell() {
    let name = "a_look_at_beam_lights_its_cell";
    if let Some((before, with)) = beam_pixels(name, BackendPref::Auto) {
        assert_lit(name, before, with);
    }
}

/// One blob of a built store (`<dir>/manifest` names the manifest blob; blobs are `<dir>/<hash>`).
fn store_blob(dir: &std::path::Path, m: &asset_format::Manifest, id: &str) -> Option<Vec<u8>> {
    let v = m.variants(&LogicalId::new(id)).first()?;
    std::fs::read(dir.join(v.hash.to_string())).ok()
}

/// The real "look here" beam (`fx:fx_look_at_me` of the set `b2-fx`) over a flat map, for the eye
/// against the S1a capture. Runs when `CIV5_FX_STORE` names a store with the set; writes the frame
/// as raw RGBA to `RENDER_DUMP_DIR` when that is set.
#[test]
fn the_shipped_beam_plays_and_draws() {
    let Ok(dir) = std::env::var("CIV5_FX_STORE") else {
        eprintln!("SKIP: CIV5_FX_STORE is not set");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let pointer = std::fs::read_to_string(dir.join("manifest")).unwrap();
    let m =
        asset_format::Manifest::decode(&std::fs::read(dir.join(pointer.trim())).unwrap()).unwrap();
    let Ok(r) = block_on(Renderer::new_headless((960, 540), BackendPref::Auto)) else {
        eprintln!("SKIP: no adapter");
        return;
    };
    let mut r = world(r);
    let index = FxIndex::decode(&store_blob(&dir, &m, asset_format::FX_INDEX_ID).unwrap()).unwrap();
    let fx = FxEffect::decode(&store_blob(&dir, &m, "fx:fx_look_at_me").unwrap()).unwrap();
    let p = r.particles_mut();
    p.set_index(index);
    p.add_effect(LogicalId::new("fx:fx_look_at_me"), fx);
    for id in p.wanted() {
        if let Some(b) = store_blob(&dir, &m, id.as_str()) {
            p.add_texture(id, TextureBlob::decode(&b).unwrap());
        }
    }
    p.look_at(CELL.0, CELL.1);
    let camera = GameCamera::new(cell_center(CELL.0, CELL.1));
    let scene = Scene::test(0.0);
    let ui = UiDrawList::default();
    let mut image = None;
    for i in 0..=12 {
        r.set_animation_time(0.05 * i as f32);
        let readback = r.read_frame(FrameSource::World);
        r.render(&scene, &camera, &ui, None).unwrap();
        image = Some(readback.wait().unwrap());
    }
    let live = r.particles_mut().particles();
    eprintln!("the beam at 0.6 s: {live} particles");
    assert!(live > 0);
    if let (Ok(out), Some(im)) = (std::env::var("RENDER_DUMP_DIR"), image) {
        let path =
            std::path::Path::new(&out).join(format!("beam_{}x{}.rgba", im.size.0, im.size.1));
        std::fs::write(path, &im.bytes).unwrap();
    }
}

#[test]
fn particles_draw_on_webgl2_too() {
    let name = "particles_draw_on_webgl2_too";
    if let Some((before, with)) = beam_pixels(name, BackendPref::WebGl2) {
        assert_lit(name, before, with);
    }
}
