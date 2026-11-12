//! `Minimap` subsystem (world-overlays 4.6; `cvMinimapRenderer`).
//!
//! The subsystem draws the `Minimap` target (320 x 190, `config.ini`) at the start of the frame, in
//! the layers of the original: the water quad, shallow water, land, culture of each owner, ice,
//! rivers, mountain sprites, the fog over everything, and the rectangle of the game camera
//! (`layers.rs`). The meshes of the hexes are made from the view of the map (`meshes.rs`) when the
//! fog, the culture or the explored region changes; the camera of the minimap fits the explored
//! region to the target (`camera.rs`). The target is the texture the UI panel shows
//! (`Renderer::minimap_view`); `Renderer::minimap_to_world` is the click of the panel.
//!
//! `shaders/minimap/minimap.wgsl` has the shaders of the original. Their tilings are the
//! `SetupGlobals` of the original, which is not in the dump (`TEXTURE_TILE`). The textures of the
//! `minimap` group come with `set_textures`; without them a flat colour stands in for each layer.

pub mod camera;
pub mod layers;
pub mod meshes;

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use std::collections::HashMap;
use std::rc::Rc;

use glam::{Vec2, Vec3};

use self::camera::{
    CAMERA_RECT_THICKNESS, CameraParams, EXPLORED_REGION_GUTTER, FOG_ALPHA, MinimapCamera,
    ViewRect, camera_rect_mesh, camera_rect_shifts, camera_rect_visible, derive_camera,
    target_to_world,
};
use self::layers::{Dirty, MinimapMap, cell_pos};
use self::meshes::{MinimapMeshes, SpriteVertex, build_meshes, quad_indices};
use super::fow::mask::FowView;
use super::fow::state::FogInput;
use super::overlay::OwnersInput;
use crate::frame::{
    DrawContext, EncoderContext, InitContext, MINIMAP, MINIMAP_SIZE, PassId, PassSlot,
    PrepareContext, RenderSystem, SystemId, WORLD_FORMAT,
};
use crate::hexgrid::{HEX_WIDTH, ROW_STEP};
use crate::shaders::create_module;
use crate::terrain::TerrainSnapshot;
use crate::texture::{UploadOptions, upload_blob};

/// Logical ids of the minimap textures (group `minimap`) and their flat stand-ins, in the order of
/// the slots 0 to 6 of group 1 of the shaders: land, water, fog, culture, shallows, ice, sprite.
pub const TEXTURES: [(&str, [u8; 4]); 7] = [
    ("tx:minimapgrass.dds", [110, 150, 70, 255]),
    ("tx:minimapwater.dds", [40, 80, 140, 255]),
    ("tx:minimapfog.dds", [12, 12, 18, 255]),
    ("tx:minimapculture.dds", [255, 255, 255, 255]),
    ("tx:minimapshallowwater.dds", [80, 140, 170, 255]),
    ("tx:minimapice.dds", [225, 235, 245, 255]),
    ("tx:minimapmountain.dds", [120, 110, 100, 255]),
];

/// World units one repeat of a layer's texture covers. The original sets the tilings in
/// `SetupGlobals`, which is not in the dump.
// TODO(RW-21): `cvMinimapRenderer::SetupGlobals` (the tilings, `g_vCityFade`).
const TEXTURE_TILE: f32 = 8.0 * HEX_WIDTH;

/// Owners with a culture mesh at most (the constant slots of the draws).
const MAX_OWNERS: usize = 63;
/// Stride of a constant slot (`min_uniform_buffer_offset_alignment`).
const SLOT: u64 = 256;
/// Mountain sprites at most.
const MAX_SPRITES: usize = 8192;

crate::uniform_block! {
    /// The constants of `MiniMapShader`, named as the CTAB of the package names them.
    pub struct MinimapConsts {
        pub g_vOffset: [f32; 2],
        pub g_vScale: [f32; 2],
        pub g_vWorldVisibleMin: [f32; 2],
        pub g_vWorldVisibleMax: [f32; 2],
        pub g_vIceOffset: [f32; 2],
        pub g_vIceBBoxMin: [f32; 2],
        pub g_vIceBBoxMax: [f32; 2],
        pub g_vLandTextureTiling: [f32; 2],
        pub g_vWaterTextureTiling: [f32; 2],
        pub g_vIceTextureTiling: [f32; 2],
        pub g_vCultureTextureTiling: [f32; 2],
        pub g_vCityFade: [f32; 2],
        pub g_vColor: [f32; 3],
        pub g_fFogAlpha: f32,
    }
}

