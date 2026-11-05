//! The comparison wrapper checks itself ("Wrapper"): the oracle against itself
//! passes, a corrupted copy fails, every row of the format map gives the oracle the D3D9 values,
//! `data_moves` put registers into an instance attribute and the bone texture, and a
//! hand-written module is matched by CTAB names. The blocks are assembled in `common/mod.rs`,
//! the hand-written modules are the WGSL strings below: nothing comes from the game.

mod common;

use common::*;
use shader_re::harness::compare::{CompareOptions, compare_oracle, components_match, error_ratio};
use shader_re::harness::formats::{Formats, OurFormat};
use shader_re::harness::random;
use shader_re::harness::subject::Subject;
use shader_re::harness::{EntryCtx, Gpu, PIXELS, make_scene, run_subject};
use shader_re::oracle;
use shader_re::scan::Stage;
use shader_re::sm3::Shader;
use shader_re::sm3::ctab::{ParamClass, ParamType};
use shader_re::sm3::ir::{Cmp, Opcode, RegType::*, SamplerKind, SrcMod, Usage};
use shader_re::status::DataMove;

fn real_formats() -> Formats {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("maps/formats.toml");
    Formats::load(&path).expect("maps/formats.toml parses")
}

fn compare(
    gpu: &Gpu,
    o: &oracle::Oracle,
    s: &Subject,
    formats: &Formats,
    moves: &[DataMove],
) -> shader_re::harness::compare::Report {
    let mut opts = CompareOptions::new(formats);
    opts.data_moves = moves;
    compare_oracle(gpu, o, s, &opts).unwrap_or_else(|e| panic!("{e}"))
}

// ---- the metric ---------------------------------------------------------------------------

#[test]
fn the_metric_of_sr_d10() {
    // Relative above 1, absolute below.
    assert!(components_match(100.0, 100.005));
    assert!(!components_match(100.0, 100.02));
    assert!(components_match(0.0, 0.00009));
    assert!(!components_match(0.0, 0.00011));
    assert!(components_match(-0.00005, 0.00005));
    // NaN matches NaN only, an infinity its own sign only.
    assert!(components_match(f32::NAN, f32::NAN));
    assert!(!components_match(f32::NAN, 1.0));
    assert!(!components_match(1.0, f32::NAN));
    assert!(components_match(f32::INFINITY, f32::INFINITY));
    assert!(!components_match(f32::INFINITY, f32::NEG_INFINITY));
    assert!(!components_match(f32::INFINITY, 1e30));
    assert_eq!(error_ratio(1.0, 1.0), 0.0);
}

// ---- the oracle against itself, and a corrupted copy ----------------------------------------

