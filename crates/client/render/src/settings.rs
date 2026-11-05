//! Quality settings with the keys of the original. No GPU, no
//! dependencies: a plain struct, its ini text (`GraphicsSettingsDX11.ini`, and the one key of
//! `config.ini`), the presets and the set of keys that changed between two states.
//!
//! The table below is the single source of the key names, sections, ranges, the three presets and
//! the subsystems that read each key. `Renderer::set_settings` calls `on_settings` only on the
//! subsystems that read a changed key.

use std::fmt;

use crate::frame::SystemId;

/// Sections of the ini files the keys live in.
pub const SECTION_DETAIL: &str = "GraphicsDetailSettings";
pub const SECTION_USER: &str = "UserSettings";
/// `config.ini` (the original spells it with a space).
pub const SECTION_CONFIG_USER: &str = "User Settings";

/// Samples of the main pass above one.
// SIMPLIFIED(SIMP-033): MSAA 4x instead of 8x. The user's ini says 8x; WebGPU guarantees only 1
// and 4 samples (R-D3, AC-R6.3), so every request of 2 or more gets 4.
const MSAA_ON: u32 = 4;

/// The quality levels of the settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    Low,
    Medium,
    High,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Low, Preset::Medium, Preset::High];

    /// `low`, `medium` or `high`, any case.
    pub fn from_name(name: &str) -> Option<Preset> {
        let name = name.trim();
        Preset::ALL
            .into_iter()
            .find(|p| p.name().eq_ignore_ascii_case(name))
    }

    pub fn name(self) -> &'static str {
        match self {
            Preset::Low => "low",
            Preset::Medium => "medium",
            Preset::High => "high",
        }
    }
}

/// One key of the table: its place in the ini, what it may hold and who reads it.
#[derive(Debug)]
pub struct KeyDef {
    /// Name of the key in the ini of the original, exactly.
    pub name: &'static str,
    pub section: &'static str,
    pub min: u32,
    pub max: u32,
    /// The only values allowed inside `min..=max`; empty means any.
    pub allowed: &'static [u32],
    /// The subsystems that read it (`on_settings` goes to these).
    pub readers: &'static [SystemId],
    /// Read outside the subsystems: `render-core` itself or the asset loader.
    pub other_reader: Option<&'static str>,
    /// Nothing will read it, and why.
    pub unused: Option<&'static str>,
}

/// The keys that differ between two [`RenderSettings`] (a bit per entry of [`KEYS`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeySet(u64);

impl KeySet {
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    pub fn len(&self) -> usize {
        self.0.count_ones() as usize
    }

    /// Whether the key `name` (any case) is in the set.
    pub fn contains(&self, name: &str) -> bool {
        key_index(name).is_some_and(|i| self.0 & (1 << i) != 0)
    }

    pub fn intersects(&self, other: &KeySet) -> bool {
        self.0 & other.0 != 0
    }

    pub fn insert(&mut self, name: &str) {
        if let Some(i) = key_index(name) {
            self.0 |= 1 << i;
        }
    }

    /// The names in the set, in table order.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        KEYS.iter()
            .enumerate()
            .filter(|(i, _)| self.0 & (1 << i) != 0)
            .map(|(_, k)| k.name)
    }
}

impl FromIterator<&'static str> for KeySet {
    fn from_iter<T: IntoIterator<Item = &'static str>>(iter: T) -> Self {
        let mut set = KeySet::default();
        for name in iter {
            set.insert(name);
        }
        set
    }
}

/// The keys the subsystem `id` reads.
pub fn readers_of(id: SystemId) -> KeySet {
    let mut set = KeySet::default();
    for (i, k) in KEYS.iter().enumerate() {
        if k.readers.contains(&id) {
            set.0 |= 1 << i;
        }
    }
    set
}

