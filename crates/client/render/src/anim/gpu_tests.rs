//! The bone texture on the GPU (native, the default backend and GL, the nearest to WebGL2): the
//! shaders of `shaders/skin/` read the baked frames the way the cache wrote them, and the blend
//! of two frames in the shader is the blend of the matrices on the CPU.

use std::sync::Arc;

use super::clip_cache::{BoneSlots, ClipCache, ClipKey, MemoryBones};
use super::data::{ModelAnim, fixtures};
use crate::gpu_test::{Gpu, gpu, read_texture};
use crate::shaders::create_module;

const WIDTH: u32 = 1536;

/// The slots `0..slots` of the store as the bone texture: `Rgba32Float`, three texels a bone.
fn bone_texture(gpu: &Gpu, store: &MemoryBones, slots: u32) -> wgpu::Texture {
    let rows = slots.div_ceil(512).max(1);
    let mut bytes = vec![0u8; (WIDTH * rows * 16) as usize];
    for s in 0..slots {
        let bone = store.bone(s);
        let (row, col) = (s / 512, (s % 512) * 3);
        for j in 0..3 {
            let at = ((row * WIDTH + col + j) * 16) as usize;
            for c in 0..4 {
                let v = bone[(j * 4 + c) as usize];
                bytes[at + c as usize * 4..at + c as usize * 4 + 4]
                    .copy_from_slice(&v.to_le_bytes());
            }
        }
    }
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bones"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: rows,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 16),
            rows_per_image: Some(rows),
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: rows,
            depth_or_array_layers: 1,
        },
    );
    tex
}

