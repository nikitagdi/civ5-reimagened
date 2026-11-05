//! The coast styles (WW-3a): `coaststyles.xml` and its nine character
//! profiles as the stages use them. The asset pipeline has read the XML into
//! [`asset_format::CoastSettings`]; this file holds the typed view that `LoadCoastDefinitions`
//! and `LoadSplineFromFile` of the original build: the five offset levels of a style (with the two
//! negations), the terrain types of the four strips between them, and the profiles smoothed and
//! stored in the frame of the tangent.

use asset_format::{COAST_NONE, CoastLevel, CoastProfile, CoastSettings};
use glam::Vec2;

use super::bspline;

/// Styles of `coaststyles.xml`, in the order of the names (`g_apszCoastStyleNames`); the index is
/// what the grid stores as the kind of a hex edge and what the control points carry.
// re: g_apszCoastStyleNames 0x0a2f2dd4 (cvCoastSplineSystem.cpp:647), LoadCoastDefinitions 0x088ee7f4
pub const STYLE_NAMES: [&str; STYLE_COUNT] = [
    "Default",
    "Grass",
    "GrassHill",
    "Plains",
    "PlainsHill",
    "Snow",
    "SnowHill",
    "Desert",
    "DesertHill",
    "Tundra",
    "TundraHill",
    "Mountain",
    "DefaultShallows",
];

/// Number of styles.
// re: LoadCoastDefinitions 0x088ee7f4 (`for (i = 0; i < 13; ++i)`)
pub const STYLE_COUNT: usize = 13;
/// Number of character profiles of `coaststyles.xml` (`SplineCharacterList`).
// re: coaststyles.xml (read)
pub const PROFILE_COUNT: usize = 9;
/// Samples a profile keeps: the file is read up to 98 lines (`0x61 < uVar9` stops the loop), the
/// asset of the game has 100.
// re: LoadSplineFromFile 0x088ef232 (`kNumSamples = 98`)
pub const PROFILE_SAMPLES: usize = 98;
/// Levels of a style: land side, high beach, waterline, low beach, sea side.
// re: GenerateLevels 0x088e7918 (`apOut[5]`)
pub const LEVELS: usize = 5;
/// Strips between the levels.
pub const STRIPS: usize = LEVELS - 1;

/// Index of the style of a hex edge kind (`CoastSplineHex::m_ucLandKind`).
pub mod kind {
    pub const DEFAULT: u8 = 0;
    pub const GRASS: u8 = 1;
    pub const GRASS_HILL: u8 = 2;
    pub const PLAINS: u8 = 3;
    pub const PLAINS_HILL: u8 = 4;
    pub const SNOW: u8 = 5;
    pub const SNOW_HILL: u8 = 6;
    pub const DESERT: u8 = 7;
    pub const DESERT_HILL: u8 = 8;
    pub const TUNDRA: u8 = 9;
    pub const TUNDRA_HILL: u8 = 10;
    pub const MOUNTAIN: u8 = 11;
    /// Water: the kind of a shelf edge and of a shore edge seen from the water.
    pub const SHALLOWS: u8 = 12;
}

/// A level of a style.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleLevel {
    /// Shift along the normal of the coastline, in texels; positive is toward the sea.
    pub offset: f32,
    /// 0 on the two outer levels and 1 on the inner three: the blend of the mesh.
    pub value: f32,
    /// Height as a fraction of 65535.
    pub height: f32,
    /// Index of the character profile, `None` for no character.
    pub character: Option<usize>,
}

impl StyleLevel {
    /// The height as the u16 of the field.
    pub fn height_raw(&self) -> u16 {
        (self.height * 65535.0).round().clamp(0.0, 65535.0) as u16
    }
}

/// A style of the coast: five levels and the terrain type of each of the four strips.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleDef {
    pub levels: [StyleLevel; LEVELS],
    /// Row id of `TerrainDef` per strip (`TerrainType`), `None` for no terrain: the first and the
    /// last strip never have one.
    pub terrain_type: [Option<u16>; STRIPS],
}

