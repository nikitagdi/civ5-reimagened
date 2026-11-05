//! World settings (`C5WS`, manifest id `tb:world-b2`): the settings files of the original that the
//! B2 world renderer reads, as typed fields.
//!
//! The sections: fog, the world overlay (hex grid, culture borders, highlights, unit selection,
//! arrows), the colour-key table, the terrain definitions and decals, the coast, wave and ice
//! settings and the list of `.lean` textures. The forest settings join with their own card.
//!
//! Every section ends in `extra`: attributes (or elements) the pipeline does not know by name,
//! kept as written. For the files we know, `extra` is empty; the pipeline reports anything in it.
//! Colours are lists of numbers as in the file (a highlight style of Lekmod has five).
//!
//! Framing is the common blob header; the payload is postcard, so equal settings give equal bytes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{FormatError, LogicalId, blob};

pub const WORLD_SETTINGS_MAGIC: [u8; 4] = *b"C5WS";

/// Manifest id of the settings blob.
pub const WORLD_SETTINGS_ID: &str = "tb:world-b2";

/// Unknown attributes or elements of a section: name to value as written.
pub type Extra = BTreeMap<String, String>;

/// `fluidfowsettings.xml`: the fluid simulation of the fog of war clouds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FluidFow {
    pub spawn_frequency: f32,
    pub size_mean: f32,
    pub size_min: f32,
    pub size_max: f32,
    pub size_deviation: f32,
    pub mass_mean: f32,
    pub mass_min: f32,
    pub mass_max: f32,
    pub mass_deviation: f32,
    pub mass_and_size_correlation: f32,
    pub height_variation: f32,
    pub fluid_density: f32,
    pub speed: f32,
    pub drag: f32,
    pub master_size: f32,
    pub force_strength: f32,
    pub max_rotate: f32,
    pub fog_height: f32,
    pub extra: Extra,
}

/// `HexGrid.xml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct HexGrid {
    /// RGBA as written (`255,255,255,75`).
    pub color: Vec<u8>,
    /// 0 (centre) to 1 (edge of the hex), as in the file.
    pub width: f32,
    pub extra: Extra,
}

/// `CultureBorders.xml`; the `cs_*` attributes are the city state borders.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CultureBorders {
    pub primary_width: f32,
    pub secondary_width: f32,
    pub primary_aa_width: f32,
    pub secondary_aa_width: f32,
    pub primary_alpha: f32,
    pub secondary_alpha: f32,
    pub corner_bias: f32,
    pub cs_primary_width: f32,
    pub cs_outline_width: f32,
    pub cs_aa_width: f32,
    pub cs_primary_alpha: f32,
    pub cs_outline_alpha: f32,
    pub cs_dash_width: f32,
    pub randomness: f32,
    pub expansion: f32,
    pub extra: Extra,
}

/// The `type` of a highlight style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightKind {
    HexContour,
    FilledHex,
    StaticTexture,
    HexModel,
    SplineBorder,
}

/// One `<style>` of `Highlights.xml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HighlightStyle {
    pub name: String,
    pub kind: HighlightKind,
    pub width: Option<f32>,
    /// Texture of the style, as the `tx:` id of the file.
    pub texture: Option<LogicalId>,
    /// File name of the model (`.fxsxml`) as written; the models are another set.
    pub model: Option<String>,
    /// Numbers of the `color` attribute as written; empty when there is none.
    pub color: Vec<u8>,
    pub extra: Extra,
}

/// How the selection ring turns. The file says: "off" and "clockwise" are read, anything else is
/// counter-clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    Off,
    Clockwise,
    CounterClockwise,
}

/// `<UnitSelectionHighlight>` of `Civ5ArtDefines_Overlay.xml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct UnitSelectionHighlight {
    /// `red`, `green`, `blue`, `alpha`, each 0 to 1.
    pub color: [f32; 4],
    pub pulse: bool,
    pub pulse_speed: f32,
    pub rotation: Rotation,
    pub rotation_speed: f32,
    pub extra: Extra,
}

/// `Civ5ArtDefines_Overlay.xml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct OverlayDefs {
    pub unit_selection_highlight: UnitSelectionHighlight,
    /// Other children of the root, by element name (their attributes and text, as written).
    pub extra: Extra,
}

