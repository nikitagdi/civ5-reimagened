//! The height tiles of the terrain on the GPU: the `R8Unorm` textures
//! that `TerrainShaderVS` reads in the vertex stage (`DisplacementMap`, an A8 texture in the
//! original). The field is cut into tiles of 1024 x 1024 texels, which overlap by one texel so the
//! last vertex of a cell is in the tile of the cell. A tile is made when the first cell of it is
//! built and written cell by cell afterwards; a cell whose neighbour is built later is written
//! again (its last row and column are the neighbour's first).
//!
//! The byte of a texel is `raw / 255` rounded: the vertex stage multiplies the texture value by the
//! scale `m_vTerrainScale.z` and by 255 (`cell_vs.wgsl`), so raw 255 is one unit of the original's
//! scale, as the A8 tile of the original. The tile does not hold the bytes themselves but the bicubic
//! surface the original's tessellation makes of them ([`patch_surface`]), in `R16Float`: the vertices
//! of the lattice are the vertices of its patches.

use crate::perf::{Cluster, DeviceTrack, Tex};
use std::collections::HashMap;

use super::cells::CellBox;
use super::sampler::TerrainSampler;

/// Texels per side of a tile, the step of the tiles over the field.
// re: "Тайлы высот на GPU" (about 1 MB a tile)
pub const TILE_TEXELS: i32 = 1024;
/// Texels per side of a tile texture: the overlap of one texel.
pub const TILE_SIDE: u32 = TILE_TEXELS as u32 + 1;
/// Cells (64 texels) per side of a tile.
pub const CELLS_PER_TILE: i32 = TILE_TEXELS / 64;

/// The tile of a cell: the tile holds the cell and its last vertex.
pub fn tile_of_cell(cx: i32, cy: i32) -> (i32, i32) {
    (cx.div_euclid(CELLS_PER_TILE), cy.div_euclid(CELLS_PER_TILE))
}

/// The texel of the field a tile starts at.
pub fn tile_origin(tile: (i32, i32)) -> (i32, i32) {
    (tile.0 * TILE_TEXELS, tile.1 * TILE_TEXELS)
}

/// The byte of a raw height.
pub fn height_byte(raw: u16) -> u8 {
    ((u32::from(raw) + 127) / 255).min(255) as u8
}

/// The format of a tile: the heights of the bicubic surface are not on the steps of the bytes.
pub const TILE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R16Float;

/// The rows of the basis matrix of the patches (`cb1[10..13]` of the domain stage): the weights of the
/// four control points of a row by `(u^3, u^2, u, 1)`.
// re: S2 ev 793 (`TerrainShader` hull and domain stages, ds f53bf44a2913), constant buffer 1, c10..c13
const PATCH_BASIS: [[f32; 4]; 4] = [
    [-0.125, 0.875, -0.875, 0.125],
    [0.375, -1.5, 1.125, 0.0],
    [-0.375, 0.0, 0.375, 0.0],
    [0.125, 0.75, 0.125, 0.0],
];

/// The weights of the four control points of a patch at `u` (0..1).
fn patch_weights(u: f32) -> [f32; 4] {
    let p = [u * u * u, u * u, u, 1.0];
    std::array::from_fn(|k| (0..4).map(|r| p[r] * PATCH_BASIS[r][k]).sum())
}

/// The offsets of the four control points of the patch under a texel, and their weights: an even
/// texel is the start of a patch (`u = 0`), an odd one its middle (`u = 1/2`). The original draws the
/// terrain as bicubic patches of 2 x 2 texels (`BicubicTerrainTessellation`, a hull and a domain
/// stage): the control points are two texels apart, each the height texture read at level 1 in the
/// middle of an even texel, and the patch is cut in two in each axis (tessellation factor 2), so its
/// vertices are on the texels. WebGL2 and WebGPU have no tessellation stage, the tile holds the
/// surface instead.
// re: S2 ev 793 vs 4d6b3fd682a4 (`(id & 255) - 3`, `(id >> 8) - 3` times `cb0[6].xy` = 2, `sample_l`
// at level `cb0[6].z` = 1), hs 332b9f4d09a6 (factor `cb1[14].x` = 2), ds f53bf44a2913 (the patch
// starts at control point 5)
fn control_points(odd: bool) -> [(i32, f32); 4] {
    // The control points of the patch: the first starts two texels before the patch.
    let (start, u) = if odd { (-1, 0.5) } else { (0, 0.0) };
    let w = patch_weights(u);
    std::array::from_fn(|i| (start + 2 * (i as i32 - 1), w[i]))
}

