//! Self-test of the replay: a "capture" made by our own code replays exactly,
//! and a changed shader is caught. Skipped when the machine has no adapter.

use std::collections::BTreeMap;

use golden::replay::compare::compare;
use golden::replay::inputs::{Call, MemBuffers};
use golden::replay::{DrawSpec, IoReg, ReplayGpu, ShaderSide, run_draw};
use shader_re::harness::{Registers, Subject};
use shader_re::scan::Stage;
use shader_re::sm3::Ctab;

const SHADER: &str = r#"
struct VsIn { v0: vec4<f32>, v1: vec4<f32> }
struct VsOut { o0: vec4<f32>, o1: vec4<f32> }
fn vs_body(i: VsIn) -> VsOut {
    var o: VsOut;
    o.o0 = i.v0;
    o.o1 = i.v1;
    return o;
}
struct PsIn { v0: vec4<f32> }
struct PsOut { oc0: vec4<f32> }
fn ps_body(i: PsIn) -> PsOut {
    var o: PsOut;
    o.oc0 = vec4<f32>(i.v0.xyz * 0.5, 1.0);
    return o;
}
fn ps_changed_body(i: PsIn) -> PsOut {
    var o: PsOut;
    o.oc0 = vec4<f32>(i.v0.xyz * 0.5 + vec3<f32>(0.2, 0.0, 0.0), 1.0);
    return o;
}
"#;

fn ctab() -> Ctab {
    Ctab {
        creator: String::new(),
        version: 0,
        flags: 0,
        target: String::new(),
        constants: Vec::new(),
    }
}

fn io(reg: &str, semantic: &str) -> IoReg {
    IoReg {
        reg: reg.into(),
        semantic: semantic.into(),
        index: 0,
    }
}

fn side(
    entry: &str,
    stage: Stage,
    body: &str,
    inputs: Vec<IoReg>,
    outputs: Vec<IoReg>,
) -> ShaderSide {
    ShaderSide {
        entry: entry.into(),
        subject: Subject::hand(entry, SHADER, stage, body).expect("subject"),
        inputs,
        outputs,
        ctab: ctab(),
        samplers: Vec::new(),
        regs: Registers {
            c: vec![[0.0; 4]; 64],
            i: Default::default(),
            b: 0,
        },
        moves: Vec::new(),
    }
}

fn call() -> Call {
    serde_json::from_value(serde_json::json!({
        "eid": 1,
        "topology": "TriangleList",
        "vertex_count": 3,
        "instances": 1,
        "input_layout": [
            {"semanticName": "POSITION", "format": "R32G32B32A32_FLOAT", "byteOffset": 0},
            {"semanticName": "COLOR", "format": "R32G32B32A32_FLOAT", "byteOffset": 16}
        ],
        "vertex_buffers": [{"slot": 0, "resource": "vb", "stride": 32}],
        "rasterizer": {"cullMode": "None"},
        "viewports": [{"enabled": true, "x": 0.0, "y": 0.0, "width": 32.0, "height": 32.0,
                       "minDepth": 0.0, "maxDepth": 1.0}],
        "targets": [{"resource": "rt", "view_format": "R8G8B8A8_UNORM", "format": "R8G8B8A8_UNORM",
                     "width": 32, "height": 32, "samples": 1}]
    }))
    .expect("call")
}

fn vertices() -> MemBuffers {
    let v: [[f32; 8]; 3] = [
        [-1.0, -1.0, 0.5, 1.0, 1.0, 0.0, 0.0, 1.0],
        [3.0, -1.0, 0.5, 1.0, 0.0, 1.0, 0.0, 1.0],
        [-1.0, 3.0, 0.5, 1.0, 0.0, 0.0, 1.0, 1.0],
    ];
    let bytes: Vec<u8> = v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
    MemBuffers(BTreeMap::from([("vb".to_string(), bytes)]))
}

fn replay(
    gpu: &ReplayGpu,
    ps_body: &str,
    call: &Call,
    bufs: &MemBuffers,
) -> golden::replay::DrawResult {
    let vs = side(
        "Test::Vs:VS",
        Stage::Vs,
        "vs_body",
        vec![io("v0", "position"), io("v1", "color")],
        vec![io("o0", "position"), io("o1", "color")],
    );
    let ps = side(
        "Test::Ps:PS",
        Stage::Ps,
        ps_body,
        vec![io("v0", "color")],
        Vec::new(),
    );
    let spec = DrawSpec {
        label: "selftest".into(),
        wgsl: SHADER.into(),
        vs,
        ps,
        call,
        buffers: bufs,
        textures: BTreeMap::new(),
        before: vec![0u8; 32 * 32 * 4],
        before_depth: None,
    };
    run_draw(gpu, &spec).expect("replay")
}

#[test]
fn own_snapshot_replays_exactly_and_a_changed_shader_fails() {
    let gpu = match ReplayGpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipped: no adapter ({e})");
            return;
        }
    };
    let call = call();
    let bufs = vertices();
    // The "capture": our own renderer's output, quantised to the 8-bit target.
    let snapshot = replay(&gpu, "ps_body", &call, &bufs);
    assert!(
        snapshot.cover.iter().all(|&c| c),
        "the triangle covers the target"
    );
    let again = replay(&gpu, "ps_body", &call, &bufs);
    let (s, _) = compare(&again.ours, &snapshot.ours, &again.cover, 32, 4, true);
    assert!(
        s.passed && s.max_abs == 0.0 && s.compared == 32 * 32,
        "{s:?}"
    );

    let changed = replay(&gpu, "ps_changed_body", &call, &bufs);
    let (s, map) = compare(&changed.ours, &snapshot.ours, &changed.cover, 32, 4, true);
    assert!(!s.passed, "{s:?}");
    assert!(s.failing > 0 && map.iter().any(|&r| r > 1.0));
}
