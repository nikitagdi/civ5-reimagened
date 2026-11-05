//! The oracle (9 "Tests without binaries"): every opcode of the table gives
//! WGSL that naga and the WebGL2 limits accept, and the special cases give the D3D9 values on a
//! native GPU. The blocks are assembled in `common/mod.rs`; nothing here comes from the game.

mod common;

use common::*;
use shader_re::oracle::{self, Chan, FormatView, Options, Quantize};
use shader_re::sm3::ir::{Opcode, RegType::*, SamplerKind, SrcMod, Usage};
use shader_re::validate::validate_wgsl;

const OC0: u32 = 0;

fn out0(mask: u8) -> u32 {
    dm(ColorOut, OC0, mask)
}

fn assert_valid(name: &str, o: &oracle::Oracle) {
    let r = validate_wgsl(name, &o.wgsl);
    assert!(r.ok(), "{r}\n{}", o.wgsl);
}

// ---- every opcode gives valid WGSL ----------------------------------------------------------

/// One small pixel shader per opcode of the table, keyed by its mnemonic.
fn opcode_programs() -> Vec<(&'static str, shader_re::sm3::Shader)> {
    let c = |n| s(Const, n, XYZW);
    let r = |n| s(Temp, n, XYZW);
    let mut v = Vec::new();
    let mut add = |name: &'static str, f: &dyn Fn(&mut Prog)| {
        v.push((name, ps(|p| f(p))));
    };
    let bin = |op: Opcode| {
        move |p: &mut Prog| {
            p.inst(op, 0, &[out0(0xF), c(0), c(1)]);
        }
    };
    let tri = |op: Opcode| {
        move |p: &mut Prog| {
            p.inst(op, 0, &[out0(0xF), c(0), c(1), c(2)]);
        }
    };
    let un = |op: Opcode| {
        move |p: &mut Prog| {
            p.inst(op, 0, &[out0(0xF), c(0)]);
        }
    };
    add("mov", &un(Opcode::Mov));
    add("add", &bin(Opcode::Add));
    add("mul", &bin(Opcode::Mul));
    add("mad", &tri(Opcode::Mad));
    add("min", &bin(Opcode::Min));
    add("max", &bin(Opcode::Max));
    add("abs", &un(Opcode::Abs));
    add("frc", &un(Opcode::Frc));
    add("lrp", &tri(Opcode::Lrp));
    add("cmp", &tri(Opcode::Cmp));
    add("dp3", &bin(Opcode::Dp3));
    add("dp4", &bin(Opcode::Dp4));
    add("dp2add", &tri(Opcode::Dp2Add));
    add("rcp", &un(Opcode::Rcp));
    add("rsq", &un(Opcode::Rsq));
    add("exp", &un(Opcode::Exp));
    add("log", &un(Opcode::Log));
    add("pow", &bin(Opcode::Pow));
    add("nrm", &un(Opcode::Nrm));
    add("sincos", &un(Opcode::SinCos));
    add("dsx", &un(Opcode::Dsx));
    add("dsy", &un(Opcode::Dsy));
    for (name, op, control, kind) in [
        ("texld", Opcode::TexLd, 0u8, SamplerKind::Tex2d),
        ("texldp", Opcode::TexLd, 1, SamplerKind::Tex2d),
        ("texldb", Opcode::TexLd, 2, SamplerKind::Tex2d),
        ("texldl", Opcode::TexLdl, 0, SamplerKind::Tex2d),
        ("texld_cube", Opcode::TexLd, 0, SamplerKind::Cube),
        ("texldl_volume", Opcode::TexLdl, 0, SamplerKind::Volume),
    ] {
        add(name, &move |p: &mut Prog| {
            p.dcl_sampler(kind, 0);
            p.inst(op, control, &[out0(0xF), c(0), s(Sampler, 0, XYZW)]);
        });
    }
    add("texkill", &|p| {
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), c(0)]);
        p.inst(Opcode::TexKill, 0, &[d(Temp, 0)]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), r(0)]);
    });
    add("def_vpos_vface_depth_mrt", &|p| {
        p.def(3, [1.0, 2.0, 3.0, 4.0]);
        p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 0, 0x3)]);
        p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 1, 0x1)]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(MiscType, 0, XYZW)]);
        p.inst(
            Opcode::Mov,
            0,
            &[dm(ColorOut, 1, 0xF), s(MiscType, 1, XYZW)],
        );
        p.inst(Opcode::Mov, 0, &[dm(DepthOut, 0, 0x1), s(Const, 3, X)]);
    });
    v
}

#[test]
fn every_opcode_gives_valid_wgsl() {
    for (name, sh) in opcode_programs() {
        let o = translate(&sh);
        assert_valid(name, &o);
    }
}

