//! The ground of the milestone M1 (world-terrain WT-0, WT-D7): B1's terrain mesh drawn with the
//! light of the original, in one pass (`shaders/terrain/m1_forward.wgsl`). The adapter of B1
//! (`frame::b1_systems::B1Terrain`) draws through it when the renderer waits for the boot group
//! (`Renderer::set_boot_required`) and the group has arrived; otherwise B1's own pipeline draws the
//! ground, as before.
//!
//! What the adapter keeps of B1: the vertex (`TerrainGpuVertex`), the object group of the map copy
//! (group 1), the fog state (group 2) and the texture array with its slot table (group 3). What
//! comes in group 0 is the frame (`FrameGlobals`, binding 1) and the light of the continent style:
//! its sun and shadow cubes and the three colour volumes, the shadow map and the fog ramp. The
//! light depends on the style, which B1 gives per cell, so the mesh is drawn once per style over
//! the triangles of that style ([`ClassRanges`]); each draw has its own group 0.

use std::ops::Range;

use crate::lighting::{LightingEnv, STYLES as ENV_STYLES};
use crate::shaders::{catalog, create_module};
use crate::terrain::texture_array::STYLES as ART_STYLES;

/// Names of the continent styles of B1's art table (`TerrainCell::art`), in its order.
// re: TerrainSystem.cpp:666-676 ([0] Generic, [1] America, [2] Asia,
// [3] Africa, [4] Europe)
pub const ART_STYLE_NAMES: [&str; ART_STYLES] = ["generic", "america", "asia", "africa", "europe"];

/// The index of the style `art` in `LightingEnv`'s order (which is that of the file names).
pub fn env_class(art_style: usize) -> usize {
    let name = ART_STYLE_NAMES.get(art_style).copied().unwrap_or("generic");
    ENV_STYLES
        .iter()
        .position(|s| *s == name)
        .expect("the styles of the art table are the styles of the light files")
}

/// The ranges of the index buffer of one region (the land, or the water surface) by style.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassRanges {
    pub ranges: [Range<u32>; ART_STYLES],
}

/// The indices of `region`, triangle by triangle, stable-sorted by the style of the triangle (that
/// of its first vertex, `style_of_vertex`), and the range each style takes.
pub fn partition_by_class(
    region: &[u32],
    style_of_vertex: impl Fn(u32) -> usize,
) -> (Vec<u32>, ClassRanges) {
    let mut buckets: [Vec<u32>; ART_STYLES] = Default::default();
    for tri in region.chunks_exact(3) {
        let style = style_of_vertex(tri[0]).min(ART_STYLES - 1);
        buckets[style].extend_from_slice(tri);
    }
    let mut out = Vec::with_capacity(region.len());
    let mut ranges: [Range<u32>; ART_STYLES] = Default::default();
    for (style, b) in buckets.iter().enumerate() {
        let start = out.len() as u32;
        out.extend_from_slice(b);
        ranges[style] = start..out.len() as u32;
    }
    (out, ClassRanges { ranges })
}

/// Shifts every range by `by` (the water region follows the land in the index buffer).
pub fn shifted(ranges: &ClassRanges, by: u32) -> ClassRanges {
    ClassRanges {
        ranges: std::array::from_fn(|i| {
            let r = &ranges.ranges[i];
            r.start + by..r.end + by
        }),
    }
}

/// Group-0 layout: the frame constants (binding 1), then (texture, sampler) pairs: 2/3 the sun cube,
/// 4/5 the shadow cube, 6/7 the colour volume of the page, 8/9 the shadow map, 10/11 the shadow
/// colour volume, 12/13 the fog-of-war volume, 14/15 the fog ramp (`terrain/common.wgsl`,
/// `m1_forward.wgsl`).
pub fn frame_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    use wgpu::TextureViewDimension::{Cube, D2, D3};
    let texture = |binding, dim| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    };
    let sampler = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    let mut entries = vec![wgpu::BindGroupLayoutEntry {
        binding: 1,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }];
    for (binding, dim) in [
        (2, Cube),
        (4, Cube),
        (6, D3),
        (8, D2),
        (10, D3),
        (12, D3),
        (14, D2),
    ] {
        entries.push(texture(binding, dim));
        entries.push(sampler(binding + 1));
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain m1 frame layout"),
        entries: &entries,
    })
}

