//! Tests of the UI v2 pass: the shader against naga (WGSL validation, GLSL ES 3.00 output as WebGL2 gets it,
//! the WebGL2 limits), then GPU tests that draw a small scene into an `Rgba8Unorm` texture and check pixels (a
//! test that finds no adapter prints why it skips), and the pass in a whole headless frame.

use std::path::PathBuf;
use std::sync::Arc;

use draw_list::UiDrawList;
use glam::{Affine2, Vec2};
use ui2_draw::instance::{ATTRIBUTES, INSTANCE_SIZE};
use ui2_draw::{
    AnchorXform, Border, BoxItem, ChunkBuilder, ChunkId, Clip, Color, EllipticBox, ExtTexture,
    FaceId, Fill, Glyphs, Gradient, GradientStop, ImageItem, ImageRef, Item, LayerId, MaskItem,
    NineSlice, Radii, Rect, Scene, ShadowItem, testkit,
};
use ui2_text::{RunStyle, TextEngine};

use super::*;
use crate::gpu_test::{Gpu, block_on, gpu, read_texture};

const SHADER: &str = include_str!("ui2.wgsl");
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const FIRA: FaceId = FaceId(0);
const RED: Color = Color::rgba(255, 0, 0, 255);
const GREEN: Color = Color::rgba(0, 255, 0, 255);
const BLUE: Color = Color::rgba(0, 0, 255, 255);
const WHITE: Color = Color::rgba(255, 255, 255, 255);
const YELLOW: Color = Color::rgba(255, 255, 0, 255);

fn module() -> (naga::Module, naga::valid::ModuleInfo) {
    let module = naga::front::wgsl::parse_str(SHADER)
        .unwrap_or_else(|e| panic!("ui2.wgsl: {}", e.emit_to_string(SHADER)));
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("ui2.wgsl: {}", e.emit_to_string(SHADER)));
    (module, info)
}

#[test]
fn ui2_wgsl_validates_and_translates_to_webgl2_glsl() {
    let (module, info) = module();
    for (stage, entry) in [
        (naga::ShaderStage::Vertex, "vs"),
        (naga::ShaderStage::Fragment, "fs"),
    ] {
        let options = naga::back::glsl::Options {
            version: naga::back::glsl::Version::Embedded {
                version: 300,
                is_webgl: true,
            },
            ..Default::default()
        };
        let pipeline = naga::back::glsl::PipelineOptions {
            shader_stage: stage,
            entry_point: entry.to_string(),
            multiview: None,
        };
        let mut out = String::new();
        let mut writer = naga::back::glsl::Writer::new(
            &mut out,
            &module,
            &info,
            &options,
            &pipeline,
            naga::proc::BoundsCheckPolicies::default(),
        )
        .unwrap_or_else(|e| panic!("{entry}: {e}"));
        writer.write().unwrap_or_else(|e| panic!("{entry}: {e}"));
        assert!(out.starts_with("#version 300 es"), "{entry}: {out}");
    }
}

#[test]
fn ui2_vertex_layout_fits_the_webgl2_limits() {
    let limits = wgpu::Limits::downlevel_webgl2_defaults();
    assert!(INSTANCE_SIZE as u32 <= limits.max_vertex_buffer_array_stride);
    assert!(ATTRIBUTES.len() as u32 <= limits.max_vertex_attributes);
    // Half floats are a core vertex format on WebGPU and on GLES 3.0 / WebGL2 (`HALF_FLOAT`); the pipeline keeps
    // the `Float16x4` of the instance layout.
    let attrs = vertex_attributes();
    assert_eq!(attrs.len(), 12);
    assert_eq!(attrs[1].format, wgpu::VertexFormat::Float16x4);
    assert_eq!(attrs[11].format, wgpu::VertexFormat::Uint32x3);
    let mut end = 0;
    for a in &attrs {
        assert!(
            a.offset >= end,
            "attributes overlap at {}",
            a.shader_location
        );
        end = a.offset + a.format.size();
    }
    assert_eq!(end, INSTANCE_SIZE as u64);

    // Inter-stage variables: the fragment inputs at user locations.
    let (module, _) = module();
    let fs = module.entry_points.iter().find(|e| e.name == "fs").unwrap();
    let input = &fs.function.arguments[0];
    let naga::TypeInner::Struct { members, .. } = &module.types[input.ty].inner else {
        panic!("VsOut is a struct");
    };
    let locations = members
        .iter()
        .filter(|m| matches!(m.binding, Some(naga::Binding::Location { .. })))
        .count() as u32;
    assert!(
        locations <= limits.max_inter_stage_shader_variables,
        "{locations} varyings"
    );
    // Gradients, shadows and ellipses fit in 12 (colours packed): three stay free for later primitives.
    assert!(locations <= 12);
}

fn engine() -> TextEngine {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut e = TextEngine::new();
    e.load_face(FIRA, Arc::from(bytes)).expect("Fira Sans");
    e
}

fn chunk(items: Vec<Item>) -> ui2_draw::ChunkItems {
    let mut b = ChunkBuilder::new();
    for i in items {
        b.push(i);
    }
    b.build()
}

/// External textures of a test: id to (serial, texture, view, uv scale).
#[derive(Default)]
struct FakeTextures {
    map: std::collections::HashMap<u64, (u64, Tex, wgpu::TextureView, [f32; 2])>,
    serial: u64,
    asked: Vec<u64>,
}

impl Ui2ExtTextures for FakeTextures {
    fn resolve(&mut self, id: u64) -> Option<(u64, &wgpu::TextureView, [f32; 2])> {
        self.asked.push(id);
        self.map.get(&id).map(|(s, _, v, k)| (*s, v, *k))
    }
}

struct Rig {
    gpu: Gpu,
    target: Tex,
    view: wgpu::TextureView,
    pass: Ui2Pass,
    size: (u32, u32),
    ext: FakeTextures,
}

impl Rig {
    /// Makes (or replaces) external texture `id`: `w` x `h` straight RGBA pixels, `scale` its UV scale.
    fn add_ext(&mut self, id: u64, w: u32, h: u32, pixels: &[u8], scale: [f32; 2]) {
        let tex = self.gpu.device.tex(
            &wgpu::TextureDescriptor {
                label: Some("ui2 test external"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            Cluster::Other,
        );
        self.gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.ext.serial += 1;
        self.ext.map.insert(id, (self.ext.serial, tex, view, scale));
    }

    /// A solid external texture of 4 x 4.
    fn add_solid_ext(&mut self, id: u64, c: Color) {
        let px: Vec<u8> = (0..16).flat_map(|_| c.0).collect();
        self.add_ext(id, 4, 4, &px, [1.0, 1.0]);
    }

    fn new(test: &str, backends: wgpu::Backends, size: (u32, u32)) -> Option<Rig> {
        let gpu = gpu(test, backends, wgpu::Features::empty())?;
        let target = gpu.device.tex(
            &wgpu::TextureDescriptor {
                label: Some("ui2 test target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
            Cluster::Other,
        );
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pass = Ui2Pass::new(&gpu.device, FORMAT);
        if let Some(e) = block_on(scope.pop()) {
            panic!("{test}: making the pass: {e}");
        }
        Some(Rig {
            gpu,
            target,
            view,
            pass,
            size,
            ext: FakeTextures::default(),
        })
    }

    /// Prepares `scene` and draws it over opaque black: the RGBA bytes of the target. Fails on any validation
    /// error.
    fn frame(&mut self, scene: &mut Scene) -> Vec<u8> {
        let scope = self
            .gpu
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let target = UiTarget {
            format: FORMAT,
            size: self.size,
        };
        self.pass.prepare(
            &self.gpu.device,
            &self.gpu.queue,
            scene,
            &target,
            &mut self.ext,
        );
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut p = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui2 test"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.pass.draw(&mut p);
        }
        self.gpu.queue.submit([encoder.finish()]);
        let img = read_texture(&self.gpu, &self.target, 0, 0, self.size, 4);
        if let Some(e) = block_on(scope.pop()) {
            panic!("validation error in the frame: {e}");
        }
        img
    }
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * w + x) * 4) as usize;
    [img[at], img[at + 1], img[at + 2], img[at + 3]]
}

fn rgb(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let p = px(img, w, x, y);
    [p[0], p[1], p[2]]
}

/// A rounded box with a border, a box clipped by a rectangle, a line of Fira Sans 14 px.
fn mixed_scene(text: &mut TextEngine) -> Scene {
    let mut scene = Scene::new(1.0);
    let rounded = BoxItem {
        rect: Rect::new(10.0, 10.0, 60.0, 40.0),
        fill: Fill::Solid(RED),
        border: Border::uniform(2.0, WHITE),
        radii: Radii::uniform(12.0),
        ..BoxItem::default()
    };
    let clipped = BoxItem {
        clip: Some(Clip::rect(Rect::new(90.0, 10.0, 30.0, 30.0))),
        ..BoxItem::solid(Rect::new(90.0, 10.0, 60.0, 60.0), GREEN)
    };
    let run = text
        .shape(&RunStyle::new(FIRA, 14.0), "Hamburgefonstiv")
        .expect("shape");
    let glyphs = Glyphs::from_run(&run, 1.0, 10.0, 70.0, WHITE);
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(vec![
            Item::Box(rounded),
            Item::Box(clipped),
            Item::Glyphs(glyphs),
        ]),
    );
    scene.resolve(text, 1000.0);
    scene
}