/// Runs `body` (the statements of a fragment shader that return a `vec4`) with the module of
/// `frame.wgsl`, `skin.wgsl`, `skin/common.wgsl`, over one pixel; the four floats written.
fn probe(gpu: &Gpu, bones: &wgpu::Texture, body: &str) -> [f32; 4] {
    let source = format!(
        "{}\n{}\n{}\n\
         @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{\n\
             let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));\n\
             return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);\n\
         }}\n\
         @fragment fn fs() -> @location(0) vec4<f32> {{\n{body}\n}}\n",
        include_str!("../shaders/common/frame.wgsl"),
        include_str!("../shaders/common/skin.wgsl"),
        include_str!("../shaders/skin/common.wgsl"),
    );
    let shader = create_module(&gpu.device, "skin probe", &source);
    let empty = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("empty"),
            entries: &[],
        });
    let layout = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bones"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 14,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
    let view = bones.create_view(&wgpu::TextureViewDescriptor::default());
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bones"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 14,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    let pl = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("skin probe"),
            bind_group_layouts: &[Some(&empty), Some(&layout)],
            immediate_size: 0,
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("skin probe"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba32Float,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("skin probe target"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let tv = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("skin probe"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tv,
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
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(1, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    gpu.queue.submit([encoder.finish()]);
    let bytes = read_texture(gpu, &target, 0, 0, (1, 1), 16);
    let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
    [f(0), f(1), f(2), f(3)]
}

/// The CPU answer: the point through the blend of the matrices of bone `k` in the slots `a` and
/// `b`.
fn cpu_blend(store: &MemoryBones, a: u32, b: u32, blend: f32, p: [f32; 3]) -> [f32; 3] {
    let (ma, mb) = (store.bone(a), store.bone(b));
    let row = |j: usize| -> f32 {
        let m: Vec<f32> = (0..4)
            .map(|c| ma[j * 4 + c] * (1.0 - blend) + mb[j * 4 + c] * blend)
            .collect();
        m[0] * p[0] + m[1] * p[1] + m[2] * p[2] + m[3]
    };
    [row(0), row(1), row(2)]
}

fn backends() -> [(&'static str, wgpu::Backends); 2] {
    [
        ("default", wgpu::Backends::PRIMARY),
        ("gl", wgpu::Backends::GL),
    ]
}

/// The blend of two frames in the shader (`bone_matrix_blend`, `bind_matrix`) is the blend of the
/// matrices on the CPU, for bones of a baked clip and at the edges of the row of the texture.
#[test]
fn the_shader_blends_two_baked_frames_as_the_cpu_does() {
    // 3 bones, a clip of 61 frames: 183 slots; pushed up the store so that frames sit across the
    // 512-slot row.
    let (m, pack) = fixtures::model("warrior", 3);
    let model = Arc::new(ModelAnim::new(&m, Some(pack), None).unwrap());
    let mut cache = ClipCache::new(MemoryBones::new(4000));
    cache.store_mut().alloc(400).unwrap();
    let key = ClipKey { model: 0, clip: 0 };
    cache.request(key, &model);
    cache.bake_pending();
    let clip = cache.get(key).unwrap().clone();
    assert_eq!(clip.range.start, 400);
    let slots = clip.range.end;

    for (name, backend) in backends() {
        let Some(gpu) = gpu(
            "the_shader_blends_two_baked_frames_as_the_cpu_does",
            backend,
            wgpu::Features::empty(),
        ) else {
            continue;
        };
        let tex = bone_texture(&gpu, cache.store(), slots);
        let p = [1.0, 2.0, 3.0];
        // The frames 10 and 11 (blend 0.25), and 33 and 34 (blend 0.8, across the row: slot 512 is
        // frame 38), bone 1 and 2.
        for (frame, blend, bone) in [
            (10u32, 0.25f32, 1u32),
            (33, 0.8, 2),
            (37, 0.5, 1),
            (0, 0.0, 2),
        ] {
            let base = clip.range.start + frame * 3;
            let next = base + 3;
            let body = format!(
                "let m = bone_matrix_blend({base}u, {next}u, {blend:?}, {bone}u);\n\
                 return vec4<f32>(skin_position(m, vec3<f32>({:?}, {:?}, {:?})), 1.0);",
                p[0], p[1], p[2]
            );
            let got = probe(&gpu, &tex, &body);
            let want = cpu_blend(cache.store(), base + bone, next + bone, blend, p);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 1e-4,
                    "{name}: frame {frame} bone {bone}: {got:?} {want:?}"
                );
            }
            // `bind_matrix` is the same read for a rigid mesh on bone `bone`.
            let body = format!(
                "let m = bind_matrix(BoneRef({base}u, {next}u, {blend:?}), {bone}u);\n\
                 return vec4<f32>(skin_position(m, vec3<f32>({:?}, {:?}, {:?})), 1.0);",
                p[0], p[1], p[2]
            );
            let got = probe(&gpu, &tex, &body);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 1e-4,
                    "{name}: bind_matrix frame {frame} bone {bone}: {got:?} {want:?}"
                );
            }
        }
        // A palette texel as the original's `c8[a0.x]` reads it: bone 1 is texel 3 of the frame.
        let base = clip.range.start + 20 * 3;
        let got = probe(
            &gpu,
            &tex,
            &format!("return palette_row({base}u, {base}u, 0.0, 4u);"),
        );
        let want = cache.store().bone(base + 1);
        for c in 0..4 {
            assert!((got[c] - want[4 + c]).abs() < 1e-6, "{name}: texel row 1");
        }
    }
}

/// Packet F: `bone_pose` in the shader finds, from a clocked reference (header, clock, start),
/// the frames and the blend the CPU walk gives at that time.
#[test]
fn the_shader_runs_the_clock_of_a_clocked_reference_as_the_cpu_does() {
    use super::{AnimServiceOf, AnimTarget};
    use asset_format::LogicalId;
    let mut s = AnimServiceOf::new(MemoryBones::new(20_000));
    let (m, pack) = fixtures::model("warrior", 3);
    s.register_model(
        LogicalId::new("warrior"),
        ModelAnim::new(&m, Some(pack), None).unwrap(),
    );
    assert!(s.set_gpu_clock(true));
    let hs: Vec<_> = (0..6u8)
        .map(|i| {
            s.attach(
                &LogicalId::new("warrior"),
                AnimTarget::Figure {
                    unit: (0, 3),
                    index: i,
                },
            )
            .unwrap()
        })
        .collect();
    for (name, backend) in backends() {
        let Some(gpu) = gpu(
            "the_shader_runs_the_clock_of_a_clocked_reference_as_the_cpu_does",
            backend,
            wgpu::Features::empty(),
        ) else {
            continue;
        };
        for t in [0.37f32, 1.91, 7.4, 123.45] {
            s.step(t);
            let slots = s
                .cache()
                .store()
                .free_runs()
                .first()
                .map_or(20_000, |r| r.start);
            let tex = bone_texture(&gpu, s.cache().store(), slots);
            for &h in &hs {
                let r = s.bone_ref(h);
                assert_ne!(r.next & super::gpu_clock::CLOCK_FLAG, 0, "clocked");
                let want = s.resolved_ref(h, t);
                let body = format!(
                    "let p = bone_pose(vec2<u32>({}u, {}u), {:?});\n\
                     return vec4<f32>(f32(p.base), f32(p.next), p.blend, 0.0);",
                    r.base, r.next, r.blend
                );
                let got = probe(&gpu, &tex, &body);
                assert_eq!(
                    (got[0] as u32, got[1] as u32),
                    (want.base, want.next),
                    "{name} t {t}: {got:?} {want:?}"
                );
                assert!(
                    (got[2] - want.blend).abs() < 1e-3,
                    "{name} t {t}: {got:?} {want:?}"
                );
            }
        }
    }
}

