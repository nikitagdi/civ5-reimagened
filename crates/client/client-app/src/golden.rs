//! Our frame for `golden` (render-core 4.14): a scene saved by `golden world`
//! (`mapbasis.wv3`, `world.wv3`, `catalog.wv3`) drawn from the camera and the time constants of a capture, with no
//! session and no server. The same driver runs natively without a window (`run_native`, `app-desktop --golden`)
//! and in the browser (`?test=1&golden=<url>`, the frame loop of `App`).
//!
//! The view comes from the files as the stream would give it: the map basis first, then the keyframe with the
//! hash of that basis. It goes through the same `WorldBridge` as a game, so the frame is what a player would see
//! of that view. The camera is the capture's: `P_ours = P_orig * ours_to_orig`, with the `ours_to_orig` the
//! renderer has for the map (`Renderer::ours_to_orig`), read back into a `FixedCamera`.
//!
//! A capture's file `<capture>.golden` (written by `golden render`) says what to draw. Plain lines, `key values`:
//!
//! ```text
//! civ5-golden/1
//! capture S1a
//! size 1914 1051
//! preset high
//! msaa 4
//! disable YieldIcons
//! grid 0
//! time 12.5
//! view_proj_orig <16 floats, the columns of the matrix, in the basis of the original>
//! const g_ShadowFade 1
//! ring 415.98 -166.24 -1.21 0
//! contour <x y pairs: centroids of the triangles of the outline of the reachable cells>
//! gfx FarLOD:0
//! game_camera 20.5
//! ```
//!
//! `gfx` puts settings on top of the preset; `game_camera` (the far-view scene S5, which has no capture)
//! draws from the game's camera at that zoom level over the player's start, in place of `view_proj_orig`.

use std::collections::HashMap;

use assets::AssetLoader;
use client_core::ViewChange;
use draw_list::UiDrawList;
use glam::Mat4;
use platform::{HttpClient, HttpOp};
use protocol::{BasisState, Catalog, MapBasis, WorldView, wv3};
use render::{FixedCamera, FrameSource, GameCamera, Image, Readback, Renderer, Scene};

use crate::asset_boot::{LightBoot, TerrainBoot, WaterBoot};
use crate::bridge::WorldBridge;
use crate::entity_assets::EntityAssets;
use crate::icon_assets::IconAssets;
use crate::settings_store::GraphicsOptions;
use crate::terrain_assets::TerrainAssets;

/// First line of a `.golden` file.
pub const SPEC_HEADER: &str = "civ5-golden/1";
/// The frame is final when nothing was loading for this many frames in a row.
const QUIET_FRAMES: u32 = 5;
/// The longest the scene may take to settle.
const SETTLE_LIMIT_MS: u64 = 120_000;

// ------------------------------------------------------------------ what to draw

/// One capture's settings of the frame.
#[derive(Debug, Clone, PartialEq)]
pub struct GoldenSpec {
    pub capture: String,
    pub size: (u32, u32),
    pub preset: String,
    pub msaa: u32,
    /// `[DisableSystems]` names.
    pub disable: Vec<String>,
    pub grid: bool,
    /// Pins the time the shaders see (`Renderer::set_time_override`).
    pub time: Option<f32>,
    /// The columns of the capture's view-projection, in the basis of the original.
    pub view_proj_orig: [f32; 16],
    /// Constants of the capture by name (`Renderer::set_constant_overrides`).
    pub constants: Vec<(String, Vec<f32>)>,
    /// The selection ring of the capture (its `PSValidMoves` draw): the centre in the original's
    /// basis (x, y), the turn of the ring in radians and `fAlpha`.
    pub ring: Option<[f32; 4]>,
    /// The outline of the reachable cells of the capture (its `PSContour` draw): the centroids of the
    /// triangles of the strip in the original's basis. The cells under them and the cells they enclose
    /// are the reachable set.
    pub contour: Vec<[f32; 2]>,
    /// `reveal all`: every plot of the view is revealed and visible (the fog of the capture, where the
    /// game's reveal is not it: S3).
    pub reveal_all: bool,
    /// `settle <seconds>`: the longest the scene may take to settle (`SETTLE_LIMIT_MS` without it): the
    /// whole map of S3 takes longer to build.
    pub settle_ms: u64,
    /// `gfx Key:value,...`: settings on top of the preset (`FarLOD:0`), as `?gfx=` takes them.
    pub gfx: Option<String>,
    /// `game_camera <level>`: the game's own camera at that zoom level over the middle of the map
    /// instead of the capture's (the far-view scene S5); `view_proj_orig` is not needed then.
    pub game_camera: Option<f32>,
}

