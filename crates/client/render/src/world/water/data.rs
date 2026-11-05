//! What the water needs from the game's files: the textures of the
//! group `water` and the settings of the coast, the waves and the ice (`C5WS`). The client does
//! not decode anything: the textures arrive as `C5TX` blobs and are uploaded raw; the LEAN maps are
//! decoded by the pipeline (WW-D7) into `Rgba16F` (the mean slopes and moments) and `R16F` (the
//! cross moment).

use std::collections::BTreeMap;

use asset_format::{
    CoastSettings, IceSettings, LeanInfo, TextureBlob, WaveSettings, WorldSettings,
};

/// The LEAN map of the ocean: `Rgba16F`, `LEAN1_1` of the shaders.
// re: WaterSystem::Startup (0x0868ae38): `FLeanMap::DeSerializeFromMemory("waterbumps16.lean")`
pub const LEAN_BUMPS: &str = "waterbumps16.lean";

/// Its second texture: `R16F`, `LEAN1_2`.
pub const LEAN_BUMPS_CROSS: &str = "waterbumps16.lean#cross";

/// The ramp of the colour of the water by depth (256 x 4), `WaterColor`.
// re: WaterSystem::Startup: `LoadTexture("waterdepthcolor_new.dds")` (the old file is in the pack too)
pub const WATER_COLOR: &str = "waterdepthcolor_new.dds";

/// The old file of the same ramp, taken where `_new` is not in the pack.
pub const WATER_COLOR_OLD: &str = "waterdepthcolor.dds";

/// The sky of the water (2 x 4): `LoadSunSky` reads the colours of the lights from it (RW-4 b).
pub const WATER_SKY: &str = "watersky.dds";

/// The mask of the marsh (128 x 128 L8, `R8` in the pack): the stage `Marsh` stamps it into the ids.
// re: cvMarshSimSystem::Startup 0x087e6af2 (`Format("marsh_stamp.dds")`)
pub const MARSH_STAMP: &str = "marsh_stamp.dds";

/// The colour map of the rivers (64 x 16), `WaterColor` of `PSRiverHigh`; the original reads it through an sRGB
/// view (S1a ev 1104, t6: R8G8B8A8_UNORM_SRGB).
// re: WaterSystem::Startup: `LoadTexture("river_diffuse_new.dds")`
pub const RIVER_COLOR: &str = "river_diffuse_new.dds";

/// The blend map of the rivers (128 x 32), `WaterBlendMap` of `PSRiverHigh` (S1a ev 1104, t3): its alpha is the
/// opacity of the river, high in the middle, fading to the banks and the ends.
// re: WaterSystem::Startup: `LoadTexture("river_alpha_new.dds")`
pub const RIVER_BLEND: &str = "river_alpha_new.dds";

/// The crest atlas of the surf (`g_WaveTexture`).
// re: cvWaveSystem (rw-a6-answers RW-22 (a) 4: "Wave_Coastline_ALL.dds")
pub const WAVE_CREST: &str = "wave_coastline_all.dds";

/// The textures of the ice (rw-a6-answers RW-22 (b) 4: surface slots 0 .. 6, and the
/// displacement map of the mesh).
pub const ICE_CUBE: &str = "ice_environment_diffuse.dds";
pub const ICE_DIFFUSE: &str = "ice_d.dds";
pub const ICE_DIFFUSE_SIDES: &str = "ice_d_sides.dds";
pub const ICE_BUMP: &str = "ice_bump.lean";
pub const ICE_BUMP_CROSS: &str = "ice_bump.lean#cross";
pub const ICE_BUMP_SIDES: &str = "ice_bump_sides.lean";
pub const ICE_BUMP_SIDES_CROSS: &str = "ice_bump_sides.lean#cross";
pub const ICE_DISPLACEMENT: &str = "icedisplacement.dds";

/// The textures the ice cannot be drawn without (the displacement only shapes the mesh).
pub const ICE_FILES: [&str; 6] = [
    ICE_CUBE,
    ICE_DIFFUSE,
    ICE_DIFFUSE_SIDES,
    ICE_BUMP,
    ICE_BUMP_SIDES,
    ICE_BUMP_SIDES_CROSS,
];

/// The textures of the group `water` the subsystems read, as `tx:<file>` in the pack: the client
/// asks for these (the rest of the group is for the river banks).
pub const PACK_FILES: [&str; 17] = [
    LEAN_BUMPS,
    LEAN_BUMPS_CROSS,
    WATER_COLOR,
    WATER_COLOR_OLD,
    WATER_SKY,
    MARSH_STAMP,
    RIVER_COLOR,
    RIVER_BLEND,
    WAVE_CREST,
    ICE_CUBE,
    ICE_DIFFUSE,
    ICE_DIFFUSE_SIDES,
    ICE_BUMP,
    ICE_BUMP_CROSS,
    ICE_BUMP_SIDES,
    ICE_BUMP_SIDES_CROSS,
    ICE_DISPLACEMENT,
];

/// The files the ocean cannot be drawn without, in the order they are listed in the report.
pub const OCEAN_FILES: [&str; 2] = [LEAN_BUMPS, LEAN_BUMPS_CROSS];

