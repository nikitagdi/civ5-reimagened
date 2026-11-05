//! Unit showcase: loads the Warrior model (or the synthetic demo model), places it at the scene
//! centre and loops its animations. `N` (or a 5 s timer on touch devices) cycles them.

use std::collections::HashMap;
use std::rc::Rc;

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use glam::{Mat4, Vec3};
use platform::{InputEvent, Key, PointerKind};
use render::asset_format::{AnimationBlob, ModelBlob, TextureBlob};
use render::synthetic;
use render::{Camera, GpuModel, InstanceId, PoseEvaluator, Renderer};

/// Logical id of the model shown by default.
pub(crate) const WARRIOR_MODEL_ID: &str = "model:art_def_unit_member_warrior";
/// `fScale` of ART_DEF_UNIT_MEMBER_WARRIOR in Civ5ArtDefines_UnitMembers.xml. Blobs do not bake it.
const WARRIOR_FSCALE: f32 = 0.14;
/// Engine world units per Civ5 world unit. Same constant as
/// `asset_pipeline::models::set::ENGINE_UNITS_PER_CIV5_UNIT`, which the glTF export uses.
const ENGINE_UNITS_PER_CIV5_UNIT: f32 = 1.0;
/// The camera framing below was tuned when one Civ5 unit was `sqrt(3) / 128` engine units.
const OLD_ENGINE_UNITS_PER_CIV5_UNIT: f32 = 1.732_050_8 / 128.0;
/// Multiplies the camera framing so the Warrior keeps its old size on screen.
const FRAMING_SCALE: f32 = ENGINE_UNITS_PER_CIV5_UNIT / OLD_ENGINE_UNITS_PER_CIV5_UNIT;
/// Total model scale: Warrior is about 118 Civ5 units tall, so about 16.5 engine units.
pub(crate) const WARRIOR_SCALE: f32 = WARRIOR_FSCALE * ENGINE_UNITS_PER_CIV5_UNIT;
/// The synthetic model is authored in world units.
const DEMO_SCALE: f32 = 1.0;
/// Touch devices have no `N` key: advance by timer.
const TOUCH_CYCLE_SECS: f32 = 5.0;
/// Camera framing when the model appears.
const VIEW_DISTANCE: f32 = 2.4 * FRAMING_SCALE;
const VIEW_HEIGHT: f32 = 0.13 * FRAMING_SCALE;

/// What to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Load `WARRIOR_MODEL_ID` through the asset loader.
    Warrior,
    /// Inject the synthetic skinned model, no network.
    SyntheticDemo,
}

struct Playing {
    instance: InstanceId,
    model: Rc<GpuModel>,
    /// Loaded clips only.
    clips: Vec<(String, AnimationBlob)>,
    current: usize,
    started_ms: u64,
    pose: PoseEvaluator,
}

struct Deps {
    model: ModelBlob,
    textures: Vec<(LogicalId, AssetHandle)>,
    clips: Vec<(usize, AssetHandle)>,
    loaded_textures: HashMap<LogicalId, TextureBlob>,
    loaded_clips: Vec<Option<AnimationBlob>>,
}

enum Stage {
    /// Waiting for the renderer before anything is requested.
    Start,
    Model(AssetHandle),
    Deps(Box<Deps>),
    Playing(Box<Playing>),
    /// Nothing to show (already logged).
    Done,
}

pub(crate) struct Showcase {
    source: Source,
    stage: Stage,
    touch_seen: bool,
    cycle_requested: bool,
}

impl Showcase {
    pub(crate) fn new(source: Source) -> Self {
        Self {
            source,
            stage: Stage::Start,
            touch_seen: false,
            cycle_requested: false,
        }
    }

    /// True once a model is on screen; the test sphere is hidden then.
    pub(crate) fn is_showing(&self) -> bool {
        matches!(self.stage, Stage::Playing(_))
    }

    pub(crate) fn handle_input(&mut self, event: &InputEvent) {
        match event {
            InputEvent::Key(k)
                if k.pressed
                    && !k.repeat
                    && matches!(&k.key, Key::Character(c) if c.eq_ignore_ascii_case("n")) =>
            {
                self.cycle_requested = true;
            }
            InputEvent::Pointer(p) if p.kind == PointerKind::Touch => self.touch_seen = true,
            _ => {}
        }
    }

