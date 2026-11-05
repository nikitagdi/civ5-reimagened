//! The `static` pipeline (shared with entity-render §3.1):
//! instanced rigid models, drawn once per (model, material).
//!
//! A model is a set of *variants* (the tree models hold one mesh per tree variant). All variants
//! of one material go into one vertex buffer, every vertex tagged with its variant; an instance
//! carries the variant it shows and the vertex shader collapses the other variants' vertices. So
//! a forest of twelve tree shapes is one draw call per model, not twelve. Instance data comes in
//! a second vertex buffer with `step_mode: Instance` (WebGL2 has no storage buffers), the base
//! vertex is always 0.
//!
//! The CPU half (instance and vertex layouts, [`build_geometry`]) has no GPU types and is tested
//! on its own.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::{BTreeMap, HashMap};

use asset_format::{Indices, LogicalId, ModelBlob, TextureBlob};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use crate::world::unit::cull::Planes;

use crate::entities::building_family::BuildingFamily;
use crate::entities::unit_family::UnitFamily;
use crate::lighting::LightingEnv;
use crate::renderer::FrameStats;
use crate::texture::{self, UploadOptions};

/// `StaticInstance::cell` of an instance that is not fogged.
pub const NO_CELL: u32 = u32::MAX;

/// Per-instance data, 48 bytes: position (0), rotation about Y in
/// radians (12), uniform scale (16), tint multiplier (20), alpha (32), fog-state cell (36),
/// variant (40), team colour (44).
///
/// `cell` indexes the fog-state texture (cell index of `TerrainSnapshot`), [`NO_CELL`] for none;
/// an instance on an unrevealed cell is not drawn and one on a hidden cell is darkened, like the
/// terrain. `alpha` multiplies the texture alpha before the cutout test.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StaticInstance {
    pub position: [f32; 3],
    pub rotation_y: f32,
    pub scale: f32,
    pub tint: [f32; 3],
    pub alpha: f32,
    pub cell: u32,
    /// Variant of the model to show; taken modulo the model's variant count on upload.
    pub variant: u32,
    /// The colour of the team as `0x01RRGGBB` ([`pack_team`]) for a unit figure; the ported
    /// `UnitShader` reads it as `TeamColorR` (B1's own pipeline ignores it).
    pub team: u32,
}

/// The team colour of an instance: `0x01RRGGBB`, the 1 marking a colour as given.
pub fn pack_team(rgb: [u8; 3]) -> u32 {
    0x0100_0000 | u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2])
}

impl StaticInstance {
    pub const STRIDE: u64 = 48;

    pub fn new(position: [f32; 3], rotation_y: f32, scale: f32) -> Self {
        Self {
            position,
            rotation_y,
            scale,
            tint: [1.0; 3],
            alpha: 1.0,
            cell: NO_CELL,
            variant: 0,
            team: 0,
        }
    }
}

const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    4 => Float32x4,
    5 => Float32x4,
    6 => Float32,
    7 => Uint32,
    8 => Uint32,
];

/// Vertex of a static model, 24 bytes: position (0), normal snorm16 octahedral (12), uv f16 (16),
/// variant (20).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct StaticVertex {
    pub position: [f32; 3],
    pub normal: [i16; 2],
    /// f16 bits.
    pub uv: [u16; 2],
    pub variant: u32,
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Snorm16x2,
    2 => Float16x2,
    3 => Uint32,
];

/// What a model's meshes are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variants {
    /// Every mesh is a variant of its own (tree models).
    PerMesh,
    /// All meshes together are one object (a unit figure with several materials).
    Single,
}

/// The vertices and indices of one material of a model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MaterialGeometry {
    /// Index into `ModelBlob::materials`.
    pub material: usize,
    pub vertices: Vec<StaticVertex>,
    pub indices: Vec<u32>,
}

/// A model split by material, with the triangle count of every variant.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StaticGeometry {
    pub materials: Vec<MaterialGeometry>,
    /// Triangles one instance of variant `i` shows.
    pub variant_triangles: Vec<u32>,
}

impl StaticGeometry {
    pub fn variant_count(&self) -> u32 {
        self.variant_triangles.len() as u32
    }
}