#[test]
fn vertex_opcodes_give_valid_wgsl() {
    // mova + relative constant read, texldl (vertex texture fetch).
    let sh = vs(|p| {
        p.dcl(Usage::Position, 0, d(Input, 0));
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        p.inst(Opcode::Mova, 0, &[dm(Addr, 0, 0x3), s(Input, 0, XYZW)]);
        let [a, b] = srel(4, XYZW, 0);
        p.inst(Opcode::Mov, 0, &[d(Output, 0), a, b]);
        p.inst(
            Opcode::TexLdl,
            0,
            &[d(Output, 1), s(Input, 0, XYZW), s(Sampler, 0, XYZW)],
        );
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 20, XYZW)]);
    });
    let o = translate(&sh);
    assert_valid("vs", &o);
}

#[test]
fn modifiers_masks_and_shifts_give_valid_wgsl() {
    let sh = ps(|p| {
        p.inst(
            Opcode::Mad,
            0,
            &[
                dmod(ColorOut, 0, 0b0110, true, 1),
                sm(Const, 0, [3, 2, 1, 0], SrcMod::AbsNeg),
                sm(Const, 1, Z, SrcMod::Neg),
                sm(Const, 2, XYZW, SrcMod::Abs),
            ],
        );
    });
    assert_valid("mods", &translate(&sh));
}

// ---- what the oracle refuses ----------------------------------------------------------------

#[test]
fn unsupported_instructions_name_the_block_and_the_instruction() {
    let sh = ps(|p| {
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Const, 0, XYZW)]);
        p.inst(
            Opcode::Sub,
            0,
            &[d(Temp, 0), s(Const, 0, XYZW), s(Const, 1, XYZW)],
        );
    });
    let e = oracle::translate(&sh, "Pkg::Entry:PS", SHA, &Options::default()).unwrap_err();
    let text = e.to_string();
    assert!(text.contains("Pkg::Entry:PS"), "{text}");
    assert!(text.contains("sub r0, c0, c1"), "{text}");
    assert!(text.contains("instruction 1"), "{text}");

    // texldd, loop/aL, a predicated instruction and texld in a vertex shader are refused too.
    let sh = ps(|p| {
        p.inst(Opcode::Loop, 0, &[s(Loop, 0, XYZW), s(ConstInt, 0, XYZW)]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Const, 0, XYZW)]);
        p.inst(Opcode::EndLoop, 0, &[]);
    });
    let e = oracle::translate(&sh, "Pkg::Loop:PS", SHA, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("loop"), "{e}");
    let sh = vs(|p| {
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        p.inst(
            Opcode::TexLd,
            0,
            &[d(Output, 1), s(Const, 0, XYZW), s(Sampler, 0, XYZW)],
        );
    });
    let e = oracle::translate(&sh, "Pkg::Vtf:VS", SHA, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("vertex shader"), "{e}");
}

#[test]
fn reads_of_undeclared_registers_are_errors() {
    let sh = ps(|p| {
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Input, 2, XYZW)]);
    });
    let e = oracle::translate(&sh, "P::E:PS", SHA, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("v2"), "{e}");
}

// ---- the special cases on the GPU -----------------------------------------------------------

/// One pixel shader, constants in, `oC0` out.
fn eval(
    gpu: &shader_re::harness::Gpu,
    sh: &shader_re::sm3::Shader,
    consts: &[[f32; 4]],
) -> [f32; 4] {
    let o = translate(sh);
    let got = run(
        gpu,
        &o,
        &Run {
            consts: consts.to_vec(),
            size: (1, 1),
            ..Default::default()
        },
        0,
    );
    got[0]
}

fn per_lane(op: Opcode) -> shader_re::sm3::Shader {
    ps(|p| {
        for (k, sw) in [X, Y, Z, W].into_iter().enumerate() {
            p.inst(op, 0, &[out0(1 << k), s(Const, 0, sw)]);
        }
    })
}

#[test]
fn rcp_rsq_log_special_values() {
    let Some(gpu) = gpu("rcp_rsq_log_special_values") else {
        return;
    };
    // rcp(0) = +inf, rcp(-0) = -inf (4.6).
    let got = eval(&gpu, &per_lane(Opcode::Rcp), &[[0.0, -0.0, 4.0, -8.0]]);
    assert_vec(got, [INF, -INF, 0.25, -0.125], "rcp");
    // rsq takes abs(x); rsq(0) = +inf.
    let got = eval(&gpu, &per_lane(Opcode::Rsq), &[[0.0, -4.0, 4.0, 0.25]]);
    assert_vec(got, [INF, 0.5, 0.5, 2.0], "rsq");
    // log is base 2 and takes abs(x); log(0) = -inf.
    let got = eval(&gpu, &per_lane(Opcode::Log), &[[0.0, -8.0, 8.0, 1.0]]);
    assert_vec(got, [-INF, 3.0, 3.0, 0.0], "log");
    // exp is base 2.
    let got = eval(&gpu, &per_lane(Opcode::Exp), &[[0.0, 1.0, 3.0, -1.0]]);
    assert_vec(got, [1.0, 2.0, 8.0, 0.5], "exp");
}

