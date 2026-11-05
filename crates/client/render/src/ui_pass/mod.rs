//! Drawing of the original UI's [`UiDrawList`] without egui (B4 `ui-render`).
//!
//! `prepare` uploads the geometry, resolves the textures of the commands and builds the draw
//! operations; `draw` replays them into the open pass. Commands are drawn in list order, never
//! reordered (the exe's `SortDrawCalls` is only a batching step that keeps overlapping calls in
//! tree order). A command whose texture is not loaded is skipped for the frame.

mod blend;
mod clip;
#[cfg(test)]
mod tests;
mod textures;

use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use draw_list::{TextureId, UiDrawList, UiSampler, UiShade, UiVertex};

use crate::overlay::MinimapInfo;
use crate::perf::gpu_mem::{Buf, Cluster, DeviceTrack};

pub use textures::{UiTextureData, UiTextureError, UiTextures};

/// Where the pass draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiTarget {
    /// Format of the colour view; an sRGB one makes the shader linearise its output.
    pub format: wgpu::TextureFormat,
    /// Physical pixels.
    pub size: (u32, u32),
}

/// Textures of the renderer that commands may name by an external id.
#[derive(Clone, Copy, Default)]
pub struct UiExternal<'a> {
    /// `TextureId::MINIMAP`; `None` while the minimap target does not exist or is switched off.
    pub minimap: Option<(&'a wgpu::TextureView, MinimapInfo)>,
}

/// The `ui.*` counters of one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiStats {
    pub cmds: u64,
    pub quads: u64,
    pub verts: u64,
    pub cmds_drawn: u64,
    pub cmds_skipped: u64,
    pub tex_bound: u64,
    pub scissor_sets: u64,
    pub pipeline_sets: u64,
    /// Geometry and new textures written to the GPU in the frame.
    pub upload_bytes: u64,
    pub tex_resident_mb: u64,
    pub tex_uploads: u64,
    pub tex_evictions: u64,
    /// Commands skipped for a texture that is not loaded.
    pub tex_missing: u64,
    /// Commands skipped for an external texture the renderer does not have.
    pub handle_missing: u64,
}

impl UiStats {
    /// Names as `__civ5stats.counters` and the stats table show them.
    pub fn counters(&self) -> [(&'static str, u64); 14] {
        [
            ("ui.cmds", self.cmds),
            ("ui.quads", self.quads),
            ("ui.verts", self.verts),
            ("ui.cmds_drawn", self.cmds_drawn),
            ("ui.cmds_skipped", self.cmds_skipped),
            ("ui.tex_bound", self.tex_bound),
            ("ui.scissor_sets", self.scissor_sets),
            ("ui.pipeline_sets", self.pipeline_sets),
            ("ui.upload_bytes", self.upload_bytes),
            ("ui.tex_resident_mb", self.tex_resident_mb),
            ("ui.tex_uploads", self.tex_uploads),
            ("ui.tex_evictions", self.tex_evictions),
            ("ui.tex_missing", self.tex_missing),
            ("ui.handle_missing", self.handle_missing),
        ]
    }
}

/// Times of the UI passes in ms, from the frame's pass table: `ui.cpu_ms`, `ui.gpu_ms` and
/// `ui.overlay_ms`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct UiTimes {
    /// CPU time of `prepare` and of recording the `UiGame` pass.
    pub cpu_ms: f32,
    /// GPU time of the `UiGame` pass; `None` when the device gives no timestamps.
    pub gpu_ms: Option<f32>,
    /// The egui pass (B3 panel, debug windows), shown as the table shows it.
    pub overlay_ms: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    to_ndc: [f32; 2],
    target_srgb: f32,
    pad: f32,
}

/// Frames a cached bind group may stay unused before it is dropped (and with it a hold on an
/// evicted texture).
const GROUP_AGE: u64 = crate::ui_pass::textures::PROTECTED_FRAMES;
const MIN_BUFFER: u64 = 64 * 1024;
/// Bytes of the per-command uniform (`Cmd` of `ui.wgsl`): the mask transform and the UV scale.
const CMD_BYTES: u64 = 32;
/// The params of a command that needs none: no mask, the whole texture.
const IDENTITY: CmdParams = CmdParams {
    mask_xform: [1.0, 1.0, 0.0, 0.0],
    uv_scale: [1.0, 1.0, 0.0, 0.0],
};

