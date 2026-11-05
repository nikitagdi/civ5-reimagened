//! Captures of the original game: parsing with `rdc/extract.py` and `check-captures`
//!
//! Layout: the user's captures are `<cache>/captures/<scene>/<name>.rdc`; the parse of one capture is
//! `<cache>/golden/<scene>/<name>/` (`meta.json`, `events.json`, `draws.json`, `textures/`, `buffers/`,
//! `targets/`, `world.dds`, `final.dds`). All of it is derived from the user's game: never in git.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::scene::{SCENE_IDS, Scene};

/// Schema tag of `draws.json`; `shader-re` (`dxbc/twins.rs`) reads exactly this.
pub const DRAWS_SCHEMA: &str = "civ5-golden-draws/1";

/// Frame the captures must have.
pub const EXPECTED_SIZE: [u32; 2] = [1920, 1080];
pub const EXPECTED_MSAA: u32 = 4;

/// Packages of the checks of `check-captures`. A draw belongs to a package by the
/// catalog entry of its shader (`Package::Entry:Stage`), so these checks need the shader index.
const FOW_PACKAGES: [&str; 1] = ["FOW"];
const S4_PACKAGES: [&str; 2] = ["OverlayShaders", "GridShader"];

/// The cache folder: `CIV5_CACHE`, else `cache` next to the repository (`tools/golden` is two levels below its root).
pub fn default_cache() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("CIV5_CACHE").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    match Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3) {
        Some(parent) => Ok(parent.join("cache")),
        None => bail!(
            "cannot place the cache folder next to the repository; set CIV5_CACHE or pass --cache"
        ),
    }
}

pub fn parsed_dir(cache: &Path, scene: &str, capture: &str) -> PathBuf {
    cache.join("golden").join(scene).join(capture)
}

// ------------------------------------------------------------ how to run RenderDoc

/// `cache/golden/renderdoc.json`: which way of reading `.rdc` works on this machine.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct RenderdocConfig {
    /// `python` (a Python that can import `renderdoc.pyd`) or `qrenderdoc` (`qrenderdoc.exe --python`).
    pub mode: Option<String>,
    /// Python executable for `python` mode (and for synthetic captures); default `python`.
    pub python: Option<String>,
    /// Folder of the RenderDoc install (has `renderdoc.pyd`, `qrenderdoc.exe`).
    pub renderdoc_path: Option<String>,
}

impl RenderdocConfig {
    pub fn load(cache: &Path) -> Result<Self> {
        let p = cache.join("golden").join("renderdoc.json");
        if !p.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&p)
            .with_context(|| format!("failed to read {}", p.display()))?;
        serde_json::from_str(&text).with_context(|| p.display().to_string())
    }

    fn python(&self) -> &str {
        self.python.as_deref().unwrap_or("python")
    }
}

/// sha1 of a DX11 shader block -> catalog entry (`UnitShader::UnitPS:PS`), from the `twins.json` that
/// `shader-re dx11 twins` writes under `<cache>/shader-re/`. Empty when there is none yet.
pub fn shader_index(cache: &Path) -> BTreeMap<String, String> {
    let mut files = Vec::new();
    let root = cache.join("shader-re");
    files.push(root.join("twins.json"));
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            files.push(e.path().join("twins.json"));
            files.push(e.path().join("dx11").join("twins.json"));
        }
    }
    let mut out = BTreeMap::new();
    for f in files.iter().filter(|f| f.is_file()) {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        for t in v["twins"].as_array().into_iter().flatten() {
            let Some(entry) = t["entry"].as_str() else {
                continue;
            };
            for sha in t["blocks"].as_object().into_iter().flat_map(|m| m.values()) {
                if let Some(s) = sha.as_str() {
                    out.insert(s.to_ascii_lowercase(), entry.to_string());
                }
            }
        }
    }
    out
}

// ------------------------------------------------------------ parsing

#[derive(Debug, Clone, Default)]
pub struct ParseOpts {
    /// `none`, `world-top3` or `12,40` (`--targets`): calls whose targets are saved before and after.
    pub targets: Option<String>,
}

/// What a capture is read from.
pub enum Source<'a> {
    Rdc(&'a Path),
    /// A capture-equivalent JSON (tests and development without RenderDoc).
    Model(&'a Path),
}

fn extract_py() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("rdc")
        .join("extract.py")
}