impl MinimapConsts {
    /// The constants of a draw over `view`, with the colour `color` (linear light) for the culture.
    pub fn of(view: &ViewRect, color: [f32; 3]) -> Self {
        let (offset, scale) = view.offset_scale();
        let tiling = [1.0 / TEXTURE_TILE, 0.0];
        Self {
            g_vOffset: offset.to_array(),
            g_vScale: scale.to_array(),
            g_vWorldVisibleMin: view.min.to_array(),
            g_vWorldVisibleMax: view.max.to_array(),
            g_vIceOffset: [0.0; 2],
            g_vIceBBoxMin: view.min.to_array(),
            g_vIceBBoxMax: view.max.to_array(),
            g_vLandTextureTiling: tiling,
            g_vWaterTextureTiling: tiling,
            g_vIceTextureTiling: tiling,
            g_vCultureTextureTiling: tiling,
            g_vCityFade: [0.5, 1.0],
            g_vColor: color,
            g_fFogAlpha: FOG_ALPHA,
        }
    }
}

/// The owner's colour as the original hands it to the culture pass: `pow(byte / 255, 2.2)`.
// re: cvMinimapRenderer.cpp:680-682
pub fn culture_color(rgb: [u8; 3]) -> [f32; 3] {
    rgb.map(|b| (f32::from(b) / 255.0).powf(2.2))
}

/// Counters of `?stats=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MinimapCounters {
    /// Times the meshes were made (the fog, the culture or the explored region changed).
    pub fills: u64,
    /// Draw calls of the last frame.
    pub draws: u32,
    /// Hexes of the last meshes (all layers).
    pub hexes: u32,
}

/// What the GPU holds of the meshes of the last build.
struct GpuMeshes {
    vertices: Buf,
    indices: Buf,
    sprites: Option<Buf>,
}

struct Pipelines {
    water: wgpu::RenderPipeline,
    shallows: wgpu::RenderPipeline,
    land: wgpu::RenderPipeline,
    culture: wgpu::RenderPipeline,
    ice: wgpu::RenderPipeline,
    river: wgpu::RenderPipeline,
    sprite: wgpu::RenderPipeline,
    fog: wgpu::RenderPipeline,
    rect: wgpu::RenderPipeline,
}

/// The pipelines, constants, textures and the buffers that do not change.
struct MinimapGpu {
    pipelines: Pipelines,
    consts: Buf,
    consts_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The textures of the `minimap` group that arrived, by slot.
    loaded: [Option<wgpu::TextureView>; 7],
    /// What stands in for each slot (and the debug copy's) until its texture arrives.
    stand_ins: Vec<wgpu::TextureView>,
    texture_group: Option<wgpu::BindGroup>,
    water_vertices: Buf,
    water_indices: Buf,
    sprite_indices: Buf,
    rect_vertices: Buf,
    rect_indices: Buf,
    meshes: Option<GpuMeshes>,
}

/// Draws the minimap into its target.
#[derive(Default)]
pub struct Minimap {
    gpu: Option<MinimapGpu>,
    ground: Option<Rc<TerrainSnapshot>>,
    map: Option<MinimapMap>,
    dirty: Dirty,
    camera: Option<MinimapCamera>,
    meshes: MinimapMeshes,
    /// A rebuild of the meshes happened that the GPU has not been given.
    upload_pending: bool,
    view: FowView,
    /// The camera's quadrilateral on the ground, and the pieces of its band to draw.
    rect: RectPlan,
    counters: MinimapCounters,
}

/// The camera rectangle of a frame.
#[derive(Debug, Clone, Default, PartialEq)]
struct RectPlan {
    /// Index ranges of the band for each piece (a copy shifted in X).
    pieces: Vec<std::ops::Range<u32>>,
    visible: bool,
}

impl Minimap {
    /// The terrain of a new map: everything is made again.
    pub fn set_ground(&mut self, ground: Rc<TerrainSnapshot>) {
        self.map = Some(MinimapMap::from_snapshot(&ground));
        self.ground = Some(ground);
        self.dirty = Dirty(Dirty::FOG | Dirty::CULTURE | Dirty::EXPLORED);
        self.camera = None;
    }

    /// The fog of the feed (`OnFOWChanged`).
    pub fn set_fog(&mut self, fog: &FogInput) {
        if let Some(map) = &mut self.map {
            let d = map.set_fog(fog, self.ground.as_deref());
            self.dirty.0 |= d.0;
        }
    }

    /// The owners of the feed (`OnHexCultureChanged`).
    pub fn set_owners(&mut self, owners: &OwnersInput) {
        if let Some(map) = &mut self.map {
            let d = map.set_owners(owners);
            self.dirty.0 |= d.0;
        }
    }

    /// Back to no map.
    pub fn clear(&mut self) {
        self.map = None;
        self.ground = None;
        self.camera = None;
        self.meshes = MinimapMeshes::default();
        self.dirty = Dirty::default();
        self.upload_pending = true;
    }

