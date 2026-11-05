//! Skinned/rigid model rendering: vertex format, GPU upload, instances.
//!
//! The bones of an instance live in the bone texture (`skin.rs`, RC-10): a range of slots with one
//! more slot at the end, always identity, that rigid meshes (no joints) are skinned to, so one
//! pipeline draws everything. The vertex shader reads them with `textureLoad` (`skin.wgsl`).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::rc::Rc;

use asset_format::{Indices, LogicalId, Mesh, ModelBlob, Skeleton, TextureBlob};
use glam::Mat4;

use crate::renderer::FrameStats;
use crate::skin::{BONE_TEXTURE_BINDING, BoneTexture, IDENTITY_BONE, pack_bone};
use crate::texture;

/// Bones a model can have; the joint of a vertex is a `u8`, so the rest are clamped. The slot after
/// the last bone holds the identity, and its number is at most 255 too.
pub const MAX_BONES: usize = 255;

/// Bones of the instance of `skeleton`: its own, up to [`MAX_BONES`].
fn bone_count(skeleton: &Skeleton) -> usize {
    skeleton.bones.len().min(MAX_BONES)
}

/// Vertex buffer element, 28 bytes: position (0), normal snorm16 octahedral (12), uv f16 (16),
/// joints u8 (20), weights unorm8 (24).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelVertex {
    pub position: [f32; 3],
    pub normal: [i16; 2],
    /// f16 bits.
    pub uv: [u16; 2],
    pub joints: [u8; 4],
    pub weights: [u8; 4],
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Snorm16x2,
    2 => Float16x2,
    3 => Uint8x4,
    4 => Unorm8x4,
];

pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ModelVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &VERTEX_ATTRIBUTES,
    }
}

/// Packs a mesh into [`ModelVertex`]. Joints beyond [`MAX_BONES`] are clamped; the second value
/// counts the vertices that needed it. `identity` is the joint of the identity bone, which rigid
/// vertices get: the number of bones of the model.
pub fn build_vertices(mesh: &Mesh, identity: u8) -> (Vec<ModelVertex>, usize) {
    let skinned = !mesh.joints.is_empty();
    let mut clamped = 0;
    let vertices = (0..mesh.positions.len())
        .map(|i| {
            let (joints, weights) = if skinned {
                let mut j = mesh.joints[i];
                if j.iter().any(|&b| usize::from(b) >= MAX_BONES) {
                    clamped += 1;
                    j = j.map(|b| b.min((MAX_BONES - 1) as u8));
                }
                (j, mesh.weights[i])
            } else {
                ([identity, 0, 0, 0], [255, 0, 0, 0])
            };
            ModelVertex {
                position: mesh.positions[i],
                normal: mesh.normals[i],
                uv: mesh.uvs[i],
                joints,
                weights,
            }
        })
        .collect();
    (vertices, clamped)
}

/// Packs skin matrices as bones, at most [`MAX_BONES`].
pub fn pack_pose(skin: &[Mat4]) -> Vec<crate::skin::Bone> {
    skin.iter().take(MAX_BONES).map(pack_bone).collect()
}

/// Group 3 of the model shader: the matrix of the instance and where its bones are.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct InstanceUniform {
    model: [[f32; 4]; 4],
    /// Slot of the first bone (`bone_base`).
    bone_base: u32,
    /// Slot of the first bone of the pose to blend to (`bone_base_next`); B1 has none.
    bone_base_next: u32,
    bone_blend: f32,
    pad: u32,
}

impl InstanceUniform {
    fn new(model: Mat4, bone_base: u32) -> Self {
        Self {
            model: model.to_cols_array_2d(),
            bone_base,
            bone_base_next: bone_base,
            bone_blend: 0.0,
            pad: 0,
        }
    }
}

struct GpuModelMesh {
    vertices: Buf,
    indices: Buf,
    index_format: wgpu::IndexFormat,
    index_count: u32,
    /// Index into [`GpuModel::materials`].
    material: usize,
}

/// Immutable GPU data of one model, shared by its instances.
pub struct GpuModel {
    meshes: Vec<GpuModelMesh>,
    materials: Vec<wgpu::BindGroup>,
    skeleton: Skeleton,
}

