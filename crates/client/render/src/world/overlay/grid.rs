//! The hex grid (world-overlays WOV2, `HexOverlay`, RW-3).
//!
//! The original draws every visible hex as a ring of twelve vertices: the six corners of the hex
//! and the same corners moved towards the centre by the line's width (`HexGrid.xml`: `width`, 0.2),
//! a quad per side (`HexOverlay::Startup`, `Fun_08a4daab`), and puts the texture `hexgrid.dds` on it.
//! The vertices go into the map of the overlay (`map.rs`) through `OverlayShaders::VSOverlay` and
//! `PSSimpleTexture`. The key G switches it on and off (events 112 and 113). This replaces the line of
//! B1 (`terrain/grid.rs`), which had a fixed width.
//!
//! The coordinates are the original's (x east, y north): the map of the overlay is a picture of the
//! ground in that basis.
// `Fun_08a4daab` works the texture coordinates out through `g_HexToTex`, a matrix of the terrain
// sampler that is not in the dump; the vertex buffer of the S1a capture (ev 741) gives them: the
// hex is the middle half of the texture (`HEX_WIDTH` by twice the corner radius over 0.25..0.75).
// The colour of `HexGrid.xml` is applied by the pixel shader of the product (see `overlay.wgsl`).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2};

use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH};

/// Logical id of the texture of the grid (group `overlay`).
pub const HEX_GRID_TEXTURE: &str = "tx:hexgrid.dds";

/// Vertices of a hex: the outer ring 0..5, the inner ring 6..11.
pub const VERTICES_PER_HEX: usize = 12;
/// Indices of a hex: a quad per side.
pub const INDICES_PER_HEX: usize = 36;
/// Hexes of a draw at most. The original's buffer holds 0x400 hexes of the visible grid; ours covers
/// the padded rectangle of the map, which holds more at the far zoom.
pub const MAX_HEXES: usize = 0x1000;

/// The look of the grid: `HexGrid.xml`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HexGridStyle {
    /// RGBA, 0 to 1.
    pub color: [f32; 4],
    /// The width of the line from the edge towards the centre, as a part of the corner radius.
    pub width: f32,
}

impl Default for HexGridStyle {
    /// What the game ships: 255,255,255,75 and 0.2.
    // re: HexGrid.xml (RW-3)
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 75.0 / 255.0],
            width: 0.2,
        }
    }
}

impl HexGridStyle {
    /// From the settings of `tb:world-b2`; a file with a short colour keeps the default for the rest.
    pub fn from_settings(grid: &asset_format::HexGrid) -> Self {
        let mut s = Self::default();
        for (c, &b) in s.color.iter_mut().zip(&grid.color) {
            *c = f32::from(b) / 255.0;
        }
        if grid.width > 0.0 && grid.width < 1.0 {
            s.width = grid.width;
        }
        s
    }
}

/// A vertex of the grid: `VSOverlay`'s `dcl_position`, `dcl_color`, `dcl_texcoord`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct GridVertex {
    pub position: [f32; 4],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

/// Corner `i` of a hex, original basis: angle 30 + 60 i degrees.
fn corner(i: usize) -> Vec2 {
    let a = f32::to_radians(30.0 + 60.0 * i as f32);
    Vec2::new(CORNER_RADIUS * a.cos(), CORNER_RADIUS * a.sin())
}

/// The indices of one hex, the order of `HexOverlay::Startup`: a quad per side from the outer to the
/// inner ring.
pub fn ring_indices() -> [u16; INDICES_PER_HEX] {
    let mut out = [0u16; INDICES_PER_HEX];
    for side in 0..5u16 {
        let q = &mut out[side as usize * 6..side as usize * 6 + 6];
        q.copy_from_slice(&[side, side + 1, side + 7, side, side + 7, side + 6]);
    }
    // The last side closes the ring.
    // re: HexOverlay.cpp:86-91 (the binary's own indices 5, 6, 0 and 5, 6, 11)
    out[30..].copy_from_slice(&[5, 6, 0, 5, 6, 11]);
    out
}