/// The pipeline and the groups 0 of the styles, once the light environment is complete.
pub struct TerrainM1 {
    frame_layout: wgpu::BindGroupLayout,
    /// Cubes and volumes clamp, the shadow map repeats (the shader's coordinates are negative).
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    pipeline: Option<wgpu::RenderPipeline>,
    /// Group 0 of each style of B1's art table, made for the frame being drawn.
    groups: [Option<wgpu::BindGroup>; ART_STYLES],
    target: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    samples: u32,
    /// A try that failed is not repeated every frame.
    failed: bool,
}

impl TerrainM1 {
    pub fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        Self {
            frame_layout: frame_layout(device),
            clamp: sampler("terrain m1 clamp", wgpu::AddressMode::ClampToEdge),
            repeat: sampler("terrain m1 repeat", wgpu::AddressMode::Repeat),
            pipeline: None,
            groups: Default::default(),
            target,
            depth,
            samples,
            failed: false,
        }
    }

    /// The ground is drawn with the light of the original this frame.
    pub fn on(&self) -> bool {
        self.pipeline.is_some() && self.groups.iter().any(Option::is_some)
    }

    pub fn pipeline(&self) -> Option<&wgpu::RenderPipeline> {
        self.pipeline.as_ref()
    }

    /// Group 0 of the style `art_style`; the generic one for a style that has no light of its own.
    pub fn group(&self, art_style: usize) -> Option<&wgpu::BindGroup> {
        self.groups
            .get(art_style)
            .and_then(Option::as_ref)
            .or(self.groups[0].as_ref())
    }

    /// Makes the pipeline (once) and the groups 0 of the frame. `object`, `fog` and `material` are
    /// B1's layouts of groups 1..3. Does nothing before the environment is complete, and after a
    /// failure leaves B1's pipeline in charge.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        env: &LightingEnv,
        frame_globals: &wgpu::Buffer,
        shadow_map: &wgpu::TextureView,
        layouts: [&wgpu::BindGroupLayout; 3],
    ) {
        if self.failed || !env.ready() {
            return;
        }
        if self.pipeline.is_none() && !self.make_pipeline(device, layouts) {
            self.failed = true;
            return;
        }
        let Some(ramp) = env.fog_ramp() else {
            self.failed = true;
            return;
        };
        for (style, slot) in self.groups.iter_mut().enumerate() {
            *slot = Self::make_group(
                device,
                &self.frame_layout,
                &self.clamp,
                &self.repeat,
                env,
                env_class(style),
                frame_globals,
                shadow_map,
                ramp,
            );
        }
        if self.groups[0].is_none() {
            // Not even the generic style is complete: the boot group lacks a file.
            self.failed = true;
            self.pipeline = None;
        }
    }

    fn make_pipeline(
        &mut self,
        device: &wgpu::Device,
        layouts: [&wgpu::BindGroupLayout; 3],
    ) -> bool {
        let Some(module) = catalog().iter().find(|m| m.name == "terrain_m1") else {
            tracing::error!("the terrain m1 module is not in the catalog");
            return false;
        };
        let shader = create_module(device, "terrain m1", &module.source());
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain m1 layout"),
            bind_group_layouts: &[
                Some(&self.frame_layout),
                Some(layouts[0]),
                Some(layouts[1]),
                Some(layouts[2]),
            ],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Uint32, 3 => Uint32
        ];
        let pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("terrain m1 pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("terrain_m1_vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<
                            crate::terrain::texture_array::TerrainGpuVertex,
                        >() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: self.depth,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: crate::frame::multisample(self.samples),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("terrain_m1_ps"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.target,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        self.pipeline = Some(pipeline);
        tracing::info!("the terrain draws with the light of the original (M1)");
        true
    }

    /// Group 0 for one style, `None` if one of its files is missing.
    #[allow(clippy::too_many_arguments)]
    fn make_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        clamp: &wgpu::Sampler,
        repeat: &wgpu::Sampler,
        env: &LightingEnv,
        class: usize,
        frame_globals: &wgpu::Buffer,
        shadow_map: &wgpu::TextureView,
        ramp: &wgpu::TextureView,
    ) -> Option<wgpu::BindGroup> {
        let sun = env.terrain_sunlight(class)?;
        // Upgrade1 has a second shadow cube for three styles; that one is the shadow of the game.
        let shade = env
            .terrain_shadowlight(class, true)
            .or_else(|| env.terrain_shadowlight(class, false))?;
        // k = 0 colour correction of the page, 1 shadow colour, 2 fog of war (RW-4 g: the real
        // order of `g_ColorKeySystem.m_auKeyHandles[k]` is not read yet).
        let correct = env.terrain_color_key(0, class)?;
        let shadow_color = env.terrain_color_key(1, class)?;
        let fow_color = env.terrain_color_key(2, class)?;
        let tex = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let smp = |binding, s| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Sampler(s),
        };
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain m1 frame"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: frame_globals.as_entire_binding(),
                },
                tex(2, sun),
                smp(3, clamp),
                tex(4, shade),
                smp(5, clamp),
                tex(6, correct),
                smp(7, clamp),
                tex(8, shadow_map),
                smp(9, repeat),
                tex(10, shadow_color),
                smp(11, clamp),
                tex(12, fow_color),
                smp(13, clamp),
                tex(14, ramp),
                smp(15, clamp),
            ],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pipeline of M1 is made over B1's layouts with the cutout mask in the material group (the
    /// device validates the layouts against the shader).
    #[test]
    fn the_m1_pipeline_takes_the_cutout_bindings_of_the_material_group() {
        use crate::terrain_pipeline::{TerrainPipeline, TerrainTargets};
        let Some(gpu) = crate::gpu_test::gpu(
            "the_m1_pipeline_takes_the_cutout_bindings_of_the_material_group",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let device = &gpu.device;
        let uniform = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        let globals = layout(&[uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)]);
        let object = layout(&[uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT)]);
        let fog = layout(&[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }]);
        let target = wgpu::TextureFormat::Rgba8Unorm;
        let depth = wgpu::TextureFormat::Depth24PlusStencil8;
        let terrain = TerrainPipeline::new(
            device,
            &globals,
            &object,
            &fog,
            TerrainTargets {
                view_format: target,
                depth_format: depth,
                samples: 1,
            },
        );
        let mut m1 = TerrainM1::new(device, target, depth, 1);
        assert!(m1.make_pipeline(device, [&object, &fog, terrain.material_layout()]));
        assert!(m1.pipeline().is_some());
    }

    #[test]
    fn the_styles_of_b1_are_the_styles_of_the_light_files() {
        // B1's order is Generic, America, Asia, Africa, Europe; the files' is alphabetical.
        assert_eq!(
            env_class(0),
            ENV_STYLES.iter().position(|s| *s == "generic").unwrap()
        );
        assert_eq!(
            env_class(1),
            ENV_STYLES.iter().position(|s| *s == "america").unwrap()
        );
        assert_eq!(
            env_class(3),
            ENV_STYLES.iter().position(|s| *s == "africa").unwrap()
        );
        assert_eq!(
            env_class(4),
            ENV_STYLES.iter().position(|s| *s == "europe").unwrap()
        );
        // A style outside the table is the generic one.
        assert_eq!(env_class(9), env_class(0));
        // Every style is distinct.
        let mut all: Vec<usize> = (0..ART_STYLES).map(env_class).collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), ART_STYLES);
    }

    #[test]
    fn triangles_are_sorted_by_style_and_keep_their_vertices() {
        // Four triangles with the styles 2, 0, 2, 4 (by the first vertex: 0..3 -> style).
        let region = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let style = |v: u32| match v {
            0 => 2,
            3 => 0,
            6 => 2,
            9 => 4,
            _ => 1,
        };
        let (sorted, ranges) = partition_by_class(&region, style);
        assert_eq!(sorted.len(), region.len());
        assert_eq!(&sorted[..3], &[3, 4, 5]);
        // The stable order of style 2.
        assert_eq!(&sorted[3..9], &[0, 1, 2, 6, 7, 8]);
        assert_eq!(&sorted[9..], &[9, 10, 11]);
        assert_eq!(ranges.ranges[0], 0..3);
        assert_eq!(ranges.ranges[1], 3..3);
        assert_eq!(ranges.ranges[2], 3..9);
        assert_eq!(ranges.ranges[3], 9..9);
        assert_eq!(ranges.ranges[4], 9..12);
        // The ranges tile the region.
        let total: u32 = ranges.ranges.iter().map(|r| r.end - r.start).sum();
        assert_eq!(total as usize, region.len());
        let s = shifted(&ranges, 100);
        assert_eq!(s.ranges[2], 103..109);
    }

    #[test]
    fn a_style_outside_the_table_is_the_last_bucket() {
        let (_, ranges) = partition_by_class(&[0, 1, 2], |_| 77);
        assert_eq!(ranges.ranges[ART_STYLES - 1], 0..3);
    }

    // ---- the whole chain on a GPU ----

    use crate::game_camera::GameCamera;
    use crate::gpu_test::{gpu, read_texture};
    use crate::hexgrid::cell_center;
    use crate::lighting::tests::{boot_blobs_with, settings};
    use crate::renderer::Renderer;
    use crate::scene::Scene;
    use crate::terrain::{Fog, TerrainCell, TerrainMesh, TerrainSnapshot};
    use draw_list::UiDrawList;

    const SIZE: (u32, u32) = (96, 96);

    /// The colour of every texel of a file of the boot group: `terrain_<style>_colorkey` (the
    /// colour correction of the page) 100, `terrainshadow_*` 150, `terrainfow_*` 50, the rest 200.
    fn grey_of(file: &str, _layer: usize) -> [u8; 4] {
        let v = if file.starts_with("terrainshadow_") {
            150
        } else if file.starts_with("terrainfow_") {
            50
        } else if file.starts_with("terrain_") {
            100
        } else {
            200
        };
        [v, v, v, 255]
    }

    /// The colour at the middle of a frame of a 7 x 5 map whose cell `(x, y)` has the fog `fog_of`,
    /// seen from above the cell `(3, 2)`.
    fn middle_pixel(test: &str, fog_of: impl Fn(i32, i32) -> Fog) -> Option<[u8; 4]> {
        let gpu = gpu(test, wgpu::Backends::PRIMARY, wgpu::Features::empty())?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut r = Renderer::offscreen(&gpu, SIZE, format, 1);
        r.set_boot_required(true);
        for (file, b) in boot_blobs_with(&[], &grey_of) {
            r.add_lighting_texture(&file, &b).unwrap();
        }
        r.set_world_settings(settings());
        let (w, h) = (7u32, 5u32);
        let cells = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .map(|(x, y)| TerrainCell {
                fog: fog_of(x, y),
                ..TerrainCell::default()
            })
            .collect();
        let snapshot = TerrainSnapshot::new(w, h, false, cells).unwrap();
        r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
        let mut camera = GameCamera::new(cell_center(3, 2));
        camera.set_level(10.0);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain m1 target"),
            size: wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
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
        let commands = r.encode_frame(
            &view,
            &Scene::test(0.0),
            &camera,
            &UiDrawList::default(),
            None,
        );
        gpu.queue.submit([commands]);
        r.frame_submitted();
        assert!(r.terrain_m1_on(), "the light of the original is on");
        let px = read_texture(&gpu, &target, 0, 0, SIZE, 4);
        let at = ((SIZE.1 / 2) * SIZE.0 + SIZE.0 / 2) as usize * 4;
        Some([px[at], px[at + 1], px[at + 2], px[at + 3]])
    }

    /// The light of the original with the colour volumes at one grey: the ground that is seen has
    /// the grey of the colour correction of the page (the cubes and the albedo go through it), the
    /// ground that is known but not seen the grey of the fog-of-war volume.
    #[test]
    fn seen_ground_takes_the_colour_correction_and_hidden_ground_the_fog_of_war_volume() {
        let Some(seen) = middle_pixel("terrain m1 seen", |_, _| Fog::Visible) else {
            return;
        };
        // 100 / 255 through the volume of the page, no shadow, no fog: 100.
        assert!(
            seen[..3].iter().all(|&c| (i32::from(c) - 100).abs() <= 2),
            "{seen:?}"
        );
        let hidden = middle_pixel("terrain m1 hidden", |_, _| Fog::Hidden).unwrap();
        // The volume of the fog of war: 50.
        assert!(
            hidden[..3].iter().all(|&c| (i32::from(c) - 50).abs() <= 2),
            "{hidden:?}"
        );
    }

    /// Cells nobody has seen show the base colour of the fog of war: B1's flat colour as gamma.
    #[test]
    fn unrevealed_ground_has_the_base_colour() {
        let Some(px) = middle_pixel("terrain m1 unrevealed", |_, _| Fog::Unrevealed) else {
            return;
        };
        let lin = crate::blend::srgb_to_linear(crate::terrain::fog::FOG_UNREVEALED_RGB);
        for k in 0..3 {
            let want = (lin[k].powf(1.0 / 2.2) * 255.0).round() as i32;
            assert!((i32::from(px[k]) - want).abs() <= 2, "{px:?} {want}");
        }
    }
}
