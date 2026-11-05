//! GPU tests of the pass. Each draws into a small `Rgba8Unorm` texture and checks pixels (a test
//! that finds no adapter prints why it skips). Colours are exact where the maths is exact and
//! within two levels where blending rounds.

use draw_list::{
    ClipRect, TextureId, UiBlend, UiDrawList, UiMaterial, UiSampler, UiShade, UiVertex,
};
use glam::Vec2;

use asset_format::TextureFormat;

use super::*;
use crate::gpu_test::{Gpu, gpu, read_texture};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Rig {
    gpu: Gpu,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    pass: UiPass,
    textures: UiTextures,
    size: (u32, u32),
}

impl Rig {
    fn new(test: &str, size: (u32, u32)) -> Option<Rig> {
        Self::in_format(test, size, FORMAT)
    }

    fn in_format(test: &str, size: (u32, u32), format: wgpu::TextureFormat) -> Option<Rig> {
        let gpu = gpu(test, wgpu::Backends::PRIMARY, wgpu::Features::empty())?;
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("ui test target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let pass = UiPass::new(&gpu.device, format);
        let textures = UiTextures::new(&gpu.device, &gpu.queue);
        Some(Rig {
            gpu,
            target,
            view,
            pass,
            textures,
            size,
        })
    }

    /// One frame over `clear`: the RGBA bytes of the target.
    fn render(&mut self, list: &UiDrawList, external: &UiExternal<'_>, clear: [f64; 4]) -> Vec<u8> {
        let target = UiTarget {
            format: self.target.format(),
            size: self.size,
        };
        self.pass.prepare(
            &self.gpu.device,
            &self.gpu.queue,
            list,
            &mut self.textures,
            external,
            &target,
        );
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut p = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui test"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: clear[0],
                                g: clear[1],
                                b: clear[2],
                                a: clear[3],
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            if self.pass.has_work() {
                self.pass.draw(&mut p);
            }
        }
        self.gpu.queue.submit([encoder.finish()]);
        read_texture(&self.gpu, &self.target, 0, 0, self.size, 4)
    }

    fn frame(&mut self, list: &UiDrawList) -> Vec<u8> {
        self.render(list, &UiExternal::default(), [0.0, 0.0, 0.0, 1.0])
    }

    fn rgba8(&mut self, id: u64, w: u32, h: u32, texels: &[[u8; 4]]) {
        let data = UiTextureData {
            width: w,
            height: h,
            format: TextureFormat::Rgba8,
            mips: vec![texels.iter().flatten().copied().collect()],
            content: None,
        };
        self.textures.upload(TextureId(id), &data).unwrap();
    }
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * w + x) * 4) as usize;
    [img[at], img[at + 1], img[at + 2], img[at + 3]]
}

fn near(a: [u8; 4], b: [u8; 4], tol: u8) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= tol)
}

fn white() -> [u8; 4] {
    [255; 4]
}

/// Nine quads on a 4x4 vertex grid: corners keep `border` pixels (and `border / tex_size` of UV).
#[allow(clippy::too_many_arguments)]
fn nine_slice(
    list: &mut UiDrawList,
    tex: TextureId,
    tex_size: f32,
    border: f32,
    min: Vec2,
    size: Vec2,
    color: [u8; 4],
) {
    let xs = [
        min.x,
        min.x + border,
        min.x + size.x - border,
        min.x + size.x,
    ];
    let ys = [
        min.y,
        min.y + border,
        min.y + size.y - border,
        min.y + size.y,
    ];
    let us = [0.0, border / tex_size, 1.0 - border / tex_size, 1.0];
    let mut vertices = Vec::new();
    for j in 0..4 {
        for i in 0..4 {
            vertices.push(UiVertex {
                pos: [xs[i], ys[j]],
                uv: [us[i], us[j]],
                color,
            });
        }
    }
    let mut indices = Vec::new();
    for j in 0..3u32 {
        for i in 0..3u32 {
            let a = j * 4 + i;
            indices.extend([a, a + 1, a + 5, a, a + 5, a + 4]);
        }
    }
    list.add_triangles(tex, &vertices, &indices);
}

