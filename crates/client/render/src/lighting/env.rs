//! `LightingEnv`: the shared light resources of the world: the light cubes
//! of the terrain by continent style, `specscreenmap`, the cubes of the models, the colour-key
//! volumes and the distance-fog ramp. They come from `assets-b2` as the `boot` group of the set
//! `b2-light` (`PrefetchGroup(1)`), the settings of the world as the blob `tb:world-b2`.
//!
//! The names are those of the files of the original (`tx:<file name>`, lower case): the pipeline
//! of `b2-light` (`tools/asset-pipeline/src/sets/b2_light.rs`) writes them under these ids.

use crate::perf::Tex;
use std::collections::HashMap;

use asset_format::{LogicalId, PrefetchGroup, TexDim, TextureBlob, WorldSettings};

use crate::texture::{self, UploadOptions};

/// `Variant.prefetch = 1` is `Boot` in B2 (assets-b2 AB-D10): the group the first map frame waits
/// for.
pub const BOOT_GROUP: PrefetchGroup = PrefetchGroup(1);

/// Id of the world settings blob.
pub const WORLD_SETTINGS_ID: &str = asset_format::WORLD_SETTINGS_ID;

/// Continent styles of the light cubes and of the colour-key tables. The order is the file-name
/// order; it is the `class` index of [`terrain_table_file`] until RW-4 (g) names the classes.
// TODO(RW-4 g): the original's class order of `g_ColorKeySystem.m_auKeyHandles[k][class]`.
pub const STYLES: [&str; 5] = ["africa", "america", "asia", "europe", "generic"];

/// Styles that have a second shadow cube (`_2`, Upgrade1).
// measured: 2026-10-21 (b2_light.rs), `shadowlight_{america,asia,europe}_terrain_2.dds` exist.
pub const STYLES_WITH_SECOND_SHADOW: [&str; 3] = ["america", "asia", "europe"];

/// Kinds `k` of the terrain colour keys: 0..2 of `color_keys[k][class]`.
///
/// `TerrainCell.cpp:492-494, :672-676` indexes `[k][class]` with k = 0..2; the samplers are
/// `ColorKey…`, `ShadowColor…` and `TransFOWColor…`. Which k is which is RW-4 (g).
// TODO(RW-4 g): the real k order; this is the order of the names in the spec.
pub const TERRAIN_KEY_SYSTEMS: [&str; 3] = ["terrain", "terrainshadow", "terrainfow"];

/// Terrain classes of the colour keys.
pub const TERRAIN_KEY_CLASSES: usize = STYLES.len();

/// The cubes of the models: those of the units, and the cube lights and the reflection cubes of
/// the houses of the cities ([`CITY_CUBE_LIGHTS`], [`CITY_REFLECTION_SOFT`] and `_SHARP`).
pub const MODEL_CUBES: [&str; 10] = [
    "unit_irradiance.dds",
    "unit_environment_dull.dds",
    "unit_environment_sharp.dds",
    "cubelight_generic_32.dds",
    CITY_CUBE_LIGHTS[0],
    CITY_CUBE_LIGHTS[1],
    CITY_CUBE_LIGHTS[2],
    CITY_CUBE_LIGHTS[3],
    CITY_REFLECTION_SOFT,
    CITY_REFLECTION_SHARP,
];

/// The cube lights (`Irradiance`) of the houses of the cities, by culture: Africa, America, Asia,
/// Europe.
// measured: S3 (2026-10-28), t2 of the instanced `BuildingPS_CODE_Opaque` draws of the houses is
// `europe_cubelight.dds` or `asia_cubelight.dds` byte for byte (the 128 px BC1 cubes of the install).
pub const CITY_CUBE_LIGHTS: [&str; 4] = [
    "africa_cubelight.dds",
    "americas_cubelight.dds",
    "asia_cubelight.dds",
    "europe_cubelight.dds",
];
/// t3 of the houses (S3 resource 5244, the cube the gloss lerps to: `SpecSharp` of the port).
pub const CITY_REFLECTION_SOFT: &str = "hiconreflmap_soft.dds";
/// t4 of the houses (S3 resource 5247, the cube at gloss 0: `SpecSoft` of the port).
pub const CITY_REFLECTION_SHARP: &str = "hiconreflmap_sharp.dds";

