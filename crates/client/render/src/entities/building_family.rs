//! The `Building` family in B1 (world-objects WOB1): the composed city model of B1 drawn with the
//! ported `BuildingShader` (`shaders/building/building.wgsl`) instead of the lambert of
//! `static.wgsl`: light from the cubes of the `boot` group, the gamma 2.2 of the original.
//!
//! It is the counterpart of `unit_family` and keeps B1's vertex and instance (`VertexLayout` /
//! `InstanceLayout` of `unit_family` say so for the pipeline cache), B1's map copy and fog of war,
//! and the frame constants in group 0. It runs only when the renderer waits for the boot group
//! (`set_boot_required`) and the group has arrived; otherwise B1's own pipeline draws the city.
//!
//! The cubes. `BuildingPS` names `Irradiance`, `SpecSharp` and `SpecSoft`. The game's own binding
//! is not recovered (RW-4 d): the specular pair is that of the units (`unit_environment_sharp` and
//! `_dull`, the same pair of the original's own models), the irradiance the generic cube of the
//! models (`cubelight_generic_32`).
// TODO(RW-4 d): the cubes `pBuildingGlobal` binds.

use crate::entities::unit_family::{instance_layout, material_layout, vertex_layout};
use crate::lighting::LightingEnv;
use crate::lighting::env::{MODEL_CUBES, UNIT_ENVIRONMENT_DULL, UNIT_ENVIRONMENT_SHARP};
use crate::material::{
    MaterialDesc, PassKind, PipelineCache, PipelineRequest, RenderState, Selection, resolve,
};
use crate::perf::{Buf, Cluster, DeviceTrack};

/// `MaterialDesc.package` of the buildings of a city.
pub const PACKAGE: &str = "BuildingShader";

/// The irradiance cube of the buildings: `CubeLight_Generic_32` (see the module text).
pub const IRRADIANCE: &str = MODEL_CUBES[3];

/// Size of the `BuildingFow` block of the shader: `OverlayProjection` and `f4FOWBaseScale`.
const FOW_BLOCK_SIZE: u64 = 64 + 16;

/// The group-0 layout: the frame constants (binding 1), the three cubes (2..7) and the fog-of-war
/// projection (10).
pub fn frame_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let cube = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::Cube,
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
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("building frame layout"),
        entries: &[
            uniform(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
            cube(2),
            sampler(3),
            cube(4),
            sampler(5),
            cube(6),
            sampler(7),
            uniform(10, wgpu::ShaderStages::VERTEX),
        ],
    })
}

/// Where a frame draws buildings with the ported shader: the group 0 and the pipeline.
pub struct BuildingGpu {
    pub frame_group: wgpu::BindGroup,
    pub pipeline: wgpu::RenderPipeline,
    /// The reflection pass: the pipeline with the cut at the water plane and the group 0 of the
    /// reflection camera's constants. Absent when the pipeline could not be made.
    pub reflection: Option<(wgpu::RenderPipeline, wgpu::BindGroup)>,
}

/// What the family keeps: the layouts, the cache, and the group and pipeline once the boot group
/// is in.
pub struct BuildingFamily {
    frame_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    cache: PipelineCache,
    /// `BuildingFow`: zero until `FowMask` of `world-overlays` supplies the projection.
    fow: Buf,
    gpu: Option<BuildingGpu>,
    target: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    samples: u32,
    /// A try that failed is not repeated every frame.
    failed: bool,
}