/// Parses one capture into `<cache>/golden/<scene>/<capture>/`. Returns the `meta.json` it wrote.
pub fn parse_capture(
    cfg: &RenderdocConfig,
    cache: &Path,
    scene: &str,
    capture: &str,
    source: Source<'_>,
    opts: &ParseOpts,
) -> Result<Meta> {
    let out = parsed_dir(cache, scene, capture);
    if out.exists() {
        std::fs::remove_dir_all(&out)
            .with_context(|| format!("failed to clear {}", out.display()))?;
    }
    std::fs::create_dir_all(&out)?;

    // The shader index goes to a file: the Python side needs it for the UI boundary and `entry` fields.
    let index = shader_index(cache);
    let index_path = out.join("shader-index.json");
    if !index.is_empty() {
        std::fs::write(&index_path, serde_json::to_string(&index)?)?;
    }
    let mut job = serde_json::json!({
        "out": out.to_string_lossy(),
        "scene": scene,
        "capture": capture,
        "targets": opts.targets.as_deref().unwrap_or("none"),
    });
    if !index.is_empty() {
        job["index"] = index_path.to_string_lossy().into();
    }
    if let Some(p) = &cfg.renderdoc_path {
        job["renderdoc_path"] = p.clone().into();
    }
    match source {
        Source::Model(m) => job["model"] = m.to_string_lossy().into(),
        Source::Rdc(r) => job["rdc"] = r.to_string_lossy().into(),
    }

    if cfg.mode.as_deref() == Some("qrenderdoc") && matches!(source, Source::Rdc(_)) {
        run_qrenderdoc(cfg, &out, &mut job)?;
    } else {
        let job_path = out.join("job.json");
        std::fs::write(&job_path, serde_json::to_string_pretty(&job)?)?;
        let output = Command::new(cfg.python())
            .arg("-I")
            .arg(extract_py())
            .arg("--out")
            .arg(&out)
            .arg("--scene")
            .arg(scene)
            .arg("--capture")
            .arg(capture)
            .arg("--job")
            .arg(&job_path)
            .output()
            .with_context(|| {
                format!(
                    "failed to start {} (set `python` in cache/golden/renderdoc.json)",
                    cfg.python()
                )
            })?;
        if !output.status.success() {
            bail!(
                "extract.py failed for {scene}/{capture}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    let _ = std::fs::remove_file(&index_path);
    let _ = std::fs::remove_file(out.join("job.json"));
    Meta::load(&out)
}

/// `qrenderdoc.exe FILE --python extract.py`; the script writes a done file, then the app is closed.
fn run_qrenderdoc(cfg: &RenderdocConfig, out: &Path, job: &mut serde_json::Value) -> Result<()> {
    let dir = cfg
        .renderdoc_path
        .as_deref()
        .context("`renderdoc_path` is needed in cache/golden/renderdoc.json for mode qrenderdoc")?;
    let done = out.join("qrenderdoc.done");
    job["done_file"] = done.to_string_lossy().into();
    let job_path = out.join("job.json");
    std::fs::write(&job_path, serde_json::to_string_pretty(job)?)?;
    // No capture argument: the script opens the capture itself (a script that waited for the UI to load
    // it would block the UI thread for ever).
    let mut child = Command::new(Path::new(dir).join("qrenderdoc.exe"))
        .arg("--python")
        .arg(extract_py())
        .env("CIV5_GOLDEN_JOB", &job_path)
        .env(
            "CIV5_GOLDEN_RDC_DIR",
            extract_py().parent().unwrap_or(Path::new(".")),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start qrenderdoc.exe")?;
    let t0 = Instant::now();
    while !done.is_file() {
        if t0.elapsed() > Duration::from_secs(900) {
            let _ = child.kill();
            bail!("qrenderdoc did not finish in 15 minutes");
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let report = std::fs::read_to_string(&done).unwrap_or_default();
    let _ = child.kill();
    let _ = child.wait();
    let (code, msg) = report.split_once('\n').unwrap_or((report.as_str(), ""));
    if code.trim() != "0" {
        bail!(
            "qrenderdoc script failed (code {}): {}",
            code.trim(),
            msg.trim()
        );
    }
    Ok(())
}

// ------------------------------------------------------------ parsed files

/// `meta.json` of a parsed capture.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Meta {
    pub scene: String,
    pub capture: String,
    pub renderdoc: Option<String>,
    pub api: Option<String>,
    pub gpu: Option<String>,
    pub driver: Option<String>,
    pub width: u32,
    pub height: u32,
    pub swapchain_format: Option<String>,
    pub msaa: Option<u32>,
    pub event_count: usize,
    pub draw_count: usize,
    pub world_draw_count: usize,
    pub pass_count: usize,
    pub has_markers: bool,
    pub ui_boundary: Option<UiBoundary>,
    pub shader_index_used: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UiBoundary {
    pub event: u32,
    /// `shader` or `marker`.
    pub reason: String,
    pub detail: String,
}

impl Meta {
    pub fn load(dir: &Path) -> Result<Self> {
        let p = dir.join("meta.json");
        let text = std::fs::read_to_string(&p)
            .with_context(|| format!("failed to read {}", p.display()))?;
        serde_json::from_str(&text).with_context(|| p.display().to_string())
    }
}

/// `draws.json`: the draw calls of a capture (the schema `shader-re` reads).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DrawsFile {
    pub schema: String,
    pub scene: String,
    pub capture: String,
    pub width: u32,
    pub height: u32,
    pub ui_boundary_event: Option<u32>,
    pub draws: Vec<Draw>,
    /// Catalog entry -> events (empty without a shader index).
    pub by_entry: BTreeMap<String, Vec<u32>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Draw {
    pub event: u32,
    pub pass: u32,
    /// Before the UI boundary.
    pub world: bool,
    /// sha1 (40 hex) of the DXBC container of each stage; `null` when the stage is off.
    pub vs: Option<String>,
    pub ps: Option<String>,
    pub hs: Option<String>,
    pub ds: Option<String>,
    pub vs_entry: Option<String>,
    pub ps_entry: Option<String>,
    pub hs_entry: Option<String>,
    pub ds_entry: Option<String>,
    pub topology: Option<String>,
    pub index_count: u64,
    pub vertex_count: u64,
    pub instances: u64,
    pub targets: Vec<DrawTarget>,
    pub depth_format: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DrawTarget {
    pub format: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub samples: Option<u32>,
}

impl DrawsFile {
    pub fn load(dir: &Path) -> Result<Self> {
        let p = dir.join("draws.json");
        let text = std::fs::read_to_string(&p)
            .with_context(|| format!("failed to read {}", p.display()))?;
        let d: DrawsFile = serde_json::from_str(&text).with_context(|| p.display().to_string())?;
        if d.schema != DRAWS_SCHEMA {
            bail!(
                "{}: schema {:?}, expected {DRAWS_SCHEMA}",
                p.display(),
                d.schema
            );
        }
        Ok(d)
    }

    fn has_package(&self, packages: &[&str], world_only: bool) -> bool {
        self.draws
            .iter()
            .filter(|d| !world_only || d.world)
            .any(|d| {
                [&d.vs_entry, &d.ps_entry, &d.hs_entry, &d.ds_entry]
                    .into_iter()
                    .flatten()
                    .any(|e| packages.contains(&e.split("::").next().unwrap_or("")))
            })
    }
}

// ------------------------------------------------------------ check-captures

/// What `check-captures` knows about one capture.
pub struct CaptureFacts {
    pub meta: Meta,
    pub draws: DrawsFile,
}

impl CaptureFacts {
    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self {
            meta: Meta::load(dir)?,
            draws: DrawsFile::load(dir)?,
        })
    }
}

/// Problems of one capture (empty = fine), as sentences that name the cause. `notes` get what could
/// not be checked.
pub fn check_facts(capture: &str, f: &CaptureFacts, notes: &mut Vec<String>) -> Vec<String> {
    let mut p = Vec::new();
    let m = &f.meta;
    if [m.width, m.height] != EXPECTED_SIZE {
        // Not a problem: the real captures are of a window (1914x1051, S3 1914x1171), and they cannot
        // be retaken. The renderer and the comparison run at the captured size.
        notes.push(format!(
            "{capture}: frame is {}x{}, not {}x{} (the game window was not set by prep.ps1); \
             the golden comparison uses the captured size",
            m.width, m.height, EXPECTED_SIZE[0], EXPECTED_SIZE[1]
        ));
    }
    if m.msaa != Some(EXPECTED_MSAA) {
        p.push(format!(
            "MSAA is {:?}, must be {EXPECTED_MSAA} (prep.ps1 sets MSAASamples = 4)",
            m.msaa
        ));
    }
    if !m
        .api
        .as_deref()
        .is_some_and(|a| a.to_ascii_uppercase().contains("D3D11"))
    {
        p.push(format!(
            "API is {:?}, must be D3D11 (start CivilizationV_DX11.exe)",
            m.api
        ));
    }
    if m.ui_boundary.is_none() && capture == "S3" {
        // S3 is the stress frame taken with the UI hidden: every draw is a world draw.
        notes.push("S3: no UI boundary (the frame has no UI): every draw is a world draw".into());
    } else if m.ui_boundary.is_none() {
        p.push("no UI boundary found (no UI shader draw and no UI marker): the frame has no UI or the catalog does not know the UI shaders".into());
    }
    if m.draw_count == 0 {
        p.push("no draw calls in the capture".into());
    }
    let indexed = m.shader_index_used;
    let scene = capture.split('-').next().unwrap_or(capture);
    let is_s2 = capture == "S2" || capture == "S2-bake";
    if is_s2 {
        if indexed {
            if f.draws.has_package(&FOW_PACKAGES, false) {
                // Not a problem: the real S2 shows the fog (the game draws FOW::Visible and
                // FOW::BasePS although [DisableSystems] FOW = 1); the `fog` mask must not be
                // compared for S2.
                notes.push(format!(
                    "{capture}: has FOW draws (fog of war was not off in the game); S2 comparisons must leave the fog out"
                ));
            }
        } else {
            notes.push(format!(
                "{capture}: FOW check skipped (no shader index: run `shader-re dx11 twins`)"
            ));
        }
    }
    if capture == "S2-bake" {
        // The bake draws into cache pages: a target that is not the frame.
        let offscreen = f.draws.draws.iter().any(|d| {
            d.targets.first().is_some_and(|t| {
                t.width.is_some_and(|w| w != m.width) || t.height.is_some_and(|h| h != m.height)
            })
        });
        if !offscreen {
            p.push("S2-bake has no draw into an off-screen target (the terrain cache bake): F12 was pressed too late, retake S2".into());
        }
    }
    if scene == "S4" {
        if indexed {
            if !f.draws.has_package(&S4_PACKAGES, true) {
                p.push("S4 has no highlight or grid draws (OverlayShaders, GridShader): the unit was not selected at F12".into());
            }
        } else {
            notes.push(format!(
                "{capture}: highlight check skipped (no shader index: run `shader-re dx11 twins`)"
            ));
        }
    }
    p
}

#[derive(Debug, Default)]
pub struct CheckReport {
    /// (capture, problem), in scene order.
    pub problems: Vec<(String, String)>,
    pub ok: Vec<String>,
    pub notes: Vec<String>,
}

impl CheckReport {
    pub fn passed(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Checks every capture of every scene: file present, parsed, and the capture rules.
pub fn check_captures(cache: &Path, scenes_dir: &Path) -> Result<CheckReport> {
    let mut r = CheckReport::default();
    for id in SCENE_IDS {
        let scene = Scene::load_by_id(scenes_dir, id)?;
        let required = scene.required_captures();
        for name in &scene.scene.captures {
            let rdc = scene.capture_path(cache, name);
            let is_required = required.contains(&name.as_str());
            if !rdc.is_file() {
                if is_required {
                    r.problems.push((
                        name.clone(),
                        format!("missing {}: take scene {id} again (step for {name} in the capture howto)", rdc.display()),
                    ));
                } else if name == "S1b" {
                    r.notes.push(
                        "S1b: optional, not taken; the variance step (`golden noise`, S1a against S1b) \
                         is skipped, thresholds.toml keeps its defaults"
                            .into(),
                    );
                } else {
                    r.notes.push(format!("{name}: optional, not taken"));
                }
                continue;
            }
            let dir = parsed_dir(cache, id, name);
            if !dir.join("meta.json").is_file() {
                r.problems.push((
                    name.clone(),
                    format!("not parsed yet: run `cargo xtask golden parse {name}`"),
                ));
                continue;
            }
            match CaptureFacts::load(&dir) {
                Err(e) => r.problems.push((
                    name.clone(),
                    format!("parse output is unreadable ({e:#}): run `golden parse {name}` again"),
                )),
                Ok(f) => {
                    let probs = check_facts(name, &f, &mut r.notes);
                    if probs.is_empty() {
                        r.ok.push(name.clone());
                    }
                    for p in probs {
                        r.problems.push((
                            name.clone(),
                            format!("{p}; retake scene {id} ({name} only)"),
                        ));
                    }
                }
            }
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(
        capture: &str,
        w: u32,
        msaa: u32,
        entries: &[&str],
        ui: bool,
        indexed: bool,
    ) -> CaptureFacts {
        let draw = |e: &str, tw: u32| Draw {
            event: 1,
            pass: 0,
            world: true,
            vs: Some("a".repeat(40)),
            ps: Some("b".repeat(40)),
            hs: None,
            ds: None,
            vs_entry: None,
            ps_entry: Some(e.to_string()),
            hs_entry: None,
            ds_entry: None,
            topology: None,
            index_count: 3,
            vertex_count: 0,
            instances: 1,
            targets: vec![DrawTarget {
                format: None,
                width: Some(tw),
                height: Some(1080),
                samples: None,
            }],
            depth_format: None,
        };
        let mut draws: Vec<Draw> = entries.iter().map(|e| draw(e, w)).collect();
        if capture == "S2-bake" {
            draws.push(draw("X::Y:PS", 512));
        }
        CaptureFacts {
            meta: Meta {
                scene: "S".into(),
                capture: capture.into(),
                renderdoc: None,
                api: Some("D3D11".into()),
                gpu: None,
                driver: None,
                width: w,
                height: 1080,
                swapchain_format: None,
                msaa: Some(msaa),
                event_count: 1,
                draw_count: draws.len(),
                world_draw_count: draws.len(),
                pass_count: 1,
                has_markers: false,
                ui_boundary: ui.then(|| UiBoundary {
                    event: 9,
                    reason: "shader".into(),
                    detail: "UIShader::X:PS".into(),
                }),
                shader_index_used: indexed,
            },
            draws: DrawsFile {
                schema: DRAWS_SCHEMA.into(),
                scene: "S".into(),
                capture: capture.into(),
                width: w,
                height: 1080,
                ui_boundary_event: ui.then_some(9),
                draws,
                by_entry: BTreeMap::new(),
            },
        }
    }

    #[test]
    fn good_s1_passes() {
        let f = facts("S1a", 1920, 4, &["TerrainShader::P:PS"], true, true);
        assert!(check_facts("S1a", &f, &mut Vec::new()).is_empty());
    }

    #[test]
    fn size_msaa_and_ui_are_named() {
        let f = facts("S1a", 2560, 8, &[], false, true);
        let mut notes = Vec::new();
        let p = check_facts("S1a", &f, &mut notes);
        // The size is a note, not a problem: the real captures are of a window and cannot be retaken.
        assert!(notes.iter().any(|s| s.contains("2560x1080")), "{notes:?}");
        assert!(p.iter().any(|s| s.contains("MSAA")), "{p:?}");
        assert!(p.iter().any(|s| s.contains("UI boundary")), "{p:?}");
        assert!(p.iter().any(|s| s.contains("no draw calls")), "{p:?}");
    }

    #[test]
    fn s2_with_fog_is_noted_only_with_the_index() {
        let f = facts("S2", 1920, 4, &["FOW::Mask:PS"], true, true);
        let mut notes = Vec::new();
        assert!(check_facts("S2", &f, &mut notes).is_empty());
        assert!(notes.iter().any(|s| s.contains("FOW draws")), "{notes:?}");
        let f = facts("S2", 1920, 4, &["FOW::Mask:PS"], true, false);
        let mut notes = Vec::new();
        assert!(check_facts("S2", &f, &mut notes).is_empty());
        assert!(notes[0].contains("skipped"));
    }

    #[test]
    fn s2_bake_needs_an_offscreen_draw() {
        let f = facts("S2-bake", 1920, 4, &["TerrainShader::P:PS"], true, true);
        assert!(check_facts("S2-bake", &f, &mut Vec::new()).is_empty());
        let mut f = facts("S2-bake", 1920, 4, &["TerrainShader::P:PS"], true, true);
        f.draws.draws.pop();
        let p = check_facts("S2-bake", &f, &mut Vec::new());
        assert!(p.iter().any(|s| s.contains("off-screen")), "{p:?}");
    }

    #[test]
    fn s4_needs_highlights() {
        let f = facts(
            "S4-select",
            1920,
            4,
            &["OverlayShaders::Ring:PS"],
            true,
            true,
        );
        assert!(check_facts("S4-select", &f, &mut Vec::new()).is_empty());
        let f = facts("S4-select", 1920, 4, &["TerrainShader::P:PS"], true, true);
        let p = check_facts("S4-select", &f, &mut Vec::new());
        assert!(p.iter().any(|s| s.contains("highlight")), "{p:?}");
    }
}
