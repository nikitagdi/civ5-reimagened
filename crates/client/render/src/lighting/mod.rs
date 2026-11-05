//! Light of the world: [`LightingEnv`], the cubes, colour keys and
//! fog ramp of the `boot` group; [`FrameGlobals`], the sun, shadow, reflection and fog constants
//! of a frame.
//!
//! Until the boot group has arrived `LightingEnv::ready()` is false and a renderer that waits for
//! it (`Renderer::set_boot_required`) draws no world; `client-app` shows the loading screen.

pub mod env;
pub mod globals;

pub use env::{
    BOOT_GROUP, LightTexture, LightingEnv, LightingError, STYLES, TERRAIN_KEY_SYSTEMS,
    WORLD_SETTINGS_ID, file_of, model_table_file, required_files, required_ids, terrain_table_file,
    texture_id,
};
pub use globals::{FrameGlobals, FrameInputs, row_major};

/// Whether the main pass draws the world: always, unless the renderer was told to wait for the
/// boot group and it is not complete.
pub fn world_drawn(boot_required: bool, env: &LightingEnv) -> bool {
    !boot_required || env.ready()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use asset_format::{TexDim, TextureBlob, TextureFormat, WorldSettings};

    /// A tiny blob of the shape the file has; texel `i` of a cube face / layer is `colour(face)`.
    pub(crate) fn blob(dim: TexDim, colour: impl Fn(usize) -> [u8; 4]) -> TextureBlob {
        let (side, faces, depth) = match dim {
            TexDim::D2 => (2u32, 1usize, 1u32),
            TexDim::Cube => (2, 6, 1),
            TexDim::D3 => (2, 1, 2),
        };
        let texel = |layer: usize| -> Vec<u8> {
            let n = (side * side) as usize * if dim == TexDim::D3 { depth as usize } else { 1 };
            (0..n).flat_map(|_| colour(layer)).collect()
        };
        let data: Vec<u8> = (0..faces).flat_map(texel).collect();
        TextureBlob {
            format: TextureFormat::Rgba8,
            dim,
            srgb: false,
            width: side,
            height: side,
            depth,
            mips: vec![data],
        }
    }

    pub(crate) fn settings() -> WorldSettings {
        // The settings blob round trip is tested in asset-format; the default is enough here.
        let bytes = {
            use asset_format::*;
            let ws = WorldSettings {
                fog_colors: vec![],
                fluid_fow: FluidFow::default(),
                hex_grid: HexGrid::default(),
                culture_borders: CultureBorders::default(),
                highlights: vec![],
                overlay_defs: OverlayDefs {
                    unit_selection_highlight: UnitSelectionHighlight {
                        color: [1.0; 4],
                        pulse: false,
                        pulse_speed: 0.0,
                        rotation: Rotation::Off,
                        rotation_speed: 0.0,
                        extra: Default::default(),
                    },
                    extra: Default::default(),
                },
                arrows: Arrows::default(),
                color_keys: vec![],
                ..WorldSettings::default()
            };
            ws.encode()
        };
        WorldSettings::decode(&bytes).expect("settings round trip")
    }

    /// Every file of the boot group but `skip`, in the shape the pipeline makes it.
    pub(crate) fn boot_blobs(skip: &[&str]) -> Vec<(String, TextureBlob)> {
        boot_blobs_with(skip, &|_, l| [l as u8 * 40, 7, 9, 255])
    }

    /// As [`boot_blobs`], the colour of layer `l` of a file given by `colour(file, l)`.
    pub(crate) fn boot_blobs_with(
        skip: &[&str],
        colour: &dyn Fn(&str, usize) -> [u8; 4],
    ) -> Vec<(String, TextureBlob)> {
        required_files()
            .into_iter()
            .filter(|f| !skip.contains(&f.as_str()))
            .map(|file| {
                let dim = if file.ends_with(".tga") {
                    TexDim::D3
                } else if file == env::SPEC_SCREEN_MAP || file == env::FOG_RAMP {
                    TexDim::D2
                } else {
                    TexDim::Cube
                };
                let b = blob(dim, |l| colour(&file, l));
                (file, b)
            })
            .collect()
    }

    fn fill(env: &mut LightingEnv, gpu: &Gpu, skip: &[&str]) {
        for (file, b) in boot_blobs(skip) {
            env.add_texture(&gpu.device, &gpu.queue, &file, &b)
                .unwrap_or_else(|e| panic!("{e}"));
        }
    }

    #[test]
    fn the_environment_holds_fifteen_terrain_tables_and_waits_for_boot() {
        let Some(gpu) = gpu(
            "the_environment_holds_fifteen_terrain_tables_and_waits_for_boot",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut env = LightingEnv::new();
        assert!(!env.ready());
        // One file short: not ready, and the progress says which.
        fill(&mut env, &gpu, &["fogramp.dds"]);
        env.set_world_settings(settings());
        assert!(!env.ready());
        assert_eq!(env.missing(), ["fogramp.dds"]);
        assert!(env.fog_ramp().is_none());
        // The 15 tables of the terrain are all there, by (k, class).
        assert_eq!(env.terrain_tables_loaded(), 15);
        for k in 0..3 {
            for class in 0..5 {
                assert!(env.terrain_color_key(k, class).is_some(), "{k} {class}");
            }
        }
        assert!(env.model_color_key(0).is_some() && env.rivers_color_key().is_some());
        assert!(env.terrain_sunlight(4).is_some());
        assert!(env.terrain_shadowlight(1, true).is_some());
        assert!(
            env.terrain_shadowlight(0, true).is_none(),
            "africa has no second cube"
        );
        // The bake reads the terrain cubes through an sRGB copy.
        assert!(env.terrain_sunlight_srgb(4).is_some());
        assert!(env.terrain_shadowlight_srgb(1, true).is_some());
        assert!(env.terrain_shadowlight_srgb(0, true).is_none());
        // The last file completes the group.
        env.add_texture(
            &gpu.device,
            &gpu.queue,
            "fogramp.dds",
            &blob(TexDim::D2, |_| [1, 2, 3, 4]),
        )
        .unwrap();
        assert!(env.ready());
        assert_eq!(env.progress().0, env.progress().1);
        // A texture of the wrong shape or an unknown file is refused.
        let wrong = blob(TexDim::D2, |_| [0; 4]);
        assert!(matches!(
            env.add_texture(&gpu.device, &gpu.queue, "unit_irradiance.dds", &wrong),
            Err(LightingError::Shape { .. })
        ));
        assert!(matches!(
            env.add_texture(&gpu.device, &gpu.queue, "nothing.dds", &wrong),
            Err(LightingError::Unknown(_))
        ));
    }

    #[test]
    fn without_boot_the_world_is_not_drawn_with_it_it_is() {
        let mut env = LightingEnv::new();
        // A renderer that waits for the boot group draws nothing before it, and one that does not
        // wait (the assets of B1) draws as before.
        assert!(!world_drawn(true, &env));
        assert!(world_drawn(false, &env));
        env.set_world_settings(settings());
        assert!(
            !world_drawn(true, &env),
            "the settings alone are not the group"
        );
    }

    /// The renderer waits for the boot group: a frame before it has no draw call, a frame after
    /// it has the world (here the test grid and sphere, which stand in for a map).
    #[test]
    fn a_renderer_that_waits_for_boot_draws_the_world_only_after_it() {
        use crate::game_camera::GameCamera;
        use crate::renderer::Renderer;
        use crate::scene::Scene;
        use draw_list::UiDrawList;
        let Some(gpu) = gpu(
            "a_renderer_that_waits_for_boot_draws_the_world_only_after_it",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (64, 64);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut r = Renderer::offscreen(&gpu, size, format, 1);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("boot gate target"),
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
        let camera = GameCamera::new(glam::Vec2::ZERO);
        let draws = |r: &mut Renderer| {
            let commands = r.encode_frame(
                &view,
                &Scene::test(0.0),
                &camera,
                &UiDrawList::default(),
                None,
            );
            gpu.queue.submit([commands]);
            r.frame_submitted();
            r.last_frame_stats().draws
        };
        // Not waiting (B1 assets): drawn.
        assert!(draws(&mut r) > 0);
        r.set_boot_required(true);
        assert!(!r.world_drawn());
        assert_eq!(draws(&mut r), 0, "no world before the boot group");
        // The group arrives.
        for (file, b) in boot_blobs(&[]) {
            r.add_lighting_texture(&file, &b).unwrap();
        }
        r.set_world_settings(settings());
        assert!(r.lighting().ready() && r.world_drawn());
        assert!(draws(&mut r) > 0, "the world after it");
    }

    const CUBE_TEST: &str = "
@group(0) @binding(0) var<uniform> dir: vec4<f32>;
@group(0) @binding(1) var cube: texture_cube<f32>;
@group(0) @binding(2) var cube_smp: sampler;
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}
@fragment
fn fs() -> @location(0) vec4<f32> {
    return textureSampleLevel(cube, cube_smp, dir.xyz, 0.0);
}
";

    /// The colour a cube gives for the direction `dir`, on the backend of `gpu`.
    fn sample_cube(gpu: &Gpu, cube: &LightTexture, dir: [f32; 3]) -> [u8; 4] {
        use wgpu::util::DeviceExt;
        let device = &gpu.device;
        let module = crate::shaders::create_module(device, "cube test", CUBE_TEST);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cube test"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dir"),
            contents: bytemuck::cast_slice(&[dir[0], dir[1], dir[2], 0.0]),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cube test"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&cube.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cube test"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cube test"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cube test target"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cube test"),
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
            use crate::perf::PassCount as _;
            pass.set_pipeline_c(&pipeline);
            pass.set_bind_group_c(0, &group, &[]);
            pass.draw_c(0..3, 0..1);
        }
        gpu.queue.submit([encoder.finish()]);
        let px = read_texture(gpu, &target, 0, 0, (1, 1), 4);
        [px[0], px[1], px[2], px[3]]
    }

    /// A cube whose faces are labelled (`face * 40` in red): the direction along an axis gives
    /// the face of that axis, on WebGPU and on GL (the nearest to WebGL2). The face order is that
    /// of D3D and of the blobs: +X, -X, +Y, -Y, +Z, -Z.
    #[test]
    fn a_cube_with_labelled_faces_is_sampled_by_axis_on_both_backends() {
        for (name, backend) in [
            ("default", wgpu::Backends::PRIMARY),
            ("gl", wgpu::Backends::GL),
        ] {
            let Some(gpu) = gpu(
                "a_cube_with_labelled_faces_is_sampled_by_axis_on_both_backends",
                backend,
                wgpu::Features::empty(),
            ) else {
                continue;
            };
            let mut env = LightingEnv::new();
            env.add_texture(
                &gpu.device,
                &gpu.queue,
                env::UNIT_IRRADIANCE,
                &blob(TexDim::Cube, |face| [(face as u8 + 1) * 30, 0, 0, 255]),
            )
            .unwrap();
            let cube = env.texture(env::UNIT_IRRADIANCE).unwrap();
            assert_eq!(cube.dim, TexDim::Cube);
            let axes = [
                [1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 0.0, -1.0],
            ];
            for (face, dir) in axes.iter().enumerate() {
                let got = sample_cube(&gpu, cube, *dir);
                assert_eq!(
                    got[0],
                    (face as u8 + 1) * 30,
                    "{name}: direction {dir:?} gave {got:?}, face {face} expected"
                );
            }
        }
    }
}