#[test]
fn a_translucent_rect_blends_over_the_frame() {
    let Some(mut rig) = Rig::new("a_translucent_rect_blends_over_the_frame", (32, 32)) else {
        return;
    };
    let mut list = UiDrawList::new();
    list.add_filled_rect(Vec2::new(4.0, 2.0), Vec2::new(20.0, 10.0), [255, 0, 0, 128]);
    let img = rig.render(&list, &UiExternal::default(), [0.0, 0.0, 1.0, 1.0]);
    // src * a + dst * (1 - a); the destination alpha is kept.
    assert!(
        near(px(&img, 32, 10, 5), [128, 0, 127, 255], 2),
        "{:?}",
        px(&img, 32, 10, 5)
    );
    assert_eq!(px(&img, 32, 3, 5), [0, 0, 255, 255]);
    assert_eq!(px(&img, 32, 20, 5), [0, 0, 255, 255]);
    // Y runs down: rows 2..10 are covered, row 1 and row 10 are not.
    assert_eq!(px(&img, 32, 10, 1), [0, 0, 255, 255]);
    assert_eq!(px(&img, 32, 10, 10), [0, 0, 255, 255]);
    assert!(px(&img, 32, 10, 2)[0] > 100 && px(&img, 32, 10, 9)[0] > 100);
    let s = rig.pass.stats();
    assert_eq!(
        (s.cmds, s.quads, s.verts, s.cmds_drawn, s.cmds_skipped),
        (1, 1, 4, 1, 0)
    );
}

#[test]
fn an_srgb_target_gets_the_same_stored_colours() {
    let Some(mut rig) = Rig::in_format(
        "an_srgb_target_gets_the_same_stored_colours",
        (8, 8),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ) else {
        return;
    };
    let mut list = UiDrawList::new();
    list.add_filled_rect(Vec2::ZERO, Vec2::splat(8.0), [128, 64, 200, 255]);
    let img = rig.frame(&list);
    // The shader linearises, the hardware encodes: the bytes are the vertex colour.
    assert!(
        near(px(&img, 8, 4, 4), [128, 64, 200, 255], 1),
        "{:?}",
        px(&img, 8, 4, 4)
    );
}

#[test]
fn commands_are_drawn_in_list_order_not_batched_by_texture() {
    let Some(mut rig) = Rig::new("commands_are_drawn_in_list_order", (32, 32)) else {
        return;
    };
    rig.rgba8(1, 1, 1, &[[255, 255, 255, 255]]);
    let t = TextureId(1);
    let mut list = UiDrawList::new();
    // red (tex 1), green (white), blue (tex 1): a texture sort would put blue under green.
    list.add_rect(
        t,
        Vec2::new(0.0, 0.0),
        Vec2::new(20.0, 20.0),
        Vec2::ZERO,
        Vec2::ONE,
        [255, 0, 0, 255],
    );
    list.add_filled_rect(
        Vec2::new(10.0, 10.0),
        Vec2::new(30.0, 30.0),
        [0, 255, 0, 255],
    );
    list.add_rect(
        t,
        Vec2::new(15.0, 15.0),
        Vec2::new(25.0, 25.0),
        Vec2::ZERO,
        Vec2::ONE,
        [0, 0, 255, 255],
    );
    assert_eq!(list.commands().len(), 3);
    let img = rig.frame(&list);
    assert_eq!(px(&img, 32, 5, 5), [255, 0, 0, 255]);
    assert_eq!(px(&img, 32, 12, 12), [0, 255, 0, 255]);
    assert_eq!(px(&img, 32, 18, 18), [0, 0, 255, 255]);
    assert_eq!(px(&img, 32, 28, 28), [0, 255, 0, 255]);
}

#[test]
fn corners_keep_their_texels_and_the_middle_stretches() {
    let Some(mut rig) = Rig::new("corners_keep_their_texels", (40, 30)) else {
        return;
    };
    // 6x6, nine 2x2 cells of one colour each.
    let cells: [[[u8; 4]; 3]; 3] = [
        [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]],
        [[255, 255, 0, 255], [0, 255, 255, 255], [255, 0, 255, 255]],
        [
            [255, 255, 255, 255],
            [128, 128, 128, 255],
            [255, 128, 0, 255],
        ],
    ];
    let mut texels = Vec::new();
    for y in 0..6 {
        for x in 0..6 {
            texels.push(cells[y / 2][x / 2]);
        }
    }
    rig.rgba8(7, 6, 6, &texels);
    let mut list = UiDrawList::new();
    nine_slice(
        &mut list,
        TextureId(7),
        6.0,
        2.0,
        Vec2::new(3.0, 2.0),
        Vec2::new(20.0, 14.0),
        white(),
    );
    assert_eq!(list.commands().len(), 1, "nine quads are one command");
    let img = rig.frame(&list);
    let at = |x, y| px(&img, 40, x, y);
    // 1:1 corners on whole pixels: the texels exactly.
    assert_eq!(at(3, 2), cells[0][0]);
    assert_eq!(at(4, 3), cells[0][0]);
    assert_eq!(at(22, 2), cells[0][2]);
    assert_eq!(at(3, 15), cells[2][0]);
    assert_eq!(at(22, 15), cells[2][2]);
    // Stretched cells, away from their borders.
    assert_eq!(at(12, 2), cells[0][1]);
    assert_eq!(at(3, 8), cells[1][0]);
    assert_eq!(at(12, 8), cells[1][1]);
    assert_eq!(at(22, 8), cells[1][2]);
    assert_eq!(at(12, 15), cells[2][1]);
    // Outside: untouched.
    assert_eq!(at(2, 2), [0, 0, 0, 255]);
    assert_eq!(at(23, 15), [0, 0, 0, 255]);
}