/// `ArrowSettings.xml`: the movement and trade arrows.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Arrows {
    pub min_width: f32,
    pub max_width: f32,
    pub width2: f32,
    pub arc_height: f32,
    pub outline_width: f32,
    pub outline_width2: f32,
    /// RGB as written.
    pub body_color: Vec<u8>,
    pub outline_color: Vec<u8>,
    pub alpha: f32,
    pub target_height: f32,
    pub anim_speed: f32,
    pub ambient_light_level: f32,
    pub extra: Extra,
}

/// One colour-key table: the system and kind it belongs to, and its file. Until the meaning of the
/// original's `ColorKeyData[5][5]` is known (RW-4) the table is the file list: `system` and `kind`
/// are the parts of the file name (`terrainshadow_europe_colorkey.tga` is `terrainshadow` and
/// `europe`), `kind` is empty when the file has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorKey {
    pub system: String,
    pub kind: String,
    /// The `tx:` id of the volume.
    pub file: LogicalId,
}

/// One row of `terraindef.csv`: the textures and parameters of a terrain type of a continent style.
/// The ids are the `tx:` ids of the files; `NULL` in the CSV is `None` (the copy of the file that
/// is packed in `TerrainModels.fpk` has `NULL` diffuse and height in its first row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainDefRow {
    /// `Continent ID`: 0 Generic, 1 America, 2 Asia, 3 Africa, 4 Europe.
    pub continent: u8,
    pub diffuse: Option<LogicalId>,
    pub heightmap: Option<LogicalId>,
    pub specmap: Option<LogicalId>,
    /// `BaseDominance`, 0 or 1 in the file.
    pub base_dominance: u8,
    /// `Texture Scale`.
    pub texture_scale: f32,
    /// `Projection`: 0 horizontal, 1 vertical.
    pub projection: u8,
    /// `ID (unique for continent)`: the number the terrain id codes of the pieces are matched to.
    pub id: u32,
}

/// One decal of `decal_filenames.xml`: `<Floodplains_EndCap1><Floodplains_01 albedo=... />`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecalDef {
    /// The element under the root (`Floodplains_EndCap1`).
    pub kind: String,
    /// The element under it (`Floodplains_01`).
    pub name: String,
    /// The `tx:` ids of the files named by `albedo` and `heightmap`.
    pub albedo: Option<LogicalId>,
    pub heightmap: Option<LogicalId>,
    pub priority: Option<f32>,
    pub extra: Extra,
}

/// `Character` and `TerrainType` of a coast style level when the style has none.
pub const COAST_NONE: u16 = 65535;

/// One of the 9 character splines of `coaststyles.xml` (`coastcharacterspline_01_water.txt`, ...),
/// in the order the file lists them: the styles refer to them by that index. The third coordinate
/// of every line of the file is 0 and is dropped.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoastProfile {
    /// File name as the XML writes it.
    pub file: String,
    pub points: Vec<[f32; 2]>,
}

/// `HighBeach`, `LowBeach` or `Waterline` of a coast style.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoastLevel {
    pub height: f32,
    pub offset: f32,
    /// Index into [`CoastSettings::profiles`], or [`COAST_NONE`].
    pub character: u16,
    /// `TerrainType` as written ([`COAST_NONE`] for none); `None` for the `Waterline`, which has
    /// no such attribute.
    pub terrain_type: Option<u16>,
}

/// One `<CoastStyle>` of `coaststyles.xml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoastStyle {
    pub name: String,
    pub smoothing_high_side: f32,
    pub smoothing_low_side: f32,
    pub high_beach: CoastLevel,
    pub low_beach: CoastLevel,
    pub waterline: CoastLevel,
    pub extra: Extra,
}

/// `coaststyles.xml` with its character splines, and `coastspline01.txt`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CoastSettings {
    pub profiles: Vec<CoastProfile>,
    pub styles: Vec<CoastStyle>,
    /// `coastspline01.txt`: the profile the coast blend follows, `[x, y]` per point.
    pub spline: Vec<[f32; 2]>,
}

/// `wavesettings.xml`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WaveSettings {
    pub waves_per_edge: f32,
    pub random_seed: f32,
    pub crest_pages_u: u32,
    pub crest_pages_v: u32,
    pub crest_pages_in_use: u32,
    pub min_size: f32,
    pub max_size: f32,
    pub start_distance: f32,
    pub end_distance: f32,
    pub cycle_time: f32,
    pub fade_in_start: f32,
    pub fade_in_end: f32,
    pub fade_out_start: f32,
    pub fade_out_end: f32,
    pub restart_delay_min: f32,
    pub restart_delay_max: f32,
    /// `Color_R`, `Color_G`, `Color_B`.
    pub color: [u8; 3],
    pub extra: Extra,
}