impl GoldenSpec {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        if lines.next() != Some(SPEC_HEADER) {
            return Err(format!(
                "not a golden spec: the first line must be {SPEC_HEADER}"
            ));
        }
        let (mut capture, mut size, mut view_proj) = (None, None, None);
        let mut spec = GoldenSpec {
            capture: String::new(),
            size: (0, 0),
            preset: "high".into(),
            msaa: 4,
            disable: Vec::new(),
            grid: false,
            time: None,
            view_proj_orig: [0.0; 16],
            constants: Vec::new(),
            ring: None,
            contour: Vec::new(),
            reveal_all: false,
            settle_ms: SETTLE_LIMIT_MS,
            gfx: None,
            game_camera: None,
        };
        for line in lines {
            let (key, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let rest = rest.trim();
            match key {
                "capture" => capture = Some(rest.to_owned()),
                "size" => {
                    let n = numbers::<u32>(rest, "size")?;
                    let [w, h] = <[u32; 2]>::try_from(n).map_err(|_| "size needs w h")?;
                    size = Some((w, h));
                }
                "preset" => spec.preset = rest.to_owned(),
                "msaa" => spec.msaa = rest.parse().map_err(|_| format!("msaa {rest:?}"))?,
                "disable" => {
                    spec.disable = rest
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                }
                "grid" => spec.grid = rest == "1",
                "settle" => {
                    let s: u64 = rest.parse().map_err(|_| format!("settle {rest:?}"))?;
                    spec.settle_ms = s * 1000;
                }
                "reveal" => match rest {
                    "all" => spec.reveal_all = true,
                    other => return Err(format!("reveal {other:?}: only all")),
                },
                "time" => spec.time = Some(rest.parse().map_err(|_| format!("time {rest:?}"))?),
                "gfx" => spec.gfx = (!rest.is_empty()).then(|| rest.to_owned()),
                "game_camera" => {
                    let level: f32 = rest
                        .parse()
                        .map_err(|_| format!("game_camera {rest:?}: a zoom level"))?;
                    spec.game_camera = Some(level);
                }
                "view_proj_orig" => {
                    let n = numbers::<f32>(rest, "view_proj_orig")?;
                    view_proj = Some(
                        <[f32; 16]>::try_from(n).map_err(|_| "view_proj_orig needs 16 numbers")?,
                    );
                }
                "const" => {
                    let (name, values) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                    if name.is_empty() {
                        return Err("const needs a name".into());
                    }
                    spec.constants
                        .push((name.to_owned(), numbers::<f32>(values, name)?));
                }
                "ring" => {
                    let n = numbers::<f32>(rest, "ring")?;
                    spec.ring =
                        Some(<[f32; 4]>::try_from(n).map_err(|_| "ring needs x y angle alpha")?);
                }
                "contour" => {
                    let n = numbers::<f32>(rest, "contour")?;
                    if n.len() % 2 != 0 {
                        return Err("contour needs x y pairs".into());
                    }
                    spec.contour = n.chunks_exact(2).map(|p| [p[0], p[1]]).collect();
                }
                other => return Err(format!("unknown key {other:?}")),
            }
        }
        spec.capture = capture.ok_or("no capture line")?;
        spec.size = size.ok_or("no size line")?;
        spec.view_proj_orig = match (view_proj, spec.game_camera) {
            (Some(m), _) => m,
            (None, Some(_)) => [0.0; 16],
            (None, None) => return Err("no view_proj_orig line".into()),
        };
        if spec.size.0 == 0 || spec.size.1 == 0 {
            return Err("size is zero".into());
        }
        Ok(spec)
    }

    /// The preset, the samples and the switches as the options of the app.
    pub fn graphics_options(&self) -> GraphicsOptions {
        let mut overrides = format!("MSAASamples:{}", self.msaa);
        if let Some(gfx) = &self.gfx {
            overrides = format!("{overrides},{gfx}");
        }
        GraphicsOptions {
            preset: Some(self.preset.clone()),
            overrides: Some(overrides),
            disable: (!self.disable.is_empty()).then(|| self.disable.join(",")),
            debug: None,
        }
    }
}

/// The reachable cells of a capture: the cells under the points of its contour strip, and the cells the strip
/// encloses (found by filling from outside the box of the strip; the strip lies inside the edge cells).
fn reach_cells(
    contour: &[[f32; 2]],
    from_orig: impl Fn(f32, f32) -> (i32, i32),
) -> Vec<(i32, i32)> {
    use std::collections::HashSet;
    if contour.is_empty() {
        return Vec::new();
    }
    let edge: HashSet<(i32, i32)> = contour.iter().map(|p| from_orig(p[0], p[1])).collect();
    // The neighbours of a cell, by the centres: 6 steps of one hex width.
    let neighbours = |c: (i32, i32)| -> Vec<(i32, i32)> {
        let at = render::hexgrid::cell_center(c.0, c.1);
        (0..6)
            .map(|k| {
                let a = std::f32::consts::FRAC_PI_3 * k as f32;
                render::hexgrid::nearest_cell(
                    at + glam::Vec2::new(a.cos(), a.sin()) * render::hexgrid::HEX_WIDTH,
                )
            })
            .collect()
    };
    let (mut x0, mut x1, mut y0, mut y1) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, y) in &edge {
        (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
    }
    let (x0, x1, y0, y1) = (x0 - 2, x1 + 2, y0 - 2, y1 + 2);
    let inside_box = |c: (i32, i32)| c.0 >= x0 && c.0 <= x1 && c.1 >= y0 && c.1 <= y1;
    let mut outside: HashSet<(i32, i32)> = HashSet::new();
    let mut stack = vec![(x0, y0)];
    while let Some(c) = stack.pop() {
        if !inside_box(c) || edge.contains(&c) || !outside.insert(c) {
            continue;
        }
        stack.extend(neighbours(c));
    }
    let mut out: Vec<(i32, i32)> = (y0..=y1)
        .flat_map(|y| (x0..=x1).map(move |x| (x, y)))
        .filter(|c| !outside.contains(c))
        .collect();
    out.sort_unstable();
    out
}