/// Index of the key `name` in [`KEYS`]; the case does not matter, spaces round it are dropped.
pub fn key_index(name: &str) -> Option<usize> {
    let name = name.trim();
    KEYS.iter().position(|k| k.name.eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    UnknownKey(String),
    OutOfRange {
        key: &'static str,
        value: i64,
        min: u32,
        max: u32,
    },
    NotAllowed {
        key: &'static str,
        value: i64,
        allowed: &'static [u32],
    },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::UnknownKey(k) => write!(f, "unknown key {k:?}"),
            SettingsError::OutOfRange {
                key,
                value,
                min,
                max,
            } => write!(f, "{key} = {value} is outside {min}..={max}"),
            SettingsError::NotAllowed {
                key,
                value,
                allowed,
            } => write!(f, "{key} = {value} is not one of {allowed:?}"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// A line of an ini text that could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IniWarning {
    /// One-based.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for IniWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

/// Generates the struct, [`KEYS`] and the presets from one table so they cannot drift apart.
/// Row: `field: "Key", section, min, max, allowed, [readers], other reader, unused reason,
/// high, medium, low;`.
macro_rules! settings_table {
    ($(
        $field:ident: $name:literal, $section:expr, $min:expr, $max:expr, $allowed:expr,
        [$($reader:ident),*], $other:expr, $unused:expr, $high:expr, $medium:expr, $low:expr;
    )*) => {
        /// The quality keys of the original, named like their fields in snake case. The table in
        /// the spec (4.3) gives the three presets; [`RenderSettings::default`] is High.
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub struct RenderSettings {
            $(pub $field: u32,)*
        }

        /// Every key, in the order of the ini text and of the bits of [`KeySet`].
        pub const KEYS: &[KeyDef] = &[
            $(KeyDef {
                name: $name,
                section: $section,
                min: $min,
                max: $max,
                allowed: $allowed,
                readers: &[$(SystemId::$reader),*],
                other_reader: $other,
                unused: $unused,
            },)*
        ];

        impl RenderSettings {
            fn to_array(&self) -> Vec<u32> {
                vec![$(self.$field),*]
            }

            fn from_array(values: &[u32]) -> Self {
                let mut it = values.iter().copied();
                Self { $($field: it.next().expect("one value per key"),)* }
            }

            /// The values of a preset.
            pub fn preset(p: Preset) -> Self {
                match p {
                    Preset::High => Self { $($field: $high,)* },
                    Preset::Medium => Self { $($field: $medium,)* },
                    Preset::Low => Self { $($field: $low,)* },
                }
            }
        }
    };
}

const NONE: &[u32] = &[];
const CORE: Option<&str> = Some("render-core");
const LOADER: Option<&str> = Some("asset loader (client-app)");
const NO_TESSELLATION: Option<&str> =
    Some("no tessellation stage on WebGL2: world-terrain bakes the patches");

// re: GraphicsSettingsDX11.ini of the user, High (2026-10-21).
// SIMPLIFIED(SIMP-034): Low and Medium take the labels of the original's options menu
// (`Assets/UI/Options/OptionsMenu.lua:564-605`), and where a key has no label the values of the
// user's `GraphicsSettingsDX9.ini`. The exe has a table of graphics profiles (RW-7: six, 0..5);
// when `re-world` has read it into this table the presets become the original's profiles.
// The Medium and Low columns below are those presets.
settings_table! {
    overlay_level: "OverlayLevel", SECTION_DETAIL, 0, 2, NONE,
        [Overlay, Terrain], None, None, 2, 1, 0;
    shadow_level: "ShadowLevel", SECTION_DETAIL, 0, 3, NONE,
        [Shadows, Unit, City, Forest, Landmark], CORE, None, 3, 2, 0;
    reflection_level: "ReflectionLevel", SECTION_DETAIL, 0, 1, NONE,
        [Water, Ice], CORE, None, 1, 0, 0;
    texture_quality: "TextureQuality", SECTION_DETAIL, 0, 1, NONE,
        [], LOADER, None, 1, 1, 0;
    fow_level: "FOWLevel", SECTION_DETAIL, 0, 3, NONE,
        [FluidFow, FowClassic], None, None, 3, 2, 1;
    terrain_detail_level: "TerrainDetailLevel", SECTION_DETAIL, 0, 3, NONE,
        [Terrain], None, None, 3, 2, 1;
    terrain_tess_level: "TerrainTessLevel", SECTION_DETAIL, 0, 2, NONE,
        [Terrain, River], None, None, 2, 1, 0;
    terrain_shadow_quality: "TerrainShadowQuality", SECTION_DETAIL, 0, 3, NONE,
        [Terrain], None, None, 3, 2, 1;
    terrain_water_quality: "TerrainWaterQuality", SECTION_DETAIL, 0, 2, NONE,
        [Water, River], None, None, 2, 1, 0;
    terrain_pagein_speed_still: "TerrainPageinSpeedStill", SECTION_DETAIL, 0, 16, NONE,
        [Terrain], None, None, 2, 2, 1;
    terrain_pagein_speed_moving: "TerrainPageinSpeedMoving", SECTION_DETAIL, 0, 16, NONE,
        [Terrain], None, None, 1, 1, 0;
    terrain_64_chunks: "Terrain64Chunks", SECTION_DETAIL, 0, 65536, NONE,
        [Terrain], None, None, 4096, 2048, 2048;
    terrain_128_chunks: "Terrain128Chunks", SECTION_DETAIL, 0, 65536, NONE,
        [Terrain], None, None, 512, 256, 256;
    terrain_256_chunks: "Terrain256Chunks", SECTION_DETAIL, 0, 65536, NONE,
        [Terrain], None, None, 256, 128, 128;
    terrain_512_chunks: "Terrain512Chunks", SECTION_DETAIL, 0, 65536, NONE,
        [Terrain], None, None, 64, 64, 64;
    bicubic_terrain_tessellation: "BicubicTerrainTessellation", SECTION_DETAIL, 0, 1, NONE,
        [Terrain], None, NO_TESSELLATION, 1, 0, 0;
    bicubic_terrain_tess_tri_size: "BicubicTerrainTessTriSize", SECTION_DETAIL, 1, 64, NONE,
        [Terrain], None, NO_TESSELLATION, 4, 4, 4;
    bicubic_terrain_tess_subdiv: "BicubicTerrainTessSubdiv", SECTION_DETAIL, 0, 8, NONE,
        [Terrain], None, NO_TESSELLATION, 2, 2, 2;
    allow_cheap_dx11_shadow_blur: "AllowCheapDX11ShadowBlur", SECTION_DETAIL, 0, 1, NONE,
        [Shadows], None, None, 1, 1, 1;
    fade_shadows: "FadeShadows", SECTION_DETAIL, 0, 1, NONE,
        [Shadows], None, None, 1, 1, 1;
    minimize_gray_tiles: "MinimizeGrayTiles", SECTION_DETAIL, 0, 1, NONE,
        [Terrain], None, None, 0, 0, 0;
    msaa_samples: "MSAASamples", SECTION_USER, 0, 8, &[0, 1, 2, 4, 8],
        [], CORE, None, 4, 4, 1;
    use_screen_space_overlay: "Use Screen Space Overlay", SECTION_CONFIG_USER, 0, 1, NONE,
        [Overlay, Terrain], None, None, 1, 1, 1;
    // Ours, not a key of the original: the far-view LOD (`world::far_lod`). The original's ini has
    // no such key, and the original ignores keys it does not know. Off on High: the far view
    // changes visibly (golden S5: FLIP 0.055 on the forest) for little GPU time once the trees pull
    // their vertices (B3, 2026-10-29).
    far_lod: "FarLOD", SECTION_DETAIL, 0, 1, NONE,
        [], CORE, None, 0, 1, 1;
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self::preset(Preset::High)
    }
}

impl RenderSettings {
    /// Samples the main pass really gets: 4 when the ini asks for 2 or more, else 1 (SIMP-033).
    /// The ini keeps what the player chose; the log says what is in effect.
    pub fn effective_msaa(&self) -> u32 {
        if self.msaa_samples >= 2 { MSAA_ON } else { 1 }
    }

    /// The value of `key` (any case), or `None` for a name that is not a key.
    pub fn get(&self, key: &str) -> Option<i64> {
        let i = key_index(key)?;
        Some(i64::from(self.to_array()[i]))
    }

    /// Sets `key` (any case) after checking the range of the original.
    pub fn set(&mut self, key: &str, v: i64) -> Result<(), SettingsError> {
        let i = key_index(key).ok_or_else(|| SettingsError::UnknownKey(key.trim().to_owned()))?;
        let def = &KEYS[i];
        if v < i64::from(def.min) || v > i64::from(def.max) {
            return Err(SettingsError::OutOfRange {
                key: def.name,
                value: v,
                min: def.min,
                max: def.max,
            });
        }
        let v = v as u32;
        if !def.allowed.is_empty() && !def.allowed.contains(&v) {
            return Err(SettingsError::NotAllowed {
                key: def.name,
                value: i64::from(v),
                allowed: def.allowed,
            });
        }
        let mut values = self.to_array();
        values[i] = v;
        *self = Self::from_array(&values);
        Ok(())
    }

    /// The keys whose values differ between `self` and `other`.
    pub fn changed(&self, other: &Self) -> KeySet {
        let (a, b) = (self.to_array(), other.to_array());
        let mut set = KeySet::default();
        for i in 0..a.len() {
            if a[i] != b[i] {
                set.0 |= 1 << i;
            }
        }
        set
    }

    /// Reads the keys out of the text of `GraphicsSettingsDX11.ini` (or `config.ini`: the key of
    /// the screen-space overlay is there). A key counts only inside its own section; keys that are
    /// not ours (the file has many) are skipped quietly. Missing keys keep the High values; a bad
    /// value is a warning and keeps the default too.
    pub fn from_ini(text: &str) -> (Self, Vec<IniWarning>) {
        let mut out = Self::default();
        let mut warnings = Vec::new();
        let mut section = String::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                section = rest.split(']').next().unwrap_or("").trim().to_owned();
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let Some(i) = key_index(key) else { continue };
            if !KEYS[i].section.eq_ignore_ascii_case(&section) {
                continue;
            }
            let value = value.split(';').next().unwrap_or("").trim();
            let parsed = value.parse::<i64>();
            let result = match parsed {
                Ok(v) => out.set(KEYS[i].name, v).map_err(|e| e.to_string()),
                Err(_) => Err(format!("{} = {value:?} is not a number", KEYS[i].name)),
            };
            if let Err(message) = result {
                warnings.push(IniWarning {
                    line: n + 1,
                    message,
                });
            }
        }
        (out, warnings)
    }

    /// The ini text: the three sections of the keys, every key with its value.
    pub fn to_ini(&self) -> String {
        let values = self.to_array();
        let mut text = String::new();
        for section in [SECTION_DETAIL, SECTION_USER, SECTION_CONFIG_USER] {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("[{section}]\n\n"));
            for (def, v) in KEYS.iter().zip(&values) {
                if def.section == section {
                    text.push_str(&format!("{} = {v}\n", def.name));
                }
            }
        }
        text
    }

    /// Applies `Key:value` pairs on top of the current values (`?gfx=ShadowLevel:0,FOWLevel:1`,
    /// `--gfx`); `=` works instead of `:`. Words without a value are backend names (`webgl2`,
    /// `gl`, `auto`) and belong to the caller. Returns what could not be applied.
    pub fn apply_overrides(&mut self, text: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        for token in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let Some((key, value)) = token.split_once([':', '=']) else {
                if !is_backend_name(token) {
                    warnings.push(format!("{token:?} is neither Key:value nor a backend"));
                }
                continue;
            };
            match value.trim().parse::<i64>() {
                Ok(v) => {
                    if let Err(e) = self.set(key, v) {
                        warnings.push(e.to_string());
                    }
                }
                Err(_) => warnings.push(format!("{token:?}: the value is not a number")),
            }
        }
        warnings
    }
}

