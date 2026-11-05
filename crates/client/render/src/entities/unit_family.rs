//! The pilot family in B1: the rigid unit models of B1 drawn with the ported `UnitShader`
//! (`shaders/unit/unit.wgsl`, RC-15) instead of the lambert of `static.wgsl`: light from the cubes
//! of the `boot` group, the team colour, `SREFMap`, the gamma of the original.
//!
//! The adapter turns B1's data into what the shader of the original expects: the vertex and the
//! instance of the static pipeline keep their layout (`VertexLayout` / `InstanceLayout` below say
//! so for the pipeline cache), the map copy and the fog of war of B1 stay as they are, and the
//! frame constants (`FrameGlobals`) and the cubes come in group 0.
//!
//! It runs only when the renderer waits for the boot group (`set_boot_required`) and the group has
//! arrived; otherwise B1's own pipeline draws the unit, as before.

use crate::lighting::LightingEnv;
use crate::material::{
    InstanceLayout, MaterialDesc, PassKind, PipelineCache, PipelineRequest, RenderState, Selection,
    VertexAttr, VertexLayout, resolve,
};
use crate::shaders::Semantic;

/// `MaterialDesc.package` of the rigid opaque unit models of B1.
pub const PACKAGE: &str = "UnitShader";

/// The group-0 layout: the frame constants (binding 1) and the light environment (2..9).
pub fn frame_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("unit frame layout"),
        entries: &frame_entries(),
    })
}

/// The entries of [`frame_layout`] (data, so that a test counts the textures of a pipeline).
pub fn frame_entries() -> Vec<wgpu::BindGroupLayoutEntry> {
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
    use wgpu::TextureViewDimension::{Cube, D2};
    vec![
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        texture(2, Cube),
        sampler(3),
        texture(4, Cube),
        sampler(5),
        texture(6, Cube),
        sampler(7),
        texture(8, D2),
        sampler(9),
    ]
}

/// The group-3 layout: `BaseTextureMap` and `SREFMap` with their samplers.
pub fn material_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
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
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("unit material layout"),
        entries: &[texture(0), sampler(1), texture(2), sampler(3)],
    })
}

/// The vertex of B1's static pipeline (`StaticVertex`, 24 bytes) in the semantics of the table:
/// position, octahedral normal, `TEXCOORD0` the uv, `TEXCOORD1` the variant.
pub fn vertex_layout() -> VertexLayout {
    use wgpu::VertexFormat as F;
    let attr = |semantic, format, offset| VertexAttr {
        semantic,
        format,
        offset,
    };
    VertexLayout::new(
        24,
        vec![
            attr(Semantic::Position0, F::Float32x3, 0),
            attr(Semantic::Normal0, F::Snorm16x2, 12),
            attr(Semantic::Texcoord(0), F::Float16x2, 16),
            attr(Semantic::Texcoord(1), F::Uint32, 20),
        ],
    )
    .expect("the static vertex is a valid layout")
}

/// The instance of B1 (`StaticInstance`, 48 bytes): they take `TEXCOORD2..` (locations 4..9).
pub fn instance_layout() -> InstanceLayout {
    use wgpu::VertexFormat as F;
    InstanceLayout::new(
        48,
        vec![
            (F::Float32x4, 0),  // position, turn about Y
            (F::Float32x4, 16), // scale, tint
            (F::Float32, 32),   // alpha
            (F::Uint32, 36),    // fog cell
            (F::Uint32, 40),    // variant
            (F::Uint32, 44),    // team colour
        ],
    )
}

/// Where a frame draws units with the ported shader: the group 0 and the pipeline.
pub struct UnitGpu {
    pub frame_group: wgpu::BindGroup,
    pub pipeline: wgpu::RenderPipeline,
}

/// What the adapter keeps: the layouts, the cache, and the group and pipeline once the boot group
/// is in.
pub struct UnitFamily {
    frame_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    cache: PipelineCache,
    gpu: Option<UnitGpu>,
    target: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    samples: u32,
    /// A try that failed is not repeated every frame.
    failed: bool,
}

