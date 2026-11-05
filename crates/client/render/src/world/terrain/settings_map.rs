//! The quality keys of the terrain (render-core 4.3): what each key of the
//! original's ini sets in `Terrain`, read once into a plain struct so the code that bakes, pages and
//! draws does not look keys up. A change of a key reaches `Terrain::on_settings`, which builds this
//! again; the field and the build do not change with it (AC-R8.1).

use crate::settings::RenderSettings;
use crate::terrain_detail::TerrainDetail;

/// The highlight of the original (`g_bSpecEnable`) only exists at this `TerrainDetailLevel`.
// re: TerrainSystem.cpp:191 (only at TerrainDetailLevel = 3)
pub const SPEC_DETAIL_LEVEL: u32 = 3;

/// Sizes of the pages of the cache, in texels, smallest first (the pools 64, 128, 256, 512).
// re: TerrainSystem.cpp:240-273
pub const PAGE_SIZES: [u32; 4] = [64, 128, 256, 512];

/// The keys of table 4.12, as the terrain reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainSettings {
    /// `TerrainDetailLevel` 0..=3 (`g_uTerrainDetailLevel`): the highlight at 3.
    pub detail_level: u32,
    /// `TerrainTessLevel` 0..=2 (`g_uTerrainTessLevel`): the step of the lattice of a cell; 0 has
    /// its own buffer with the heights and no vertex texture.
    pub tess_level: u32,
    /// `TerrainShadowQuality` 0..=3 (`g_uShadowQuality`): the number of passes of "Macro Shadow".
    pub shadow_quality: u32,
    /// `TerrainPageinSpeedStill` (`g_uTilesToCalcPerFrame`): cells prepared per frame, still camera.
    pub pagein_still: u32,
    /// `TerrainPageinSpeedMoving` (`g_uTilesToCalcPerFrameMoving`).
    pub pagein_moving: u32,
    /// `Terrain64Chunks` .. `Terrain512Chunks` (`g_n64Textures` .. `g_n512Textures`): the number of
    /// pages of each size the cache holds at most.
    pub chunks: [u32; 4],
    /// `BicubicTerrainTessellation` (`g_bUseTessellation`).
    pub bicubic: bool,
    /// `BicubicTerrainTessTriSize` (`g_iTessellationPixelsPerTri`).
    pub tess_tri_size: u32,
    /// `BicubicTerrainTessSubdiv` (`g_iTessellationPatchDivisions`).
    pub tess_subdiv: u32,
    /// `MinimizeGrayTiles` (`g_bMinimizeGrayTiles`): the full budget of the frame.
    pub minimize_gray_tiles: bool,
    /// `OverlayLevel` 0..=2 (`g_uOverlayLevel`).
    pub overlay_level: u32,
    /// `Use Screen Space Overlay`.
    pub screen_space_overlay: bool,
    /// The player's terrain detail (not a key of the ini): the coarsest lattice, the biggest page and
    /// the memory of the pages. `High` leaves the keys as they are.
    pub detail: TerrainDetail,
}

impl TerrainSettings {
    /// Reads the keys. A key that is not in the table (cannot happen) is its High value.
    pub fn from_settings(s: &RenderSettings) -> Self {
        let get = |key: &str| s.get(key).unwrap_or(0).max(0) as u32;
        Self {
            detail_level: get("TerrainDetailLevel"),
            tess_level: get("TerrainTessLevel"),
            shadow_quality: get("TerrainShadowQuality"),
            pagein_still: get("TerrainPageinSpeedStill"),
            pagein_moving: get("TerrainPageinSpeedMoving"),
            chunks: [
                get("Terrain64Chunks"),
                get("Terrain128Chunks"),
                get("Terrain256Chunks"),
                get("Terrain512Chunks"),
            ],
            bicubic: get("BicubicTerrainTessellation") != 0,
            tess_tri_size: get("BicubicTerrainTessTriSize"),
            tess_subdiv: get("BicubicTerrainTessSubdiv"),
            minimize_gray_tiles: get("MinimizeGrayTiles") != 0,
            overlay_level: get("OverlayLevel"),
            screen_space_overlay: get("Use Screen Space Overlay") != 0,
            detail: TerrainDetail::High,
        }
    }

    /// The same keys under another terrain detail.
    pub fn with_detail(mut self, detail: TerrainDetail) -> Self {
        self.detail = detail;
        self
    }

