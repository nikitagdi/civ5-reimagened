//! Scene files `scenes/*.toml`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Scene ids in the order of the capture instruction.
pub const SCENE_IDS: [&str; 4] = ["S1", "S2", "S3", "S4"];

/// Scenes with no capture of the original: our frame against our frame (`golden far`). S5 is the far view
/// (B3 far LOD): the game's camera at the farthest zoom over the world of another scene, drawn with `FarLOD`
/// off and on.
pub const OWN_SCENE_IDS: [&str; 1] = ["S5"];

/// Captures that may be missing: S4-attack (only when the player had a ranged unit) and
/// S1b (the variance pair of S1a; without it the noise step is skipped).
const OPTIONAL_CAPTURES: [&str; 2] = ["S1b", "S4-attack"];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub scene: SceneHead,
    pub setup: Setup,
    pub view: View,
    pub render: Render,
    pub compare: Compare,
    /// The far view of an own scene ([`OWN_SCENE_IDS`]).
    #[serde(default)]
    pub far: Option<FarView>,
}

/// What the far-view scene draws: the world of another scene (`cache/golden/<world>`, made by `golden world`)
/// from the game's camera at a zoom level.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarView {
    pub world: String,
    pub level: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneHead {
    pub id: String,
    /// `cache/captures/<id>/<name>.rdc`
    pub captures: Vec<String>,
}