#[test]
fn pow_special_values() {
    let Some(gpu) = gpu("pow_special_values") else {
        return;
    };
    let sh = ps(|p| {
        for (k, sw) in [X, Y, Z, W].into_iter().enumerate() {
            p.inst(
                Opcode::Pow,
                0,
                &[out0(1 << k), s(Const, 0, sw), s(Const, 1, sw)],
            );
        }
    });
    // pow(-2, 2) = 4 (abs(x)); pow(2, 3) = 8; pow(0, 0) = 1 (D3D9: 0 times anything is 0, so
    // `exp2(0 * log(0))` is `exp2(0)`; orchestrator decision of the R1.7/R1.8 review); pow(0, 2) = 0.
    let got = eval(&gpu, &sh, &[[-2.0, 2.0, 0.0, 0.0], [2.0, 3.0, 0.0, 2.0]]);
    assert_vec(got, [4.0, 8.0, 1.0, 0.0], "pow");
}

#[test]
fn nrm_and_cmp_special_values() {
    let Some(gpu) = gpu("nrm_and_cmp_special_values") else {
        return;
    };
    let nrm = ps(|p| {
        p.inst(Opcode::Nrm, 0, &[out0(0xF), s(Const, 0, XYZW)]);
    });
    // nrm(0) is the zero vector: D3D9 says 0 times anything is 0 (so not 0 * inf = NaN).
    let got = eval(&gpu, &nrm, &[[0.0; 4]]);
    assert_vec(got, [0.0; 4], "nrm(0)");
    let got = eval(&gpu, &nrm, &[[3.0, 0.0, 4.0, 10.0]]);
    assert_vec(got, [0.6, 0.0, 0.8, 2.0], "nrm");

    let cmp = ps(|p| {
        p.inst(
            Opcode::Cmp,
            0,
            &[
                out0(0xF),
                s(Const, 0, XYZW),
                s(Const, 1, XYZW),
                s(Const, 2, XYZW),
            ],
        );
    });
    // cmp picks src1 where src0 >= 0 (also at -0 and +0), src2 otherwise; NaN picks src2.
    let got = eval(
        &gpu,
        &cmp,
        &[
            [NAN, -1.0, 0.0, -0.0],
            [1.0, 1.0, 1.0, 1.0],
            [2.0, 2.0, 2.0, 2.0],
        ],
    );
    assert_vec(got, [2.0, 2.0, 1.0, 1.0], "cmp");
}

#[test]
fn arithmetic_modifiers_masks_and_shifts() {
    let Some(gpu) = gpu("arithmetic_modifiers_masks_and_shifts") else {
        return;
    };
    // mad_x2_sat oC0.yz, -|c0|.wzyx... : result = sat(2 * (c0.wzyx_abs_neg * (-c1.z) + |c2|)).
    let sh = ps(|p| {
        p.inst(
            Opcode::Mad,
            0,
            &[
                dmod(ColorOut, 0, 0b0110, true, 1),
                sm(Const, 0, [3, 2, 1, 0], SrcMod::AbsNeg),
                sm(Const, 1, Z, SrcMod::Neg),
                sm(Const, 2, XYZW, SrcMod::Abs),
            ],
        );
    });
    let c0 = [1.0f32, -2.0, 3.0, -4.0];
    let c1 = [0.0f32, 0.0, 0.5, 0.0];
    let c2 = [-0.25f32, 0.125, -0.5, 0.0];
    let got = eval(&gpu, &sh, &[c0, c1, c2]);
    let lane = |l: usize, src: usize| -(c0[src].abs()) * (-c1[2]) + c2[l].abs();
    let want_y = (2.0 * lane(1, 2)).clamp(0.0, 1.0);
    let want_z = (2.0 * lane(2, 1)).clamp(0.0, 1.0);
    assert_vec(got, [0.0, want_y, want_z, 0.0], "mad_x2_sat");

    // Half shift and the dp family.
    let sh = ps(|p| {
        p.inst(
            Opcode::Dp3,
            0,
            &[
                dmod(ColorOut, 0, 0b0001, false, -1),
                s(Const, 0, XYZW),
                s(Const, 1, XYZW),
            ],
        );
        p.inst(
            Opcode::Dp4,
            0,
            &[
                dm(ColorOut, 0, 0b0010),
                s(Const, 0, XYZW),
                s(Const, 1, XYZW),
            ],
        );
        p.inst(
            Opcode::Dp2Add,
            0,
            &[
                dm(ColorOut, 0, 0b0100),
                s(Const, 0, XYZW),
                s(Const, 1, XYZW),
                s(Const, 2, Z),
            ],
        );
        p.inst(
            Opcode::Lrp,
            0,
            &[
                dm(ColorOut, 0, 0b1000),
                s(Const, 0, W),
                s(Const, 1, W),
                s(Const, 2, W),
            ],
        );
    });
    let a = [1.0f32, 2.0, 3.0, 0.25];
    let b = [4.0f32, 5.0, 6.0, 8.0];
    let c = [0.0f32, 0.0, 10.0, 2.0];
    let got = eval(&gpu, &sh, &[a, b, c]);
    let dp3 = 4.0 + 10.0 + 18.0;
    let dp4 = dp3 + 0.25 * 8.0;
    let dp2add = 4.0 + 10.0 + 10.0;
    // lrp(s0, s1, s2) = s0 * (s1 - s2) + s2
    let lrp = 0.25 * (8.0 - 2.0) + 2.0;
    assert_vec(got, [dp3 * 0.5, dp4, dp2add, lrp], "dp family and lrp");

    // frc, min, max, abs, sincos.
    let sh = ps(|p| {
        p.inst(Opcode::Frc, 0, &[dm(ColorOut, 0, 0b0001), s(Const, 0, X)]);
        p.inst(
            Opcode::Min,
            0,
            &[dm(ColorOut, 0, 0b0010), s(Const, 0, Y), s(Const, 1, Y)],
        );
        p.inst(
            Opcode::Max,
            0,
            &[dm(ColorOut, 0, 0b0100), s(Const, 0, Y), s(Const, 1, Y)],
        );
        p.inst(Opcode::Abs, 0, &[dm(ColorOut, 0, 0b1000), s(Const, 0, Z)]);
    });
    let got = eval(&gpu, &sh, &[[-1.25, 2.0, -3.5, 0.0], [0.0, 5.0, 0.0, 0.0]]);
    assert_vec(got, [0.75, 2.0, 5.0, 3.5], "frc min max abs");
    let sh = ps(|p| {
        p.inst(
            Opcode::SinCos,
            0,
            &[dm(ColorOut, 0, 0b0011), s(Const, 0, X)],
        );
    });
    let got = eval(&gpu, &sh, &[[std::f32::consts::FRAC_PI_2, 0.0, 0.0, 0.0]]);
    assert_vec(got, [0.0, 1.0, 0.0, 0.0], "sincos: x = cos, y = sin");
}