/// A pixel shader with the whole range: two textures (2D and cube), a matrix, a branch, a loop
/// with a break, derivatives, a kill, two outputs and depth.
fn rich_ps() -> Shader {
    let mut p = Prog::new(PS_3_0);
    p.ctab(vec![
        float4("g_A", 0),
        float4("g_B", 1),
        float4x4("g_M", 2, ParamClass::MatrixColumns),
        sampler("BaseTex", 0, ParamType::Sampler2d),
        sampler("EnvCube", 1, ParamType::SamplerCube),
    ]);
    p.dcl(Usage::TexCoord, 0, d(Input, 0));
    p.dcl(Usage::TexCoord, 1, d(Input, 1));
    p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 0, 0x3)]);
    p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 1, 0x1)]);
    p.dcl_sampler(SamplerKind::Tex2d, 0);
    p.dcl_sampler(SamplerKind::Cube, 1);
    let r = |n| s(Temp, n, XYZW);
    let c = |n| s(Const, n, XYZW);
    p.inst(
        Opcode::TexLd,
        0,
        &[d(Temp, 0), s(Input, 0, XYZW), s(Sampler, 0, XYZW)],
    );
    p.inst(
        Opcode::TexLd,
        0,
        &[d(Temp, 1), s(Input, 1, XYZW), s(Sampler, 1, XYZW)],
    );
    p.inst(Opcode::Mad, 0, &[d(Temp, 2), r(0), c(0), r(1)]);
    for j in 0..4u8 {
        p.inst(
            Opcode::Dp4,
            0,
            &[dm(Temp, 3, 1 << j), r(2), c(2 + j as u32)],
        );
    }
    p.inst(Opcode::Rcp, 0, &[dm(Temp, 4, 1), s(Input, 0, X)]);
    p.inst(
        Opcode::Rsq,
        0,
        &[dm(Temp, 4, 2), sm(Input, 1, Y, SrcMod::Abs)],
    );
    p.inst(Opcode::Log, 0, &[dm(Temp, 4, 4), s(Input, 0, Y)]);
    p.inst(
        Opcode::Pow,
        0,
        &[dm(Temp, 4, 8), s(Input, 0, Z), s(Input, 1, Z)],
    );
    p.inst(Opcode::Nrm, 0, &[dm(Temp, 5, 7), s(Input, 1, XYZW)]);
    p.inst(
        Opcode::Cmp,
        0,
        &[
            dm(Temp, 5, 8),
            s(Input, 0, W),
            s(Const, 1, X),
            s(Const, 1, Y),
        ],
    );
    // if / else
    p.inst(
        Opcode::Ifc,
        Cmp::Gt.code(),
        &[s(Input, 0, X), s(Const, 1, Z)],
    );
    p.inst(Opcode::Mul, 0, &[d(Temp, 6), r(3), r(4)]);
    p.inst(Opcode::Else, 0, &[]);
    p.inst(Opcode::Add, 0, &[d(Temp, 6), r(3), r(5)]);
    p.inst(Opcode::EndIf, 0, &[]);
    // rep with a break: i0 random in 0..8
    p.inst(Opcode::Mov, 0, &[d(Temp, 7), s(Const, 1, XYZW)]);
    p.inst(Opcode::Rep, 0, &[s(ConstInt, 0, XYZW)]);
    p.inst(Opcode::Add, 0, &[d(Temp, 7), r(7), r(6)]);
    p.inst(
        Opcode::BreakC,
        Cmp::Ge.code(),
        &[s(Temp, 7, X), s(Input, 0, W)],
    );
    p.inst(Opcode::EndRep, 0, &[]);
    // derivatives, vPos, vFace
    p.inst(Opcode::Dsx, 0, &[d(Temp, 8), r(0)]);
    p.inst(Opcode::Dsy, 0, &[d(Temp, 9), r(1)]);
    p.inst(
        Opcode::Mad,
        0,
        &[d(Temp, 10), r(8), r(9), s(MiscType, 0, XYZW)],
    );
    p.inst(Opcode::Mul, 0, &[d(Temp, 11), r(10), s(MiscType, 1, X)]);
    p.inst(Opcode::Mov, 0, &[dm(Temp, 12, 0b0111), s(Input, 1, XYZW)]);
    p.inst(Opcode::TexKill, 0, &[dm(Temp, 12, 0b0111)]);
    p.inst(
        Opcode::Add,
        0,
        &[dmod(ColorOut, 0, 0xF, true, 0), r(7), r(11)],
    );
    p.inst(Opcode::Mul, 0, &[dm(ColorOut, 1, 0xF), r(2), r(6)]);
    p.inst(Opcode::Mov, 0, &[dm(DepthOut, 0, 1), s(Temp, 7, X)]);
    p.shader()
}

#[test]
fn oracle_against_itself_passes() {
    let Some(gpu) = gpu("oracle_against_itself_passes") else {
        return;
    };
    let o = translate(&rich_ps());
    let s = Subject::from_oracle(&o).unwrap();
    let r = compare(&gpu, &o, &s, &Formats::default(), &[]);
    assert!(r.pass(), "{}", r.summary());
    assert_eq!(r.runs, 6, "three seeds, linear and point");
    // Several outputs, the kill flag and the depth took part.
    assert!(r.values > 3 * 6 * PIXELS, "{}", r.values);
}

