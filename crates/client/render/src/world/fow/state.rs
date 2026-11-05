//! Fog of war, the state half (RV-10 `FluidFOWSystem`): what the feed gives
//! ([`FogInput`]), the state texture the B1 shaders read, the look of the two fogged states and
//! the cloud layers. CPU only; `FluidFow` uploads [`fog_texels`] as an `R8Uint` texture. The fog
//! of the original (mask, cull, clouds) is in `mask.rs`, `cull.rs` and `clouds.rs`.
//!
//! SIMPLIFIED(SIMP-013): the terrain, static and water shaders of B1 still darken a revealed cell
//! that is not visible themselves ([`FOG_HIDDEN_FACTOR`], [`FOG_HIDDEN_SATURATION`], the same
//! numbers in `terrain.wgsl` and `static.wgsl`) and paint an unrevealed one the flat fog colour
//! ([`FOG_UNREVEALED_RGB`], 146,182,243 in `fogcolor.csv`); the original does both from the mask.
//! They stop when `world-terrain` reads `FowMask` in its own shaders. What is closed here: the
//! fog is drawn by the passes of the original (the mask of the cells, the cull, the three
//! scrolling cloud layers, the classic fog at level 0), at their heights and scroll.

use crate::hexgrid::GridShape;
use crate::terrain::{Fog, TerrainSnapshot};

/// Colour of the unrevealed fog, 146,182,243.
// re: fogcolor.csv, FluidFOWSystem
pub const FOG_UNREVEALED_RGB: [u8; 3] = [146, 182, 243];
/// Revealed but not visible: colour times this.
// SIMPLIFIED(SIMP-013): own value, the original says "slightly darkened"
pub const FOG_HIDDEN_FACTOR: f32 = 0.6;
/// Revealed but not visible: how much of its colour a cell keeps (1 is all, 0 is grey). The same
/// number is `HIDDEN_SATURATION` in `terrain.wgsl` and `static.wgsl`.
// SIMPLIFIED(SIMP-013): own value
pub const FOG_HIDDEN_SATURATION: f32 = 0.55;

/// One cloud layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudLayer {
    /// Logical id of its texture.
    pub texture: &'static str,
    /// Height in multiples of the terrain scale `g_TerrainScale.z`.
    pub height: f32,
    /// Scroll along X, world units per second (`kFogScrollRate` times the layer's scale).
    pub scroll_x: f32,
}

/// The three layers, lowest first (`FogCloudTile_dif_02`, `_03`, `_01`).
// re: FluidFOWSystem.cpp:97-206 (wave 5)
pub const CLOUD_LAYERS: [CloudLayer; 3] = [
    CloudLayer {
        texture: "tx:fogcloudtile_dif_02.dds",
        height: 56.0,
        scroll_x: -2.0,
    },
    CloudLayer {
        texture: "tx:fogcloudtile_dif_03.dds",
        height: 191.25,
        scroll_x: -3.0,
    },
    CloudLayer {
        texture: "tx:fogcloudtile_dif_01.dds",
        height: 255.0,
        scroll_x: -4.0,
    },
];

/// Texture repeats per world unit: the texture repeats `floor(0.002 W)` times over the map width
/// `W` (at least once).
// re: FluidFOWSystem.cpp:97-206 (wave 5)
pub fn cloud_uv_scale(world_width: f32) -> f32 {
    (0.002 * world_width).floor().max(1.0) / world_width
}

/// Texel value for a fog state: 0 unrevealed, 1 revealed and hidden, 2 visible.
pub const fn fog_texel(fog: Fog) -> u8 {
    match fog {
        Fog::Unrevealed => 0,
        Fog::Hidden => 1,
        Fog::Visible => 2,
    }
}

/// The fog of the map as the feed gives it (`set_fog`): the state of every cell, row-major from
/// y = 0 (south) like the protocol. Replaces the whole fog; the renderer compares it with the last
/// one and does nothing when they are equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FogInput {
    pub width: u32,
    pub height: u32,
    pub wrap_x: bool,
    pub cells: Vec<Fog>,
}

impl FogInput {
    /// `None` if `cells.len() != width * height`.
    pub fn new(width: u32, height: u32, wrap_x: bool, cells: Vec<Fog>) -> Option<Self> {
        (cells.len() == width as usize * height as usize).then_some(Self {
            width,
            height,
            wrap_x,
            cells,
        })
    }