fn check_mixed_scene(img: &[u8], w: u32) {
    // Inside the fill, on the border, outside the rounded corner.
    assert_eq!(rgb(img, w, 40, 30), [255, 0, 0]);
    assert_eq!(rgb(img, w, 10, 30), [255, 255, 255], "left border");
    assert_eq!(rgb(img, w, 40, 49), [255, 255, 255], "bottom border");
    assert_eq!(rgb(img, w, 11, 11), [0, 0, 0], "outside the rounded corner");
    assert_eq!(rgb(img, w, 69, 49), [0, 0, 0], "outside the rounded corner");
    // The corner is anti-aliased: a pixel on the arc is between black and white.
    let arc = (0..12)
        .map(|i| rgb(img, w, 10 + i, 10 + i))
        .find(|p| p[1] > 0 && p[1] < 255);
    assert!(
        arc.is_some(),
        "a partly covered pixel on the diagonal of the corner"
    );
    // The clip keeps the top left 30 x 30 of the green box.
    assert_eq!(rgb(img, w, 100, 20), [0, 255, 0]);
    assert_eq!(rgb(img, w, 119, 39), [0, 255, 0]);
    assert_eq!(rgb(img, w, 125, 20), [0, 0, 0], "clipped on the right");
    assert_eq!(rgb(img, w, 100, 45), [0, 0, 0], "clipped below");
    // Glyph coverage under the text line (x 10..150, y 56..74), nothing above the cap height.
    let mut ink = 0u64;
    let mut full = 0;
    for y in 56..75 {
        for x in 10..150 {
            let v = px(img, w, x, y)[1];
            ink += u64::from(v);
            full += usize::from(v == 255);
        }
    }
    assert!(ink > 255 * 100, "the line has ink: {ink}");
    assert!(full > 10, "stems are fully covered: {full}");
    for x in 10..90 {
        assert_eq!(
            rgb(img, w, x, 54),
            [0, 0, 0],
            "nothing between the box and the text"
        );
    }
}

fn mixed_scene_on(test: &str, backends: wgpu::Backends) {
    let Some(mut rig) = Rig::new(test, backends, (160, 80)) else {
        return;
    };
    let mut text = engine();
    let mut scene = mixed_scene(&mut text);
    let img = rig.frame(&mut scene);
    check_mixed_scene(&img, rig.size.0);
    let s = rig.pass.stats();
    assert_eq!(s.draws, 1, "one layer, one draw");
    assert_eq!(s.layer_draws, [0, 1, 0, 0]);
    assert_eq!(rig.pass.draw_totals().0, 1);
    assert!(s.glyph_bytes > 0);
}

#[test]
fn ui2_draws_rounded_bordered_clipped_boxes_and_glyphs() {
    mixed_scene_on(
        "ui2_draws_rounded_bordered_clipped_boxes_and_glyphs",
        wgpu::Backends::PRIMARY,
    );
}

/// The same scene on the GL backend (the WebGL2 code path of wgpu, natively), when the machine has one.
#[test]
fn ui2_draws_the_same_on_gl() {
    mixed_scene_on("ui2_draws_the_same_on_gl", wgpu::Backends::GL);
}

#[test]
fn ui2_unchanged_frame_uploads_nothing_and_a_colour_change_patches_one_instance() {
    let Some(mut rig) = Rig::new(
        "ui2_unchanged_frame_uploads_nothing",
        wgpu::Backends::PRIMARY,
        (64, 32),
    ) else {
        return;
    };
    let mut scene = Scene::new(1.0);
    let boxes = |c: Color| {
        chunk(vec![
            Item::Box(BoxItem::solid(Rect::new(0.0, 0.0, 10.0, 10.0), BLUE)),
            Item::Box(BoxItem::solid(Rect::new(20.0, 0.0, 10.0, 10.0), c)),
        ])
    };
    scene.set_chunk(LayerId::Hud, ChunkId(1), boxes(RED));
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, 64, 25, 5), [255, 0, 0]);
    let first = rig.pass.stats();
    assert!(first.upload_bytes > 0);
    assert_eq!(first.layers_uploaded, 1);

    let _ = rig.frame(&mut scene);
    let idle = rig.pass.stats();
    assert_eq!((idle.upload_bytes, idle.layers_uploaded), (0, 0));
    assert_eq!(idle.draws, 1);

    scene.set_chunk(LayerId::Hud, ChunkId(1), boxes(GREEN));
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, 64, 25, 5), [0, 255, 0]);
    assert_eq!(rgb(&img, 64, 5, 5), [0, 0, 255]);
    let patched = rig.pass.stats();
    assert_eq!(patched.layers_uploaded, 1);
    assert_eq!(
        patched.upload_bytes,
        (2 * INSTANCE_SIZE) as u64,
        "the chunk's range"
    );
}

/// Partial upload: a chunk whose instance count changes uploads the layer from that chunk on; a layer that
/// outgrows its buffer is written whole into a bigger one.
#[test]
fn ui2_a_count_change_uploads_the_tail_and_growth_rewrites_the_layer() {
    let Some(mut rig) = Rig::new(
        "ui2_count_change_uploads_the_tail",
        wgpu::Backends::PRIMARY,
        (64, 32),
    ) else {
        return;
    };
    let mut scene = Scene::new(1.0);
    let row = |n: usize, x0: f32, c: Color| {
        chunk(
            (0..n)
                .map(|i| {
                    Item::Box(BoxItem::solid(
                        Rect::new(x0 + 8.0 * i as f32, 0.0, 6.0, 6.0),
                        c,
                    ))
                })
                .collect(),
        )
    };
    scene.set_chunk(LayerId::Hud, ChunkId(1), row(2, 0.0, BLUE));
    scene.set_chunk(LayerId::Hud, ChunkId(2), row(1, 32.0, RED));
    let _ = rig.frame(&mut scene);
    // Chunk 2 grows: instances 2.. are written, chunk 1's are not.
    scene.set_chunk(LayerId::Hud, ChunkId(2), row(3, 32.0, GREEN));
    let img = rig.frame(&mut scene);
    let s = rig.pass.stats();
    assert_eq!(
        (s.upload_bytes, s.layers_uploaded),
        ((3 * INSTANCE_SIZE) as u64, 1)
    );
    assert_eq!(rgb(&img, 64, 2, 2), [0, 0, 255]);
    assert_eq!(rgb(&img, 64, 50, 2), [0, 255, 0]);
    // Chunk 2 shrinks to one instance: that one is written, the draw is shorter.
    scene.set_chunk(LayerId::Hud, ChunkId(2), row(1, 32.0, GREEN));
    let img = rig.frame(&mut scene);
    let s = rig.pass.stats();
    assert_eq!((s.upload_bytes, s.instances), (INSTANCE_SIZE as u64, 3));
    assert_eq!(rgb(&img, 64, 50, 2), [0, 0, 0]);
    // A chunk appended past the buffer's 64 KiB: a bigger buffer, written whole.
    let many = MIN_BUFFER as usize / INSTANCE_SIZE + 10;
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(3),
        row(many, 0.0, Color::rgba(0, 0, 0, 255)),
    );
    scene.set_chunk(LayerId::Hud, ChunkId(4), row(1, 16.0, YELLOW));
    let img = rig.frame(&mut scene);
    let s = rig.pass.stats();
    assert_eq!(s.upload_bytes, ((3 + many + 1) * INSTANCE_SIZE) as u64);
    assert!(rig.pass.layers[LayerId::Hud.index()].capacity > MIN_BUFFER);
    assert_eq!(rgb(&img, 64, 18, 2), [255, 255, 0]);
}