#[test]
fn vertex_oracle_against_itself_passes() {
    let Some(gpu) = gpu("vertex_oracle_against_itself_passes") else {
        return;
    };
    let mut p = Prog::new(VS_3_0);
    p.ctab(vec![
        float4("g_A", 0),
        sampler("Height", 0, ParamType::Sampler2d),
    ]);
    p.dcl(Usage::Position, 0, d(Input, 0));
    p.dcl(Usage::TexCoord, 1, d(Input, 1));
    p.dcl(Usage::Position, 0, d(Output, 0));
    p.dcl(Usage::TexCoord, 1, d(Output, 1));
    p.dcl_sampler(SamplerKind::Tex2d, 0);
    p.inst(Opcode::Mova, 0, &[dm(Addr, 0, 1), s(Input, 1, X)]);
    p.inst(Opcode::Mov, 0, &[d(Temp, 1), s(Const, 15, XYZW)]);
    let [a, b] = srel(2, XYZW, 0);
    p.inst(Opcode::Mul, 0, &[d(Output, 0), s(Input, 0, XYZW), a, b]);
    p.inst(
        Opcode::TexLdl,
        0,
        &[d(Temp, 0), s(Input, 1, XYZW), s(Sampler, 0, XYZW)],
    );
    p.inst(
        Opcode::Add,
        0,
        &[d(Output, 1), s(Temp, 0, XYZW), s(Const, 0, XYZW)],
    );
    let o = translate(&p.shader());
    let s = Subject::from_oracle(&o).unwrap();
    let r = compare(&gpu, &o, &s, &Formats::default(), &[]);
    assert!(r.pass(), "{}", r.summary());
}

/// A pixel shader whose one `mad` reaches `oC0`.
fn mad_ps() -> Shader {
    ps(|p| {
        p.dcl(Usage::TexCoord, 0, d(Input, 0));
        p.dcl(Usage::TexCoord, 1, d(Input, 1));
        p.inst(
            Opcode::Mad,
            0,
            &[
                dm(ColorOut, 0, 0xF),
                s(Input, 0, XYZW),
                s(Const, 0, XYZW),
                s(Input, 1, XYZW),
            ],
        );
    })
}

#[test]
fn a_corrupted_copy_fails() {
    let Some(gpu) = gpu("a_corrupted_copy_fails") else {
        return;
    };
    let o = translate(&mad_ps());
    let good = Subject::from_oracle(&o).unwrap();
    let r = compare(&gpu, &o, &good, &Formats::default(), &[]);
    assert!(r.pass(), "{}", r.summary());

    // One sign changed in one line: `a * b + c` becomes `a * b - c`.
    let mut bad = good.clone();
    let body = bad.wgsl.find("fn oracle_body").unwrap();
    let plus = bad.wgsl[body..].find(" + ").unwrap() + body;
    bad.wgsl.replace_range(plus..plus + 3, " - ");
    let r = compare(&gpu, &o, &bad, &Formats::default(), &[]);
    assert!(!r.pass(), "the corrupted copy must fail");
    let w = r.worst.as_ref().expect("the worst pixel is reported");
    assert_eq!(w.output, "oc0");
    assert!(w.ratio > 1.0);
    assert!(w.inputs.iter().any(|(n, _)| n == "v0"), "{}", r.summary());
    assert!(r.summary().contains("FAIL"));

    // A change nobody can see (a dead local) does not fail.
    let mut same = good.clone();
    let end = same.wgsl.find("var _res: OracleOut;").unwrap();
    same.wgsl.insert_str(end, "let _dead = 1.0;\n    ");
    assert!(compare(&gpu, &o, &same, &Formats::default(), &[]).pass());
}

// ---- format map ---------------------------------------------------------------------------

fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1F) as i32;
    let man = (h & 0x3FF) as f32;
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 => sign * f32::INFINITY,
        e => sign * (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Texel `(x, y)` of level 0 as the 4 numbers our format holds.
fn texel(t: &shader_re::harness::formats::TexData, x: usize, y: usize) -> [f32; 4] {
    let e = t.size as usize;
    let at = (y * e + x) * t.format.bytes_per_texel();
    let m = &t.mips[0];
    match t.format {
        OurFormat::Rgba8Unorm => {
            let b = |k: usize| f32::from(m[at + k]) / 255.0;
            [b(0), b(1), b(2), b(3)]
        }
        OurFormat::R8Unorm => [f32::from(m[at]) / 255.0, 0.0, 0.0, 1.0],
        OurFormat::Rgba16Float => {
            let h = |k: usize| f16_to_f32(u16::from_le_bytes([m[at + 2 * k], m[at + 2 * k + 1]]));
            [h(0), h(1), h(2), h(3)]
        }
    }
}

#[test]
fn every_format_row_gives_the_oracle_the_d3d9_values() {
    let Some(gpu) = gpu("every_format_row") else {
        return;
    };
    // texldl at level 0, point sampling: the texel is known, so is what D3D9 would have seen.
    let sh = {
        let mut p = Prog::new(PS_3_0);
        p.ctab(vec![sampler("Tex", 0, ParamType::Sampler2d)]);
        p.def(0, [0.0; 4]);
        p.dcl(Usage::TexCoord, 0, d(Input, 0));
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Input, 0, XYZW)]);
        p.inst(Opcode::Mov, 0, &[dm(Temp, 0, 0b1100), s(Const, 0, XYZW)]);
        p.inst(
            Opcode::TexLdl,
            0,
            &[d(Temp, 1), s(Temp, 0, XYZW), s(Sampler, 0, XYZW)],
        );
        p.inst(Opcode::Mov, 0, &[d(ColorOut, 0), s(Temp, 1, XYZW)]);
        p.shader()
    };
    let base = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("maps/formats.toml"),
    )
    .unwrap();
    let rows = real_formats().defs.keys().cloned().collect::<Vec<_>>();
    assert_eq!(rows, ["A16B16G16R16", "A8", "L8", "LEAN_CROSS", "R5G6B5"]);
    for row in rows {
        let text =
            format!("{base}\n[[sampler]]\nentry = \"*\"\nname = \"Tex\"\nformat = \"{row}\"\n");
        let formats = Formats::parse(&text).unwrap();
        let o = shader_re::harness::compare::oracle_for(&sh, "T::E:PS", SHA, &formats).unwrap();
        let ctx = EntryCtx {
            name: "T::E:PS",
            ctab: o.ctab.as_ref(),
            samplers: &o.samplers,
        };
        let scene = make_scene(&ctx, 1, 5, false, &formats, &[], &[]).unwrap();
        let out = run_subject(&gpu, &Subject::from_oracle(&o).unwrap(), &ctx, &scene).unwrap();
        let got = out.get("oc0").unwrap();
        let t = &scene.textures["Tex"];
        let uv = random::input_slot(5, "v0", PIXELS);
        let mut checked = 0;
        for p in 0..PIXELS {
            let (u, v) = (uv[p][0], uv[p][1]);
            let (fu, fv) = ((u - u.floor()) * 32.0, (v - v.floor()) * 32.0);
            // Texel borders are a coin toss between GPUs.
            if (fu - fu.round()).abs() < 1e-2 || (fv - fv.round()).abs() < 1e-2 {
                continue;
            }
            let [r, g, b, a] = texel(t, fu as usize, fv as usize);
            let want = match row.as_str() {
                // (r, r, r, 1), (0, 0, 0, r), (r, g, b, 1), the value quantised to 16 bits.
                "L8" => [r, r, r, 1.0],
                "A8" => [0.0, 0.0, 0.0, r],
                "R5G6B5" => [r, g, b, 1.0],
                "A16B16G16R16" => [r, g, b, a].map(|x| (x * 65535.0).round() / 65535.0),
                // The second LEAN map keeps only w, stored in x of our texture: (0, 0, 0, r) at 16 bits.
                "LEAN_CROSS" => [0.0, 0.0, 0.0, (r * 65535.0).round() / 65535.0],
                other => panic!("a new row {other}: add its expected values here"),
            };
            for c in 0..4 {
                assert!(
                    components_match(got[p][c], want[c]),
                    "{row}: pixel {p} component {c}: got {:?}, want {want:?}",
                    got[p]
                );
            }
            checked += 1;
        }
        assert!(checked > PIXELS / 2, "{row}: only {checked} pixels checked");
    }
}

// ---- hand-written modules, matched by CTAB name ---------------------------------------------

fn tinted_ps() -> Shader {
    let mut p = Prog::new(PS_3_0);
    p.ctab(vec![
        float4("g_Color", 0),
        float1("g_Scale", 1),
        sampler("Base", 0, ParamType::Sampler2d),
    ]);
    p.dcl(Usage::TexCoord, 0, d(Input, 0));
    p.dcl_sampler(SamplerKind::Tex2d, 0);
    p.inst(
        Opcode::TexLd,
        0,
        &[d(Temp, 0), s(Input, 0, XYZW), s(Sampler, 0, XYZW)],
    );
    p.inst(
        Opcode::Mul,
        0,
        &[d(Temp, 0), s(Temp, 0, XYZW), s(Const, 0, XYZW)],
    );
    p.inst(
        Opcode::Mul,
        0,
        &[d(ColorOut, 0), s(Temp, 0, XYZW), s(Const, 1, X)],
    );
    p.shader()
}

const TINTED_PS: &str = r#"
@group(1) @binding(0) var Base: texture_2d<f32>;
@group(1) @binding(1) var Base_smp: sampler;
struct Material { g_Color: vec4<f32>, g_Scale: f32 }
@group(2) @binding(0) var<uniform> material: Material;