/// `IrradianceCubeMap` of `UnitPS`.
pub const UNIT_IRRADIANCE: &str = "unit_irradiance.dds";
/// `DullEnvironmentCubeMap` of `UnitPS`.
pub const UNIT_ENVIRONMENT_DULL: &str = "unit_environment_dull.dds";
/// `EnvironmentCubeMap` of `UnitPS`.
pub const UNIT_ENVIRONMENT_SHARP: &str = "unit_environment_sharp.dds";
/// The screen of the terrain's specular highlight.
pub const SPEC_SCREEN_MAP: &str = "specscreenmap.dds";
/// The ramp of the distance fog (`FogRampCached`, `DistanceFogRamp`).
pub const FOG_RAMP: &str = "fogramp.dds";
/// The colour key of the rivers.
pub const RIVERS_TABLE: &str = "rivers_generic_colorkey.tga";
/// `TerrainColorKey`, the single table of the terrain.
pub const TERRAIN_COLOR_KEY: &str = "terraincolorkey.tga";

/// File of the terrain table `color_keys[k][class]`.
pub fn terrain_table_file(k: usize, class: usize) -> Option<String> {
    let system = TERRAIN_KEY_SYSTEMS.get(k)?;
    let style = STYLES.get(class)?;
    Some(format!("{system}_{style}_colorkey.tga"))
}

/// File of the table of the models of a style (`Buildings_*`).
pub fn model_table_file(class: usize) -> Option<String> {
    STYLES
        .get(class)
        .map(|style| format!("buildings_{style}_colorkey.tga"))
}

/// Every file `LightingEnv` needs from the boot group, lower case, by file name.
pub fn required_files() -> Vec<String> {
    let mut v = Vec::new();
    for style in STYLES {
        v.push(format!("sunlight_{style}_terrain.dds"));
        v.push(format!("shadowlight_{style}_terrain.dds"));
    }
    for style in STYLES_WITH_SECOND_SHADOW {
        v.push(format!("shadowlight_{style}_terrain_2.dds"));
    }
    v.extend(MODEL_CUBES.map(str::to_owned));
    v.push(SPEC_SCREEN_MAP.to_owned());
    v.push(FOG_RAMP.to_owned());
    for k in 0..TERRAIN_KEY_SYSTEMS.len() {
        for class in 0..TERRAIN_KEY_CLASSES {
            v.extend(terrain_table_file(k, class));
        }
    }
    for class in 0..STYLES.len() {
        v.extend(model_table_file(class));
    }
    v.push(RIVERS_TABLE.to_owned());
    v.push(TERRAIN_COLOR_KEY.to_owned());
    v
}

/// The manifest id of a file: `tx:<file name>` (the pipeline's `tx_id`).
pub fn texture_id(file: &str) -> LogicalId {
    LogicalId::new(format!("tx:{file}"))
}

/// Every manifest id `LightingEnv` needs: its textures and the settings blob.
pub fn required_ids() -> Vec<LogicalId> {
    let mut v: Vec<LogicalId> = required_files().iter().map(|f| texture_id(f)).collect();
    v.push(LogicalId::new(WORLD_SETTINGS_ID));
    v
}

/// The file name behind an id, if it is one of ours.
pub fn file_of(id: &LogicalId) -> Option<&str> {
    id.as_str().strip_prefix("tx:")
}

/// An uploaded light texture.
pub struct LightTexture {
    pub texture: Tex,
    pub view: wgpu::TextureView,
    pub dim: TexDim,
}

