//! Camera check on synthetic matrices: our own `GameCamera` is moved into the original's basis
//! (`P_orig = P_ours * M^-1`), written as a capture would have it, and read back.

use std::path::{Path, PathBuf};

use glam::{DMat4, DVec3};
use golden::camera::{
    ASPECT_16_9, CameraOpts, Mode, OurCamera, OursToOrig, basis_b, check_matrix, decompose,
    eye_from_matrix, matrix_from_rows, our_camera_from_eye, run, shift_from_terrain_constants,
};
use render::camera_params as p;

fn cam(level: f64, focus: [f64; 2]) -> OurCamera {
    OurCamera {
        h: f64::from(p::height_for_level(level as f32)),
        level,
        focus,
    }
}

fn frame() -> OursToOrig {
    // An arbitrary origin: a 40x24 map, hex (0, 0) centre at texel (32, 32), 64 texels per hex.
    OursToOrig {
        shift: shift_from_terrain_constants([1.0, 1.0], [1280.0, 768.0], [32.0, 32.0]),
    }
}

/// What the capture holds: the four registers, row by row.
fn captured_floats(p_orig: &DMat4) -> Vec<f32> {
    p_orig
        .transpose()
        .to_cols_array()
        .iter()
        .map(|&x| x as f32)
        .collect()
}

fn p_orig_of(c: &OurCamera, f: &OursToOrig) -> DMat4 {
    c.matrix(ASPECT_16_9) * f.inverse()
}

#[test]
fn basis_b_is_a_rotation_of_the_stated_axes() {
    let b = basis_b();
    assert!((b.determinant() - 1.0).abs() < 1e-12);
    assert!((b * b.transpose() - glam::DMat3::IDENTITY).abs_diff_eq(glam::DMat3::ZERO, 1e-12));
    // East stays east, up becomes the original's Z, north (-Z ours) becomes +Y.
    assert_eq!(b * DVec3::X, DVec3::X);
    assert_eq!(b * DVec3::Y, DVec3::Z);
    assert_eq!(b * DVec3::new(0.0, 0.0, -1.0), DVec3::Y);
}

#[test]
fn m_and_its_inverse_cancel_and_move_the_origin() {
    let f = frame();
    let id = f.inverse() * f.matrix();
    assert!(id.abs_diff_eq(DMat4::IDENTITY, 1e-9));
    // The centre of hex (0, 0) (our origin) is the shift in the original's world.
    assert!(
        f.matrix()
            .transform_point3(DVec3::ZERO)
            .abs_diff_eq(f.shift, 1e-12)
    );
    assert_eq!(f.shift, DVec3::new(32.0 - 1280.0, 32.0 - 768.0, 0.0));
    // A point 10 east and 5 north of it.
    let q = f.matrix().transform_point3(DVec3::new(10.0, 0.0, -5.0));
    assert!(q.abs_diff_eq(f.shift + DVec3::new(10.0, 5.0, 0.0), 1e-12));
}

#[test]
fn game_camera_matrix_decomposes_to_the_game_numbers() {
    for level in [11.0, 14.25, 20.5] {
        let m = cam(level, [300.0, -200.0]).matrix(ASPECT_16_9);
        let d = decompose(&m).unwrap();
        assert!((d.pitch_deg - 45.0).abs() < 1e-3, "{d:?}");
        assert!(
            (d.near - 50.0).abs() < 1e-2 && (d.far - 4000.0).abs() < 1.0,
            "{d:?}"
        );
        assert!((d.aspect - ASPECT_16_9).abs() < 1e-5, "{d:?}");
        let want = f64::from(p::fov_y_for_aspect(ASPECT_16_9 as f32)).to_degrees();
        assert!((d.fov_y_deg - want).abs() < 1e-3, "{d:?}");
    }
}

#[test]
fn eye_is_the_null_vector_of_rows_x_y_w() {
    let c = cam(15.0, [123.0, -456.0]);
    let eye = eye_from_matrix(&c.matrix(ASPECT_16_9)).unwrap();
    assert!(eye.abs_diff_eq(c.eye(), 1e-2), "{eye} {}", c.eye());
}