    /// The camera of the frame about to be drawn (the rectangle on the minimap).
    pub fn set_frame(&mut self, view: FowView) {
        self.view = view;
    }

    pub fn stats(&self) -> MinimapCounters {
        self.counters
    }

    /// Whether the target shows a map: without one it is only cleared to the fog's black.
    pub fn shows_map(&self) -> bool {
        self.camera.is_some()
    }

    /// The view the minimap shows (`None` without a map).
    pub fn view(&self) -> Option<&ViewRect> {
        self.camera.as_ref().map(MinimapCamera::main)
    }

    /// The point of our world `(X, Z)` under the position `(u, v)` of the minimap, in 1/65535 of
    /// the width and height from the top left; `None` while there is no view.
    pub fn to_world(&self, u: u16, v: u16) -> Option<Vec2> {
        let view = self.view()?;
        let wrap = self
            .map
            .as_ref()
            .filter(|m| m.wrap_x)
            .map(MinimapMap::world_width);
        let p = target_to_world(view, u, v, wrap);
        // The original's y is north, ours Z is south.
        Some(Vec2::new(p.x, -p.y))
    }

    /// Makes the camera and the meshes again if the grid changed. True if it did.
    pub fn refresh(&mut self) -> bool {
        let Some(map) = &self.map else {
            return false;
        };
        if self.camera.is_some() && !self.dirty.any() {
            return false;
        }
        self.dirty.take();
        let (world_min, world_max) = map.world_extent();
        let params = CameraParams {
            world_min,
            world_max,
            wrap_x: map.wrap_x,
            viewport: MINIMAP_SIZE,
            gutter: EXPLORED_REGION_GUTTER,
        };
        let (lo, hi) = match map.explored_bounds() {
            Some((lo, hi)) => {
                // The last row of an odd-row region sticks half a hex further east.
                let mut hi_pos = cell_pos(hi.0, hi.1);
                if hi.1 > lo.1 {
                    hi_pos.x += HEX_WIDTH * 0.5;
                }
                (cell_pos(lo.0, lo.1), hi_pos)
            }
            // Nothing explored: the whole world is the region.
            None => (
                Vec2::new(0.0, 0.0),
                Vec2::new(
                    HEX_WIDTH * map.width as f32,
                    ROW_STEP * map.height.saturating_sub(1) as f32,
                ),
            ),
        };
        let camera = derive_camera(lo, hi, &params);
        let view = *camera.main();
        let ppu = MINIMAP_SIZE.1 as f32 / view.size().y;
        self.meshes = build_meshes(map, &view, ppu);
        self.camera = Some(camera);
        self.counters.fills += 1;
        self.counters.hexes = (self.meshes.vertices.len() / 7) as u32;
        self.upload_pending = true;
        true
    }

    /// The rectangle of the game camera for this frame: the band's mesh and its pieces.
    fn plan_rect(&mut self) -> Option<(Vec<camera::RectVertex>, Vec<u32>)> {
        self.rect = RectPlan::default();
        let (map, camera) = (self.map.as_ref()?, self.camera.as_ref()?);
        let view = *camera.main();
        let corners = ground_corners(&self.view)?;
        let (lo, hi) = corners.iter().fold(
            (Vec2::splat(f32::INFINITY), Vec2::splat(f32::NEG_INFINITY)),
            |(lo, hi), c| (lo.min(*c), hi.max(*c)),
        );
        if !camera_rect_visible(&view, lo, hi) {
            return None;
        }
        let wrap = map.wrap_x.then(|| map.world_width());
        let shifts = camera_rect_shifts(lo, hi, &view, wrap);
        if shifts.is_empty() {
            return None;
        }
        let ppu = MINIMAP_SIZE.1 as f32 / view.size().y;
        let (vertices, indices) = camera_rect_mesh(&corners, CAMERA_RECT_THICKNESS / ppu);
        // One copy of the band per piece, shifted in X.
        let n = vertices.len() as u32;
        let mut all_v = Vec::new();
        let mut all_i = Vec::new();
        for (k, dx) in shifts.iter().enumerate() {
            let base = k as u32 * n;
            all_v.extend(vertices.iter().map(|v| camera::RectVertex {
                position: [
                    v.position[0] + dx,
                    v.position[1],
                    v.position[2],
                    v.position[3],
                ],
                ..*v
            }));
            let first = all_i.len() as u32;
            all_i.extend(indices.iter().map(|i| i + base));
            self.rect.pieces.push(first..all_i.len() as u32);
        }
        self.rect.visible = true;
        Some((all_v, all_i))
    }
}