/// One `<IceSettings>` file (`icesettings.xml` or `icesettings_low.xml`). The names are the
/// attributes of the file; the counts are kept as numbers like the rest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IceParams {
    pub dump_input: bool,
    pub far_sample_dist: f32,
    pub far_samples: f32,
    pub near_sample_dist: f32,
    pub near_samples: f32,
    pub crack_cost: f32,
    pub initial_potential: f32,
    pub crack_min_clip: f32,
    pub crack_max_clip: f32,
    pub edge_pull_amount: f32,
    pub small_poly_shrink_amount: f32,
    pub lattice_verts_per_hex: f32,
    pub displace_frequency: f32,
    pub displace_min: f32,
    pub displace_max: f32,
    pub displace_exponent: f32,
    pub small_poly_depth_min: f32,
    pub small_poly_depth_max: f32,
    pub small_poly_height_min: f32,
    pub small_poly_height_max: f32,
    pub large_poly_edge_height_min: f32,
    pub large_poly_edge_height_max: f32,
    pub large_poly_edge_height_freq: f32,
    pub skirt_slope_small: f32,
    pub skirt_slope_large: f32,
    pub large_poly_height: f32,
    pub large_poly_depth: f32,
    pub boundary_kill_rate: f32,
    pub interior_kill_rate: f32,
    /// `Subsurface_R`, `_G`, `_B`.
    pub subsurface: [u8; 3],
    pub specular_power: f32,
    pub texture_repeat: f32,
    pub bump_repeat: f32,
    pub side_texture_repeat_u: f32,
    pub side_bump_repeat_u: f32,
    pub side_texture_repeat_v: f32,
    pub side_bump_repeat_v: f32,
    /// `Underwater_R1`, `_G1`, `_B1`.
    pub underwater_1: [u8; 3],
    /// `Underwater_R2`, `_G2`, `_B2`.
    pub underwater_2: [u8; 3],
    pub underwater_color_ramp: f32,
    pub underwater_alpha_ramp: f32,
    pub extra: Extra,
}

/// Both ice settings files. Which one the engine takes is RW-22 (b); both are kept.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct IceSettings {
    /// `icesettings.xml`.
    pub full: IceParams,
    /// `icesettings_low.xml`.
    pub low: IceParams,
}

/// One `.lean` file of the install: its header, and the two textures the
/// pipeline made of its two chains.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeanInfo {
    /// Name of the file, lower case (`waterbumps16.lean`).
    pub file: String,
    /// `-output_scale` of LeanMaker, the header's second word.
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// 1 or 2: the file held 8 or 16 bit channels.
    pub bytes_per_channel: u8,
    /// Mean slopes and first moments: `Rgba16F`.
    pub slopes: LogicalId,
    /// Cross moment: `R16F`.
    pub cross: LogicalId,
}

/// Empty until a set fills a section: the sets of the pipeline each write their own part and merge
/// it into the blob already in the manifest.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct WorldSettings {
    /// `fogcolor.csv`: one RGB per row.
    pub fog_colors: Vec<[u8; 3]>,
    pub fluid_fow: FluidFow,
    pub hex_grid: HexGrid,
    pub culture_borders: CultureBorders,
    /// In file order.
    pub highlights: Vec<HighlightStyle>,
    pub overlay_defs: OverlayDefs,
    pub arrows: Arrows,
    pub color_keys: Vec<ColorKey>,
    /// `terraindef.csv`, rows in file order.
    pub terrain_defs: Vec<TerrainDefRow>,
    /// `decal_filenames.xml`, in file order.
    pub decals: Vec<DecalDef>,
    /// `coaststyles.xml`, its profiles and `coastspline01.txt`.
    pub coast: CoastSettings,
    pub waves: WaveSettings,
    pub ice: IceSettings,
    /// The `.lean` files, sorted by file name.
    pub lean: Vec<LeanInfo>,
}