#[test]
fn texkill_and_vpos() {
    let Some(gpu) = gpu("texkill_and_vpos") else {
        return;
    };
    let sh = ps(|p| {
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 0, XYZW)]);
        p.inst(Opcode::TexKill, 0, &[dm(Temp, 0, 0b0111)]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Const, 1, XYZW)]);
    });
    // Nothing negative: the pixel is written. A negative x, y or z: it is discarded.
    let live = [[1.0, 1.0, 1.0, 1.0], [5.0, 6.0, 7.0, 8.0]];
    assert_vec(eval(&gpu, &sh, &live), [5.0, 6.0, 7.0, 8.0], "texkill live");
    for bad in [
        [-1.0, 1.0, 1.0, 1.0],
        [1.0, -1.0, 1.0, 1.0],
        [1.0, 1.0, -0.001, 1.0],
    ] {
        let got = eval(&gpu, &sh, &[bad, [5.0, 6.0, 7.0, 8.0]]);
        assert_vec(
            got,
            [7.0; 4],
            "texkill kills (target keeps its clear value)",
        );
    }

    // vPos: pixel position, integer centres (4.6, `dcl_*`).
    let sh = ps(|p| {
        p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 0, 0x3)]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(MiscType, 0, XYZW)]);
    });
    let o = translate(&sh);
    let px = run(
        &gpu,
        &o,
        &Run {
            size: (4, 4),
            ..Default::default()
        },
        0,
    );
    assert_vec(px[4 + 2], [2.0, 1.0, 0.0, 0.0], "vPos at (2, 1)");
    assert_vec(px[12], [0.0, 3.0, 0.0, 0.0], "vPos at (0, 3)");
}

fn gradient_texture() -> TestTex {
    let red = [255, 0, 0, 255];
    let green = [0, 255, 0, 255];
    let blue = [0, 0, 255, 255];
    let white = [255, 255, 255, 255];
    TestTex {
        size: 2,
        mips: vec![vec![red, green, blue, white], vec![[128, 128, 128, 255]]],
    }
}