    /// The fog of a terrain snapshot: the B1 adapter of `Renderer::set_terrain`, until every caller
    /// feeds `set_fog` itself.
    pub fn from_snapshot(snapshot: &TerrainSnapshot) -> Self {
        Self {
            width: snapshot.width(),
            height: snapshot.height(),
            wrap_x: snapshot.wrap_x(),
            cells: snapshot.cells().iter().map(|c| c.fog).collect(),
        }
    }

    pub fn shape(&self) -> GridShape {
        GridShape {
            width: self.width as i32,
            height: self.height as i32,
            wrap_x: self.wrap_x,
        }
    }

    pub fn cell(&self, x: i32, y: i32) -> Option<Fog> {
        let in_map = (0..self.width as i32).contains(&x) && (0..self.height as i32).contains(&y);
        in_map.then(|| self.cells[y as usize * self.width as usize + x as usize])
    }

    /// Cells that differ from `other`, as indices; `None` when the size differs (everything).
    pub fn changed_cells(&self, other: &Self) -> Option<Vec<u32>> {
        ((self.width, self.height) == (other.width, other.height)).then(|| {
            self.cells
                .iter()
                .zip(&other.cells)
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i as u32)
                .collect()
        })
    }
}

/// The `width x height` state texture, row-major from y = 0, one byte per cell. The cell index is
/// the same as in `TerrainSnapshot::cells`, so a vertex's cell id addresses it directly.
// SIMPLIFIED(SIMP-013): a plain per-cell state texture instead of FluidFOWSystem
pub fn fog_texels(fog: &FogInput) -> Vec<u8> {
    fog.cells.iter().map(|&c| fog_texel(c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainCell;
    use crate::terrain::test_util::{land, map};

    fn input(snap: &TerrainSnapshot) -> FogInput {
        FogInput::from_snapshot(snap)
    }

    #[test]
    fn texels_follow_cell_fog_in_row_major_order() {
        let snap = map(3, 2, |x, y| match (x + y) % 3 {
            0 => TerrainCell::default(),
            1 => TerrainCell {
                fog: Fog::Hidden,
                ..land()
            },
            _ => land(),
        });
        assert_eq!(fog_texels(&input(&snap)), vec![0, 1, 2, 1, 2, 0]);
    }

    #[test]
    fn texel_values_are_distinct() {
        let t = [Fog::Unrevealed, Fog::Hidden, Fog::Visible].map(fog_texel);
        assert_eq!(t, [0, 1, 2]);
    }

    #[test]
    fn the_hidden_look_matches_the_shaders() {
        for src in [
            include_str!("../../terrain.wgsl"),
            include_str!("../../static.wgsl"),
        ] {
            let line = src
                .lines()
                .find(|l| l.contains("const HIDDEN_SATURATION"))
                .expect("shader has HIDDEN_SATURATION");
            let value: f32 = line
                .split('=')
                .nth(1)
                .unwrap()
                .trim()
                .trim_end_matches(';')
                .parse()
                .unwrap();
            assert_eq!(value, FOG_HIDDEN_SATURATION);
        }
    }

    #[test]
    fn layers_follow_the_reverse() {
        let h: Vec<f32> = CLOUD_LAYERS.iter().map(|l| l.height).collect();
        assert_eq!(h, [56.0, 191.25, 255.0]);
        let s: Vec<f32> = CLOUD_LAYERS.iter().map(|l| l.scroll_x).collect();
        assert_eq!(s, [-2.0, -3.0, -4.0]);
        // Bottom layer is the opaque one, drawn first; heights rise.
        assert!(CLOUD_LAYERS.windows(2).all(|w| w[0].height < w[1].height));
        // 40 cells of 64: the texture repeats floor(0.002 * 2560) = 5 times.
        assert!((cloud_uv_scale(2560.0) * 2560.0 - 5.0).abs() < 1e-4);
        // A tiny map still repeats once.
        assert!((cloud_uv_scale(100.0) * 100.0 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn an_input_of_the_wrong_size_is_refused() {
        assert!(FogInput::new(2, 2, false, vec![Fog::Visible; 3]).is_none());
        let a = FogInput::new(2, 2, false, vec![Fog::Visible; 4]).unwrap();
        let mut cells = vec![Fog::Visible; 4];
        cells[3] = Fog::Hidden;
        let b = FogInput::new(2, 2, false, cells).unwrap();
        assert_eq!(a.changed_cells(&b), Some(vec![3]), "one cell is one texel");
        let c = FogInput::new(4, 1, false, vec![Fog::Visible; 4]).unwrap();
        assert_eq!(a.changed_cells(&c), None);
    }
}