struct PsIn { v0: vec2<f32> }
struct PsOut { oc0: vec4<f32> }

fn tinted_ps_body(inp: PsIn) -> PsOut {
    var o: PsOut;
    o.oc0 = textureSample(Base, Base_smp, inp.v0) * material.g_Color * material.g_Scale;
    return o;
}
"#;

#[test]
fn a_hand_written_pixel_shader_matches_by_ctab_names() {
    let Some(gpu) = gpu("hand_written_pixel_shader") else {
        return;
    };
    let o = translate(&tinted_ps());
    let hand = Subject::hand("tinted", TINTED_PS, Stage::Ps, "tinted_ps_body").unwrap();
    let r = compare(&gpu, &o, &hand, &Formats::default(), &[]);
    assert!(r.pass(), "{}", r.summary());
    assert!(r.notes.is_empty(), "{}", r.summary());

    // A wrong constant name leaves the member at zero: the output is zero, the comparison fails,
    // and the report says which member nobody filled.
    let wrong = TINTED_PS.replace("g_Scale", "g_Scal");
    let hand = Subject::hand("tinted", &wrong, Stage::Ps, "tinted_ps_body").unwrap();
    let r = compare(&gpu, &o, &hand, &Formats::default(), &[]);
    assert!(!r.pass());
    assert!(
        r.notes.iter().any(|n| n.contains("g_Scal")),
        "{}",
        r.summary()
    );

    // A wrong factor.
    let wrong = TINTED_PS.replace("* material.g_Scale", "* material.g_Scale * 1.01");
    let hand = Subject::hand("tinted", &wrong, Stage::Ps, "tinted_ps_body").unwrap();
    assert!(!compare(&gpu, &o, &hand, &Formats::default(), &[]).pass());

    // `zero_in_compare`: with g_Scale zero on both sides, a wrong factor cannot be seen.
    let none = Formats::default();
    let mut opts = CompareOptions::new(&none);
    let zero = ["g_Scale".to_string()];
    opts.zero_in_compare = &zero;
    let r = compare_oracle(&gpu, &o, &hand, &opts).unwrap();
    assert!(r.pass(), "{}", r.summary());
}

#[test]
fn the_format_map_changes_what_the_oracle_sees() {
    let Some(gpu) = gpu("format_map_in_compare") else {
        return;
    };
    let sh = {
        let mut p = Prog::new(PS_3_0);
        p.ctab(vec![sampler("Base", 0, ParamType::Sampler2d)]);
        p.dcl(Usage::TexCoord, 0, d(Input, 0));
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        p.inst(
            Opcode::TexLd,
            0,
            &[d(ColorOut, 0), s(Input, 0, XYZW), s(Sampler, 0, XYZW)],
        );
        p.shader()
    };
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("maps/formats.toml"),
    )
    .unwrap()
        + "\n[[sampler]]\nentry = \"*\"\nname = \"Base\"\nformat = \"A8\"\n";
    let formats = Formats::parse(&text).unwrap();
    let o = shader_re::harness::compare::oracle_for(&sh, "T::E:PS", SHA, &formats).unwrap();
    let hand = |body: &str| {
        let src = format!(
            "@group(1) @binding(0) var Base: texture_2d<f32>;\n@group(1) @binding(1) var Base_smp: sampler;\n\
             struct PsIn {{ v0: vec2<f32> }}\nstruct PsOut {{ oc0: vec4<f32> }}\n\
             fn a8_body(inp: PsIn) -> PsOut {{ var o: PsOut; let t = textureSample(Base, Base_smp, inp.v0); o.oc0 = {body}; return o; }}"
        );
        Subject::hand("a8", &src, Stage::Ps, "a8_body").unwrap()
    };
    // A8: the oracle sees (0, 0, 0, r); our R8Unorm texture gives (r, 0, 0, 1). The hand-written
    // module that moves r into alpha matches, the one that forgets does not.
    let ok = compare_oracle(
        &gpu,
        &o,
        &hand("vec4<f32>(0.0, 0.0, 0.0, t.r)"),
        &CompareOptions::new(&formats),
    )
    .unwrap();
    assert!(ok.pass(), "{}", ok.summary());
    let bad = compare_oracle(&gpu, &o, &hand("t"), &CompareOptions::new(&formats)).unwrap();
    assert!(!bad.pass());
}

