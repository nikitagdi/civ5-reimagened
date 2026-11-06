//! The cutout mask of the terrain (client-hooks, D13, T28 and T29): a mod writes one byte per column
//! of its grid, and the terrain is not drawn over a column with a byte above 0.
//!
//! **The grid.** A block is [`BLOCK_U`] (4) world units, a hex is 16 blocks wide ([`COLUMNS_PER_HEX`]).
//! The column of a point `(x, z)` of our basis (Y up, north is -Z) is `(floor(x / 4), floor(z / 4))`.
//! The mask covers the map: [`CutoutGrid::cols`] columns, `width * 16`, with X wrapping at that period
//! (a column left of 0 is the east end of the map), and [`CutoutGrid::rows`] rows from the block row
//! [`CutoutGrid::bz0`] (the rows of a map are at `z <= 0`, so the row numbers are negative). Row 0 of the
//! texture is the northernmost.
//!
//! **What reads it.** The pixel stages of the three ways the ground is drawn (the cells through the
//! cache, the ground of the milestone M1, B1's own pipeline) `discard` over a cut column. With no mask
//! `grid.x` of the uniform is 0 and the stages skip the test: the picture is the one without the hook
//! (`the_frame_without_a_mask_is_the_frame_before`).
//!
//! **What does not read it.** The dynamic shadow map is cast by models only (trees, units, cities;
//! `shadow/mod.rs`): the terrain is a receiver, so a cut column casts no shadow of its own and there is
//! nothing to discard. The macro shadow of the relief is baked into the page of a cell from the heights
//! (`shadow_pass.rs`), which the mask does not change, and it stays: the pages are not baked again for a
//! cut, a 4 u hole changes the relief shadow of its surroundings only by the height step the voxels hide.
//!
//! **Binding.** Group 3, binding 3 the mask, binding 4 [`CutoutParams`]. The cells draw with this group
//! alone ([`CutoutShared::bind_group`]); B1's ground and M1's carry the same two bindings in their
//! material group ([`CutoutShared::refs`]), made again when [`CutoutShared::revision`] changes.

use crate::hexgrid::{CORNER_RADIUS, ROW_STEP};
use crate::perf::{Buf, Cluster, DeviceTrack, Tex};

/// World units of a column.
pub const BLOCK_U: f32 = 4.0;
/// Columns across the width of a hex.
pub const COLUMNS_PER_HEX: u32 = 16;
/// The binding of the mask texture in group 3 (the material of B1 has 0 to 2).
pub const MASK_BINDING: u32 = 3;
/// The binding of the constants in group 3.
pub const PARAMS_BINDING: u32 = 4;

/// Columns of the mask a mod writes: a rectangle of the column grid. `x0` is a block column (any
/// integer, it wraps at [`CutoutGrid::cols`]), `z0` the block row of the first row (the rows of the
/// map are negative), `width` and `height` in columns. One byte per column, row by row from `z0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutoutRegion {
    pub x0: i32,
    pub z0: i32,
    pub width: u32,
    pub height: u32,
}

impl CutoutRegion {
    /// Number of bytes a mask for this region holds.
    pub fn byte_len(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

/// The column grid of a map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutoutGrid {
    /// Columns across the map: `width * 16`.
    pub cols: u32,
    /// Rows from the north edge of the map to the south.
    pub rows: u32,
    /// The block row of the first (northernmost) row; negative for any map taller than a hex.
    pub bz0: i32,
    /// The map wraps along X.
    pub wrap_x: bool,
}

impl CutoutGrid {
    /// The grid over a map of `width` x `height` hexes: the rows that hold any point of a hex, from the
    /// top corner of row 0 (`z = CORNER_RADIUS`) to the bottom corner of the last row.
    pub fn for_map(width: u32, height: u32, wrap_x: bool) -> Self {
        let north = (CORNER_RADIUS / BLOCK_U).floor() as i32;
        let south_z = -(ROW_STEP * height.saturating_sub(1) as f32 + CORNER_RADIUS);
        let bz0 = (south_z / BLOCK_U).floor() as i32;
        Self {
            cols: width * COLUMNS_PER_HEX,
            rows: (north - bz0 + 1) as u32,
            bz0,
            wrap_x,
        }
    }

