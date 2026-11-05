//! "Decal Layer" in the bake: the decals of the area of a cell, drawn into
//! the albedo and the height target of the page after "HeightMap" and before "Normal Calculation".
//! `DecalVSCompressed` + `DecalPS` (`shaders/terrain/decal.wgsl`).
//!
//! A vertex of a decal goes to the shader as a place in the bounds of the decal (0 to 1) and a texture
//! coordinate; `DecalTransform` takes a texel of the field (the bounds are in texels) to the clip space of
//! the page. The shader adds one to the place before the transform, which the matrix takes off again.
//! The decals are drawn in the order of their priority, the lowest first; there is no depth buffer.
//!
//! The blend of the two targets is state 8 of the command sets (`TerrainCell.cpp:922`), the state the
//! draws of the FOW base layer and of the minimap have in the captures (S2-bake, resource 350): the
//! colour `SrcAlpha, InvSrcAlpha`, the alpha `One, One`, in both targets. (S2-bake has no draw of the
//! bake itself: the pages were in the cache when it was taken.)

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::HashMap;

use asset_format::LogicalId;
use glam::{Mat4, Vec2, Vec4};

use super::cells::CellBox;
use super::decals::{DecalData, DecalLayer, PASS_LAYER, PASS_SHADOW};
use super::sampler::TerrainFrame;
use super::shadow_pass::SHADOW_FORMAT;
use crate::lighting::row_major;
use crate::shaders::{catalog, create_module};

/// Bytes of a vertex: the place in the bounds and the texture coordinate, two `vec4`.
const VERTEX_STRIDE: u64 = 32;
/// Bytes of `DecalVsConsts` (std140: 25 floats rounded up to a multiple of 16 bytes).
const CONSTS_BYTES: usize = 112;

/// What the pass reads of the world: the layer, and the textures of the decals on the GPU.
pub struct DecalBake<'a> {
    pub layer: &'a DecalLayer,
    pub textures: &'a HashMap<LogicalId, wgpu::TextureView>,
}

/// The pipeline and the resources of the pass.
pub struct DecalPass {
    pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    layout1: wgpu::BindGroupLayout,
    layout2: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The sampler of the decals whose texture coordinates leave 0..1: the texture repeats.
    repeat: wgpu::Sampler,
    /// 1x1 grey: the height of a decal that has none; `DecalPS` makes no height of it (alpha 0).
    neutral: wgpu::TextureView,
    /// The neutral texture was cleared to grey in the commands of the pass.
    neutral_ready: std::cell::Cell<bool>,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// The matrix that takes `(x + 1, y + 1)` for a texel `(x, y)` of the field to the clip space of the
/// page of a cell: the page covers the texels `rect.x - 1 ..= rect.x + rect.w`, edge to edge.
pub fn page_transform(rect: &CellBox) -> Mat4 {
    let (w, h) = (rect.w as f32 + 1.0, rect.h as f32 + 1.0);
    Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -2.0 / h, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(
            -2.0 * rect.x as f32 / w - 1.0,
            2.0 * rect.y as f32 / h + 1.0,
            0.5,
            1.0,
        ),
    )
}

/// The world rectangle (our basis) of the texels a page covers.
fn page_world_rect(frame: &TerrainFrame, rect: &CellBox) -> (Vec2, Vec2) {
    let a = frame.tex_to_world(Vec2::new(rect.x as f32 - 1.0, rect.y as f32 - 1.0));
    let b = frame.tex_to_world(Vec2::new(
        (rect.x + rect.w) as f32,
        (rect.y + rect.h) as f32,
    ));
    (a.min(b), a.max(b))
}

/// One decal to draw: the decal and the move of the copy of the map it is seen at (world units).
pub struct DecalDraw<'a> {
    pub data: &'a DecalData,
    pub shift: f32,
}

/// The decals of the area of a page, in drawing order, for the copies of the map that reach it.
pub fn decals_of_page<'a>(
    layer: &'a DecalLayer,
    frame: &TerrainFrame,
    rect: &CellBox,
    pass: u32,
) -> Vec<DecalDraw<'a>> {
    let (min, max) = page_world_rect(frame, rect);
    let shifts: Vec<f32> = if frame.wrap_x() {
        let w = frame.world_width();
        vec![0.0, w, -w]
    } else {
        vec![0.0]
    };
    let mut out: Vec<DecalDraw<'a>> = Vec::new();
    for shift in shifts {
        let s = Vec2::new(shift, 0.0);
        for data in layer.in_area(min + s, max + s, pass) {
            out.push(DecalDraw { data, shift });
        }
    }
    out.sort_by(|a, b| {
        a.data
            .priority
            .total_cmp(&b.data.priority)
            .then(a.data.handle.cmp(&b.data.handle))
            .then(a.shift.total_cmp(&b.shift))
    });
    out
}