    /// Advances loading and animation; call once per frame after the renderer exists.
    pub(crate) fn update(
        &mut self,
        now_ms: u64,
        renderer: &mut Renderer,
        loader: Option<&mut AssetLoader>,
        camera: &mut Camera,
    ) {
        let cycle = std::mem::take(&mut self.cycle_requested);
        let stage = std::mem::replace(&mut self.stage, Stage::Done);
        self.stage = match stage {
            Stage::Start => self.start(renderer, loader, now_ms, camera),
            Stage::Model(h) => match loader {
                Some(l) => poll_model(l, h, renderer),
                None => Stage::Done,
            },
            Stage::Deps(d) => match loader {
                Some(l) => poll_deps(l, *d, renderer, now_ms, camera),
                None => Stage::Done,
            },
            Stage::Playing(mut p) => {
                self.tick(&mut p, renderer, now_ms, cycle);
                Stage::Playing(p)
            }
            Stage::Done => Stage::Done,
        };
    }

    fn start(
        &self,
        renderer: &mut Renderer,
        loader: Option<&mut AssetLoader>,
        now_ms: u64,
        camera: &mut Camera,
    ) -> Stage {
        match self.source {
            Source::SyntheticDemo => {
                tracing::info!("demo: showing the synthetic skinned model");
                let s = synthetic::skinned_cylinder();
                let clips = s.animations.into_iter().map(Some).collect();
                begin_playing(
                    renderer,
                    &s.model,
                    &s.textures,
                    clips,
                    DEMO_SCALE,
                    now_ms,
                    camera,
                )
            }
            Source::Warrior => {
                let Some(loader) = loader else {
                    tracing::info!("assets are disabled; keeping the test scene");
                    return Stage::Done;
                };
                let profile = renderer.platform_profile();
                tracing::info!(?profile, "requesting {WARRIOR_MODEL_ID}");
                Stage::Model(loader.request(LogicalId::new(WARRIOR_MODEL_ID), &profile))
            }
        }
    }

    fn tick(&self, p: &mut Playing, renderer: &Renderer, now_ms: u64, cycle: bool) {
        let elapsed = now_ms.saturating_sub(p.started_ms) as f32 / 1000.0;
        let auto = self.touch_seen && elapsed >= TOUCH_CYCLE_SECS;
        if (cycle || auto) && !p.clips.is_empty() {
            p.current = next_index(p.current, p.clips.len());
            p.started_ms = now_ms;
            tracing::info!(animation = %p.clips[p.current].0, "animation {}/{}", p.current + 1, p.clips.len());
        }
        let t = now_ms.saturating_sub(p.started_ms) as f32 / 1000.0;
        let clip = p.clips.get(p.current).map(|(_, a)| a);
        let skin = p.pose.evaluate(p.model.skeleton(), clip, t, true);
        renderer.set_pose(p.instance, skin);
    }
}

fn poll_model(loader: &mut AssetLoader, handle: AssetHandle, renderer: &Renderer) -> Stage {
    let bytes = match loader.poll(handle) {
        AssetPoll::Pending => return Stage::Model(handle),
        AssetPoll::Ready(b) => b,
        AssetPoll::Failed(e) => {
            tracing::warn!("{WARRIOR_MODEL_ID} unavailable ({e}); keeping the test scene");
            return Stage::Done;
        }
    };
    let model = match ModelBlob::decode(&bytes) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("{WARRIOR_MODEL_ID} is not a valid model blob: {e}");
            return Stage::Done;
        }
    };
    let profile = renderer.platform_profile();
    let mut texture_ids: Vec<LogicalId> = Vec::new();
    for id in model.materials.iter().filter_map(|m| m.diffuse.as_ref()) {
        if !texture_ids.contains(id) {
            texture_ids.push(id.clone());
        }
    }
    let textures = texture_ids
        .into_iter()
        .map(|id| {
            let h = loader.request(id.clone(), &profile);
            (id, h)
        })
        .collect();
    let clips: Vec<(usize, AssetHandle)> = model
        .animations
        .iter()
        .enumerate()
        .map(|(i, a)| (i, loader.request(a.blob.clone(), &profile)))
        .collect();
    tracing::info!(
        meshes = model.meshes.len(),
        bones = model.skeleton.bones.len(),
        animations = clips.len(),
        "model loaded, fetching textures and animations"
    );
    Stage::Deps(Box::new(Deps {
        loaded_clips: vec![None; clips.len()],
        model,
        textures,
        clips,
        loaded_textures: HashMap::new(),
    }))
}

