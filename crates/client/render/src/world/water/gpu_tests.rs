//! The water of a test world on the GPU: the whole path of the subsystem (data, heights, currents,
//! bind group, draw) at the three qualities.

use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};

use super::Water;
use super::data::tests::{blob, ocean_data};
use super::data::{
    ICE_BUMP, ICE_BUMP_SIDES, ICE_BUMP_SIDES_CROSS, ICE_CUBE, ICE_DIFFUSE, ICE_DIFFUSE_SIDES,
    WaterData,
};
use super::ocean::{self, WaterFrame};
use super::river::{RiverSim, RiverSpline};
use crate::frame::mask::{MASK_FORMAT, class};
use crate::frame::system::WorldServices;
use crate::frame::targets::{DEPTH_FORMAT, TargetPool, WORLD_FORMAT};
use crate::frame::{
    DrawContext, EncoderContext, InitContext, MainSlot, MaskContext, PassId, PassSlot,
    PrepareContext, RenderSystem,
};
use crate::gpu_test::{Gpu, gpu, read_texture};
use crate::renderer::FrameStats;
use crate::world::ice::{Ice, IceFeed};
use crate::world::river::River;
use crate::world::terrain::basis::{BasisFeature, PlotBasis, TerrainBasis};
use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};

fn gpu_water(g: &Gpu) -> Water {
    let mut w = Water::default();
    w.bind_device(&g.device, &g.queue, 1);
    w.set_data(Arc::new(ocean_data()));
    // 128 x 64 texels, a shallow bank in the west.
    let mut s = TerrainSampler::new(TerrainFrame::new(2, 1, false));
    for v in 0..64 {
        for u in 0..128 {
            s.field_mut()
                .set_height(u, v, if u < 40 { 20000 } else { 6000 });
        }
    }
    w.set_terrain(&s, [2.0, 1.0], None, &Vec::new);
    let rgba = wgpu::TextureFormat::Rgba8Unorm;
    let d2 = wgpu::TextureDimension::D2;
    w.set_lighting(
        Some(ocean::one_texel(
            &g.device,
            &g.queue,
            "ramp",
            rgba,
            [0, 0, 0, 255],
            d2,
        )),
        Some(ocean::one_texel(
            &g.device,
            &g.queue,
            "lut",
            rgba,
            [128, 128, 128, 255],
            wgpu::TextureDimension::D3,
        )),
    );
    w
}

