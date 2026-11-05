//! The selection ring of a unit (world-overlays WOV5, RW-20 (d)): a quad of the overlay map around the
//! selected unit's cell, drawn through `VSOverlay` and `PSValidMoves` (`ring.wgsl`) with the textures
//! `GUI_UnitSelectRing01` and `GUI_UnitSelectRingGlow01`. The colour, the pulse and the turn are
//! `Civ5ArtDefines_Overlay.xml` (`UnitSelectionHighlight`).
//!
//! What the capture `S4-select` (draw 731, `OverlayShaders::PSValidMoves`) says, and this file takes:
//! - the quad is the bounding box of two hexes, centred on the unit's cell: half a side `HEX_WIDTH`
//!   (64) across and `HEX_WIDTH * 2 / sqrt(3)` (73.9) along, vertices `(+-64, +-73.9)` from the cell
//!   (`Local` rows `(64, 0)` and `(32, 55.43)` times the vertex positions `(-5/3, 4/3)` .. `(5/3, -4/3)`);
//! - the textures cover a square of `QUAD_SIDE` (128) units, so a texel of the 512 square is a
//!   quarter of a unit: `TransScale` is `(u, 1.1547 v - 0.07735)`;
//! - the ring's coordinates are turned about the middle of the texture by the angle
//!   `rotation_speed * time` (`TransScaleRot`: `(c, -s)` and `(1.1547 s, 1.1547 c)` as the rows,
//!   the offset keeps `(0.5, 0.5)` in place); the frame's angle was 0.351 rad;
//! - the ring texture has its crest at a radius of 116 texels (29 units) and fades by 160 (40 units);
//! - the sampler clamps, the blend is ONE, INV_SRC_ALPHA, there is no depth, the colour of the draw is
//!   `(0.9, 0.9, 0.45, 1)` and `fAlpha` is the pulse (0.0108 in the frame).
// `fAlpha` is `MovementOverlay::Update`'s pulse, a triangle wave 0..0.5 (`RingStyle::alpha`); the exe loads only the
// ring and its glow (`MovementOverlay::Startup`): gui_unitselectedhexglow_01.dds is used by nothing (re-ui-live-answers
// ITEM 1 and its follow-up B).

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::HashMap;

use asset_format::{LogicalId, OverlayDefs, Rotation, TextureBlob};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec2};

use super::grid::GridVertex;
use crate::frame::WORLD_FORMAT;
use crate::hexgrid::{HEX_WIDTH, cell_center};
use crate::world::overlay::borders::gpu::local_matrix;

/// The textures of the ring (group `overlay`): the ring and its glow.
pub const TEXTURES: [&str; 2] = [
    "tx:gui_unitselectring01.dds",
    "tx:gui_unitselectringglow01.dds",
];
/// Side of the square the textures cover, in units of the map.
// re: S4-select draw 731, `TransScale` (1, 1.1547) over the quad 128 by 147.8
pub const QUAD_SIDE: f32 = 2.0 * HEX_WIDTH;
/// `2 / sqrt(3)`: the height of the hex over its width, and the stretch of the textures along V.
pub const STRETCH: f32 = 1.154_700_5;
/// Half of the quad: along X, and along Y (the ground's Z).
pub const QUAD_HALF: Vec2 = Vec2::new(HEX_WIDTH, HEX_WIDTH * STRETCH);
/// Radius of the ring's crest in the texture, and where it has faded, in units of the map.
// re: S4-select, texture of `RingSampler`: crest 116 of 512 texels, gone by 160, 128 units across
pub const CREST_RADIUS: f32 = 116.0 / 512.0 * QUAD_SIDE;
pub const FADE_RADIUS: f32 = 160.0 / 512.0 * QUAD_SIDE;
/// Space of a constant block.
const SLOT: u64 = 256;
const MAX_COPIES: usize = 3;

/// `UnitSelectionHighlight`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingStyle {
    pub color: [f32; 4],
    pub pulse: bool,
    pub pulse_speed: f32,
    pub rotation: Rotation,
    pub rotation_speed: f32,
}

impl Default for RingStyle {
    /// What the game ships.
    // re: Civ5ArtDefines_Overlay.xml:3-10 (0.9, 0.9, 0.45, 1; pulse 2; clockwise 1)
    fn default() -> Self {
        Self {
            color: [0.9, 0.9, 0.45, 1.0],
            pulse: true,
            pulse_speed: 2.0,
            rotation: Rotation::Clockwise,
            rotation_speed: 1.0,
        }
    }
}