// ---- data moves -----------------------------------------------------------------------------

fn world_vs(class: ParamClass) -> Shader {
    let mut p = Prog::new(VS_3_0);
    p.ctab(vec![float4x4("g_mWorld", 0, class), float4("g_Tint", 4)]);
    p.dcl(Usage::Position, 0, d(Input, 0));
    p.dcl(Usage::Position, 0, d(Output, 0));
    p.dcl(Usage::TexCoord, 1, d(Output, 1));
    let c = |n| s(Const, n, XYZW);
    if class == ParamClass::MatrixColumns {
        // o0 = v0 * M with the registers as columns: o0.j = dot(v0, c_j).
        for j in 0..4u8 {
            p.inst(
                Opcode::Dp4,
                0,
                &[dm(Output, 0, 1 << j), s(Input, 0, XYZW), c(j as u32)],
            );
        }
    } else {
        // Registers as rows: o0 = sum_k v0.k * c_k.
        p.inst(Opcode::Mul, 0, &[d(Temp, 0), s(Input, 0, X), c(0)]);
        p.inst(
            Opcode::Mad,
            0,
            &[d(Temp, 0), s(Input, 0, Y), c(1), s(Temp, 0, XYZW)],
        );
        p.inst(
            Opcode::Mad,
            0,
            &[d(Temp, 0), s(Input, 0, Z), c(2), s(Temp, 0, XYZW)],
        );
        p.inst(
            Opcode::Mad,
            0,
            &[d(Output, 0), s(Input, 0, W), c(3), s(Temp, 0, XYZW)],
        );
    }
    p.inst(Opcode::Mov, 0, &[d(Output, 1), c(4)]);
    p.shader()
}

const WORLD_VS: &str = r#"
struct Material { g_Tint: vec4<f32> }
@group(0) @binding(0) var<uniform> material: Material;

struct VsIn { v0: vec4<f32>, instance_world: mat4x4<f32> }
struct VsOut { o0: vec4<f32>, o1: vec4<f32> }

fn world_vs_body(inp: VsIn) -> VsOut {
    var o: VsOut;
    o.o0 = inp.v0 * inp.instance_world;
    o.o1 = material.g_Tint;
    return o;
}
"#;

#[test]
fn data_moves_put_registers_into_an_instance_attribute() {
    let Some(gpu) = gpu("data_moves_instance") else {
        return;
    };
    let moves = [DataMove {
        ctab: "g_mWorld".into(),
        to: "instance:world".into(),
    }];
    for class in [ParamClass::MatrixColumns, ParamClass::MatrixRows] {
        let o = translate(&world_vs(class));
        let hand = Subject::hand("world", WORLD_VS, Stage::Vs, "world_vs_body").unwrap();
        let r = compare(&gpu, &o, &hand, &Formats::default(), &moves);
        assert!(r.pass(), "{}: {}", class.name(), r.summary());

        // Without the move the instance matrix has no source: an error that names it.
        let none = Formats::default();
        let mut opts = CompareOptions::new(&none);
        opts.data_moves = &[];
        let e = compare_oracle(&gpu, &o, &hand, &opts)
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("instance_world") && e.contains("data move"),
            "{e}"
        );
    }
    // A transposed use in the hand-written module is not the same function.
    let o = translate(&world_vs(ParamClass::MatrixColumns));
    let wrong = WORLD_VS.replace("inp.v0 * inp.instance_world", "inp.instance_world * inp.v0");
    let hand = Subject::hand("world", &wrong, Stage::Vs, "world_vs_body").unwrap();
    assert!(!compare(&gpu, &o, &hand, &Formats::default(), &moves).pass());
}

fn bone_vs() -> Shader {
    let mut p = Prog::new(VS_3_0);
    p.ctab(vec![konst(
        "g_Bones",
        shader_re::sm3::ctab::RegisterSet::Float4,
        8,
        6,
        ParamClass::Vector,
        ParamType::Float,
        1,
        4,
        6,
    )]);
    p.dcl(Usage::Position, 0, d(Input, 0));
    p.dcl(Usage::Position, 0, d(Output, 0));
    p.dcl(Usage::TexCoord, 1, d(Output, 1));
    p.def(20, [0.0, 0.0, 0.0, 1.0]);
    for j in 0..3u8 {
        p.inst(
            Opcode::Dp4,
            0,
            &[
                dm(Output, 0, 1 << j),
                s(Input, 0, XYZW),
                s(Const, 8 + j as u32, XYZW),
            ],
        );
    }
    p.inst(Opcode::Mov, 0, &[dm(Output, 0, 0b1000), s(Const, 20, W)]);
    p.inst(Opcode::Mov, 0, &[d(Output, 1), s(Const, 11, XYZW)]);
    p.shader()
}

