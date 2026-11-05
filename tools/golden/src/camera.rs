//! Camera check against a capture (`golden camera`, AC-R9.6 second half).
//!
//! The view-projection matrix of the reference draw is read from the constant buffer through the
//! constant map of `shader-re`, translated into our world basis (`P_ours = P_orig * M`, where `M` =
//! `ours_to_orig`: the rotation `B` then the shift `T` of the world origin), and fitted by our
//! `GameCamera` (`h`, level, focus from the eye). The capture passes when the matrix of that
//! `GameCamera` is the captured one within 0.1 %.
//!
//! Conventions. A matrix is stored as the original reads it: four registers are the four rows, and
//! `clip = P * (x, y, z, 1)` (`dp4` of a row with the position). Our basis: X east, Y up, north is
//! -Z; the original: Z up, north is +Y; so `B: (x, y, z) -> (x, -z, y)`. The shift `T` of the origin
//! (the original's origin is the map centre, ours the centre of hex (0, 0)) belongs to
//! `TerrainFrame` (world-terrain 4.4, R2.8); until it exists it is given by hand or computed from
//! the terrain constants of the capture by `shift_from_terrain_constants`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use glam::{DMat3, DMat4, DVec3, DVec4, Vec2};
use render::GameCamera;
use render::camera_params as p;
use serde::{Deserialize, Serialize};

use crate::captures::{DrawsFile, parsed_dir};

/// Names (CTAB) the view-projection constant of a world vertex shader can have. The names in the real
/// captures: `g_VP` (WaterShader), `g_ViewProjMatrix` (IceShader, WaveShader), `mtxViewProj` (ForestShader),
/// `ViewProjMatrix` (FXShader); the terrain draws have no CTAB (DX11-only vertex shader).
pub const VIEW_PROJ_NAMES: [&str; 7] = [
    "m_mViewProj",
    "ViewProjMatrix",
    "g_mViewProj",
    "ViewProj",
    "g_VP",
    "g_ViewProjMatrix",
    "mtxViewProj",
];

/// A matrix, a distance or an angle agrees when it is within this fraction.
pub const TOLERANCE: f64 = 0.001;

/// Aspect of the captured frames (1920x1080).
pub const ASPECT_16_9: f64 = 16.0 / 9.0;

// ------------------------------------------------------------ the basis

/// `B`: our (x, y, z) -> original (x, -z, y). A rotation, determinant 1.
pub fn basis_b() -> DMat3 {
    DMat3::from_cols(
        DVec3::new(1.0, 0.0, 0.0),  // our X -> original X
        DVec3::new(0.0, 0.0, 1.0),  // our Y (up) -> original Z
        DVec3::new(0.0, -1.0, 0.0), // our Z (south) -> original -Y
    )
}

/// `ours_to_orig`: `p_orig = B * p_ours + shift`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OursToOrig {
    pub shift: DVec3,
}

impl OursToOrig {
    pub fn matrix(&self) -> DMat4 {
        let b = basis_b();
        DMat4::from_cols(
            b.x_axis.extend(0.0),
            b.y_axis.extend(0.0),
            b.z_axis.extend(0.0),
            self.shift.extend(1.0),
        )
    }

    pub fn inverse(&self) -> DMat4 {
        self.matrix().inverse()
    }
}

/// The shift `T` from the terrain constants of the capture (`m_vTerrainScale`, `m_fHalfWorldWidth`,
/// `m_fHalfWorldHeight`) and the texel `(u, v)` of the centre of hex (0, 0) (`g_HexToTex`, RW-6;
/// `TerrainFrame` gives it from R2.8). The original's world is `u * sx - half_w` east and
/// `v * sy - half_h` north (WT-D11: the origin is the map centre; `TerrainFrame`: `Z = -(v * sy + ay)`),
/// heights are not shifted (plain ground is 12 in both worlds).
pub fn shift_from_terrain_constants(
    scale_xy: [f64; 2],
    half_world: [f64; 2],
    origin_texel: [f64; 2],
) -> DVec3 {
    DVec3::new(
        origin_texel[0] * scale_xy[0] - half_world[0],
        origin_texel[1] * scale_xy[1] - half_world[1],
        0.0,
    )
}