fn numbers<T: std::str::FromStr>(text: &str, what: &str) -> Result<Vec<T>, String> {
    text.split_whitespace()
        .map(|w| {
            w.parse()
                .map_err(|_| format!("{what}: {w:?} is not a number"))
        })
        .collect()
}

// ------------------------------------------------------------------ the scene

/// The bytes of the files of a scene and one capture's spec.
#[derive(Debug, Clone)]
pub struct GoldenFiles {
    pub basis: Vec<u8>,
    pub world: Vec<u8>,
    pub catalog: Option<Vec<u8>>,
    pub spec: String,
}

impl GoldenFiles {
    /// Reads the files of the folder of a scene.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_dir(dir: &std::path::Path, capture: &str) -> Result<Self, String> {
        let read = |name: &str| {
            std::fs::read(dir.join(name)).map_err(|e| format!("{}: {e}", dir.join(name).display()))
        };
        Ok(Self {
            basis: read("mapbasis.wv3")?,
            world: read("world.wv3")?,
            catalog: read("catalog.wv3").ok(),
            spec: String::from_utf8(read(&format!("{capture}.golden"))?)
                .map_err(|e| format!("{capture}.golden: {e}"))?,
        })
    }
}

/// What the files hold: the view as the stream gives it.
pub struct GoldenScene {
    pub basis: MapBasis,
    pub view: WorldView,
    pub catalog: Catalog,
}

impl GoldenScene {
    /// The map basis first and the keyframe second, as `Basis` before `Full`; the hash of the basis must be
    /// the one the keyframe was built with.
    pub fn load(files: &GoldenFiles) -> Result<Self, String> {
        let basis = BasisState::new(wv3::decode_basis(&files.basis)?)
            .map_err(|e| format!("mapbasis.wv3: {e:?}"))?;
        let keyframe = wv3::decode_world(&files.world)?;
        if keyframe.basis_hash != basis.hash() {
            return Err(format!(
                "world.wv3 was made with the basis {:016x}, mapbasis.wv3 has {:016x}",
                keyframe.basis_hash,
                basis.hash()
            ));
        }
        let catalog = match &files.catalog {
            Some(bytes) => wv3::decode_catalog(bytes)?,
            None => {
                tracing::warn!(
                    "no catalog.wv3 (run `golden world` again): the names behind the type numbers are missing"
                );
                Catalog::default()
            }
        };
        let mut view = keyframe.view;
        if GoldenSpec::parse(&files.spec).is_ok_and(|s| s.reveal_all) {
            for p in &mut view.map.plots {
                p.flags = protocol::PlotFlags(
                    p.flags.0 | protocol::PlotFlags::REVEALED | protocol::PlotFlags::VISIBLE,
                );
            }
        }
        Ok(Self {
            basis: basis.map().clone(),
            view,
            catalog,
        })
    }
}

// ------------------------------------------------------------------ fetching (web)

/// The files of a scene on their way over HTTP (`?golden=<url>&capture=<name>`).
pub struct GoldenFetch {
    ops: Vec<(&'static str, Box<dyn HttpOp>)>,
    got: HashMap<&'static str, Option<Vec<u8>>>,
    spec_name: String,
}

impl GoldenFetch {
    pub fn start(http: &dyn HttpClient, base: &str, capture: &str) -> Self {
        let base = base.trim_end_matches('/');
        let spec_name = format!("{capture}.golden");
        let names = ["mapbasis.wv3", "world.wv3", "catalog.wv3", "spec"];
        let ops = names
            .iter()
            .map(|&n| {
                let file = if n == "spec" { spec_name.as_str() } else { n };
                (n, http.get(&format!("{base}/{file}")))
            })
            .collect();
        Self {
            ops,
            got: HashMap::new(),
            spec_name,
        }
    }

