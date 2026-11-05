//! `golden render <scene> [--backend native|webgpu|webgl2]`: our frame of every parsed capture
//! of a scene. The tool writes `<capture>.golden` next to the scene's `.wv3` files (the camera of the capture in
//! the basis of the original, its time constants, the preset and the switches of the scene), runs the frame
//! (`app-desktop --golden` natively, the `golden` Playwright scenario for the browser) and turns the raw frames
//! into `<capture>-<backend>.png`; the native frame is also `<capture>.dds`, the file `golden report` reads.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use ddsfile::{AlphaMode, D3D10ResourceDimension, Dds, DxgiFormat, NewDxgiParams};
use glam::DMat4;

use crate::camera::{self, CbMapFile, EventsFile, OursToOrig, cbmap_file};
use crate::captures::{DrawsFile, parsed_dir};
use crate::image::{Rgba8, read_raw_rgba, write_png};
use crate::scene::Scene;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Native,
    WebGpu,
    WebGl2,
}

impl Backend {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "native" => Ok(Self::Native),
            "webgpu" => Ok(Self::WebGpu),
            "webgl2" => Ok(Self::WebGl2),
            other => bail!("render: unknown backend {other:?} (native|webgpu|webgl2)"),
        }
    }

    /// The word in the names of the files.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::WebGpu => "webgpu",
            Self::WebGl2 => "webgl2",
        }
    }
}

pub struct Options {
    pub cache: PathBuf,
    pub scenes_dir: PathBuf,
    /// Frames go here (`target/golden/render`).
    pub out: PathBuf,
    /// Root of the workspace (cargo runs from here).
    pub root: PathBuf,
    pub scene: String,
    pub backend: Backend,
    /// Captures to draw; empty: every parsed capture of the scene.
    pub captures: Vec<String>,
    /// The assets as the server serves them.
    pub assets: PathBuf,
    /// For the browser backends: the real gamecore for the web bundle (`cargo xtask playtest` needs it).
    pub gamecore_lib: Option<PathBuf>,
    pub cbmap_dirs: Vec<PathBuf>,
}

/// `*` matches any run of characters; the rest is literal.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first)
        || !name[first.len()..].ends_with(last)
        || name.len() < first.len() + last.len()
    {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    for mid in &parts[1..parts.len() - 1] {
        match rest.find(mid) {
            Some(at) => rest = &rest[at + mid.len()..],
            None => return false,
        }
    }
    true
}

/// Names of the constants that are the time of the shaders, for the `time` line.
const TIME_NAMES: [&str; 4] = ["g_Time", "Time", "g_fTime", "fTime"];