impl RingStyle {
    pub fn from_defs(defs: &OverlayDefs) -> Self {
        let u = &defs.unit_selection_highlight;
        // An empty table (no assets) keeps the shipped look.
        if u.color == [0.0; 4] && u.pulse_speed == 0.0 && u.rotation_speed == 0.0 {
            return Self::default();
        }
        Self {
            color: u.color,
            pulse: u.pulse,
            pulse_speed: u.pulse_speed,
            rotation: u.rotation,
            rotation_speed: u.rotation_speed,
        }
    }

    /// `fAlpha` at `t` seconds: the exe's `MovementOverlay::Update` pulse, a triangle wave from 0 up to 0.5 and back with the slope
    /// `PulseSpeed` per second (0.25 s up, 0.25 s down at the shipped 2), starting at 0 and rising; 1 without a pulse
    /// (re-ui-live-answers ITEM 1, section 2).
    pub fn alpha(&self, t: f32) -> f32 {
        if !self.pulse {
            return 1.0;
        }
        let phase = (t * self.pulse_speed).rem_euclid(1.0);
        if phase < 0.5 { phase } else { 1.0 - phase }
    }

    /// The turn of the ring's texture coordinates at `t` seconds, radians.
    pub fn angle(&self, t: f32) -> f32 {
        match self.rotation {
            Rotation::Off => 0.0,
            Rotation::Clockwise => self.rotation_speed * t,
            Rotation::CounterClockwise => -self.rotation_speed * t,
        }
    }

    /// Seconds of one pulse (up and down).
    pub fn pulse_period(&self) -> f32 {
        1.0 / self.pulse_speed
    }

    /// Seconds of one turn of the ring.
    pub fn turn_period(&self) -> f32 {
        std::f32::consts::TAU / self.rotation_speed
    }
}

/// `TransScale` as the registers of a `row_major float4x4`: `(u, STRETCH v - (STRETCH - 1) / 2)`.
pub fn trans_scale() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, STRETCH, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.5 - 0.5 * STRETCH, 0.0, 1.0],
    ]
}