/// What the water takes from the pack and the settings. Cheap to share: the subsystem holds an
/// `Arc`.
#[derive(Clone, Debug, Default)]
pub struct WaterData {
    /// By lower case file name (`waterdepthcolor_new.dds`, `waterbumps16.lean#cross`).
    textures: BTreeMap<String, TextureBlob>,
    coast: CoastSettings,
    waves: WaveSettings,
    ice: IceSettings,
    /// The headers of the `.lean` files (`C5WS.lean`), for their `-output_scale`.
    lean: Vec<LeanInfo>,
    has_settings: bool,
    /// Grows with every change, so the subsystem sees that it has to upload again.
    revision: u64,
}

impl WaterData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the texture of `file` (any case).
    pub fn add_texture(&mut self, file: &str, blob: TextureBlob) {
        self.textures.insert(file.to_ascii_lowercase(), blob);
        self.revision += 1;
    }

    /// Takes the settings of the coast, the waves and the ice out of `tb:world-b2`.
    pub fn set_settings(&mut self, settings: &WorldSettings) {
        self.coast = settings.coast.clone();
        self.waves = settings.waves.clone();
        self.ice = settings.ice.clone();
        self.lean = settings.lean.clone();
        self.has_settings = true;
        self.revision += 1;
    }

    pub fn texture(&self, file: &str) -> Option<&TextureBlob> {
        self.textures.get(&file.to_ascii_lowercase())
    }

    /// The ramp of the colours: the `_new` file where it is there, else the old one.
    pub fn water_color(&self) -> Option<&TextureBlob> {
        self.texture(WATER_COLOR)
            .or_else(|| self.texture(WATER_COLOR_OLD))
    }

    pub fn coast(&self) -> &CoastSettings {
        &self.coast
    }

    pub fn waves(&self) -> &WaveSettings {
        &self.waves
    }

    pub fn ice(&self) -> &IceSettings {
        &self.ice
    }

    pub fn has_settings(&self) -> bool {
        self.has_settings
    }

    /// The `-output_scale` of a `.lean` file of the pack (any case), when its header is in.
    pub fn lean_scale(&self, file: &str) -> Option<f32> {
        self.lean
            .iter()
            .find(|l| l.file.eq_ignore_ascii_case(file))
            .map(|l| l.scale)
    }

    /// The textures of the ice that have not arrived yet.
    pub fn missing_for_ice(&self) -> Vec<&'static str> {
        ICE_FILES
            .into_iter()
            .filter(|f| self.texture(f).is_none())
            .collect()
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The files of the ocean that have not arrived yet.
    pub fn missing_for_ocean(&self) -> Vec<&'static str> {
        let mut missing: Vec<&'static str> = OCEAN_FILES
            .into_iter()
            .filter(|f| self.texture(f).is_none())
            .collect();
        if self.water_color().is_none() {
            missing.push(WATER_COLOR);
        }
        missing
    }

    /// The ocean can be drawn: both LEAN textures and the ramp of colours are in.
    pub fn ocean_ready(&self) -> bool {
        self.missing_for_ocean().is_empty()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use asset_format::{TexDim, TextureFormat};

    use super::*;

    /// A texture of `w` x `h` texels of `bytes` bytes each, every byte `fill`.
    pub(crate) fn blob(
        format: TextureFormat,
        w: u32,
        h: u32,
        bytes: usize,
        fill: u8,
    ) -> TextureBlob {
        TextureBlob {
            format,
            dim: TexDim::D2,
            srgb: false,
            width: w,
            height: h,
            depth: 1,
            mips: vec![vec![fill; (w * h) as usize * bytes]],
        }
    }

    /// The textures the ocean needs, small.
    pub(crate) fn ocean_data() -> WaterData {
        let mut d = WaterData::new();
        d.add_texture(LEAN_BUMPS, blob(TextureFormat::Rgba16F, 4, 4, 8, 0));
        d.add_texture(LEAN_BUMPS_CROSS, blob(TextureFormat::R16F, 4, 4, 2, 0));
        d.add_texture(WATER_COLOR, blob(TextureFormat::Rgba8, 8, 4, 4, 128));
        d
    }

    #[test]
    fn the_ocean_waits_for_its_three_textures() {
        let mut d = WaterData::new();
        assert!(!d.ocean_ready());
        assert_eq!(
            d.missing_for_ocean(),
            [LEAN_BUMPS, LEAN_BUMPS_CROSS, WATER_COLOR]
        );
        d.add_texture(
            "WaterBumps16.lean",
            blob(TextureFormat::Rgba16F, 2, 2, 8, 0),
        );
        d.add_texture(LEAN_BUMPS_CROSS, blob(TextureFormat::R16F, 2, 2, 2, 0));
        assert_eq!(d.missing_for_ocean(), [WATER_COLOR]);
        // The old file stands in for the new one.
        d.add_texture(WATER_COLOR_OLD, blob(TextureFormat::Rgba8, 8, 4, 4, 1));
        assert!(d.ocean_ready());
        assert!(d.water_color().is_some());
    }

    #[test]
    fn a_change_of_the_data_moves_the_revision() {
        let mut d = WaterData::new();
        let r0 = d.revision();
        d.add_texture(WATER_SKY, blob(TextureFormat::Rgba8, 2, 4, 4, 0));
        assert!(d.revision() > r0);
        assert!(!d.has_settings());
        d.set_settings(&WorldSettings::default());
        assert!(d.has_settings());
    }
}