impl WorldSettings {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(WORLD_SETTINGS_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, s): (u16, Self) = blob::decode_range(
            WORLD_SETTINGS_MAGIC,
            Self::MIN_VERSION,
            Self::VERSION,
            bytes,
        )?;
        s.check()?;
        Ok(s)
    }

    /// Every texture id the settings name, with what names it (`highlights[EnemyOutline]`): the
    /// closure check of the build asks the manifest for each.
    pub fn texture_refs(&self) -> Vec<(String, LogicalId)> {
        let mut v = Vec::new();
        for h in &self.highlights {
            if let Some(t) = &h.texture {
                v.push((format!("highlights[{}]", h.name), t.clone()));
            }
        }
        for k in &self.color_keys {
            v.push((
                format!("color_keys[{}/{}]", k.system, k.kind),
                k.file.clone(),
            ));
        }
        for (i, r) in self.terrain_defs.iter().enumerate() {
            let at = |what: &str| {
                format!(
                    "terrain_defs[{i}] (continent {}, id {}) {what}",
                    r.continent, r.id
                )
            };
            for (what, t) in [
                ("diffuse", &r.diffuse),
                ("heightmap", &r.heightmap),
                ("specmap", &r.specmap),
            ] {
                if let Some(t) = t {
                    v.push((at(what), t.clone()));
                }
            }
        }
        for d in &self.decals {
            for (what, t) in [("albedo", &d.albedo), ("heightmap", &d.heightmap)] {
                if let Some(t) = t {
                    v.push((format!("decals[{}/{}] {what}", d.kind, d.name), t.clone()));
                }
            }
        }
        for l in &self.lean {
            v.push((format!("lean[{}] slopes", l.file), l.slopes.clone()));
            v.push((format!("lean[{}] cross", l.file), l.cross.clone()));
        }
        v
    }

    /// Names of the unknown attributes and elements, as `section: name`; empty for the files we
    /// know.
    pub fn extras(&self) -> Vec<String> {
        let mut v = Vec::new();
        let mut add = |section: String, e: &Extra| {
            v.extend(e.keys().map(|k| format!("{section}: {k}")));
        };
        add("fluid_fow".into(), &self.fluid_fow.extra);
        add("hex_grid".into(), &self.hex_grid.extra);
        add("culture_borders".into(), &self.culture_borders.extra);
        for h in &self.highlights {
            add(format!("highlights[{}]", h.name), &h.extra);
        }
        add(
            "overlay_defs.unit_selection_highlight".into(),
            &self.overlay_defs.unit_selection_highlight.extra,
        );
        add("overlay_defs".into(), &self.overlay_defs.extra);
        add("arrows".into(), &self.arrows.extra);
        for d in &self.decals {
            add(format!("decals[{}/{}]", d.kind, d.name), &d.extra);
        }
        for s in &self.coast.styles {
            add(format!("coast[{}]", s.name), &s.extra);
        }
        add("waves".into(), &self.waves.extra);
        add("ice.full".into(), &self.ice.full.extra);
        add("ice.low".into(), &self.ice.low.extra);
        v
    }

    fn check(&self) -> Result<(), FormatError> {
        let mut floats = vec![
            self.fluid_fow.spawn_frequency,
            self.fluid_fow.fluid_density,
            self.fluid_fow.fog_height,
            self.hex_grid.width,
            self.culture_borders.primary_width,
            self.arrows.alpha,
        ];
        floats.extend(self.terrain_defs.iter().map(|r| r.texture_scale));
        floats.extend(self.decals.iter().filter_map(|d| d.priority));
        floats.extend(self.coast.spline.iter().flatten());
        floats.extend(
            self.coast
                .profiles
                .iter()
                .flat_map(|p| p.points.iter().flatten()),
        );
        for s in &self.coast.styles {
            for l in [&s.high_beach, &s.low_beach, &s.waterline] {
                floats.extend([l.height, l.offset]);
            }
            floats.extend([s.smoothing_high_side, s.smoothing_low_side]);
        }
        let w = &self.waves;
        floats.extend([
            w.waves_per_edge,
            w.cycle_time,
            w.min_size,
            w.max_size,
            w.fade_in_start,
            w.fade_out_end,
        ]);
        for i in [&self.ice.full, &self.ice.low] {
            floats.extend([
                i.far_sample_dist,
                i.near_sample_dist,
                i.displace_min,
                i.displace_max,
                i.lattice_verts_per_hex,
                i.specular_power,
                i.texture_repeat,
                i.bump_repeat,
            ]);
        }
        floats.extend(self.lean.iter().map(|l| l.scale));
        if floats.iter().any(|f| !f.is_finite()) {
            return Err(FormatError::invalid("settings hold a non-finite number"));
        }
        for s in &self.coast.styles {
            for (what, l) in [
                ("high_beach", &s.high_beach),
                ("low_beach", &s.low_beach),
                ("waterline", &s.waterline),
            ] {
                if l.character != COAST_NONE
                    && usize::from(l.character) >= self.coast.profiles.len()
                {
                    return Err(FormatError::invalid(format!(
                        "coast style {} {what}: the character {} is not one of the {} profiles",
                        s.name,
                        l.character,
                        self.coast.profiles.len()
                    )));
                }
            }
        }
        if self.lean.windows(2).any(|p| p[0].file >= p[1].file) {
            return Err(FormatError::invalid("the lean list is not sorted by file"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> WorldSettings {
        WorldSettings {
            fog_colors: vec![[146, 182, 243]],
            fluid_fow: FluidFow {
                spawn_frequency: 22.0,
                fog_height: 11.0,
                ..FluidFow::default()
            },
            hex_grid: HexGrid {
                color: vec![255, 255, 255, 75],
                width: 0.2,
                extra: Extra::new(),
            },
            culture_borders: CultureBorders {
                primary_width: 1.4,
                ..CultureBorders::default()
            },
            highlights: vec![HighlightStyle {
                name: "EnemyOutline".into(),
                kind: HighlightKind::SplineBorder,
                width: Some(7.0),
                texture: Some(LogicalId::new("tx:hex_contour1.dds")),
                model: None,
                color: vec![255, 0, 0, 0, 200],
                extra: Extra::new(),
            }],
            overlay_defs: OverlayDefs {
                unit_selection_highlight: UnitSelectionHighlight {
                    color: [0.9, 0.9, 0.45, 1.0],
                    pulse: true,
                    pulse_speed: 2.0,
                    rotation: Rotation::Clockwise,
                    rotation_speed: 1.0,
                    extra: Extra::new(),
                },
                extra: Extra::new(),
            },
            arrows: Arrows {
                body_color: vec![255, 0, 0],
                alpha: 0.5,
                ..Arrows::default()
            },
            color_keys: vec![ColorKey {
                system: "terrainshadow".into(),
                kind: "europe".into(),
                file: LogicalId::new("tx:terrainshadow_europe_colorkey.tga"),
            }],
            terrain_defs: vec![
                TerrainDefRow {
                    continent: 4,
                    diffuse: Some(LogicalId::new("tx:euro_grassland_d.dds")),
                    heightmap: Some(LogicalId::new("tx:euro_grassland_h.dds")),
                    specmap: None,
                    base_dominance: 0,
                    texture_scale: 0.014,
                    projection: 0,
                    id: 2,
                },
                TerrainDefRow {
                    continent: 0,
                    diffuse: Some(LogicalId::new("tx:generic_snow_d.dds")),
                    heightmap: Some(LogicalId::new("tx:generic_snow_h.dds")),
                    specmap: Some(LogicalId::new("tx:generic_snow_s.dds")),
                    base_dominance: 1,
                    texture_scale: 0.014,
                    projection: 1,
                    id: 30,
                },
            ],
            decals: vec![DecalDef {
                kind: "Floodplains".into(),
                name: "Floodplains_01".into(),
                albedo: Some(LogicalId::new("tx:floodplains_d.dds")),
                heightmap: Some(LogicalId::new("tx:floodplains_h.dds")),
                priority: Some(0.4),
                extra: Extra::new(),
            }],
            coast: CoastSettings {
                profiles: vec![CoastProfile {
                    file: "CoastCharacterSpline_01_Water.txt".into(),
                    points: vec![[400.0, 0.10114], [395.96, 0.858047]],
                }],
                styles: vec![CoastStyle {
                    name: "Default".into(),
                    smoothing_high_side: 20.0,
                    smoothing_low_side: 20.0,
                    high_beach: CoastLevel {
                        height: 0.2244,
                        offset: 1.5,
                        character: COAST_NONE,
                        terrain_type: Some(5),
                    },
                    low_beach: CoastLevel::default(),
                    waterline: CoastLevel {
                        character: 0,
                        ..CoastLevel::default()
                    },
                    extra: Extra::new(),
                }],
                spline: vec![[-357.119, -2.61595]],
            },
            waves: WaveSettings {
                waves_per_edge: 1.13,
                crest_pages_u: 8,
                color: [64, 64, 64],
                ..WaveSettings::default()
            },
            ice: IceSettings {
                full: IceParams {
                    far_samples: 48.0,
                    subsurface: [19, 142, 205],
                    ..IceParams::default()
                },
                low: IceParams {
                    far_samples: 16.0,
                    ..IceParams::default()
                },
            },
            lean: vec![LeanInfo {
                file: "waterbumps16.lean".into(),
                scale: 0.33,
                width: 512,
                height: 512,
                mips: 10,
                bytes_per_channel: 2,
                slopes: LogicalId::new("tx:waterbumps16.lean"),
                cross: LogicalId::new("tx:waterbumps16.lean#cross"),
            }],
        }
    }

    #[test]
    fn round_trips_and_is_deterministic() {
        let s = sample();
        let bytes = s.encode();
        assert_eq!(&bytes[..4], b"C5WS");
        assert_eq!(WorldSettings::decode(&bytes).unwrap(), s);
        assert_eq!(sample().encode(), bytes);
    }

    #[test]
    fn extras_are_listed_by_section() {
        let mut s = sample();
        assert!(s.extras().is_empty());
        s.hex_grid.extra.insert("glow".into(), "1".into());
        s.highlights[0].extra.insert("pulse".into(), "x".into());
        s.decals[0].extra.insert("tint".into(), "red".into());
        s.coast.styles[0]
            .extra
            .insert("Foam@Height".into(), "1".into());
        s.waves.extra.insert("Wind".into(), "3".into());
        s.ice.low.extra.insert("Snow".into(), "1".into());
        assert_eq!(
            s.extras(),
            [
                "hex_grid: glow",
                "highlights[EnemyOutline]: pulse",
                "decals[Floodplains/Floodplains_01]: tint",
                "coast[Default]: Foam@Height",
                "waves: Wind",
                "ice.low: Snow"
            ]
        );
    }

    #[test]
    fn a_coast_character_outside_the_profiles_is_refused() {
        let mut s = sample();
        s.coast.styles[0].waterline.character = 1;
        assert!(WorldSettings::decode(&s.encode()).is_err());
        s.coast.styles[0].waterline.character = COAST_NONE;
        assert!(WorldSettings::decode(&s.encode()).is_ok());
    }

    #[test]
    fn the_lean_list_must_be_sorted() {
        let mut s = sample();
        let mut second = s.lean[0].clone();
        second.file = "ice_bump.lean".into();
        s.lean.push(second);
        assert!(WorldSettings::decode(&s.encode()).is_err());
        s.lean.reverse();
        assert!(WorldSettings::decode(&s.encode()).is_ok());
    }

    #[test]
    fn texture_refs_cover_every_section_that_names_a_texture() {
        let refs: Vec<String> = sample()
            .texture_refs()
            .into_iter()
            .map(|(_, id)| id.as_str().to_owned())
            .collect();
        for want in [
            "tx:hex_contour1.dds",
            "tx:terrainshadow_europe_colorkey.tga",
            "tx:euro_grassland_h.dds",
            "tx:generic_snow_s.dds",
            "tx:floodplains_h.dds",
            "tx:waterbumps16.lean#cross",
        ] {
            assert!(refs.iter().any(|r| r == want), "{want}");
        }
        // A NULL specmap names nothing: 1 + 1 + (2 + 3) + 2 + 2.
        assert_eq!(refs.len(), 11);
    }

    #[test]
    fn wrong_magic_version_and_tail_are_refused() {
        let good = sample().encode();
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(matches!(
            WorldSettings::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut v2 = good.clone();
        v2[4] = 2;
        assert!(matches!(
            WorldSettings::decode(&v2),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        let mut tail = good;
        tail.push(0);
        assert!(matches!(
            WorldSettings::decode(&tail),
            Err(FormatError::TrailingBytes(1))
        ));
    }

    #[test]
    fn a_non_finite_number_is_refused() {
        let mut s = sample();
        s.hex_grid.width = f32::NAN;
        assert!(WorldSettings::decode(&s.encode()).is_err());
    }
}