/// The bytes of level 1 of a height tile as the original builds them on the CPU
/// (`Terrain::RecreateGPUHeightmapAndIDMap`, RW8 Q3), tessellation on: a 2 x 2 box average, floored
/// (`(a + b + c + d) >> 2`), then a 3 x 3 sharpen with the kernel `-1 .. 9 .. -1` of
/// `g_aiHeightMipKernel`, `((S - c) >> 3) + c` clamped to `0..=255` with `S` the weighted sum and `c`
/// the box value (level 2 and up are made from the sharpened level; only level 1 is read).
/// `box_at(i, j)` is the floored box of level 1 texel `(i, j)`.
// re: RW8 Q3, 0x0890c454 (the kernel bytes at 0x092148c8). The sharpen clamps its neighbours to the
// border of the tile's own level; the border is in the 64 texels of overlap of the original's tiles,
// so the continuous field is used here.
pub fn sharpened_level1(box_at: impl Fn(i32, i32) -> i32, i: i32, j: i32) -> i32 {
    let mut sum = 0;
    for dj in -1..=1 {
        for di in -1..=1 {
            sum -= box_at(i + di, j + dj);
        }
    }
    let c = box_at(i, j);
    // `sum` is minus the nine box values: `S = 9 c - (the eight around)` is `10 c + sum`.
    let s = 10 * c + sum;
    (((s - c) >> 3) + c).clamp(0, 255)
}

/// The heights of the bicubic surface at the texels of `rect`, row by row, from the bytes of the field
/// (`byte(u, v)`, any texel, the wrap and the clamp done by the caller), in the unit of the bytes.
/// A control point of the patches is level 1 of the original's mips ([`sharpened_level1`]) read at
/// the middle of an even texel: the bilinear mix of the level 1 texels `c / 2 - 1` (1/4) and `c / 2`
/// (3/4) in each axis.
pub fn patch_surface(byte: impl Fn(i32, i32) -> f32, rect: &CellBox) -> Vec<f32> {
    if rect.w <= 0 || rect.h <= 0 {
        return Vec::new();
    }
    let ctrl = [control_points(false), control_points(true)];
    let pick = |t: i32| &ctrl[t.rem_euclid(2) as usize];
    // The control points (even coordinates) any texel of the rectangle reads.
    let even_down = |t: i32| t.div_euclid(2) * 2;
    let (cx0, cx1) = (even_down(rect.x - 3), even_down(rect.x + rect.w + 3) + 2);
    let (cy0, cy1) = (even_down(rect.y - 3), even_down(rect.y + rect.h + 3) + 2);
    // Level 1 texels they read, and one more around for the sharpen.
    let (i0, i1) = (cx0 / 2 - 1, cx1 / 2);
    let (j0, j1) = (cy0 / 2 - 1, cy1 / 2);
    let (bi0, bj0) = (i0 - 1, j0 - 1);
    let bw = (i1 - i0 + 3) as usize;
    let boxes: Vec<i32> = (bj0..=j1 + 1)
        .flat_map(|j| (bi0..=i1 + 1).map(move |i| (i, j)))
        .map(|(i, j)| {
            let b = |du, dv| byte(2 * i + du, 2 * j + dv).round() as i32;
            (b(0, 0) + b(1, 0) + b(0, 1) + b(1, 1)) >> 2
        })
        .collect();
    let box_at = |i: i32, j: i32| boxes[(j - bj0) as usize * bw + (i - bi0) as usize];
    let lw = (i1 - i0 + 1) as usize;
    let level1: Vec<f32> = (j0..=j1)
        .flat_map(|j| (i0..=i1).map(move |i| (i, j)))
        .map(|(i, j)| sharpened_level1(box_at, i, j) as f32)
        .collect();
    let l1 = |i: i32, j: i32| level1[(j - j0) as usize * lw + (i - i0) as usize];
    // Control points on the even lattice.
    let cw = ((cx1 - cx0) / 2 + 1) as usize;
    let ch = ((cy1 - cy0) / 2 + 1) as usize;
    let mut points = vec![0.0f32; cw * ch];
    for (jj, row) in points.chunks_mut(cw).enumerate() {
        let j = (cy0 + 2 * jj as i32) / 2;
        for (ii, p) in row.iter_mut().enumerate() {
            let i = (cx0 + 2 * ii as i32) / 2;
            *p = 0.0625 * l1(i - 1, j - 1)
                + 0.1875 * (l1(i, j - 1) + l1(i - 1, j))
                + 0.5625 * l1(i, j);
        }
    }
    // Across, for every row of control points; then along.
    let w = rect.w as usize;
    let mut across = vec![0.0f32; ch * w];
    for (jj, row) in across.chunks_mut(w).enumerate() {
        for (i, u) in (rect.x..rect.x + rect.w).enumerate() {
            row[i] = pick(u)
                .iter()
                .map(|&(o, k)| k * points[jj * cw + ((u + o - cx0) / 2) as usize])
                .sum();
        }
    }
    let mut out = Vec::with_capacity(w * rect.h as usize);
    for v in rect.y..rect.y + rect.h {
        for i in 0..w {
            out.push(
                pick(v)
                    .iter()
                    .map(|&(o, k)| k * across[((v + o - cy0) / 2) as usize * w + i])
                    .sum(),
            );
        }
    }
    out
}