/// Constants of the world draws of a capture whose names match `patterns`, read from the constant buffers of the
/// first draw that has each (the values come from the capture through the constant maps).
pub fn time_constants(
    cache: &Path,
    scene: &str,
    capture: &str,
    patterns: &[String],
    cbmap_dirs: &[PathBuf],
) -> Result<Vec<(String, Vec<f32>)>> {
    let dir = parsed_dir(cache, scene, capture);
    let draws = DrawsFile::load(&dir)?;
    let text = std::fs::read_to_string(dir.join("events.json"))
        .with_context(|| format!("failed to read events.json of {capture}"))?;
    let events: EventsFile = serde_json::from_str(&text).context("events.json")?;
    let mut found: Vec<(String, Vec<f32>)> = Vec::new();
    for d in draws.draws.iter().filter(|d| d.world) {
        let Some(call) = events.calls.iter().find(|c| c.eid == d.event) else {
            continue;
        };
        let stages = [
            ("vs", &d.vs_entry),
            ("ps", &d.ps_entry),
            ("hs", &d.hs_entry),
            ("ds", &d.ds_entry),
        ];
        for (stage, entry) in stages {
            let Some(entry) = entry else { continue };
            for dir in cbmap_dirs {
                let Ok(text) = std::fs::read_to_string(cbmap_file(dir, entry)) else {
                    continue;
                };
                let Ok(map) = serde_json::from_str::<CbMapFile>(&text) else {
                    continue;
                };
                for c in &map.consts {
                    if found.iter().any(|(n, _)| *n == c.name)
                        || !patterns.iter().any(|p| glob_match(p, &c.name))
                    {
                        continue;
                    }
                    let Some(b) = call
                        .cbuffers
                        .get(stage)
                        .and_then(|v| v.iter().find(|b| b.slot == c.cb))
                    else {
                        continue;
                    };
                    let Some(start) = c.offset.checked_sub(b.offset).map(|o| (o / 4) as usize)
                    else {
                        continue;
                    };
                    let n = c.rows as usize * 4;
                    if let Some(v) = b.floats.get(start..start + n) {
                        let v = v
                            .iter()
                            .map(|x| if x.is_finite() { *x } else { 0.0 })
                            .collect();
                        found.push((c.name.clone(), v));
                    }
                }
                break;
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// The selection ring of a capture, from its `OverlayShaders::PSValidMoves` draw: the centre in the
/// original's basis (the translation of `Local`, vertex constants 12 and 13), the turn (the first row
/// of `TransScaleRot` is `(cos, -sin)`, constants 16 and 17) and `fAlpha` (pixel constant 52, after
/// `DrawColor`). `None` when the capture draws no ring.
pub fn selection_ring(cache: &Path, scene: &str, capture: &str) -> Result<Option<[f32; 4]>> {
    let dir = parsed_dir(cache, scene, capture);
    let draws = DrawsFile::load(&dir)?;
    let Some(draw) = draws.draws.iter().find(|d| {
        d.ps_entry
            .as_deref()
            .is_some_and(|e| e.starts_with("OverlayShaders::PSValidMoves"))
    }) else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(dir.join("events.json"))
        .with_context(|| format!("failed to read events.json of {capture}"))?;
    let events: EventsFile = serde_json::from_str(&text).context("events.json")?;
    let Some(call) = events.calls.iter().find(|c| c.eid == draw.event) else {
        return Ok(None);
    };
    let slot0 = |stage: &str| {
        call.cbuffers
            .get(stage)
            .and_then(|v| v.iter().find(|b| b.slot == 0))
            .map(|b| b.floats.as_slice())
    };
    let (Some(vs), Some(ps)) = (slot0("vs"), slot0("ps")) else {
        return Ok(None);
    };
    let (Some(local), Some(rot), Some(alpha)) = (vs.get(12..14), vs.get(16..18), ps.get(52)) else {
        return Ok(None);
    };
    let angle = (-rot[1]).atan2(rot[0]);
    Ok(Some([local[0], local[1], angle, *alpha]))
}

/// The outline of the reachable cells of a capture, from its `OverlayShaders::PSContour` draw: the centroids of the
/// triangles of the strip in the original's basis (the vertices are in it, `Local` is the identity), deduplicated.
/// The client sets them on the cells under them and fills the inside (`client-app/src/golden.rs`). Empty when the
/// capture draws no contour.
pub fn contour_points(cache: &Path, scene: &str, capture: &str) -> Result<Vec<[f32; 2]>> {
    let dir = parsed_dir(cache, scene, capture);
    let draws = DrawsFile::load(&dir)?;
    let Some(draw) = draws.draws.iter().find(|d| {
        d.ps_entry
            .as_deref()
            .is_some_and(|e| e.starts_with("OverlayShaders::PSContour"))
    }) else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(dir.join("events.json"))
        .with_context(|| format!("failed to read events.json of {capture}"))?;
    let events: serde_json::Value = serde_json::from_str(&text).context("events.json")?;
    let call = events["calls"]
        .as_array()
        .and_then(|c| {
            c.iter()
                .find(|c| c["eid"].as_u64() == Some(u64::from(draw.event)))
        })
        .context("the contour draw is not in events.json")?;
    let buffer = |v: &serde_json::Value| -> Result<(Vec<u8>, usize, usize)> {
        let res = v["resource"].as_str().context("buffer resource")?;
        let bytes = std::fs::read(dir.join("buffers").join(format!("{res}.bin")))
            .with_context(|| format!("buffer {res} of {capture}"))?;
        let offset = v["offset"].as_u64().unwrap_or(0) as usize;
        let stride = v["stride"].as_u64().context("buffer stride")? as usize;
        Ok((bytes, offset, stride))
    };
    let (vb, voff, vstride) = buffer(&call["vertex_buffers"][0])?;
    let (ib, ioff, istride) = buffer(&call["index_buffer"])?;
    let count = call["index_count"].as_u64().unwrap_or(0) as usize;
    let index = |k: usize| -> Option<usize> {
        let at = ioff + k * istride;
        match istride {
            2 => Some(usize::from(u16::from_le_bytes(
                ib.get(at..at + 2)?.try_into().ok()?,
            ))),
            4 => Some(u32::from_le_bytes(ib.get(at..at + 4)?.try_into().ok()?) as usize),
            _ => None,
        }
    };
    let vertex = |i: usize| -> Option<[f32; 2]> {
        let at = voff + i * vstride;
        let b = vb.get(at..at + 8)?;
        Some([
            f32::from_le_bytes(b[0..4].try_into().ok()?),
            f32::from_le_bytes(b[4..8].try_into().ok()?),
        ])
    };
    let mut out: Vec<[f32; 2]> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for t in 0..count / 3 {
        let corners: Option<Vec<[f32; 2]>> =
            (0..3).map(|k| index(t * 3 + k).and_then(vertex)).collect();
        let Some(c) = corners else { continue };
        let p = [
            (c[0][0] + c[1][0] + c[2][0]) / 3.0,
            (c[0][1] + c[1][1] + c[2][1]) / 3.0,
        ];
        if seen.insert(((p[0] * 4.0).round() as i64, (p[1] * 4.0).round() as i64)) {
            out.push(p);
        }
    }
    Ok(out)
}

/// The text of `<capture>.golden` (the reader is `client-app/src/golden.rs`).
pub fn spec_text(
    scene: &Scene,
    capture: &str,
    size: (u32, u32),
    view_proj_orig: &DMat4,
    constants: &[(String, Vec<f32>)],
    ring: Option<[f32; 4]>,
    contour: &[[f32; 2]],
) -> String {
    let r = &scene.render;
    let mut s = String::from("civ5-golden/1\n");
    let _ = writeln!(s, "capture {capture}");
    let _ = writeln!(s, "size {} {}", size.0, size.1);
    let _ = writeln!(s, "preset {}", r.preset);
    let _ = writeln!(s, "msaa {}", r.msaa);
    let _ = writeln!(s, "disable {}", r.disable.join(","));
    let _ = writeln!(s, "grid {}", u8::from(r.grid));
    // `reveal = "capture-mask"` (S3, a world builder map): the fog of the capture, not the game's. The
    // capture keeps no `FOWMask` target, and its frame has no cloud and no dimmed hex: every plot is
    // revealed and visible. (The game's reveal is player 0's at the start of the scenario: clouds over
    // most of the map.)
    if let Some(secs) = r.settle_s {
        let _ = writeln!(s, "settle {secs}");
    }
    if scene.view.reveal == "capture-mask" {
        let _ = writeln!(s, "reveal all");
    }
    if let Some((_, v)) = constants
        .iter()
        .find(|(n, v)| TIME_NAMES.contains(&n.as_str()) && !v.is_empty())
    {
        let _ = writeln!(s, "time {}", v[0]);
    }
    let cols: Vec<String> = view_proj_orig
        .to_cols_array()
        .iter()
        .map(|x| format!("{}", *x as f32))
        .collect();
    let _ = writeln!(s, "view_proj_orig {}", cols.join(" "));
    for (name, v) in constants {
        let v: Vec<String> = v.iter().map(|x| format!("{x}")).collect();
        let _ = writeln!(s, "const {name} {}", v.join(" "));
    }
    if let Some([x, y, angle, alpha]) = ring {
        let _ = writeln!(s, "ring {x} {y} {angle} {alpha}");
    }
    if !contour.is_empty() {
        let pts: Vec<String> = contour.iter().map(|[x, y]| format!("{x} {y}")).collect();
        let _ = writeln!(s, "contour {}", pts.join(" "));
    }
    s
}

/// An uncompressed `R8G8B8A8_UNORM` DDS, the form `golden report` reads for a frame of ours.
pub fn dds_bytes(img: &Rgba8) -> Result<Vec<u8>> {
    let mut dds = Dds::new_dxgi(NewDxgiParams {
        height: img.height as u32,
        width: img.width as u32,
        depth: None,
        format: DxgiFormat::R8G8B8A8_UNorm,
        mipmap_levels: Some(1),
        array_layers: None,
        caps2: None,
        is_cubemap: false,
        resource_dimension: D3D10ResourceDimension::Texture2D,
        alpha_mode: AlphaMode::Straight,
    })
    .context("DDS header")?;
    dds.data.clone_from(&img.rgba);
    let mut out = Vec::new();
    dds.write(&mut out).context("write the DDS")?;
    Ok(out)
}

fn run(root: &Path, mut c: Command, what: &str) -> Result<()> {
    c.current_dir(root);
    let status = c.status().with_context(|| format!("cannot start {what}"))?;
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

/// Frames of ours for the captures of a scene; returns the captures drawn.
pub fn run_scene(o: &Options) -> Result<Vec<String>> {
    let scene = Scene::load_by_id(&o.scenes_dir, &o.scene)?;
    let scene_dir = o.cache.join("golden").join(&scene.scene.id);
    for f in ["mapbasis.wv3", "world.wv3"] {
        if !scene_dir.join(f).is_file() {
            bail!(
                "render: {} is missing: run `cargo xtask golden world {}` first",
                scene_dir.join(f).display(),
                scene.scene.id
            );
        }
    }
    if !scene_dir.join("catalog.wv3").is_file() {
        println!(
            "note: no catalog.wv3 in {}: terrain and units have no names; run `golden world {}` again",
            scene_dir.display(),
            scene.scene.id
        );
    }
    let mut drawn = Vec::new();
    let shift_free = OursToOrig {
        shift: glam::DVec3::ZERO,
    };
    for capture in &scene.scene.captures {
        if !o.captures.is_empty() && !o.captures.contains(capture) {
            continue;
        }
        let dir = parsed_dir(&o.cache, &scene.scene.id, capture);
        if !dir.join("draws.json").is_file() {
            println!("{capture}: not parsed, skipped");
            continue;
        }
        let draws = DrawsFile::load(&dir)?;
        let reference = camera::reference_matrix(
            &o.cache,
            &scene.scene.id,
            capture,
            &o.cbmap_dirs,
            None,
            None,
        )?;
        let matrix = camera::registers_to_matrix(&reference.matrix, reference.aspect, &shift_free)
            .with_context(|| {
                format!(
                    "{capture}: the view-projection constant {}",
                    reference.const_name
                )
            })?;
        let constants = time_constants(
            &o.cache,
            &scene.scene.id,
            capture,
            &scene.render.time_constants,
            &o.cbmap_dirs,
        )?;
        println!(
            "{capture}: {}x{}, camera from event {} ({}), {} time constants",
            draws.width,
            draws.height,
            reference.event,
            reference.const_name,
            constants.len()
        );
        let text = spec_text(
            &scene,
            capture,
            (draws.width, draws.height),
            &matrix,
            &constants,
            selection_ring(&o.cache, &scene.scene.id, capture)?,
            &contour_points(&o.cache, &scene.scene.id, capture)?,
        );
        std::fs::write(scene_dir.join(format!("{capture}.golden")), text)?;
        drawn.push(capture.clone());
    }
    if drawn.is_empty() {
        bail!(
            "render: no parsed capture of {} (run `cargo xtask golden parse`)",
            scene.scene.id
        );
    }
    std::fs::create_dir_all(&o.out)?;
    match o.backend {
        Backend::Native => {
            let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            c.args(["run", "-p", "app-desktop", "--release", "--", "--golden"])
                .arg(&scene_dir)
                .arg("--out")
                .arg(&o.out)
                .arg("--assets")
                .arg(&o.assets);
            for capture in &drawn {
                c.args(["--capture", capture]);
            }
            run(&o.root, c, "app-desktop --golden")?;
        }
        Backend::WebGpu | Backend::WebGl2 => {
            let lib = o
                .gamecore_lib
                .as_ref()
                .context("render: the browser backends need --gamecore-lib <dir> (the web bundle of the playtest)")?;
            let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            c.args([
                "xtask",
                "playtest",
                "--scenario",
                "golden",
                "--headed",
                "--gamecore-lib",
            ])
            .arg(lib)
            .arg("--assets")
            .arg(&o.assets)
            .arg("--golden-dir")
            .arg(&scene_dir)
            .arg("--golden-out")
            .arg(&o.out)
            .arg("--golden-captures")
            .arg(drawn.join(","))
            .arg("--golden-backend")
            .arg(o.backend.tag());
            run(&o.root, c, "cargo xtask playtest --scenario golden")?;
        }
    }
    for capture in &drawn {
        finish_frame(&o.out, capture, o.backend)?;
    }
    Ok(drawn)
}

/// Raw frame -> PNG (and, natively, the DDS of the report).
pub fn finish_frame(out: &Path, capture: &str, backend: Backend) -> Result<()> {
    let base = out.join(format!("{capture}-{}", backend.tag()));
    let size = std::fs::read_to_string(base.with_extension("txt"))
        .with_context(|| format!("{}: the frame was not written", base.display()))?;
    let n: Vec<usize> = size
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .context("size file")?;
    let [w, h] = n.as_slice() else {
        bail!("{}: size file needs width and height", base.display());
    };
    let img = read_raw_rgba(&base.with_extension("rgba"), *w, *h)?;
    write_png(&base.with_extension("png"), &img)?;
    if backend == Backend::Native {
        std::fs::write(out.join(format!("{capture}.dds")), dds_bytes(&img)?)?;
    }
    println!("{capture}: {}", base.with_extension("png").display());
    Ok(())
}

/// The classes of the class mask the two backends are compared on: the land with the
/// decals baked into it (G-T6), the water (G-W6) and the rivers (G-W6).
pub const BACKEND_CLASSES: [(&str, u8); 3] = [("land", 1), ("water", 2), ("river", 3)];

/// Mean FLIP the WebGL2 frame may differ from the WebGPU frame by, per class (G-T6, G-W6: 0.01).
pub const BACKEND_FLIP_MAX: f64 = 0.01;

/// One row of [`compare_backends`].
#[derive(Debug, Clone, PartialEq)]
pub struct BackendRow {
    pub class: &'static str,
    /// Pixels of the class mask after the erosion; the row says nothing when there are none.
    pub pixels: usize,
    pub flip: f64,
}

impl BackendRow {
    pub fn passes(&self) -> bool {
        self.pixels == 0 || self.flip <= BACKEND_FLIP_MAX
    }
}

/// Our WebGL2 frame against our WebGPU frame (AC-R8.5; G-T6, G-W6): mean FLIP over the masks `land`, `water`
/// and `river` of the WebGPU frame's class mask. Both frames are `<capture>-<backend>.rgba` in `dir`, as
/// `golden render --backend webgpu|webgl2` leaves them.
pub fn compare_backends(dir: &Path, capture: &str) -> Result<Vec<BackendRow>> {
    use crate::metrics::{Mask, PairMaps};
    let size_of = |backend: Backend| -> Result<(usize, usize)> {
        let p = dir.join(format!("{capture}-{}.txt", backend.tag()));
        let text = std::fs::read_to_string(&p).with_context(|| {
            format!(
                "{}: render the capture with this backend first",
                p.display()
            )
        })?;
        let n: Vec<usize> = text
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .with_context(|| format!("{}: size file", p.display()))?;
        match n.as_slice() {
            [w, h] => Ok((*w, *h)),
            _ => bail!("{}: size file needs width and height", p.display()),
        }
    };
    let (gpu, gl) = (size_of(Backend::WebGpu)?, size_of(Backend::WebGl2)?);
    if gpu != gl {
        bail!(
            "{capture}: the frames differ in size ({}x{} WebGPU, {}x{} WebGL2)",
            gpu.0,
            gpu.1,
            gl.0,
            gl.1
        );
    }
    let (w, h) = gpu;
    let read = |backend: Backend| {
        read_raw_rgba(&dir.join(format!("{capture}-{}.rgba", backend.tag())), w, h)
    };
    let reference = read(Backend::WebGpu)?;
    let test = read(Backend::WebGl2)?;
    let classes = std::fs::read(dir.join(format!("{capture}-webgpu.mask.r8")))
        .with_context(|| format!("{capture}: the class mask of the WebGPU frame"))?;
    if classes.len() != w * h {
        bail!(
            "{capture}: class mask has {} bytes for {w}x{h}",
            classes.len()
        );
    }
    let maps = PairMaps::new(&reference.to_image(), &test.to_image());
    Ok(BACKEND_CLASSES
        .iter()
        .map(|&(class, number)| {
            let m = maps.over(&Mask::from_class(w, h, &classes, number));
            BackendRow {
                class,
                pixels: m.flip.count,
                flip: m.flip.mean,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_frames_do_not_differ_on_any_class() {
        let dir = std::env::temp_dir().join(format!("golden-backends-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (48usize, 40usize);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
        for backend in ["webgpu", "webgl2"] {
            std::fs::write(dir.join(format!("C-{backend}.rgba")), &rgba).unwrap();
            std::fs::write(dir.join(format!("C-{backend}.txt")), format!("{w} {h}")).unwrap();
        }
        // Land on the left half, water on the right.
        let classes: Vec<u8> = (0..w * h)
            .map(|i| if i % w < w / 2 { 1 } else { 2 })
            .collect();
        std::fs::write(dir.join("C-webgpu.mask.r8"), classes).unwrap();
        let rows = compare_backends(&dir, "C").unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|r| (r.pixels == 0 || r.flip < 1e-4) && r.passes()),
            "{rows:?}"
        );
        assert!(rows[0].pixels > 0 && rows[1].pixels > 0);
        assert_eq!(rows[2].pixels, 0, "no river in this frame");
        // A frame that differs in the water only.
        let mut other = rgba.clone();
        for y in 0..h {
            for x in w / 2..w {
                other[(y * w + x) * 4] ^= 0xFF;
            }
        }
        std::fs::write(dir.join("C-webgl2.rgba"), other).unwrap();
        let rows = compare_backends(&dir, "C").unwrap();
        assert!(rows[0].passes(), "land is the same");
        assert!(!rows[1].passes(), "water differs: {rows:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn globs() {
        assert!(glob_match("*Time*", "g_fTimeOfDay"));
        assert!(glob_match("*Scroll*", "v2CloudScroll"));
        assert!(glob_match("Time", "Time"));
        assert!(!glob_match("Time", "g_Time"));
        assert!(glob_match("g_*Scroll", "g_UVScroll"));
        assert!(!glob_match("g_*Scroll", "g_UVScrolls"));
        assert!(!glob_match("a*a", "a"));
    }

    #[test]
    fn dds_round_trips_through_the_reader() {
        let img = Rgba8 {
            width: 2,
            height: 2,
            rgba: (0..16).collect(),
        };
        let back = crate::image::read_dds_rgba8(&dds_bytes(&img).unwrap()).unwrap();
        assert_eq!(back, img);
    }
}