// ------------------------------------------------------------ matrices

/// 16 floats = four registers = the rows of the matrix.
pub fn matrix_from_rows(f: &[f32]) -> DMat4 {
    let c: Vec<f64> = f.iter().take(16).map(|&x| f64::from(x)).collect();
    DMat4::from_cols_array(&c.try_into().expect("16 floats")).transpose()
}

fn row(m: &DMat4, i: usize) -> DVec4 {
    m.transpose().col(i)
}

/// Max of the absolute values of the elements.
pub fn max_abs(m: &DMat4) -> f64 {
    m.to_cols_array().iter().fold(0.0, |a, x| a.max(x.abs()))
}

/// Largest elementwise difference.
pub fn max_diff(a: &DMat4, b: &DMat4) -> f64 {
    a.to_cols_array()
        .iter()
        .zip(b.to_cols_array())
        .fold(0.0, |m, (x, y)| m.max((x - y).abs()))
}

/// The point where clip x, y and w are all zero: the eye of a perspective matrix.
pub fn eye_from_matrix(m: &DMat4) -> Option<DVec3> {
    let (a, b, c) = (row(m, 0), row(m, 1), row(m, 3));
    let det3 = |skip: usize| {
        let pick = |r: DVec4| {
            let v = r.to_array();
            let mut o = [0.0; 3];
            let mut k = 0;
            for (i, x) in v.iter().enumerate() {
                if i != skip {
                    o[k] = *x;
                    k += 1;
                }
            }
            DVec3::from_array(o)
        };
        pick(a).dot(pick(b).cross(pick(c)))
    };
    let e = [det3(0), -det3(1), det3(2), -det3(3)];
    (e[3].abs() > 1e-12 * e.iter().fold(0.0f64, |m, x| m.max(x.abs())).max(1e-300))
        .then(|| DVec3::new(e[0] / e[3], e[1] / e[3], e[2] / e[3]))
}

/// What a perspective matrix says about the camera, whatever the handedness of the basis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Decomposed {
    /// Angle of the view direction below the horizon, degrees (45 in the game). Needs our basis
    /// (Y up): call on `P_ours`.
    pub pitch_deg: f64,
    pub fov_y_deg: f64,
    /// Width / height of the frustum.
    pub aspect: f64,
    pub near: f64,
    pub far: f64,
}

/// Pitch, vertical FOV, aspect and clip planes of `P` (clip depth 0..1, as D3D). Row 3 is the view
/// direction (clip w = depth along it), row 1 and row 0 are the up and right axes scaled by the
/// projection, row 2 is depth: `z = a * w + b`.
pub fn decompose(m: &DMat4) -> Result<Decomposed> {
    let (r0, r1, r2, r3) = (row(m, 0), row(m, 1), row(m, 2), row(m, 3));
    let fwd_raw = r3.truncate();
    let len = fwd_raw.length();
    if len < 1e-12 {
        bail!("not a perspective matrix: the w row has no direction");
    }
    let fwd = fwd_raw / len;
    let eye = eye_from_matrix(m).context("matrix has no eye (not a perspective)")?;
    let tan_half = len / r1.truncate().length();
    let aspect = r1.truncate().length() / r0.truncate().length();
    let a = r2.truncate().dot(fwd) / len;
    let b = r2.w / len + a * fwd.dot(eye);
    // Depth along the view direction d = w / len; z_ndc = (a * d + b) / d; 0 at near, 1 at far.
    let near = -b / a;
    let far = b / (1.0 - a);
    Ok(Decomposed {
        pitch_deg: (-fwd.y).clamp(-1.0, 1.0).asin().to_degrees(),
        fov_y_deg: 2.0 * tan_half.atan().to_degrees(),
        aspect,
        near,
        far,
    })
}