impl DecalPass {
    pub fn new(device: &wgpu::Device, target: wgpu::TextureFormat) -> Self {
        let m = catalog()
            .iter()
            .find(|m| m.name == "terrain_decal")
            .expect("the module terrain_decal is in the catalog");
        let module = create_module(device, "terrain_decal", &m.source());
        let layout1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain decal textures"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                sampler_entry(3),
            ],
        });
        let layout2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain decal vs"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain decal"),
            bind_group_layouts: &[None, Some(&layout1), Some(&layout2)],
            immediate_size: 0,
        });
        let attributes = [0u32, 1].map(|i| wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: u64::from(i) * 16,
            shader_location: i,
        });
        let over = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let target_state = wgpu::ColorTargetState {
            format: target,
            blend: Some(over),
            write_mask: wgpu::ColorWrites::ALL,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Decal Layer"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("decal_vs_compressed"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("decal_ps"),
                compilation_options: Default::default(),
                targets: &[Some(target_state.clone()), Some(target_state)],
            }),
            multiview_mask: None,
            cache: None,
        });
        // "Decal Macro Shadow": the albedo of the decal added to the macro shadow, one target. The pass
        // has the blend state of "Macro Shadow" (state 4, whose draws add up), not that of "Decal Layer"
        // (state 8, the alpha blend of the FOW base layer and the minimap); the `Tree_Shadow` textures
        // are a white shadow on black with an alpha of 1, so only an addition keeps the ground around
        // the shadow as it was.
        // re: TerrainCell.cpp:922, :941, :977 (the second argument of `BeginCommandSet`: 8, 4, 4)
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let shadow_state = wgpu::ColorTargetState {
            format: SHADOW_FORMAT,
            blend: Some(wgpu::BlendState {
                color: add,
                alpha: add,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        };
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Decal Macro Shadow"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("decal_vs_compressed"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("decal_ps_shadow"),
                compilation_options: Default::default(),
                targets: &[Some(shadow_state)],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain decal"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // A decal laid in the world (the fields of a farm, uv past 1): the original samples its albedo
        // with sampler 2 of the renderer (TerrainDecalLayer::Render, field_38[0] = 2), whose address
        // mode is not read; the fields only look whole when it wraps.
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain decal repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let neutral = device
            .tex(
                &wgpu::TextureDescriptor {
                    label: Some("terrain decal neutral"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                },
                Cluster::Decals,
            )
            .create_view(&Default::default());
        Self {
            pipeline,
            shadow_pipeline,
            layout1,
            layout2,
            sampler,
            repeat,
            neutral,
            neutral_ready: std::cell::Cell::new(false),
        }
    }

    fn buffer(device: &wgpu::Device, usage: wgpu::BufferUsages, bytes: &[u8]) -> Buf {
        let size = (bytes.len() as u64).next_multiple_of(4).max(4);
        let b = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("terrain decal"),
                size,
                usage,
                mapped_at_creation: true,
            },
            Cluster::Decals,
        );
        if let Ok(mut m) = b.get_mapped_range_mut(..) {
            let mut padded = bytes.to_vec();
            padded.resize(size as usize, 0);
            m.copy_from_slice(&padded);
        }
        b.unmap();
        b
    }

    /// The constants of the vertex stage for a decal at a place of a page.
    fn constants(bounds: (Vec2, Vec2), transform: &Mat4) -> [f32; CONSTS_BYTES / 4] {
        let mut c = [0.0f32; CONSTS_BYTES / 4];
        c[0..4].copy_from_slice(&[bounds.0.x, bounds.0.y, bounds.1.x, bounds.1.y]);
        // The texture coordinate goes through as it is.
        c[4..8].copy_from_slice(&[1.0, 1.0, 0.0, 0.0]);
        for (k, col) in row_major(*transform).iter().enumerate() {
            c[8 + k * 4..12 + k * 4].copy_from_slice(col);
        }
        // zDist 0.
        c
    }

    /// Draws the decals into `albedo` and `height` (already holding "HeightMap"), which are the
    /// two attachments of the pass. Returns how many decals were drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        albedo: &wgpu::TextureView,
        height: &wgpu::TextureView,
        frame: &TerrainFrame,
        rect: &CellBox,
        bake: &DecalBake<'_>,
    ) -> usize {
        self.encode_with(
            device,
            encoder,
            &[albedo, height],
            &self.pipeline,
            "Decal Layer",
            PASS_LAYER,
            frame,
            rect,
            bake,
        )
    }

    /// "Decal Macro Shadow": the decals that cast a shadow, drawn over the macro shadow of the page
    /// (`shadow` holds it already).
    pub fn encode_shadow(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        shadow: &wgpu::TextureView,
        frame: &TerrainFrame,
        rect: &CellBox,
        bake: &DecalBake<'_>,
    ) -> usize {
        self.encode_with(
            device,
            encoder,
            &[shadow],
            &self.shadow_pipeline,
            "Decal Macro Shadow",
            PASS_SHADOW,
            frame,
            rect,
            bake,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn encode_with(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        targets: &[&wgpu::TextureView],
        pipeline: &wgpu::RenderPipeline,
        label: &str,
        pass_flag: u32,
        frame: &TerrainFrame,
        rect: &CellBox,
        bake: &DecalBake<'_>,
    ) -> usize {
        let draws = decals_of_page(bake.layer, frame, rect, pass_flag);
        let mut ready = Vec::new();
        for d in &draws {
            let Some(a) = d.data.albedo.as_ref().and_then(|id| bake.textures.get(id)) else {
                continue;
            };
            let h = d
                .data
                .height
                .as_ref()
                .and_then(|id| bake.textures.get(id))
                .unwrap_or(&self.neutral);
            ready.push((d, a, h));
        }
        if ready.is_empty() {
            return 0;
        }
        if !self.neutral_ready.replace(true) {
            // The grey of a height that is not there: `DecalPS` turns 0.5 into no height at all.
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Decal Layer neutral"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.neutral,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.5,
                            g: 0.5,
                            b: 0.5,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            }));
        }
        let transform = page_transform(rect);
        let mut prepared = Vec::with_capacity(ready.len());
        for (d, a, h) in ready {
            // The decal in texels of the field, moved to the copy of the map it is seen at.
            let texel = |p: Vec2| frame.world_to_tex(p - Vec2::new(d.shift, 0.0));
            let pts: Vec<Vec2> = d.data.verts.iter().map(|v| texel(v.pos)).collect();
            let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for p in &pts {
                lo = lo.min(*p);
                hi = hi.max(*p);
            }
            let span = (hi - lo).max(Vec2::splat(1e-4));
            let mut vertices: Vec<f32> = Vec::with_capacity(pts.len() * 8);
            for (p, v) in pts.iter().zip(&d.data.verts) {
                let q = (*p - lo) / span;
                vertices.extend_from_slice(&[q.x, q.y, 0.0, 0.0, v.uv.x, v.uv.y, 0.0, 0.0]);
            }
            let indices: Vec<u16> = d.data.tris.iter().flatten().copied().collect();
            let wraps = d
                .data
                .verts
                .iter()
                .any(|v| v.uv.min_element() < -1e-3 || v.uv.max_element() > 1.0 + 1e-3);
            let sampler = if wraps { &self.repeat } else { &self.sampler };
            let vb = Self::buffer(
                device,
                wgpu::BufferUsages::VERTEX,
                &vertices
                    .iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect::<Vec<u8>>(),
            );
            let ib = Self::buffer(
                device,
                wgpu::BufferUsages::INDEX,
                &indices
                    .iter()
                    .flat_map(|i| i.to_le_bytes())
                    .collect::<Vec<u8>>(),
            );
            let consts = Self::constants((lo, hi), &transform);
            let ub = Self::buffer(
                device,
                wgpu::BufferUsages::UNIFORM,
                &consts
                    .iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect::<Vec<u8>>(),
            );
            let group1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain decal textures"),
                layout: &self.layout1,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(h),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            });
            let group2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain decal vs"),
                layout: &self.layout2,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                }],
            });
            prepared.push((vb, ib, indices.len() as u32, group1, group2));
        }
        let attachment = |view| wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        };
        let attachments: Vec<Option<wgpu::RenderPassColorAttachment<'_>>> =
            targets.iter().map(|&t| Some(attachment(t))).collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline_c(pipeline);
        for (vb, ib, count, g1, g2) in &prepared {
            pass.set_bind_group_c(1, g1, &[]);
            pass.set_bind_group_c(2, g2, &[]);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed_c(0..*count, 0, 0..1);
        }
        prepared.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_takes_the_texels_of_the_page_edge_to_edge() {
        let rect = CellBox {
            x: 128,
            y: 64,
            w: 64,
            h: 64,
        };
        let m = page_transform(&rect);
        // The shader adds one to the texel before the transform.
        let at = |tx: f32, ty: f32| m.transform_point3(glam::Vec3::new(tx + 1.0, ty + 1.0, 0.0));
        // The page starts one texel before the cell and covers w + 1 texels.
        let first = at(rect.x as f32 - 1.0, rect.y as f32 - 1.0);
        assert!(
            (first.x + 1.0).abs() < 1e-5 && (first.y - 1.0).abs() < 1e-5,
            "{first:?}"
        );
        let last = at((rect.x + rect.w) as f32, (rect.y + rect.h) as f32);
        assert!(
            (last.x - 1.0).abs() < 1e-5 && (last.y + 1.0).abs() < 1e-5,
            "{last:?}"
        );
        assert_eq!(first.z, 0.5);
    }
}