/// The words `?gfx=` and `--gfx` take for the backend.
pub fn is_backend_name(word: &str) -> bool {
    ["auto", "gl", "webgl", "webgl2"]
        .iter()
        .any(|b| b.eq_ignore_ascii_case(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The user's `GraphicsSettingsDX11.ini`, as far as our keys go (read 2026-10-21).
    const USER_INI: &str = "\
[LeaderheadSettings]
LeaderQuality = 3
EnableShadows = 1

[GraphicsDetailSettings]

HDStrategicView = 1
OverlayLevel = 2
ShadowLevel = 3
ReflectionLevel = 1
TextureQuality = 1
FOWLevel = 3
TerrainDetailLevel = 3
TerrainTessLevel = 2
TerrainShadowQuality = 3
TerrainWaterQuality = 2
; Terrain Page in Rate
TerrainPageinSpeedStill = 2
TerrainPageinSpeedMoving = 1
MinimizeGrayTiles = 0
FadeShadows = 1
Terrain64Chunks = 4096
Terrain128Chunks = 512
Terrain256Chunks = 256
Terrain512Chunks = 64
BicubicTerrainTessellation = 1
BicubicTerrainTessTriSize = 4
BicubicTerrainTessSubdiv = 2
AllowCheapDX11ShadowBlur = 1

[UserSettings]
MSAASamples = 8
WaitForVSync = 1
";

    #[test]
    fn every_key_has_its_exact_name_section_and_is_unique() {
        // The names of the original (table 4.3 and the ini of the user), in order.
        let names: Vec<&str> = KEYS.iter().map(|k| k.name).collect();
        assert_eq!(
            names,
            [
                "OverlayLevel",
                "ShadowLevel",
                "ReflectionLevel",
                "TextureQuality",
                "FOWLevel",
                "TerrainDetailLevel",
                "TerrainTessLevel",
                "TerrainShadowQuality",
                "TerrainWaterQuality",
                "TerrainPageinSpeedStill",
                "TerrainPageinSpeedMoving",
                "Terrain64Chunks",
                "Terrain128Chunks",
                "Terrain256Chunks",
                "Terrain512Chunks",
                "BicubicTerrainTessellation",
                "BicubicTerrainTessTriSize",
                "BicubicTerrainTessSubdiv",
                "AllowCheapDX11ShadowBlur",
                "FadeShadows",
                "MinimizeGrayTiles",
                "MSAASamples",
                "Use Screen Space Overlay",
                "FarLOD",
            ]
        );
        for k in KEYS {
            let section = match k.name {
                "MSAASamples" => SECTION_USER,
                "Use Screen Space Overlay" => SECTION_CONFIG_USER,
                _ => SECTION_DETAIL,
            };
            assert_eq!(k.section, section, "{}", k.name);
            assert!(k.min <= k.max);
            assert!(
                k.readers.len() + usize::from(k.other_reader.is_some()) > 0 || k.unused.is_some()
            );
        }
        assert!(KEYS.len() <= 64, "KeySet is a u64");
    }

    #[test]
    fn presets_are_the_table() {
        let row = |s: &RenderSettings| {
            [
                s.overlay_level,
                s.shadow_level,
                s.reflection_level,
                s.texture_quality,
                s.fow_level,
                s.terrain_detail_level,
                s.terrain_tess_level,
                s.terrain_shadow_quality,
                s.terrain_water_quality,
                s.terrain_pagein_speed_still,
                s.terrain_pagein_speed_moving,
                s.terrain_64_chunks,
                s.terrain_128_chunks,
                s.terrain_256_chunks,
                s.terrain_512_chunks,
                s.bicubic_terrain_tessellation,
                s.bicubic_terrain_tess_tri_size,
                s.bicubic_terrain_tess_subdiv,
                s.allow_cheap_dx11_shadow_blur,
                s.fade_shadows,
                s.minimize_gray_tiles,
                s.msaa_samples,
                s.use_screen_space_overlay,
                s.far_lod,
            ]
        };
        assert_eq!(
            row(&RenderSettings::preset(Preset::High)),
            [
                2, 3, 1, 1, 3, 3, 2, 3, 2, 2, 1, 4096, 512, 256, 64, 1, 4, 2, 1, 1, 0, 4, 1, 0
            ]
        );
        assert_eq!(
            row(&RenderSettings::preset(Preset::Medium)),
            [
                1, 2, 0, 1, 2, 2, 1, 2, 1, 2, 1, 2048, 256, 128, 64, 0, 4, 2, 1, 1, 0, 4, 1, 1
            ]
        );
        assert_eq!(
            row(&RenderSettings::preset(Preset::Low)),
            [
                0, 0, 0, 0, 1, 1, 0, 1, 0, 1, 0, 2048, 256, 128, 64, 0, 4, 2, 1, 1, 0, 1, 1, 1
            ]
        );
        assert_eq!(
            RenderSettings::default(),
            RenderSettings::preset(Preset::High)
        );
        // Low is AC-R8.1: no reflections, no shadows, FOW level 1, no MSAA.
        let low = RenderSettings::preset(Preset::Low);
        assert_eq!(
            (low.reflection_level, low.shadow_level, low.fow_level),
            (0, 0, 1)
        );
        assert_eq!(low.effective_msaa(), 1);
    }

    #[test]
    fn preset_names_parse_in_any_case() {
        assert_eq!(Preset::from_name("Low"), Some(Preset::Low));
        assert_eq!(Preset::from_name(" HIGH "), Some(Preset::High));
        assert_eq!(Preset::from_name("ultra"), None);
        for p in Preset::ALL {
            assert_eq!(Preset::from_name(p.name()), Some(p));
        }
    }

    /// AC-R6.3: whatever the ini asks for, the main pass gets 1 or 4 samples.
    #[test]
    fn effective_msaa_is_one_or_four() {
        for (ini, effective) in [(0, 1), (1, 1), (2, 4), (4, 4), (8, 4)] {
            let mut s = RenderSettings::default();
            s.set("MSAASamples", ini).unwrap();
            assert_eq!(s.effective_msaa(), effective, "MSAASamples = {ini}");
        }
        assert_eq!(RenderSettings::preset(Preset::High).effective_msaa(), 4);
        assert_eq!(RenderSettings::preset(Preset::Medium).effective_msaa(), 4);
        assert_eq!(RenderSettings::preset(Preset::Low).effective_msaa(), 1);
        assert!(RenderSettings::default().set("MSAASamples", 3).is_err());
        assert!(RenderSettings::default().set("MSAASamples", 16).is_err());
    }

    #[test]
    fn the_ini_of_the_user_reads_as_high_but_keeps_its_msaa() {
        let (s, warnings) = RenderSettings::from_ini(USER_INI);
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut high = RenderSettings::preset(Preset::High);
        high.msaa_samples = 8;
        assert_eq!(s, high);
        assert_eq!(s.effective_msaa(), 4);
    }

    #[test]
    fn ini_text_round_trips_with_exact_keys() {
        for p in Preset::ALL {
            let s = RenderSettings::preset(p);
            let text = s.to_ini();
            for k in KEYS {
                assert!(text.contains(&format!("{} = ", k.name)), "{}", k.name);
            }
            let (back, warnings) = RenderSettings::from_ini(&text);
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(back, s, "{p:?}");
        }
        let mut s = RenderSettings::default();
        s.set("MSAASamples", 8).unwrap();
        assert_eq!(RenderSettings::from_ini(&s.to_ini()).0.msaa_samples, 8);
    }

    #[test]
    fn a_key_counts_only_in_its_own_section() {
        // `ShadowLevel` under the leaderhead section is not ours; neither is `FOWLevel` outside.
        let (s, warnings) = RenderSettings::from_ini("[LeaderheadSettings]\nShadowLevel = 0\n");
        assert!(warnings.is_empty());
        assert_eq!(s, RenderSettings::default());
        // config.ini carries the screen-space overlay key under `[User Settings]`.
        let (s, _) = RenderSettings::from_ini("[User Settings]\nUse Screen Space Overlay = 0\n");
        assert_eq!(s.use_screen_space_overlay, 0);
    }

    #[test]
    fn bad_lines_warn_and_keep_the_default() {
        let text =
            "[GraphicsDetailSettings]\nShadowLevel = 9\nFOWLevel = lots\nOverlayLevel = 0 ; low\n";
        let (s, warnings) = RenderSettings::from_ini(text);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(warnings[0].line, 2);
        assert_eq!(warnings[1].line, 3);
        assert_eq!(s.shadow_level, 3);
        assert_eq!(s.fow_level, 3);
        assert_eq!(s.overlay_level, 0, "a trailing comment is cut off");
    }

    #[test]
    fn get_and_set_use_the_names_of_the_original() {
        let mut s = RenderSettings::default();
        assert_eq!(s.get("ShadowLevel"), Some(3));
        assert_eq!(s.get("shadowlevel"), Some(3));
        assert_eq!(s.get("Terrain64Chunks"), Some(4096));
        assert_eq!(s.get("Nope"), None);
        s.set("ShadowLevel", 1).unwrap();
        assert_eq!(s.shadow_level, 1);
        assert_eq!(
            s.set("ShadowLevel", 4),
            Err(SettingsError::OutOfRange {
                key: "ShadowLevel",
                value: 4,
                min: 0,
                max: 3
            })
        );
        assert!(s.set("ShadowLevel", -1).is_err());
        assert!(matches!(
            s.set("Bogus", 1),
            Err(SettingsError::UnknownKey(_))
        ));
        assert_eq!(s.shadow_level, 1, "a failed set changes nothing");
    }

    #[test]
    fn changed_lists_only_the_keys_that_differ() {
        let high = RenderSettings::preset(Preset::High);
        assert!(high.changed(&high.clone()).is_empty());
        let mut s = high.clone();
        s.set("ShadowLevel", 0).unwrap();
        s.set("MSAASamples", 1).unwrap();
        let changed = high.changed(&s);
        assert_eq!(
            changed.names().collect::<Vec<_>>(),
            ["ShadowLevel", "MSAASamples"]
        );
        assert!(changed.contains("shadowlevel"));
        assert!(!changed.contains("FOWLevel"));
        assert_eq!(changed.len(), 2);
        // Every key differs between High and Low somewhere, but not all of them.
        let low = RenderSettings::preset(Preset::Low);
        let all = high.changed(&low);
        assert!(all.contains("OverlayLevel") && !all.contains("MinimizeGrayTiles"));
    }

    #[test]
    fn readers_name_the_subsystems_of_each_key() {
        let shadows = readers_of(SystemId::Shadows);
        assert!(shadows.contains("ShadowLevel"));
        assert!(shadows.contains("FadeShadows"));
        assert!(!shadows.contains("FOWLevel"));
        assert!(readers_of(SystemId::FluidFow).contains("FOWLevel"));
        // No subsystem reads the MSAA key: render-core does.
        for id in SystemId::ALL {
            assert!(!readers_of(id).contains("MSAASamples"), "{id:?}");
        }
        assert_eq!(
            KEYS.iter()
                .find(|k| k.name == "MSAASamples")
                .unwrap()
                .other_reader,
            Some("render-core")
        );
        // A key nothing will read says why.
        let tess = KEYS
            .iter()
            .find(|k| k.name == "BicubicTerrainTessSubdiv")
            .unwrap();
        assert!(tess.unused.is_some());
    }

    #[test]
    fn overrides_go_on_top_and_bad_ones_warn() {
        let mut s = RenderSettings::preset(Preset::High);
        let warnings = s.apply_overrides("ShadowLevel:0, FOWLevel=1,webgl2,MSAASamples:1");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!((s.shadow_level, s.fow_level, s.msaa_samples), (0, 1, 1));
        let warnings = s.apply_overrides("ShadowLevel:7,Nope:1,OverlayLevel:x,typo");
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert_eq!(s.shadow_level, 0);
        assert!(s.apply_overrides("Use Screen Space Overlay:0").is_empty());
        assert_eq!(s.use_screen_space_overlay, 0);
    }
}