/// The matrix `P` with `clip = P * (x, y, z, 1)` in the basis of the original, from the registers of the
/// constant buffer: they are its rows (SM3) or its columns (the DX11 captures); the one that is a perspective
/// of the frame's aspect, looking down at a sane angle, is taken. The shift of `frame` does not matter here.
pub fn registers_to_matrix(read: &DMat4, aspect: f64, frame: &OursToOrig) -> Result<DMat4> {
    let sane = |m: &DMat4| {
        decompose(&(*m * frame.matrix())).is_ok_and(|d| {
            (d.aspect / aspect - 1.0).abs() < 0.02
                && (5.0..89.0).contains(&d.pitch_deg)
                && d.near > 0.0
                && d.far > d.near
        })
    };
    if sane(read) {
        return Ok(*read);
    }
    let t = read.transpose();
    if sane(&t) {
        return Ok(t);
    }
    bail!(
        "neither the registers nor their transpose are a perspective matrix of aspect {aspect:.3}"
    )
}

// ------------------------------------------------------------ our camera

/// `GameCamera` parameters that put the eye where the capture has it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OurCamera {
    /// Height of the eye above the focus plane.
    pub h: f64,
    pub level: f64,
    /// Ground point (X, Z) in the middle of the screen.
    pub focus: [f64; 2],
}

/// `h = eye.y - 12`; `level = ln(h / 50) / 0.15`; `focus = (eye.x, eye.z - h / tan 45deg)`.
pub fn our_camera_from_eye(eye: DVec3) -> OurCamera {
    let h = eye.y - f64::from(p::FOCUS_HEIGHT);
    OurCamera {
        h,
        level: (h / f64::from(p::ZOOM_BASE_HEIGHT)).ln() / f64::from(p::ZOOM_RATE),
        focus: [eye.x, eye.z - h / f64::from(p::PITCH).tan()],
    }
}

impl OurCamera {
    /// Whether `GameCamera` can have this level (it clamps to its range).
    pub fn on_grid(&self) -> bool {
        self.h > 0.0
            && (f64::from(p::LEVEL_MIN) - 1e-6..=f64::from(p::LEVEL_MAX) + 1e-6)
                .contains(&self.level)
    }

    /// View-projection of the real `GameCamera` at these parameters.
    pub fn matrix(&self, aspect: f64) -> DMat4 {
        let mut c = GameCamera::new(Vec2::new(self.focus[0] as f32, self.focus[1] as f32));
        c.set_level(self.level as f32);
        c.view_proj(aspect as f32).as_dmat4()
    }

    pub fn eye(&self) -> DVec3 {
        let mut c = GameCamera::new(Vec2::new(self.focus[0] as f32, self.focus[1] as f32));
        c.set_level(self.level as f32);
        c.eye().as_dvec3()
    }
}

// ------------------------------------------------------------ the check

/// How strict the check is: the matrix itself (S1a, S1b, S2) or only pitch, FOV and planes (S3,
/// whose benchmark camera may stand off the zoom grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Matrix,
    PitchFovPlanes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraReport {
    pub mode: Mode,
    pub passed: bool,
    pub problems: Vec<String>,
    pub eye_ours: [f64; 3],
    pub our_camera: OurCamera,
    pub on_grid: bool,
    pub decomposed: Decomposed,
    /// `max |P_ours - G|` and the limit `0.001 * max |G|` (matrix mode, on grid).
    pub matrix_max_err: Option<f64>,
    pub matrix_limit: Option<f64>,
    /// Eye of the constant against the eye of the matrix, as a fraction of `h` (if the draw has one).
    pub eye_err_rel_h: Option<f64>,
    pub fov_err_rel: f64,
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-300)
}

