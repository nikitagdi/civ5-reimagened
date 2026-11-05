//! Multisampled targets: the captured targets have 4 samples and `before` and
//! `after` are their sample 0. The replay draws into 4 samples too, so that coverage, the depth
//! test and the blending of sample 0 are those of the capture, also on the edges of triangles and
//! where fragments overlap; sample 0 is read out by a full-screen pass.

use super::ReplayGpu;

const COPY_WGSL: &str = r"
@group(0) @binding(0) var src: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_copy(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(p.xy), 0);
}
";

const RESOLVE_WGSL: &str = r"
@group(0) @binding(0) var ms: texture_multisampled_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

// Sample 0 only: the capture's before and after are the sample 0 of its multisampled targets.
@fragment
fn fs_sample0(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(ms, vec2<i32>(p.xy), 0);
}
";

/// One full-screen pass from `src` into `dst`: `resolve0` reads sample 0 of a multisampled `src`
/// into the single-sample `dst`; otherwise a single-sample `src` is written to every sample of
/// `dst` (`dst_samples` a pixel).
fn full_screen_pass(
    gpu: &ReplayGpu,
    src: &wgpu::TextureView,
    dst: &wgpu::TextureView,
    format: wgpu::TextureFormat,
    dst_samples: u32,
    resolve0: bool,
) {
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("replay copy"),
            source: wgpu::ShaderSource::Wgsl(
                if resolve0 { RESOLVE_WGSL } else { COPY_WGSL }.into(),
            ),
        });
    let bgl = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("replay copy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: resolve0,
                },
                count: None,
            }],
        });
    let layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("replay copy"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("replay copy"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: dst_samples,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if resolve0 { "fs_sample0" } else { "fs_copy" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
    let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("replay copy"),
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(src),
        }],
    });
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("replay copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
    gpu.queue.submit([enc.finish()]);
}

/// Copies the single-sample `src` into every sample of the multisampled `dst`.
pub fn copy_to_all_samples(
    gpu: &ReplayGpu,
    src: &wgpu::Texture,
    dst: &wgpu::Texture,
    format: wgpu::TextureFormat,
    samples: u32,
) {
    let sv = src.create_view(&wgpu::TextureViewDescriptor::default());
    let dv = dst.create_view(&wgpu::TextureViewDescriptor::default());
    full_screen_pass(gpu, &sv, &dv, format, samples, false);
}

/// Sample 0 of the multisampled `src` as a new single-sample texture (`COPY_SRC`).
pub fn resolve_sample0(
    gpu: &ReplayGpu,
    src: &wgpu::Texture,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
) -> wgpu::Texture {
    let dst = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("replay sample 0"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let sv = src.create_view(&wgpu::TextureViewDescriptor::default());
    let dv = dst.create_view(&wgpu::TextureViewDescriptor::default());
    full_screen_pass(gpu, &sv, &dv, format, 1, true);
    dst
}