/// The four corners of the picture on the ground (z = 0), in the original's basis, in the order
/// bottom left, bottom right, top right, top left of the screen. `None` if a corner looks at the
/// sky: the rectangle is not drawn then.
pub fn ground_corners(view: &FowView) -> Option<[Vec2; 4]> {
    let inverse = view.view_proj.inverse();
    let mut out = [Vec2::ZERO; 4];
    for (slot, (x, y)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .into_iter()
        .enumerate()
    {
        let far = inverse.project_point3(Vec3::new(x, y, 1.0));
        let dir = far - view.eye;
        if dir.y.abs() < 1e-6 {
            return None;
        }
        // Our Y is the original's z.
        let t = (0.0 - view.eye.y) / dir.y;
        if t <= 0.0 {
            return None;
        }
        // The minimap lays the cells out in the grid's own frame (`cell_pos`, `to_world`: x east, y north from
        // cell (0, 0), as `cell_center` is ours with Z south), so only the rotation applies. `ours_to_orig` also
        // carries the move of the terrain frame into the original's world; with it the rectangle sat off the
        // place the camera looks at by that move (live report 2026-10-30).
        let p = view.eye + dir * t;
        out[slot] = Vec2::new(p.x, -p.z);
    }
    Some(out)
}

impl MinimapGpu {
    fn new(device: &wgpu::Device) -> Self {
        let module = create_module(
            device,
            "minimap",
            crate::wgsl_module!("common/sm3.wgsl", "minimap/minimap.wgsl"),
        );
        let consts_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("minimap consts"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<MinimapConsts>() as u64
                    ),
                },
                count: None,
            }],
        });
        // Eight textures and their samplers, binding 2k and 2k + 1 (the last is the debug copy's).
        let mut entries = Vec::new();
        for k in 0..8u32 {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * k,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * k + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("minimap textures"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("minimap"),
            bind_group_layouts: &[Some(&consts_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let alpha = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let hex_attr = wgpu::vertex_attr_array![0 => Float32x4];
        let sprite_attr = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
        let rect_attr = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let pipeline = |label: &str,
                        vs: &str,
                        fs: &str,
                        stride: u64,
                        attrs: &[wgpu::VertexAttribute],
                        blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: stride,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: attrs,
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
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: WORLD_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let hex = |label, fs, blend| pipeline(label, "vs_minimap", fs, 16, &hex_attr, blend);
        let pipelines = Pipelines {
            water: pipeline(
                "minimap water",
                "vs_minimap_ocean",
                "ps_ocean",
                16,
                &hex_attr,
                Some(alpha),
            ),
            shallows: hex("minimap shallows", "ps_shallows", Some(alpha)),
            land: hex("minimap land", "ps_land", Some(alpha)),
            culture: hex("minimap culture", "ps_culture", Some(alpha)),
            ice: hex("minimap ice", "ps_minimapice", Some(alpha)),
            river: hex("minimap rivers", "ps_river", Some(alpha)),
            sprite: pipeline(
                "minimap sprites",
                "vs_sprite",
                "ps_texturedsprite",
                std::mem::size_of::<SpriteVertex>() as u64,
                &sprite_attr,
                Some(alpha),
            ),
            // Pass 0 draws without blending: the fog covers what is under it.
            fog: hex("minimap fog", "ps_fog", None),
            rect: pipeline(
                "minimap camera rect",
                "vs_camerarect",
                "ps_camerarect",
                std::mem::size_of::<camera::RectVertex>() as u64,
                &rect_attr,
                Some(alpha),
            ),
        };

        let consts = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("minimap consts"),
                size: SLOT * (MAX_OWNERS as u64 + 1),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let consts_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("minimap consts"),
            layout: &consts_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &consts,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<MinimapConsts>() as u64),
                }),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("minimap"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let quad = [
            [0.0f32, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 1.0, 0.0, 1.0],
            [0.0, 1.0, 0.0, 1.0],
        ];
        let water_vertices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("minimap water"),
                contents: bytemuck::cast_slice(&quad),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        );
        let water_indices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("minimap water"),
                contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        );
        let sprite_indices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("minimap sprites"),
                contents: bytemuck::cast_slice(&quad_indices(MAX_SPRITES)),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        );
        // Three pieces of the band at most (-W, 0, W).
        let rect_vertices = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("minimap camera rect"),
                size: 3 * 12 * std::mem::size_of::<camera::RectVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        let rect_indices = device.buf(
            &wgpu::BufferDescriptor {
                label: Some("minimap camera rect"),
                size: 3 * 48 * 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
            Cluster::Other,
        );
        Self {
            pipelines,
            consts,
            consts_group,
            texture_layout,
            sampler,
            loaded: Default::default(),
            stand_ins: Vec::new(),
            texture_group: None,
            water_vertices,
            water_indices,
            sprite_indices,
            rect_vertices,
            rect_indices,
            meshes: None,
        }
    }

    /// The stand-in textures, once the queue exists, and the group of the textures in use.
    fn ensure_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if self.stand_ins.is_empty() {
            self.stand_ins = TEXTURES
                .iter()
                .map(|(_, color)| *color)
                // The debug copy's slot.
                .chain([[255, 255, 255, 255]])
                .map(|color| flat_texture(device, queue, color))
                .collect();
            self.texture_group = None;
        }
        if self.texture_group.is_none() {
            let views: Vec<&wgpu::TextureView> = self
                .stand_ins
                .iter()
                .enumerate()
                .map(|(k, stand_in)| {
                    self.loaded
                        .get(k)
                        .and_then(Option::as_ref)
                        .unwrap_or(stand_in)
                })
                .collect();
            self.texture_group = Some(texture_group(
                device,
                &self.texture_layout,
                &self.sampler,
                &views,
            ));
        }
    }
}