/// The check of one captured matrix. `eye_const` is the eye position the draw carries, if it does
/// (original world coordinates).
pub fn check_matrix(
    p_orig: &DMat4,
    frame: &OursToOrig,
    mode: Mode,
    eye_const: Option<DVec3>,
) -> Result<CameraReport> {
    check_matrix_aspect(p_orig, frame, mode, eye_const, ASPECT_16_9)
}

/// `check_matrix` for a frame of another aspect (the real captures are windows: 1914x1051, 1914x1171).
pub fn check_matrix_aspect(
    p_orig: &DMat4,
    frame: &OursToOrig,
    mode: Mode,
    eye_const: Option<DVec3>,
    aspect: f64,
) -> Result<CameraReport> {
    let mut r = check_matrix_as_read(p_orig, frame, mode, eye_const, aspect);
    // A constant stored column by column reads as the transpose: say so when that one fits.
    let transposed_fits = || {
        check_matrix_as_read(&p_orig.transpose(), frame, mode, None, aspect).is_ok_and(|t| t.passed)
    };
    match &mut r {
        Ok(rep) if !rep.passed && transposed_fits() => rep
            .problems
            .push("the transposed matrix fits: the constant is stored column-major".into()),
        Err(_) if transposed_fits() => {
            bail!(
                "not a perspective matrix as read, but its transpose fits: the constant is stored column-major"
            )
        }
        _ => {}
    }
    r
}

fn check_matrix_as_read(
    p_orig: &DMat4,
    frame: &OursToOrig,
    mode: Mode,
    eye_const: Option<DVec3>,
    aspect: f64,
) -> Result<CameraReport> {
    let m = frame.matrix();
    let p_ours = *p_orig * m;
    let d = decompose(&p_ours)?;
    let eye_orig = eye_from_matrix(p_orig).context("the matrix of the draw has no eye")?;
    let eye = frame.inverse().transform_point3(eye_orig);
    let cam = our_camera_from_eye(eye);
    let mut problems = Vec::new();

    let fov_ref = f64::from(p::fov_y_for_aspect(aspect as f32)).to_degrees();
    let fov_err = rel(d.fov_y_deg, fov_ref);
    if fov_err > TOLERANCE {
        problems.push(format!(
            "FOV {:.4} deg, ours {:.4} deg for aspect {aspect:.4} ({:.3} % off)",
            d.fov_y_deg,
            fov_ref,
            fov_err * 100.0
        ));
    }
    if rel(d.aspect, aspect) > TOLERANCE {
        problems.push(format!("aspect {:.4}, the frame is {aspect:.4}", d.aspect));
    }
    let pitch_ref = f64::from(p::PITCH).to_degrees();
    if rel(d.pitch_deg, pitch_ref) > TOLERANCE {
        problems.push(format!("pitch {:.4} deg, ours {pitch_ref:.4}", d.pitch_deg));
    }
    for (name, got, want) in [
        ("near plane", d.near, f64::from(p::NEAR)),
        ("far plane", d.far, f64::from(p::FAR)),
    ] {
        if rel(got, want) > TOLERANCE {
            problems.push(format!("{name} {got:.3}, ours {want}"));
        }
    }
    let eye_err_rel_h =
        eye_const.map(|e| (frame.inverse().transform_point3(e) - eye).length() / cam.h.abs());
    if let Some(e) = eye_err_rel_h
        && e > TOLERANCE
    {
        problems.push(format!(
            "eye constant is {:.3} % of h away from the eye of the matrix",
            e * 100.0
        ));
    }

    let on_grid = cam.on_grid();
    let (mut matrix_max_err, mut matrix_limit) = (None, None);
    if mode == Mode::Matrix {
        if !on_grid {
            problems.push(format!(
                "level {:.3} is outside GameCamera's {}..{} (h = {:.1}): the camera is off our zoom grid",
                cam.level,
                p::LEVEL_MIN,
                p::LEVEL_MAX,
                cam.h
            ));
        } else {
            let g = cam.matrix(aspect);
            let err = max_diff(&p_ours, &g);
            let limit = TOLERANCE * max_abs(&g);
            matrix_max_err = Some(err);
            matrix_limit = Some(limit);
            if err > limit {
                problems.push(format!(
                    "matrix differs from GameCamera by {err:.6} (limit {limit:.6})"
                ));
            }
            let ge = cam.eye();
            if (ge - eye).length() > TOLERANCE * cam.h {
                problems.push("GameCamera's eye is not the eye of the capture".into());
            }
        }
    }
    Ok(CameraReport {
        mode,
        passed: problems.is_empty(),
        problems,
        eye_ours: eye.to_array(),
        our_camera: cam,
        on_grid,
        decomposed: d,
        matrix_max_err,
        matrix_limit,
        eye_err_rel_h,
        fov_err_rel: fov_err,
    })
}

