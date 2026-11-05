//! `golden parse` and `check-captures` on synthetic captures: capture-equivalent JSON written here,
//! read by `rdc/extract.py` (RenderDoc is not needed, and no game file is read).

use std::path::{Path, PathBuf};
use std::process::Command;

use golden::captures::{
    self, CaptureFacts, DrawsFile, Meta, ParseOpts, RenderdocConfig, Source, check_captures,
};
use golden::image::read_dds_file;
use serde_json::{Value, json};

/// sha1 of the bytes below, known from the SHA-1 test vectors.
const SHA_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
const SHA_EMPTY: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
const LONG: &str = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
const SHA_LONG: &str = "84983e441c3bd26ebaae4aa1f95129e5e54670f1";

fn python() -> Option<String> {
    let exe = std::env::var("CIV5_TEST_PYTHON").unwrap_or_else(|_| "python".into());
    Command::new(&exe)
        .args([
            "-I",
            "-c",
            "import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| exe)
}

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("golden-test-{tag}-{}", std::process::id()));
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

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn target(w: u32, h: u32) -> Value {
    json!({"resource": format!("rt{w}"), "format": "R8G8B8A8_UNORM", "width": w, "height": h, "samples": 4})
}

fn draw(eid: u32, vs: &str, ps: &str, tw: u32, instances: u32) -> Value {
    json!({
        "eid": eid, "kind": "draw", "name": "DrawIndexed", "vs": vs, "ps": ps, "hs": null, "ds": null,
        "topology": "TriangleList", "index_count": 3, "vertex_count": 0, "instances": instances,
        "input_layout": [{"semantic": "POSITION", "format": "R32G32B32_FLOAT"}],
        "vertex_buffers": [{"slot": 0, "resource": "b1", "offset": 0, "stride": 12}],
        "index_buffer": {"resource": "b2", "offset": 0, "stride": 2},
        "blend": {}, "depth_stencil": {}, "rasterizer": {},
        "viewports": [{"x": 0, "y": 0, "w": tw, "h": 1080}], "scissors": [],
        "targets": [target(tw, 1080)], "depth_target": null,
        "textures": {"ps": [{"slot": 0, "resource": "t1", "format": "BC1_UNORM_SRGB"}]},
        "samplers": {}, "cbuffers": {"vs": [{"slot": 0, "resource": "c1", "floats": [1.0, 2.0, 3.0, 4.0]}]}
    })
}

/// A capture with a world pass and a UI draw. `extra` events come before the UI draw.
fn model(w: u32, h: u32, extra: Vec<Value>, with_images: bool) -> Value {
    let mut events = vec![
        json!({"eid": 1, "kind": "push", "name": "World"}),
        draw(2, "vs", "ps_world", w, 1),
        draw(3, "vs", "ps_world", w, 5),
    ];
    events.extend(extra);
    events.push(json!({"eid": 20, "kind": "pop"}));
    events.push(draw(21, "vs", "ps_ui", w, 1));
    let mut m = json!({
        "schema": "civ5-golden-model/1",
        "meta": {"renderdoc": "1.0-synthetic", "api": "D3D11", "gpu": "none", "driver": "none",
                 "width": w, "height": h, "swapchain_format": "R8G8B8A8_UNORM", "msaa": 4},
        "shaders": {"vs": hex("abc"), "ps_world": hex("abc"), "ps_ui": hex(LONG), "ps_overlay": ""},
        "textures": {"t1": {"name": "tex", "format": "BC1_UNORM_SRGB", "width": 4, "height": 4,
                            "mips": 1, "layers": 1, "data": [b64(&[1, 2, 3, 4, 5, 6, 7, 8])]}},
        "buffers": {"b1": {"name": "vb", "data": b64(&[9; 36])}, "b2": {"name": "ib", "data": b64(&[0, 0, 1, 0, 2, 0])}},
        "events": events,
    });
    if with_images {
        // 2x1 frame, RGBA and BGRA.
        m["images"] = json!({
            "before:21": {"format": "R8G8B8A8_UNORM", "width": 2, "height": 1, "data": b64(&[10, 20, 30, 255, 40, 50, 60, 255])},
            "final": {"format": "B8G8R8A8_UNORM_SRGB", "width": 2, "height": 1, "data": b64(&[30, 20, 10, 255, 60, 50, 40, 255])},
            "target:2:before:0": {"format": "R8G8B8A8_UNORM", "width": 1, "height": 1, "data": b64(&[1, 1, 1, 1])},
            "target:2:after:0": {"format": "R8G8B8A8_UNORM", "width": 1, "height": 1, "data": b64(&[2, 2, 2, 2])},
        });
    }
    m
}

fn write_model(dir: &Path, name: &str, m: &Value) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_string(m).unwrap()).unwrap();
    p
}