/// Packet F: the three ways the time of a clocked play runs (a loop, once, a landmark's cycle of
/// repeats and a pause) give in the shader the frames `GpuClock::decode` (the CPU's functions) does.
#[test]
fn the_shader_runs_loops_plays_once_and_landmark_cycles_as_the_cpu_does() {
    use super::gpu_clock::{CLOCK_BLOCK, GpuClock, PlayKind};
    use super::landmark_tree::Cycle;
    let (m, pack) = fixtures::model("warrior", 3);
    let model = Arc::new(ModelAnim::new(&m, Some(pack), None).unwrap());
    let mut cache = ClipCache::new(MemoryBones::new(20_000));
    cache.reserve_identity();
    let block = cache.reserve_block(CLOCK_BLOCK).unwrap();
    let mut clock = GpuClock::new(block);
    let key = ClipKey { model: 0, clip: 0 };
    cache.request(key, &model);
    cache.bake_pending();
    let clip = cache.get(key).unwrap().clone();
    let d = clip.duration;
    let kinds = [
        PlayKind::new(key, true, d, None),
        PlayKind::new(key, false, d, None),
        PlayKind::new(
            key,
            true,
            d,
            Some(Cycle {
                repeat: 3,
                pause: 0.7,
            }),
        ),
        PlayKind::new(
            key,
            true,
            d,
            Some(Cycle {
                repeat: 1,
                pause: 1.3,
            }),
        ),
    ];
    let headers: Vec<u32> = kinds
        .iter()
        .map(|k| {
            let store = cache.store_mut();
            clock
                .header_for(*k, &clip, |r, b| store.write(r, b))
                .unwrap()
        })
        .collect();
    let slots = clip.range.end;
    for (name, backend) in backends() {
        let Some(gpu) = gpu(
            "the_shader_runs_loops_plays_once_and_landmark_cycles_as_the_cpu_does",
            backend,
            wgpu::Features::empty(),
        ) else {
            continue;
        };
        for now in [0.05f32, 0.9, 2.3, 3.0 * d + 0.35, 3.0 * d + 0.9, 17.77] {
            let slot = clock.clock_slot();
            cache
                .store_mut()
                .write(slot..slot + 1, &[GpuClock::clock_bone(now)]);
            let tex = bone_texture(&gpu, cache.store(), slots);
            for (i, &h) in headers.iter().enumerate() {
                let r = clock.encode(h, 0.013);
                let want = clock.decode(r, now);
                let body = format!(
                    "let p = bone_pose(vec2<u32>({}u, {}u), {:?});\n\
                     return vec4<f32>(f32(p.base), f32(p.next), p.blend, 0.0);",
                    r.base, r.next, r.blend
                );
                let got = probe(&gpu, &tex, &body);
                assert_eq!(
                    (got[0] as u32, got[1] as u32),
                    (want.base, want.next),
                    "{name} kind {i} now {now}: {got:?} {want:?}"
                );
                assert!(
                    (got[2] - want.blend).abs() < 1e-3,
                    "{name} kind {i} now {now}: {got:?} {want:?}"
                );
            }
        }
    }
}
