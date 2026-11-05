//! The special cases of `sm3.wgsl` on the GPU (native, the default backend and GL, the nearest to
//! WebGL2), and the `override` variants of a module.

use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::*;
use crate::frame::ViewGlobals;
use crate::gpu_test::{Gpu, gpu, read_texture};

/// Runs the fragment entry `entry` of the module `common_probe` over one pixel of an
/// `Rgba32Float` target, with `constants` for its `override`s; the four floats written.
fn run(gpu: &Gpu, entry: &str, constants: &[(&str, f64)]) -> [f32; 4] {
    let module = catalog()
        .iter()
        .find(|m| m.name == "common_probe")
        .expect("common_probe");
    let shader = create_module(&gpu.device, "probe", &module.source());
    let layout = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("probe"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
    let globals = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("probe globals"),
            contents: bytemuck::bytes_of(&ViewGlobals::new(Mat4::IDENTITY, Vec3::ZERO)),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe"),
        layout: &layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: globals.as_entire_binding(),
        }],
    });
    let pipeline_layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("probe"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("probe"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fullscreen"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
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
        label: Some("probe target"),
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
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("probe"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
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
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    gpu.queue.submit([encoder.finish()]);
    let bytes = read_texture(gpu, &target, 0, 0, (1, 1), 16);
    let f = |i: usize| f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
    [f(0), f(1), f(2), f(3)]
}

fn backends() -> [(&'static str, wgpu::Backends); 2] {
    [
        ("default", wgpu::Backends::PRIMARY),
        ("gl", wgpu::Backends::GL),
    ]
}

/// The values of the table of shader-re 4.6 for the special cases (D3D9's answers).
#[test]
fn sm3_special_cases_give_the_d3d9_values() {
    for (name, backend) in backends() {
        let Some(gpu) = gpu(
            "sm3_special_cases_give_the_d3d9_values",
            backend,
            wgpu::Features::empty(),
        ) else {
            continue;
        };
        // pow(0, 0), rcp(0), rsq(0), log(0)
        let [pow00, rcp0, rsq0, log0] = run(&gpu, "fs_sm3_a", &[]);
        assert_eq!(pow00, 1.0, "{name}: pow(0, 0) = 1 (0 * anything is 0)");
        assert_eq!(rcp0, f32::INFINITY, "{name}: rcp(0)");
        assert_eq!(rsq0, f32::INFINITY, "{name}: rsq(0)");
        assert_eq!(log0, f32::NEG_INFINITY, "{name}: log(0)");
        // nrm(0) is the zero vector; pow(-2, 2) takes |x|; an ordinary rcp.
        let [nx, ny, pow_neg, rcp4] = run(&gpu, "fs_sm3_b", &[]);
        assert!(nx == 0.0 && ny == 0.0, "{name}: nrm(0) = 0, got {nx} {ny}");
        assert!(
            (pow_neg - 4.0).abs() < 1e-4,
            "{name}: pow(-2, 2) = {pow_neg}"
        );
        assert!((rcp4 - 0.25).abs() < 1e-6, "{name}: rcp(4) = {rcp4}");
        // cmp: negative and NaN take the third operand.
        assert_eq!(
            run(&gpu, "fs_sm3_c", &[]),
            [20.0, 10.0, 10.0, 20.0],
            "{name}: cmp"
        );
        // The gamma helpers go there and back.
        let [t, m, t2, _] = run(&gpu, "fs_gamma", &[]);
        assert!(
            (t - 0.25).abs() < 2e-3 && (m - 0.5).abs() < 2e-3 && (t2 - 0.75).abs() < 2e-3,
            "{name}: gamma round trip {t} {m} {t2}"
        );
    }
}

/// Two values of the `override` constants make two different pipelines of one text.
#[test]
fn override_variants_change_the_pipeline_not_the_text() {
    let module = catalog().iter().find(|m| m.name == "common_probe").unwrap();
    let source = module.source();
    let off = Variant::new();
    let on = Variant::new()
        .with("PROBE_SCALE", 2.5)
        .with_flag("PROBE_ON", true);
    assert_eq!(on.check(&source), Ok(()));
    assert_ne!(on.key(), off.key());
    for (name, backend) in backends() {
        let Some(gpu) = gpu(
            "override_variants_change_the_pipeline_not_the_text",
            backend,
            wgpu::Features::empty(),
        ) else {
            continue;
        };
        assert_eq!(
            run(&gpu, "fs_override", &off.constants()),
            [1.0, 0.0, 0.0, 1.0],
            "{name}"
        );
        assert_eq!(
            run(&gpu, "fs_override", &on.constants()),
            [2.5, 1.0, 0.0, 1.0],
            "{name}"
        );
    }
}