/// Glyph eviction on the GPU: small glyph pages, 400 strings of their own sizes through one chunk while a label
/// stays; the pages are cleared again and again and only dirty rectangles are uploaded. The last frame is the
/// same, pixel for pixel, as the same chunks drawn by a fresh pass from a fresh scene.
#[test]
fn ui2_glyph_eviction_keeps_the_gpu_pages_right() {
    let size = (200, 60);
    let Some(mut rig) = Rig::new(
        "ui2_glyph_eviction_keeps_the_gpu_pages_right",
        wgpu::Backends::PRIMARY,
        size,
    ) else {
        return;
    };
    let mut text = engine();
    let cfg = ui2_text::AtlasConfig {
        page_size: 128,
        max_pages: 4,
        padding: 1,
        shelf_quantum: 4,
    };
    let line = |text: &mut TextEngine, s: &str, px: f32, y: f32| {
        let run = text.shape(&RunStyle::new(FIRA, px), s).expect("shape");
        chunk(vec![Item::Glyphs(Glyphs::from_run(
            &run, 1.0, 4.0, y, WHITE,
        ))])
    };
    let label = line(&mut text, "Science +42", 14.0, 20.0);
    let mut scene = Scene::with_atlas(1.0, cfg);
    scene.set_chunk(LayerId::Hud, ChunkId(1), label.clone());
    let mut layers = Vec::new();
    for i in 0..400u32 {
        let c = line(
            &mut text,
            &format!("Tile {i} yields {}", i % 9),
            11.0 + i as f32 * 0.01,
            45.0,
        );
        scene.set_chunk(LayerId::Window, ChunkId(2), c);
        scene.resolve(&mut text, 1000.0);
        let _ = rig.frame(&mut scene);
        layers.push(rig.pass.mask.layers());
    }
    let last = line(&mut text, "Last words", 13.0, 45.0);
    scene.set_chunk(LayerId::Window, ChunkId(2), last.clone());
    scene.resolve(&mut text, 1000.0);
    let evicting = rig.frame(&mut scene);
    assert!(
        scene.glyphs().evictions() > 5,
        "{}",
        scene.glyphs().evictions()
    );
    assert_eq!(scene.stats().atlas_full, 0);
    // The array grew 2, then 4 layers (powers of two), and never past the store's limit.
    assert_eq!(*layers.last().unwrap(), 4);
    assert!(
        layers.windows(2).filter(|w| w[0] != w[1]).count() <= 2,
        "{layers:?}"
    );

    let Some(mut fresh_rig) = Rig::new(
        "ui2_glyph_eviction_reference",
        wgpu::Backends::PRIMARY,
        size,
    ) else {
        return;
    };
    let mut fresh = Scene::new(1.0);
    fresh.set_chunk(LayerId::Hud, ChunkId(1), label);
    fresh.set_chunk(LayerId::Window, ChunkId(2), last);
    fresh.resolve(&mut text, 1000.0);
    let reference = fresh_rig.frame(&mut fresh);
    assert!(
        reference.chunks(4).any(|p| p[0] == 255),
        "the reference has text"
    );
    assert!(
        evicting == reference,
        "the evicting scene draws what a fresh one draws"
    );
}

#[test]
fn ui2_layers_draw_map_hud_window_tooltip_one_call_each() {
    let Some(mut rig) = Rig::new(
        "ui2_layers_draw_in_order",
        wgpu::Backends::PRIMARY,
        (64, 32),
    ) else {
        return;
    };
    let mut scene = Scene::new(1.0);
    let full = Rect::new(0.0, 0.0, 40.0, 20.0);
    // Inserted in reverse: the layer, not the call order, decides.
    scene.set_chunk(
        LayerId::Tooltip,
        ChunkId(4),
        chunk(vec![Item::Box(BoxItem::solid(
            Rect::new(0.0, 0.0, 10.0, 20.0),
            YELLOW,
        ))]),
    );
    scene.set_chunk(
        LayerId::Window,
        ChunkId(3),
        chunk(vec![Item::Box(BoxItem::solid(
            Rect::new(0.0, 0.0, 20.0, 20.0),
            GREEN,
        ))]),
    );
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(2),
        chunk(vec![Item::Box(BoxItem::solid(
            Rect::new(0.0, 0.0, 30.0, 20.0),
            BLUE,
        ))]),
    );
    scene.set_chunk(
        LayerId::Map,
        ChunkId(1),
        chunk(vec![Item::Box(BoxItem::solid(full, RED))]),
    );
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, 64, 5, 10), [255, 255, 0]);
    assert_eq!(rgb(&img, 64, 15, 10), [0, 255, 0]);
    assert_eq!(rgb(&img, 64, 25, 10), [0, 0, 255]);
    assert_eq!(rgb(&img, 64, 35, 10), [255, 0, 0]);
    assert_eq!(rgb(&img, 64, 45, 10), [0, 0, 0]);
    let s = rig.pass.stats();
    assert_eq!(s.layer_draws, [1, 1, 1, 1]);
    assert_eq!(s.draws, 4);
    assert_eq!(s.instances, 4);
    let names: Vec<_> = s.counters().iter().map(|(n, _)| *n).collect();
    for n in [
        "ui2.draws",
        "ui2.draws.map",
        "ui2.draws.hud",
        "ui2.draws.window",
        "ui2.upload_bytes",
    ] {
        assert!(names.contains(&n), "{n}");
    }
}

#[test]
fn ui2_anchors_and_element_transforms_move_boxes() {
    let Some(mut rig) = Rig::new("ui2_anchors_move_boxes", wgpu::Backends::PRIMARY, (64, 48))
    else {
        return;
    };
    let mut scene = Scene::new(1.0);
    let slot = scene.alloc_anchor().unwrap();
    scene.set_anchor(
        slot,
        AnchorXform {
            pos: Vec2::new(30.0, 10.0),
            scale: 2.0,
            visible: true,
        },
    );
    let mut b = ChunkBuilder::new().anchor(slot);
    // 0..10 in chunk space, moved 2 right by its own transform: (2..12) x 2 + 30 = 34..54 on screen.
    b.push(Item::Box(BoxItem {
        transform: Some(Affine2::from_translation(Vec2::new(2.0, 0.0))),
        ..BoxItem::solid(Rect::new(0.0, 0.0, 10.0, 10.0), GREEN)
    }));
    scene.set_chunk(LayerId::Map, ChunkId(1), b.build());
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, 64, 44, 20), [0, 255, 0]);
    assert_eq!(rgb(&img, 64, 35, 11), [0, 255, 0]);
    assert_eq!(rgb(&img, 64, 32, 20), [0, 0, 0]);
    assert_eq!(rgb(&img, 64, 44, 31), [0, 0, 0]);

    // Moving the anchor uploads its 32 bytes, nothing else.
    scene.set_anchor(
        slot,
        AnchorXform {
            pos: Vec2::new(0.0, 0.0),
            scale: 1.0,
            visible: true,
        },
    );
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, 64, 5, 5), [0, 255, 0]);
    assert_eq!(rgb(&img, 64, 44, 20), [0, 0, 0]);
    let s = rig.pass.stats();
    assert_eq!((s.upload_bytes, s.layers_uploaded), (32, 0));

    // A hidden anchor draws nothing.
    scene.set_anchor(
        slot,
        AnchorXform {
            pos: Vec2::new(0.0, 0.0),
            scale: 1.0,
            visible: false,
        },
    );
    let img = rig.frame(&mut scene);
    assert!(img.chunks(4).all(|p| p[..3] == [0, 0, 0]));
}