/// Covered pixels (centre inside the rectangle) of a translucent nine-slice carry exactly one
/// blend of the colour; a gap shows black, a doubled edge shows brighter.
fn check_seams(rig: &mut Rig, min: Vec2, size: Vec2, border: f32) {
    let (w, h) = rig.size;
    let mut list = UiDrawList::new();
    nine_slice(
        &mut list,
        TextureId::WHITE,
        1.0,
        border,
        min,
        size,
        [255, 255, 255, 128],
    );
    let img = rig.frame(&list);
    let max = min + size;
    let (x0, y0) = ((min.x - 3.0).max(0.0) as u32, (min.y - 3.0).max(0.0) as u32);
    let (x1, y1) = (((max.x + 3.0) as u32).min(w), ((max.y + 3.0) as u32).min(h));
    for y in y0..y1 {
        for x in x0..x1 {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = cx > min.x && cx < max.x && cy > min.y && cy < max.y;
            let want = if inside {
                [128, 128, 128, 255]
            } else {
                [0, 0, 0, 255]
            };
            assert!(
                near(px(&img, w, x, y), want, 1),
                "pixel {x},{y} of {w}x{h}: {:?}, wanted {want:?}",
                px(&img, w, x, y)
            );
        }
    }
    // Far from the rectangle nothing is drawn.
    assert_eq!(px(&img, w, w - 1, h - 1), [0, 0, 0, 255]);
}

#[test]
fn fractional_nine_slices_have_no_seams() {
    let Some(mut rig) = Rig::new("fractional_nine_slices_have_no_seams", (200, 160)) else {
        return;
    };
    for scale in [1.0f32, 1.25, 1.5] {
        check_seams(
            &mut rig,
            Vec2::new(3.3, 2.7) * scale,
            Vec2::new(30.4, 22.2) * scale,
            6.0 * scale,
        );
    }
}

#[test]
fn nine_slices_are_clean_at_every_resolution() {
    for (size, scale) in [
        ((1280, 720), 1.0f32),
        ((1920, 1080), 1.5),
        ((2560, 1440), 1.25),
        ((3840, 2160), 1.5),
    ] {
        let name = format!("nine_slices_are_clean_at_{}x{}", size.0, size.1);
        let Some(mut rig) = Rig::new(&name, size) else {
            return;
        };
        let at = Vec2::new(size.0 as f32 - 95.3 * scale, size.1 as f32 - 63.7 * scale);
        check_seams(&mut rig, at, Vec2::new(77.7, 41.3) * scale, 9.0 * scale);
    }
}

#[test]
fn clips_cut_nested_and_round_outwards() {
    let Some(mut rig) = Rig::new("clips_cut_nested_and_round_outwards", (48, 48)) else {
        return;
    };
    let rect = |min: f32, max: f32| ClipRect {
        min: Vec2::splat(min),
        max: Vec2::splat(max),
    };
    let mut list = UiDrawList::new();
    list.push_clip(rect(5.0, 25.0));
    list.push_clip(rect(14.4, 60.0));
    // Intersection is 14.4..25 and rounds to 14..25.
    list.add_filled_rect(Vec2::ZERO, Vec2::splat(48.0), [255, 0, 0, 255]);
    list.pop_clip();
    // Back in the outer clip, above the first rectangle.
    list.add_filled_rect(Vec2::new(5.0, 5.0), Vec2::new(9.0, 9.0), [0, 255, 0, 255]);
    list.pop_clip();
    // Outside every clip, wholly beyond the target: skipped.
    list.push_clip(rect(60.0, 70.0));
    list.add_filled_rect(Vec2::ZERO, Vec2::splat(80.0), [0, 0, 255, 255]);
    list.pop_clip();
    // A clip larger than the target is cut to it.
    list.push_clip(ClipRect {
        min: Vec2::new(40.0, 40.0),
        max: Vec2::splat(1.0e9),
    });
    list.add_filled_rect(
        Vec2::new(30.0, 30.0),
        Vec2::splat(100.0),
        [255, 255, 0, 255],
    );
    let img = rig.frame(&list);
    let at = |x, y| px(&img, 48, x, y);
    assert_eq!(at(14, 14), [255, 0, 0, 255]);
    assert_eq!(at(24, 24), [255, 0, 0, 255]);
    assert_eq!(at(13, 20), [0, 0, 0, 255]);
    assert_eq!(at(25, 20), [0, 0, 0, 255]);
    assert_eq!(at(6, 6), [0, 255, 0, 255]);
    assert_eq!(at(4, 6), [0, 0, 0, 255]);
    assert_eq!(at(47, 47), [255, 255, 0, 255]);
    assert_eq!(at(39, 45), [0, 0, 0, 255]);
    let s = rig.pass.stats();
    assert_eq!((s.cmds, s.cmds_drawn, s.cmds_skipped), (4, 3, 1));
    // Full target -> clip 1 -> clip 2 -> clip 3: the third command is skipped.
    assert_eq!(s.scissor_sets, 3);
}