/// `Cmd` of `ui.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct CmdParams {
    /// Mask UV from the vertex position: scale xy, offset zw; already scaled by the mask's own UV
    /// scale.
    mask_xform: [f32; 4],
    /// xy: the scale of the texture's UV (a texture padded to whole blocks,
    /// `UiTextureData::content`); zw unused.
    uv_scale: [f32; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Source {
    id: u64,
    serial: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GroupKey {
    tex: Source,
    tex2: Source,
    point: bool,
}

struct Cached {
    group: wgpu::BindGroup,
    last_used: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Op {
    pipeline: u8,
    group: u32,
    param_offset: u32,
    scissor: [u32; 4],
    first_index: u32,
    index_count: u32,
}

struct Growing {
    buffer: Buf,
    capacity: u64,
}

impl Growing {
    fn new(device: &wgpu::Device, label: &str, capacity: u64, usage: wgpu::BufferUsages) -> Self {
        Self {
            buffer: device.buf(
                &wgpu::BufferDescriptor {
                    label: Some(label),
                    size: capacity,
                    usage: usage | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Ui,
            ),
            capacity,
        }
    }

    /// Writes `bytes`; grows by 1.5x steps when they do not fit. True when the buffer was made again.
    fn write(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        label: &str,
        usage: wgpu::BufferUsages,
        bytes: &[u8],
    ) -> bool {
        let need = bytes.len() as u64;
        let mut made = false;
        if need > self.capacity {
            let mut cap = self.capacity.max(MIN_BUFFER);
            while cap < need {
                cap = (cap + cap / 2).next_multiple_of(4);
            }
            *self = Self::new(device, label, cap, usage);
            made = true;
        }
        if !bytes.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytes);
        }
        made
    }
}

pub struct UiPass {
    /// `[blend slot * 2 + masked]`.
    pipelines: Vec<wgpu::RenderPipeline>,
    frame_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    samplers: [wgpu::Sampler; 2],
    frame_uniform: Buf,
    vertices: Growing,
    indices: Growing,
    params: Growing,
    frame_group: wgpu::BindGroup,
    param_align: u64,
    /// Slot 0 of `params` holds the identity (see `upload_params`).
    identity_written: bool,
    cache: HashMap<GroupKey, Cached>,
    /// Per frame: bind groups in use, indexed by `Op::group`.
    groups: Vec<wgpu::BindGroup>,
    group_of: HashMap<GroupKey, u32>,
    ids_seen: HashSet<u64>,
    ops: Vec<Op>,
    param_bytes: Vec<u8>,
    /// Revision and sizes of the list in the GPU buffers.
    uploaded: Option<(u64, usize, usize)>,
    frame: u64,
    size: (u32, u32),
    stats: UiStats,
}

impl UiPass {
    /// Makes every pipeline up front (4 blend slots x plain and masked) for a target of `format`.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::include_wgsl!("ui.wgsl"));
        let uniform_entry = |binding: u32, dynamic: bool, size: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui frame layout"),
            entries: &[
                uniform_entry(0, false, 16),
                uniform_entry(1, true, CMD_BYTES),
            ],
        });
        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui texture layout"),
            entries: &[
                tex_entry(0),
                tex_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui pipeline layout"),
            bind_group_layouts: &[Some(&frame_layout), Some(&tex_layout)],
            immediate_size: 0,
        });
        let mut pipelines = Vec::new();
        for slot in blend::SLOTS {
            for masked in [false, true] {
                pipelines.push(
                    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("ui pipeline"),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some("vs"),
                            compilation_options: Default::default(),
                            buffers: &[Some(wgpu::VertexBufferLayout {
                                array_stride: UiVertex::STRIDE,
                                step_mode: wgpu::VertexStepMode::Vertex,
                                attributes: &wgpu::vertex_attr_array![
                                    0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4
                                ],
                            })],
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some(if masked { "fs_masked" } else { "fs_plain" }),
                            compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState {
                                format,
                                blend: Some(blend::blend_state(slot)),
                                write_mask: wgpu::ColorWrites::ALL,
                            })],
                        }),
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            cull_mode: None,
                            ..Default::default()
                        },
                        depth_stencil: None,
                        multisample: wgpu::MultisampleState::default(),
                        multiview_mask: None,
                        cache: None,
                    }),
                );
            }
        }
        let sampler = |filter: wgpu::FilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("ui sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };
        let param_align =
            u64::from(device.limits().min_uniform_buffer_offset_alignment).max(CMD_BYTES);
        let frame_uniform = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("ui frame"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Ui,
        );
        let params = Growing::new(
            device,
            "ui params",
            param_align * 16,
            wgpu::BufferUsages::UNIFORM,
        );
        let frame_group = Self::make_frame_group(device, &frame_layout, &frame_uniform, &params);
        Self {
            pipelines,
            frame_layout,
            tex_layout,
            samplers: [
                sampler(wgpu::FilterMode::Linear),
                sampler(wgpu::FilterMode::Nearest),
            ],
            frame_uniform,
            vertices: Growing::new(
                device,
                "ui vertices",
                MIN_BUFFER,
                wgpu::BufferUsages::VERTEX,
            ),
            indices: Growing::new(device, "ui indices", MIN_BUFFER, wgpu::BufferUsages::INDEX),
            params,
            frame_group,
            param_align,
            identity_written: false,
            cache: HashMap::new(),
            groups: Vec::new(),
            group_of: HashMap::new(),
            ids_seen: HashSet::new(),
            ops: Vec::new(),
            param_bytes: Vec::new(),
            uploaded: None,
            frame: 0,
            size: (0, 0),
            stats: UiStats::default(),
        }
    }

    fn make_frame_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        frame: &wgpu::Buffer,
        params: &Growing,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui frame group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params.buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(CMD_BYTES),
                    }),
                },
            ],
        })
    }

    /// The counters of the last `prepare`.
    pub fn stats(&self) -> UiStats {
        self.stats
    }

    /// True when `draw` has something to record; the pass is not opened otherwise.
    pub fn has_work(&self) -> bool {
        !self.ops.is_empty()
    }

    /// Uploads the list and builds the draw operations for this frame. Call before the pass opens.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        list: &UiDrawList,
        textures: &mut UiTextures,
        external: &UiExternal<'_>,
        target: &UiTarget,
    ) {
        self.frame += 1;
        self.size = target.size;
        self.ops.clear();
        self.groups.clear();
        self.group_of.clear();
        self.ids_seen.clear();
        self.param_bytes.clear();
        textures.begin_frame();
        let mut stats = UiStats {
            cmds: list.commands().len() as u64,
            quads: list.indices().len() as u64 / 6,
            verts: list.vertices().len() as u64,
            ..UiStats::default()
        };
        let mut upload_bytes = 0u64;

        if !list.is_empty() {
            upload_bytes += self.upload_geometry(device, queue, list);
            let uniform = FrameUniform {
                to_ndc: [
                    2.0 / target.size.0.max(1) as f32,
                    2.0 / target.size.1.max(1) as f32,
                ],
                // SIMPLIFIED(SIMP-uir-4): the exe blends in gamma space. A surface without an sRGB
                // format does the same; an sRGB-only one gets the colour linearised and blends in
                // linear space, so translucent edges differ a little.
                target_srgb: if target.format.is_srgb() { 1.0 } else { 0.0 },
                pad: 0.0,
            };
            queue.write_buffer(&self.frame_uniform, 0, bytemuck::bytes_of(&uniform));
            self.build_ops(device, list, textures, external, &mut stats);
            upload_bytes += self.upload_params(device, queue);
            self.prune_cache();
        }

        let t = textures.take_frame();
        stats.upload_bytes = upload_bytes + t.upload_bytes;
        stats.tex_uploads = t.uploads;
        stats.tex_evictions = t.evictions;
        stats.tex_resident_mb = t.resident_bytes >> 20;
        stats.tex_bound = self.ids_seen.len() as u64;
        (stats.scissor_sets, stats.pipeline_sets) = self.state_changes();
        self.stats = stats;
    }

    fn upload_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        list: &UiDrawList,
    ) -> u64 {
        let key = (list.revision(), list.vertices().len(), list.indices().len());
        if self.uploaded == Some(key) {
            return 0;
        }
        let vertices: &[u8] = bytemuck::cast_slice(list.vertices());
        let indices: &[u8] = bytemuck::cast_slice(list.indices());
        self.vertices.write(
            device,
            queue,
            "ui vertices",
            wgpu::BufferUsages::VERTEX,
            vertices,
        );
        self.indices.write(
            device,
            queue,
            "ui indices",
            wgpu::BufferUsages::INDEX,
            indices,
        );
        self.uploaded = Some(key);
        (vertices.len() + indices.len()) as u64
    }

    fn upload_params(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> u64 {
        // Slot 0 is the identity every other plain command reads; it never changes, so it is
        // written once per buffer and not counted.
        if self.param_bytes.len() as u64 <= self.param_align {
            if !self.identity_written {
                queue.write_buffer(&self.params.buffer, 0, &self.param_bytes);
                self.identity_written = true;
            }
            return 0;
        }
        let made = self.params.write(
            device,
            queue,
            "ui params",
            wgpu::BufferUsages::UNIFORM,
            &self.param_bytes,
        );
        self.identity_written = true;
        if made {
            self.frame_group = Self::make_frame_group(
                device,
                &self.frame_layout,
                &self.frame_uniform,
                &self.params,
            );
        }
        self.param_bytes.len() as u64
    }

    /// Source of `id` for this frame: the serial that keys bind groups. `None` is counted as missing.
    fn resolve(
        textures: &mut UiTextures,
        external: &UiExternal<'_>,
        id: TextureId,
        stats: &mut UiStats,
    ) -> Option<Source> {
        if id.is_external() {
            return match (id, external.minimap) {
                (TextureId::MINIMAP, Some((_, info))) => Some(Source {
                    id: id.0,
                    // Generation 0 is a valid one; keep it apart from serial 0 (white).
                    serial: info.generation + 1,
                }),
                _ => {
                    stats.handle_missing += 1;
                    None
                }
            };
        }
        match textures.touch(id) {
            Some(serial) => Some(Source { id: id.0, serial }),
            None => {
                stats.tex_missing += 1;
                None
            }
        }
    }

    fn build_ops(
        &mut self,
        device: &wgpu::Device,
        list: &UiDrawList,
        textures: &mut UiTextures,
        external: &UiExternal<'_>,
        stats: &mut UiStats,
    ) {
        // Slot 0 of the params is the identity every plain command of a whole texture points at.
        push_params(&mut self.param_bytes, IDENTITY, self.param_align);
        let mut last: Option<(GroupKey, u32)> = None;
        for cmd in list.commands() {
            if cmd.index_count == 0 {
                stats.cmds_skipped += 1;
                continue;
            }
            let Some(scissor) = clip::scissor(cmd.clip, self.size) else {
                stats.cmds_skipped += 1;
                continue;
            };
            // SIMPLIFIED(SIMP-uir-2): the palette branch is off until the exe's shader formula is
            // known; a palette pair is drawn as its first texture, plain.
            let masked = cmd.shade == UiShade::Masked;
            let tex = Self::resolve(textures, external, cmd.texture, stats);
            let tex2 = if masked {
                Self::resolve(textures, external, cmd.texture2, stats)
            } else {
                Some(Source { id: 0, serial: 0 })
            };
            let (Some(tex), Some(tex2)) = (tex, tex2) else {
                stats.cmds_skipped += 1;
                continue;
            };
            let key = GroupKey {
                tex,
                tex2,
                point: cmd.sampler == UiSampler::Point,
            };
            let group = match last {
                Some((k, g)) if k == key => g,
                _ => match self.group_of.get(&key) {
                    Some(&g) => g,
                    None => {
                        let group = self.bind_group(
                            device,
                            textures,
                            external,
                            key,
                            cmd.texture,
                            cmd.texture2,
                        );
                        let Some(group) = group else {
                            stats.cmds_skipped += 1;
                            continue;
                        };
                        self.ids_seen.insert(tex.id);
                        if masked {
                            self.ids_seen.insert(tex2.id);
                        }
                        let g = self.groups.len() as u32;
                        self.groups.push(group);
                        self.group_of.insert(key, g);
                        g
                    }
                },
            };
            last = Some((key, group));
            let [su, sv] = textures.uv_scale(cmd.texture);
            let params = if masked {
                // The mask's UV is scaled by its own padding: folded into the transform.
                let [mu, mv] = textures.uv_scale(cmd.texture2);
                let x = cmd.tex2_xform;
                CmdParams {
                    mask_xform: [x[0] * mu, x[1] * mv, x[2] * mu, x[3] * mv],
                    uv_scale: [su, sv, 0.0, 0.0],
                }
            } else {
                CmdParams {
                    uv_scale: [su, sv, 0.0, 0.0],
                    ..IDENTITY
                }
            };
            let param_offset = if params == IDENTITY {
                0
            } else {
                let at = self.param_bytes.len();
                push_params(&mut self.param_bytes, params, self.param_align);
                at as u32
            };
            self.ops.push(Op {
                pipeline: cmd.blend.slot() * 2 + u8::from(masked),
                group,
                param_offset,
                scissor,
                first_index: cmd.first_index,
                index_count: cmd.index_count,
            });
            stats.cmds_drawn += 1;
        }
    }

    /// The bind group for `key`, from the cache or made now.
    fn bind_group(
        &mut self,
        device: &wgpu::Device,
        textures: &UiTextures,
        external: &UiExternal<'_>,
        key: GroupKey,
        id1: TextureId,
        id2: TextureId,
    ) -> Option<wgpu::BindGroup> {
        if let Some(c) = self.cache.get_mut(&key) {
            c.last_used = self.frame;
            return Some(c.group.clone());
        }
        let view = |id: TextureId| -> Option<&wgpu::TextureView> {
            if id.is_external() {
                external
                    .minimap
                    .map(|(v, _)| v)
                    .filter(|_| id == TextureId::MINIMAP)
            } else {
                textures.view(id)
            }
        };
        let v1 = view(id1)?;
        // A plain command binds the white texture in the second slot.
        let v2 = if key.tex2.id == 0 {
            textures.view(TextureId::WHITE)?
        } else {
            view(id2)?
        };
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ui texture group"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(v1),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(v2),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(
                        &self.samplers[usize::from(key.point)],
                    ),
                },
            ],
        });
        self.cache.insert(
            key,
            Cached {
                group: group.clone(),
                last_used: self.frame,
            },
        );
        Some(group)
    }

    fn prune_cache(&mut self) {
        let frame = self.frame;
        self.cache
            .retain(|_, c| frame.saturating_sub(c.last_used) <= GROUP_AGE);
    }

    /// Scissor and pipeline changes the ops need, counted the way `draw` replays them.
    fn state_changes(&self) -> (u64, u64) {
        let mut scissor = [0, 0, self.size.0, self.size.1];
        let mut pipeline = None;
        let (mut s, mut p) = (0, 0);
        for op in &self.ops {
            if op.scissor != scissor {
                scissor = op.scissor;
                s += 1;
            }
            if pipeline != Some(op.pipeline) {
                pipeline = Some(op.pipeline);
                p += 1;
            }
        }
        (s, p)
    }

    /// Records the frame's operations into `pass`, which loads the colour and has no depth.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        if self.ops.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertices.buffer.slice(..));
        pass.set_index_buffer(self.indices.buffer.slice(..), wgpu::IndexFormat::Uint32);
        let mut scissor = [0, 0, self.size.0, self.size.1];
        let mut pipeline = None;
        let mut group = None;
        let mut offset = None;
        for op in &self.ops {
            if pipeline != Some(op.pipeline) {
                pass.set_pipeline(&self.pipelines[usize::from(op.pipeline)]);
                pipeline = Some(op.pipeline);
            }
            if offset != Some(op.param_offset) {
                pass.set_bind_group(0, &self.frame_group, &[op.param_offset]);
                offset = Some(op.param_offset);
            }
            if group != Some(op.group) {
                pass.set_bind_group(1, &self.groups[op.group as usize], &[]);
                group = Some(op.group);
            }
            if op.scissor != scissor {
                let [x, y, w, h] = op.scissor;
                pass.set_scissor_rect(x, y, w, h);
                scissor = op.scissor;
            }
            pass.draw_indexed(op.first_index..op.first_index + op.index_count, 0, 0..1);
        }
    }

    /// Draw calls and triangles of the last `prepare`.
    pub fn draw_totals(&self) -> (u32, u32) {
        (
            self.ops.len() as u32,
            self.ops.iter().map(|o| o.index_count / 3).sum(),
        )
    }
}

/// Appends one `align`-sized params slot holding `params`.
fn push_params(out: &mut Vec<u8>, params: CmdParams, align: u64) {
    let at = out.len();
    out.resize(at + align as usize, 0);
    out[at..at + CMD_BYTES as usize].copy_from_slice(bytemuck::bytes_of(&params));
}