/// Why a texture was not taken.
#[derive(Debug, thiserror::Error)]
pub enum LightingError {
    #[error("{0} is not a texture of the lighting environment")]
    Unknown(String),
    #[error("{file} is a {got:?} texture, expected {want:?}")]
    Shape {
        file: String,
        got: TexDim,
        want: TexDim,
    },
    #[error("{file}: {why}")]
    Upload { file: String, why: String },
}

/// What shape a file of the environment has.
fn expected_dim(file: &str) -> TexDim {
    if file.ends_with(".tga") {
        TexDim::D3
    } else if file.starts_with("sunlight_")
        || file.starts_with("shadowlight_")
        || MODEL_CUBES.contains(&file)
    {
        TexDim::Cube
    } else {
        TexDim::D2
    }
}

/// `sunlight_*_terrain*` and `shadowlight_*_terrain*`: the light cubes of the terrain.
fn is_terrain_cube(file: &str) -> bool {
    (file.starts_with("sunlight_") || file.starts_with("shadowlight_")) && file.ends_with(".dds")
}

/// The light resources of the world.
#[derive(Default)]
pub struct LightingEnv {
    textures: HashMap<String, LightTexture>,
    /// The terrain light cubes again, uploaded `*Srgb` (the page bake reads these).
    srgb_cubes: HashMap<String, LightTexture>,
    settings: Option<Box<WorldSettings>>,
}