#[test]
fn an_empty_list_has_no_work() {
    let Some(mut rig) = Rig::new("an_empty_list_has_no_work", (16, 16)) else {
        return;
    };
    let img = rig.frame(&UiDrawList::new());
    assert!(!rig.pass.has_work());
    assert_eq!(px(&img, 16, 3, 3), [0, 0, 0, 255]);
    assert_eq!(rig.pass.stats(), UiStats::default());
}

#[test]
fn a_mask_multiplies_the_alpha_and_the_sampler_is_the_commands() {
    let Some(mut rig) = Rig::new("a_mask_multiplies_the_alpha", (32, 16)) else {
        return;
    };
    // R8 mask, two texels: opaque left, clear right.
    rig.textures
        .upload(
            TextureId(9),
            &UiTextureData {
                width: 2,
                height: 1,
                format: TextureFormat::R8,
                mips: vec![vec![255, 0]],
                content: None,
            },
        )
        .unwrap();
    let mut list = UiDrawList::new();
    let material = UiMaterial {
        sampler: UiSampler::Point,
        shade: UiShade::Masked,
        texture2: TextureId(9),
        // The mask covers x 0..20, y 0..10 of the screen.
        tex2_xform: [1.0 / 20.0, 1.0 / 10.0, 0.0, 0.0],
    };
    list.push_blend(UiBlend::Slot2);
    list.add_triangles_with(
        TextureId::WHITE,
        &material,
        &[
            UiVertex {
                pos: [0.0, 0.0],
                uv: [0.0, 0.0],
                color: [200, 100, 50, 255],
            },
            UiVertex {
                pos: [20.0, 0.0],
                uv: [1.0, 0.0],
                color: [200, 100, 50, 255],
            },
            UiVertex {
                pos: [20.0, 10.0],
                uv: [1.0, 1.0],
                color: [200, 100, 50, 255],
            },
            UiVertex {
                pos: [0.0, 10.0],
                uv: [0.0, 1.0],
                color: [200, 100, 50, 255],
            },
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    list.pop_blend();
    let img = rig.render(&list, &UiExternal::default(), [0.0, 0.0, 0.0, 1.0]);
    assert!(
        near(px(&img, 32, 4, 5), [200, 100, 50, 255], 1),
        "{:?}",
        px(&img, 32, 4, 5)
    );
    assert_eq!(px(&img, 32, 15, 5), [0, 0, 0, 255]);
    assert_eq!(rig.pass.stats().cmds_drawn, 1);
    // A mask texture that is not loaded skips the command and asks for the id.
    let mut list = UiDrawList::new();
    list.add_triangles_with(
        TextureId::WHITE,
        &UiMaterial {
            shade: UiShade::Masked,
            texture2: TextureId(77),
            ..UiMaterial::PLAIN
        },
        &[
            UiVertex {
                pos: [0.0, 0.0],
                uv: [0.0, 0.0],
                color: [255; 4],
            },
            UiVertex {
                pos: [8.0, 0.0],
                uv: [0.0, 0.0],
                color: [255; 4],
            },
            UiVertex {
                pos: [8.0, 8.0],
                uv: [0.0, 0.0],
                color: [255; 4],
            },
        ],
        &[0, 1, 2],
    );
    rig.frame(&list);
    assert_eq!(rig.pass.stats().tex_missing, 1);
    assert_eq!(rig.textures.take_wanted(), vec![TextureId(77)]);
}

/// A 3x1 image stored padded to 4x4 (a compressed UI variant has whole 4x4 blocks): the quad's UV
/// 0..1 spans the three image texels, never the padding.
#[test]
fn a_padded_texture_is_sampled_over_its_image_only() {
    let Some(mut rig) = Rig::new("a_padded_texture_is_sampled_over_its_image_only", (12, 4)) else {
        return;
    };
    let (red, green, blue, pad) = (
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
    );
    let mut texels = Vec::new();
    for y in 0..4 {
        let row = if y == 0 {
            [red, green, blue, pad]
        } else {
            [pad; 4]
        };
        texels.extend(row);
    }
    let data = UiTextureData {
        width: 4,
        height: 4,
        format: TextureFormat::Rgba8,
        mips: vec![texels.iter().flatten().copied().collect()],
        content: Some((3, 1)),
    };
    assert_eq!(data.uv_scale(), [0.75, 0.25]);
    rig.textures.upload(TextureId(5), &data).unwrap();
    let mut list = UiDrawList::new();
    let v = |x: f32, y: f32, u: f32, w: f32| UiVertex {
        pos: [x, y],
        uv: [u, w],
        color: [255; 4],
    };
    list.add_triangles_with(
        TextureId(5),
        &UiMaterial {
            sampler: UiSampler::Point,
            ..UiMaterial::PLAIN
        },
        &[
            v(0.0, 0.0, 0.0, 0.0),
            v(12.0, 0.0, 1.0, 0.0),
            v(12.0, 4.0, 1.0, 1.0),
            v(0.0, 4.0, 0.0, 1.0),
        ],
        &[0, 1, 2, 0, 2, 3],
    );
    let img = rig.frame(&list);
    for y in 0..4 {
        for (x, want) in [(1, red), (5, green), (10, blue), (11, blue)] {
            assert_eq!(px(&img, 12, x, y), want, "pixel {x},{y}");
        }
    }
}

#[test]
fn point_and_linear_sampling_differ() {
    let Some(mut rig) = Rig::new("point_and_linear_sampling_differ", (16, 4)) else {
        return;
    };
    rig.rgba8(2, 2, 1, &[[255, 0, 0, 255], [0, 0, 255, 255]]);
    let draw = |rig: &mut Rig, sampler: UiSampler| {
        let mut list = UiDrawList::new();
        let v = |x: f32, u: f32| UiVertex {
            pos: [x, 0.0],
            uv: [u, 0.5],
            color: [255; 4],
        };
        let w = |x: f32, u: f32| UiVertex {
            pos: [x, 4.0],
            uv: [u, 0.5],
            color: [255; 4],
        };
        list.add_triangles_with(
            TextureId(2),
            &UiMaterial {
                sampler,
                ..UiMaterial::PLAIN
            },
            &[v(0.0, 0.0), v(8.0, 1.0), w(8.0, 1.0), w(0.0, 0.0)],
            &[0, 1, 2, 0, 2, 3],
        );
        rig.frame(&list)
    };
    let point = draw(&mut rig, UiSampler::Point);
    assert_eq!(px(&point, 16, 3, 1), [255, 0, 0, 255]);
    assert_eq!(px(&point, 16, 4, 1), [0, 0, 255, 255]);
    let linear = draw(&mut rig, UiSampler::Linear);
    let p = px(&linear, 16, 3, 1);
    assert!(p[0] > 100 && p[2] > 50 && p[2] < 200, "{p:?}");
}

#[test]
fn additive_blend_adds_the_scaled_source() {
    let Some(mut rig) = Rig::new("additive_blend_adds_the_scaled_source", (8, 8)) else {
        return;
    };
    let mut list = UiDrawList::new();
    list.push_blend(UiBlend::Slot1);
    list.add_filled_rect(Vec2::ZERO, Vec2::splat(8.0), [200, 0, 0, 128]);
    list.pop_blend();
    let img = rig.render(&list, &UiExternal::default(), [0.0, 0.2, 0.0, 1.0]);
    assert!(
        near(px(&img, 8, 4, 4), [100, 51, 0, 255], 2),
        "{:?}",
        px(&img, 8, 4, 4)
    );
}

fn quad_list(tex: TextureId, size: f32) -> UiDrawList {
    let mut list = UiDrawList::new();
    list.add_rect(
        tex,
        Vec2::ZERO,
        Vec2::splat(size),
        Vec2::ZERO,
        Vec2::ONE,
        [255; 4],
    );
    list
}

fn four_colours(rig: &Rig, colours: [[u8; 4]; 4]) -> wgpu::Texture {
    use wgpu::util::DeviceExt;
    rig.gpu.device.create_texture_with_data(
        &rig.gpu.queue,
        &wgpu::TextureDescriptor {
            label: Some("external"),
            size: wgpu::Extent3d {
                width: 2,
                height: 2,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &colours.concat(),
    )
}

#[test]
fn the_minimap_handle_is_resolved_per_frame_and_follows_the_generation() {
    let Some(mut rig) = Rig::new("the_minimap_handle_is_resolved_per_frame", (16, 16)) else {
        return;
    };
    let red = four_colours(&rig, [[255, 0, 0, 255]; 4]);
    let green = four_colours(&rig, [[0, 255, 0, 255]; 4]);
    let (red_view, green_view) = (
        red.create_view(&wgpu::TextureViewDescriptor::default()),
        green.create_view(&wgpu::TextureViewDescriptor::default()),
    );
    let list = quad_list(TextureId::MINIMAP, 8.0);
    let info = |generation| MinimapInfo {
        size: (2, 2),
        generation,
    };
    let clear = [0.0, 0.0, 0.0, 1.0];

    let img = rig.render(
        &list,
        &UiExternal {
            minimap: Some((&red_view, info(1))),
        },
        clear,
    );
    assert_eq!(px(&img, 16, 4, 4), [255, 0, 0, 255]);
    assert_eq!(rig.pass.stats().handle_missing, 0);
    // The same generation again: the cached group is used.
    let img = rig.render(
        &list,
        &UiExternal {
            minimap: Some((&red_view, info(1))),
        },
        clear,
    );
    assert_eq!(px(&img, 16, 4, 4), [255, 0, 0, 255]);
    // The texture was made again: a new generation shows the new view.
    let img = rig.render(
        &list,
        &UiExternal {
            minimap: Some((&green_view, info(2))),
        },
        clear,
    );
    assert_eq!(px(&img, 16, 4, 4), [0, 255, 0, 255]);
    // Lost minimap: the command is skipped, nothing panics, the frame stays as it was.
    let img = rig.render(&list, &UiExternal::default(), clear);
    assert_eq!(px(&img, 16, 4, 4), [0, 0, 0, 255]);
    let s = rig.pass.stats();
    assert_eq!((s.handle_missing, s.cmds_skipped, s.cmds_drawn), (1, 1, 0));
    assert!(!rig.pass.has_work());
    // An unknown external id is a missing handle too.
    let other = quad_list(TextureId::external(2), 8.0);
    rig.render(
        &other,
        &UiExternal {
            minimap: Some((&red_view, info(1))),
        },
        clear,
    );
    assert_eq!(rig.pass.stats().handle_missing, 1);
}

#[test]
fn textures_load_lazily_and_a_missing_one_is_skipped_once_asked_for() {
    let Some(mut rig) = Rig::new("textures_load_lazily", (16, 16)) else {
        return;
    };
    let list = quad_list(TextureId(5), 8.0);
    let img = rig.frame(&list);
    assert_eq!(
        px(&img, 16, 4, 4),
        [0, 0, 0, 255],
        "no texture, no stand-in"
    );
    assert_eq!(rig.pass.stats().tex_missing, 1);
    assert_eq!(rig.pass.stats().cmds_skipped, 1);
    assert_eq!(rig.textures.take_wanted(), vec![TextureId(5)]);
    // Still missing: not asked for again.
    rig.frame(&list);
    assert_eq!(rig.pass.stats().tex_missing, 1);
    assert!(rig.textures.take_wanted().is_empty());
    // The client uploads it.
    rig.rgba8(5, 1, 1, &[[10, 200, 30, 255]]);
    let img = rig.frame(&list);
    assert_eq!(px(&img, 16, 4, 4), [10, 200, 30, 255]);
    let s = rig.pass.stats();
    assert_eq!((s.tex_missing, s.tex_uploads, s.tex_bound), (0, 1, 1));
    assert!(s.upload_bytes >= 4);
    // Evicted: asked for again.
    rig.textures.evict(TextureId(5));
    rig.frame(&list);
    assert_eq!(rig.pass.stats().tex_evictions, 1);
    assert_eq!(rig.textures.take_wanted(), vec![TextureId(5)]);
}

#[test]
fn only_asset_ids_can_be_uploaded() {
    let Some(mut rig) = Rig::new("only_asset_ids_can_be_uploaded", (4, 4)) else {
        return;
    };
    let ok = UiTextureData {
        width: 1,
        height: 1,
        format: TextureFormat::Rgba8,
        mips: vec![vec![0; 4]],
        content: None,
    };
    assert!(matches!(
        rig.textures.upload(TextureId::WHITE, &ok),
        Err(UiTextureError::NotAnAsset(0))
    ));
    assert!(matches!(
        rig.textures.upload(TextureId::MINIMAP, &ok),
        Err(UiTextureError::NotAnAsset(_))
    ));
    let empty = UiTextureData {
        mips: vec![],
        ..ok.clone()
    };
    assert!(matches!(
        rig.textures.upload(TextureId(1), &empty),
        Err(UiTextureError::Empty)
    ));
    let zero = UiTextureData { width: 0, ..ok };
    assert!(matches!(
        rig.textures.upload(TextureId(1), &zero),
        Err(UiTextureError::Empty)
    ));
    assert!(!rig.textures.contains(TextureId(1)));
    assert!(rig.textures.contains(TextureId::WHITE));
}

#[test]
fn a_bc1_texture_is_drawn_whatever_the_device_supports() {
    let Some(mut rig) = Rig::new("a_bc1_texture_is_drawn", (8, 8)) else {
        return;
    };
    // One block: colour0 = pure red (RGB565 0xF800), every index 0.
    let block = vec![0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
    rig.textures
        .upload(
            TextureId(3),
            &UiTextureData {
                width: 4,
                height: 4,
                format: TextureFormat::Bc1,
                mips: vec![block],
                content: None,
            },
        )
        .unwrap();
    let img = rig.frame(&quad_list(TextureId(3), 8.0));
    assert!(
        near(px(&img, 8, 4, 4), [255, 0, 0, 255], 2),
        "{:?}",
        px(&img, 8, 4, 4)
    );
}

#[test]
fn the_budget_evicts_the_least_recently_used_after_the_protected_frames() {
    let Some(mut rig) = Rig::new("the_budget_evicts_the_least_recently_used", (8, 8)) else {
        return;
    };
    for id in 1..=5 {
        rig.rgba8(id, 16, 16, &[[1, 2, 3, 255]; 256]);
    }
    assert_eq!(rig.textures.resident_bytes(), 5 * 1024);
    rig.textures.set_budget(3 * 1024);
    // All five were just uploaded: protected, nothing goes yet.
    assert_eq!(rig.textures.resident_bytes(), 5 * 1024);
    let keep = quad_list(TextureId(1), 4.0);
    let mut evictions = 0;
    for _ in 0..6 {
        rig.frame(&keep);
        evictions += rig.pass.stats().tex_evictions;
    }
    // 2 and 3 are the oldest of the unused (ties go to the lower id); 1 is used every frame.
    assert_eq!(evictions, 2);
    assert!(rig.textures.contains(TextureId(1)));
    assert!(!rig.textures.contains(TextureId(2)));
    assert!(!rig.textures.contains(TextureId(3)));
    assert!(rig.textures.contains(TextureId(4)) && rig.textures.contains(TextureId(5)));
    assert_eq!(rig.textures.resident_bytes(), 3 * 1024);
    assert_eq!(rig.pass.stats().tex_resident_mb, 0);
}

#[test]
fn an_unchanged_list_is_not_uploaded_again() {
    let Some(mut rig) = Rig::new("an_unchanged_list_is_not_uploaded_again", (16, 16)) else {
        return;
    };
    let mut list = quad_list(TextureId::WHITE, 8.0);
    rig.frame(&list);
    let first = rig.pass.stats().upload_bytes;
    assert_eq!(first, 4 * 20 + 6 * 4);
    let img = rig.frame(&list);
    assert_eq!(rig.pass.stats().upload_bytes, 0);
    assert_eq!(px(&img, 16, 4, 4), [255; 4], "commands are still drawn");
    list.add_filled_rect(
        Vec2::new(10.0, 10.0),
        Vec2::new(12.0, 12.0),
        [255, 0, 0, 255],
    );
    let img = rig.frame(&list);
    assert!(rig.pass.stats().upload_bytes > 0);
    assert_eq!(px(&img, 16, 11, 11), [255, 0, 0, 255]);
    // A different list with the same size is not mistaken for it.
    let other = quad_list(TextureId::WHITE, 4.0);
    let img = rig.frame(&other);
    assert!(rig.pass.stats().upload_bytes > 0);
    assert_eq!(px(&img, 16, 6, 6), [0, 0, 0, 255]);
}

#[test]
fn buffers_grow_past_their_first_size() {
    let Some(mut rig) = Rig::new("buffers_grow_past_their_first_size", (64, 64)) else {
        return;
    };
    let mut list = UiDrawList::new();
    for i in 0..4000 {
        let x = (i % 60) as f32;
        let y = ((i / 60) % 60) as f32;
        list.add_filled_rect(
            Vec2::new(x, y),
            Vec2::new(x + 1.0, y + 1.0),
            [0, 255, 0, 255],
        );
    }
    assert!(list.vertices().len() * 20 > MIN_BUFFER as usize);
    let img = rig.frame(&list);
    assert_eq!(px(&img, 64, 59, 59), [0, 255, 0, 255]);
    assert_eq!(rig.pass.stats().quads, 4000);
}

/// The CPU cost of `prepare` on a heavy scene: 4000 quads in 600 commands (AC-F5: 2 ms). Prints
/// the numbers; the bound is checked in release builds only.
#[test]
fn prepare_stays_inside_its_cpu_budget() {
    let Some(mut rig) = Rig::new("prepare_stays_inside_its_cpu_budget", (1280, 720)) else {
        return;
    };
    for id in 1..=12u64 {
        rig.rgba8(id, 4, 4, &[[200, 200, 200, 255]; 16]);
    }
    let build = |shift: f32| {
        let mut list = UiDrawList::new();
        for i in 0..600u64 {
            // Alternating textures and clips keep the commands apart.
            let tex = TextureId(1 + i % 12);
            let x = (i % 40) as f32 * 30.0 + shift;
            let y = (i / 40) as f32 * 40.0;
            list.push_clip(ClipRect {
                min: Vec2::new(x - 1.0, y - 1.0),
                max: Vec2::new(x + 29.0, y + 39.0),
            });
            for q in 0..7 {
                let o = q as f32 * 3.0;
                list.add_rect(
                    tex,
                    Vec2::new(x + o, y),
                    Vec2::new(x + o + 10.0, y + 20.0),
                    Vec2::ZERO,
                    Vec2::ONE,
                    [255; 4],
                );
            }
            list.pop_clip();
        }
        list
    };
    let (a, b) = (build(0.0), build(0.5));
    assert_eq!(a.commands().len(), 600);
    let target = UiTarget {
        format: FORMAT,
        size: rig.size,
    };
    let mut time = |list: &UiDrawList, runs: u32| {
        let t = std::time::Instant::now();
        for _ in 0..runs {
            rig.pass.prepare(
                &rig.gpu.device,
                &rig.gpu.queue,
                list,
                &mut rig.textures,
                &UiExternal::default(),
                &target,
            );
        }
        t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs)
    };
    time(&a, 3);
    let unchanged = time(&a, 50);
    // Alternating lists: every prepare uploads.
    let mut changing = 0.0;
    for _ in 0..25 {
        changing += time(&a, 1) + time(&b, 1);
    }
    changing /= 50.0;
    let s = rig.pass.stats();
    println!(
        "ui prepare: {unchanged:.3} ms unchanged, {changing:.3} ms changed ({} cmds, {} quads, {} draws)",
        s.cmds, s.quads, s.cmds_drawn
    );
    assert!(s.cmds >= 600 && s.quads >= 4200 && s.cmds_drawn == s.cmds);
    if !cfg!(debug_assertions) {
        assert!(changing < 2.0, "prepare took {changing:.3} ms");
    }
}

fn counter(stats: &crate::FrameStats, name: &str) -> Option<u64> {
    stats
        .counters
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| *v)
}

fn headless(test: &str) -> Option<crate::Renderer> {
    use crate::gpu_test::block_on;
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

#[test]
fn the_renderer_draws_the_list_over_the_world_and_reports_the_counters() {
    let Some(mut r) = headless("the_renderer_draws_the_list_over_the_world") else {
        return;
    };
    // Empty list: no pass, zero counters, but the names are there.
    let img = frame_with(&mut r, &UiDrawList::new());
    let stats = r.last_frame_stats().clone();
    let game = stats
        .passes
        .iter()
        .find(|p| p.pass == crate::frame::PassId::UiGame)
        .unwrap();
    assert_eq!((game.draws, game.triangles), (0, 0));
    for (name, _) in UiStats::default().counters() {
        assert_eq!(counter(&stats, name), Some(0), "{name}");
    }
    let under = img.pixel(10, 10);
    // An opaque rectangle replaces the world under it.
    let mut ui = UiDrawList::new();
    ui.add_filled_rect(
        Vec2::new(5.0, 5.0),
        Vec2::new(30.0, 30.0),
        [255, 0, 255, 255],
    );
    let img = frame_with(&mut r, &ui);
    let stats = r.last_frame_stats().clone();
    assert_eq!(&img.pixel(10, 10)[..3], &[255, 0, 255]);
    assert_ne!(&under[..3], &[255, 0, 255]);
    let game = stats
        .passes
        .iter()
        .find(|p| p.pass == crate::frame::PassId::UiGame)
        .unwrap();
    assert_eq!((game.draws, game.triangles), (1, 2));
    assert_eq!(counter(&stats, "ui.cmds_drawn"), Some(1));
    assert_eq!(counter(&stats, "ui.quads"), Some(1));
    assert_eq!(stats.ui.verts, 4);
    assert_eq!(stats.ui_times.cpu_ms, game.cpu_ms);
    // A texture the list names and the client has not loaded yet.
    let mut ui = UiDrawList::new();
    ui.add_rect(
        TextureId(42),
        Vec2::ZERO,
        Vec2::splat(8.0),
        Vec2::ZERO,
        Vec2::ONE,
        [255; 4],
    );
    frame_with(&mut r, &ui);
    assert_eq!(counter(r.last_frame_stats(), "ui.tex_missing"), Some(1));
    assert_eq!(r.ui_textures_mut().take_wanted(), vec![TextureId(42)]);
}

#[test]
fn the_minimap_handle_follows_the_layer_switch_in_the_renderer() {
    let Some(mut r) = headless("the_minimap_handle_follows_the_layer_switch") else {
        return;
    };
    let mut ui = UiDrawList::new();
    ui.add_rect(
        TextureId::MINIMAP,
        Vec2::ZERO,
        Vec2::splat(8.0),
        Vec2::ZERO,
        Vec2::ONE,
        [255; 4],
    );
    frame_with(&mut r, &ui);
    frame_with(&mut r, &ui);
    let present = r.minimap_view().is_some();
    assert_eq!(
        counter(r.last_frame_stats(), "ui.handle_missing"),
        Some(u64::from(!present))
    );
    let (off, _) = crate::LayerSwitches::from_names("Minimap");
    r.set_layers(off);
    frame_with(&mut r, &ui);
    assert_eq!(counter(r.last_frame_stats(), "ui.handle_missing"), Some(1));
}