/// A package page table: an `A8` page 32 x 32 (left half covered), an `Rgba8` page 16 x 16 (top half opaque red,
/// bottom half blue at alpha 128, premultiplied).
fn picture_pages() -> (ui2_package::Images, Vec<Arc<[u8]>>) {
    use ui2_package::{BlobRef, BlobSection, Page, PageFormat};
    let page = |format, size| Page {
        format,
        scale: 1,
        size,
        blob: BlobRef {
            section: BlobSection::Imgs,
            offset: 0,
            len: 0,
            raw_len: 0,
            zstd: false,
        },
        blake3: [0; 32],
    };
    let mask: Vec<u8> = (0..32 * 32)
        .map(|i| if i % 32 < 16 { 255 } else { 0 })
        .collect();
    let rgba: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            if i / 16 < 8 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 128, 128]
            }
        })
        .collect();
    (
        ui2_package::Images {
            pages: vec![
                page(ui2_package::PageFormat::A8, [32, 32]),
                page(PageFormat::Rgba8, [16, 16]),
            ],
            imgs: Vec::new(),
        },
        vec![Arc::from(mask), Arc::from(rgba)],
    )
}

fn pictures_on(test: &str, backends: wgpu::Backends) {
    let Some(mut rig) = Rig::new(test, backends, (160, 80)) else {
        return;
    };
    let mut text = engine();
    let mut scene = Scene::new(1.0);
    let (images, pages) = picture_pages();
    scene.set_pages(&images, pages.clone()).expect("pages");
    let whole = [0.0, 0.0, 1.0, 1.0];
    let run = text
        .shape(&RunStyle::new(FIRA, 14.0), "Hamburgefonstiv")
        .expect("shape");
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(vec![
            Item::Mask(MaskItem {
                rect: Rect::new(0.0, 0.0, 32.0, 32.0),
                mask: ImageRef {
                    layer: 0,
                    uv: whole,
                },
                color: GREEN,
                ..MaskItem::default()
            }),
            Item::Image(ImageItem {
                rect: Rect::new(40.0, 0.0, 32.0, 32.0),
                image: ImageRef {
                    layer: 0,
                    uv: whole,
                },
                ..ImageItem::default()
            }),
            Item::Image(ImageItem {
                rect: Rect::new(80.0, 0.0, 32.0, 32.0),
                image: ImageRef {
                    layer: 0,
                    uv: whole,
                },
                tint: Color::rgba(255, 255, 255, 128),
                ..ImageItem::default()
            }),
            Item::Glyphs(Glyphs::from_run(&run, 1.0, 10.0, 60.0, WHITE)),
        ]),
    );
    scene.resolve(&mut text, 1000.0);
    let img = rig.frame(&mut scene);
    let w = rig.size.0;
    // The mask icon: the covered half in the paint colour, the other half empty.
    assert_eq!(rgb(&img, w, 6, 16), [0, 255, 0]);
    assert_eq!(rgb(&img, w, 26, 16), [0, 0, 0]);
    // The picture: opaque red on top, half-transparent blue (premultiplied) over black below.
    assert_eq!(rgb(&img, w, 56, 4), [255, 0, 0]);
    let blue = rgb(&img, w, 56, 28);
    assert_eq!(&blue[..2], &[0, 0]);
    assert!((126..=130).contains(&blue[2]), "{blue:?}");
    // The same picture at half opacity.
    let red = rgb(&img, w, 96, 4);
    assert!(
        (126..=130).contains(&red[0]) && red[1..] == [0, 0],
        "{red:?}"
    );
    // The glyphs moved one layer up (after the mask page) and still draw.
    let ink: u64 = (46..64)
        .flat_map(|y| (10..150).map(move |x| (x, y)))
        .map(|(x, y)| u64::from(px(&img, w, x, y)[1]))
        .sum();
    assert!(ink > 255 * 100, "the line has ink: {ink}");
    let s = rig.pass.stats();
    assert_eq!(s.draws, 1, "pages, masks and glyphs share the draw");
    assert!(s.page_bytes > 0 && s.glyph_bytes > 0);

    // Idle frames, and the same pages again, upload nothing.
    let _ = rig.frame(&mut scene);
    assert_eq!(rig.pass.stats().upload_bytes, 0);
    scene.set_pages(&images, pages).expect("same pages");
    let _ = rig.frame(&mut scene);
    let idle = rig.pass.stats();
    assert_eq!((idle.upload_bytes, idle.layers_uploaded), (0, 0));

    // Dropping the pages (a skin without pictures): the pictures make no instance any more.
    scene
        .set_pages(&ui2_package::Images::default(), Vec::new())
        .expect("no pages");
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, w, 56, 4), [0, 0, 0]);
    assert_eq!(rgb(&img, w, 96, 4), [0, 0, 0]);
    assert_eq!(rig.pass.stats().page_bytes, 0);
}

#[test]
fn ui2_draws_package_pictures_and_mask_icons() {
    pictures_on(
        "ui2_draws_package_pictures_and_mask_icons",
        wgpu::Backends::PRIMARY,
    );
}

#[test]
fn ui2_draws_package_pictures_on_gl() {
    pictures_on("ui2_draws_package_pictures_on_gl", wgpu::Backends::GL);
}

/// Coverage (0..255) of the red channel at `(x, y)` over black.
fn red(img: &[u8], w: u32, x: u32, y: u32) -> i32 {
    i32::from(px(img, w, x, y)[0])
}

/// Asserts that `got` is within 2 levels of `want` (0..1 coverage).
#[track_caller]
fn near(got: i32, want: f32, what: &str) {
    let want = (want * 255.0).round() as i32;
    assert!((got - want).abs() <= 2, "{what}: {got}, want {want}");
}

/// Test 2 (boxes): per-corner radii, a fractional edge, 1 px and 2 px borders, a border-only box; every edge
/// pixel against the analytic coverage.
fn boxes_on(test: &str, backends: wgpu::Backends) {
    let Some(mut rig) = Rig::new(test, backends, (160, 80)) else {
        return;
    };
    let mut scene = Scene::new(1.0);
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(vec![
            // Per-corner radii: tl 0, tr 4, br 8, bl 16.
            Item::Box(BoxItem {
                radii: Radii {
                    tl: 0.0,
                    tr: 4.0,
                    br: 8.0,
                    bl: 16.0,
                },
                ..BoxItem::solid(Rect::new(4.0, 4.0, 40.0, 40.0), RED)
            }),
            // Left edge at 50.5 and right edge at 69.25: half and quarter covered pixels.
            Item::Box(BoxItem::solid(Rect::new(50.5, 4.0, 18.75, 20.0), RED)),
            // A key: border 1 px on three sides, 2 px at the bottom, white over a red fill.
            Item::Box(BoxItem {
                rect: Rect::new(80.0, 4.0, 30.0, 20.0),
                fill: Fill::Solid(RED),
                border: Border {
                    top: 1.0,
                    right: 1.0,
                    bottom: 2.0,
                    left: 1.0,
                    color: WHITE,
                },
                ..BoxItem::default()
            }),
            // Border only, 2 px, rounded 6: the inside stays black.
            Item::Box(BoxItem {
                rect: Rect::new(120.0, 4.0, 30.0, 30.0),
                border: Border::uniform(2.0, RED),
                radii: Radii::uniform(6.0),
                ..BoxItem::default()
            }),
        ]),
    );
    let img = rig.frame(&mut scene);
    let w = rig.size.0;

    // Per-corner radii: the square corner is fully covered, the others cut by their arcs.
    assert_eq!(red(&img, w, 4, 4), 255, "square top left corner");
    for (cx, cy, r, sx, sy) in [
        (44.0 - 4.0, 4.0 + 4.0, 4.0, 1.0, -1.0),
        (44.0 - 8.0, 44.0 - 8.0, 8.0, 1.0, 1.0),
        (4.0 + 16.0, 44.0 - 16.0, 16.0, -1.0, 1.0),
    ] {
        // Pixels along the diagonal through the arc centre.
        for i in 0..(r as i32 + 2) {
            let (x, y) = (cx + sx * (i as f32 + 0.5), cy + sy * (i as f32 + 0.5));
            if !(4.0..44.0).contains(&x) || !(4.0..44.0).contains(&y) {
                continue;
            }
            let d = Vec2::new(x - cx, y - cy).length() - r;
            near(
                red(&img, w, x as u32, y as u32),
                (0.5 - d).clamp(0.0, 1.0),
                &format!("arc r {r} at ({x}, {y})"),
            );
        }
    }
    // Fractional edges.
    near(red(&img, w, 50, 10), 0.5, "left edge at 50.5");
    assert_eq!(red(&img, w, 51, 10), 255);
    assert_eq!(red(&img, w, 68, 10), 255);
    near(red(&img, w, 69, 10), 0.25, "right edge at 69.25");
    assert_eq!(red(&img, w, 70, 10), 0);
    // The key: 1 px white top, left and right, 2 px white bottom, red inside, black outside.
    assert_eq!(rgb(&img, w, 95, 4), [255, 255, 255], "top border");
    assert_eq!(
        rgb(&img, w, 95, 5),
        [255, 0, 0],
        "fill under the top border"
    );
    assert_eq!(rgb(&img, w, 80, 14), [255, 255, 255], "left border");
    assert_eq!(rgb(&img, w, 81, 14), [255, 0, 0]);
    assert_eq!(rgb(&img, w, 109, 14), [255, 255, 255], "right border");
    assert_eq!(
        rgb(&img, w, 95, 21),
        [255, 0, 0],
        "fill above the bottom border"
    );
    assert_eq!(
        rgb(&img, w, 95, 22),
        [255, 255, 255],
        "bottom border, first row"
    );
    assert_eq!(
        rgb(&img, w, 95, 23),
        [255, 255, 255],
        "bottom border, second row"
    );
    assert_eq!(rgb(&img, w, 95, 24), [0, 0, 0], "below the key");
    // Border only: the ring is 2 px, the inside is black.
    assert_eq!(red(&img, w, 135, 4), 255);
    assert_eq!(red(&img, w, 135, 5), 255);
    assert_eq!(red(&img, w, 135, 6), 0);
    assert_eq!(red(&img, w, 135, 19), 0, "border-only centre");
    assert_eq!(red(&img, w, 120, 4), 0, "outside the rounded corner");
}