fn cfg(python: &str) -> RenderdocConfig {
    RenderdocConfig {
        python: Some(python.into()),
        ..Default::default()
    }
}

fn write_index(cache: &Path) {
    let dir = cache.join("shader-re").join("abcdef012345").join("dx11");
    std::fs::create_dir_all(&dir).unwrap();
    let twins = json!({"groups": 3, "twins": [
        {"entry": "TerrainShader::P:PS", "blocks": {"5_0": SHA_ABC}},
        {"entry": "OverlayShaders::Ring:PS", "blocks": {"5_0": SHA_EMPTY, "4_0": "f".repeat(40)}},
        {"entry": "UIShader::Text:PS", "blocks": {"5_0": SHA_LONG}},
    ], "candidates": [], "unmatched_groups": []});
    std::fs::write(dir.join("twins.json"), twins.to_string()).unwrap();
}

#[test]
fn parse_writes_draws_events_textures_and_images() {
    let Some(py) = python() else {
        eprintln!("skipped: no Python 3.8+ (set CIV5_TEST_PYTHON)");
        return;
    };
    let t = Tmp::new("parse");
    write_index(&t.0);
    let m = write_model(&t.0, "m.json", &model(1920, 1080, vec![], true));
    let opts = ParseOpts {
        targets: Some("2".into()),
    };
    let meta =
        captures::parse_capture(&cfg(&py), &t.0, "S1", "S1a", Source::Model(&m), &opts).unwrap();
    assert_eq!((meta.width, meta.height, meta.msaa), (1920, 1080, Some(4)));
    assert_eq!(
        (meta.draw_count, meta.world_draw_count, meta.pass_count),
        (3, 2, 1)
    );
    assert!(meta.has_markers && meta.shader_index_used);
    let ui = meta.ui_boundary.unwrap();
    assert_eq!((ui.event, ui.reason.as_str()), (21, "shader"));

    let dir = captures::parsed_dir(&t.0, "S1", "S1a");
    let d = DrawsFile::load(&dir).unwrap();
    assert_eq!(d.schema, captures::DRAWS_SCHEMA);
    assert_eq!(d.draws.len(), 3);
    assert_eq!(d.draws[0].vs.as_deref(), Some(SHA_ABC));
    assert_eq!(d.draws[0].ps.as_deref(), Some(SHA_ABC));
    assert_eq!(d.draws[2].ps.as_deref(), Some(SHA_LONG));
    assert_eq!(d.draws[2].ps_entry.as_deref(), Some("UIShader::Text:PS"));
    assert!(d.draws[0].world && d.draws[1].world && !d.draws[2].world);
    assert_eq!(d.draws[1].instances, 5);
    assert_eq!(d.by_entry["UIShader::Text:PS"], [21]);
    // The UI draw shares the vertex shader (the same sha1), so it is listed for that entry too.
    assert_eq!(d.by_entry["TerrainShader::P:PS"], [2, 3, 21]);

    // events.json: marker tree, one pass for the three draws with the same target size.
    let ev: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("events.json")).unwrap()).unwrap();
    assert_eq!(ev["markers"][0]["name"], "World");
    assert_eq!(ev["markers"][0]["first_event"], 1);
    assert_eq!(ev["markers"][0]["last_event"], 20);
    assert_eq!(ev["passes"].as_array().unwrap().len(), 1);
    assert_eq!(ev["calls"][0]["cbuffers"]["vs"][0]["floats"][2], 3.0);
    assert_eq!(ev["calls"][0]["ps_resource"], "ps_world");

    // World textures and buffers, in their own format.
    let dds = std::fs::read(dir.join("textures").join("t1.dds")).unwrap();
    let parsed = ddsfile::Dds::read(&dds[..]).unwrap();
    assert_eq!(
        parsed.get_dxgi_format(),
        Some(ddsfile::DxgiFormat::BC1_UNorm_sRGB)
    );
    assert_eq!((parsed.get_width(), parsed.get_height()), (4, 4));
    assert_eq!(parsed.data, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        std::fs::read(dir.join("buffers").join("b1.bin")).unwrap(),
        [9; 36]
    );

    // world.dds is the frame before the UI boundary; final.dds the end of the frame (BGRA -> RGBA).
    let w = read_dds_file(&dir.join("world.dds")).unwrap();
    assert_eq!(w.rgba, [10, 20, 30, 255, 40, 50, 60, 255]);
    let f = read_dds_file(&dir.join("final.dds")).unwrap();
    assert_eq!((f.width, f.height), (2, 1));
    assert_eq!(f.rgba, [10, 20, 30, 255, 40, 50, 60, 255]);
    // Targets of the chosen call.
    assert_eq!(
        read_dds_file(&dir.join("targets").join("2-before.dds"))
            .unwrap()
            .rgba,
        [1, 1, 1, 1]
    );
    assert_eq!(
        read_dds_file(&dir.join("targets").join("2-after.dds"))
            .unwrap()
            .rgba,
        [2, 2, 2, 2]
    );
    assert_eq!(meta_from_disk(&dir).targets_events, [2]);
}