fn none_if_unset(v: u16) -> Option<u16> {
    (v != COAST_NONE).then_some(v)
}

fn character_of(level: &CoastLevel) -> Option<usize> {
    none_if_unset(level.character).map(usize::from)
}

impl StyleDef {
    /// The style a lookup that finds nothing hands out: the original value-initialises a
    /// `CoastStyleDef`, so every number is 0, the character index (0, not -1) and the terrain
    /// type (0, not -1) included.
    // re: Fun_088e96b4 0x088e96b4 (`CoastStyleDef kStyle = CoastStyleDef();`)
    pub fn zero() -> Self {
        Self {
            levels: [StyleLevel {
                character: Some(0),
                ..StyleLevel::default()
            }; LEVELS],
            terrain_type: [Some(0); STRIPS],
        }
    }

    /// The style of one `<CoastStyle>` element.
    // re: LoadCoastDefinitions 0x088ee7f4 (cvCoastSplineSystem.cpp:616-641): HighSide and
    // HighBeach.Offset are stored negated; the outer levels have value 0 and no character, and the
    // height of the beach on their side; `0xffff` is none
    fn from_settings(s: &asset_format::CoastStyle) -> Self {
        let level = |offset: f32, value: f32, height: f32, character: Option<usize>| StyleLevel {
            offset,
            value,
            height,
            character,
        };
        Self {
            levels: [
                level(-s.smoothing_high_side, 0.0, s.high_beach.height, None),
                level(
                    -s.high_beach.offset,
                    1.0,
                    s.high_beach.height,
                    character_of(&s.high_beach),
                ),
                level(
                    s.waterline.offset,
                    1.0,
                    s.waterline.height,
                    character_of(&s.waterline),
                ),
                level(
                    s.low_beach.offset,
                    1.0,
                    s.low_beach.height,
                    character_of(&s.low_beach),
                ),
                level(s.smoothing_low_side, 0.0, s.low_beach.height, None),
            ],
            terrain_type: [
                None,
                s.high_beach.terrain_type.and_then(none_if_unset),
                s.low_beach.terrain_type.and_then(none_if_unset),
                None,
            ],
        }
    }
}

/// The smoothed profile of a character, in the frame of the tangent: `(along, across)` per
/// sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub samples: Vec<Vec2>,
}

impl Profile {
    /// `LoadSplineFromFile`: the first 98 points of the file are smoothed down two levels and
    /// back, and the difference to the raw point is kept in the frame of the tangent of the
    /// smoothed curve.
    // re: cvCoastMeshLibrary::LoadSplineFromFile 0x088ef232 (cvCoastSplineSystem.cpp:473-527)
    pub fn from_points(points: &[[f32; 2]]) -> Self {
        let mut raw = vec![Vec2::ZERO; PROFILE_SAMPLES];
        for (r, p) in raw.iter_mut().zip(points) {
            *r = Vec2::new(p[0], p[1]);
        }
        // 98 -> 50 -> 26 -> 50 -> 98.
        let coarse = bspline::decompose(&bspline::decompose(&raw));
        let fine = bspline::reconstruct(&bspline::reconstruct(&coarse));
        debug_assert_eq!(fine.len(), PROFILE_SAMPLES);
        // aSmooth[0] and aSmooth[99] are the two extrapolated ends: `x` goes on by the first and
        // the last step, `y` is the last smoothed `y` on both.
        let step = fine[1].x - fine[0].x;
        let last = fine[PROFILE_SAMPLES - 1];
        let mut smooth = Vec::with_capacity(PROFILE_SAMPLES + 2);
        smooth.push(Vec2::new(fine[0].x - step, last.y));
        smooth.extend_from_slice(&fine);
        smooth.push(Vec2::new(last.x + step, last.y));
        let samples = (0..PROFILE_SAMPLES)
            .map(|i| {
                let delta = fine[i] - raw[i];
                let g = bspline::gradient(&smooth, i as f32 + 0.5);
                let len = (g.y * g.y + g.x * g.x).sqrt();
                let (tx, ty) = (g.x / len, g.y / len);
                Vec2::new(delta.y * ty + delta.x * tx, tx * delta.y - ty * delta.x)
            })
            .collect();
        Self { samples }
    }
}