#[test]
fn ui2_box_radii_borders_and_edges_match_the_analytic_coverage() {
    boxes_on(
        "ui2_box_radii_borders_and_edges_match_the_analytic_coverage",
        wgpu::Backends::PRIMARY,
    );
}

#[test]
fn ui2_box_coverage_on_gl() {
    boxes_on("ui2_box_coverage_on_gl", wgpu::Backends::GL);
}

/// Test 2 (masks): the icons of the testkit page drawn 1:1 reproduce the page's coverage exactly, in the paint
/// colour; drawn 2x they keep it in the middle of a covered area.
#[test]
fn ui2_mask_icons_reproduce_the_page_coverage() {
    let Some(mut rig) = Rig::new(
        "ui2_mask_icons_reproduce_the_page_coverage",
        wgpu::Backends::PRIMARY,
        (96, 48),
    ) else {
        return;
    };
    let mut scene = Scene::new(1.0);
    let (images, pages) = testkit::icon_pages();
    scene.set_pages(&images, pages.clone()).expect("pages");
    let mut items = Vec::new();
    for n in 0..4u8 {
        items.push(Item::Mask(MaskItem {
            rect: Rect::new(f32::from(n) * 20.0, 0.0, 16.0, 16.0),
            mask: testkit::icon(n),
            color: YELLOW,
            ..MaskItem::default()
        }));
    }
    items.push(Item::Mask(MaskItem {
        rect: Rect::new(0.0, 16.0, 32.0, 32.0),
        mask: testkit::icon(1),
        color: GREEN,
        ..MaskItem::default()
    }));
    scene.set_chunk(LayerId::Hud, ChunkId(1), chunk(items));
    let img = rig.frame(&mut scene);
    let w = rig.size.0;
    let side = usize::from(testkit::ICON_PAGE);
    for n in 0..4u32 {
        for y in 0..16u32 {
            for x in 0..16u32 {
                let want = pages[0][y as usize * side + (n * 16 + x) as usize];
                let got = px(&img, w, n * 20 + x, y);
                assert_eq!(
                    [got[0], got[1], got[2]],
                    [want, want, 0],
                    "icon {n} ({x}, {y})"
                );
            }
        }
    }
    // The square (10 x 10 in its cell) at 2x: 20 x 20 covered in the middle.
    assert_eq!(rgb(&img, w, 16, 32), [0, 255, 0]);
    assert_eq!(rgb(&img, w, 2, 18), [0, 0, 0]);
    assert_eq!(rig.pass.stats().draws, 1);
}

fn stops(s: &[(Color, f32)]) -> Vec<GradientStop> {
    s.iter().map(|(c, p)| GradientStop::new(*c, *p)).collect()
}

fn gradient_box(rect: Rect, g: Gradient) -> Item {
    Item::Box(BoxItem {
        rect,
        fill: Fill::Gradient(g),
        ..BoxItem::default()
    })
}

/// A pixel `(x, y)` at `deg` degrees clockwise from the top and `r` px from `c`.
fn polar(c: Vec2, r: f32, deg: f32) -> (u32, u32) {
    let a = deg.to_radians();
    let p = c + Vec2::new(a.sin(), -a.cos()) * r;
    (p.x as u32, p.y as u32)
}