fn meta_from_disk(dir: &Path) -> TargetsMeta {
    serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap()
}

#[derive(serde::Deserialize)]
struct TargetsMeta {
    targets_events: Vec<u32>,
}

#[test]
fn ui_boundary_by_marker_without_an_index() {
    let Some(py) = python() else { return };
    let t = Tmp::new("marker");
    let extra = vec![
        json!({"eid": 10, "kind": "push", "name": "Game UI"}),
        draw(11, "vs", "ps_world", 1920, 1),
        json!({"eid": 12, "kind": "pop"}),
    ];
    let m = write_model(&t.0, "m.json", &model(1920, 1080, extra, false));
    let meta = captures::parse_capture(
        &cfg(&py),
        &t.0,
        "S1",
        "S1a",
        Source::Model(&m),
        &ParseOpts::default(),
    )
    .unwrap();
    let ui = meta.ui_boundary.unwrap();
    assert_eq!(
        (ui.event, ui.reason.as_str(), ui.detail.as_str()),
        (10, "marker", "Game UI")
    );
    assert!(!meta.shader_index_used);
    assert_eq!(meta.world_draw_count, 2);
}

#[test]
fn bad_input_names_the_cause() {
    let Some(py) = python() else { return };
    let t = Tmp::new("bad");
    let mut m = model(1920, 1080, vec![], false);
    m["textures"]["t1"]["format"] = json!("ASTC_4x4");
    let p = write_model(&t.0, "m.json", &m);
    let e = captures::parse_capture(
        &cfg(&py),
        &t.0,
        "S1",
        "S1a",
        Source::Model(&p),
        &ParseOpts::default(),
    )
    .unwrap_err();
    assert!(format!("{e:#}").contains("ASTC_4x4"), "{e:#}");
    let e = captures::parse_capture(
        &cfg(&py),
        &t.0,
        "S1",
        "S1a",
        Source::Rdc(&t.0.join("nope.rdc")),
        &ParseOpts::default(),
    )
    .unwrap_err();
    assert!(format!("{e:#}").contains("renderdoc"), "{e:#}");
}

fn scenes_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes")
}

/// Lays out `<cache>/captures` and the parse of every capture of the four scenes.
fn lay_out_all(py: &str, cache: &Path, skip_rdc: &[&str], s4_path_without_overlay: bool) {
    write_index(cache);
    let all: [(&str, &[&str]); 4] = [
        ("S1", &["S1a", "S1b"]),
        ("S2", &["S2-bake", "S2"]),
        ("S3", &["S3"]),
        ("S4", &["S4-select", "S4-path"]),
    ];
    for (scene, names) in all {
        for name in names {
            let mut extra = Vec::new();
            if *name == "S2-bake" {
                extra.push(draw(5, "vs", "ps_world", 512, 1));
            }
            if scene == "S4" && !(s4_path_without_overlay && *name == "S4-path") {
                extra.push(draw(6, "vs", "ps_overlay", 1920, 1));
            }
            let m = write_model(cache, "model.json", &model(1920, 1080, extra, false));
            captures::parse_capture(
                &cfg(py),
                cache,
                scene,
                name,
                Source::Model(&m),
                &ParseOpts::default(),
            )
            .unwrap();
            if !skip_rdc.contains(name) {
                let rdc = cache.join("captures").join(scene);
                std::fs::create_dir_all(&rdc).unwrap();
                std::fs::write(rdc.join(format!("{name}.rdc")), b"synthetic").unwrap();
            }
        }
    }
}