    /// The column `(bx, bz)` of a point of our basis (`bx` not wrapped).
    pub fn column_of(x: f32, z: f32) -> (i32, i32) {
        ((x / BLOCK_U).floor() as i32, (z / BLOCK_U).floor() as i32)
    }

    /// The texel of a column, `None` outside the grid.
    pub fn texel_of(&self, bx: i32, bz: i32) -> Option<(u32, u32)> {
        let col = if self.wrap_x {
            bx.rem_euclid(self.cols as i32)
        } else {
            bx
        };
        let row = bz - self.bz0;
        ((0..self.cols as i32).contains(&col) && (0..self.rows as i32).contains(&row))
            .then_some((col as u32, row as u32))
    }
}

/// Why a mask was not taken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CutoutError {
    /// No map yet (`set_terrain` comes first): the grid is not known.
    #[error("no map yet, so no column grid")]
    NoMap,
    /// The bytes are not `region.byte_len()`.
    #[error("the region holds {expected} bytes, {got} given")]
    Length { expected: usize, got: usize },
    /// The region is wider than the map or has rows outside it.
    #[error("the region is outside the {cols} x {rows} columns from block row {bz0}")]
    OutOfRange { cols: u32, rows: u32, bz0: i32 },
    /// The mask does not fit a texture of this device.
    #[error("a mask of {cols} x {rows} does not fit a texture of {limit}")]
    TooLarge { cols: u32, rows: u32, limit: u32 },
}

/// The constants of the pixel stages (`CutoutParams` of `cutout.wgsl`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CutoutParams {
    /// x: 1 when a mask is set. y, z: columns and rows. w: the block row of row 0.
    pub grid: [f32; 4],
    /// x: 1 when the map wraps along X.
    pub flags: [f32; 4],
}

impl CutoutParams {
    fn new(grid: Option<CutoutGrid>) -> Self {
        match grid {
            Some(g) => Self {
                grid: [1.0, g.cols as f32, g.rows as f32, g.bz0 as f32],
                flags: [f32::from(u8::from(g.wrap_x)), 0.0, 0.0, 0.0],
            },
            _ => Self {
                grid: [0.0; 4],
                flags: [0.0; 4],
            },
        }
    }
}

/// The two bindings a material group needs to carry the mask (see [`CutoutShared::refs`]).
pub struct CutoutRefs<'a> {
    pub view: &'a wgpu::TextureView,
    pub params: &'a wgpu::Buffer,
}

/// The entries of group 3 for the mask: texture (unfiltered, read by `textureLoad`) and constants.
pub fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: MASK_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: PARAMS_BINDING,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<CutoutParams>() as u64),
            },
            count: None,
        },
    ]
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    params: Buf,
    /// 1x1 zero: bound while no mask is set.
    zero: wgpu::TextureView,
    _zero_tex: Tex,
    mask: Option<(Tex, wgpu::TextureView)>,
    group: wgpu::BindGroup,
}

impl Gpu {
    fn view(&self) -> &wgpu::TextureView {
        self.mask.as_ref().map_or(&self.zero, |(_, v)| v)
    }

    fn remake_group(&mut self) {
        self.group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain cutout"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: MASK_BINDING,
                    resource: wgpu::BindingResource::TextureView(self.view()),
                },
                wgpu::BindGroupEntry {
                    binding: PARAMS_BINDING,
                    resource: self.params.as_entire_binding(),
                },
            ],
        });
    }
}

/// The mask and its binding, owned by the world services. Inert (no GPU) until [`Self::init`].
#[derive(Default)]
pub struct CutoutShared {
    gpu: Option<Gpu>,
    grid: Option<CutoutGrid>,
    revision: u64,
}