impl GpuModel {
    pub fn skeleton(&self) -> &Skeleton {
        &self.skeleton
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(u32);

struct GpuInstance {
    model: Rc<GpuModel>,
    transform: Buf,
    bind_group: wgpu::BindGroup,
    /// Its bones and the identity after them; `None` when the bone texture was full, and the
    /// instance is not drawn.
    slots: Option<Range<u32>>,
}

fn bones_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    bones: &BoneTexture,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model bones"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: BONE_TEXTURE_BINDING,
            resource: wgpu::BindingResource::TextureView(bones.view()),
        }],
    })
}

/// Pipeline, shared layouts, the bone texture and all model instances.
pub(crate) struct ModelRenderer {
    pipeline: wgpu::RenderPipeline,
    instance_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    bones_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    white: wgpu::TextureView,
    instances: BTreeMap<InstanceId, GpuInstance>,
    next_id: u32,
    /// Shared by all instances; written through `&self` (`set_pose`) and sent by [`Self::prepare`].
    bones: RefCell<BoneTexture>,
    /// Group 1, with the generation of the texture it was made for.
    bones_group: RefCell<(u64, wgpu::BindGroup)>,
}

impl ModelRenderer {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        globals_layout: &wgpu::BindGroupLayout,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let shader = crate::shaders::create_module(
            device,
            "model",
            concat!(
                include_str!("shaders/common/skin.wgsl"),
                "\n",
                include_str!("model.wgsl")
            ),
        );
        let instance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model instance layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bones_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model bones layout"),
            entries: &[BoneTexture::layout_entry()],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("model material layout"),
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
            label: Some("model pipeline layout"),
            // Group 1 is the bone texture, group 3 the call.
            bind_group_layouts: &[
                Some(globals_layout),
                Some(&bones_layout),
                Some(&material_layout),
                Some(&instance_layout),
            ],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("model pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout())],
            },
            // Double-sided: winding of converted content is not guaranteed yet.
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: crate::frame::multisample(samples),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("model sampler"),
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
                    label: Some("white 1x1"),
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
        let bones = BoneTexture::new(device);
        let bones_group = (
            bones.generation(),
            bones_group(device, &bones_layout, &bones),
        );
        Self {
            pipeline,
            instance_layout,
            material_layout,
            bones_layout,
            sampler,
            white,
            instances: BTreeMap::new(),
            next_id: 0,
            bones: RefCell::new(bones),
            bones_group: RefCell::new(bones_group),
        }
    }

    /// Sends this frame's share of the bone changes and remakes group 1 if the texture grew. Call
    /// once a frame before the passes.
    pub(crate) fn prepare(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut bones = self.bones.borrow_mut();
        bones.flush(device, queue);
        let mut group = self.bones_group.borrow_mut();
        if group.0 != bones.generation() {
            *group = (
                bones.generation(),
                bones_group(device, &self.bones_layout, &bones),
            );
        }
    }

    /// Rows of the bone texture in use and bytes the last frame sent, for `?stats=1`.
    pub(crate) fn bone_stats(&self) -> (u32, usize) {
        let b = self.bones.borrow();
        (b.store().used_rows(), b.uploaded_last())
    }

    fn material_group(&self, device: &wgpu::Device, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// Uploads meshes and the textures its materials reference; a missing or unusable texture
    /// falls back to white.
    pub(crate) fn create_model(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blob: &ModelBlob,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) -> Rc<GpuModel> {
        if blob.skeleton.bones.len() > MAX_BONES {
            tracing::warn!(
                bones = blob.skeleton.bones.len(),
                "model has more bones than a vertex can name ({MAX_BONES}); extra bones are ignored"
            );
        }
        let identity = bone_count(&blob.skeleton) as u8;
        let materials = blob
            .materials
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let texture = m.diffuse.as_ref().and_then(|id| {
                    let Some(t) = textures.get(id) else {
                        tracing::warn!(id = id.as_str(), "diffuse texture not loaded, using white");
                        return None;
                    };
                    match texture::upload(device, queue, t, id.as_str()) {
                        Ok(tex) => Some(tex.create_view(&wgpu::TextureViewDescriptor::default())),
                        Err(e) => {
                            tracing::warn!(id = id.as_str(), "texture unusable ({e}), using white");
                            None
                        }
                    }
                });
                tracing::debug!(material = i, textured = texture.is_some(), "model material");
                self.material_group(device, texture.as_ref().unwrap_or(&self.white))
            })
            .collect();
        let mut clamped_total = 0;
        let meshes = blob
            .meshes
            .iter()
            .filter(|m| !m.indices.is_empty())
            .map(|mesh| {
                let (vertices, clamped) = build_vertices(mesh, identity);
                clamped_total += clamped;
                let (index_bytes, index_format): (&[u8], _) = match &mesh.indices {
                    Indices::U16(v) => (bytemuck::cast_slice(v), wgpu::IndexFormat::Uint16),
                    Indices::U32(v) => (bytemuck::cast_slice(v), wgpu::IndexFormat::Uint32),
                };
                GpuModelMesh {
                    vertices: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("model vertices"),
                            contents: bytemuck::cast_slice(&vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        },
                        Cluster::Models,
                    ),
                    indices: device.buf_init(
                        &wgpu::util::BufferInitDescriptor {
                            label: Some("model indices"),
                            contents: index_bytes,
                            usage: wgpu::BufferUsages::INDEX,
                        },
                        Cluster::Models,
                    ),
                    index_format,
                    index_count: mesh.indices.len() as u32,
                    material: usize::from(mesh.material),
                }
            })
            .collect();
        if clamped_total > 0 {
            tracing::warn!(clamped_total, "vertices reference bones beyond the limit");
        }
        Rc::new(GpuModel {
            meshes,
            materials,
            skeleton: blob.skeleton.clone(),
        })
    }

    pub(crate) fn add_instance(
        &mut self,
        device: &wgpu::Device,
        model: Rc<GpuModel>,
        transform: Mat4,
    ) -> InstanceId {
        // The bones of the model and the identity after them, all identity until a pose comes.
        let count = bone_count(model.skeleton()) + 1;
        let slots = self.bones.borrow_mut().alloc(count as u32);
        match &slots {
            Some(range) => self
                .bones
                .borrow_mut()
                .write(range.clone(), &vec![IDENTITY_BONE; count]),
            None => tracing::warn!(count, "the bone texture is full; the instance is not drawn"),
        }
        let transform_buf = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("model instance"),
                contents: bytemuck::bytes_of(&InstanceUniform::new(
                    transform,
                    slots.as_ref().map_or(0, |r| r.start),
                )),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
            Cluster::Models,
        );
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("model instance"),
            layout: &self.instance_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: transform_buf.as_entire_binding(),
            }],
        });
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.instances.insert(
            id,
            GpuInstance {
                model,
                transform: transform_buf,
                bind_group,
                slots,
            },
        );
        id
    }

    pub(crate) fn set_transform(&self, queue: &wgpu::Queue, id: InstanceId, transform: Mat4) {
        if let Some(i) = self.instances.get(&id) {
            queue.write_buffer(
                &i.transform,
                0,
                bytemuck::bytes_of(&InstanceUniform::new(
                    transform,
                    i.slots.as_ref().map_or(0, |r| r.start),
                )),
            );
        }
    }

    /// Only the bones given are written; the rest, including the identity at the end, stay as
    /// they were.
    pub(crate) fn set_pose(&self, id: InstanceId, skin: &[Mat4]) {
        let Some(i) = self.instances.get(&id) else {
            return;
        };
        let Some(slots) = &i.slots else {
            return;
        };
        let bones = pack_pose(skin);
        let n = bones.len().min(bone_count(i.model.skeleton()));
        self.bones
            .borrow_mut()
            .write(slots.start..slots.start + n as u32, &bones[..n]);
    }

    pub(crate) fn remove_instance(&mut self, id: InstanceId) {
        if let Some(slots) = self.instances.remove(&id).and_then(|i| i.slots) {
            self.bones.borrow_mut().free(slots);
        }
    }

    pub(crate) fn skeleton(&self, id: InstanceId) -> Option<&Skeleton> {
        self.instances.get(&id).map(|i| i.model.skeleton())
    }

    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        globals: &wgpu::BindGroup,
        stats: &mut FrameStats,
    ) {
        if self.instances.is_empty() {
            return;
        }
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(0, globals, &[]);
        pass.set_bind_group_c(1, &self.bones_group.borrow().1, &[]);
        for inst in self.instances.values() {
            if inst.slots.is_none() {
                continue;
            }
            pass.set_bind_group_c(3, &inst.bind_group, &[]);
            for mesh in &inst.model.meshes {
                let Some(material) = inst.model.materials.get(mesh.material) else {
                    continue;
                };
                pass.set_bind_group_c(2, material, &[]);
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), mesh.index_format);
                pass.draw_indexed_c(0..mesh.index_count, 0, 0..1);
                stats.record_draw(mesh.index_count);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic;
    use glam::Vec3;
    use wgpu::util::DeviceExt as _;

    #[test]
    fn vertex_layout_matches_struct() {
        assert_eq!(std::mem::size_of::<ModelVertex>(), 28);
        let layout = vertex_layout();
        assert_eq!(layout.array_stride, 28);
        let offsets: Vec<u64> = layout.attributes.iter().map(|a| a.offset).collect();
        assert_eq!(offsets, [0, 12, 16, 20, 24]);
        let v = ModelVertex {
            position: [1.0, 2.0, 3.0],
            normal: [-4, 5],
            uv: [0x3c00, 0x4000],
            joints: [1, 2, 3, 4],
            weights: [10, 20, 30, 40],
        };
        let bytes = bytemuck::bytes_of(&v);
        assert_eq!(&bytes[0..4], &1.0f32.to_ne_bytes());
        assert_eq!(&bytes[12..14], &(-4i16).to_ne_bytes());
        assert_eq!(&bytes[16..18], &0x3c00u16.to_ne_bytes());
        assert_eq!(&bytes[20..24], &[1, 2, 3, 4]);
        assert_eq!(&bytes[24..28], &[10, 20, 30, 40]);
    }

    #[test]
    fn synthetic_vertices_pack_skinned_and_rigid() {
        let s = synthetic::skinned_cylinder();
        let (tube, clamped) = build_vertices(&s.model.meshes[0], 2);
        assert_eq!(
            (tube.len(), clamped),
            (s.model.meshes[0].positions.len(), 0)
        );
        assert_eq!(tube[0].joints, [0, 1, 0, 0]);
        assert_eq!(tube[0].weights, [255, 0, 0, 0]);
        assert_eq!(tube.last().unwrap().weights, [0, 255, 0, 0]);

        // The rigid pad is skinned to the identity bone after the two bones of the model.
        let (pad, _) = build_vertices(&s.model.meshes[1], 2);
        assert_eq!(pad.len(), 4);
        assert!(
            pad.iter()
                .all(|v| v.joints == [2, 0, 0, 0] && v.weights == [255, 0, 0, 0])
        );
    }

    #[test]
    fn out_of_range_joints_are_clamped() {
        let mut mesh = synthetic::skinned_cylinder().model.meshes.remove(0);
        mesh.joints[0] = [255, 1, 0, 0];
        let (v, clamped) = build_vertices(&mesh, 2);
        assert_eq!(clamped, 1);
        assert!(
            v.iter()
                .flat_map(|v| v.joints)
                .all(|j| usize::from(j) < MAX_BONES)
        );
    }

    /// The model shader (skin.wgsl in front of model.wgsl) is valid and reads the bone texture
    /// where `skin.rs` binds it; the instance block is the 80 bytes of `InstanceUniform`.
    #[test]
    fn the_model_shader_with_skin_wgsl_validates() {
        let text = concat!(
            include_str!("shaders/common/skin.wgsl"),
            "\n",
            include_str!("model.wgsl")
        );
        let parsed = naga::front::wgsl::parse_str(text)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&parsed)
        .unwrap_or_else(|e| panic!("{}", e.emit_to_string(text)));
        let tex = parsed
            .global_variables
            .iter()
            .find(|(_, g)| g.name.as_deref() == Some("bone_tex"))
            .expect("bone_tex")
            .1;
        let b = tex.binding.expect("bound");
        assert_eq!((b.group, b.binding), (1, BONE_TEXTURE_BINDING));
        let (_, inst) = parsed
            .types
            .iter()
            .find(|(_, t)| t.name.as_deref() == Some("Instance"))
            .expect("Instance");
        let naga::TypeInner::Struct { span, .. } = &inst.inner else {
            panic!("a struct");
        };
        assert_eq!(*span as usize, std::mem::size_of::<InstanceUniform>());
    }

    /// Draws the synthetic model through the bone texture on a native device: the layouts fit the
    /// shader, and a pose written with `set_pose` moves what is drawn.
    #[test]
    fn a_pose_in_the_bone_texture_moves_the_drawn_model() {
        use crate::gpu_test::{gpu, read_texture};
        let Some(g) = gpu(
            "model bones",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let (device, queue) = (&g.device, &g.queue);
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        // Group 0 as B1 has it: view_proj, camera, light, params (112 bytes).
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("test globals"),
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
        // x in -1.5..1.5, y in -0.2..2.2 to the clip square; z 0.5 + 0.05 z.
        let view_proj = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.5))
            * Mat4::from_scale(Vec3::new(1.0 / 1.5, 1.0 / 1.2, 0.05))
            * Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0));
        let mut globals = [0.0f32; 28];
        globals[..16].copy_from_slice(&view_proj.to_cols_array());
        globals[20..24].copy_from_slice(&[0.0, 0.0, 1.0, 0.0]);
        let globals_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("test globals"),
            contents: bytemuck::cast_slice(&globals),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let globals_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("test globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buf.as_entire_binding(),
            }],
        });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let depth_format = wgpu::TextureFormat::Depth24PlusStencil8;
        let mut models =
            ModelRenderer::new(device, queue, &globals_layout, format, depth_format, 1);
        let s = synthetic::skinned_cylinder();
        let gpu_model = models.create_model(device, queue, &s.model, &s.textures);
        let id = models.add_instance(device, gpu_model, Mat4::IDENTITY);

        let size = 32u32;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test target"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test depth"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: depth_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let draw = |models: &ModelRenderer| {
            models.prepare(device, queue);
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("test"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.create_view(&Default::default()),
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth.create_view(&Default::default()),
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                let mut stats = FrameStats::default();
                models.draw(&mut pass, &globals_group, &mut stats);
                assert_eq!(stats.draws, 2, "the tube and the pad");
            }
            queue.submit([encoder.finish()]);
            read_texture(&g, &target, 0, 0, (size, size), 4)
        };
        let rest = draw(&models);
        let lit = |img: &[u8]| img.chunks_exact(4).filter(|p| p[..3] != [0, 0, 0]).count();
        assert!(lit(&rest) > 20, "the model is drawn: {} pixels", lit(&rest));
        // Bone 1 moves a unit sideways: the top of the tube follows.
        models.set_pose(
            id,
            &[
                Mat4::IDENTITY,
                Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            ],
        );
        let bent = draw(&models);
        assert_ne!(rest, bent, "the pose reached the vertex shader");
        assert_eq!(models.bone_stats().0, 1, "one row of the texture is in use");
        let error = crate::gpu_test::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
        // The instance gives its bones back.
        models.remove_instance(id);
        assert_eq!(models.bones.borrow().store().used_slots(), 0);
    }

    #[test]
    fn a_pose_packs_at_most_max_bones() {
        let skin = vec![Mat4::from_translation(Vec3::X); 300];
        let bones = pack_pose(&skin);
        assert_eq!(bones.len(), MAX_BONES);
        assert_eq!(bones[0], pack_bone(&skin[0]));
        assert_eq!(std::mem::size_of::<InstanceUniform>(), 80);
        // The joint of the identity bone of the biggest model still fits a `u8`.
        assert_eq!(MAX_BONES, usize::from(u8::MAX));
    }

    #[test]
    fn synthetic_pose_follows_animation() {
        let s = synthetic::skinned_cylinder();
        let mut ev = crate::PoseEvaluator::new();
        let skin = ev
            .evaluate(&s.model.skeleton, Some(&s.animations[0]), 0.5, true)
            .to_vec();
        let bones = pack_pose(&skin);
        assert_eq!(bones.len(), 2);
        // Tube top vertex is fully weighted to bone 1: its skinned position moves when bent.
        let top = Vec3::from(s.model.meshes[0].positions.last().copied().unwrap());
        let b = &bones[1];
        let moved = Vec3::new(
            b[0] * top.x + b[1] * top.y + b[2] * top.z + b[3],
            b[4] * top.x + b[5] * top.y + b[6] * top.z + b[7],
            b[8] * top.x + b[9] * top.y + b[10] * top.z + b[11],
        );
        assert!(moved.distance(top) > 0.05, "{moved:?} vs {top:?}");
    }
}