#[test]
fn fit_recovers_h_level_and_focus() {
    let c = cam(16.5, [800.0, -300.0]);
    let fitted = our_camera_from_eye(c.eye());
    assert!((fitted.level - 16.5).abs() < 1e-4, "{fitted:?}");
    assert!((fitted.h - c.h).abs() < 1e-2);
    assert!((fitted.focus[0] - 800.0).abs() < 1e-3 && (fitted.focus[1] + 300.0).abs() < 1e-2);
}

#[test]
fn game_camera_round_trips_through_the_originals_basis() {
    let f = frame();
    for (level, focus) in [
        (11.0, [0.0, 0.0]),
        (14.25, [420.0, -310.0]),
        (19.0, [2000.0, -900.0]),
        (20.5, [64.0, -10.0]),
    ] {
        let c = cam(level, focus);
        let p_orig = matrix_from_rows(&captured_floats(&p_orig_of(&c, &f)));
        let r = check_matrix(&p_orig, &f, Mode::Matrix, None).unwrap();
        assert!(r.passed, "level {level}: {:?}", r.problems);
        assert!(r.on_grid);
        assert!(
            (r.our_camera.level - level).abs() < 1e-3,
            "{:?}",
            r.our_camera
        );
        assert!(
            (r.our_camera.focus[0] - focus[0]).abs() < 0.05,
            "{:?}",
            r.our_camera
        );
        assert!(r.matrix_max_err.unwrap() < r.matrix_limit.unwrap());
        // The eye the draw would carry as a constant agrees.
        let eye_orig = f.matrix().transform_point3(c.eye());
        let r = check_matrix(&p_orig, &f, Mode::Matrix, Some(eye_orig)).unwrap();
        assert!(
            r.passed && r.eye_err_rel_h.unwrap() < 1e-3,
            "{:?}",
            r.problems
        );
    }
}

#[test]
fn another_shift_moves_the_fitted_focus() {
    // With another T the matrix still matches its own fit, so the camera check passes: T is
    // checked by the terrain constants of the capture. The fitted focus moves by the difference.
    let c = cam(14.0, [500.0, -400.0]);
    let p_orig = p_orig_of(&c, &frame());
    let other = OursToOrig {
        shift: frame().shift + DVec3::new(64.0, 0.0, 0.0),
    };
    let r = check_matrix(&p_orig, &other, Mode::Matrix, None).unwrap();
    assert!(
        (r.our_camera.focus[0] - 500.0).abs() > 60.0,
        "{:?}",
        r.our_camera
    );
}

#[test]
fn a_different_pitch_fails() {
    let f = frame();
    let c = cam(14.0, [500.0, -400.0]);
    // Tilt the world 5 degrees around the eye: the same eye, a steeper (or flatter) view.
    let eye = c.eye();
    let tilt = DMat4::from_translation(eye)
        * DMat4::from_rotation_x(5f64.to_radians())
        * DMat4::from_translation(-eye);
    let p_ours = c.matrix(ASPECT_16_9) * tilt;
    let r = check_matrix(&(p_ours * f.inverse()), &f, Mode::Matrix, None).unwrap();
    assert!(!r.passed);
    assert!(
        r.problems.iter().any(|s| s.contains("pitch")),
        "{:?}",
        r.problems
    );
    assert!(
        r.problems.iter().any(|s| s.contains("matrix differs")),
        "{:?}",
        r.problems
    );
    let loose = check_matrix(&(p_ours * f.inverse()), &f, Mode::PitchFovPlanes, None).unwrap();
    assert!(!loose.passed, "pitch is checked in the loose mode too");
}