/// Merges the meshes of `blob` by material. Meshes without indices are skipped and take no
/// variant number.
pub fn build_geometry(blob: &ModelBlob, variants: Variants) -> StaticGeometry {
    let mut out = StaticGeometry::default();
    if variants == Variants::Single {
        out.variant_triangles.push(0);
    }
    let mut by_material: BTreeMap<usize, MaterialGeometry> = BTreeMap::new();
    for mesh in blob.meshes.iter().filter(|m| !m.indices.is_empty()) {
        let variant = match variants {
            Variants::PerMesh => {
                out.variant_triangles.push(0);
                out.variant_triangles.len() as u32 - 1
            }
            Variants::Single => 0,
        };
        let triangles = (mesh.indices.len() / 3) as u32;
        out.variant_triangles[variant as usize] += triangles;
        let geometry = by_material
            .entry(usize::from(mesh.material))
            .or_insert_with(|| MaterialGeometry {
                material: usize::from(mesh.material),
                ..MaterialGeometry::default()
            });
        let base = geometry.vertices.len() as u32;
        geometry
            .vertices
            .extend((0..mesh.positions.len()).map(|i| StaticVertex {
                position: mesh.positions[i],
                normal: mesh.normals[i],
                uv: mesh.uvs[i],
                variant,
            }));
        match &mesh.indices {
            Indices::U16(v) => geometry
                .indices
                .extend(v.iter().map(|&i| base + u32::from(i))),
            Indices::U32(v) => geometry.indices.extend(v.iter().map(|&i| base + i)),
        }
    }
    out.materials = by_material.into_values().collect();
    out
}

/// Handle of an uploaded model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StaticModelId(u32);

/// Who owns a set of batches; [`StaticRenderer::replace_set`] replaces one owner's batches only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BatchSet {
    /// Trees of forests and jungles.
    Trees,
    /// Units and cities (entity-render).
    Entities,
}

struct GpuMaterial {
    vertices: Buf,
    indices: Buf,
    index_count: u32,
    bind_group: wgpu::BindGroup,
    /// Group 3 of the ported family (raw textures), for a [`Look::Unit`] or [`Look::Building`]
    /// model.
    family_group: Option<wgpu::BindGroup>,
}

/// How a model is lit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// B1's lambert (`static.wgsl`).
    Plain,
    /// The ported `UnitShader` of the original when the light data has arrived (RC-15); B1's
    /// lambert until then, and for ever without a boot group. Its textures are uploaded raw
    /// (no sRGB), as the ported shader does its own gamma.
    Unit,
    /// The ported `BuildingShader` (WOB1), the same way: the composed city model of B1.
    Building,
}

/// Side of a block of casters, in world units (four hexes of 64).
const CASTER_BLOCK: f32 = 256.0;
/// A batch of fewer instances than this is not put in blocks: the cull would cost more.
const CASTER_MIN: usize = 512;

/// A run of the caster copy of a batch and the box that holds its instances.
#[derive(Clone, Copy, Debug, PartialEq)]
struct CasterBlock {
    first: u32,
    count: u32,
    min: Vec3,
    max: Vec3,
}

/// The instances of a batch in blocks of the map, for the shadow pass: the whole-map forests put
/// 300 000 trees in the light's map, most of them outside what the light's rectangle shows.
/// The batch's own buffer keeps its order (the main pass draws it); this copy is for the casters.
struct Casters {
    instances: Buf,
    blocks: Vec<CasterBlock>,
}

/// `instances` ordered by block (rows of the map, then columns) and the blocks with their boxes.
/// `reach` is how far a vertex of the model goes from its place at scale 1.
fn caster_blocks(
    instances: &[StaticInstance],
    reach: f32,
) -> (Vec<StaticInstance>, Vec<CasterBlock>) {
    let key = |i: &StaticInstance| {
        (
            (i.position[2] / CASTER_BLOCK).floor() as i32,
            (i.position[0] / CASTER_BLOCK).floor() as i32,
        )
    };
    let mut sorted = instances.to_vec();
    sorted.sort_by_key(key);
    let mut blocks: Vec<CasterBlock> = Vec::new();
    let mut last = None;
    for (n, i) in sorted.iter().enumerate() {
        // The reach with room for what the vertex stage moves.
        let r = reach * i.scale.abs() * 1.1 + 1.0;
        let p = Vec3::from(i.position);
        let (lo, hi) = (p - Vec3::splat(r), p + Vec3::splat(r));
        if last == Some(key(i)) {
            let b = blocks.last_mut().expect("a block is open");
            b.count += 1;
            b.min = b.min.min(lo);
            b.max = b.max.max(hi);
        } else {
            last = Some(key(i));
            blocks.push(CasterBlock {
                first: n as u32,
                count: 1,
                min: lo,
                max: hi,
            });
        }
    }
    (sorted, blocks)
}