    /// `None` while files are on their way.
    pub fn poll(&mut self) -> Option<Result<GoldenFiles, String>> {
        let mut failed = None;
        self.ops.retain_mut(|(name, op)| match op.poll() {
            None => true,
            Some(Ok(r)) if r.is_success() => {
                self.got.insert(name, Some(r.body));
                false
            }
            Some(Ok(r)) => {
                if *name != "catalog.wv3" {
                    failed = Some(format!("{name}: HTTP {}", r.status));
                }
                self.got.insert(name, None);
                false
            }
            Some(Err(e)) => {
                failed = Some(format!("{name}: {e}"));
                self.got.insert(name, None);
                false
            }
        });
        if let Some(e) = failed {
            return Some(Err(e));
        }
        if !self.ops.is_empty() {
            return None;
        }
        let mut take = |n: &str| self.got.remove(n).flatten();
        let (basis, world, catalog, spec) = (
            take("mapbasis.wv3"),
            take("world.wv3"),
            take("catalog.wv3"),
            take("spec"),
        );
        Some(match (basis, world, spec) {
            (Some(basis), Some(world), Some(spec)) => String::from_utf8(spec)
                .map_err(|e| format!("{}: {e}", self.spec_name))
                .map(|spec| GoldenFiles {
                    basis,
                    world,
                    catalog,
                    spec,
                }),
            _ => Err("a file of the scene is missing".into()),
        })
    }
}

// ------------------------------------------------------------------ the frame

/// The pixels of a finished frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenOutput {
    pub capture: String,
    pub size: (u32, u32),
    /// `Rgba8Unorm`, the world without the UI, rows from the top.
    pub rgba: Vec<u8>,
    /// The class mask, one byte per pixel (`R8Uint`).
    pub mask: Vec<u8>,
}

pub enum Progress {
    Running,
    Done(GoldenOutput),
    Failed(String),
}

enum Stage {
    Warm,
    Reading {
        world: Readback,
        mask: Readback,
        world_image: Option<Image>,
        mask_image: Option<Image>,
    },
    Finished,
}

/// Draws one capture of a scene until it is final, then reads the frame.
pub struct GoldenDriver {
    spec: GoldenSpec,
    scene: GoldenScene,
    bridge: WorldBridge,
    game_camera: GameCamera,
    light_boot: LightBoot,
    terrain_boot: TerrainBoot,
    water_boot: WaterBoot,
    terrain_assets: TerrainAssets,
    entity_assets: EntityAssets,
    icon_assets: IconAssets,
    ui: UiDrawList,
    fed: bool,
    started_ms: Option<u64>,
    quiet: u32,
    last_frame: Option<Mat4>,
    stage: Stage,
}

impl GoldenDriver {
    pub fn new(spec: GoldenSpec, scene: GoldenScene) -> Self {
        Self {
            spec,
            scene,
            bridge: WorldBridge::new(),
            game_camera: GameCamera::default(),
            light_boot: LightBoot::new(),
            terrain_boot: TerrainBoot::new(),
            water_boot: WaterBoot::new(),
            terrain_assets: TerrainAssets::new(),
            entity_assets: EntityAssets::new(),
            icon_assets: IconAssets::new(),
            ui: UiDrawList::new(),
            fed: false,
            started_ms: None,
            quiet: 0,
            last_frame: None,
            stage: Stage::Warm,
        }
    }

    pub fn spec(&self) -> &GoldenSpec {
        &self.spec
    }

    /// The view is in the renderer, the assets are all in, and nothing changed for a few frames.
    /// Without assets nothing is loading and nothing will: the flat-colour frame is as final as it gets.
    fn is_quiet(&self, renderer: &Renderer, assets: bool) -> bool {
        self.fed
            && renderer.world_drawn()
            && self.bridge.terrain().is_some()
            && (!assets
                || (renderer.settled()
                    && self.light_boot.map_may_draw()
                    && self.terrain_boot.idle()
                    && self.water_boot.idle()
                    && self.terrain_assets.idle()
                    && self.entity_assets.idle()))
    }