/// The twelve vertices of a hex centred on `center` (original basis) for a line `width` wide.
pub fn hex_ring(center: Vec2, width: f32) -> [GridVertex; VERTICES_PER_HEX] {
    let inner = 1.0 - width;
    std::array::from_fn(|i| {
        let c = corner(i % 6) * if i < 6 { 1.0 } else { inner };
        let p = center + c;
        GridVertex {
            position: [p.x, p.y, 0.0, 1.0],
            color: [1.0; 4],
            // The hex covers the middle half of the texture (its dark hex), north is its top: the
            // outer corners at 0.25 / 0.75, the inner ones at 0.3 / 0.7 for the width 0.2.
            // re: S1a capture, buffer of ev 741 (`OverlayShaders::PSSimpleTexture`, hexgrid.dds)
            uv: [
                0.5 + c.x / (2.0 * HEX_WIDTH),
                0.5 - c.y / (4.0 * CORNER_RADIUS),
            ],
        }
    })
}

/// The grid of `centres` (original basis): vertices and indices. More than [`MAX_HEXES`] are cut.
pub fn grid_mesh(centres: &[Vec2], width: f32) -> (Vec<GridVertex>, Vec<u16>) {
    let n = centres.len().min(MAX_HEXES);
    let template = ring_indices();
    let mut vertices = Vec::with_capacity(n * VERTICES_PER_HEX);
    let mut indices = Vec::with_capacity(n * INDICES_PER_HEX);
    for (h, c) in centres.iter().take(n).enumerate() {
        vertices.extend_from_slice(&hex_ring(*c, width));
        let base = (h * VERTICES_PER_HEX) as u16;
        indices.extend(template.iter().map(|i| i + base));
    }
    (vertices, indices)
}