#[test]
fn check_captures_passes_on_a_good_set() {
    let Some(py) = python() else { return };
    let t = Tmp::new("check-ok");
    lay_out_all(&py, &t.0, &[], false);
    let r = check_captures(&t.0, &scenes_dir()).unwrap();
    assert!(r.passed(), "{:?}", r.problems);
    assert_eq!(r.ok.len(), 7);
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("S4-attack") && n.contains("optional"))
    );
}

#[test]
fn check_captures_without_s1b_skips_the_variance_step() {
    let Some(py) = python() else { return };
    let t = Tmp::new("check-no-s1b");
    lay_out_all(&py, &t.0, &["S1b"], false);
    let r = check_captures(&t.0, &scenes_dir()).unwrap();
    assert!(r.passed(), "{:?}", r.problems);
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("S1b") && n.contains("variance") && n.contains("skipped")),
        "{:?}",
        r.notes
    );
}

#[test]
fn check_captures_names_the_scene_to_retake() {
    let Some(py) = python() else { return };
    let t = Tmp::new("check-bad");
    lay_out_all(&py, &t.0, &["S3"], true);
    let r = check_captures(&t.0, &scenes_dir()).unwrap();
    assert!(!r.passed());
    let by = |c: &str| {
        r.problems
            .iter()
            .find(|(n, _)| n == c)
            .map(|(_, p)| p.clone())
    };
    assert!(
        by("S3").unwrap().contains("take scene S3 again"),
        "{:?}",
        r.problems
    );
    let s4 = by("S4-path").unwrap();
    assert!(
        s4.contains("highlight") && s4.contains("retake scene S4"),
        "{s4}"
    );
    assert!(by("S1a").is_none() && by("S4-select").is_none());
}

#[test]
fn check_captures_asks_for_a_parse_first() {
    let t = Tmp::new("check-noparse");
    let rdc = t.0.join("captures").join("S1");
    std::fs::create_dir_all(&rdc).unwrap();
    std::fs::write(rdc.join("S1a.rdc"), b"x").unwrap();
    let r = check_captures(&t.0, &scenes_dir()).unwrap();
    let p = r.problems.iter().find(|(n, _)| n == "S1a").unwrap();
    assert!(p.1.contains("golden parse S1a"), "{}", p.1);
}

/// The sample `draws.json` is also read by a test of `shader-re` (the two sides of the schema).
#[test]
fn draws_sample_matches_the_rust_schema() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures");
    let s = std::fs::read_to_string(dir.join("draws.sample.json")).unwrap();
    let tmp = Tmp::new("sample");
    std::fs::write(tmp.0.join("draws.json"), s).unwrap();
    let d = DrawsFile::load(&tmp.0).unwrap();
    assert_eq!(d.draws.len(), 3);
    assert!(d.draws[1].ps.is_none());
    assert_eq!(d.ui_boundary_event, Some(21));
}

#[test]
fn facts_load_from_disk() {
    let Some(py) = python() else { return };
    let t = Tmp::new("facts");
    let m = write_model(&t.0, "m.json", &model(1920, 1080, vec![], false));
    captures::parse_capture(
        &cfg(&py),
        &t.0,
        "S1",
        "S1a",
        Source::Model(&m),
        &ParseOpts::default(),
    )
    .unwrap();
    let dir = captures::parsed_dir(&t.0, "S1", "S1a");
    let f = CaptureFacts::load(&dir).unwrap();
    let meta: Meta = f.meta;
    assert_eq!(meta.capture, "S1a");
}

#[test]
fn facts_helpers_selftest() {
    let Some(py) = python() else { return };
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("rdc")
        .join("selftest_facts.py");
    let out = Command::new(&py).arg("-I").arg(&script).output().unwrap();
    assert!(
        out.status.success(),
        "selftest_facts.py failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