    /// One frame: loads what is missing, draws, and, once final, reads the frame back. Call it every frame
    /// of the loop until it says `Done` or `Failed`.
    pub fn frame(
        &mut self,
        renderer: &mut Renderer,
        mut loader: Option<&mut AssetLoader>,
        now_ms: u64,
    ) -> Progress {
        let started = *self.started_ms.get_or_insert(now_ms);
        if now_ms.saturating_sub(started) > self.spec.settle_ms {
            return Progress::Failed(format!(
                "the scene did not settle within {} s (renderer settled {} [pending: {}], world drawn {}, boot {}, terrain {}, water {}, terrain assets {}, entity assets {}, terrain fed {})",
                self.spec.settle_ms / 1000,
                renderer.settled(),
                renderer.pending_work(),
                renderer.world_drawn(),
                self.light_boot.map_may_draw(),
                self.terrain_boot.idle(),
                self.water_boot.idle(),
                self.terrain_assets.idle(),
                self.entity_assets.idle(),
                self.bridge.terrain().is_some(),
            ));
        }
        if matches!(self.stage, Stage::Finished) {
            return Progress::Failed("the frame was already given".into());
        }
        let has_assets = loader.is_some();
        self.light_boot.update(loader.as_deref_mut(), renderer);
        self.terrain_boot
            .update(&self.light_boot, loader.as_deref_mut(), renderer);
        self.water_boot
            .update(&self.light_boot, loader.as_deref_mut(), renderer);
        self.terrain_assets.update(
            loader.as_deref_mut(),
            renderer,
            self.light_boot.map_may_draw(),
        );
        self.entity_assets.update(loader.as_deref_mut(), renderer);
        self.icon_assets.update(loader, renderer);
        if !self.fed {
            // Same constants in every run: the time and the values of the capture.
            renderer.set_time_override(self.spec.time);
            renderer.set_constant_overrides(
                self.spec
                    .constants
                    .iter()
                    .cloned()
                    .collect::<HashMap<_, _>>(),
            );
            renderer.set_grid_visible(self.spec.grid);
            renderer.set_time(0.0);
            renderer.set_animation_time(0.0);
            self.bridge.set_catalog(&self.scene.catalog);
            let change = ViewChange {
                basis_plots: None,
                ..ViewChange::all()
            };
            self.bridge.sync(
                &self.scene.view,
                Some(&self.scene.basis),
                &change,
                &mut self.game_camera,
            );
            self.bridge.flush(renderer);
            self.fed = true;
        }

        // `P_ours = P_orig * ours_to_orig`; the map sets its `ours_to_orig` while its textures load.
        let to_orig = renderer.ours_to_orig();
        let p_ours = match self.spec.game_camera {
            // The game's camera where the view put it (the start of the player), at the level.
            Some(level) => {
                let mut c = self.game_camera;
                c.set_level(level);
                c.view_proj(self.spec.size.0 as f32 / self.spec.size.1.max(1) as f32)
            }
            None => Mat4::from_cols_array(&self.spec.view_proj_orig) * to_orig,
        };
        let Some(camera) = FixedCamera::from_view_proj(p_ours) else {
            return Progress::Failed(
                "the view-projection of the capture is not a perspective in our basis".into(),
            );
        };
        // The selection ring of the capture, on the cell under its centre, and the cells it reaches.
        if self.spec.ring.is_some() || !self.spec.contour.is_empty() {
            let from_orig = |x: f32, y: f32| {
                let ours = to_orig
                    .inverse()
                    .transform_point3(glam::Vec3::new(x, y, 0.0));
                render::hexgrid::nearest_cell(glam::Vec2::new(ours.x, ours.z))
            };
            let mut shown = render::entities::Highlights {
                selected: self.spec.ring.map(|[x, y, _, _]| from_orig(x, y)),
                reachable: reach_cells(&self.spec.contour, from_orig),
                ..Default::default()
            };
            // As the game does: through the styles of `Highlights.xml` when they are in.
            if renderer.has_highlight_styles() {
                renderer.set_highlight_set(crate::play::highlight_calls::from_highlights(
                    &shown, false,
                ));
                shown.reachable.clear();
            }
            renderer.set_highlights(shown);
            if let Some([_, _, angle, alpha]) = self.spec.ring {
                renderer.set_ring_pin(Some((angle, alpha)));
            }
        }
        let mut scene = Scene::test(0.0);
        scene.model_transform = Mat4::from_scale(glam::Vec3::ZERO);

        if matches!(self.stage, Stage::Warm) {
            let steady = self.is_quiet(renderer, has_assets) && self.last_frame == Some(to_orig);
            self.quiet = if steady { self.quiet + 1 } else { 0 };
            self.last_frame = Some(to_orig);
            if self.quiet >= QUIET_FRAMES {
                self.stage = Stage::Reading {
                    world: renderer.read_frame(FrameSource::World),
                    mask: renderer.read_frame(FrameSource::ClassMask),
                    world_image: None,
                    mask_image: None,
                };
            }
        }
        if let Err(e) = renderer.render(&scene, &camera, &self.ui, None) {
            return Progress::Failed(format!("the frame failed: {e}"));
        }
        let Stage::Reading {
            world,
            mask,
            world_image,
            mask_image,
        } = &mut self.stage
        else {
            return Progress::Running;
        };
        for (readback, image) in [(world, &mut *world_image), (mask, &mut *mask_image)] {
            if image.is_none() {
                match readback.poll() {
                    Ok(got) => *image = got,
                    Err(e) => return Progress::Failed(e.to_string()),
                }
            }
        }
        let (Some(w), Some(m)) = (world_image.take(), mask_image.take()) else {
            return Progress::Running;
        };
        if w.size != self.spec.size {
            return Progress::Failed(format!(
                "the frame is {}x{}, the capture {}x{}",
                w.size.0, w.size.1, self.spec.size.0, self.spec.size.1
            ));
        }
        self.stage = Stage::Finished;
        Progress::Done(GoldenOutput {
            capture: self.spec.capture.clone(),
            size: w.size,
            rgba: w.bytes,
            mask: m.bytes,
        })
    }
}