impl BuildingFamily {
    pub fn new(
        device: &wgpu::Device,
        target: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        // Linear filter and repeat, as the unit family (`material::sampler::ORIGINAL_SAMPLERS`
        // has no row yet).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("building sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let fow = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("building fow projection"),
                size: FOW_BLOCK_SIZE,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: true,
            },
            Cluster::Other,
        );
        fow.unmap();
        Self {
            frame_layout: frame_layout(device),
            // `Diffuse` and `BuildingSREF`: two textures and their samplers, as the units'.
            material_layout: material_layout(device),
            sampler,
            cache: PipelineCache::new(),
            fow,
            gpu: None,
            target,
            depth,
            samples,
            failed: false,
        }
    }

    pub fn gpu(&self) -> Option<&BuildingGpu> {
        self.gpu.as_ref()
    }

    /// The group 3 of a material: its `Diffuse` and its `BuildingSREF` (the black one when the
    /// material has none: no highlight).
    pub fn material_group(
        &self,
        device: &wgpu::Device,
        diffuse: &wgpu::TextureView,
        sref: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let tex = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let smp = |binding| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::Sampler(&self.sampler),
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("building material"),
            layout: &self.material_layout,
            entries: &[tex(0, diffuse), smp(1), tex(2, sref), smp(3)],
        })
    }

    /// Makes the group 0 and the pipeline once the light environment is complete. `object` and
    /// `fog` are B1's layouts of groups 1 and 2. Does nothing before, or after a failure.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        env: &LightingEnv,
        frame_globals: &wgpu::Buffer,
        reflection_globals: &wgpu::Buffer,
        object: &wgpu::BindGroupLayout,
        fog: &wgpu::BindGroupLayout,
    ) {
        if self.gpu.is_some() || self.failed || !env.ready() {
            return;
        }
        let (Some(irradiance), Some(sharp), Some(soft)) = (
            env.view(IRRADIANCE),
            env.view(UNIT_ENVIRONMENT_SHARP),
            env.view(UNIT_ENVIRONMENT_DULL),
        ) else {
            self.failed = true;
            return;
        };
        let desc = MaterialDesc {
            package: PACKAGE.to_owned(),
            ..MaterialDesc::default()
        };
        let (family, selection): (_, Selection) = match resolve(&desc, PassKind::Main) {
            Ok(r) => r,
            Err(why) => {
                tracing::error!("the building family: {why}");
                self.failed = true;
                return;
            }
        };
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
                tracing::error!("the building pipeline: {why}");
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
        let group = |label: &str, globals: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &self.frame_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: globals.as_entire_binding(),
                    },
                    tex(2, irradiance),
                    smp(3),
                    tex(4, sharp),
                    smp(5),
                    tex(6, soft),
                    smp(7),
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: self.fow.as_entire_binding(),
                    },
                ],
            })
        };
        let frame_group = group("building frame", frame_globals);
        // The reflection pass: the same entries cut at the water plane, into a target that is never
        // multisampled.
        let reflection = match resolve(&desc, PassKind::Reflection) {
            Ok((family, selection)) => {
                let request = PipelineRequest {
                    family,
                    selection: &selection,
                    pass: PassKind::Reflection,
                    state: RenderState::of_material(&desc, PassKind::Reflection),
                    target: self.target,
                    depth: Some(self.depth),
                    samples: 1,
                    vertex: &vertex,
                    instance: Some(&instance),
                    layouts: &[&self.frame_layout, object, fog, &self.material_layout],
                };
                match self.cache.get(device, &request) {
                    Ok(p) => Some((p, group("building reflection frame", reflection_globals))),
                    Err(why) => {
                        tracing::error!("the building reflection pipeline: {why}");
                        None
                    }
                }
            }
            Err(why) => {
                tracing::error!("the building reflection entries: {why}");
                None
            }
        };
        tracing::info!("the building family is on: BuildingShader::BuildingVS + BuildingPS");
        self.gpu = Some(BuildingGpu {
            frame_group,
            pipeline,
            reflection,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_has_a_family_with_the_building_entries() {
        let desc = MaterialDesc {
            package: PACKAGE.into(),
            ..MaterialDesc::default()
        };
        let (family, s) = resolve(&desc, PassKind::Main).unwrap();
        assert_eq!(
            (family.module, s.vs, s.fs),
            ("building", "building_vs", "building_ps")
        );
        // The shadow entries are not ported: no call in that pass.
        assert!(resolve(&desc, PassKind::Shadow).is_err());
        // The reflection pass runs the same entries, cut at the water plane.
        let (_, r) = resolve(&desc, PassKind::Reflection).unwrap();
        assert_eq!((r.vs, r.fs), ("building_vs", "building_reflect_ps"));
    }

    /// A city through the whole chain: the cubes of the boot group light it, with no highlight
    /// (the `BuildingSREF` is black), in the gamma 2.2 of `BuildingPS`. With every cube a grey `g`
    /// and a diffuse grey `d` the colour is `(2 (d g)^2.2)^(1/2.2) = 2^(1/2.2) d g`, wherever the
    /// normal points: (128, 200) give 138.
    #[test]
    fn a_city_takes_the_light_of_the_cubes_in_gamma_2_2() {
        use crate::game_camera::GameCamera;
        use crate::gpu_test::{gpu, read_texture};
        use crate::hexgrid::cell_center;
        use crate::lighting::tests::{boot_blobs_with, settings};
        use crate::renderer::Renderer;
        use crate::scene::Scene;
        use crate::static_model::{Look, StaticInstance};
        use crate::terrain::{TerrainCell, TerrainMesh, TerrainSnapshot};
        use asset_format::{LogicalId, Material, TextureBlob, TextureFormat};
        use draw_list::UiDrawList;
        use std::collections::HashMap;

        let Some(gpu) = gpu(
            "a_city_takes_the_light_of_the_cubes_in_gamma_2_2",
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
        // A diffuse texture of grey 128, opaque.
        let id = LogicalId::new("tx:building_diffuse");
        let mut model = crate::entities::marker::pillar((6.0, 30.0));
        model.materials = vec![Material {
            diffuse: Some(id.clone()),
            specular: None,
        }];
        let diffuse = TextureBlob {
            format: TextureFormat::Rgba8,
            dim: asset_format::TexDim::D2,
            srgb: true,
            width: 1,
            height: 1,
            depth: 1,
            mips: vec![vec![128, 128, 128, 255]],
        };
        let textures: HashMap<_, _> = [(id, diffuse)].into();
        let at = cell_center(3, 2);
        let camera = {
            let mut c = GameCamera::new(at);
            c.set_level(10.0);
            c
        };
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("building target"),
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
        r.test_model(
            Look::Building,
            &model,
            &textures,
            &[StaticInstance::new([at.x, 0.0, at.y], 0.0, 1.0)],
        );
        let mut frame = || {
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
        // The first frame makes the family; the second draws with it.
        frame();
        let px = frame();
        assert!(r.statics_building_family_on(), "the family is on");
        // Some pixels of the pillar have the colour of the formula, in all three channels.
        let lit = px
            .chunks(4)
            .filter(|p| (0..3).all(|k| (i32::from(p[k]) - 138).abs() <= 3))
            .count();
        assert!(lit > 20, "{lit} pixels of the building's colour");
    }

    #[test]
    fn the_fow_block_is_a_matrix_and_a_vector() {
        // `BuildingFow` of the shader: mat4x4 and vec4, 80 bytes.
        assert_eq!(FOW_BLOCK_SIZE, 80);
    }

    #[test]
    fn the_cubes_are_those_of_the_boot_group() {
        let files = crate::lighting::env::required_files();
        for f in [IRRADIANCE, UNIT_ENVIRONMENT_SHARP, UNIT_ENVIRONMENT_DULL] {
            assert!(files.iter().any(|x| x == f), "{f}");
        }
    }
}