/// `TransScale` of the grid's draw: the texture's v stretched by `2 / sqrt(3)` about its middle, so
/// that the hex of the vertices (corner radius over the half width) meets the hex drawn in
/// `hexgrid.dds`, which is as tall as it is wide plus an eighth.
// re: S1a capture, ev 741 `VSOverlay` cb0[8..11] = (1, 0, 0, 0), (0, 1.15473, 0, 0), (0, 0, 1, 0),
// (0, -0.07737, 0, 1); `Local` and `TransScaleRot` are the identity.
pub fn trans_scale() -> Mat4 {
    let s = 2.0 / 3.0f32.sqrt();
    Mat4::from_cols(
        glam::Vec4::new(1.0, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, s, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
        glam::Vec4::new(0.0, 0.5 - 0.5 * s, 0.0, 1.0),
    )
}

/// The constants of `VSOverlay` (`WVP`, `Local`, `TransScaleRot`, `TransScale`) for a draw of the
/// grid through `projection`: the grid is already in the world, so `Local` and `TransScaleRot` are
/// the identity; `TransScale` is [`trans_scale`].
pub fn vs_constants(projection: &Mat4) -> [[[f32; 4]; 4]; 4] {
    let id = Mat4::IDENTITY.to_cols_array_2d();
    let row_major = crate::world::fow::mask::row_major;
    [row_major(projection), id, id, row_major(&trans_scale())]
}

/// The pipeline, constants, texture and buffers of the grid's draw into the map of the overlay.
pub struct GridGpu {
    pipeline: wgpu::RenderPipeline,
    consts: Buf,
    consts_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// A white texel until `hexgrid.dds` arrives: the whole ring is the line.
    white: wgpu::TextureView,
    texture_group: wgpu::BindGroup,
    tint: Buf,
    tint_group: wgpu::BindGroup,
    vertices: Buf,
    indices: Buf,
    index_count: u32,
}

impl GridGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let module = crate::shaders::create_module(
            device,
            "overlay grid",
            crate::wgsl_module!("overlay/overlay.wgsl"),
        );
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay grid consts"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(256),
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay grid texture"),
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
        let tint_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay grid tint"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay grid"),
            bind_group_layouts: &[
                Some(&consts_layout),
                Some(&texture_layout),
                Some(&tint_layout),
            ],
            immediate_size: 0,
        });
        // `m_hAlphaState`: ONE, INV_SRC_ALPHA (the pixel shader writes colour times alpha).
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay grid"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vsoverlay"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GridVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x4, 1 => Float32x4, 2 => Float32x2
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("grid_ps"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: crate::frame::WORLD_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: premultiplied,
                        alpha: premultiplied,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("overlay grid consts"),
                size: 256,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay grid consts"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: consts.as_entire_binding(),
            }],
        });
        let tint = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("overlay grid tint"),
                contents: bytemuck::cast_slice(&HexGridStyle::default().color),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
            Cluster::Other,
        );
        let tint_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay grid tint"),
            layout: &tint_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: tint.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay grid"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let white = device
            .tex_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("overlay grid white"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: crate::frame::WORLD_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[255, 255, 255, 255],
                Cluster::Other,
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        let texture_group = Self::group(device, &texture_layout, &white, &sampler);
        let vertices = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("overlay grid vertices"),
                size: (MAX_HEXES * VERTICES_PER_HEX * std::mem::size_of::<GridVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let indices = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("overlay grid indices"),
                size: (MAX_HEXES * INDICES_PER_HEX * 2) as u64,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        Self {
            pipeline,
            consts,
            consts_group,
            texture_layout,
            sampler,
            white,
            texture_group,
            tint,
            tint_group,
            vertices,
            indices,
            index_count: 0,
        }
    }

    fn group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay grid texture"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// `hexgrid.dds`. Returns false if it cannot be used (the white texel stays).
    pub fn set_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        blob: &asset_format::TextureBlob,
    ) -> bool {
        match crate::texture::upload_blob(
            device,
            queue,
            blob,
            "hexgrid.dds",
            crate::texture::UploadOptions::WORLD,
        ) {
            Ok(up) => {
                let view = up
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                self.texture_group =
                    Self::group(device, &self.texture_layout, &view, &self.sampler);
                true
            }
            Err(e) => {
                tracing::warn!("hexgrid.dds left out: {e}");
                self.texture_group =
                    Self::group(device, &self.texture_layout, &self.white, &self.sampler);
                false
            }
        }
    }

    pub fn set_style(&self, queue: &wgpu::Queue, style: &HexGridStyle) {
        queue.write_buffer(&self.tint, 0, bytemuck::cast_slice(&style.color));
    }

    /// Uploads a frame's mesh and the projection it is drawn through.
    pub fn set_frame(
        &mut self,
        queue: &wgpu::Queue,
        projection: &Mat4,
        vertices: &[GridVertex],
        indices: &[u16],
    ) {
        queue.write_buffer(
            &self.consts,
            0,
            bytemuck::cast_slice(&vs_constants(projection)),
        );
        if !indices.is_empty() {
            queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
            queue.write_buffer(&self.indices, 0, bytemuck::cast_slice(indices));
        }
        self.index_count = indices.len() as u32;
    }

    /// Indices drawn by the last frame.
    pub fn index_count(&self) -> u32 {
        self.index_count
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.index_count == 0 {
            return;
        }
        pass.set_pipeline_c(&self.pipeline);
        pass.set_bind_group_c(0, &self.consts_group, &[]);
        pass.set_bind_group_c(1, &self.texture_group, &[]);
        pass.set_bind_group_c(2, &self.tint_group, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed_c(0..self.index_count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hex_has_twelve_vertices_and_the_inner_ring_is_the_width_smaller() {
        let v = hex_ring(Vec2::new(100.0, 50.0), 0.2);
        assert_eq!(v.len(), 12);
        let c = Vec2::new(100.0, 50.0);
        for i in 0..6 {
            let outer = Vec2::new(v[i].position[0], v[i].position[1]) - c;
            let inner = Vec2::new(v[6 + i].position[0], v[6 + i].position[1]) - c;
            assert!((outer.length() - CORNER_RADIUS).abs() < 1e-3);
            assert!((inner.length() - CORNER_RADIUS * 0.8).abs() < 1e-3);
            // The same direction.
            assert!(outer.normalize().dot(inner.normalize()) > 0.9999);
        }
    }

    #[test]
    fn the_hex_covers_the_middle_half_of_the_texture_like_the_capture() {
        let v = hex_ring(Vec2::ZERO, 0.2);
        // The capture's buffer (S1a ev 741): outer corners at u 0.25 / 0.75, v 0.25 .. 0.75; the
        // inner ring at 0.3 / 0.7.
        assert!(
            v.iter()
                .all(|p| (0.25..=0.75).contains(&p.uv[0]) && (0.25..=0.75).contains(&p.uv[1]))
        );
        let widest = v[..6].iter().map(|p| p.uv[0]).fold(0.0f32, f32::max);
        assert!((widest - 0.75).abs() < 1e-3, "{widest}");
        let inner_widest = v[6..].iter().map(|p| p.uv[0]).fold(0.0f32, f32::max);
        assert!((inner_widest - 0.7).abs() < 1e-3, "{inner_widest}");
        let tallest = v[..6].iter().map(|p| p.uv[1]).fold(0.0f32, f32::max);
        assert!((tallest - 0.75).abs() < 1e-3, "{tallest}");
        // North is the top of the texture.
        let top = v[..6]
            .iter()
            .min_by(|a, b| a.uv[1].total_cmp(&b.uv[1]))
            .unwrap();
        assert!(top.position[1] > 0.0);
    }

    #[test]
    fn the_indices_are_a_quad_per_side_like_the_original() {
        let i = ring_indices();
        assert_eq!(&i[..6], &[0, 1, 7, 0, 7, 6]);
        assert_eq!(&i[24..30], &[4, 5, 11, 4, 11, 10]);
        // The binary's closing quad.
        assert_eq!(&i[30..], &[5, 6, 0, 5, 6, 11]);
        assert!(i.iter().all(|&k| usize::from(k) < VERTICES_PER_HEX));
    }

    #[test]
    fn a_mesh_has_a_hex_after_a_hex_and_is_cut_at_the_limit() {
        let c: Vec<Vec2> = (0..3).map(|i| Vec2::new(i as f32 * 64.0, 0.0)).collect();
        let (v, i) = grid_mesh(&c, 0.2);
        assert_eq!((v.len(), i.len()), (36, 108));
        assert_eq!(i[36], 12, "the second hex starts at its own vertices");
        assert!(i.iter().all(|&k| usize::from(k) < v.len()));
        let many = vec![Vec2::ZERO; MAX_HEXES + 10];
        let (v, _) = grid_mesh(&many, 0.2);
        assert_eq!(v.len(), MAX_HEXES * VERTICES_PER_HEX);
        // 0x1000 hexes of 12 vertices fit 16-bit indices.
        assert!(v.len() <= usize::from(u16::MAX) + 1);
    }

    #[test]
    fn the_style_is_what_the_game_ships_and_the_file_overrides_it() {
        let s = HexGridStyle::default();
        assert_eq!(s.width, 0.2);
        assert!((s.color[3] - 75.0 / 255.0).abs() < 1e-6);
        let g = asset_format::HexGrid {
            color: vec![255, 0, 0, 128],
            width: 0.5,
            extra: Default::default(),
        };
        let f = HexGridStyle::from_settings(&g);
        assert_eq!(f.width, 0.5);
        assert!((f.color[0] - 1.0).abs() < 1e-6 && f.color[1] == 0.0);
        // A broken width keeps the default.
        let bad = asset_format::HexGrid {
            color: vec![],
            width: 0.0,
            extra: Default::default(),
        };
        assert_eq!(HexGridStyle::from_settings(&bad), HexGridStyle::default());
    }

    #[test]
    fn the_constants_have_the_projection_first_and_identities_after() {
        let p = Mat4::from_translation(glam::Vec3::new(3.0, 4.0, 0.0));
        let c = vs_constants(&p);
        // `row_major` is the transpose: the translation is in the last column of the logical matrix,
        // so in the last row of the stored one.
        assert_eq!(c[0][0][3], 3.0);
        assert_eq!(c[0][1][3], 4.0);
        assert_eq!(c[1], Mat4::IDENTITY.to_cols_array_2d());
    }
}