#[test]
fn texture_loads() {
    let Some(gpu) = gpu("texture_loads") else {
        return;
    };
    let load = |op: Opcode, control: u8, sampler_sw: [u8; 4]| {
        ps(|p| {
            p.dcl_sampler(SamplerKind::Tex2d, 0);
            p.inst(
                op,
                control,
                &[out0(0xF), s(Const, 0, XYZW), s(Sampler, 0, sampler_sw)],
            );
        })
    };
    let eval_tex = |sh: &shader_re::sm3::Shader, coord: [f32; 4]| {
        let o = translate(sh);
        let mut run_spec = Run {
            consts: vec![coord],
            size: (2, 2),
            ..Default::default()
        };
        run_spec.textures.insert(0, gradient_texture());
        run(&gpu, &o, &run_spec, 0)[0]
    };
    let red = [1.0, 0.0, 0.0, 1.0];
    let green = [0.0, 1.0, 0.0, 1.0];
    let gray = [128.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];
    // texld: the xy of the coordinate.
    assert_vec(
        eval_tex(&load(Opcode::TexLd, 0, XYZW), [0.25, 0.25, 0.0, 0.0]),
        red,
        "texld",
    );
    assert_vec(
        eval_tex(&load(Opcode::TexLd, 0, XYZW), [0.75, 0.25, 0.0, 0.0]),
        green,
        "texld",
    );
    // texldp divides by w.
    assert_vec(
        eval_tex(&load(Opcode::TexLd, 1, XYZW), [0.5, 0.5, 0.0, 2.0]),
        red,
        "texldp",
    );
    assert_vec(
        eval_tex(&load(Opcode::TexLd, 1, XYZW), [1.5, 0.5, 0.0, 2.0]),
        green,
        "texldp",
    );
    // texldl: w is the level; texldb: w is the bias (the derivatives are zero).
    assert_vec(
        eval_tex(&load(Opcode::TexLdl, 0, XYZW), [0.25, 0.25, 0.0, 0.0]),
        red,
        "texldl 0",
    );
    assert_vec(
        eval_tex(&load(Opcode::TexLdl, 0, XYZW), [0.25, 0.25, 0.0, 1.0]),
        gray,
        "texldl 1",
    );
    // With zero derivatives the bias has nothing to add to: level 0.
    assert_vec(
        eval_tex(&load(Opcode::TexLd, 2, XYZW), [0.25, 0.25, 0.0, 1.0]),
        red,
        "texldb, flat",
    );
    // The coordinate follows vPos: one texel per pixel, level 0; the bias in w adds a level.
    let sh = ps(|p| {
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 0, 0x3)]);
        p.inst(
            Opcode::Mad,
            0,
            &[
                dm(Temp, 0, 0b0011),
                s(MiscType, 0, XYZW),
                s(Const, 0, X),
                s(Const, 0, Y),
            ],
        );
        p.inst(Opcode::Mov, 0, &[dm(Temp, 0, 0b1100), s(Const, 1, XYZW)]);
        p.inst(
            Opcode::TexLd,
            2,
            &[out0(0xF), s(Temp, 0, XYZW), s(Sampler, 0, XYZW)],
        );
    });
    let o = translate(&sh);
    let mut spec = Run {
        consts: vec![[0.5, 0.25, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]],
        size: (2, 2),
        ..Default::default()
    };
    spec.textures.insert(0, gradient_texture());
    assert_vec(run(&gpu, &o, &spec, 0)[0], red, "texldb, bias 0");
    spec.consts[1] = [0.0, 0.0, 0.0, 1.0];
    assert_vec(run(&gpu, &o, &spec, 0)[0], gray, "texldb, bias 1");
    // The swizzle of the sampler operand picks components of the result: .bgra.
    assert_vec(
        eval_tex(
            &load(Opcode::TexLd, 0, [2, 1, 0, 3]),
            [0.25, 0.25, 0.0, 0.0],
        ),
        [0.0, 0.0, 1.0, 1.0],
        ".bgra",
    );
}

#[test]
fn format_views_show_what_d3d9_saw() {
    let Some(gpu) = gpu("format_views_show_what_d3d9_saw") else {
        return;
    };
    let sh = ps(|p| {
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        p.inst(
            Opcode::TexLd,
            0,
            &[out0(0xF), s(Const, 0, XYZW), s(Sampler, 0, XYZW)],
        );
    });
    let tex = TestTex {
        size: 1,
        mips: vec![vec![[51, 102, 153, 204]]],
    };
    let texel = [51.0 / 255.0, 102.0 / 255.0, 153.0 / 255.0, 204.0 / 255.0];
    let mut results = Vec::new();
    // Without a view the sampler reads the texture as it is.
    for (view, want) in [
        (FormatView::IDENTITY, texel),
        (
            // L8: (r, r, r, 1).
            FormatView {
                chans: [Chan::R, Chan::R, Chan::R, Chan::One],
                quantize: Quantize::None,
            },
            [texel[0], texel[0], texel[0], 1.0],
        ),
        (
            // A8: (0, 0, 0, r).
            FormatView {
                chans: [Chan::Zero, Chan::Zero, Chan::Zero, Chan::R],
                quantize: Quantize::None,
            },
            [0.0, 0.0, 0.0, texel[0]],
        ),
        (
            // 565: (r, g, b, 1).
            FormatView {
                chans: [Chan::R, Chan::G, Chan::B, Chan::One],
                quantize: Quantize::None,
            },
            [texel[0], texel[1], texel[2], 1.0],
        ),
        (
            // 16 bit UNORM: the value rounded to k / 65535.
            FormatView {
                chans: [Chan::R, Chan::G, Chan::B, Chan::A],
                quantize: Quantize::Unorm16,
            },
            texel.map(|x: f32| (x * 65535.0).round() / 65535.0),
        ),
    ] {
        let mut opts = Options::default();
        opts.formats.insert("s0".to_string(), view);
        let o = oracle::translate(&sh, "T::E:PS", SHA, &opts).unwrap();
        assert_valid("fmt", &o);
        let mut spec = Run {
            consts: vec![[0.5, 0.5, 0.0, 0.0]],
            size: (1, 1),
            ..Default::default()
        };
        spec.textures.insert(0, tex.clone());
        let got = run(&gpu, &o, &spec, 0)[0];
        results.push((got, want));
    }
    for (i, (got, want)) in results.into_iter().enumerate() {
        assert_vec(got, want, &format!("view {i}"));
    }
}