// ------------------------------------------------------------ reading a parsed capture

#[derive(Debug, Deserialize)]
pub(crate) struct EventsFile {
    pub(crate) calls: Vec<Call>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Call {
    pub(crate) eid: u32,
    #[serde(default)]
    pub(crate) cbuffers: BTreeMap<String, Vec<CBuffer>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CBuffer {
    pub(crate) slot: u32,
    #[serde(default)]
    pub(crate) offset: u32,
    pub(crate) floats: Vec<f32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CbMapFile {
    pub(crate) consts: Vec<CbConst>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CbConst {
    pub(crate) name: String,
    pub(crate) cb: u32,
    pub(crate) offset: u32,
    pub(crate) rows: u32,
}

/// `<cache>/shader-re/<key>/dx11/cbmap` folders (also `<cache>/shader-re/dx11/cbmap`).
pub fn cbmap_dirs(cache: &Path) -> Vec<PathBuf> {
    let root = cache.join("shader-re");
    let mut v = vec![root.join("dx11").join("cbmap"), root.join("cbmap")];
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            v.push(e.path().join("dx11").join("cbmap"));
        }
    }
    v.retain(|d| d.is_dir());
    v
}

/// File of an entry in a cbmap folder: `Pkg::Entry:Stage` -> `Pkg.Entry.Stage.json`.
pub(crate) fn cbmap_file(dir: &Path, entry: &str) -> PathBuf {
    dir.join(format!(
        "{}.json",
        entry.replace("::", ".").replace(':', ".")
    ))
}

/// Where the matrix constant is, if the entry has a map with one of `names`.
fn find_const(dirs: &[PathBuf], entry: &str, names: &[&str]) -> Option<(String, u32, u32)> {
    for d in dirs {
        let f = cbmap_file(d, entry);
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Ok(m) = serde_json::from_str::<CbMapFile>(&text) else {
            continue;
        };
        if let Some(c) = m
            .consts
            .iter()
            .find(|c| names.contains(&c.name.as_str()) && c.rows == 4)
        {
            return Some((c.name.clone(), c.cb, c.offset));
        }
    }
    None
}

/// The reference draw of a capture and its view-projection matrix.
#[derive(Debug, Clone)]
pub struct ReferenceMatrix {
    pub event: u32,
    pub entry: String,
    pub const_name: String,
    pub matrix: DMat4,
    /// Eye constant of the same draw, when asked for and present.
    pub eye: Option<DVec3>,
    /// Width / height of the captured frame.
    pub aspect: f64,
}

/// Looks for the first draw of the main pass (colour target of the frame size) whose vertex shader
/// has a mapped view-projection constant; `event` forces a draw, `eye_const` names an eye constant.
pub fn reference_matrix(
    cache: &Path,
    scene: &str,
    capture: &str,
    cbmap_dirs_: &[PathBuf],
    event: Option<u32>,
    eye_const: Option<&str>,
) -> Result<ReferenceMatrix> {
    let dir = parsed_dir(cache, scene, capture);
    let draws = DrawsFile::load(&dir)?;
    let text = std::fs::read_to_string(dir.join("events.json"))
        .with_context(|| format!("failed to read events.json of {capture}"))?;
    let events: EventsFile = serde_json::from_str(&text).context("events.json")?;
    for d in &draws.draws {
        if event.is_some_and(|e| e != d.event) || (event.is_none() && !d.world) {
            continue;
        }
        // The frame target of the main pass is the MSAA one: the shadow casters and the UI also draw into
        // frame-sized targets (single sample), and their matrices are not the camera's.
        let main_pass = d.targets.first().is_some_and(|t| {
            t.width == Some(draws.width)
                && t.height == Some(draws.height)
                && t.samples.is_none_or(|s| s > 1)
        });
        if !main_pass && event.is_none() {
            continue;
        }
        let Some(entry) = &d.vs_entry else { continue };
        let Some((name, cb, offset)) = find_const(cbmap_dirs_, entry, &VIEW_PROJ_NAMES) else {
            continue;
        };
        let call = events
            .calls
            .iter()
            .find(|c| c.eid == d.event)
            .context("draw is not in events.json")?;
        let floats_at = |slot: u32, byte_off: u32, n: usize| -> Result<Vec<f32>> {
            let b = call
                .cbuffers
                .get("vs")
                .and_then(|v| v.iter().find(|b| b.slot == slot))
                .with_context(|| {
                    format!("event {}: vertex cbuffer {slot} was not saved", d.event)
                })?;
            let start = (byte_off
                .checked_sub(b.offset)
                .context("constant before the saved range")?
                / 4) as usize;
            b.floats
                .get(start..start + n)
                .map(<[f32]>::to_vec)
                .with_context(|| {
                    format!("event {}: cbuffer {slot} is too short for {name}", d.event)
                })
        };
        let matrix = matrix_from_rows(&floats_at(cb, offset, 16)?);
        let eye = match eye_const {
            Some(n) => {
                let (_, cb, off) = find_const(cbmap_dirs_, entry, &[n])
                    .or_else(|| find_any(cbmap_dirs_, entry, n))
                    .with_context(|| format!("{entry} has no constant {n}"))?;
                let f = floats_at(cb, off, 3)?;
                Some(DVec3::new(
                    f64::from(f[0]),
                    f64::from(f[1]),
                    f64::from(f[2]),
                ))
            }
            None => None,
        };
        return Ok(ReferenceMatrix {
            event: d.event,
            entry: entry.clone(),
            const_name: name,
            matrix,
            eye,
            aspect: f64::from(draws.width) / f64::from(draws.height),
        });
    }
    bail!(
        "{capture}: no main-pass draw has a mapped view-projection constant ({}): run `shader-re dx11 cbmap` for the terrain entries, or name a draw with --event",
        VIEW_PROJ_NAMES.join(", ")
    )
}

/// A constant of any row count (an eye is one float4).
fn find_any(dirs: &[PathBuf], entry: &str, name: &str) -> Option<(String, u32, u32)> {
    for d in dirs {
        let Ok(text) = std::fs::read_to_string(cbmap_file(d, entry)) else {
            continue;
        };
        let Ok(m) = serde_json::from_str::<CbMapFile>(&text) else {
            continue;
        };
        if let Some(c) = m.consts.iter().find(|c| c.name == name) {
            return Some((c.name.clone(), c.cb, c.offset));
        }
    }
    None
}

/// Scene id of a capture name (`S2-bake` -> `S2`, `S1a` -> `S1`).
pub fn scene_of(capture: &str) -> &str {
    &capture[..2.min(capture.len())]
}

/// Captures that are checked, and how.
pub const CAPTURES: [(&str, Mode); 4] = [
    ("S1a", Mode::Matrix),
    ("S1b", Mode::Matrix),
    ("S2", Mode::Matrix),
    ("S3", Mode::PitchFovPlanes),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureCamera {
    pub capture: String,
    pub event: u32,
    pub entry: String,
    pub const_name: String,
    pub report: CameraReport,
}

/// `camera.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraFile {
    pub shift: [f64; 3],
    pub captures: Vec<CaptureCamera>,
    /// S1a against S1b: the same camera within 0.1 %.
    pub s1_pair_max_err: Option<f64>,
    pub s1_pair_passed: Option<bool>,
    pub passed: bool,
    pub notes: Vec<String>,
}

pub struct CameraOpts {
    pub frame: OursToOrig,
    pub cbmap_dirs: Vec<PathBuf>,
    pub event: Option<u32>,
    pub eye_const: Option<String>,
    /// Captures to look at (default: all of `CAPTURES` that are parsed).
    pub only: Vec<String>,
}

/// Runs the check on every parsed capture of `CAPTURES`.
pub fn run(cache: &Path, opts: &CameraOpts) -> Result<CameraFile> {
    let mut out = CameraFile {
        shift: opts.frame.shift.to_array(),
        captures: Vec::new(),
        s1_pair_max_err: None,
        s1_pair_passed: None,
        passed: true,
        notes: Vec::new(),
    };
    let mut s1: BTreeMap<&str, DMat4> = BTreeMap::new();
    for (name, mode) in CAPTURES {
        if !opts.only.is_empty() && !opts.only.iter().any(|o| o == name) {
            continue;
        }
        let dir = parsed_dir(cache, scene_of(name), name);
        if !dir.join("draws.json").is_file() {
            out.notes.push(format!("{name}: not parsed, skipped"));
            continue;
        }
        let r = reference_matrix(
            cache,
            scene_of(name),
            name,
            &opts.cbmap_dirs,
            opts.event,
            opts.eye_const.as_deref(),
        )?;
        // DX11 constants of the real captures are column-major (the registers are the columns): when the
        // matrix as read (SM3 rows) does not fit and its transpose does, the transpose is the matrix.
        let (matrix, report) = match check_matrix_aspect(
            &r.matrix,
            &opts.frame,
            mode,
            r.eye,
            r.aspect,
        ) {
            Ok(rep) if rep.passed => (r.matrix, rep),
            first => {
                let t = r.matrix.transpose();
                match check_matrix_aspect(&t, &opts.frame, mode, r.eye, r.aspect) {
                    Ok(rep) if rep.passed => {
                        out.notes.push(format!(
                            "{name}: the registers of {} are the columns of the matrix (column-major, DX11)",
                            r.const_name
                        ));
                        (t, rep)
                    }
                    // Neither fits: report the one with fewer problems (the DX11 one is the transpose).
                    Ok(rep)
                        if first
                            .as_ref()
                            .map_or(true, |f| rep.problems.len() < f.problems.len()) =>
                    {
                        out.notes.push(format!(
                            "{name}: read as columns (DX11), it still fails: see the problems"
                        ));
                        (t, rep)
                    }
                    second => (r.matrix, first.or(second)?),
                }
            }
        };

        out.passed &= report.passed;
        if name.starts_with("S1") {
            s1.insert(name, matrix * opts.frame.matrix());
        }
        out.captures.push(CaptureCamera {
            capture: name.to_string(),
            event: r.event,
            entry: r.entry,
            const_name: r.const_name,
            report,
        });
    }
    if let (Some(a), Some(b)) = (s1.get("S1a"), s1.get("S1b")) {
        let err = max_diff(a, b);
        let ok = err <= TOLERANCE * max_abs(a);
        out.s1_pair_max_err = Some(err);
        out.s1_pair_passed = Some(ok);
        out.passed &= ok;
    }
    if out.captures.is_empty() {
        out.notes
            .push("no parsed captures of S1a, S1b, S2, S3: nothing to check".into());
        out.passed = false;
    }
    Ok(out)
}