    /// The biggest page the cache bakes (`TerrainDetail::max_page`).
    pub fn max_page(&self) -> u32 {
        self.detail.max_page()
    }

    /// The most GPU memory the pages of the cache take (`TerrainDetail::pool_budget_bytes`).
    pub fn pool_budget_bytes(&self) -> u64 {
        self.detail.pool_budget_bytes()
    }

    /// The highlight page exists (`TerrainDetailLevel` 3).
    pub fn has_spec(&self) -> bool {
        self.detail_level == SPEC_DETAIL_LEVEL
    }

    /// The bicubic tessellation runs: the key is on and the lattice is not the fallback buffer.
    pub fn tessellates(&self) -> bool {
        self.bicubic && self.tess_level != 0
    }

    /// Step of the lattice of a cell by `TerrainTessLevel` (1 -> 2, 2 -> 1, else 3).
    // re: Terrain.cpp:185-192
    pub fn lattice_step(&self) -> u32 {
        let by_key = match self.tess_level {
            1 => 2,
            2 => 1,
            _ => 3,
        };
        // The detail only makes the lattice coarser, never finer than the preset.
        by_key.max(self.detail.lattice_floor())
    }

    /// The most pages of `size` (64, 128, 256 or 512) the cache holds; 0 for another size, and for a
    /// size above [`Self::max_page`].
    pub fn page_limit(&self, size: u32) -> u32 {
        if size > self.max_page() {
            return 0;
        }
        PAGE_SIZES
            .iter()
            .position(|&s| s == size)
            .map_or(0, |i| self.chunks[i])
    }
}

