//! The `Present` pass: copies `Scene` onto the surface. The world
//! is in gamma space, so a surface without sRGB gets the values as they are; a surface that only
//! offers an sRGB format gets `srgb_to_linear(x)`, which its hardware encodes back to the same
//! bytes. With a render scale below one the scene is smaller than the surface and is stretched over
//! it with the linear filter.

use crate::perf::PassCount;

/// The pipeline of the pass, made for one surface format.
pub struct Present {
    layout: wgpu::BindGroupLayout,
    /// Texel by texel: the scene has the size of the target.
    pipeline: wgpu::RenderPipeline,
    /// With the linear filter: the scene is smaller than the target (a render scale below one).
    scaled: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl Present {
    /// `surface_format`: the format of the view the pass writes to.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("../present.wgsl"));
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("present layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("present pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let srgb = surface_format.is_srgb();
        let make = |label: &'static str, fragment: &'static str| {
            Self::pipeline(
                device,
                &shader,
                &pipeline_layout,
                surface_format,
                label,
                fragment,
            )
        };
        let pipeline = make("present", if srgb { "fs_linearize" } else { "fs_copy" });
        let scaled = make(
            "present scaled",
            if srgb {
                "fs_scaled_linearize"
            } else {
                "fs_scaled"
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("present sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            layout,
            pipeline,
            scaled,
            sampler,
        }
    }

    fn pipeline(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        pipeline_layout: &wgpu::PipelineLayout,
        surface_format: wgpu::TextureFormat,
        label: &'static str,
        fragment: &'static str,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fragment),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Draws `scene` over all of `target`. `stretch`: the scene is smaller than the target (a render
    /// scale below one) and is drawn with the linear filter; else both have the same size.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView,
        target: &wgpu::TextureView,
        stretch: bool,
        timestamps: Option<wgpu::RenderPassTimestampWrites<'_>>,
    ) {
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("present"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline_c(if stretch {
            &self.scaled
        } else {
            &self.pipeline
        });
        pass.set_bind_group_c(0, &bind_group, &[]);
        pass.draw_c(0..3, 0..1);
    }
}