fn target(g: &Gpu, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
    g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("water test"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// One frame of the water seen from above, 64 x 64; the background is grey 128.
fn render(g: &Gpu, quality: i64) -> Vec<u8> {
    render_scene(g, quality, None, false)
}

/// The same with the rivers of `sim` over the water.
fn render_scene(g: &Gpu, quality: i64, sim: Option<Arc<RiverSim>>, ice: bool) -> Vec<u8> {
    render_with_mask(g, quality, sim, ice).0
}

/// The same, with the class mask of the same draws (R8Uint, 64 x 64) as well.
fn render_with_mask(
    g: &Gpu,
    quality: i64,
    sim: Option<Arc<RiverSim>>,
    ice: bool,
) -> (Vec<u8>, Vec<u8>) {
    let mut w = gpu_water(g);
    let mut rivers = River::default();
    rivers.bind_device(&g.device, &g.queue, 1);
    rivers.set_sim(sim.clone());
    w.set_quality(quality);
    let mut pool = TargetPool::new(false, (64, 64));
    w.init(&mut InitContext {
        device: &g.device,
        targets: &mut pool,
    });
    pool.sync(&g.device);
    let half = w.map.expect("map").half_world;
    // From above: x to the right, north (-Z) up, depth 0.5 at y = 0 and nearer upward.
    let view_proj = Mat4::from_cols(
        Vec4::new(1.0 / half.x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, -0.001, 0.0),
        Vec4::new(0.0, -1.0 / half.y, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.5, 1.0),
    );
    w.set_frame(WaterFrame {
        view_proj,
        eye: Vec3::new(0.0, 100.0, 0.0),
        ..Default::default()
    });
    let services = WorldServices::default();
    let mut world_frame = None;
    w.prepare(&mut PrepareContext::new(
        &g.device,
        &g.queue,
        &services,
        &mut world_frame,
    ));
    let mut stats = FrameStats::default();
    let mut encoder = g.device.create_command_encoder(&Default::default());
    w.record_offscreen(
        PassId::WaterPrep,
        &mut EncoderContext::new(
            &g.device,
            &g.queue,
            &mut encoder,
            &pool,
            &mut stats,
            &services,
        ),
    );
    assert!(w.ready(), "the water is bound");
    rivers.feed(&w);
    rivers.prepare(&mut PrepareContext::new(
        &g.device,
        &g.queue,
        &services,
        &mut world_frame,
    ));
    assert_eq!(rivers.drawn_natively(), sim.is_some());
    let mut floes = Ice::default();
    if ice {
        floes.bind_device(&g.device, &g.queue, 1);
        let data = Arc::new(ice_data());
        floes.feed(w.river_feed().map(|water| IceFeed {
            water,
            data,
            sea_level: ocean::OCEAN_HEIGHT,
        }));
        let (basis, frame) = ice_map();
        floes.set_map(&basis, &frame);
        assert!(!floes.mesh().is_empty(), "the floes are made");
        floes.prepare(&mut PrepareContext::new(
            &g.device,
            &g.queue,
            &services,
            &mut world_frame,
        ));
    }
    let colour = target(
        g,
        WORLD_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth = target(g, DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let empty = g
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("empty"),
            entries: &[],
        });
    let group = g.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("empty"),
        layout: &empty,
        entries: &[],
    });
    {
        let colour_view = colour.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water test"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.5,
                            g: 0.5,
                            b: 0.5,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        w.record(
            PassSlot::Main(MainSlot::Terrain),
            &mut DrawContext::new(&mut pass, &group, None, 0.0, &mut stats, &services),
        );
        rivers.record(
            PassSlot::Main(MainSlot::Terrain),
            &mut DrawContext::new(&mut pass, &group, None, 0.0, &mut stats, &services),
        );
        floes.record(
            PassSlot::Main(MainSlot::Terrain),
            &mut DrawContext::new(&mut pass, &group, None, 0.0, &mut stats, &services),
        );
    }
    let mask = target(
        g,
        MASK_FORMAT,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    {
        let mask_view = mask.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let mut pass =
            crate::frame::mask::begin_pass(&mut encoder, &mask_view, &depth_view).forget_lifetime();
        w.record_mask(
            PassSlot::Main(MainSlot::Terrain),
            &mut MaskContext::new(&mut pass, &group, 0.0, &services),
        );
        rivers.record_mask(
            PassSlot::Main(MainSlot::Terrain),
            &mut MaskContext::new(&mut pass, &group, 0.0, &services),
        );
    }
    g.queue.submit([encoder.finish()]);
    assert_eq!(floes.drawn(), u32::from(ice), "the ice draws once");
    assert_eq!(w.drawn(), 1);
    assert_eq!(rivers.drawn_natively(), sim.is_some());
    (
        read_texture(g, &colour, 0, 0, (64, 64), 4),
        read_texture(g, &mask, 0, 0, (64, 64), 1),
    )
}

/// The ocean's textures, the ice's (small and flat; the cube has six faces) and the ice settings.
fn ice_data() -> WaterData {
    use asset_format::{TexDim, TextureBlob, TextureFormat};
    let mut d = ocean_data();
    let flat = |format, bytes: usize, fill| blob(format, 4, 4, bytes, fill);
    d.add_texture(
        ICE_CUBE,
        TextureBlob {
            format: TextureFormat::Rgba8,
            dim: TexDim::Cube,
            srgb: false,
            width: 4,
            height: 4,
            depth: 1,
            mips: vec![vec![90; 4 * 4 * 4 * 6]],
        },
    );
    d.add_texture(ICE_DIFFUSE, flat(TextureFormat::Rgba8, 4, 230));
    d.add_texture(ICE_DIFFUSE_SIDES, flat(TextureFormat::Rgba8, 4, 200));
    d.add_texture(ICE_BUMP, flat(TextureFormat::Rgba16F, 8, 0));
    d.add_texture(ICE_BUMP_SIDES, flat(TextureFormat::Rgba16F, 8, 0));
    d.add_texture(ICE_BUMP_SIDES_CROSS, flat(TextureFormat::R16F, 2, 0));
    let mut s = asset_format::WorldSettings::default();
    s.ice.full = crate::world::water::ice::tests::params();
    s.ice.low = s.ice.full.clone();
    d.set_settings(&s);
    d
}

/// The two hexes of the test map, both ice.
fn ice_map() -> (TerrainBasis, TerrainFrame) {
    let plot = PlotBasis {
        terrain: crate::terrain::TerrainKind::Ocean,
        feature: BasisFeature::Ice,
        ..PlotBasis::default()
    };
    (
        TerrainBasis::uniform(2, 1, false, 1, plot),
        TerrainFrame::new(2, 1, false),
    )
}

/// R5.8: the ice of a polar test map draws over the water, the same twice.
#[test]
fn the_ice_is_drawn_over_the_water_and_is_the_same_against_itself() {
    let Some(g) = gpu("ice draw", wgpu::Backends::PRIMARY, wgpu::Features::empty()) else {
        return;
    };
    let without = render_scene(&g, 2, None, false);
    let with = render_scene(&g, 2, None, true);
    let again = render_scene(&g, 2, None, true);
    assert_eq!(with, again, "two runs differ");
    let differing = with
        .chunks(4)
        .zip(without.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing > 200,
        "only {differing} pixels changed under the ice"
    );
}

/// One river along the middle of the test map (128 x 64 texels), 12 texels wide.
fn test_river() -> Arc<RiverSim> {
    let line = |dv: f32| RiverSpline {
        points: (0..9)
            .map(|i| Vec3::new(10.0 + 13.5 * i as f32, 32.0 + dv, 0.0))
            .collect(),
        ends_at_mouth: false,
    };
    Arc::new(RiverSim {
        rivers: vec![line(0.0)],
        left_banks: vec![line(6.0)],
        right_banks: vec![line(-6.0)],
        heights: vec![vec![13000.0; 9]],
        ..RiverSim::default()
    })
}

#[test]
fn the_rivers_are_drawn_over_the_water_and_are_the_same_against_themselves() {
    let Some(g) = gpu(
        "river draw",
        wgpu::Backends::PRIMARY,
        wgpu::Features::empty(),
    ) else {
        return;
    };
    for quality in [2, 0] {
        let without = render_scene(&g, quality, None, false);
        let with = render_scene(&g, quality, Some(test_river()), false);
        let again = render_scene(&g, quality, Some(test_river()), false);
        assert_eq!(with, again, "quality {quality}: two runs differ");
        let differing = with
            .chunks(4)
            .zip(without.chunks(4))
            .filter(|(a, b)| a != b)
            .count();
        // 12 texels of 110 rows over 64 pixels is about 7 pixels wide, along most of the map.
        assert!(
            differing > 150,
            "quality {quality}: only {differing} pixels changed under the river"
        );
        // Far from the river the water is the same.
        assert_eq!(with[..4 * 64 * 4], without[..4 * 64 * 4]);
    }
}

/// Spec 4.14: the ocean writes class WATER and the rivers class RIVER into the class mask.
#[test]
fn the_ocean_and_the_rivers_write_their_classes_into_the_class_mask() {
    let Some(g) = gpu(
        "water class mask",
        wgpu::Backends::PRIMARY,
        wgpu::Features::empty(),
    ) else {
        return;
    };
    for quality in [2, 0] {
        let (_, water_only) = render_with_mask(&g, quality, None, false);
        let count = |m: &[u8], c: u32| m.iter().filter(|&&p| u32::from(p) == c).count();
        assert!(
            count(&water_only, class::WATER) > 64 * 64 / 2,
            "quality {quality}: the water is not in the mask"
        );
        assert_eq!(count(&water_only, class::RIVER), 0);
        let (_, with_river) = render_with_mask(&g, quality, Some(test_river()), false);
        assert!(
            count(&with_river, class::RIVER) > 150,
            "quality {quality}: the river is not in the mask ({} pixels)",
            count(&with_river, class::RIVER)
        );
        assert!(count(&with_river, class::WATER) > 64 * 64 / 4);
    }
}

#[test]
fn the_water_of_a_test_world_draws_and_is_the_same_against_itself() {
    let Some(g) = gpu(
        "water draw",
        wgpu::Backends::PRIMARY,
        wgpu::Features::empty(),
    ) else {
        return;
    };
    for quality in [2, 1, 0] {
        let a = render(&g, quality);
        let b = render(&g, quality);
        assert_eq!(a, b, "quality {quality}: two runs differ");
        let background = a
            .chunks(4)
            .filter(|p| p[..3] == [188, 188, 188] || p[..3] == [128, 128, 128])
            .count();
        assert!(
            background < a.len() / 8,
            "quality {quality}: the water did not cover the map ({background} grey texels)"
        );
    }
}