/// Test 2 (the rest): gradients (hard stops, smooth, repeating, conic, repeating conic, tiled), an elliptic
/// ring, shadows (hard with spread, blurred, offset), a polygon clip (a bevelled plate) and a rotated box.
fn primitives_on(test: &str, backends: wgpu::Backends) {
    let Some(mut rig) = Rig::new(test, backends, (200, 130)) else {
        return;
    };
    let v = Vec2::new;
    let tr = Color::TRANSPARENT;
    let hard = stops(&[(RED, 0.0), (RED, 0.5), (GREEN, 0.5), (GREEN, 1.0)]);
    let mut items = vec![
        // Hard stop in the middle of a 40 px line.
        gradient_box(
            Rect::new(0.0, 0.0, 40.0, 20.0),
            Gradient::linear(v(0.0, 0.0), v(40.0, 0.0), &hard).unwrap(),
        ),
        // Red to blue over 40 px.
        gradient_box(
            Rect::new(0.0, 20.0, 41.0, 10.0),
            Gradient::linear(
                v(0.5, 0.0),
                v(40.5, 0.0),
                &stops(&[(RED, 0.0), (BLUE, 1.0)]),
            )
            .unwrap(),
        ),
        // Stripes: 5 px red, 5 px nothing.
        gradient_box(
            Rect::new(50.0, 0.0, 40.0, 20.0),
            Gradient::linear(
                v(0.0, 0.0),
                v(10.0, 0.0),
                &stops(&[(RED, 0.0), (RED, 0.5), (tr, 0.5), (tr, 1.0)]),
            )
            .unwrap()
            .repeating(),
        ),
        // Conic halves: red on the right (from the top, clockwise), blue on the left; then turned a quarter.
        gradient_box(
            Rect::new(100.0, 0.0, 40.0, 40.0),
            Gradient::conic(
                v(20.0, 20.0),
                0.0,
                &stops(&[(RED, 0.0), (RED, 0.5), (BLUE, 0.5), (BLUE, 1.0)]),
            )
            .unwrap(),
        ),
        gradient_box(
            Rect::new(150.0, 0.0, 40.0, 40.0),
            Gradient::conic(
                v(20.0, 20.0),
                0.25,
                &stops(&[(RED, 0.0), (RED, 0.5), (BLUE, 0.5), (BLUE, 1.0)]),
            )
            .unwrap(),
        ),
        // Repeating conic: 45 degrees red, 45 nothing.
        gradient_box(
            Rect::new(0.0, 40.0, 40.0, 40.0),
            Gradient::conic(
                v(20.0, 20.0),
                0.0,
                &stops(&[(RED, 0.0), (RED, 0.125), (tr, 0.125), (tr, 0.25)]),
            )
            .unwrap()
            .repeating(),
        ),
        // The hard stop gradient in 10 x 20 tiles.
        gradient_box(
            Rect::new(50.0, 40.0, 40.0, 20.0),
            Gradient::linear(v(0.0, 0.0), v(10.0, 0.0), &hard)
                .unwrap()
                .tiled(v(10.0, 20.0)),
        ),
        // An elliptic ring, 2 px, on a 60 x 24 box (CSS `border-radius: 50%`).
        Item::EllipticBox(EllipticBox::ellipse(
            Rect::new(100.0, 44.0, 60.0, 24.0),
            Fill::None,
            Border::uniform(2.0, WHITE),
        )),
        // Shadows: hard with a 4 px spread, blurred (sigma 4), offset 6 px down.
        Item::Shadow(ShadowItem {
            rect: Rect::new(20.0, 90.0, 20.0, 20.0),
            spread: 4.0,
            color: RED,
            ..ShadowItem::default()
        }),
        Item::Shadow(ShadowItem {
            rect: Rect::new(60.0, 90.0, 20.0, 20.0),
            blur: 8.0,
            color: RED,
            ..ShadowItem::default()
        }),
        Item::Shadow(ShadowItem {
            rect: Rect::new(170.0, 70.0, 20.0, 10.0),
            offset: v(0.0, 6.0),
            color: GREEN,
            ..ShadowItem::default()
        }),
        // A box rotated by 45 degrees about its centre (160, 110).
        Item::Box(BoxItem {
            transform: Some(
                Affine2::from_translation(v(160.0, 110.0))
                    * Affine2::from_angle(std::f32::consts::FRAC_PI_4)
                    * Affine2::from_translation(v(-160.0, -110.0)),
            ),
            ..BoxItem::solid(Rect::new(150.0, 100.0, 20.0, 20.0), YELLOW)
        }),
    ];
    // A plate bevelled at the top left: polygon clip and the engraved gradient line along the cut.
    items.extend(testkit::plate(
        Rect::new(100.0, 76.0, 44.0, 40.0),
        12.0,
        [true, false, false, false],
    ));
    let mut scene = Scene::new(1.0);
    scene.set_chunk(LayerId::Hud, ChunkId(1), chunk(items));
    let img = rig.frame(&mut scene);
    let w = rig.size.0;
    let c = |x, y| rgb(&img, w, x, y);

    // Hard stop: pixel 19 (centre 19.5) is red, pixel 20 green.
    assert_eq!(c(10, 10), [255, 0, 0]);
    assert_eq!(c(19, 10), [255, 0, 0]);
    assert_eq!(c(20, 10), [0, 255, 0]);
    assert_eq!(c(39, 10), [0, 255, 0]);
    // Smooth: half way at pixel 20, a quarter at pixel 10.
    let mid = c(20, 25);
    assert!(
        (126..=130).contains(&mid[0]) && mid[1] == 0 && (126..=130).contains(&mid[2]),
        "{mid:?}"
    );
    let q = c(10, 25);
    assert!(
        (189..=193).contains(&q[0]) && (62..=66).contains(&q[2]),
        "{q:?}"
    );
    // Stripes.
    for (x, on) in [
        (52, true),
        (57, false),
        (62, true),
        (67, false),
        (87, false),
    ] {
        assert_eq!(c(x, 10)[0] == 255, on, "stripe at {x}");
    }
    // Conic halves.
    assert_eq!(c(130, 20), [255, 0, 0], "right half");
    assert_eq!(c(110, 20), [0, 0, 255], "left half");
    assert_eq!(
        c(170, 30),
        [255, 0, 0],
        "bottom half when started at a quarter"
    );
    assert_eq!(
        c(170, 10),
        [0, 0, 255],
        "top half when started at a quarter"
    );
    // Repeating conic sectors of 45 degrees.
    let centre = v(20.0, 60.0);
    for (deg, on) in [
        (22.5, true),
        (67.5, false),
        (112.5, true),
        (157.5, false),
        (337.5, false),
    ] {
        let (x, y) = polar(centre, 12.0, deg);
        assert_eq!(c(x, y)[0] == 255, on, "sector at {deg} degrees");
    }
    // Tiles.
    for (x, red) in [(52, true), (57, false), (62, true), (67, false), (82, true)] {
        let want = if red { [255, 0, 0] } else { [0, 255, 0] };
        assert_eq!(c(x, 50), want, "tile at {x}");
    }
    // Elliptic ring: white at the ends of both axes, black in the middle and outside the corner.
    assert_eq!(c(130, 44), [255, 255, 255], "top of the ellipse");
    assert_eq!(c(130, 45), [255, 255, 255]);
    assert_eq!(c(130, 46), [0, 0, 0], "inside the ring");
    assert_eq!(c(130, 67), [255, 255, 255], "bottom of the ellipse");
    // At the ends of the long axis the pixel centre is 0.5 px off the axis, where the ellipse has moved in by
    // 0.03 px: coverage 0.97.
    near(red(&img, w, 100, 56), 0.974, "left end");
    near(red(&img, w, 159, 56), 0.974, "right end");
    assert_eq!(c(130, 56), [0, 0, 0], "centre");
    assert_eq!(c(102, 46), [0, 0, 0], "outside the elliptic corner");
    // Along the diagonal of the bounding box the ring is crossed once: the pixels go black, white, black.
    let diag: Vec<u8> = (0..12).map(|i| c(100 + i * 2, 44 + i)[1]).collect();
    let first = diag
        .iter()
        .position(|g| *g > 200)
        .expect("the ring on the diagonal");
    assert!(
        first > 0 && diag[first + 2..].iter().any(|g| *g < 50),
        "{diag:?}"
    );
    // Hard shadow with spread: a 4 px red ring around the element's box, nothing inside it.
    assert_eq!(c(17, 100), [255, 0, 0], "spread");
    assert_eq!(c(15, 100), [0, 0, 0], "beyond the spread");
    assert_eq!(c(30, 100), [0, 0, 0], "the element's own box");
    assert_eq!(c(30, 112), [255, 0, 0], "spread below");
    // Blurred shadow: Gaussian of sigma 4; half a pixel outside the edge about 0.44 of the colour.
    let edge = red(&img, w, 59, 100);
    assert!((105..=121).contains(&edge), "blurred edge {edge}");
    assert!(red(&img, w, 55, 100) < edge && red(&img, w, 55, 100) > 20);
    assert!(red(&img, w, 47, 100) < 3, "three sigma away");
    assert_eq!(red(&img, w, 70, 100), 0, "the element's own box");
    // Offset shadow: the 6 px below the element only.
    assert_eq!(c(180, 82), [0, 255, 0]);
    assert_eq!(c(180, 85), [0, 255, 0]);
    assert_eq!(c(180, 86), [0, 0, 0]);
    assert_eq!(c(180, 75), [0, 0, 0], "under the element: cut out");
    assert_eq!(c(180, 68), [0, 0, 0]);
    // Rotated box: a diamond around (160, 110).
    assert_eq!(c(160, 110), [255, 255, 0]);
    assert_eq!(
        c(147, 110),
        [255, 255, 0],
        "left tip, outside the unrotated box"
    );
    assert_eq!(c(160, 97), [255, 255, 0], "top tip");
    assert_eq!(c(151, 101), [0, 0, 0], "the unrotated corner");
    // Plate: the cut corner is clipped, the body lacquer, an engraved line along the cut.
    assert_eq!(c(101, 77), [0, 0, 0], "cut corner");
    assert_eq!(c(130, 100), testkit::LACQUER.0[..3]);
    assert_eq!(c(100, 100), testkit::ENGRAVE.0[..3], "the plate's ring");
    let line = (104..110)
        .flat_map(|x| (78..86).map(move |y| (x, y)))
        .filter(|&(x, y)| c(x, y)[1] > 50)
        .count();
    assert!(line >= 4, "engraved pixels along the cut: {line}");
    assert_eq!(rig.pass.stats().draws, 1);
}

#[test]
fn ui2_draws_gradients_ellipses_shadows_polygon_clips_and_rotations() {
    primitives_on(
        "ui2_draws_gradients_ellipses_shadows_polygon_clips_and_rotations",
        wgpu::Backends::PRIMARY,
    );
}

#[test]
fn ui2_draws_gradients_and_shadows_on_gl() {
    primitives_on("ui2_draws_gradients_and_shadows_on_gl", wgpu::Backends::GL);
}