impl UnitFamily {
    pub fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        // Linear filter and repeat: the table of sampler states of the original has no row yet
        // (`material::sampler::ORIGINAL_SAMPLERS`, filled from the capture S1).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("unit sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            frame_layout: frame_layout(device),
            material_layout: material_layout(device),
            sampler,
            cache: PipelineCache::new(),
            gpu: None,
            target,
            depth,
            samples,
            failed: false,
        }
    }

    pub fn gpu(&self) -> Option<&UnitGpu> {
        self.gpu.as_ref()
    }

    /// The group 3 of a material: its base texture and its `SREFMap` (the black one when the
    /// material has none: no highlight).
    pub fn material_group(
        &self,
        device: &wgpu::Device,
        base: &wgpu::TextureView,
        sref: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unit material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(base),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(sref),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Makes the group 0 and the pipeline once the light environment is complete. `object` and
    /// `fog` are B1's layouts of groups 1 and 2. Does nothing before, or after a failure.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        env: &LightingEnv,
        frame_globals: &wgpu::Buffer,
        object: &wgpu::BindGroupLayout,
        fog: &wgpu::BindGroupLayout,
    ) {
        if self.gpu.is_some() || self.failed || !env.ready() {
            return;
        }
        let (Some(irradiance), Some(dull), Some(sharp), Some(ramp)) = (
            env.unit_irradiance(),
            env.unit_environment_dull(),
            env.unit_environment_sharp(),
            env.fog_ramp(),
        ) else {
            self.failed = true;
            return;
        };
        let desc = MaterialDesc {
            package: PACKAGE.to_owned(),
            ..MaterialDesc::default()
        };
        let selection: (_, Selection) = match resolve(&desc, PassKind::Main) {
            Ok(r) => r,
            Err(why) => {
                tracing::error!("the unit family: {why}");
                self.failed = true;
                return;
            }
        };
        let (family, selection) = selection;
        let vertex = vertex_layout();
        let instance = instance_layout();
        let request = PipelineRequest {
            family,
            selection: &selection,
            pass: PassKind::Main,
            state: RenderState::of_material(&desc, PassKind::Main),
            target: self.target,
            depth: Some(self.depth),
            samples: self.samples,
            vertex: &vertex,
            instance: Some(&instance),
            layouts: &[&self.frame_layout, object, fog, &self.material_layout],
        };
        let pipeline = match self.cache.get(device, &request) {
            Ok(p) => p,
            Err(why) => {
                tracing::error!("the unit pipeline: {why}");
                self.failed = true;
                return;
            }
        };
        let tex = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let smp = |binding| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        };
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("unit frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: frame_globals.as_entire_binding(),
                },
                tex(2, irradiance),
                smp(3),
                tex(4, dull),
                smp(5),
                tex(6, sharp),
                smp(7),
                tex(8, ramp),
                smp(9),
            ],
        });
        tracing::info!("the unit family is on: UnitShader::UnitVS + UnitPS");
        self.gpu = Some(UnitGpu {
            frame_group,
            pipeline,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders::free_texcoord_locations;

    #[test]
    fn the_instance_takes_the_locations_the_static_shader_uses() {
        let attrs = instance_layout().attributes(&vertex_layout()).unwrap();
        let at: Vec<(u32, u64)> = attrs
            .iter()
            .map(|a| (a.shader_location, a.offset))
            .collect();
        // B1's INSTANCE_ATTRIBUTES are at 4..8; the team colour joins at 9.
        assert_eq!(at, [(4, 0), (5, 16), (6, 32), (7, 36), (8, 40), (9, 44)]);
        let free = free_texcoord_locations(&vertex_layout().semantics());
        assert_eq!(free, [4, 5, 6, 7, 8, 9]);
    }

    /// A unit through the whole chain: the cubes of the boot group light it, the team colour
    /// comes through the alpha of the base texture (`lerp(TeamColor^2, 1, base.a^2)`), the
    /// result is gamma 2.0. Irradiance (200, 200, 200)/255 and a base with alpha 0 give, at the
    /// pixels facing the camera, the team colour times 200/255 per channel.
    #[test]
    fn a_unit_takes_the_light_of_the_cubes_and_the_team_colour() {
        use crate::game_camera::GameCamera;
        use crate::gpu_test::{gpu, read_texture};
        use crate::hexgrid::cell_center;
        use crate::lighting::tests::{boot_blobs_with, settings};
        use crate::renderer::Renderer;
        use crate::scene::Scene;
        use crate::static_model::{StaticInstance, pack_team};
        use crate::terrain::{TerrainCell, TerrainMesh, TerrainSnapshot};
        use asset_format::{LogicalId, Material, TextureBlob, TextureFormat};
        use draw_list::UiDrawList;
        use std::collections::HashMap;

        let Some(gpu) = gpu(
            "a_unit_takes_the_light_of_the_cubes_and_the_team_colour",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let size = (96u32, 96u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut r = Renderer::offscreen(&gpu, size, format, 1);
        r.set_boot_required(true);
        // Every cube is grey 200, the ramp and the volumes whatever.
        for (file, b) in boot_blobs_with(&[], &|_, _| [200, 200, 200, 255]) {
            r.add_lighting_texture(&file, &b).unwrap();
        }
        r.set_world_settings(settings());
        assert!(r.world_drawn());
        let snapshot = TerrainSnapshot::new(6, 4, false, vec![TerrainCell::default(); 24]).unwrap();
        r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
        // A base texture with alpha 0: all team colour.
        let id = LogicalId::new("tx:unit_base");
        let mut model = crate::entities::marker::pillar((6.0, 30.0));
        model.materials = vec![Material {
            diffuse: Some(id.clone()),
            specular: None,
        }];
        let base = TextureBlob {
            format: TextureFormat::Rgba8,
            dim: asset_format::TexDim::D2,
            srgb: true,
            width: 1,
            height: 1,
            depth: 1,
            mips: vec![vec![255, 255, 255, 0]],
        };
        let textures: HashMap<_, _> = [(id, base)].into();
        let at = cell_center(3, 2);
        let camera = {
            let mut c = GameCamera::new(at);
            c.set_level(10.0);
            c
        };
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("unit target"),
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
        let frame = |r: &mut Renderer| {
            let commands = r.encode_frame(
                &view,
                &Scene::test(0.0),
                &camera,
                &UiDrawList::default(),
                None,
            );
            gpu.queue.submit([commands]);
            r.frame_submitted();
            read_texture(&gpu, &target, 0, 0, size, 4)
        };
        let mut best = |team: [u8; 3]| -> [u8; 4] {
            let mut instance = StaticInstance::new([at.x, 0.0, at.y], 0.0, 1.0);
            instance.team = pack_team(team);
            r.test_unit(&model, &textures, &[instance]);
            let px = frame(&mut r);
            // The pixel that is most the team's colour.
            px.chunks(4)
                .map(|p| [p[0], p[1], p[2], p[3]])
                .max_by_key(|p| {
                    let want = |c: u8, t: u8| if t > 128 { i32::from(c) } else { -i32::from(c) };
                    (0..3).map(|k| want(p[k], team[k])).sum::<i32>()
                })
                .unwrap()
        };
        let red = best([255, 0, 0]);
        let blue = best([0, 0, 255]);
        assert!(r.statics_unit_family_on(), "the family is on");
        // Gamma 2.0 of the whole chain: sqrt(1 * team^2 * (200/255)^2) = team * 200/255.
        assert!((i32::from(red[0]) - 200).abs() <= 3, "red {red:?}");
        assert!(red[1] <= 3 && red[2] <= 3, "red {red:?}");
        assert!((i32::from(blue[2]) - 200).abs() <= 3, "blue {blue:?}");
        assert!(blue[0] <= 3 && blue[1] <= 3, "blue {blue:?}");
    }

    #[test]
    fn the_vertex_is_the_static_vertex() {
        let v = vertex_layout();
        assert_eq!(
            v.stride(),
            std::mem::size_of::<crate::static_model::StaticVertex>() as u64
        );
        let locations: Vec<u32> = v
            .wgpu_attributes()
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(locations, [0, 1, 2, 3]);
    }

    #[test]
    fn the_package_has_a_family_with_the_unit_entries() {
        let desc = MaterialDesc {
            package: PACKAGE.into(),
            ..MaterialDesc::default()
        };
        let (family, s) = resolve(&desc, PassKind::Main).unwrap();
        assert_eq!((family.module, s.vs, s.fs), ("unit", "unit_vs", "unit_ps"));
        // The shadow entry is not ported: no call in that pass.
        assert!(resolve(&desc, PassKind::Shadow).is_err());
    }
}