/// The runs `(first, count)` of the blocks that meet the volume; blocks that follow each other are
/// one run.
fn visible_blocks(blocks: &[CasterBlock], planes: &Planes) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for b in blocks {
        if !planes.sees_box(b.min, b.max) {
            continue;
        }
        match runs.last_mut() {
            Some((first, count)) if *first + *count == b.first => *count += b.count,
            _ => runs.push((b.first, b.count)),
        }
    }
    runs
}

struct GpuStaticModel {
    materials: Vec<GpuMaterial>,
    /// The farthest a vertex is from the origin of the model.
    reach: f32,
    variant_triangles: Vec<u32>,
    /// Alpha-tested (foliage) rather than opaque.
    cutout: bool,
    look: Look,
}

struct Batch {
    instances: Buf,
    count: u32,
    triangles: u32,
    /// For a big batch: its instances in blocks, to cull the shadow casters.
    casters: Option<Casters>,
}

pub(crate) struct StaticTargets {
    pub view_format: wgpu::TextureFormat,
    pub depth_format: wgpu::TextureFormat,
    /// Samples of the main pass (1 or 4).
    pub samples: u32,
}

/// The pipelines, the models and the instance batches.
pub(crate) struct StaticRenderer {
    opaque: wgpu::RenderPipeline,
    cutout: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    models: Vec<GpuStaticModel>,
    batches: BTreeMap<(BatchSet, StaticModelId), Batch>,
    /// The pilot family (RC-15) and the black `SREFMap` of materials without one.
    unit: UnitFamily,
    /// The family of the city model of B1 (WOB1).
    building: BuildingFamily,
    black: wgpu::TextureView,
    object_layout: wgpu::BindGroupLayout,
    fog_layout: wgpu::BindGroupLayout,
}