// ------------------------------------------------------------------ native, without a window

#[cfg(not(target_arch = "wasm32"))]
pub use native::{NativeRun, run_native, write_outputs};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;

    use platform::{HttpError, HttpResponse};

    use super::*;
    use crate::asset_boot::AssetsBoot;
    use crate::settings_store;

    /// What `app-desktop --golden <dir> --out <dir>` asks for.
    pub struct NativeRun {
        /// The folder of the scene: `mapbasis.wv3`, `world.wv3`, `catalog.wv3` and the `<capture>.golden` files.
        pub scene_dir: PathBuf,
        /// The assets of the game as the server would serve them (`manifest` and the blobs by hash). `None`:
        /// no assets, flat colours.
        pub assets_dir: Option<PathBuf>,
        /// Captures to draw; empty: every `*.golden` of the folder.
        pub captures: Vec<String>,
        pub backend: render::BackendPref,
    }

    /// The assets folder as an HTTP server: `http://golden.local/assets/<name>`.
    struct DirHttp {
        dir: Option<PathBuf>,
    }

    struct Ready(Option<Result<HttpResponse, HttpError>>);

    impl HttpOp for Ready {
        fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
            self.0.take()
        }
    }

    impl HttpClient for DirHttp {
        fn get(&self, url: &str) -> Box<dyn HttpOp> {
            let name = url
                .split_once("://")
                .map(|(_, rest)| rest)
                .and_then(|rest| rest.split_once('/'))
                .and_then(|(_, path)| path.strip_prefix("assets/"));
            let result = match (name, &self.dir) {
                (Some(name), Some(dir))
                    if !name.is_empty()
                        && name.bytes().all(|b| {
                            b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_'
                        })
                        && !name.starts_with('.') =>
                {
                    Ok(match std::fs::read(dir.join(name)) {
                        Ok(body) => HttpResponse { status: 200, body },
                        Err(_) => HttpResponse {
                            status: 404,
                            body: Vec::new(),
                        },
                    })
                }
                _ => Err(HttpError::InvalidUrl(url.to_owned())),
            };
            Box::new(Ready(Some(result)))
        }
    }

    fn captures_in(dir: &Path) -> Result<Vec<String>, String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .map_err(|e| format!("{}: {e}", dir.display()))?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".golden").map(str::to_owned)
            })
            .collect();
        names.sort();
        Ok(names)
    }

    /// Draws every capture of the scene folder without a window and returns the frames.
    pub fn run_native(
        run: &NativeRun,
        spawner: &crate::Spawner,
    ) -> Result<Vec<GoldenOutput>, String> {
        let captures = if run.captures.is_empty() {
            captures_in(&run.scene_dir)?
        } else {
            run.captures.clone()
        };
        if captures.is_empty() {
            return Err(format!("no *.golden file in {}", run.scene_dir.display()));
        }
        let mut frames = Vec::new();
        for capture in &captures {
            let files = GoldenFiles::read_dir(&run.scene_dir, capture)?;
            let spec =
                GoldenSpec::parse(&files.spec).map_err(|e| format!("{capture}.golden: {e}"))?;
            let scene = GoldenScene::load(&files)?;
            let resolved = settings_store::resolve(None, &spec.graphics_options());
            for w in &resolved.warnings {
                tracing::warn!("graphics settings: {w}");
            }
            // The samples of the main pass are fixed when the renderer is made.
            let slot = Rc::new(RefCell::new(None));
            let (slot2, size, pref, settings) =
                (Rc::clone(&slot), spec.size, run.backend, resolved.settings);
            spawner(Box::pin(async move {
                *slot2.borrow_mut() = Some(Renderer::new_headless_with(size, pref, settings).await);
            }));
            let mut renderer = slot
                .borrow_mut()
                .take()
                .ok_or("the renderer was not made")?
                .map_err(|e| format!("renderer: {e}"))?;
            renderer.set_layers(resolved.layers);
            let http: Rc<dyn HttpClient> = Rc::new(DirHttp {
                dir: run.assets_dir.clone(),
            });
            let mut assets = run
                .assets_dir
                .as_ref()
                .and_then(|_| AssetsBoot::start_at(http, "http://golden.local/assets/", None));
            let mut driver = GoldenDriver::new(spec, scene);
            let clock = std::time::Instant::now();
            loop {
                if let Some(a) = assets.as_mut() {
                    a.update();
                }
                let now = clock.elapsed().as_millis() as u64;
                match driver.frame(
                    &mut renderer,
                    assets.as_mut().map(AssetsBoot::loader_mut),
                    now,
                ) {
                    Progress::Running => {}
                    Progress::Done(out) => {
                        frames.push(out);
                        break;
                    }
                    Progress::Failed(e) => return Err(format!("{capture}: {e}")),
                }
            }
        }
        Ok(frames)
    }

    /// `<capture>-<tag>.rgba`, `<capture>-<tag>.mask.r8` and `<capture>-<tag>.txt` (`width height`) in `dir`.
    pub fn write_outputs(dir: &Path, tag: &str, out: &GoldenOutput) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let base = dir.join(format!("{}-{tag}", out.capture));
        let write = |ext: &str, bytes: &[u8]| {
            let path = base.with_extension(ext);
            std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
        };
        write("rgba", &out.rgba)?;
        write("mask.r8", &out.mask)?;
        write("txt", format!("{} {}\n", out.size.0, out.size.1).as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "civ5-golden/1\ncapture S1a\nsize 1914 1051\npreset high\nmsaa 4\ndisable YieldIcons,FOW\ngrid 0\ntime 12.5\nview_proj_orig 1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1\nconst g_ShadowFade 1\nconst g_FogConsts -0.05 0 0 0.34\n";

    #[test]
    fn a_spec_is_read() {
        let s = GoldenSpec::parse(SPEC).unwrap();
        assert_eq!(s.capture, "S1a");
        assert_eq!(s.size, (1914, 1051));
        assert_eq!(s.disable, ["YieldIcons", "FOW"]);
        assert_eq!(s.time, Some(12.5));
        assert_eq!(s.constants.len(), 2);
        assert_eq!(s.constants[1].1, [-0.05, 0.0, 0.0, 0.34]);
        let g = s.graphics_options();
        assert_eq!(g.overrides.as_deref(), Some("MSAASamples:4"));
        assert_eq!(g.disable.as_deref(), Some("YieldIcons,FOW"));
    }

    /// The far-view scene: settings on top of the preset, and the game's camera in place of the
    /// capture's.
    #[test]
    fn gfx_and_the_game_camera_are_read() {
        let s = GoldenSpec::parse(&format!("{SPEC}gfx FarLOD:0\n")).unwrap();
        assert_eq!(
            s.graphics_options().overrides.as_deref(),
            Some("MSAASamples:4,FarLOD:0")
        );
        assert_eq!(s.game_camera, None);
        let no_matrix: String = SPEC
            .lines()
            .filter(|l| !l.starts_with("view_proj_orig"))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(GoldenSpec::parse(&no_matrix).is_err());
        let far = GoldenSpec::parse(&format!("{no_matrix}game_camera 20.5\n")).unwrap();
        assert_eq!(far.game_camera, Some(20.5));
        assert!(GoldenSpec::parse(&format!("{no_matrix}game_camera far\n")).is_err());
    }

    #[test]
    fn reveal_all_is_read() {
        assert!(!GoldenSpec::parse(SPEC).unwrap().reveal_all);
        assert!(
            GoldenSpec::parse(&format!("{SPEC}reveal all\n"))
                .unwrap()
                .reveal_all
        );
        assert!(GoldenSpec::parse(&format!("{SPEC}reveal some\n")).is_err());
        assert_eq!(GoldenSpec::parse(SPEC).unwrap().settle_ms, SETTLE_LIMIT_MS);
        assert_eq!(
            GoldenSpec::parse(&format!("{SPEC}settle 600\n"))
                .unwrap()
                .settle_ms,
            600_000
        );
    }

    #[test]
    fn the_contour_of_the_reachable_cells_is_read() {
        let s = GoldenSpec::parse(&format!("{SPEC}contour 1 2 3.5 -4\n")).unwrap();
        assert_eq!(s.contour, [[1.0, 2.0], [3.5, -4.0]]);
        assert!(GoldenSpec::parse(&format!("{SPEC}contour 1 2 3\n")).is_err());
        assert!(GoldenSpec::parse(SPEC).unwrap().contour.is_empty());
    }

    /// The strip lies in the cells of the edge of the set; the cells inside are reached too.
    #[test]
    fn the_cells_inside_the_contour_are_reachable() {
        use render::hexgrid::{HEX_WIDTH, cell_center, nearest_cell};
        let centre = nearest_cell(glam::Vec2::new(500.0, 400.0));
        // The ring of the six neighbours of a cell: the edge; the cell itself is inside.
        let at = cell_center(centre.0, centre.1);
        let edge: Vec<[f32; 2]> = (0..6)
            .map(|k| {
                let a = std::f32::consts::FRAC_PI_3 * k as f32;
                let p = at + glam::Vec2::new(a.cos(), a.sin()) * HEX_WIDTH;
                [p.x, p.y]
            })
            .collect();
        let from = |x: f32, y: f32| nearest_cell(glam::Vec2::new(x, y));
        let cells = reach_cells(&edge, from);
        assert!(cells.contains(&centre));
        assert_eq!(cells.len(), 7);
        assert!(reach_cells(&[], from).is_empty());
    }

    #[test]
    fn a_bad_spec_is_refused() {
        assert!(GoldenSpec::parse("").is_err());
        assert!(GoldenSpec::parse("civ5-golden/2\n").is_err());
        assert!(GoldenSpec::parse(&SPEC.replace("size 1914 1051", "size 0 0")).is_err());
        assert!(GoldenSpec::parse(&SPEC.replace("capture S1a\n", "")).is_err());
        assert!(GoldenSpec::parse(&SPEC.replace("grid 0", "bogus 1")).is_err());
        assert!(
            GoldenSpec::parse(&SPEC.replace("view_proj_orig 1 0", "view_proj_orig 1")).is_err()
        );
    }

    /// The map basis of the fixture world: its relief, no rivers, no features.
    pub(crate) fn fixture_basis(world: &WorldView) -> MapBasis {
        let plots = world
            .map
            .plots
            .iter()
            .map(|p| protocol::PlotBasisView {
                plot_type: p.plot_type,
                terrain: p.terrain,
                continent_art: p.continent_art,
                feature: -1,
                river_flow: [-1; 3],
                ..protocol::PlotBasisView::default()
            })
            .collect();
        let mut basis = MapBasis {
            game: 1,
            basis_hash: 0,
            width: world.map.width,
            height: world.map.height,
            wrap_x: world.map.wrap_x,
            wrap_y: false,
            terrain_seed: 77,
            plots,
        };
        basis.basis_hash = protocol::basis_hash_of(&basis);
        basis
    }

    /// Two frames of one scene are the same bytes, and the frame is not empty. Skipped
    /// where there is no GPU adapter.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn two_native_frames_of_a_scene_are_byte_identical() {
        let (world, catalog) = crate::test_world::world_and_catalog();
        let basis = fixture_basis(&world);
        let dir = std::env::temp_dir().join(format!("civ5-golden-repeat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = BasisState::new(basis.clone()).unwrap();
        let key = protocol::Keyframe {
            view: world,
            hash: 1,
            basis_hash: state.hash(),
        };
        std::fs::write(dir.join("mapbasis.wv3"), wv3::encode_basis(&basis).unwrap()).unwrap();
        std::fs::write(dir.join("world.wv3"), wv3::encode_world(&key).unwrap()).unwrap();
        std::fs::write(
            dir.join("catalog.wv3"),
            wv3::encode_catalog(&catalog).unwrap(),
        )
        .unwrap();
        // The camera of the game over the map, written in the basis of the original.
        let ours = GameCamera::new(glam::Vec2::new(10.0, 10.0)).view_proj(16.0 / 9.0);
        let orig = ours * render::frame::basis_rotation().inverse();
        let cols: Vec<String> = orig.to_cols_array().iter().map(f32::to_string).collect();
        let spec = format!(
            "{SPEC_HEADER}\ncapture T\nsize 320 180\npreset low\nmsaa 4\ndisable YieldIcons\nview_proj_orig {}\n",
            cols.join(" ")
        );
        std::fs::write(dir.join("T.golden"), spec).unwrap();
        let run = NativeRun {
            scene_dir: dir.clone(),
            assets_dir: None,
            captures: vec![],
            backend: render::BackendPref::Auto,
        };
        let spawner: crate::Spawner = Box::new(|f| {
            // The renderer's futures are ready at once.
            let waker = std::task::Waker::noop();
            let mut cx = std::task::Context::from_waker(waker);
            let mut f = f;
            while f.as_mut().poll(&mut cx).is_pending() {
                std::thread::yield_now();
            }
        });
        let first = match run_native(&run, &spawner) {
            Ok(f) => f,
            Err(e) if e.contains("renderer:") => {
                eprintln!("no GPU adapter, repeatability test skipped: {e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        let second = run_native(&run, &spawner).unwrap();
        assert_eq!(first, second, "two frames of the same scene differ");
        assert_eq!(first[0].size, (320, 180));
        assert!(first[0].rgba.iter().any(|&b| b != 0), "the frame is empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_basis_must_belong_to_the_keyframe() {
        let (world, _) = crate::test_world::world_and_catalog();
        let basis = fixture_basis(&world);
        let state = BasisState::new(basis.clone()).unwrap();
        let ok = protocol::Keyframe {
            view: world,
            hash: 1,
            basis_hash: state.hash(),
        };
        let files = |k: &protocol::Keyframe| GoldenFiles {
            basis: wv3::encode_basis(&basis).unwrap(),
            world: wv3::encode_world(k).unwrap(),
            catalog: None,
            spec: SPEC.into(),
        };
        let scene = GoldenScene::load(&files(&ok)).unwrap();
        assert_eq!(scene.basis, basis);
        assert!(
            scene.catalog.terrains.is_empty(),
            "no catalog file, no names"
        );
        let bad = protocol::Keyframe {
            basis_hash: ok.basis_hash ^ 1,
            ..ok
        };
        assert!(GoldenScene::load(&files(&bad)).is_err());
    }
}