/// The typed view of the coast data: the thirteen styles and the nine profiles.
#[derive(Clone, Debug, PartialEq)]
pub struct CoastStyles {
    /// One per name of [`STYLE_NAMES`]; a style the XML does not have is `None`.
    styles: [Option<StyleDef>; STYLE_COUNT],
    profiles: Vec<Profile>,
}

impl CoastStyles {
    /// The view of `coaststyles.xml` as the asset pipeline read it. A style is picked by its
    /// `type` name; of two with one name the later wins, as in the loop of the original.
    // re: LoadCoastDefinitions 0x088ee7f4 (`strcmp(kType, g_apszCoastStyleNames[i])`),
    // GenerateLevels' Fun_088e96b4 0x088e96b4 (`for (...) if (m_uiStyleIndex == uiStyle) kStyle = ...`)
    pub fn from_settings(settings: &CoastSettings) -> Self {
        let mut styles = [None; STYLE_COUNT];
        for s in &settings.styles {
            if let Some(i) = STYLE_NAMES.iter().position(|n| *n == s.name) {
                styles[i] = Some(StyleDef::from_settings(s));
            }
        }
        let profiles = settings
            .profiles
            .iter()
            .map(|p: &CoastProfile| Profile::from_points(&p.points))
            .collect();
        Self { styles, profiles }
    }

    /// The style `index` (a [`kind`]); `None` for a style the data does not have, which the
    /// original reads as an all-zero style ([`StyleDef::zero`]).
    pub fn style(&self, index: u8) -> Option<&StyleDef> {
        self.styles.get(usize::from(index))?.as_ref()
    }

    /// The style as `Fun_088e96b4` hands it out: a missing one is all zero.
    pub fn style_or_zero(&self, index: u8) -> StyleDef {
        self.style(index).copied().unwrap_or_else(StyleDef::zero)
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.profiles
    }

    pub fn profile(&self, index: usize) -> Option<&Profile> {
        self.profiles.get(index)
    }

    /// How many of the thirteen styles the data has.
    pub fn style_count(&self) -> usize {
        self.styles.iter().flatten().count()
    }
}