/// A 1 x 1 texture of one colour.
fn flat_texture(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::TextureView {
    let texture = device.tex_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("minimap stand-in"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORLD_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
        Cluster::RenderTargets,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn texture_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    sampler: &wgpu::Sampler,
    views: &[&wgpu::TextureView],
) -> wgpu::BindGroup {
    let mut entries = Vec::new();
    for (k, view) in views.iter().enumerate() {
        entries.push(wgpu::BindGroupEntry {
            binding: 2 * k as u32,
            resource: wgpu::BindingResource::TextureView(view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 2 * k as u32 + 1,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("minimap textures"),
        layout,
        entries: &entries,
    })
}

impl Minimap {
    /// Takes the textures of the minimap (group `minimap`) from `textures`. A missing one keeps its
    /// flat stand-in. Returns how many were taken.
    pub fn set_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &HashMap<asset_format::LogicalId, asset_format::TextureBlob>,
    ) -> usize {
        let Some(gpu) = &mut self.gpu else { return 0 };
        let mut taken = 0;
        for (k, (id, _)) in TEXTURES.iter().enumerate() {
            let Some(blob) = textures.get(&asset_format::LogicalId::new(*id)) else {
                continue;
            };
            match upload_blob(device, queue, blob, id, UploadOptions::WORLD) {
                Ok(up) => {
                    gpu.loaded[k] = Some(
                        up.texture
                            .create_view(&wgpu::TextureViewDescriptor::default()),
                    );
                    taken += 1;
                }
                Err(e) => tracing::warn!(id, "minimap texture left out: {e}"),
            }
        }
        gpu.texture_group = None;
        taken
    }

    /// Writes the meshes of the last build into buffers.
    fn upload_meshes(&mut self, device: &wgpu::Device) {
        let Some(gpu) = &mut self.gpu else { return };
        self.upload_pending = false;
        if self.meshes.indices.is_empty() && self.meshes.mountains.is_empty() {
            gpu.meshes = None;
            return;
        }
        let vertices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("minimap vertices"),
                contents: bytemuck::cast_slice(&self.meshes.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            },
            Cluster::Other,
        );
        let indices = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("minimap indices"),
                contents: bytemuck::cast_slice(&self.meshes.indices),
                usage: wgpu::BufferUsages::INDEX,
            },
            Cluster::Other,
        );
        let sprites = (!self.meshes.mountains.is_empty()).then(|| {
            let n = self.meshes.mountains.len().min(MAX_SPRITES * 4);
            device.buf_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("minimap sprites"),
                    contents: bytemuck::cast_slice(&self.meshes.mountains[..n]),
                    usage: wgpu::BufferUsages::VERTEX,
                },
                Cluster::Other,
            )
        });
        gpu.meshes = Some(GpuMeshes {
            vertices,
            indices,
            sprites,
        });
    }

    /// The constants of the frame: the base block and one per culture owner.
    fn write_constants(&self, queue: &wgpu::Queue) {
        let (Some(gpu), Some(camera)) = (&self.gpu, &self.camera) else {
            return;
        };
        let view = camera.main();
        let n = self.meshes.culture.len().min(MAX_OWNERS);
        let mut bytes = vec![0u8; (n + 1) * SLOT as usize];
        let base = MinimapConsts::of(view, [1.0; 3]);
        bytes[..std::mem::size_of::<MinimapConsts>()].copy_from_slice(bytemuck::bytes_of(&base));
        if let Some(map) = &self.map {
            for (k, c) in self.meshes.culture.iter().take(n).enumerate() {
                let block = MinimapConsts::of(view, culture_color(map.owner_color(c.owner)));
                let at = (k + 1) * SLOT as usize;
                bytes[at..at + std::mem::size_of::<MinimapConsts>()]
                    .copy_from_slice(bytemuck::bytes_of(&block));
            }
        }
        queue.write_buffer(&gpu.consts, 0, &bytes);
    }
}