impl CutoutShared {
    /// Called once by the renderer, with its device.
    pub(crate) fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain cutout layout"),
            entries: &layout_entries(),
        });
        let params = device.buf_init(
            &wgpu::util::BufferInitDescriptor {
                label: Some("terrain cutout params"),
                contents: bytemuck::bytes_of(&CutoutParams::new(None)),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            },
            Cluster::Other,
        );
        let zero_tex = device.tex_with_data(
            queue,
            &texture_desc(1, 1),
            wgpu::util::TextureDataOrder::LayerMajor,
            &[0],
            Cluster::TerrainTex,
        );
        let zero = zero_tex.create_view(&Default::default());
        let mut gpu = Gpu {
            device: device.clone(),
            queue: queue.clone(),
            layout,
            params,
            zero,
            _zero_tex: zero_tex,
            mask: None,
            group: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("terrain cutout (placeholder)"),
                layout: &device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &[],
                }),
                entries: &[],
            }),
        };
        gpu.remake_group();
        self.gpu = Some(gpu);
        self.revision += 1;
    }

    /// The group the cells bind at 3. `None` before [`Self::init`].
    pub fn bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.gpu.as_ref().map(|g| &g.group)
    }

    /// The view and the constants for a material group that carries the mask itself.
    pub(crate) fn refs(&self) -> Option<CutoutRefs<'_>> {
        self.gpu.as_ref().map(|g| CutoutRefs {
            view: g.view(),
            params: &g.params,
        })
    }

    /// Grows each time the view or the group is made again: the groups built over [`Self::refs`] are
    /// stale when it changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The grid of the map, `None` before the first map.
    pub fn grid(&self) -> Option<CutoutGrid> {
        self.grid
    }

    /// Whether a mask is set.
    pub fn is_set(&self) -> bool {
        self.gpu.as_ref().is_some_and(|g| g.mask.is_some())
    }

    /// A map of this size is drawn. The mask goes if the grid changed; a new map of the same size
    /// keeps it (the heights change, the columns do not).
    pub(crate) fn set_map(&mut self, width: u32, height: u32, wrap_x: bool) {
        let grid = CutoutGrid::for_map(width, height, wrap_x);
        if self.grid == Some(grid) {
            return;
        }
        let size_changed = self
            .grid
            .is_none_or(|g| (g.cols, g.rows, g.bz0) != (grid.cols, grid.rows, grid.bz0));
        self.grid = Some(grid);
        if size_changed {
            self.drop_mask();
        } else {
            self.write_params();
        }
    }

    fn write_params(&self) {
        if let Some(g) = &self.gpu {
            let params = CutoutParams::new(self.grid.filter(|_| g.mask.is_some()));
            g.queue
                .write_buffer(&g.params, 0, bytemuck::bytes_of(&params));
        }
    }

    fn drop_mask(&mut self) {
        let Some(g) = self.gpu.as_mut() else { return };
        let had = g.mask.take().is_some();
        self.write_params();
        if had && let Some(g) = self.gpu.as_mut() {
            g.remake_group();
            self.revision += 1;
        }
    }

    /// Writes the bytes of `region`. The first write makes the texture (zero: nothing is cut).
    pub fn set_mask(&mut self, region: CutoutRegion, bytes: &[u8]) -> Result<(), CutoutError> {
        let grid = self.grid.ok_or(CutoutError::NoMap)?;
        if bytes.len() != region.byte_len() {
            return Err(CutoutError::Length {
                expected: region.byte_len(),
                got: bytes.len(),
            });
        }
        let out_of_range = CutoutError::OutOfRange {
            cols: grid.cols,
            rows: grid.rows,
            bz0: grid.bz0,
        };
        let rows_end = i64::from(region.z0) + i64::from(region.height);
        if region.width > grid.cols
            || region.z0 < grid.bz0
            || rows_end > i64::from(grid.bz0) + i64::from(grid.rows)
        {
            return Err(out_of_range);
        }
        if region.byte_len() == 0 {
            return Ok(());
        }
        let g = self.gpu.as_mut().ok_or(CutoutError::NoMap)?;
        let limit = g.device.limits().max_texture_dimension_2d;
        if grid.cols > limit || grid.rows > limit {
            return Err(CutoutError::TooLarge {
                cols: grid.cols,
                rows: grid.rows,
                limit,
            });
        }
        let fresh = g.mask.is_none();
        if fresh {
            let tex = g
                .device
                .tex(&texture_desc(grid.cols, grid.rows), Cluster::TerrainTex);
            let view = tex.create_view(&Default::default());
            g.mask = Some((tex, view));
        }
        let (tex, _) = g.mask.as_ref().expect("the texture was just made");
        let cols = grid.cols as i32;
        let x0 = region.x0.rem_euclid(cols) as u32;
        let first = region.width.min(grid.cols - x0);
        let row0 = (region.z0 - grid.bz0) as u32;
        // The part east of the seam goes to the west edge: two writes, the source rows keep their stride.
        for (dst_x, src_x, n) in [(x0, 0, first), (0, first, region.width - first)] {
            if n == 0 {
                continue;
            }
            g.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: dst_x,
                        y: row0,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: u64::from(src_x),
                    bytes_per_row: Some(region.width),
                    rows_per_image: Some(region.height),
                },
                wgpu::Extent3d {
                    width: n,
                    height: region.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        if fresh {
            g.remake_group();
            self.revision += 1;
            self.write_params();
        }
        Ok(())
    }

    /// Removes the mask: the terrain is drawn whole again, the texture is freed.
    pub fn clear(&mut self) {
        self.drop_mask();
    }
}