const BONE_VS: &str = r#"
@group(0) @binding(0) var bone_texture: texture_2d<f32>;

struct VsIn { v0: vec4<f32>, bone_base: u32, bone_blend: f32 }
struct VsOut { o0: vec4<f32>, o1: vec4<f32> }

fn bone_vs_body(inp: VsIn) -> VsOut {
    let row = i32(inp.bone_base);
    let t0 = textureLoad(bone_texture, vec2<i32>(0, row), 0);
    let t1 = textureLoad(bone_texture, vec2<i32>(1, row), 0);
    let t2 = textureLoad(bone_texture, vec2<i32>(2, row), 0);
    let t3 = textureLoad(bone_texture, vec2<i32>(3, row), 0);
    var o: VsOut;
    o.o0 = vec4<f32>(dot(inp.v0, t0), dot(inp.v0, t1), dot(inp.v0, t2), 1.0);
    o.o1 = t3;
    return o;
}
"#;

#[test]
fn data_moves_put_the_palette_into_the_bone_texture() {
    let Some(gpu) = gpu("data_moves_bone_texture") else {
        return;
    };
    let o = translate(&bone_vs());
    let hand = Subject::hand("bones", BONE_VS, Stage::Vs, "bone_vs_body").unwrap();
    let moves = [DataMove {
        ctab: "g_Bones".into(),
        to: "bone_texture".into(),
    }];
    let r = compare(&gpu, &o, &hand, &Formats::default(), &moves);
    assert!(r.pass(), "{}", r.summary());

    // Without the move the texture has no content.
    let none = Formats::default();
    let opts = CompareOptions::new(&none);
    let e = compare_oracle(&gpu, &o, &hand, &opts)
        .unwrap_err()
        .to_string();
    assert!(e.contains("bone_texture"), "{e}");

    // Reading the second bone's row by mistake (texel 3 is register c11, not c8) is caught.
    let wrong = BONE_VS.replace("vec2<i32>(0, row)", "vec2<i32>(3, row)");
    let hand = Subject::hand("bones", &wrong, Stage::Vs, "bone_vs_body").unwrap();
    assert!(!compare(&gpu, &o, &hand, &Formats::default(), &moves).pass());
}

// ---- contract errors ------------------------------------------------------------------------

#[test]
fn contract_violations_are_reported_not_run() {
    // No such function, a body with two arguments, a result that is not a struct.
    let e = Subject::hand("m", "fn f() {}", Stage::Ps, "g")
        .unwrap_err()
        .to_string();
    assert!(e.contains("no function `g`"), "{e}");
    let two = "struct A { v0: vec4<f32> }\nstruct B { oc0: vec4<f32> }\nfn f(a: A, b: A) -> B { var r: B; return r; }";
    assert!(Subject::hand("m", two, Stage::Ps, "f").is_err());
    let plain = "struct A { v0: vec4<f32> }\nfn f(a: A) -> vec4<f32> { return a.v0; }";
    assert!(Subject::hand("m", plain, Stage::Ps, "f").is_err());
    // A syntax error carries the compiler's message.
    let e = Subject::hand("m", "fn f( {", Stage::Ps, "f")
        .unwrap_err()
        .to_string();
    assert!(e.contains("m"), "{e}");
}

#[test]
fn a_module_without_an_output_the_oracle_has_is_an_error() {
    let Some(gpu) = gpu("missing_output") else {
        return;
    };
    let o = translate(&mad_ps());
    let src = "struct PsIn { v0: vec4<f32>, v1: vec4<f32> }\nstruct PsOut { oc7: vec4<f32> }\n\
               fn m_body(inp: PsIn) -> PsOut { var o: PsOut; o.oc7 = inp.v0; return o; }";
    let hand = Subject::hand("m", src, Stage::Ps, "m_body").unwrap();
    let e = compare_oracle(&gpu, &o, &hand, &CompareOptions::new(&Formats::default()))
        .unwrap_err()
        .to_string();
    assert!(e.contains("no output `oc0`"), "{e}");
}