/// The end-turn dial of the mockup: conic scale, rotated ticks, an arc of two clipped halves.
#[test]
fn ui2_draws_the_dial() {
    let Some(mut rig) = Rig::new("ui2_draws_the_dial", wgpu::Backends::PRIMARY, (120, 120)) else {
        return;
    };
    let centre = Vec2::new(60.0, 60.0);
    let mut scene = Scene::new(1.0);
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(testkit::dial(centre, 50.0, 0.7)),
    );
    let img = rig.frame(&mut scene);
    let w = rig.size.0;
    let at = |deg: f32, r: f32| {
        let (x, y) = polar(centre, r, deg);
        rgb(&img, w, x, y)
    };
    let (brass, lacquer, engrave, bone) = (
        &testkit::BRASS.0[..3],
        &testkit::LACQUER.0[..3],
        &testkit::ENGRAVE.0[..3],
        &testkit::BONE.0[..3],
    );
    // The arc covers 0.7 of a turn, clockwise from the top.
    for deg in [10.0, 45.0, 135.0, 179.0, 200.0, 240.0] {
        assert_eq!(&at(deg, 24.0)[..], brass, "arc at {deg}");
    }
    for deg in [260.0, 300.0, 350.0] {
        assert_eq!(&at(deg, 24.0)[..], lacquer, "no arc at {deg}");
    }
    // The conic scale: one engraved degree of every nine.
    assert_eq!(
        &rgb(&img, w, 60, 18)[..],
        engrave,
        "scale tick at 0 degrees"
    );
    assert_eq!(&rgb(&img, w, 63, 18)[..], lacquer, "between scale ticks");
    // A rotated tick at 90 degrees: x 90..96, y 59..61.
    assert_eq!(&rgb(&img, w, 93, 60)[..], bone);
    assert_eq!(rig.pass.stats().draws, 1);
}

/// A colour page of 16 x 16: a 4 px red frame around a green middle (opaque, so premultiplied as it is).
fn frame_page() -> (ui2_package::Images, Vec<Arc<[u8]>>) {
    use ui2_package::{BlobRef, BlobSection, Page, PageFormat};
    let px: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            let (x, y) = (i % 16, i / 16);
            if (4..12).contains(&x) && (4..12).contains(&y) {
                [0, 255, 0, 255]
            } else {
                [255, 0, 0, 255]
            }
        })
        .collect();
    let page = Page {
        format: PageFormat::Rgba8,
        scale: 1,
        size: [16, 16],
        blob: BlobRef {
            section: BlobSection::Imgs,
            offset: 0,
            len: 0,
            raw_len: 0,
            zstd: false,
        },
        blake3: [0; 32],
    };
    (
        ui2_package::Images {
            pages: vec![page],
            imgs: Vec::new(),
        },
        vec![Arc::from(px)],
    )
}

fn nine_on(test: &str, backends: wgpu::Backends) {
    let Some(mut rig) = Rig::new(test, backends, (160, 110)) else {
        return;
    };
    let mut scene = Scene::new(1.0);
    let (images, pages) = frame_page();
    scene.set_pages(&images, pages).expect("page");
    let picture = |rect: Rect, dst: f32, fill: bool| {
        Item::Image(ImageItem {
            rect,
            image: ImageRef {
                layer: 0,
                uv: [0.0, 0.0, 1.0, 1.0],
            },
            nine: Some(NineSlice {
                src: [0.25; 4],
                dst: [dst; 4],
                fill,
            }),
            ..ImageItem::default()
        })
    };
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(vec![
            picture(Rect::new(10.0, 10.0, 60.0, 40.0), 4.0, true),
            picture(Rect::new(90.0, 10.0, 60.0, 40.0), 4.0, false),
            picture(Rect::new(10.0, 60.0, 60.0, 40.0), 8.0, true),
        ]),
    );
    let img = rig.frame(&mut scene);
    let w = rig.size.0;
    let c = |x, y| rgb(&img, w, x, y);
    let (r, g, k) = ([255, 0, 0], [0, 255, 0], [0, 0, 0]);
    // 4 px frame at 1:1: corners and edges red, the stretched middle green, nothing outside.
    for (x, y) in [
        (10, 10),
        (13, 13),
        (40, 10),
        (40, 13),
        (10, 30),
        (69, 49),
        (66, 30),
    ] {
        assert_eq!(c(x, y), r, "frame at ({x}, {y})");
    }
    // (The middle is stretched bilinearly: its pixels next to the seam blend in the frame's last texel, as a
    // stretched picture does.)
    for (x, y) in [(20, 20), (40, 30), (60, 40)] {
        assert_eq!(c(x, y), g, "middle at ({x}, {y})");
    }
    assert_eq!(c(9, 30), k);
    assert_eq!(c(70, 30), k);
    // Without `fill` the middle stays empty.
    assert_eq!(c(120, 11), r);
    assert_eq!(c(120, 30), k, "no middle");
    assert_eq!(c(91, 30), r);
    // Destination widths of 8 px: the 4 px slices drawn twice as large.
    for (x, y) in [(10, 60), (16, 66), (40, 61), (40, 66), (63, 80)] {
        assert_eq!(c(x, y), r, "wide frame at ({x}, {y})");
    }
    assert_eq!(c(40, 80), g);
    let s = rig.pass.stats();
    assert_eq!(s.instances, 9 + 8 + 9);
    assert_eq!(s.draws, 1);
}

#[test]
fn ui2_draws_nine_slices() {
    nine_on("ui2_draws_nine_slices", wgpu::Backends::PRIMARY);
}

#[test]
fn ui2_draws_nine_slices_on_gl() {
    nine_on("ui2_draws_nine_slices_on_gl", wgpu::Backends::GL);
}

fn ext_picture(x: f32, tex: ExtTexture) -> Item {
    Item::Image(ImageItem {
        rect: Rect::new(x, 0.0, 10.0, 10.0),
        image: ImageRef {
            layer: 0,
            uv: [0.0, 0.0, 1.0, 1.0],
        },
        external: Some(tex),
        ..ImageItem::default()
    })
}

fn external_on(test: &str, backends: wgpu::Backends) {
    let Some(mut rig) = Rig::new(test, backends, (100, 20)) else {
        return;
    };
    let (a, b, scaled, missing) = (
        ExtTexture(0x51),
        ExtTexture::MINIMAP,
        ExtTexture(0x52),
        ExtTexture(0x53),
    );
    rig.add_solid_ext(a.0, BLUE);
    rig.add_solid_ext(b.0, YELLOW);
    // 8 x 8 stored, the picture is its top left 4 x 4 (white); the rest is red padding.
    let px: Vec<u8> = (0..64)
        .flat_map(|i| {
            if i % 8 < 4 && i / 8 < 4 {
                [255, 255, 255, 255]
            } else {
                [255, 0, 0, 255]
            }
        })
        .collect();
    rig.add_ext(scaled.0, 8, 8, &px, [0.5, 0.5]);
    let boxed = |x: f32, col: Color| Item::Box(BoxItem::solid(Rect::new(x, 0.0, 10.0, 10.0), col));
    let mut scene = Scene::new(1.0);
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(vec![
            boxed(0.0, RED),
            ext_picture(10.0, a),
            boxed(20.0, GREEN),
            boxed(30.0, GREEN),
            ext_picture(40.0, a),
            ext_picture(50.0, b),
            ext_picture(60.0, scaled),
            ext_picture(70.0, missing),
            boxed(80.0, WHITE),
        ]),
    );
    let img = rig.frame(&mut scene);
    let w = rig.size.0;
    let c = |x| rgb(&img, w, x, 5);
    assert_eq!(c(5), [255, 0, 0]);
    assert_eq!(c(15), [0, 0, 255]);
    assert_eq!(c(25), [0, 255, 0]);
    assert_eq!(c(45), [0, 0, 255]);
    assert_eq!(c(55), [255, 255, 0]);
    assert_eq!(c(65), [255, 255, 255], "the UV scale keeps the padding out");
    assert_eq!(
        c(75),
        [0, 0, 0],
        "a texture that is not there draws nothing"
    );
    assert_eq!(c(85), [255, 255, 255]);
    let s = rig.pass.stats();
    // Runs: box | a | box box | a | b | scaled | (missing) | box. Only a texture change splits the draw.
    assert_eq!(s.draws, 7, "{s:?}");
    assert_eq!(s.ext_missing, 1);
    assert_eq!(s.ext_groups_made, 3);
    assert!(rig.ext.asked.contains(&missing.0));

    // An idle frame makes no group and uploads nothing.
    let _ = rig.frame(&mut scene);
    let idle = rig.pass.stats();
    assert_eq!(
        (idle.upload_bytes, idle.ext_groups_made, idle.draws),
        (0, 0, 7)
    );
    // A new upload of a texture makes its group again; the missing one arriving draws it.
    rig.add_solid_ext(a.0, GREEN);
    rig.add_solid_ext(missing.0, RED);
    let img = rig.frame(&mut scene);
    assert_eq!(rgb(&img, w, 15, 5), [0, 255, 0]);
    assert_eq!(rgb(&img, w, 75, 5), [255, 0, 0]);
    let s = rig.pass.stats();
    assert_eq!((s.ext_groups_made, s.ext_missing, s.draws), (2, 0, 8));
}