fn texture_desc(width: u32, height: u32) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("terrain cutout mask"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_covers_every_point_of_every_hex() {
        for (w, h) in [(1, 1), (6, 4), (80, 52), (128, 80)] {
            let g = CutoutGrid::for_map(w, h, true);
            assert_eq!(g.cols, w * 16);
            // Row 0 (z = 0) has its top corner in the first rows; the last row's bottom corner is inside.
            for (z, name) in [
                (CORNER_RADIUS - 0.01, "north corner"),
                (
                    -(ROW_STEP * (h - 1) as f32 + CORNER_RADIUS) + 0.01,
                    "south corner",
                ),
            ] {
                let (_, bz) = CutoutGrid::column_of(0.0, z);
                assert!(
                    g.texel_of(0, bz).is_some(),
                    "{w}x{h} {name} row {bz} in {g:?}"
                );
            }
            // Not a row more than needed on each side.
            assert!(g.texel_of(0, g.bz0 - 1).is_none());
            assert!(g.texel_of(0, g.bz0 + g.rows as i32).is_none());
        }
        // A mod's S(col) example: the middle of the 100th row.
        let g = CutoutGrid::for_map(10, 100, true);
        assert_eq!(
            g.bz0,
            ((-(ROW_STEP * 99.0 + CORNER_RADIUS)) / 4.0).floor() as i32
        );
    }

    #[test]
    fn x_wraps_at_the_width_of_the_map_and_stops_at_its_edge_without_wrap() {
        let wrap = CutoutGrid::for_map(6, 4, true);
        let row = (0 - wrap.bz0) as u32;
        assert_eq!(wrap.texel_of(-1, 0), Some((95, row)));
        assert_eq!(wrap.texel_of(96, 0), Some((0, row)));
        assert_eq!(wrap.texel_of(96 * 3 + 5, 0), Some((5, row)));
        let flat = CutoutGrid::for_map(6, 4, false);
        assert_eq!(flat.texel_of(-1, 0), None);
        assert_eq!(flat.texel_of(96, 0), None);
        assert_eq!(flat.texel_of(95, 0), Some((95, row)));
    }

    /// The text of the mask in `terrain.wgsl` (B1's own pipeline) is that of `common/cutout.wgsl`.
    #[test]
    fn b1s_shader_repeats_the_shared_text() {
        let shared = include_str!("shaders/common/cutout.wgsl");
        let block = &shared[shared.find("struct CutoutParams").unwrap()..];
        assert!(include_str!("terrain.wgsl").contains(block));
    }

    #[test]
    fn the_constants_are_the_size_of_their_wgsl_struct() {
        assert_eq!(std::mem::size_of::<CutoutParams>(), 32);
        assert_eq!(CutoutParams::new(None).grid, [0.0; 4]);
    }

    // ---- the pixels (GPU) ----

    use crate::game_camera::GameCamera;
    use crate::gpu_test::{Gpu, gpu, read_texture};
    use crate::hexgrid::cell_center;
    use crate::perf::groups::Group;
    use crate::renderer::Renderer;
    use crate::scene::Scene;
    use crate::terrain::{
        Fog, LAND_HEIGHT, Relief, TerrainCell, TerrainKind, TerrainMesh, TerrainSnapshot,
    };
    use draw_list::UiDrawList;

    const SIZE: (u32, u32) = (320, 180);
    const MAP: (u32, u32) = (16, 10);

    /// A flat green map of 16 x 10 hexes (all land at `LAND_HEIGHT`), nothing on it.
    fn flat_world(gpu: &Gpu) -> Renderer {
        let mut r = Renderer::offscreen(gpu, SIZE, wgpu::TextureFormat::Rgba8Unorm, 1);
        let cell = TerrainCell {
            relief: Relief::Flat,
            kind: TerrainKind::Grass,
            fog: Fog::Visible,
            ..TerrainCell::default()
        };
        let snapshot =
            TerrainSnapshot::new(MAP.0, MAP.1, true, vec![cell; (MAP.0 * MAP.1) as usize]).unwrap();
        r.set_terrain(&snapshot, &TerrainMesh::build(&snapshot));
        r
    }

    fn frame(gpu: &Gpu, r: &mut Renderer, camera: &GameCamera) -> Vec<u8> {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cutout test target"),
            size: wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let commands = r.encode_frame(
            &view,
            &Scene::test(0.0),
            camera,
            &UiDrawList::default(),
            None,
        );
        gpu.queue.submit([commands]);
        r.frame_submitted();
        read_texture(gpu, &texture, 0, 0, SIZE, 4)
    }

    /// The mask of `region` on the whole grid, as the texture holds it.
    fn full_mask(grid: CutoutGrid, region: CutoutRegion, bytes: &[u8]) -> Vec<u8> {
        let mut full = vec![0u8; (grid.cols * grid.rows) as usize];
        for j in 0..region.height {
            for i in 0..region.width {
                let (col, row) = grid
                    .texel_of(region.x0 + i as i32, region.z0 + j as i32)
                    .unwrap();
                full[(row * grid.cols + col) as usize] = bytes[(j * region.width + i) as usize];
            }
        }
        full
    }

    /// The frames of one view of the flat map: without a mask, with `bytes` over `region`, and with
    /// the terrain left out altogether. Then, for every pixel whose ground point (the ray meets the
    /// plane of the land) is well inside a cut column, the masked frame shows what the frame without
    /// terrain shows, and for every pixel well inside a column that is not cut it shows what the
    /// frame without a mask shows. Returns how many pixels of each kind were checked.
    fn check_view(
        gpu: &Gpu,
        r: &mut Renderer,
        camera: &GameCamera,
        region: CutoutRegion,
        bytes: &[u8],
    ) -> (usize, usize) {
        let grid = r.cutout_grid().unwrap();
        let full = full_mask(grid, region, bytes);
        let cut = |bx: i32, bz: i32| {
            grid.texel_of(bx, bz)
                .is_some_and(|(c, row)| full[(row * grid.cols + c) as usize] > 0)
        };
        let plain = frame(gpu, r, camera);
        r.set_cutout_mask(region, bytes).unwrap();
        let masked = frame(gpu, r, camera);
        r.set_disabled_groups(Group::Terrain.bit());
        let bare = frame(gpu, r, camera);
        r.set_disabled_groups(0);
        r.clear_cutout_mask();
        let cleared = frame(gpu, r, camera);
        assert!(
            plain == cleared,
            "a cleared mask leaves the frame as it was"
        );

        let aspect = SIZE.0 as f32 / SIZE.1 as f32;
        let (mut in_hole, mut outside) = (0, 0);
        for py in 0..SIZE.1 {
            for px in 0..SIZE.0 {
                let ndc = crate::pixels_to_ndc(
                    px as f32 + 0.5,
                    py as f32 + 0.5,
                    SIZE.0 as f32,
                    SIZE.1 as f32,
                );
                let Some(g) = camera.unproject(ndc, aspect).at_height(LAND_HEIGHT) else {
                    continue;
                };
                // The four points a little way off the pixel's: all in cut columns or none.
                let kinds: Vec<bool> = [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)]
                    .iter()
                    .map(|(dx, dz)| {
                        let (bx, bz) = CutoutGrid::column_of(g.x + dx * 1.2, g.z + dz * 1.2);
                        cut(bx, bz)
                    })
                    .collect();
                let i = ((py * SIZE.0 + px) * 4) as usize;
                let (m, p, b) = (&masked[i..i + 4], &plain[i..i + 4], &bare[i..i + 4]);
                if kinds.iter().all(|&k| k) {
                    assert_ne!(p, b, "the land is there at ({px}, {py}) without the mask");
                    assert_eq!(m, b, "terrain over a cut column at ({px}, {py})");
                    in_hole += 1;
                } else if kinds.iter().all(|&k| !k) {
                    assert_eq!(
                        m, p,
                        "the mask changed a column it does not cut at ({px}, {py})"
                    );
                    outside += 1;
                }
            }
        }
        (in_hole, outside)
    }

    fn pattern(region: CutoutRegion) -> Vec<u8> {
        (0..region.height)
            .flat_map(|j| (0..region.width).map(move |i| (i, j)))
            .map(|(i, j)| match (i, j) {
                // A pillar of kept columns, and a column of the faintest cut.
                (7..=9, 6..=8) => 0,
                (1, 1) => 1,
                _ => 255,
            })
            .collect()
    }

    #[test]
    fn no_terrain_is_drawn_over_the_cut_columns() {
        let Some(gpu) = gpu(
            "no_terrain_is_drawn_over_the_cut_columns",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = flat_world(&gpu);
        let mut camera = GameCamera::new(cell_center(8, 5));
        camera.set_level(14.0);
        // About the middle of hex (8, 5): x 544, z -277.
        let region = CutoutRegion {
            x0: 128,
            z0: -78,
            width: 18,
            height: 16,
        };
        let (in_hole, outside) = check_view(&gpu, &mut r, &camera, region, &pattern(region));
        eprintln!("cut: {in_hole} pixels in the hole, {outside} outside");
        assert!(in_hole > 800, "{in_hole}");
        assert!(outside > 10_000, "{outside}");
    }

    /// The mask wraps along X: columns left of 0 are the east end of the map, and a region across
    /// the seam is cut on both copies of the map the view shows.
    #[test]
    fn the_mask_wraps_across_the_seam() {
        let Some(gpu) = gpu(
            "the_mask_wraps_across_the_seam",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = flat_world(&gpu);
        // Looks at the seam: the copy to the west shows the east end of the map.
        let mut camera = GameCamera::new(glam::Vec2::new(0.0, cell_center(0, 5).y));
        camera.set_level(13.0);
        let region = CutoutRegion {
            x0: -12,
            z0: -78,
            width: 24,
            height: 16,
        };
        assert_eq!(r.cutout_grid().unwrap().cols, 256);
        let (in_hole, outside) = check_view(&gpu, &mut r, &camera, region, &pattern(region));
        eprintln!("seam: {in_hole} pixels in the hole, {outside} outside");
        assert!(in_hole > 800, "{in_hole}");
        assert!(outside > 10_000, "{outside}");
        // Both sides of the seam: the same columns wherever they are drawn.
        let g = r.cutout_grid().unwrap();
        assert_eq!(g.texel_of(-12, -78), g.texel_of(244, -78));
    }

    #[test]
    fn a_mask_is_checked_against_the_grid() {
        let Some(gpu) = gpu(
            "a_mask_is_checked_against_the_grid",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut r = Renderer::offscreen(&gpu, SIZE, wgpu::TextureFormat::Rgba8Unorm, 1);
        let region = CutoutRegion {
            x0: 0,
            z0: 0,
            width: 2,
            height: 2,
        };
        assert_eq!(r.set_cutout_mask(region, &[1; 4]), Err(CutoutError::NoMap));
        let mut r = flat_world(&gpu);
        let g = r.cutout_grid().unwrap();
        assert_eq!(
            r.set_cutout_mask(region, &[1; 3]),
            Err(CutoutError::Length {
                expected: 4,
                got: 3
            })
        );
        let north = CutoutRegion {
            z0: g.bz0 + g.rows as i32 - 1,
            ..region
        };
        assert!(matches!(
            r.set_cutout_mask(north, &[1; 4]),
            Err(CutoutError::OutOfRange { .. })
        ));
        let wide = CutoutRegion {
            width: g.cols + 1,
            height: 1,
            ..region
        };
        assert!(matches!(
            r.set_cutout_mask(wide, &vec![1; wide.byte_len()]),
            Err(CutoutError::OutOfRange { .. })
        ));
        let ok = CutoutRegion {
            z0: g.bz0,
            ..region
        };
        assert!(r.set_cutout_mask(ok, &[1; 4]).is_ok());
        r.clear_cutout_mask();
    }
}