/// A tile on the GPU.
pub struct HeightTile {
    pub texture: Tex,
    pub view: wgpu::TextureView,
}

/// The tiles of one map.
#[derive(Default)]
pub struct HeightTiles {
    tiles: HashMap<(i32, i32), HeightTile>,
    /// Bytes written to the GPU so far (`?stats=1`, tests).
    uploaded: u64,
}

impl HeightTiles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn tile(&self, tile: (i32, i32)) -> Option<&HeightTile> {
        self.tiles.get(&tile)
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded
    }

    /// Forgets the tiles (a new map).
    pub fn clear(&mut self) {
        self.tiles.clear();
    }

    fn make(device: &wgpu::Device, tile: (i32, i32)) -> HeightTile {
        let texture = device.tex(
            &wgpu::TextureDescriptor {
                label: Some(&format!("terrain heights {}x{}", tile.0, tile.1)),
                size: wgpu::Extent3d {
                    width: TILE_SIDE,
                    height: TILE_SIDE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TILE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
            Cluster::TerrainTex,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        HeightTile { texture, view }
    }

    /// The bytes of the texels of a cell box, row by row, read from the field (after the wrap): the
    /// A8 tile of the original.
    pub fn cell_bytes(sampler: &TerrainSampler, rect: &CellBox) -> Vec<u8> {
        let mut out = Vec::with_capacity((rect.w * rect.h).max(0) as usize);
        for v in rect.y..rect.y + rect.h {
            for u in rect.x..rect.x + rect.w {
                out.push(height_byte(sampler.raw_height(u, v)));
            }
        }
        out
    }

    /// The heights of the texels of a cell box on the bicubic surface of the original, row by row,
    /// in the unit of the tile (raw 255ths over 255), as `f16` bits ([`patch_surface`]).
    pub fn cell_surface(sampler: &TerrainSampler, rect: &CellBox) -> Vec<u16> {
        let rows = sampler.frame().texel_size().1 as i32;
        let byte =
            |u: i32, v: i32| f32::from(height_byte(sampler.raw_height(u, v.clamp(0, rows - 1))));
        patch_surface(byte, rect)
            .into_iter()
            .map(|h| asset_format::f32_to_f16_bits(h / 255.0))
            .collect()
    }

    /// Writes the texels of a cell into its tile, making the tile if it is not there.
    pub fn upload_cell(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sampler: &TerrainSampler,
        cell: (i32, i32),
        rect: &CellBox,
    ) {
        if rect.w <= 0 || rect.h <= 0 {
            return;
        }
        let tile = tile_of_cell(cell.0, cell.1);
        let (ox, oy) = tile_origin(tile);
        let bytes: Vec<u8> = Self::cell_surface(sampler, rect)
            .into_iter()
            .flat_map(u16::to_le_bytes)
            .collect();
        let t = self
            .tiles
            .entry(tile)
            .or_insert_with(|| Self::make(device, tile));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &t.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: (rect.x - ox) as u32,
                    y: (rect.y - oy) as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(rect.w as u32 * 2),
                rows_per_image: Some(rect.h as u32),
            },
            wgpu::Extent3d {
                width: rect.w as u32,
                height: rect.h as u32,
                depth_or_array_layers: 1,
            },
        );
        self.uploaded += bytes.len() as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::{gpu, read_texture};
    use crate::world::terrain::cells::CellGrid;
    use crate::world::terrain::sampler::{HEIGHT_TO_UNIT, TerrainFrame};

    #[test]
    fn a_cell_is_in_the_tile_that_holds_its_last_vertex() {
        // 16 cells a tile: cell 15 reaches texel 1024, the first texel of the next tile.
        assert_eq!(tile_of_cell(0, 0), (0, 0));
        assert_eq!(tile_of_cell(15, 15), (0, 0));
        assert_eq!(tile_of_cell(16, 0), (1, 0));
        assert_eq!(tile_origin((2, 1)), (2048, 1024));
        let last_vertex = 15 * 64 + 64;
        assert!(last_vertex <= TILE_TEXELS);
        assert_eq!(TILE_SIDE, 1025);
    }

    #[test]
    fn the_byte_of_a_height_is_the_raw_255th_rounded() {
        assert_eq!(height_byte(0), 0);
        assert_eq!(height_byte(127), 0);
        assert_eq!(height_byte(128), 1);
        // The plain (15423 -> 12.0) and the sea (13000 -> 10.12) with the scale 0.1985.
        let plain = f32::from(height_byte(15_423)) * 0.1985;
        let sea = f32::from(height_byte(13_000)) * 0.1985;
        assert!((plain - 12.0).abs() < 0.2, "{plain}");
        assert!((sea - 10.12).abs() < 0.2, "{sea}");
        // The unit of the sampler is the same 255th.
        assert!((f32::from(height_byte(15_423)) - 15_423.0 * HEIGHT_TO_UNIT).abs() < 0.5);
        assert_eq!(height_byte(u16::MAX), 255);
    }

    #[test]
    fn a_cell_goes_into_its_tile_at_its_place() {
        let Some(g) = gpu(
            "heights tile",
            wgpu::Backends::all(),
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let frame = TerrainFrame::new(4, 4, false);
        let mut s = TerrainSampler::new(frame);
        // A ramp on the field: raw = u * 100 + v.
        for v in 0..256 {
            for u in 0..256 {
                s.field_mut().set_height(u, v, (u * 100 + v) as u16);
            }
        }
        let grid = CellGrid::new(&frame);
        let rect = grid.cell_box(1, 2);
        let mut tiles = HeightTiles::new();
        tiles.upload_cell(&g.device, &g.queue, &s, (1, 2), &rect);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles.uploaded_bytes(), (rect.w * rect.h * 2) as u64);
        let t = tiles.tile((0, 0)).unwrap();
        let bytes = read_texture(&g, &t.texture, 0, 0, (TILE_SIDE, TILE_SIDE), 2);
        let at = |u: i32, v: i32| {
            let i = (v as usize * TILE_SIDE as usize + u as usize) * 2;
            asset_format::f16_bits_to_f32(u16::from_le_bytes([bytes[i], bytes[i + 1]])) * 255.0
        };
        // The cell starts at (64, 128); on a ramp the surface is the byte of the field there (the
        // kernels keep a linear ramp; the bytes round it by half a step at most).
        for (u, v) in [(64, 128), (100, 140), (101, 141)] {
            let field = f32::from(s.raw_height(u, v)) / 255.0;
            assert!(
                (at(u, v) - field).abs() < 0.75,
                "{u},{v}: {} {field}",
                at(u, v)
            );
        }
        // What the cell did not write is zero.
        assert_eq!(at(10, 10), 0.0);
        assert_eq!(at(64, 127), 0.0);
    }

    #[test]
    fn the_control_points_of_the_patches_keep_the_weight_and_the_middle() {
        for odd in [false, true] {
            let k = control_points(odd);
            let sum: f32 = k.iter().map(|&(_, w)| w).sum();
            let mid: f32 = k.iter().map(|&(o, w)| o as f32 * w).sum();
            assert!((sum - 1.0).abs() < 1e-6, "{odd}: {sum}");
            // The surface is not moved: the middle of the weights is the texel itself (the
            // control points sit at the middle of an even texel, half a texel apart from the
            // level 1 texel they read, a shift the weights of level 1 take back).
            assert!(mid.abs() < 0.5 + 1e-6, "{odd}: {mid}");
        }
        // u = 0 is (1, 6, 1) / 8 and u = 1/2 is (1, 31, 31, 1) / 64 of the control points.
        assert_eq!(patch_weights(0.0), [0.125, 0.75, 0.125, 0.0]);
        assert_eq!(
            patch_weights(0.5),
            [1.0 / 64.0, 31.0 / 64.0, 31.0 / 64.0, 1.0 / 64.0]
        );
    }

    #[test]
    fn the_mip_is_a_floored_box_then_the_sharpen_of_the_original() {
        // A box of 10 10 / 10 11 is (41 >> 2) = 10: the floor, not 10.25 and not a rounding up.
        let field = |u: i32, v: i32| if (u, v) == (1, 1) { 11 } else { 10 };
        let bx = |i: i32, j: i32| {
            (field(2 * i, 2 * j)
                + field(2 * i + 1, 2 * j)
                + field(2 * i, 2 * j + 1)
                + field(2 * i + 1, 2 * j + 1))
                >> 2
        };
        assert_eq!(bx(0, 0), 10);
        // A flat level stays flat; a texel one byte above its eight neighbours rises by one more
        // (8 c - sum = 8, shifted by 3), one below falls by one more; a neighbour one byte up
        // takes the texel down by one (the arithmetic shift floors -1/8 to -1).
        assert_eq!(sharpened_level1(|_, _| 77, 3, 4), 77);
        assert_eq!(
            sharpened_level1(|i, j| if (i, j) == (0, 0) { 51 } else { 50 }, 0, 0),
            52
        );
        assert_eq!(
            sharpened_level1(|i, j| if (i, j) == (0, 0) { 49 } else { 50 }, 0, 0),
            48
        );
        assert_eq!(
            sharpened_level1(|i, j| if (i, j) == (1, 0) { 51 } else { 50 }, 0, 0),
            49
        );
        // And clamped to a byte.
        assert_eq!(
            sharpened_level1(|i, j| if (i, j) == (0, 0) { 255 } else { 0 }, 0, 0),
            255
        );
        assert_eq!(
            sharpened_level1(|i, j| if (i, j) == (0, 0) { 0 } else { 255 }, 0, 0),
            0
        );
    }

    #[test]
    fn a_flat_field_stays_flat_and_a_step_overshoots_by_the_sharpen() {
        let rect = CellBox {
            x: 0,
            y: 0,
            w: 40,
            h: 1,
        };
        let flat = patch_surface(|_, _| 33.0, &rect);
        assert!(flat.iter().all(|&h| (h - 33.0).abs() < 1e-5));
        // Bytes 10 west of u = 20 and 11 from it: the surface climbs over several texels, and the
        // sharpen of level 1 takes the low side one byte down before the climb.
        let s = patch_surface(|u, _| if u < 20 { 10.0 } else { 11.0 }, &rect);
        assert!((s[5] - 10.0).abs() < 1e-6 && (s[35] - 11.0).abs() < 1e-6);
        let climbing = (14..26).filter(|&u| s[u] > 9.01 && s[u] < 10.99).count();
        assert!(climbing >= 4, "{:?}", &s[14..26]);
        assert!(s.iter().all(|&h| (8.99..=11.01).contains(&h)), "{s:?}");
    }
}