/// `TransScaleRot` for the angle: the stretch, then the turn about `(0.5, 0.5)`.
pub fn trans_scale_rot(angle: f32) -> [[f32; 4]; 4] {
    let (s, c) = angle.sin_cos();
    let ox = 0.5 - 0.5 * (c + s * STRETCH);
    let oy = 0.5 - 0.5 * (-s + c * STRETCH);
    [
        [c, -s, 0.0, 0.0],
        [s * STRETCH, c * STRETCH, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [ox, oy, 0.0, 1.0],
    ]
}

/// The quad around the cell, in the ground plane (`x`, `z`) with the texture's `(u, v)`.
pub fn quad(cell: (i32, i32)) -> [GridVertex; 4] {
    let c = cell_center(cell.0, cell.1);
    let corner = |sx: f32, sy: f32, u: f32, v: f32| GridVertex {
        position: [c.x + sx * QUAD_HALF.x, c.y + sy * QUAD_HALF.y, 0.0, 1.0],
        color: [1.0; 4],
        uv: [u, v],
    };
    [
        corner(-1.0, -1.0, 0.0, 0.0),
        corner(1.0, -1.0, 1.0, 0.0),
        corner(1.0, 1.0, 1.0, 1.0),
        corner(-1.0, 1.0, 0.0, 1.0),
    ]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PsBlock {
    color: [f32; 4],
    alpha: [f32; 4],
}

/// What a frame draws the ring with.
#[derive(Clone, Copy)]
pub struct RingFrame<'a> {
    pub projection: &'a Mat4,
    pub ours_to_orig: &'a Mat4,
    /// The X shifts of the copies of the map.
    pub shifts: &'a [f32],
    pub cell: Option<(i32, i32)>,
    pub style: &'a RingStyle,
    pub time: f32,
    /// The turn (radians) and `fAlpha` instead of those of `time`.
    pub pin: Option<(f32, f32)>,
}

/// The ring in the map of the overlay.
pub struct RingGpu {
    pipeline: wgpu::RenderPipeline,
    vs_consts: Buf,
    vs_group: wgpu::BindGroup,
    ps_buffer: Buf,
    ps_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<String, wgpu::TextureView>,
    texture_group: Option<wgpu::BindGroup>,
    vertices: Buf,
    indices: Buf,
    copies: usize,
    shown: bool,
}

impl RingGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let vs_module = crate::shaders::create_module(
            device,
            "overlay vs",
            crate::wgsl_module!("overlay/overlay.wgsl"),
        );
        let ps_module = crate::shaders::create_module(
            device,
            "overlay ring",
            crate::wgsl_module!("overlay/ring.wgsl"),
        );
        let uniform = |vis: wgpu::ShaderStages, size: u64| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vis,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let vs_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ring vs"),
            entries: &[uniform(wgpu::ShaderStages::VERTEX, 256)],
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
        let smp_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ring textures"),
            entries: &[tex_entry(0), smp_entry(1), tex_entry(2), smp_entry(3)],
        });
        let ps_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ring ps"),
            entries: &[uniform(wgpu::ShaderStages::FRAGMENT, 32)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ring"),
            bind_group_layouts: &[Some(&vs_layout), Some(&texture_layout), Some(&ps_layout)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ring"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &vs_module,
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
                module: &ps_module,
                entry_point: Some("psvalidmoves"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: WORLD_FORMAT,
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
        let vs_consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("ring vs consts"),
                size: SLOT * MAX_COPIES as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let vs_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ring vs"),
            layout: &vs_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &vs_consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(256),
                }),
            }],
        });
        let ps_buffer = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("ring ps"),
                size: SLOT,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let ps_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ring ps"),
            layout: &ps_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &ps_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(32),
                }),
            }],
        });
        // `RingSampler` and `GlowSampler` clamp (S4-select, the samplers of draw 731).
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ring"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let vertices = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("ring vertices"),
                size: (4 * std::mem::size_of::<GridVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let indices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("ring indices"),
                contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        );
        let _ = queue;
        Self {
            pipeline,
            vs_consts,
            vs_group,
            ps_buffer,
            ps_group,
            texture_layout,
            sampler,
            textures: HashMap::new(),
            texture_group: None,
            vertices,
            indices,
            copies: 0,
            shown: false,
        }
    }

    /// Takes the ring's textures from `textures`; returns how many of the two were taken.
    pub fn set_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &HashMap<LogicalId, TextureBlob>,
    ) -> usize {
        let mut taken = 0;
        for id in TEXTURES {
            let Some(blob) = textures.get(&LogicalId::new(id)) else {
                continue;
            };
            match crate::texture::upload_blob(
                device,
                queue,
                blob,
                id,
                crate::texture::UploadOptions::WORLD,
            ) {
                Ok(up) => {
                    self.textures.insert(
                        id.to_string(),
                        up.texture
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    );
                    taken += 1;
                }
                Err(e) => tracing::warn!("{id} left out: {e}"),
            }
        }
        // `GlowSampler` is the glow of the ring, `RingSampler` the ring.
        if let (Some(glow), Some(ring)) = (
            self.textures.get(TEXTURES[1]),
            self.textures.get(TEXTURES[0]),
        ) {
            self.texture_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ring textures"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(glow),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(ring),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
        }
        taken
    }

    /// True once the ring has its textures.
    pub fn has_textures(&self) -> bool {
        self.texture_group.is_some()
    }

    /// The constants and the quad of a frame; `frame.cell` of `None` hides the ring.
    pub fn set_frame(&mut self, queue: &wgpu::Queue, frame: &RingFrame<'_>) {
        let RingFrame {
            projection,
            ours_to_orig,
            shifts,
            cell,
            style,
            time,
            pin,
        } = *frame;
        let (angle, alpha) = pin.unwrap_or((style.angle(time), style.alpha(time)));
        self.shown = cell.is_some() && self.texture_group.is_some();
        let Some(cell) = cell else { return };
        self.copies = shifts.len().min(MAX_COPIES);
        let rows = crate::world::fow::mask::row_major;
        // The registers of `TransScaleRot` and `TransScale` are the rows of the original's matrix;
        // the shader reads register k as component k of the four vectors given (`row_major_reg`),
        // so the vectors are the columns.
        let regs = |r: [[f32; 4]; 4]| Mat4::from_cols_array_2d(&r).transpose().to_cols_array_2d();
        for (i, &shift) in shifts.iter().take(MAX_COPIES).enumerate() {
            // `VSOverlay`: `WVP`, `Local`, `TransScaleRot`, `TransScale`.
            let block = [
                rows(projection),
                rows(&local_matrix(ours_to_orig, shift)),
                regs(trans_scale_rot(angle)),
                regs(trans_scale()),
            ];
            queue.write_buffer(
                &self.vs_consts,
                i as u64 * SLOT,
                bytemuck::cast_slice(&block),
            );
        }
        queue.write_buffer(
            &self.ps_buffer,
            0,
            bytemuck::bytes_of(&PsBlock {
                color: style.color,
                alpha: [alpha, 0.0, 0.0, 0.0],
            }),
        );
        queue.write_buffer(&self.vertices, 0, bytemuck::cast_slice(&quad(cell)));
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// Draws the ring in every copy of the map.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let (true, Some(textures)) = (self.shown, &self.texture_group) else {
            return;
        };
        pass.set_pipeline_c(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.set_index_buffer(self.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_bind_group_c(1, textures, &[]);
        pass.set_bind_group_c(2, &self.ps_group, &[0]);
        for copy in 0..self.copies {
            pass.set_bind_group_c(0, &self.vs_group, &[(copy as u64 * SLOT) as u32]);
            pass.draw_indexed_c(0..6, 0, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mul(m: &[[f32; 4]; 4], u: f32, v: f32) -> (f32, f32) {
        // `VSOverlay`: t = v * row1 + u * row0 + row3.
        (
            u * m[0][0] + v * m[1][0] + m[3][0],
            u * m[0][1] + v * m[1][1] + m[3][1],
        )
    }

    /// The registers of the frame of `S4-select`: angle 0.3508 gives `(0.939, -0.3439)`,
    /// `(0.3971, 1.0843)` and the offset `(-0.168, 0.1298)`.
    #[test]
    fn the_rotation_matrix_is_the_captured_one() {
        let m = trans_scale_rot(0.3508);
        assert!((m[0][0] - 0.939).abs() < 1e-3 && (m[0][1] + 0.3439).abs() < 1e-3);
        assert!((m[1][0] - 0.3971).abs() < 1e-3 && (m[1][1] - 1.0843).abs() < 1e-3);
        assert!((m[3][0] + 0.168).abs() < 1e-3 && (m[3][1] - 0.1298).abs() < 1e-3);
        let t = trans_scale();
        assert!((t[1][1] - 1.1547).abs() < 1e-3 && (t[3][1] + 0.07737).abs() < 1e-3);
    }

    #[test]
    fn the_middle_of_the_quad_is_the_middle_of_the_glow_texture() {
        // The quad's middle is `(u, v) = (0.5, 0.5)`.
        let (x, y) = mul(&trans_scale(), 0.5, 0.5);
        assert!((x - 0.5).abs() < 1e-5 && (y - 0.5).abs() < 1e-5);
    }

    #[test]
    fn the_ring_turns_about_the_middle_of_its_texture() {
        // The quad's middle samples the texture's middle at every angle.
        for a in [0.0f32, 0.35, 1.0, 2.5] {
            let (x, y) = mul(&trans_scale_rot(a), 0.5, 0.5);
            assert!(
                (x - 0.5).abs() < 1e-4 && (y - 0.5).abs() < 1e-4,
                "{a}: {x} {y}"
            );
        }
    }

    #[test]
    fn the_quad_is_two_hexes_wide_around_the_cell() {
        let q = quad((5, 3));
        let c = cell_center(5, 3);
        assert!((c - Vec2::new(352.0, -166.27)).length() < 0.1);
        assert!((q[0].position[0] - (c.x - 64.0)).abs() < 1e-3);
        assert!((q[2].position[0] - (c.x + 64.0)).abs() < 1e-3);
        assert!(((q[2].position[1] - q[0].position[1]) - 147.8).abs() < 0.1);
        assert_eq!(QUAD_SIDE, 128.0);
        // The crest of the ring is on the rim of the hex's inscribed circle, 29 units out.
        assert!((CREST_RADIUS - 29.0).abs() < 0.1 && (FADE_RADIUS - 40.0).abs() < 0.1);
    }

    #[test]
    fn the_pulse_runs_at_speed_2_and_the_ring_turns_clockwise_at_speed_1() {
        let s = RingStyle::default();
        assert!((s.pulse_period() - 0.5).abs() < 1e-5);
        assert!((s.turn_period() - std::f32::consts::TAU).abs() < 1e-5);
        assert!(s.alpha(0.0).abs() < 1e-6);
        assert!((s.alpha(0.125) - 0.25).abs() < 1e-5);
        assert!((s.alpha(s.pulse_period() / 2.0) - 0.5).abs() < 1e-5);
        assert!((s.alpha(0.375) - 0.25).abs() < 1e-5);
        assert!((s.alpha(s.pulse_period()) - s.alpha(0.0)).abs() < 1e-5);
        assert!((s.angle(1.0) - 1.0).abs() < 1e-6);
        let off = RingStyle {
            rotation: Rotation::Off,
            pulse: false,
            ..s
        };
        assert_eq!((off.angle(5.0), off.alpha(1.3)), (0.0, 1.0));
        let ccw = RingStyle {
            rotation: Rotation::CounterClockwise,
            ..s
        };
        assert!(ccw.angle(1.0) < 0.0);
    }

    #[test]
    fn the_defs_of_the_file_are_the_style_and_an_empty_table_is_the_default() {
        assert_eq!(
            RingStyle::from_defs(&OverlayDefs::default()),
            RingStyle::default()
        );
        let mut d = OverlayDefs::default();
        d.unit_selection_highlight.color = [1.0, 0.0, 0.0, 1.0];
        d.unit_selection_highlight.pulse_speed = 3.0;
        d.unit_selection_highlight.rotation_speed = 2.0;
        let s = RingStyle::from_defs(&d);
        assert_eq!(
            (s.color[0], s.pulse_speed, s.rotation_speed),
            (1.0, 3.0, 2.0)
        );
    }
}