#[test]
fn ui2_external_textures_split_the_batch_only_where_they_change() {
    assert_eq!(ExtTexture::MINIMAP.0, draw_list::TextureId::MINIMAP.0);
    external_on(
        "ui2_external_textures_split_the_batch_only_where_they_change",
        wgpu::Backends::PRIMARY,
    );
}

#[test]
fn ui2_external_textures_on_gl() {
    external_on("ui2_external_textures_on_gl", wgpu::Backends::GL);
}

/// Test 3 (draw call budget) and test 4 (an unchanged frame uploads nothing), on the HUD of the reference
/// mockup and 200 city banners.
#[test]
fn ui2_hud_and_200_banners_stay_within_the_draw_budget_and_idle_frames_upload_nothing() {
    let size = (1920, 1080);
    let Some(mut rig) = Rig::new("ui2_hud_and_200_banners", wgpu::Backends::PRIMARY, size) else {
        return;
    };
    // The portrait (a game asset) and the minimap target.
    rig.add_solid_ext(testkit::PORTRAIT.0, BLUE);
    rig.add_solid_ext(ExtTexture::MINIMAP.0, GREEN);
    let mut text = engine();
    let mut scene = Scene::new(1.0);
    testkit::load_hud_a(&mut scene, &mut text, FIRA);
    let anchors = testkit::map_banners(
        &mut scene,
        &mut text,
        FIRA,
        200,
        Vec2::new(size.0 as f32, size.1 as f32),
    );
    assert_eq!(anchors.len(), 200);
    scene.resolve(&mut text, 1000.0);
    assert_eq!(scene.stats().raster_deferred, 0);

    let img = rig.frame(&mut scene);
    let s = rig.pass.stats();
    let (draws, _) = rig.pass.draw_totals();
    assert!(draws <= 30, "{draws} draw calls");
    assert!(draws <= 6, "{draws} draw calls, expected at most 6");
    assert!(s.layer_draws[LayerId::Map.index()] <= 3, "{s:?}");
    // The HUD splits only around the portrait and the minimap: at most five runs.
    assert!(s.layer_draws[LayerId::Hud.index()] <= 5, "{s:?}");
    assert_eq!(s.ext_missing, 0);
    assert_eq!(rgb(&img, size.0, 468, 948), [0, 0, 255], "portrait");
    assert_eq!(rgb(&img, size.0, 300, 1000), [0, 255, 0], "minimap");
    assert!(s.instances > 1000, "{s:?}");
    // Something of the HUD and of the banners is on screen: the strip's lacquer, a banner's brass tab.
    assert_eq!(rgb(&img, size.0, 960, 2), testkit::LACQUER.0[..3]);
    assert!(
        img.chunks(4).any(|p| p[..3] == testkit::BRASS.0[..3]),
        "a banner tab"
    );

    // Test 4: the same scene again uploads nothing and records the same draws.
    let _ = rig.frame(&mut scene);
    let idle = rig.pass.stats();
    assert_eq!(
        (idle.upload_bytes, idle.layers_uploaded),
        (0, 0),
        "{idle:?}"
    );
    assert_eq!(idle.draws, s.draws);
    // Setting every chunk again with equal items changes nothing either.
    for (layer, id, items) in testkit::hud_a(&mut text, FIRA, 1.0) {
        scene.set_chunk(layer, id, items);
    }
    scene.resolve(&mut text, 1000.0);
    let _ = rig.frame(&mut scene);
    let again = rig.pass.stats();
    assert_eq!(
        (again.upload_bytes, again.layers_uploaded),
        (0, 0),
        "{again:?}"
    );
}

fn headless(test: &str) -> Option<crate::Renderer> {
    match block_on(crate::Renderer::new_headless(
        (64, 48),
        crate::renderer::BackendPref::Auto,
    )) {
        Ok(r) => Some(r),
        Err(e) => {
            eprintln!("SKIP {test}: {e}");
            None
        }
    }
}

fn frame_with(r: &mut crate::Renderer, ui: &UiDrawList) -> crate::Image {
    let camera = crate::GameCamera::new(Vec2::ZERO);
    let readback = r.read_frame(crate::FrameSource::WithUi);
    r.render(&crate::Scene::test(0.0), &camera, ui, None)
        .unwrap();
    readback.wait().unwrap()
}

fn ui2_row(stats: &crate::FrameStats) -> Option<crate::PassStat> {
    stats
        .passes
        .iter()
        .find(|p| p.pass == crate::frame::PassId::Ui2)
        .copied()
}

fn has_ui2_counters(stats: &crate::FrameStats) -> bool {
    stats.counters.iter().any(|(n, _)| n.starts_with("ui2."))
}

#[test]
fn ui2_in_a_headless_frame_runs_only_when_prepared_and_under_the_original_ui() {
    let Some(mut r) = headless("ui2_in_a_headless_frame") else {
        return;
    };
    // No scene: no row, no counters, the same draws as before.
    let before = frame_with(&mut r, &UiDrawList::new());
    let stats = r.last_frame_stats().clone();
    assert!(ui2_row(&stats).is_none());
    assert!(!has_ui2_counters(&stats));
    let draws_without = stats.draws;
    assert_eq!(r.ui2_stats(), Ui2Stats::default());

    let mut scene = Scene::new(1.0);
    scene.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk(vec![Item::Box(BoxItem::solid(
            Rect::new(4.0, 4.0, 30.0, 30.0),
            GREEN,
        ))]),
    );
    // The original UI draws over UI v2.
    let mut ui = UiDrawList::new();
    ui.add_filled_rect(
        Vec2::new(20.0, 20.0),
        Vec2::new(40.0, 40.0),
        [255, 0, 255, 255],
    );
    r.prepare_ui2(&mut scene);
    let img = frame_with(&mut r, &ui);
    let stats = r.last_frame_stats().clone();
    assert_eq!(&img.pixel(10, 10)[..3], &[0, 255, 0]);
    assert_eq!(
        &img.pixel(25, 25)[..3],
        &[255, 0, 255],
        "UiGame is over Ui2"
    );
    let row = ui2_row(&stats).expect("the pass ran");
    assert_eq!((row.draws, row.triangles), (1, 2));
    let at = |p| stats.passes.iter().position(|s| s.pass == p).unwrap();
    use crate::frame::PassId;
    assert!(at(PassId::Present) < at(PassId::Ui2) && at(PassId::Ui2) < at(PassId::UiGame));
    let counter = |name: &str| stats.counters.iter().find(|(n, _)| *n == name).map(|c| c.1);
    assert_eq!(counter("ui2.draws"), Some(1));
    assert_eq!(counter("ui2.draws.hud"), Some(1));
    assert_eq!(r.ui2_stats().draws, 1);

    // Not prepared again: the frame is the one without UI v2.
    let after = frame_with(&mut r, &UiDrawList::new());
    let stats = r.last_frame_stats().clone();
    assert!(ui2_row(&stats).is_none());
    assert!(!has_ui2_counters(&stats));
    assert_eq!(stats.draws, draws_without);
    assert_eq!(after.pixel(10, 10), before.pixel(10, 10));
}