fn poll_deps(
    loader: &mut AssetLoader,
    mut d: Deps,
    renderer: &mut Renderer,
    now_ms: u64,
    camera: &mut Camera,
) -> Stage {
    d.textures.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match TextureBlob::decode(&bytes) {
                Ok(t) => {
                    d.loaded_textures.insert(id.clone(), t);
                }
                Err(e) => tracing::warn!(id = id.as_str(), "bad texture blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(id = id.as_str(), "texture unavailable ({e})");
            false
        }
    });
    d.clips.retain(|(i, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match AnimationBlob::decode(&bytes) {
                Ok(a) => d.loaded_clips[*i] = Some(a),
                Err(e) => tracing::warn!(index = *i, "bad animation blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(index = *i, "animation unavailable ({e})");
            false
        }
    });
    if !d.textures.is_empty() || !d.clips.is_empty() {
        return Stage::Deps(Box::new(d));
    }
    begin_playing(
        renderer,
        &d.model,
        &d.loaded_textures,
        d.loaded_clips,
        WARRIOR_SCALE,
        now_ms,
        camera,
    )
}

/// Uploads the model, adds it at the scene centre and starts the idle clip.
/// `clips` is parallel to `model.animations`; `None` entries are skipped.
fn begin_playing(
    renderer: &mut Renderer,
    model: &ModelBlob,
    textures: &HashMap<LogicalId, TextureBlob>,
    clips: Vec<Option<AnimationBlob>>,
    scale: f32,
    now_ms: u64,
    camera: &mut Camera,
) -> Stage {
    let clips: Vec<(String, AnimationBlob)> = model
        .animations
        .iter()
        .zip(clips)
        .filter_map(|(r, a)| a.map(|a| (r.name.clone(), a)))
        .collect();
    let gpu = renderer.create_model(model, textures);
    let center = renderer.scene_center();
    let instance = renderer.add_model_instance(
        Rc::clone(&gpu),
        Mat4::from_translation(center) * Mat4::from_scale(Vec3::splat(scale)),
    );
    camera.target = center + Vec3::Y * VIEW_HEIGHT;
    camera.distance = VIEW_DISTANCE;
    let names: Vec<&str> = clips.iter().map(|(n, _)| n.as_str()).collect();
    let current = initial_clip(&names);
    match clips.get(current) {
        Some((name, _)) => {
            tracing::info!(animation = %name, "model placed; {} animations, N cycles", clips.len())
        }
        None => tracing::info!("model placed without animations (rest pose)"),
    }
    Stage::Playing(Box::new(Playing {
        instance,
        model: gpu,
        clips,
        current,
        started_ms: now_ms,
        pose: PoseEvaluator::new(),
    }))
}

/// First clip whose name contains "idle" (case-insensitive), else the first.
fn initial_clip(names: &[&str]) -> usize {
    names
        .iter()
        .position(|n| n.to_ascii_lowercase().contains("idle"))
        .unwrap_or(0)
}

fn next_index(current: usize, len: usize) -> usize {
    if len == 0 { 0 } else { (current + 1) % len }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_preferred_case_insensitively() {
        assert_eq!(initial_clip(&["Run", "UNIT_Idle01", "idle2"]), 1);
        assert_eq!(initial_clip(&["Run", "Attack"]), 0);
        assert_eq!(initial_clip(&[]), 0);
    }

    #[test]
    fn cycling_wraps() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 0);
        assert_eq!(next_index(0, 1), 0);
        assert_eq!(next_index(0, 0), 0);
    }

    #[test]
    fn n_key_requests_a_cycle_once() {
        use platform::{KeyEvent, Modifiers};
        let key = |c: &str, pressed, repeat| {
            InputEvent::Key(KeyEvent {
                key: Key::Character(c.into()),
                code: None,
                pressed,
                repeat,
                modifiers: Modifiers::default(),
            })
        };
        let mut s = Showcase::new(Source::Warrior);
        s.handle_input(&key("x", true, false));
        s.handle_input(&key("n", true, true));
        s.handle_input(&key("n", false, false));
        assert!(!s.cycle_requested);
        s.handle_input(&key("N", true, false));
        assert!(s.cycle_requested);
    }
}
