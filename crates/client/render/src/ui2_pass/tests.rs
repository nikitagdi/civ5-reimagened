//! Tests of the UI v2 pass: the shader against naga (WGSL validation, GLSL ES 3.00 output as WebGL2 gets it,
//! the WebGL2 limits), then GPU tests that draw a small scene into an `Rgba8Unorm` texture and check pixels (a
//! test that finds no adapter prints why it skips), and the pass in a whole headless frame.

use std::path::PathBuf;
use std::sync::Arc;

use draw_list::UiDrawList;
use glam::{Affine2, Vec2};
use ui2_draw::instance::{ATTRIBUTES, INSTANCE_SIZE};
use ui2_draw::{
    AnchorXform, Border, BoxItem, ChunkBuilder, ChunkId, Clip, Color, FaceId, Fill, Glyphs,
    ImageItem, ImageRef, Item, LayerId, MaskItem, Radii, Rect, Scene,
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
    // Room is left for the gradient and image tasks.
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

struct Rig {
    gpu: Gpu,
    target: Tex,
    view: wgpu::TextureView,
    pass: Ui2Pass,
    size: (u32, u32),
}

impl Rig {
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
        self.pass
            .prepare(&self.gpu.device, &self.gpu.queue, scene, &target);
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