impl Default for TerrainSettings {
    fn default() -> Self {
        Self::from_settings(&RenderSettings::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Preset;

    #[test]
    fn the_high_preset_has_the_highlight_and_the_full_cache() {
        let t = TerrainSettings::from_settings(&RenderSettings::preset(Preset::High));
        assert!(t.has_spec() && t.tessellates());
        assert_eq!(t.chunks, [4096, 512, 256, 64]);
        assert_eq!(t.page_limit(256), 256);
        assert_eq!(t.page_limit(100), 0);
        assert_eq!(t.lattice_step(), 1);
        assert_eq!(t.shadow_quality, 3);
        assert!(t.screen_space_overlay);
    }

    #[test]
    fn low_has_neither_the_highlight_nor_the_tessellation() {
        let t = TerrainSettings::from_settings(&RenderSettings::preset(Preset::Low));
        assert!(!t.has_spec() && !t.tessellates());
        assert_eq!(t.lattice_step(), 3);
        assert_eq!(t.tess_level, 0);
    }

    #[test]
    fn a_changed_key_is_read_again() {
        let mut s = RenderSettings::preset(Preset::High);
        s.set("TerrainShadowQuality", 0).unwrap();
        s.set("TerrainTessLevel", 1).unwrap();
        let t = TerrainSettings::from_settings(&s);
        assert_eq!(t.shadow_quality, 0);
        assert_eq!(t.lattice_step(), 2);
        // The tessellation key alone is not enough with TerrainTessLevel 0.
        s.set("TerrainTessLevel", 0).unwrap();
        assert!(!TerrainSettings::from_settings(&s).tessellates());
    }

    #[test]
    fn every_preset_gives_the_values_of_the_render_settings_table() {
        for p in Preset::ALL {
            let s = RenderSettings::preset(p);
            let t = TerrainSettings::from_settings(&s);
            let v = |key: &str| s.get(key).unwrap_or_else(|| panic!("no key {key}")) as u32;
            assert_eq!(t.detail_level, v("TerrainDetailLevel"), "{p:?}");
            assert_eq!(t.tess_level, v("TerrainTessLevel"), "{p:?}");
            assert_eq!(t.shadow_quality, v("TerrainShadowQuality"), "{p:?}");
            assert_eq!(t.pagein_still, v("TerrainPageinSpeedStill"), "{p:?}");
            assert_eq!(t.pagein_moving, v("TerrainPageinSpeedMoving"), "{p:?}");
            assert_eq!(t.chunks[0], v("Terrain64Chunks"), "{p:?}");
            assert_eq!(t.chunks[1], v("Terrain128Chunks"), "{p:?}");
            assert_eq!(t.chunks[2], v("Terrain256Chunks"), "{p:?}");
            assert_eq!(t.chunks[3], v("Terrain512Chunks"), "{p:?}");
            assert_eq!(t.bicubic, v("BicubicTerrainTessellation") != 0, "{p:?}");
            assert_eq!(t.tess_tri_size, v("BicubicTerrainTessTriSize"), "{p:?}");
            assert_eq!(t.tess_subdiv, v("BicubicTerrainTessSubdiv"), "{p:?}");
            assert_eq!(t.minimize_gray_tiles, v("MinimizeGrayTiles") != 0, "{p:?}");
            assert_eq!(t.overlay_level, v("OverlayLevel"), "{p:?}");
            assert_eq!(
                t.screen_space_overlay,
                v("Use Screen Space Overlay") != 0,
                "{p:?}"
            );
        }
    }

    #[test]
    fn the_presets_are_ordered_low_to_high() {
        let read = |p| TerrainSettings::from_settings(&RenderSettings::preset(p));
        let (low, medium, high) = (read(Preset::Low), read(Preset::Medium), read(Preset::High));
        let ordered = |a: u32, b: u32, c: u32| a <= b && b <= c;
        assert!(ordered(
            low.detail_level,
            medium.detail_level,
            high.detail_level
        ));
        assert!(ordered(
            low.shadow_quality,
            medium.shadow_quality,
            high.shadow_quality
        ));
        assert!(ordered(low.tess_level, medium.tess_level, high.tess_level));
        for i in 0..4 {
            assert!(
                ordered(low.chunks[i], medium.chunks[i], high.chunks[i]),
                "chunk limit {i}"
            );
        }
    }

    #[test]
    fn each_key_changes_only_its_own_field() {
        let high_settings = RenderSettings::preset(Preset::High);
        let base = TerrainSettings::from_settings(&high_settings);
        let with = |f: fn(&mut TerrainSettings)| {
            let mut t = base.clone();
            f(&mut t);
            t
        };
        // (key, a value other than High's, the struct with only that field replaced)
        let cases: [(&str, i64, TerrainSettings); 15] = [
            ("TerrainDetailLevel", 0, with(|t| t.detail_level = 0)),
            ("TerrainTessLevel", 1, with(|t| t.tess_level = 1)),
            ("TerrainShadowQuality", 0, with(|t| t.shadow_quality = 0)),
            ("TerrainPageinSpeedStill", 5, with(|t| t.pagein_still = 5)),
            ("TerrainPageinSpeedMoving", 7, with(|t| t.pagein_moving = 7)),
            ("Terrain64Chunks", 100, with(|t| t.chunks[0] = 100)),
            ("Terrain128Chunks", 100, with(|t| t.chunks[1] = 100)),
            ("Terrain256Chunks", 100, with(|t| t.chunks[2] = 100)),
            ("Terrain512Chunks", 100, with(|t| t.chunks[3] = 100)),
            ("BicubicTerrainTessellation", 0, with(|t| t.bicubic = false)),
            (
                "BicubicTerrainTessTriSize",
                8,
                with(|t| t.tess_tri_size = 8),
            ),
            ("BicubicTerrainTessSubdiv", 4, with(|t| t.tess_subdiv = 4)),
            (
                "MinimizeGrayTiles",
                1,
                with(|t| t.minimize_gray_tiles = true),
            ),
            ("OverlayLevel", 0, with(|t| t.overlay_level = 0)),
            (
                "Use Screen Space Overlay",
                0,
                with(|t| t.screen_space_overlay = false),
            ),
        ];
        for (key, value, expected) in cases {
            let mut s = high_settings.clone();
            s.set(key, value).unwrap();
            assert_eq!(
                TerrainSettings::from_settings(&s),
                expected,
                "{key} = {value}"
            );
        }
    }

    #[test]
    fn switching_preset_is_a_pure_function_of_the_settings() {
        let high = RenderSettings::preset(Preset::High);
        let first = TerrainSettings::from_settings(&high);
        assert_eq!(TerrainSettings::from_settings(&high), first);
        // High -> Low -> High: nothing of Low is carried over (AC-R8.1).
        let low = TerrainSettings::from_settings(&RenderSettings::preset(Preset::Low));
        assert_ne!(low, first);
        let back = TerrainSettings::from_settings(&RenderSettings::preset(Preset::High));
        assert_eq!(back, first);
    }
}