/// What the original game was set up with. S3 (benchmark) only has `scenario_map`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub sync_seed: Option<u32>,
    pub map_seed: Option<u32>,
    pub world_size: Option<String>,
    pub map_script: Option<String>,
    pub human: Option<String>,
    #[serde(default)]
    pub ai: Vec<String>,
    #[serde(default)]
    pub city_states: Vec<String>,
    pub handicap: Option<String>,
    pub speed: Option<String>,
    pub era: Option<String>,
    pub scenario_map: Option<String>,
    /// The `world_builder_map` of the core (gc-abi-v2 4.15): the `.Civ5Map` as a path of the game's VFS
    /// (no `{game}/`). With it the scene is built from the map and its scenario data, not from seeds.
    pub world_builder_map: Option<String>,
    /// A `.Civ5Save` of the original taken with the capture (path under `cache/captures/`): `golden world`
    /// continues that game instead of making one from the seeds.
    pub save: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub player: u32,
    /// `game` (S1, S2) or `capture-mask` (S3).
    pub reveal: String,
    /// The only value is `all` (G-D13).
    pub basis_nw: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Render {
    pub preset: String,
    pub size: [u32; 2],
    pub msaa: u32,
    #[serde(default)]
    pub disable: Vec<String>,
    pub grid: bool,
    /// The longest the scene may take to settle, in seconds (the client's 120 without it).
    #[serde(default)]
    pub settle_s: Option<u32>,
    pub camera: String,
    #[serde(default)]
    pub time_constants: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compare {
    pub masks: Vec<String>,
}

impl Scene {
    pub fn parse(text: &str) -> Result<Self> {
        let s: Scene = toml::from_str(text).context("scene toml")?;
        s.validate()?;
        Ok(s)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        Self::parse(&text).with_context(|| path.display().to_string())
    }

    pub fn load_by_id(dir: &Path, id: &str) -> Result<Self> {
        let s = Self::load(&dir.join(format!("{id}.toml")))?;
        if s.scene.id != id {
            bail!("scene file {id}.toml has id {:?}", s.scene.id);
        }
        Ok(s)
    }

    fn validate(&self) -> Result<()> {
        let own = OWN_SCENE_IDS.contains(&self.scene.id.as_str());
        if !SCENE_IDS.contains(&self.scene.id.as_str()) && !own {
            bail!("unknown scene id {:?}", self.scene.id);
        }
        if own != self.far.is_some() {
            bail!("a [far] section belongs to the own scenes ({OWN_SCENE_IDS:?}) and only to them");
        }
        if let Some(f) = &self.far
            && !SCENE_IDS.contains(&f.world.as_str())
        {
            bail!("far.world {:?} is not a scene with a capture", f.world);
        }
        if self.scene.captures.is_empty() {
            bail!("scene {} has no captures", self.scene.id);
        }
        if self.view.basis_nw != "all" {
            bail!("basis_nw must be \"all\" (G-D13)");
        }
        if !matches!(self.view.reveal.as_str(), "game" | "capture-mask") {
            bail!("reveal must be \"game\" or \"capture-mask\"");
        }
        Ok(())
    }

    /// Captures `check-captures` requires.
    pub fn required_captures(&self) -> Vec<&str> {
        self.scene
            .captures
            .iter()
            .map(String::as_str)
            .filter(|c| !OPTIONAL_CAPTURES.contains(c))
            .collect()
    }

    /// `<cache>/captures/<id>/<name>.rdc`
    pub fn capture_path(&self, cache: &Path, name: &str) -> PathBuf {
        cache
            .join("captures")
            .join(&self.scene.id)
            .join(format!("{name}.rdc"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scenes_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes")
    }

    fn get(id: &str) -> Scene {
        Scene::load_by_id(&scenes_dir(), id).unwrap()
    }

    #[test]
    fn all_scene_files_parse_and_match_spec() {
        for id in SCENE_IDS {
            let s = get(id);
            assert_eq!(s.render.size, [1920, 1080]);
            assert_eq!(s.render.msaa, 4);
            assert!(s.render.disable.contains(&"YieldIcons".to_string()));
            // The captures show the hex grid (the overlay map of ev 741 and the cells'
            // `TerrainOverlay`). S3 has no `GridShader` draw: its overlay map was drawn in an
            // earlier frame, and its `ScreenSpaceOverlayPS` draws show the grid over land and water.
            assert!(s.render.grid, "{id}");
        }
    }

    #[test]
    fn captures_per_scene() {
        assert_eq!(get("S1").scene.captures, ["S1a", "S1b"]);
        assert_eq!(get("S2").scene.captures, ["S2-bake", "S2"]);
        assert_eq!(get("S3").scene.captures, ["S3"]);
        let s4 = get("S4");
        assert_eq!(s4.scene.captures, ["S4-select", "S4-path", "S4-attack"]);
        assert_eq!(s4.required_captures(), ["S4-select", "S4-path"]);
        assert_eq!(get("S1").required_captures(), ["S1a"]);
    }

    #[test]
    fn s1_s2_s4_share_the_setup_and_keep_the_fog() {
        let (s1, s2, s4) = (get("S1"), get("S2"), get("S4"));
        for s in [&s1, &s2, &s4] {
            assert_eq!(s.setup.sync_seed, Some(12345));
            assert_eq!(s.setup.map_seed, Some(12345));
            assert_eq!(s.setup.human.as_deref(), Some("CIVILIZATION_ROME"));
            assert_eq!(s.setup.ai, ["CIVILIZATION_GREECE"]);
            assert!(s.setup.city_states.is_empty());
            assert_eq!(s.view.reveal, "game");
        }
        // The scene description drops the fog in S2, but its captures were made with it (RP30).
        for s in [&s1, &s2, &s4] {
            assert!(!s.render.disable.contains(&"FOW".to_string()));
            assert!(!s.render.disable.contains(&"FluidFOW".to_string()));
        }
    }

    #[test]
    fn s3_is_the_benchmark_map() {
        let s3 = get("S3");
        assert!(
            s3.setup
                .scenario_map
                .unwrap()
                .ends_with("LateGameTest.Civ5Map")
        );
        assert_eq!(
            s3.setup.world_builder_map.as_deref(),
            Some("maps/Tutorials/Maps/LateGameTest.Civ5Map")
        );
        assert_eq!(s3.view.reveal, "capture-mask");
    }

    /// S5, the far view: the world of S3 at the farthest zoom of the game's camera.
    #[test]
    fn s5_is_the_far_view_over_s3() {
        let s5 = get("S5");
        let far = s5.far.as_ref().expect("a [far] section");
        assert_eq!(far.world, "S3");
        assert_eq!(
            far.level, 20.5,
            "LEVEL_MAX of the camera, where the bench's far phases are"
        );
        assert_eq!(s5.scene.captures, ["S5-far"]);
        assert_eq!(s5.render.size, [1920, 1080]);
        assert_eq!(s5.render.camera, "game");
        // A capture scene with a [far] section is not a scene.
        let bad = std::fs::read_to_string(scenes_dir().join("S1.toml")).unwrap()
            + "[far]\nworld = \"S3\"\nlevel = 20.5\n";
        assert!(Scene::parse(&bad).is_err());
    }

    #[test]
    fn rejects_bad_scene() {
        let bad = std::fs::read_to_string(scenes_dir().join("S1.toml"))
            .unwrap()
            .replace("basis_nw = \"all\"", "basis_nw = \"none\"");
        assert!(Scene::parse(&bad).is_err());
    }
}