/// A synthetic `CoastSettings` with the numbers of spec 3.1: nine profiles of 100 points
/// (`x` from 400 to 0), thirteen styles with a smoothing of 20 and 20, beaches of 1.5, the
/// waterline at 13000 / 65535 and the land at 0x3c3f / 65535. Shared by the tests of the coast.
#[cfg(test)]
pub fn synthetic_settings(with_characters: bool) -> CoastSettings {
    use asset_format::CoastStyle;
    let profile = |k: usize| CoastProfile {
        file: format!("CoastCharacterSpline_{k}.txt"),
        points: (0..100)
            .map(|i| {
                let x = 400.0 - 4.0 * i as f32;
                [x, 0.5 + 0.25 * ((i + k) as f32 * 0.7).sin()]
            })
            .collect(),
    };
    let character = |i: u16| if with_characters { i } else { COAST_NONE };
    let styles = STYLE_NAMES
        .iter()
        .map(|name| CoastStyle {
            name: (*name).into(),
            smoothing_high_side: 20.0,
            smoothing_low_side: 20.0,
            high_beach: CoastLevel {
                height: f32::from(0x3c3f_u16) / 65535.0,
                offset: 1.5,
                character: character(2),
                terrain_type: Some(5),
            },
            low_beach: CoastLevel {
                height: 0.1,
                offset: 1.5,
                character: character(8),
                terrain_type: Some(COAST_NONE),
            },
            waterline: CoastLevel {
                height: 0.19836,
                offset: 0.0,
                character: character(1),
                terrain_type: None,
            },
            extra: Default::default(),
        })
        .collect();
    CoastSettings {
        profiles: (0..PROFILE_COUNT).map(profile).collect(),
        styles,
        spline: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nine_profiles_thirteen_styles_and_the_numbers_of_the_table() {
        let s = CoastStyles::from_settings(&synthetic_settings(true));
        assert_eq!(s.profiles().len(), PROFILE_COUNT);
        assert_eq!(s.style_count(), STYLE_COUNT);
        assert_eq!(STYLE_NAMES.len(), 13);
        // Every profile has the 98 samples the original keeps of a file of up to 100 points.
        assert!(
            s.profiles()
                .iter()
                .all(|p| p.samples.len() == PROFILE_SAMPLES)
        );
        let d = s.style(kind::DEFAULT).unwrap();
        // HighSide is stored negated, LowSide is not; HighBeach.Offset negated, LowBeach.Offset not.
        assert_eq!(d.levels.map(|l| l.offset), [-20.0, -1.5, 0.0, 1.5, 20.0]);
        assert_eq!(d.levels.map(|l| l.value), [0.0, 1.0, 1.0, 1.0, 0.0]);
        // The waterline is the level of the water: 0.19836 of 65535 is 13000.
        assert_eq!(d.levels[2].height_raw(), 13_000);
        // The land is 0x3c3f; the outer levels take the height of the beach on their side.
        assert_eq!(d.levels[0].height_raw(), 0x3c3f);
        assert_eq!(d.levels[1].height_raw(), 0x3c3f);
        assert_eq!(d.levels[3].height, d.levels[4].height);
        // Characters: the outer levels have none, the others are the index of the profile.
        assert_eq!(
            d.levels.map(|l| l.character),
            [None, Some(2), Some(1), Some(8), None]
        );
        // TerrainType 5 is the row with id 5; 65535 and the outer strips are none.
        assert_eq!(d.terrain_type, [None, Some(5), None, None]);
    }

    #[test]
    fn no_character_and_no_terrain_are_the_65535_of_the_file() {
        let s = CoastStyles::from_settings(&synthetic_settings(false));
        let d = s.style(kind::GRASS).unwrap();
        assert!(d.levels.iter().all(|l| l.character.is_none()));
    }

    #[test]
    fn a_style_that_is_not_there_is_all_zero_like_the_original_reads_it() {
        let mut settings = synthetic_settings(true);
        settings.styles.retain(|s| s.name != "Snow");
        let s = CoastStyles::from_settings(&settings);
        assert_eq!(s.style_count(), 12);
        assert!(s.style(kind::SNOW).is_none());
        assert_eq!(s.style_or_zero(kind::SNOW), StyleDef::zero());
        assert_eq!(StyleDef::zero().levels[0].character, Some(0));
        // An unknown name is dropped.
        settings.styles[0].name = "Lava".into();
        assert!(
            CoastStyles::from_settings(&settings)
                .style(kind::DEFAULT)
                .is_none()
        );
    }

    #[test]
    fn a_profile_keeps_the_first_98_points_and_pads_a_short_file_with_zeros() {
        let long: Vec<[f32; 2]> = (0..100).map(|i| [400.0 - 4.0 * i as f32, 1.0]).collect();
        let a = Profile::from_points(&long);
        let b = Profile::from_points(&long[..98]);
        assert_eq!(a, b);
        let short = Profile::from_points(&long[..10]);
        assert_eq!(short.samples.len(), PROFILE_SAMPLES);
    }

    #[test]
    fn a_straight_profile_has_no_character_in_it() {
        // Points on a straight line are not changed by the smoothing: the difference is ~0.
        let line: Vec<[f32; 2]> = (0..100).map(|i| [400.0 - 4.0 * i as f32, 2.0]).collect();
        let p = Profile::from_points(&line);
        for s in &p.samples[2..PROFILE_SAMPLES - 2] {
            assert!(s.length() < 1e-2, "{s}");
        }
    }

    #[test]
    fn the_levels_of_a_style_are_ordered_from_land_to_sea() {
        let s = CoastStyles::from_settings(&synthetic_settings(false));
        for i in 0..STYLE_COUNT {
            let d = s.style(i as u8).unwrap();
            assert!(d.levels.windows(2).all(|w| w[0].offset <= w[1].offset));
        }
    }
}
