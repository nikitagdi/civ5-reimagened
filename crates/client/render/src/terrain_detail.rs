//! The terrain detail: how finely the ground is meshed and how big its texture pages may be, picked by
//! the player and not by the render scale. The lattice of a cell, the size of the page it asks for and
//! the memory the pages may take follow this level and the size of the surface (before the scale), so a
//! phone that draws the world at 60% still has the same ground as at 100%.
//!
//! No GPU: the level, its numbers and the stored form, so the shell, the settings window and the
//! terrain agree.

// SIMPLIFIED(SIMP-exe-1140): three levels instead of the original's `TerrainDetailLevel` and tessellation
// keys; the original ties the ground to the graphics preset and has no render scale.
/// The level of the terrain detail. `High` is the ground as the graphics preset makes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerrainDetail {
    Low,
    Medium,
    #[default]
    High,
}

impl TerrainDetail {
    pub const ALL: [TerrainDetail; 3] = [Self::Low, Self::Medium, Self::High];

    /// The default of a device: `Medium` on a touch device (a large map at full scale stays near a
    /// million triangles of ground), else `High`.
    pub fn default_for(touch: bool) -> Self {
        if touch { Self::Medium } else { Self::High }
    }

    /// The number the key-value store keeps (0 Low, 1 Medium, 2 High).
    pub fn index(self) -> u32 {
        self as u32
    }

    /// The level of a stored number; `None` for one that is none of them.
    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// The coarsest the lattice of a cell may be made as a step in texels (the preset's own step
    /// stays if it is coarser): High 1 (64 x 64 squares a cell), Medium 2 (a quarter of the
    /// triangles), Low 3 (about an eighth).
    pub fn lattice_floor(self) -> u32 {
        match self {
            Self::Low => 3,
            Self::Medium => 2,
            Self::High => 1,
        }
    }

    /// The biggest page the cache bakes, in texels: no 512 pages below High, no 256 on Low. A cell
    /// that wants more gets this size (a coarser page, never a hole).
    pub fn max_page(self) -> u32 {
        match self {
            Self::Low => 128,
            Self::Medium => 256,
            Self::High => 512,
        }
    }

    /// The most GPU memory the pages of the cache take, in bytes.
    pub fn pool_budget_bytes(self) -> u64 {
        match self {
            Self::Low => 192 << 20,
            Self::Medium => 384 << 20,
            Self::High => 2 << 30,
        }
    }
}

/// The width in pixels the ground takes its level of detail from (the page of a cell is chosen by its
/// width on the screen): the width of the surface, whatever the render scale makes of the scene.
pub fn lod_screen_width(surface: (u32, u32)) -> f32 {
    surface.0.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_scale;
    use crate::world::terrain::cells::page_size_for_width;
    use crate::world::terrain::settings_map::TerrainSettings;

    #[test]
    fn the_level_of_detail_does_not_follow_the_render_scale() {
        let surface = (2400, 1080);
        for scale in [0.5, 0.6, 0.75, 1.0] {
            let scene = render_scale::scene_size(surface, scale);
            assert_eq!(scene.0, (2400.0 * scale) as u32, "the scene does shrink");
            assert_eq!(lod_screen_width(surface), 2400.0, "scale {scale}");
        }
        // A cell a fifth of the surface wide wants the same page whatever the scale, and the lattice
        // is the level's and the preset's alone.
        let wants = |d: TerrainDetail| {
            (
                page_size_for_width(0.2 * lod_screen_width(surface), 64).min(d.max_page()),
                TerrainSettings::default().with_detail(d).lattice_step(),
            )
        };
        assert_eq!(wants(TerrainDetail::High), (512, 1));
        assert_eq!(wants(TerrainDetail::Medium), (256, 2));
        assert_eq!(wants(TerrainDetail::Low), (128, 3));
    }

    #[test]
    fn a_lower_detail_makes_fewer_ground_triangles_and_never_a_finer_lattice_than_the_preset() {
        use crate::settings::{Preset, RenderSettings};
        let tris = |step: u32| {
            // The squares of a cell of 64 x 64 texels, two triangles each.
            let n = 64u32.div_ceil(step);
            2 * n * n
        };
        let high = TerrainSettings::default();
        assert_eq!(
            high.lattice_step(),
            1,
            "High at the default preset is as it was"
        );
        let medium = high
            .clone()
            .with_detail(TerrainDetail::Medium)
            .lattice_step();
        let low = high.clone().with_detail(TerrainDetail::Low).lattice_step();
        assert!(
            tris(medium) * 4 <= tris(1) + 8,
            "a quarter of the triangles"
        );
        assert!(tris(low) < tris(medium));
        // A preset that is coarse already stays coarse at High.
        let coarse = TerrainSettings::from_settings(&RenderSettings::preset(Preset::Low));
        assert_eq!(coarse.lattice_step(), 3);
        assert_eq!(coarse.with_detail(TerrainDetail::High).lattice_step(), 3);
    }

    #[test]
    fn a_touch_device_starts_below_high_and_a_desktop_at_high() {
        assert_eq!(TerrainDetail::default_for(false), TerrainDetail::High);
        assert_eq!(TerrainDetail::default_for(true), TerrainDetail::Medium);
        assert_eq!(TerrainDetail::default(), TerrainDetail::High);
    }

    #[test]
    fn the_stored_number_goes_both_ways() {
        for d in TerrainDetail::ALL {
            assert_eq!(TerrainDetail::from_index(d.index()), Some(d));
        }
        assert_eq!(TerrainDetail::from_index(3), None);
    }

    #[test]
    fn the_levels_only_get_cheaper_going_down() {
        let [low, medium, high] = TerrainDetail::ALL;
        assert!(low.lattice_floor() > medium.lattice_floor());
        assert!(medium.lattice_floor() > high.lattice_floor());
        assert!(low.max_page() < medium.max_page() && medium.max_page() < high.max_page());
        assert!(low.pool_budget_bytes() < medium.pool_budget_bytes());
        assert!(medium.pool_budget_bytes() < high.pool_budget_bytes());
        // High is what the cache did before the setting existed.
        assert_eq!(high.lattice_floor(), 1);
        assert_eq!(high.max_page(), 512);
        assert_eq!(high.pool_budget_bytes(), 2 << 30);
    }
}