#[test]
fn another_fov_and_planes_fail() {
    let f = frame();
    let c = cam(14.0, [500.0, -400.0]);
    let mut m = c.matrix(ASPECT_16_9);
    // Widen the FOV by 2 %: scale the up row (clip y) and the right row (clip x) down.
    let rows = m.transpose();
    let (mut r0, mut r1) = (rows.col(0), rows.col(1));
    r0 *= 1.0 / 1.02;
    r1 *= 1.0 / 1.02;
    m = DMat4::from_cols(r0, r1, rows.col(2), rows.col(3)).transpose();
    let r = check_matrix(&(m * f.inverse()), &f, Mode::PitchFovPlanes, None).unwrap();
    assert!(!r.passed);
    assert!(
        r.problems.iter().any(|s| s.contains("FOV")),
        "{:?}",
        r.problems
    );
    assert!(r.fov_err_rel > 0.01);
}

#[test]
fn off_grid_camera_is_judged_by_pitch_fov_and_planes_only() {
    let f = frame();
    // Level 23: farther than GameCamera goes (benchmark camera). GameCamera cannot be built there,
    // so build the matrix by hand with the same math at that height.
    let h = 50.0 * (0.15f64 * 23.0).exp();
    let eye = DVec3::new(700.0, 12.0 + h, -300.0 + h);
    let m = {
        let near_cam = cam(20.5, [0.0, 0.0]);
        // Same orientation and projection as GameCamera, only the eye is farther: shift the
        // matrix by the translation that takes the near camera's eye to this eye.
        near_cam.matrix(ASPECT_16_9) * DMat4::from_translation(near_cam.eye() - eye)
    };
    let p_orig = m * f.inverse();
    let strict = check_matrix(&p_orig, &f, Mode::Matrix, None).unwrap();
    assert!(!strict.passed && !strict.on_grid);
    assert!(
        strict
            .problems
            .iter()
            .any(|s| s.contains("off our zoom grid")),
        "{:?}",
        strict.problems
    );
    let loose = check_matrix(&p_orig, &f, Mode::PitchFovPlanes, None).unwrap();
    assert!(loose.passed, "{:?}", loose.problems);
    assert!(loose.matrix_max_err.is_none());
}

#[test]
fn a_transposed_constant_is_named() {
    let f = frame();
    let c = cam(15.0, [200.0, -100.0]);
    let p_orig = p_orig_of(&c, &f);
    // Stored column by column: the registers read as the transpose.
    let stored: Vec<f32> = p_orig.to_cols_array().iter().map(|&x| x as f32).collect();
    let read = matrix_from_rows(&stored);
    let msg = match check_matrix(&read, &f, Mode::Matrix, None) {
        Ok(r) => {
            assert!(!r.passed);
            r.problems.join("; ")
        }
        Err(e) => format!("{e:#}"),
    };
    assert!(msg.contains("column-major"), "{msg}");
}

// ---- the whole command on a synthetic parsed capture

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("golden-camera-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn target(w: u32) -> serde_json::Value {
    serde_json::json!({"format": "R8G8B8A8_UNORM", "width": w, "height": if w == 1920 { 1080 } else { w }, "samples": 4})
}

fn draw(event: u32, entry: &str, tw: u32) -> serde_json::Value {
    serde_json::json!({
        "event": event, "pass": 0, "world": true,
        "vs": "a".repeat(40), "ps": "b".repeat(40), "hs": null, "ds": null,
        "vs_entry": entry, "ps_entry": null, "hs_entry": null, "ds_entry": null,
        "topology": "TriangleList", "index_count": 3, "vertex_count": 0, "instances": 1,
        "targets": [target(tw)], "depth_format": null
    })
}