impl RenderSystem for Minimap {
    fn id(&self) -> SystemId {
        SystemId::Minimap
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Pass(PassId::Minimap)]
    }

    fn init(&mut self, cx: &mut InitContext<'_>) {
        self.gpu = Some(MinimapGpu::new(cx.device));
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        if let Some(gpu) = &mut self.gpu {
            gpu.ensure_textures(cx.device, cx.queue);
        }
        self.refresh();
        if self.upload_pending {
            self.upload_meshes(cx.device);
        }
        self.write_constants(cx.queue);
        if let Some((vertices, indices)) = self.plan_rect()
            && let Some(gpu) = &self.gpu
        {
            cx.queue
                .write_buffer(&gpu.rect_vertices, 0, bytemuck::cast_slice(&vertices));
            cx.queue
                .write_buffer(&gpu.rect_indices, 0, bytemuck::cast_slice(&indices));
        }
    }

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}

    fn record_offscreen(&mut self, pass: PassId, cx: &mut EncoderContext<'_>) {
        if pass != PassId::Minimap {
            return;
        }
        let Some(target) = cx.targets.get(MINIMAP) else {
            return;
        };
        let timestamps = cx.timestamps();
        // The target is cleared to black with the alpha of the fog (`ColorClear`).
        let mut rp = cx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Minimap"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: f64::from(FOG_ALPHA),
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: timestamps,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let mut sizes: Vec<u32> = Vec::new();
        if let (Some(gpu), Some(_)) = (&self.gpu, &self.camera)
            && let Some(textures) = gpu.texture_group.as_ref()
        {
            rp.set_bind_group_c(1, textures, &[]);
            let m = &self.meshes;
            let draw_range = |rp: &mut wgpu::RenderPass<'_>,
                              sizes: &mut Vec<u32>,
                              pipeline: &wgpu::RenderPipeline,
                              slot: u32,
                              range: meshes::MeshRange| {
                if range.is_empty() {
                    return;
                }
                rp.set_pipeline_c(pipeline);
                rp.set_bind_group_c(0, &gpu.consts_group, &[slot * SLOT as u32]);
                rp.draw_indexed_c(range.indices(), 0, 0..1);
                sizes.push(range.num_indices);
            };

            // The water quad, over the whole target.
            rp.set_pipeline_c(&gpu.pipelines.water);
            rp.set_bind_group_c(0, &gpu.consts_group, &[0]);
            rp.set_vertex_buffer(0, gpu.water_vertices.slice(..));
            rp.set_index_buffer(gpu.water_indices.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed_c(0..6, 0, 0..1);
            sizes.push(6);

            if let Some(gm) = &gpu.meshes {
                rp.set_vertex_buffer(0, gm.vertices.slice(..));
                rp.set_index_buffer(gm.indices.slice(..), wgpu::IndexFormat::Uint32);
                // Shallows, land, the culture of each owner, ice, rivers.
                draw_range(&mut rp, &mut sizes, &gpu.pipelines.shallows, 0, m.coast);
                draw_range(&mut rp, &mut sizes, &gpu.pipelines.land, 0, m.land);
                for (k, c) in m.culture.iter().take(MAX_OWNERS).enumerate() {
                    draw_range(
                        &mut rp,
                        &mut sizes,
                        &gpu.pipelines.culture,
                        k as u32 + 1,
                        c.range,
                    );
                }
                draw_range(&mut rp, &mut sizes, &gpu.pipelines.ice, 0, m.ice);
                draw_range(&mut rp, &mut sizes, &gpu.pipelines.river, 0, m.rivers);
                // Mountain sprites.
                if let Some(sprites) = &gm.sprites {
                    let n = (m.mountains.len() / 4).min(MAX_SPRITES) as u32;
                    rp.set_pipeline_c(&gpu.pipelines.sprite);
                    rp.set_bind_group_c(0, &gpu.consts_group, &[0]);
                    rp.set_vertex_buffer(0, sprites.slice(..));
                    rp.set_index_buffer(gpu.sprite_indices.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed_c(0..n * 6, 0, 0..1);
                    sizes.push(n * 6);
                    rp.set_vertex_buffer(0, gm.vertices.slice(..));
                    rp.set_index_buffer(gm.indices.slice(..), wgpu::IndexFormat::Uint32);
                }
                // The fog, over everything drawn.
                draw_range(&mut rp, &mut sizes, &gpu.pipelines.fog, 0, m.fog);
            }
            // The camera rectangle.
            if self.rect.visible {
                rp.set_pipeline_c(&gpu.pipelines.rect);
                rp.set_bind_group_c(0, &gpu.consts_group, &[0]);
                rp.set_vertex_buffer(0, gpu.rect_vertices.slice(..));
                rp.set_index_buffer(gpu.rect_indices.slice(..), wgpu::IndexFormat::Uint32);
                for piece in &self.rect.pieces {
                    rp.draw_indexed_c(piece.clone(), 0, 0..1);
                    sizes.push(piece.end - piece.start);
                }
            }
        }
        drop(rp);
        self.counters.draws = sizes.len() as u32;
        for n in sizes {
            cx.stats.record_draw(n);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.counters;
        vec![
            ("minimap.fills", c.fills),
            ("minimap.draws", u64::from(c.draws)),
            ("minimap.hexes", u64::from(c.hexes)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{Fog, TerrainCell, TerrainKind};

    fn explored(kind: TerrainKind) -> TerrainCell {
        TerrainCell {
            kind,
            fog: Fog::Visible,
            ..land()
        }
    }

    fn world(w: u32, h: u32) -> Minimap {
        let mut m = Minimap::default();
        m.set_ground(Rc::new(map(w, h, |_, _| explored(TerrainKind::Grass))));
        m
    }

    #[test]
    fn nothing_is_made_without_a_map() {
        let mut m = Minimap::default();
        assert!(!m.refresh());
        assert_eq!(m.stats(), MinimapCounters::default());
        assert_eq!(m.to_world(0, 0), None);
        assert!(m.view().is_none());
    }

    #[test]
    fn the_meshes_are_made_once_and_again_only_when_the_grid_changes() {
        let mut m = world(20, 10);
        assert!(m.refresh());
        assert_eq!(m.stats().fills, 1);
        assert!(m.stats().hexes >= 200, "{}", m.stats().hexes);
        assert!(!m.refresh(), "nothing changed");
        assert_eq!(m.stats().fills, 1);

        // The camera moving does nothing to the meshes.
        m.set_frame(FowView {
            eye: Vec3::new(10.0, 500.0, 10.0),
            ..FowView::default()
        });
        assert!(!m.refresh());

        // A cell revealed: made again.
        let snap = map(20, 10, |x, _| {
            if x == 0 {
                TerrainCell::default()
            } else {
                explored(TerrainKind::Grass)
            }
        });
        let mut fresh = Minimap::default();
        fresh.set_ground(Rc::new(snap.clone()));
        fresh.refresh();
        let mut fog = FogInput::from_snapshot(&snap);
        fog.cells[0] = Fog::Visible;
        fresh.set_fog(&fog);
        assert!(fresh.refresh());
        assert_eq!(fresh.stats().fills, 2);
        // The same fog again: nothing.
        fresh.set_fog(&fog);
        assert!(!fresh.refresh());
    }

    #[test]
    fn a_change_of_owner_makes_the_meshes_again() {
        let mut m = world(6, 4);
        m.refresh();
        let owners = OwnersInput::new(6, 4, vec![0; 24], vec![[10, 20, 30]]).unwrap();
        m.set_owners(&owners);
        assert!(m.refresh());
        assert_eq!(m.meshes.culture.len(), 1);
        m.set_owners(&owners);
        assert!(!m.refresh());
    }

    #[test]
    fn the_corners_of_the_target_are_the_corners_of_the_view() {
        let mut m = world(40, 20);
        m.refresh();
        let v = *m.view().unwrap();
        // (0, 0) is the north west corner of the view; ours Z is the south.
        let nw = m.to_world(0, 0).unwrap();
        assert!(
            (nw - Vec2::new(v.min.x, -v.max.y)).length() < 1e-2,
            "{nw:?}"
        );
        let se = m.to_world(65535, 65535).unwrap();
        assert!(
            (se - Vec2::new(v.max.x, -v.min.y)).length() < 1e-2,
            "{se:?}"
        );
        let mid = m.to_world(32768, 32768).unwrap();
        let c = v.center();
        assert!((mid - Vec2::new(c.x, -c.y)).length() < 0.5, "{mid:?}");
    }

    #[test]
    fn a_wrapping_world_brings_the_click_back_into_one_period() {
        let snap = crate::terrain::TerrainSnapshot::new(
            12,
            6,
            true,
            vec![explored(TerrainKind::Grass); 72],
        )
        .unwrap();
        let mut m = Minimap::default();
        m.set_ground(Rc::new(snap));
        m.refresh();
        let w = HEX_WIDTH * 12.0;
        for (u, v) in [(0, 0), (65535, 0), (30000, 40000)] {
            let p = m.to_world(u, v).unwrap();
            assert!((0.0..w).contains(&p.x), "{p:?}");
        }
    }

    #[test]
    fn the_ground_corners_of_a_camera_looking_down_are_a_quadrilateral_on_the_ground() {
        let eye = Vec3::new(500.0, 800.0, 300.0);
        let view_m =
            glam::camera::rh::view::look_at_mat4(eye, Vec3::new(500.0, 0.0, 100.0), Vec3::NEG_Z);
        let proj = glam::camera::rh::proj::directx::perspective(0.8, 16.0 / 9.0, 10.0, 5000.0);
        let view = FowView {
            view_proj: proj * view_m,
            eye,
            ours_to_orig: crate::frame::basis_rotation(),
        };
        let c = ground_corners(&view).expect("looks at the ground");
        // The near edge is closer to the eye, hence narrower than the far one.
        let near = (c[1] - c[0]).length();
        let far = (c[2] - c[3]).length();
        assert!(far > near, "{near} {far}");
        // The move of the terrain frame does not shift them: the minimap is in the grid's frame.
        let moved = FowView {
            ours_to_orig: glam::Mat4::from_translation(Vec3::new(-1300.0, -800.0, 0.0))
                * crate::frame::basis_rotation(),
            ..view
        };
        assert_eq!(ground_corners(&moved), Some(c));
        // The camera looks at ours (500, 0, 100): grid x 500, y -100 (north is up).
        let mid = (c[0] + c[1] + c[2] + c[3]) * 0.25;
        assert!((mid.x - 500.0).abs() < 60.0 && mid.y < 0.0, "{mid}");
        // A camera looking at the sky has no corners.
        let sky = glam::camera::rh::view::look_at_mat4(eye, eye + Vec3::Y, Vec3::Z);
        let up = FowView {
            view_proj: proj * sky,
            ..view
        };
        assert_eq!(ground_corners(&up), None);
    }

    #[test]
    fn the_constant_block_is_the_size_the_shader_declares() {
        assert_eq!(std::mem::size_of::<MinimapConsts>(), 112);
        assert!(std::mem::size_of::<MinimapConsts>() as u64 <= SLOT);
        let view = ViewRect {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(200.0, 100.0),
        };
        let c = MinimapConsts::of(&view, [0.5; 3]);
        assert_eq!(c.g_vScale, [0.01, 0.02]);
        assert_eq!(c.g_vOffset, [1.0, 1.0]);
        assert_eq!(c.g_fFogAlpha, 1.0);
        assert_eq!(c.g_vColor, [0.5; 3]);
    }

    #[test]
    fn the_culture_colour_is_the_byte_to_the_2_2() {
        let c = culture_color([255, 0, 128]);
        assert!((c[0] - 1.0).abs() < 1e-6 && c[1] == 0.0);
        assert!((c[2] - (128.0f32 / 255.0).powf(2.2)).abs() < 1e-6);
    }

    #[test]
    fn the_camera_rect_is_planned_over_a_world_that_is_larger_than_the_view() {
        let mut m = world(60, 30);
        m.refresh();
        let eye = Vec3::new(1000.0, 400.0, -400.0);
        let view_m =
            glam::camera::rh::view::look_at_mat4(eye, Vec3::new(1000.0, 0.0, -500.0), Vec3::NEG_Z);
        let proj = glam::camera::rh::proj::directx::perspective(0.8, 16.0 / 9.0, 10.0, 5000.0);
        m.set_frame(FowView {
            view_proj: proj * view_m,
            eye,
            ours_to_orig: crate::frame::basis_rotation(),
        });
        let (v, i) = m.plan_rect().expect("the camera sees a part of the map");
        assert_eq!(v.len(), 12);
        assert_eq!(i.len(), 48);
        assert_eq!(m.rect.pieces.len(), 1);
        // The rectangle of a camera that sees the whole map is not drawn.
        let high = Vec3::new(1000.0, 20_000.0, -900.0);
        let down =
            glam::camera::rh::view::look_at_mat4(high, Vec3::new(1000.0, 0.0, -900.0), Vec3::NEG_Z);
        let wide = glam::camera::rh::proj::directx::perspective(1.2, 16.0 / 9.0, 10.0, 90_000.0);
        m.set_frame(FowView {
            view_proj: wide * down,
            eye: high,
            ours_to_orig: crate::frame::basis_rotation(),
        });
        assert!(m.plan_rect().is_none());
    }

    #[test]
    fn the_minimap_cell_positions_are_the_hex_grid() {
        // The mesh and `to_world` share the world of the hex grid.
        assert_eq!(cell_pos(1, 0), Vec2::new(HEX_WIDTH, 0.0));
        assert_eq!(cell_pos(0, 1), Vec2::new(HEX_WIDTH * 0.5, ROW_STEP));
    }
}