#[test]
fn vertex_mova_and_relative_constants() {
    let Some(gpu) = gpu("vertex_mova_and_relative_constants") else {
        return;
    };
    // mova rounds to nearest: 1.5 -> 2, -1.5 -> -1 (4.6). c[a0.x + 4] reads through the index and
    // is clamped into the array.
    let sh = vs(|p| {
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 10, XYZW)]);
        p.inst(Opcode::Mov, 0, &[d(Output, 0), s(Const, 1, XYZW)]);
        p.inst(Opcode::Mova, 0, &[dm(Addr, 0, 0b0011), s(Const, 2, XYZW)]);
        let [a, b] = srel(4, X, 0);
        p.inst(Opcode::Mov, 0, &[dm(Output, 1, 0b0001), a, b]);
        let [a, b] = srel(4, X, 1);
        p.inst(Opcode::Mov, 0, &[dm(Output, 1, 0b0010), a, b]);
        // a0.x + 400 is far past the array: clamped to the last register, c10.
        let [a, b] = srel(400, X, 0);
        p.inst(Opcode::Mov, 0, &[dm(Output, 1, 0b0100), a, b]);
        // a0.y - 100 is below zero: clamped to c0.
        let [a, b] = srel(0, X, 1);
        p.inst(Opcode::Mov, 0, &[dm(Output, 1, 0b1000), a, b]);
    });
    let o = translate(&sh);
    assert_valid("vs mova", &o);
    let mut consts: Vec<[f32; 4]> = (0..11).map(|i| [i as f32; 4]).collect();
    consts[1] = [0.0, 0.0, 0.0, 1.0];
    consts[2] = [1.5, -1.5, 0.0, 0.0];
    let got = run(
        &gpu,
        &o,
        &Run {
            consts,
            ..Default::default()
        },
        1,
    )[0];
    // c[2 + 4] = c6.x = 6; c[-1 + 4] = c3.x = 3; clamp -> c10.x = 10; c[-1 + 0] -> c0.x = 0.
    assert_vec(got, [6.0, 3.0, 10.0, 0.0], "mova / relative constant");
}

#[test]
fn defs_override_and_unsupported_values_are_reported() {
    let Some(gpu) = gpu("defs_override") else {
        return;
    };
    let sh = ps(|p| {
        p.def(0, [1.0, 2.0, 3.0, 4.0]);
        p.inst(
            Opcode::Mul,
            0,
            &[out0(0xF), s(Const, 0, XYZW), s(Const, 1, XYZW)],
        );
    });
    let got = eval(&gpu, &sh, &[[9.0; 4], [2.0, 2.0, 2.0, 2.0]]);
    assert_vec(
        got,
        [2.0, 4.0, 6.0, 8.0],
        "def c0 wins over the constant buffer",
    );

    let sh = ps(|p| {
        p.def(0, [f32::NAN, 0.0, 0.0, 0.0]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Const, 0, XYZW)]);
    });
    let e = oracle::translate(&sh, "T::E:PS", SHA, &Options::default()).unwrap_err();
    assert!(e.to_string().contains("not finite"), "{e}");
}

// ---- flattened branches and loops (R1.7b) -----------------------------------------------------

fn vpos_dcl(p: &mut Prog) {
    p.inst(Opcode::Dcl, 0, &[0x8000_0000, dm(MiscType, 0, 0x3)]);
}

fn run_grid(gpu: &shader_re::harness::Gpu, sh: &shader_re::sm3::Shader, r: &Run) -> Vec<[f32; 4]> {
    let o = translate(sh);
    assert_valid("flow", &o);
    run(gpu, &o, r, 0)
}