/// A parsed capture: a shadow draw (2048 target, the light's matrix) first, then the main-pass draw.
fn write_capture(cache: &Path, scene: &str, name: &str, main: &DMat4, light: &DMat4) {
    let dir = cache.join("golden").join(scene).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let draws = serde_json::json!({
        "schema": "civ5-golden-draws/1", "scene": scene, "capture": name, "width": 1920, "height": 1080,
        "ui_boundary_event": 90,
        "draws": [draw(2, "ShadowShader::S:VS", 2048), draw(5, "TerrainShader::T:VS", 1920)],
        "by_entry": {}
    });
    std::fs::write(dir.join("draws.json"), draws.to_string()).unwrap();
    let cb = |m: &DMat4| {
        let mut f = vec![0.0f32; 8]; // 2 registers of something else first
        f.extend(captured_floats(m));
        serde_json::json!({"vs": [{"slot": 1, "offset": 0, "resource": "c", "floats": f}]})
    };
    let events = serde_json::json!({"calls": [
        {"eid": 2, "cbuffers": cb(light)}, {"eid": 5, "cbuffers": cb(main)}
    ]});
    std::fs::write(dir.join("events.json"), events.to_string()).unwrap();
}

fn write_cbmaps(cache: &Path) -> PathBuf {
    let dir = cache
        .join("shader-re")
        .join("0123456789ab")
        .join("dx11")
        .join("cbmap");
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["ShadowShader.S.VS.json", "TerrainShader.T.VS.json"] {
        let m = serde_json::json!({"entry": f, "consts": [
            {"name": "m_vOther", "cb": 1, "offset": 0, "rows": 2},
            {"name": "m_mViewProj", "cb": 1, "offset": 32, "rows": 4}
        ]});
        std::fs::write(dir.join(f), m.to_string()).unwrap();
    }
    dir
}

fn opts(cache: &Path, only: &[&str]) -> CameraOpts {
    CameraOpts {
        frame: frame(),
        cbmap_dirs: golden::camera::cbmap_dirs(cache),
        event: None,
        eye_const: None,
        only: only.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn command_reads_the_main_pass_matrix_through_the_constant_map() {
    let t = Tmp::new("run");
    write_cbmaps(&t.0);
    let f = frame();
    let light = p_orig_of(&cam(11.0, [0.0, 0.0]), &f); // a different camera: must not be picked
    let a = p_orig_of(&cam(14.25, [420.0, -310.0]), &f);
    for (s, n) in [("S1", "S1a"), ("S1", "S1b"), ("S2", "S2")] {
        write_capture(&t.0, s, n, &a, &light);
    }
    let r = run(&t.0, &opts(&t.0, &[])).unwrap();
    assert!(r.passed, "{r:?}");
    assert_eq!(r.captures.len(), 3);
    assert!(
        r.captures
            .iter()
            .all(|c| c.event == 5 && c.const_name == "m_mViewProj")
    );
    assert_eq!(r.s1_pair_passed, Some(true));
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("S3") && n.contains("not parsed"))
    );
}

#[test]
fn command_fails_when_s1a_and_s1b_cameras_differ() {
    let t = Tmp::new("pair");
    write_cbmaps(&t.0);
    let f = frame();
    let light = p_orig_of(&cam(11.0, [0.0, 0.0]), &f);
    write_capture(
        &t.0,
        "S1",
        "S1a",
        &p_orig_of(&cam(14.25, [420.0, -310.0]), &f),
        &light,
    );
    write_capture(
        &t.0,
        "S1",
        "S1b",
        &p_orig_of(&cam(14.25, [430.0, -310.0]), &f),
        &light,
    );
    let r = run(&t.0, &opts(&t.0, &[])).unwrap();
    assert!(!r.passed);
    assert_eq!(r.s1_pair_passed, Some(false));
    // Each camera is fine on its own.
    assert!(r.captures.iter().all(|c| c.report.passed));
}

#[test]
fn command_explains_a_missing_constant_map() {
    let t = Tmp::new("nomap");
    let f = frame();
    write_capture(
        &t.0,
        "S1",
        "S1a",
        &p_orig_of(&cam(14.0, [0.0, 0.0]), &f),
        &p_orig_of(&cam(11.0, [0.0, 0.0]), &f),
    );
    let e = run(&t.0, &opts(&t.0, &["S1a"])).unwrap_err();
    assert!(format!("{e:#}").contains("shader-re dx11 cbmap"), "{e:#}");
}