impl LightingEnv {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the texture of `file` (`unit_irradiance.dds`, `terrainshadow_europe_colorkey.tga`)
    /// and uploads it, raw (`*Unorm`: the ported shaders do their own gamma).
    pub fn add_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        file: &str,
        blob: &TextureBlob,
    ) -> Result<(), LightingError> {
        let file = file.to_ascii_lowercase();
        if !required_files().contains(&file) {
            return Err(LightingError::Unknown(file));
        }
        let want = expected_dim(&file);
        if blob.dim != want {
            return Err(LightingError::Shape {
                file,
                got: blob.dim,
                want,
            });
        }
        let up = texture::upload_blob(device, queue, blob, &file, UploadOptions::WORLD).map_err(
            |why| LightingError::Upload {
                file: file.clone(),
                why,
            },
        )?;
        let view = up.texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(&file),
            dimension: Some(up.view_dimension),
            ..Default::default()
        });
        // The cubes of the models too: `BuildingPS_CODE` of the cities reads them decoded.
        if is_terrain_cube(&file) || MODEL_CUBES.contains(&file.as_str()) {
            // The D3D11 build binds the terrain cubes as `R8G8B8A8_SRGB`/`BC1_SRGB` (S1a ev 1897,
            // resources 2002, 2005, 2008), so the page bake reads them decoded. The cubes are tiny;
            // a second copy in the sRGB flavour keeps the raw one for the M1 forward pass. The
            // bake shader then reads them as the SM3 block does (`texld`, no gamma of its own).
            let mut srgb = blob.clone();
            srgb.srgb = true;
            let label = format!("{file} (srgb)");
            let up = texture::upload_blob(device, queue, &srgb, &label, UploadOptions::default())
                .map_err(|why| LightingError::Upload {
                file: file.clone(),
                why,
            })?;
            if !up.format.is_srgb() {
                tracing::warn!(
                    "{file}: {:?} has no sRGB flavour, the bake reads it raw",
                    up.format
                );
            }
            let view = up.texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some(&label),
                dimension: Some(up.view_dimension),
                ..Default::default()
            });
            self.srgb_cubes.insert(
                file.clone(),
                LightTexture {
                    texture: up.texture,
                    view,
                    dim: blob.dim,
                },
            );
        }
        self.textures.insert(
            file,
            LightTexture {
                texture: up.texture,
                view,
                dim: blob.dim,
            },
        );
        Ok(())
    }

    /// Takes the world settings (`tb:world-b2`).
    pub fn set_world_settings(&mut self, settings: WorldSettings) {
        self.settings = Some(Box::new(settings));
    }

    pub fn world_settings(&self) -> Option<&WorldSettings> {
        self.settings.as_deref()
    }

    /// The texture of `file`, if it is loaded.
    pub fn texture(&self, file: &str) -> Option<&LightTexture> {
        self.textures.get(file)
    }

    pub fn view(&self, file: &str) -> Option<&wgpu::TextureView> {
        self.textures.get(file).map(|t| &t.view)
    }

    /// The files of the boot group that have not arrived yet.
    pub fn missing(&self) -> Vec<String> {
        required_files()
            .into_iter()
            .filter(|f| !self.textures.contains_key(f))
            .collect()
    }

    /// Number of boot items (textures and the settings blob) loaded, and in all: the progress of
    /// the loading screen.
    pub fn progress(&self) -> (usize, usize) {
        let all = required_files().len() + 1;
        let have = all - self.missing().len() - usize::from(self.settings.is_none());
        (have, all)
    }

    /// The whole boot group has been taken: the world may be drawn (spec 4.9 "Первый кадр карты
    /// ждёт `boot`").
    pub fn ready(&self) -> bool {
        self.settings.is_some() && self.missing().is_empty()
    }

    // ---- terrain ----

    /// `sunlight_<style>_terrain`, the light cube of the terrain.
    pub fn terrain_sunlight(&self, class: usize) -> Option<&wgpu::TextureView> {
        self.view(&format!("sunlight_{}_terrain.dds", STYLES.get(class)?))
    }

    /// `shadowlight_<style>_terrain`, or its second cube (`_2`) where there is one.
    pub fn terrain_shadowlight(&self, class: usize, second: bool) -> Option<&wgpu::TextureView> {
        let style = STYLES.get(class)?;
        let name = if second {
            format!("shadowlight_{style}_terrain_2.dds")
        } else {
            format!("shadowlight_{style}_terrain.dds")
        };
        self.view(&name)
    }

    /// [`Self::terrain_sunlight`] read through an sRGB format, as the bake of the page reads it.
    pub fn terrain_sunlight_srgb(&self, class: usize) -> Option<&wgpu::TextureView> {
        let name = format!("sunlight_{}_terrain.dds", STYLES.get(class)?);
        self.srgb_cubes.get(&name).map(|t| &t.view)
    }

    /// [`Self::terrain_shadowlight`] read through an sRGB format, as the bake of the page reads it.
    pub fn terrain_shadowlight_srgb(
        &self,
        class: usize,
        second: bool,
    ) -> Option<&wgpu::TextureView> {
        let style = STYLES.get(class)?;
        let name = if second {
            format!("shadowlight_{style}_terrain_2.dds")
        } else {
            format!("shadowlight_{style}_terrain.dds")
        };
        self.srgb_cubes.get(&name).map(|t| &t.view)
    }

    /// A cube of the models ([`MODEL_CUBES`]) read through an sRGB format, as the D3D11 build binds
    /// the cubes of `BuildingPS_CODE` (S3 ev 13046: `BC1_SRGB`).
    pub fn model_cube_srgb(&self, file: &str) -> Option<&wgpu::TextureView> {
        self.srgb_cubes.get(file).map(|t| &t.view)
    }

    pub fn spec_screen_map(&self) -> Option<&wgpu::TextureView> {
        self.view(SPEC_SCREEN_MAP)
    }

    /// The table `color_keys[k][class]` of the terrain: 3 kinds by 5 classes, 15 in all.
    pub fn terrain_color_key(&self, k: usize, class: usize) -> Option<&wgpu::TextureView> {
        self.view(&terrain_table_file(k, class)?)
    }

    /// How many of the 15 tables of the terrain are there.
    pub fn terrain_tables_loaded(&self) -> usize {
        (0..TERRAIN_KEY_SYSTEMS.len())
            .flat_map(|k| (0..TERRAIN_KEY_CLASSES).map(move |c| (k, c)))
            .filter(|&(k, c)| self.terrain_color_key(k, c).is_some())
            .count()
    }

    // ---- models ----

    pub fn unit_irradiance(&self) -> Option<&wgpu::TextureView> {
        self.view(UNIT_IRRADIANCE)
    }

    pub fn unit_environment_dull(&self) -> Option<&wgpu::TextureView> {
        self.view(UNIT_ENVIRONMENT_DULL)
    }

    pub fn unit_environment_sharp(&self) -> Option<&wgpu::TextureView> {
        self.view(UNIT_ENVIRONMENT_SHARP)
    }

    /// The colour-key table of the models of a style (`Buildings_*`).
    pub fn model_color_key(&self, class: usize) -> Option<&wgpu::TextureView> {
        self.view(&model_table_file(class)?)
    }

    pub fn rivers_color_key(&self) -> Option<&wgpu::TextureView> {
        self.view(RIVERS_TABLE)
    }

    /// The ramp of the distance fog.
    pub fn fog_ramp(&self) -> Option<&wgpu::TextureView> {
        self.view(FOG_RAMP)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terrain_has_fifteen_tables_and_the_files_are_the_pipelines() {
        let mut files = Vec::new();
        for k in 0..3 {
            for class in 0..5 {
                files.push(terrain_table_file(k, class).unwrap());
            }
        }
        assert_eq!(files.len(), 15);
        let unique: std::collections::BTreeSet<_> = files.iter().collect();
        assert_eq!(unique.len(), 15);
        assert_eq!(terrain_table_file(3, 0), None);
        assert_eq!(terrain_table_file(0, 5), None);
        assert_eq!(
            terrain_table_file(1, 3).as_deref(),
            Some("terrainshadow_europe_colorkey.tga")
        );
        // All of them are in the list the boot group has.
        let required = required_files();
        for f in &files {
            assert!(required.contains(f), "{f}");
        }
    }

    #[test]
    fn the_required_files_match_the_set_b2_light() {
        let files = required_files();
        let unique: std::collections::BTreeSet<_> = files.iter().collect();
        assert_eq!(unique.len(), files.len(), "no file twice");
        // 5 + 5 + 3 cubes of the terrain, 4 + 6 of the models, specscreenmap, fogramp, 15 + 5 + 2
        // tables.
        assert_eq!(files.len(), 13 + 10 + 2 + 22);
        assert!(files.contains(&"sunlight_europe_terrain.dds".to_owned()));
        assert!(files.contains(&"shadowlight_asia_terrain_2.dds".to_owned()));
        assert!(!files.contains(&"shadowlight_africa_terrain_2.dds".to_owned()));
        let ids = required_ids();
        assert_eq!(ids.len(), files.len() + 1);
        assert!(ids.contains(&LogicalId::new("tb:world-b2")));
        assert_eq!(file_of(&texture_id("a.dds")), Some("a.dds"));
    }

    #[test]
    fn every_file_has_the_shape_the_pipeline_makes() {
        for f in required_files() {
            let dim = expected_dim(&f);
            let want = if f.ends_with(".tga") {
                TexDim::D3
            } else if f == SPEC_SCREEN_MAP || f == FOG_RAMP {
                TexDim::D2
            } else {
                TexDim::Cube
            };
            assert_eq!(dim, want, "{f}");
        }
    }

    #[test]
    fn nothing_is_ready_before_the_boot_group_arrives() {
        let env = LightingEnv::new();
        assert!(!env.ready());
        assert_eq!(env.missing().len(), required_files().len());
        assert_eq!(env.progress(), (0, required_files().len() + 1));
        assert_eq!(env.terrain_tables_loaded(), 0);
        assert!(env.terrain_color_key(0, 0).is_none());
        assert!(env.fog_ramp().is_none());
    }
}