#[test]
fn nested_if_else_gives_the_right_branch_per_pixel() {
    let Some(gpu) = gpu("nested_if_else") else {
        return;
    };
    use shader_re::sm3::ir::Cmp;
    // x < 2 (left half) takes the outer `if`; inside, y < 2 picks c2 (top) or c3 (bottom). The
    // right half takes the outer `else` (c4). A texture load sits behind both sides: it has to
    // stay in uniform control flow, and naga checks that.
    let sh = ps(|p| {
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        vpos_dcl(p);
        p.inst(Opcode::Mov, 0, &[d(Temp, 1), s(Const, 5, XYZW)]);
        p.inst(
            Opcode::Ifc,
            Cmp::Lt.code(),
            &[s(MiscType, 0, X), s(Const, 0, X)],
        );
        p.inst(
            Opcode::Ifc,
            Cmp::Lt.code(),
            &[s(MiscType, 0, Y), s(Const, 0, X)],
        );
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 2, XYZW)]);
        p.inst(
            Opcode::TexLd,
            0,
            &[d(Temp, 1), s(Const, 6, XYZW), s(Sampler, 0, XYZW)],
        );
        p.inst(Opcode::Else, 0, &[]);
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 3, XYZW)]);
        p.inst(Opcode::EndIf, 0, &[]);
        p.inst(Opcode::Else, 0, &[]);
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 4, XYZW)]);
        p.inst(Opcode::EndIf, 0, &[]);
        p.inst(
            Opcode::Add,
            0,
            &[out0(0xF), s(Temp, 0, XYZW), s(Temp, 1, XYZW)],
        );
    });
    let mut r = Run {
        consts: vec![
            [2.0; 4],
            [0.0; 4],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            [0.5, 0.5, 0.0, 0.0],
        ],
        size: (4, 4),
        ..Default::default()
    };
    r.textures.insert(0, TestTex::solid([0, 0, 0, 255]));
    let px = run_grid(&gpu, &sh, &r);
    for y in 0..4 {
        for x in 0..4 {
            // The loaded texel (0, 0, 0, 1) is added only where the innermost `if` ran; the other
            // pixels keep c5 = (0, 0, 0, 1) in r1, which has the same w: so w is always 1.
            let base = if x < 2 {
                if y < 2 {
                    [1.0, 0.0, 0.0, 1.0]
                } else {
                    [0.0, 1.0, 0.0, 1.0]
                }
            } else {
                [0.0, 0.0, 1.0, 1.0]
            };
            assert_vec(px[y * 4 + x], base, &format!("pixel ({x}, {y})"));
        }
    }
}

#[test]
fn if_on_a_boolean_register() {
    let Some(gpu) = gpu("if_on_a_boolean_register") else {
        return;
    };
    let sh = ps(|p| {
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 0, XYZW)]);
        p.inst(Opcode::If, 0, &[s(ConstBool, 3, XYZW)]);
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 1, XYZW)]);
        p.inst(Opcode::EndIf, 0, &[]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Temp, 0, XYZW)]);
    });
    let consts = vec![[1.0; 4], [2.0; 4]];
    for (bools, want) in [(0u32, 1.0f32), (1 << 3, 2.0), (1 << 2, 1.0)] {
        let r = Run {
            consts: consts.clone(),
            bools,
            size: (1, 1),
            ..Default::default()
        };
        assert_vec(
            run_grid(&gpu, &sh, &r)[0],
            [want; 4],
            &format!("b3 in {bools:#b}"),
        );
    }
}

#[test]
fn rep_with_break_per_lane_and_inside_an_if() {
    let Some(gpu) = gpu("rep_with_break") else {
        return;
    };
    use shader_re::sm3::ir::Cmp;
    // r1.x = vPos.x + 1. The loop adds 1 to r0.x each round and leaves for the lanes whose r0.x
    // reached r1.x (a `break` inside an `if`); r0.y counts the rounds that got past the `break`.
    let sh = ps(|p| {
        vpos_dcl(p);
        p.inst(
            Opcode::Add,
            0,
            &[dm(Temp, 1, 0b0001), s(MiscType, 0, X), s(Const, 1, X)],
        );
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 0, XYZW)]);
        p.inst(Opcode::Rep, 0, &[s(ConstInt, 0, XYZW)]);
        p.inst(
            Opcode::Add,
            0,
            &[dm(Temp, 0, 0b0001), s(Temp, 0, X), s(Const, 1, X)],
        );
        p.inst(Opcode::Ifc, Cmp::Ge.code(), &[s(Temp, 0, X), s(Temp, 1, X)]);
        p.inst(Opcode::Break, 0, &[]);
        p.inst(Opcode::EndIf, 0, &[]);
        p.inst(
            Opcode::Add,
            0,
            &[dm(Temp, 0, 0b0010), s(Temp, 0, Y), s(Const, 1, X)],
        );
        p.inst(Opcode::EndRep, 0, &[]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Temp, 0, XYZW)]);
    });
    let mut r = Run {
        consts: vec![[0.0; 4], [1.0; 4]],
        ints: vec![[3, 0, 0, 0]],
        size: (6, 1),
        ..Default::default()
    };
    // Three rounds at most: pixel x ends with r0.x = min(x + 1, 3), r0.y = min(x, 3).
    let px = run_grid(&gpu, &sh, &r);
    for (x, got) in px.iter().enumerate() {
        let ax = (x + 1).min(3) as f32;
        let ay = x.min(3) as f32;
        assert_vec(*got, [ax, ay, 0.0, 0.0], &format!("rep 3, pixel {x}"));
    }
    // The count comes from i0.x: 0 rounds leave the registers alone, and the count is clamped
    // to 255 rounds.
    r.ints = vec![[0, 0, 0, 0]];
    assert_vec(run_grid(&gpu, &sh, &r)[3], [0.0; 4], "rep 0");
    r.ints = vec![[-5, 0, 0, 0]];
    assert_vec(run_grid(&gpu, &sh, &r)[3], [0.0; 4], "rep -5");
}