impl StaticRenderer {
    /// Groups: 0 globals, 1 object (model matrix of the map copy), 2 fog-state texture, 3 material
    /// (diffuse texture and sampler).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        fog_layout: &wgpu::BindGroupLayout,
        targets: StaticTargets,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("static.wgsl"));
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("static material layout"),
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
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("static pipeline layout"),
            bind_group_layouts: &[
                Some(globals_layout),
                Some(object_layout),
                Some(fog_layout),
                Some(&material_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = |label: &str, fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[
                        Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<StaticVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &VERTEX_ATTRIBUTES,
                        }),
                        Some(wgpu::VertexBufferLayout {
                            array_stride: StaticInstance::STRIDE,
                            step_mode: wgpu::VertexStepMode::Instance,
                            attributes: &INSTANCE_ATTRIBUTES,
                        }),
                    ],
                },
                // Double-sided: foliage is two crossed sheets and the winding of converted
                // content is not guaranteed.
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: targets.depth_format,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: crate::frame::multisample(targets.samples),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: targets.view_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("static sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let white = device
            .tex_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("static white 1x1"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255, 255, 255, 255],
                Cluster::Models,
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        let black = device
            .tex_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("static black 1x1"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0, 0, 0, 0],
                Cluster::Models,
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            opaque: pipeline("static pipeline", "fs_opaque"),
            cutout: pipeline("static cutout pipeline", "fs_cutout"),
            material_layout,
            sampler,
            white,
            models: Vec::new(),
            batches: BTreeMap::new(),
            unit: UnitFamily::new(
                device,
                targets.view_format,
                targets.depth_format,
                targets.samples,
            ),
            building: BuildingFamily::new(
                device,
                targets.view_format,
                targets.depth_format,
                targets.samples,
            ),
            black,
            object_layout: object_layout.clone(),
            fog_layout: fog_layout.clone(),
        }
    }

    /// Makes what the ported unit shader needs once the light data is in (`frame_globals` is the
    /// buffer of the frame's constants). Call once per frame; cheap when there is nothing to do.
    pub fn prepare_units(
        &mut self,
        device: &wgpu::Device,
        env: &LightingEnv,
        frame_globals: &wgpu::Buffer,
        reflection_globals: &wgpu::Buffer,
    ) {
        self.unit.prepare(
            device,
            env,
            frame_globals,
            &self.object_layout,
            &self.fog_layout,
        );
        self.building.prepare(
            device,
            env,
            frame_globals,
            reflection_globals,
            &self.object_layout,
            &self.fog_layout,
        );
    }

    /// The ported building shader is drawing cities.
    #[cfg(test)]
    pub fn building_family_on(&self) -> bool {
        self.building.gpu().is_some()
    }

    /// The ported unit shader is drawing units.
    #[cfg(test)]
    pub fn unit_family_on(&self) -> bool {
        self.unit.gpu().is_some()
    }

    /// Uploads a model and the textures its materials reference (a missing one renders white).
    /// `cutout` drops texels with alpha under 0.5 (foliage).
    pub fn create_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
        variants: Variants,
        cutout: bool,
    ) -> StaticModelId {
        self.create_model_as(device, queue, blob, textures, variants, cutout, Look::Plain)
    }

    /// As [`Self::create_model`], with the look of the model. A [`Look::Unit`] model keeps its
    /// textures raw and gets the material group of the ported `UnitShader` (its `SREFMap` is the
    /// `specular` texture of the material, black without one).
    #[allow(clippy::too_many_arguments)]
    pub fn create_model_as(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
        variants: Variants,
        cutout: bool,
        look: Look,
    ) -> StaticModelId {
        let geometry = build_geometry(blob, variants);
        let upload = |id: &LogicalId, what: &str| -> Option<wgpu::TextureView> {
            let Some(t) = textures.get(id) else {
                tracing::warn!(
                    id = id.as_str(),
                    "{what} texture not loaded, using the default"
                );
                return None;
            };
            let up = match look {
                Look::Plain => {
                    texture::upload(device, queue, t, id.as_str()).map(|tex| (tex, None))
                }
                Look::Unit | Look::Building => {
                    texture::upload_blob(device, queue, t, id.as_str(), UploadOptions::WORLD)
                        .map(|u| (u.texture, Some(u.view_dimension)))
                }
            };
            match up {
                Ok((tex, _)) => Some(tex.create_view(&wgpu::TextureViewDescriptor::default())),
                Err(e) => {
                    tracing::warn!(
                        id = id.as_str(),
                        "texture unusable ({e}), using the default"
                    );
                    None
                }
            }
        };
        let materials = geometry
            .materials
            .iter()
            .map(|m| {
                let mat = blob.materials.get(m.material);
                let view = mat
                    .and_then(|mat| mat.diffuse.as_ref())
                    .and_then(|id| upload(id, "diffuse"));
                let sref = (look != Look::Plain)
                    .then(|| {
                        mat.and_then(|mat| mat.specular.as_ref())
                            .and_then(|id| upload(id, "specular"))
                    })
                    .flatten();
                let (diffuse, sref_view) = (
                    view.as_ref().unwrap_or(&self.white),
                    sref.as_ref().unwrap_or(&self.black),
                );
                let family_group = match look {
                    Look::Plain => None,
                    Look::Unit => Some(self.unit.material_group(device, diffuse, sref_view)),
                    Look::Building => {
                        Some(self.building.material_group(device, diffuse, sref_view))
                    }
                };
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("static material"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                view.as_ref().unwrap_or(&self.white),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                GpuMaterial {
                    vertices: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("static vertices"),
                            contents: bytemuck::cast_slice(&m.vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                        Cluster::Models,
                    ),
                    indices: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("static indices"),
                            contents: bytemuck::cast_slice(&m.indices),
                            usage: wgpu::BufferUsages::INDEX,
                        },
                        Cluster::Models,
                    ),
                    index_count: m.indices.len() as u32,
                    bind_group,
                    family_group,
                }
            })
            .collect();
        let reach = geometry
            .materials
            .iter()
            .flat_map(|m| m.vertices.iter())
            .map(|v| Vec3::from(v.position).length())
            .fold(0.0f32, f32::max);
        self.models.push(GpuStaticModel {
            reach,
            materials,
            variant_triangles: geometry.variant_triangles,
            cutout,
            look,
        });
        StaticModelId(self.models.len() as u32 - 1)
    }

    /// Variants the model has (instances take theirs modulo this).
    pub fn variant_count(&self, model: StaticModelId) -> u32 {
        self.models
            .get(model.0 as usize)
            .map_or(0, |m| m.variant_triangles.len() as u32)
    }

    /// Replaces all batches of `set` by one batch per entry. Empty instance lists are skipped.
    pub fn replace_set(
        &mut self,
        device: &wgpu::Device,
        set: BatchSet,
        batches: &[(StaticModelId, &[StaticInstance])],
    ) {
        self.batches.retain(|(s, _), _| *s != set);
        for &(model, instances) in batches {
            let Some(m) = self.models.get(model.0 as usize) else {
                continue;
            };
            if instances.is_empty() || m.variant_triangles.is_empty() {
                continue;
            }
            let triangles = instances
                .iter()
                .map(|i| m.variant_triangles[i.variant as usize % m.variant_triangles.len()])
                .sum();
            let casters = (set == BatchSet::Trees && instances.len() >= CASTER_MIN).then(|| {
                let (sorted, blocks) = caster_blocks(instances, m.reach);
                Casters {
                    instances: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("static caster instances"),
                            contents: bytemuck::cast_slice(&sorted),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                        Cluster::Models,
                    ),
                    blocks,
                }
            });
            self.batches.insert(
                (set, model),
                Batch {
                    casters,
                    instances: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("static instances"),
                            contents: bytemuck::cast_slice(instances),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                        Cluster::Models,
                    ),
                    count: instances.len() as u32,
                    triangles,
                },
            );
        }
    }

    /// Draws the batches of `set` for one map copy (`copy` is its object bind group), as the
    /// subsystem that owns the set does inside its main-pass slot. The globals (group 0) must be
    /// bound; `fog` is the fog-state bind group.
    pub fn draw_set(
        &self,
        set: BatchSet,
        pass: &mut wgpu::RenderPass<'_>,
        globals: &wgpu::BindGroup,
        copy: &wgpu::BindGroup,
        fog: &wgpu::BindGroup,
        stats: &mut FrameStats,
    ) {
        pass.set_bind_group_c(2, fog, &[]);
        pass.set_bind_group_c(1, copy, &[]);
        // Group 0 of the ported unit shader is not B1's: put B1's back before a B1 batch.
        let mut globals_bound = true;
        for ((s, model), batch) in &self.batches {
            if *s != set {
                continue;
            }
            let m = &self.models[model.0 as usize];
            // The ported family of the model, once its light data is in: its pipeline and group 0.
            let unit = match (m.look, self.unit.gpu(), self.building.gpu()) {
                (Look::Unit, Some(gpu), _) => Some((&gpu.pipeline, &gpu.frame_group)),
                (Look::Building, _, Some(gpu)) => Some((&gpu.pipeline, &gpu.frame_group)),
                _ => None,
            };
            match unit {
                Some((pipeline, frame_group)) => {
                    pass.set_pipeline_c(pipeline);
                    pass.set_bind_group_c(0, frame_group, &[]);
                    globals_bound = false;
                }
                None => {
                    pass.set_pipeline_c(if m.cutout { &self.cutout } else { &self.opaque });
                    if !globals_bound {
                        pass.set_bind_group_c(0, globals, &[]);
                        globals_bound = true;
                    }
                }
            }
            pass.set_vertex_buffer(1, batch.instances.slice(..));
            for material in &m.materials {
                let group = match (&material.family_group, unit) {
                    (Some(g), Some(_)) => g,
                    _ => &material.bind_group,
                };
                pass.set_bind_group_c(3, group, &[]);
                pass.set_vertex_buffer(0, material.vertices.slice(..));
                pass.set_index_buffer(material.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed_c(0..material.index_count, 0, 0..batch.count);
                stats.record_instanced(batch.triangles);
            }
        }
        if !globals_bound {
            pass.set_bind_group_c(0, globals, &[]);
        }
    }

    /// Draws the cities of `set` for the reflection pass (`Reflection`): the buildings with the
    /// ported shader, cut at the water plane, under the constants of the reflection camera. `copy`
    /// is the object bind group of the map copy and `fog` the fog-state one. The units of B1 are
    /// not drawn here: `assets-b2` draws the units that reflect.
    pub fn draw_reflection(
        &self,
        set: BatchSet,
        pass: &mut wgpu::RenderPass<'_>,
        copy: &wgpu::BindGroup,
        fog: &wgpu::BindGroup,
        stats: &mut FrameStats,
    ) {
        let Some((pipeline, frame_group)) =
            self.building.gpu().and_then(|gpu| gpu.reflection.as_ref())
        else {
            return;
        };
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(0, frame_group, &[]);
        pass.set_bind_group_c(1, copy, &[]);
        pass.set_bind_group_c(2, fog, &[]);
        for ((s, model), batch) in &self.batches {
            let m = &self.models[model.0 as usize];
            if *s != set || m.look != Look::Building {
                continue;
            }
            pass.set_vertex_buffer(1, batch.instances.slice(..));
            for material in &m.materials {
                let Some(group) = &material.family_group else {
                    continue;
                };
                pass.set_bind_group_c(3, group, &[]);
                pass.set_vertex_buffer(0, material.vertices.slice(..));
                pass.set_index_buffer(material.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed_c(0..material.index_count, 0, 0..batch.count);
                stats.record_instanced(batch.triangles);
            }
        }
    }

    /// Draws every batch for the shadow map (`ShadowMap` pass): the caster pipeline and its
    /// group 0 are bound; this binds the vertex and instance buffers of the `static` pipeline.
    /// With `planes` (the light's rectangle) a big batch draws only the blocks of its instances
    /// that meet it: what is outside cannot land in the map.
    pub fn draw_casters(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        stats: &mut FrameStats,
        planes: Option<&Planes>,
    ) {
        let stride = StaticInstance::STRIDE;
        let mut state = crate::perf::PassState::new();
        for ((_, model), batch) in &self.batches {
            let m = &self.models[model.0 as usize];
            // The runs of instances to draw: the whole batch, or the blocks in view.
            let (buffer, runs) = match (&batch.casters, planes) {
                (Some(c), Some(planes)) => (&c.instances, visible_blocks(&c.blocks, planes)),
                _ => (&batch.instances, vec![(0, batch.count)]),
            };
            for (first, count) in runs {
                state.vertex(
                    pass,
                    1,
                    buffer,
                    u64::from(first) * stride..u64::from(first + count) * stride,
                );
                // The triangles of the instances drawn, by their share of the batch.
                let triangles = (u64::from(batch.triangles) * u64::from(count)
                    / u64::from(batch.count.max(1))) as u32;
                for material in &m.materials {
                    state.vertex(pass, 0, &material.vertices, 0..material.vertices.size());
                    state.index(
                        pass,
                        &material.indices,
                        0..material.indices.size(),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed_c(0..material.index_count, 0, 0..count);
                    stats.record_instanced(triangles);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{Material, Mesh};

    fn mesh(verts: usize, tris: usize, material: u16) -> Mesh {
        Mesh {
            positions: vec![[0.0; 3]; verts],
            normals: vec![[0, 0]; verts],
            uvs: vec![[0, 0]; verts],
            joints: vec![],
            weights: vec![],
            indices: Indices::U16((0..tris * 3).map(|i| (i % verts) as u16).collect()),
            material,
        }
    }

    fn blob(meshes: Vec<Mesh>, materials: usize) -> ModelBlob {
        ModelBlob {
            meshes,
            materials: vec![Material::default(); materials],
            ..ModelBlob::default()
        }
    }

    #[test]
    fn instance_layout_is_48_bytes_and_matches_the_attributes() {
        assert_eq!(std::mem::size_of::<StaticInstance>(), 48);
        assert_eq!(StaticInstance::STRIDE, 48);
        let offsets: Vec<u64> = INSTANCE_ATTRIBUTES.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, [0, 16, 32, 36, 40]);
        let locations: Vec<u32> = INSTANCE_ATTRIBUTES
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(locations, [4, 5, 6, 7, 8]);
        let i = StaticInstance {
            position: [1.0, 2.0, 3.0],
            rotation_y: 4.0,
            scale: 5.0,
            tint: [6.0, 7.0, 8.0],
            alpha: 9.0,
            cell: 10,
            variant: 11,
            team: pack_team([1, 2, 3]),
        };
        let b = bytemuck::bytes_of(&i);
        let f = |at: usize| f32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
        let u = |at: usize| u32::from_ne_bytes(b[at..at + 4].try_into().unwrap());
        assert_eq!([f(0), f(4), f(8), f(12), f(16)], [1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!([f(20), f(24), f(28), f(32)], [6.0, 7.0, 8.0, 9.0]);
        assert_eq!([u(36), u(40), u(44)], [10, 11, 0x0101_0203]);
    }

    #[test]
    fn vertex_layout_is_24_bytes() {
        assert_eq!(std::mem::size_of::<StaticVertex>(), 24);
        let offsets: Vec<u64> = VERTEX_ATTRIBUTES.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, [0, 12, 16, 20]);
    }

    #[test]
    fn new_instance_is_unfogged_and_untinted() {
        let i = StaticInstance::new([0.0; 3], 0.0, 1.0);
        assert_eq!((i.cell, i.alpha, i.tint), (NO_CELL, 1.0, [1.0; 3]));
    }

    #[test]
    fn per_mesh_variants_share_one_material_draw() {
        // Twelve tree variants of 9 vertices and 8 triangles, one material.
        let b = blob((0..12).map(|_| mesh(9, 8, 0)).collect(), 1);
        let g = build_geometry(&b, Variants::PerMesh);
        assert_eq!(g.materials.len(), 1, "one draw per (model, material)");
        assert_eq!(g.variant_count(), 12);
        assert_eq!(g.variant_triangles, vec![8; 12]);
        let m = &g.materials[0];
        assert_eq!((m.vertices.len(), m.indices.len()), (108, 12 * 24));
        // Variant 3 owns vertices 27..36 and indices point into its own range.
        assert!(m.vertices[27..36].iter().all(|v| v.variant == 3));
        assert!(
            m.indices[3 * 24..4 * 24]
                .iter()
                .all(|&i| (27..36).contains(&i))
        );
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
    }

    #[test]
    fn single_variant_groups_meshes_by_material() {
        let b = blob(vec![mesh(4, 2, 1), mesh(3, 1, 0), mesh(4, 2, 1)], 2);
        let g = build_geometry(&b, Variants::Single);
        assert_eq!(g.variant_triangles, vec![5]);
        let ids: Vec<usize> = g.materials.iter().map(|m| m.material).collect();
        assert_eq!(ids, [0, 1]);
        assert_eq!(g.materials[1].vertices.len(), 8);
        assert!(
            g.materials
                .iter()
                .flat_map(|m| &m.vertices)
                .all(|v| v.variant == 0)
        );
    }

    #[test]
    fn meshes_without_indices_take_no_variant() {
        let mut empty = mesh(3, 1, 0);
        empty.indices = Indices::U32(vec![]);
        let b = blob(vec![mesh(3, 1, 0), empty, mesh(3, 1, 0)], 1);
        let g = build_geometry(&b, Variants::PerMesh);
        assert_eq!(g.variant_count(), 2);
    }

    /// The casters of a big batch are in blocks of the map; only the blocks that meet the light's
    /// rectangle are drawn, and every tree that touches it is in one of them.
    #[test]
    fn the_blocks_of_casters_keep_every_tree_that_can_land_in_the_map() {
        use glam::{Mat4, Vec4};
        let trees: Vec<StaticInstance> = (0..100 * 100)
            .map(|k| {
                StaticInstance::new(
                    [(k % 100) as f32 * 20.0, 0.0, (k / 100) as f32 * 20.0],
                    0.0,
                    1.0,
                )
            })
            .collect();
        let (sorted, blocks) = caster_blocks(&trees, 5.0);
        assert_eq!(sorted.len(), trees.len());
        assert_eq!(blocks.iter().map(|b| b.count).sum::<u32>(), 10_000);
        assert!(
            blocks
                .windows(2)
                .all(|w| w[0].first + w[0].count == w[1].first)
        );
        // A top view over x and z in 400..700: the depth is not tested.
        let m = Mat4::from_cols(
            Vec4::new(1.0 / 150.0, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.001, 0.0),
            Vec4::new(0.0, 1.0 / 150.0, 0.0, 0.0),
            Vec4::new(-550.0 / 150.0, -550.0 / 150.0, 0.5, 1.0),
        );
        let planes = Planes::sides(&m);
        let runs = visible_blocks(&blocks, &planes);
        let count: u32 = runs.iter().map(|r| r.1).sum();
        assert!(count < 2_000, "{count} of 10000 trees are drawn");
        let in_runs = |n: usize| {
            runs.iter()
                .any(|&(f, c)| (f as usize..(f + c) as usize).contains(&n))
        };
        for (n, t) in sorted.iter().enumerate() {
            let [x, _, z] = t.position;
            if (400.0..=700.0).contains(&x) && (400.0..=700.0).contains(&z) {
                assert!(
                    in_runs(n),
                    "the tree at {x} {z} is in the map but not drawn"
                );
            }
        }
    }
}