#[test]
fn breakc_nested_loops_and_defi() {
    let Some(gpu) = gpu("breakc_nested_loops") else {
        return;
    };
    use shader_re::sm3::ir::Cmp;
    // Outer loop 4 rounds (defi i1), inner loop 5 rounds (i0 from the buffer); the inner loop
    // leaves with `break_gt r0.y, c1.y` once the total passed 7: counting continues in the next
    // round of the outer loop, which restarts the inner loop. Total rounds counted by r0.x.
    let sh = ps(|p| {
        p.defi(1, [4, 0, 0, 0]);
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 0, XYZW)]);
        p.inst(Opcode::Rep, 0, &[s(ConstInt, 1, XYZW)]);
        p.inst(Opcode::Mov, 0, &[dm(Temp, 0, 0b0010), s(Const, 0, X)]);
        p.inst(Opcode::Rep, 0, &[s(ConstInt, 0, XYZW)]);
        p.inst(
            Opcode::Add,
            0,
            &[dm(Temp, 0, 0b0001), s(Temp, 0, X), s(Const, 1, X)],
        );
        p.inst(
            Opcode::Add,
            0,
            &[dm(Temp, 0, 0b0010), s(Temp, 0, Y), s(Const, 1, X)],
        );
        p.inst(
            Opcode::BreakC,
            Cmp::Ge.code(),
            &[s(Temp, 0, Y), s(Const, 2, X)],
        );
        p.inst(Opcode::EndRep, 0, &[]);
        p.inst(Opcode::EndRep, 0, &[]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Temp, 0, XYZW)]);
    });
    let r = Run {
        consts: vec![[0.0; 4], [1.0; 4], [3.0; 4]],
        ints: vec![[5, 0, 0, 0]],
        size: (1, 1),
        ..Default::default()
    };
    // Inner loop stops when its own counter (r0.y) reaches 3: 3 rounds per outer round, 4 outer.
    assert_vec(
        run_grid(&gpu, &sh, &r)[0],
        [12.0, 3.0, 0.0, 0.0],
        "nested rep",
    );
}

#[test]
fn branches_and_loops_with_texture_and_derivatives_stay_uniform() {
    // naga's uniformity analysis is part of `validate_wgsl`: derivatives and sampling behind
    // flattened branches and inside a `rep` must not trip it.
    use shader_re::sm3::ir::Cmp;
    let sh = ps(|p| {
        p.dcl_sampler(SamplerKind::Tex2d, 0);
        vpos_dcl(p);
        p.inst(Opcode::Mov, 0, &[d(Temp, 0), s(Const, 0, XYZW)]);
        p.inst(Opcode::Rep, 0, &[s(ConstInt, 0, XYZW)]);
        p.inst(
            Opcode::Ifc,
            Cmp::Gt.code(),
            &[s(MiscType, 0, X), s(Const, 1, X)],
        );
        p.inst(
            Opcode::TexLd,
            0,
            &[d(Temp, 1), s(Temp, 0, XYZW), s(Sampler, 0, XYZW)],
        );
        p.inst(Opcode::Dsx, 0, &[d(Temp, 2), s(Temp, 1, XYZW)]);
        p.inst(
            Opcode::BreakC,
            Cmp::Gt.code(),
            &[s(Temp, 2, X), s(Const, 1, X)],
        );
        p.inst(Opcode::TexKill, 0, &[dm(Temp, 2, 0b0111)]);
        p.inst(Opcode::Else, 0, &[]);
        p.inst(
            Opcode::Add,
            0,
            &[d(Temp, 0), s(Temp, 0, XYZW), s(Const, 1, XYZW)],
        );
        p.inst(Opcode::EndIf, 0, &[]);
        p.inst(Opcode::EndRep, 0, &[]);
        p.inst(Opcode::Mov, 0, &[out0(0xF), s(Temp, 1, XYZW)]);
    });
    assert_valid("uniform", &translate(&sh));
}

#[test]
fn unbalanced_flow_is_an_error() {
    for (name, build) in [
        (
            "endif",
            &(|p: &mut Prog| {
                p.inst(Opcode::EndIf, 0, &[]);
            }) as &dyn Fn(&mut Prog),
        ),
        ("else", &|p: &mut Prog| {
            p.inst(Opcode::Else, 0, &[]);
        }),
        ("endrep", &|p: &mut Prog| {
            p.inst(Opcode::EndRep, 0, &[]);
        }),
        ("break", &|p: &mut Prog| {
            p.inst(Opcode::Break, 0, &[]);
        }),
        ("open if", &|p: &mut Prog| {
            p.inst(Opcode::If, 0, &[s(ConstBool, 0, XYZW)]);
        }),
    ] {
        let mut prog = Prog::new(PS_3_0);
        build(&mut prog);
        prog.inst(Opcode::Mov, 0, &[out0(0xF), s(Const, 0, XYZW)]);
        let sh = prog.shader();
        assert!(
            oracle::translate(&sh, "T::E:PS", SHA, &Options::default()).is_err(),
            "{name}"
        );
    }
}
