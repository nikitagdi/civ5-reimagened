// Float literals below are the exact 9-digit texts the compiler prints.
#![allow(clippy::excessive_precision)]

//! SM3 token parser, CTAB and disassembler on synthetic token streams, plus a pass over the 433 real blocks when the pinned
//! binaries are available.

use shader_re::bins::{self, BinError, BinId, Search};
use shader_re::scan::{self, Stage};
use shader_re::sm3::ctab::{
    self, Ctab, CtabConstant, CtabMember, CtabType, ParamClass, ParamType, RegisterSet,
};
use shader_re::sm3::disasm::{self, fmt_float};
use shader_re::sm3::ir::*;
use shader_re::sm3::{self, Sm3Error};

// --- token builders (d3d9types.h) -------------------------------------------------------------

const VS_3_0: u32 = 0xFFFE_0300;
const PS_3_0: u32 = 0xFFFF_0300;
const END: u32 = 0x0000_FFFF;

const R: RegType = RegType::Temp;
const V: RegType = RegType::Input;
const C: RegType = RegType::Const;

fn reg_bits(ty: RegType, num: u32) -> u32 {
    let c = ty.code();
    0x8000_0000 | num | ((c & 7) << 28) | ((c & 0x18) << 8)
}

/// Destination token: mask in bits 16-19, modifiers in 20-23, shift in 24-27.
fn dst(ty: RegType, num: u32, mask: u32, mods: u32, shift: u32) -> u32 {
    reg_bits(ty, num) | (mask << 16) | (mods << 20) | (shift << 24)
}

/// Source token with a swizzle string like `"xyzw"` and a modifier code.
fn src(ty: RegType, num: u32, swz: &str, modifier: u32) -> u32 {
    reg_bits(ty, num) | (swizzle(swz) << 16) | (modifier << 24)
}

fn swizzle(s: &str) -> u32 {
    let mut v = 0;
    for (i, ch) in s.chars().enumerate() {
        let c = "xyzw".find(ch).expect("component") as u32;
        v |= c << (2 * i);
    }
    v
}

/// Relative token for `a0.<comp>` (or `aL`).
fn rel_a0(comp: &str) -> u32 {
    reg_bits(RegType::Addr, 0) | (swizzle(comp) << 16)
}

fn rel_loop() -> u32 {
    reg_bits(RegType::Loop, 0)
}

fn op(o: Opcode, n_operands: u32, control: u32) -> u32 {
    o.code() as u32 | (control << 16) | (n_operands << 24)
}

fn bytes(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn comment(payload: &[u8]) -> Vec<u32> {
    let mut p = payload.to_vec();
    while !p.len().is_multiple_of(4) {
        p.push(0);
    }
    let n = (p.len() / 4) as u32;
    let mut w = vec![0xFFFE | (n << 16)];
    w.extend(
        p.chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap())),
    );
    w
}

fn ctab_comment(ct: &Ctab) -> Vec<u32> {
    let mut payload = b"CTAB".to_vec();
    payload.extend(ctab::build(ct));
    comment(&payload)
}

/// `version`, optional CTAB, body, END.
fn block(version: u32, ct: Option<&Ctab>, body: &[u32]) -> Vec<u8> {
    let mut w = vec![version];
    if let Some(ct) = ct {
        w.extend(ctab_comment(ct));
    }
    w.extend_from_slice(body);
    w.push(END);
    bytes(&w)
}

fn parse(b: &[u8]) -> sm3::Shader {
    sm3::parse(b).expect("parse")
}

fn text(b: &[u8]) -> String {
    disasm::disassemble(&parse(b), false)
}

/// The instruction lines of a listing (no comments, no version).
fn body_lines(listing: &str) -> Vec<String> {
    listing
        .lines()
        .filter(|l| !l.starts_with("//") && !l.trim().is_empty())
        .skip(1)
        .map(|l| l.trim().to_string())
        .collect()
}

fn scalar(ty: ParamType) -> CtabType {
    CtabType {
        class: ParamClass::Scalar,
        ty,
        rows: 1,
        columns: 1,
        elements: 1,
        members: vec![],
    }
}

fn konst(name: &str, set: RegisterSet, reg: u16, count: u16, ty: CtabType) -> CtabConstant {
    CtabConstant {
        name: name.into(),
        set,
        reg,
        count,
        ty,
        default: None,
    }
}

fn table(constants: Vec<CtabConstant>) -> Ctab {
    Ctab {
        creator: "Microsoft (R) HLSL Shader Compiler 9.27.952.3022".into(),
        version: PS_3_0,
        flags: 0,
        target: "ps_3_0".into(),
        constants,
    }
}

// --- opcodes -----------------------------------------------------------------------------------

/// The 33 opcodes of the original shaders by listing mnemonic.
const ORIGINAL_MNEMONICS: [&str; 33] = [
    "mul", "add", "mad", "mov", "cmp", "dp3", "texldl", "rcp", "texld", "dp2add", "exp", "log",
    "rsq", "max", "nrm", "if", "else", "endif", "pow", "dp4", "frc", "lrp", "texkill", "dsx",
    "dsy", "mova", "rep", "endrep", "break", "min", "texldb", "sincos", "abs",
];

#[test]
fn opcode_table_is_a_bijection() {
    for (i, a) in Opcode::ALL.iter().enumerate() {
        assert_eq!(Opcode::from_code(a.code()), Some(*a));
        for b in &Opcode::ALL[i + 1..] {
            assert_ne!(a.code(), b.code());
        }
    }
    assert_eq!(Opcode::from_code(0x3F), None);
    assert_eq!(Opcode::from_code(0x61), None);
}

/// One synthetic instruction per opcode with the operand shape the opcode needs.
fn sample(o: Opcode) -> Vec<u32> {
    use Opcode::*;
    let d = dst(R, 0, 0xF, 0, 0);
    let s0 = src(R, 1, "xyzw", 0);
    let s1 = src(C, 2, "xyzw", 0);
    let s2 = src(V, 3, "xyzw", 0);
    let s3 = src(C, 4, "xyzw", 0);
    match o {
        Nop | Ret | EndLoop | Else | EndIf | Break | EndRep => vec![op(o, 0, 0)],
        Mov | Rcp | Rsq | Exp | Log | Lit | Frc | Abs | Sgn | Nrm | ExpP | LogP | Mova | Dsx
        | Dsy | SinCos => vec![op(o, 2, 0), d, s0],
        Add | Sub | Mul | Dp3 | Dp4 | Min | Max | Slt | Sge | Dst | Crs | Pow | M4x4 | M4x3
        | M3x4 | M3x3 | M3x2 | TexLdl => vec![op(o, 3, 0), d, s0, s1],
        TexLd => vec![op(o, 3, 0), d, s0, src(RegType::Sampler, 0, "xyzw", 0)],
        Mad | Lrp | Cmp | Dp2Add => vec![op(o, 4, 0), d, s0, s1, s2],
        TexLdd => vec![op(o, 5, 0), d, s0, s1, s2, s3],
        SetP => vec![op(o, 3, 3), dst(RegType::Predicate, 0, 0xF, 0, 0), s0, s1],
        TexKill => vec![op(o, 1, 0), dst(R, 0, 0xF, 0, 0)],
        If | Rep | Call | Label | BreakP => {
            vec![op(o, 1, 0), src(RegType::ConstBool, 0, "xxxx", 0)]
        }
        Ifc | BreakC => vec![op(o, 2, 3), s0, s1],
        CallNz | Loop => vec![op(o, 2, 0), s0, s1],
        Dcl => vec![op(o, 2, 0), 0x8000_0000 | 5, dst(V, 0, 0xF, 0, 0)],
        Def => vec![
            op(o, 5, 0),
            dst(C, 0, 0xF, 0, 0),
            1f32.to_bits(),
            2f32.to_bits(),
            3f32.to_bits(),
            4f32.to_bits(),
        ],
        DefI => vec![
            op(o, 5, 0),
            dst(RegType::ConstInt, 0, 0xF, 0, 0),
            1,
            2,
            3,
            4,
        ],
        DefB => vec![op(o, 2, 0), dst(RegType::ConstBool, 0, 0xF, 0, 0), 1],
    }
}

#[test]
fn every_opcode_parses_with_its_operands() {
    for o in Opcode::ALL {
        let b = block(PS_3_0, None, &sample(o));
        let sh = parse(&b);
        assert_eq!(sh.stmts.len(), 1, "{o:?}");
        match &sh.stmts[0] {
            Stmt::Inst(i) => assert_eq!(i.op, o),
            Stmt::Dcl(_) => assert_eq!(o, Opcode::Dcl),
            Stmt::DefF { .. } => assert_eq!(o, Opcode::Def),
            Stmt::DefI { .. } => assert_eq!(o, Opcode::DefI),
            Stmt::DefB { .. } => assert_eq!(o, Opcode::DefB),
        }
        // And it prints.
        let listing = text(&b);
        assert!(listing.contains("ps_3_0"), "{o:?}");
    }
}

#[test]
fn the_33_original_opcodes_print_their_mnemonics() {
    let mut w = Vec::new();
    let pieces: [(Opcode, u32); 33] = [
        (Opcode::Mul, 0),
        (Opcode::Add, 0),
        (Opcode::Mad, 0),
        (Opcode::Mov, 0),
        (Opcode::Cmp, 0),
        (Opcode::Dp3, 0),
        (Opcode::TexLdl, 0),
        (Opcode::Rcp, 0),
        (Opcode::TexLd, 0),
        (Opcode::Dp2Add, 0),
        (Opcode::Exp, 0),
        (Opcode::Log, 0),
        (Opcode::Rsq, 0),
        (Opcode::Max, 0),
        (Opcode::Nrm, 0),
        (Opcode::Ifc, 3),
        (Opcode::Else, 0),
        (Opcode::EndIf, 0),
        (Opcode::Pow, 0),
        (Opcode::Dp4, 0),
        (Opcode::Frc, 0),
        (Opcode::Lrp, 0),
        (Opcode::TexKill, 0),
        (Opcode::Dsx, 0),
        (Opcode::Dsy, 0),
        (Opcode::Mova, 0),
        (Opcode::Rep, 0),
        (Opcode::EndRep, 0),
        (Opcode::Break, 0),
        (Opcode::Min, 0),
        (Opcode::TexLd, 2),
        (Opcode::SinCos, 0),
        (Opcode::Abs, 0),
    ];
    for (o, control) in pieces {
        let mut s = sample(o);
        s[0] |= control << 16;
        if o == Opcode::Rep {
            s[1] = src(RegType::ConstInt, 0, "xyzw", 0);
        }
        w.extend(s);
    }
    let listing = text(&block(PS_3_0, None, &w));
    let mut names: Vec<String> = body_lines(&listing)
        .iter()
        .map(|l| l.split([' ', ',']).next().unwrap().to_string())
        .collect();
    names.iter_mut().for_each(|n| {
        if n.starts_with("if_") {
            *n = "if".into();
        }
    });
    for want in ORIGINAL_MNEMONICS {
        assert!(
            names.iter().any(|n| n == want),
            "{want} missing in {names:?}"
        );
    }
    assert_eq!(ORIGINAL_MNEMONICS.len(), 33);
}

// --- operands ------------------------------------------------------------------------------------

#[test]
fn write_masks_swizzles_and_modifiers() {
    let mut w = Vec::new();
    for mask in [0x1, 0x2, 0x3, 0x5, 0x7, 0xB, 0xC, 0xF] {
        w.extend([
            op(Opcode::Mov, 2, 0),
            dst(R, 0, mask, 0, 0),
            src(C, 1, "xyzw", 0),
        ]);
    }
    for sw in [
        "xyzw", "xxxx", "yyyy", "zzzz", "wwww", "wzyx", "xyzz", "yzxw",
    ] {
        w.extend([
            op(Opcode::Mov, 2, 0),
            dst(R, 0, 0xF, 0, 0),
            src(R, 1, sw, 0),
        ]);
    }
    let lines = body_lines(&text(&block(PS_3_0, None, &w)));
    assert_eq!(
        lines[..8],
        [
            "mov r0.x, c1",
            "mov r0.y, c1",
            "mov r0.xy, c1",
            "mov r0.xz, c1",
            "mov r0.xyz, c1",
            "mov r0.xyw, c1",
            "mov r0.zw, c1",
            "mov r0, c1",
        ]
    );
    assert_eq!(
        lines[8..],
        [
            "mov r0, r1",
            "mov r0, r1.x",
            "mov r0, r1.y",
            "mov r0, r1.z",
            "mov r0, r1.w",
            "mov r0, r1.wzyx",
            "mov r0, r1.xyzz",
            "mov r0, r1.yzxw",
        ]
    );
    // And the IR keeps the numbers.
    let sh = parse(&block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 2, 0),
            dst(R, 0, 0x6, 0, 0),
            src(R, 1, "wzyx", 0),
        ],
    ));
    let Stmt::Inst(i) = &sh.stmts[0] else {
        panic!()
    };
    assert_eq!(i.dst.unwrap().mask, 0x6);
    assert_eq!(i.src[0].swizzle, [3, 2, 1, 0]);
}

#[test]
fn source_modifiers() {
    let want = [
        (0, "r1"),
        (1, "-r1"),
        (2, "r1_bias"),
        (3, "-r1_bias"),
        (4, "r1_bx2"),
        (5, "-r1_bx2"),
        (6, "1-r1"),
        (7, "r1_x2"),
        (8, "-r1_x2"),
        (9, "r1_dz"),
        (10, "r1_dw"),
        (11, "r1_abs"),
        (12, "-r1_abs"),
        (13, "!r1"),
    ];
    for (code, name) in want {
        let b = block(
            PS_3_0,
            None,
            &[
                op(Opcode::Mov, 2, 0),
                dst(R, 0, 0xF, 0, 0),
                src(R, 1, "xyzw", code),
            ],
        );
        assert_eq!(body_lines(&text(&b)), [format!("mov r0, {name}")]);
    }
    // With a swizzle the modifier suffix stays before it.
    let b = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Add, 3, 0),
            dst(R, 0, 0x1, 0, 0),
            src(R, 1, "wwww", 12),
            src(R, 2, "x", 1),
        ],
    );
    assert_eq!(body_lines(&text(&b)), ["add r0.x, -r1_abs.w, -r2.x"]);
}

#[test]
fn result_modifiers_and_shift() {
    let cases = [
        (0, 0, "mul"),
        (MOD_SAT as u32, 0, "mul_sat"),
        (MOD_PP as u32, 0, "mul_pp"),
        (MOD_SAT as u32 | MOD_PP as u32, 0, "mul_sat_pp"),
        (MOD_CENTROID as u32, 0, "mul_centroid"),
        (0, 1, "mul_x2"),
        (0, 2, "mul_x4"),
        (0, 0xF, "mul_d2"),
    ];
    for (mods, shift, name) in cases {
        let b = block(
            PS_3_0,
            None,
            &[
                op(Opcode::Mul, 3, 0),
                dst(R, 0, 0xF, mods, shift),
                src(R, 1, "xyzw", 0),
                src(R, 2, "xyzw", 0),
            ],
        );
        assert_eq!(body_lines(&text(&b)), [format!("{name} r0, r1, r2")]);
    }
    let b = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 2, 0),
            dst(R, 0, 0xF, 3, 0xF),
            src(R, 1, "xyzw", 0),
        ],
    );
    let sh = parse(&b);
    let Stmt::Inst(i) = &sh.stmts[0] else {
        panic!()
    };
    let d = i.dst.unwrap();
    assert!(d.sat() && d.pp() && !d.centroid());
    assert_eq!(d.shift, -1);
}

#[test]
fn register_files_have_their_names() {
    let cases: [(Stage, RegType, u32, &str); 20] = [
        (Stage::Ps, RegType::Temp, 3, "r3"),
        (Stage::Ps, RegType::Input, 4, "v4"),
        (Stage::Ps, RegType::Const, 200, "c200"),
        (Stage::Vs, RegType::Addr, 0, "a0"),
        (Stage::Ps, RegType::Addr, 1, "t1"),
        (Stage::Vs, RegType::RastOut, 0, "oPos"),
        (Stage::Vs, RegType::RastOut, 1, "oFog"),
        (Stage::Vs, RegType::AttrOut, 1, "oD1"),
        (Stage::Vs, RegType::Output, 6, "o6"),
        (Stage::Ps, RegType::ConstInt, 2, "i2"),
        (Stage::Ps, RegType::ColorOut, 1, "oC1"),
        (Stage::Ps, RegType::DepthOut, 0, "oDepth"),
        (Stage::Ps, RegType::Sampler, 15, "s15"),
        (Stage::Vs, RegType::Const2, 1, "c2049"),
        (Stage::Vs, RegType::ConstBool, 5, "b5"),
        (Stage::Vs, RegType::Loop, 0, "aL"),
        (Stage::Ps, RegType::MiscType, 0, "vPos"),
        (Stage::Ps, RegType::MiscType, 1, "vFace"),
        (Stage::Ps, RegType::Label, 3, "l3"),
        (Stage::Ps, RegType::Predicate, 0, "p0"),
    ];
    for (stage, ty, num, name) in cases {
        assert_eq!(disasm::reg_name(Reg { ty, num }, stage), name);
        assert_eq!(RegType::from_code(ty.code()), Some(ty));
        // Codes above 7 use the second type field (bits 11-12).
        let mut w = vec![if stage == Stage::Vs { VS_3_0 } else { PS_3_0 }];
        w.extend([
            op(Opcode::Mov, 2, 0),
            dst(RegType::Temp, 0, 0xF, 0, 0),
            src(ty, num, "xyzw", 0),
            END,
        ]);
        let sh = parse(&bytes(&w));
        let Stmt::Inst(i) = &sh.stmts[0] else {
            panic!()
        };
        assert_eq!(i.src[0].reg, Reg { ty, num });
    }
}

#[test]
fn relative_addressing_through_a0_and_al() {
    let b = block(
        VS_3_0,
        None,
        &[
            // dp4 r0.x, v0, c17[a0.x] -- the relative token follows the source token.
            op(Opcode::Dp4, 4, 0),
            dst(R, 0, 0x1, 0, 0),
            src(V, 0, "xyzw", 0),
            src(C, 17, "xyzw", 0) | (1 << 13),
            rel_a0("x"),
            // mov r1, c4[a0.w]
            op(Opcode::Mov, 3, 0),
            dst(R, 1, 0xF, 0, 0),
            src(C, 4, "xyzw", 0) | (1 << 13),
            rel_a0("w"),
            // mov r2, c0[aL]
            op(Opcode::Mov, 3, 0),
            dst(R, 2, 0xF, 0, 0),
            src(C, 0, "xyzw", 0) | (1 << 13),
            rel_loop(),
        ],
    );
    let sh = parse(&b);
    let Stmt::Inst(i) = &sh.stmts[0] else {
        panic!()
    };
    assert_eq!(
        i.src[1].rel,
        Some(Rel {
            ty: RegType::Addr,
            num: 0,
            comp: 0
        })
    );
    assert_eq!(
        body_lines(&text(&b)),
        [
            "dp4 r0.x, v0, c17[a0.x]",
            "mov r1, c4[a0.w]",
            "mov r2, c0[aL]"
        ]
    );
}

#[test]
fn comparison_texld_and_predicates() {
    let w = [
        op(Opcode::Ifc, 2, 1),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndIf, 0, 0),
        op(Opcode::Ifc, 2, 2),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndIf, 0, 0),
        op(Opcode::Ifc, 2, 3),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndIf, 0, 0),
        op(Opcode::Ifc, 2, 4),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndIf, 0, 0),
        op(Opcode::Ifc, 2, 5),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndIf, 0, 0),
        op(Opcode::Ifc, 2, 6),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndIf, 0, 0),
        op(Opcode::Rep, 1, 0),
        src(RegType::ConstInt, 0, "xyzw", 0),
        op(Opcode::BreakC, 2, 3),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::EndRep, 0, 0),
        op(Opcode::TexLd, 3, 0),
        dst(R, 0, 0xF, 0, 0),
        src(V, 0, "xyzw", 0),
        src(RegType::Sampler, 0, "xyzw", 0),
        op(Opcode::TexLd, 3, 1),
        dst(R, 0, 0xF, 0, 0),
        src(V, 0, "xyzw", 0),
        src(RegType::Sampler, 0, "xyzw", 0),
        op(Opcode::TexLd, 3, 2),
        dst(R, 0, 0xF, MOD_PP as u32, 0),
        src(V, 0, "xyzw", 0),
        src(RegType::Sampler, 1, "xyzw", 0),
    ];
    let lines = body_lines(&text(&block(PS_3_0, None, &w)));
    assert_eq!(
        lines,
        [
            "if_gt r0.x, c0.y",
            "endif",
            "if_eq r0.x, c0.y",
            "endif",
            "if_ge r0.x, c0.y",
            "endif",
            "if_lt r0.x, c0.y",
            "endif",
            "if_ne r0.x, c0.y",
            "endif",
            "if_le r0.x, c0.y",
            "endif",
            "rep i0",
            "break_ge r0.x, c0.y",
            "endrep",
            "texld r0, v0, s0",
            "texldp r0, v0, s0",
            "texldb_pp r0, v0, s1",
        ]
    );
    // A comparison code outside 1..=6 is an error, not a guess.
    let bad = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Ifc, 2, 7),
            src(R, 0, "xxxx", 0),
            src(R, 0, "xxxx", 0),
        ],
    );
    assert!(sm3::parse(&bad).is_err());

    // Predicated instruction: the predicate source is the last operand.
    let pred = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 3, 0) | (1 << 28),
            dst(R, 0, 0xF, 0, 0),
            src(R, 1, "xyzw", 0),
            src(RegType::Predicate, 0, "xxxx", 0),
        ],
    );
    let sh = parse(&pred);
    let Stmt::Inst(i) = &sh.stmts[0] else {
        panic!()
    };
    assert_eq!(i.src.len(), 1);
    assert_eq!(i.pred.unwrap().reg.ty, RegType::Predicate);
    assert_eq!(body_lines(&text(&pred)), ["(p0.x) mov r0, r1"]);
}

#[test]
fn nesting_indents_two_spaces() {
    let w = [
        op(Opcode::Ifc, 2, 3),
        src(R, 0, "xxxx", 0),
        src(C, 0, "yyyy", 0),
        op(Opcode::Rep, 1, 0),
        src(RegType::ConstInt, 0, "xyzw", 0),
        op(Opcode::Mov, 2, 0),
        dst(R, 1, 0xF, 0, 0),
        src(R, 2, "xyzw", 0),
        op(Opcode::EndRep, 0, 0),
        op(Opcode::Else, 0, 0),
        op(Opcode::Mov, 2, 0),
        dst(R, 1, 0xF, 0, 0),
        src(R, 3, "xyzw", 0),
        op(Opcode::EndIf, 0, 0),
    ];
    let listing = text(&block(PS_3_0, None, &w));
    let body: Vec<&str> = listing
        .lines()
        .filter(|l| l.starts_with(' '))
        .skip(1)
        .collect();
    assert_eq!(
        body,
        [
            "    if_ge r0.x, c0.y",
            "      rep i0",
            "        mov r1, r2",
            "      endrep",
            "    else",
            "      mov r1, r3",
            "    endif",
        ]
    );
}

// --- declarations and constants ------------------------------------------------------------

#[test]
fn def_defi_defb() {
    let w = [
        op(Opcode::Def, 5, 0),
        dst(C, 3, 0xF, 0, 0),
        0.75f32.to_bits(),
        1f32.to_bits(),
        (-0f32).to_bits(),
        1e-8f32.to_bits(),
        op(Opcode::DefI, 5, 0),
        dst(RegType::ConstInt, 1, 0xF, 0, 0),
        16,
        0,
        (-2i32) as u32,
        7,
        op(Opcode::DefB, 2, 0),
        dst(RegType::ConstBool, 4, 0xF, 0, 0),
        1,
        op(Opcode::DefB, 2, 0),
        dst(RegType::ConstBool, 5, 0xF, 0, 0),
        0,
    ];
    let b = block(PS_3_0, None, &w);
    let sh = parse(&b);
    assert_eq!(
        sh.stmts[0],
        Stmt::DefF {
            reg: Reg { ty: C, num: 3 },
            value: [0.75, 1.0, -0.0, 1e-8]
        }
    );
    assert_eq!(
        sh.stmts[1],
        Stmt::DefI {
            reg: Reg {
                ty: RegType::ConstInt,
                num: 1
            },
            value: [16, 0, -2, 7]
        }
    );
    assert_eq!(
        body_lines(&text(&b)),
        [
            "def c3, 0.75, 1, -0, 9.99999994e-009",
            "defi i1, 16, 0, -2, 7",
            "defb b4, true",
            "defb b5, false",
        ]
    );
}

fn dcl_semantic(usage: u32, index: u32, d: u32) -> Vec<u32> {
    vec![
        op(Opcode::Dcl, 2, 0),
        0x8000_0000 | usage | (index << 16),
        d,
    ]
}

fn dcl_sampler(tex: u32, num: u32) -> Vec<u32> {
    vec![
        op(Opcode::Dcl, 2, 0),
        0x8000_0000 | (tex << 27),
        dst(RegType::Sampler, num, 0xF, 0, 0),
    ]
}

#[test]
fn dcl_semantics_samplers_and_misc() {
    let mut w = Vec::new();
    w.extend(dcl_semantic(5, 0, dst(V, 0, 0x3, 0, 0))); // dcl_texcoord v0.xy
    w.extend(dcl_semantic(5, 3, dst(V, 1, 0x7, MOD_PP as u32, 0))); // dcl_texcoord3_pp v1.xyz
    w.extend(dcl_semantic(10, 1, dst(V, 2, 0xF, MOD_CENTROID as u32, 0))); // dcl_color1_centroid v2
    w.extend(dcl_semantic(3, 0, dst(V, 3, 0xF, 0, 0))); // dcl_normal v3
    w.extend(dcl_semantic(2, 1, dst(V, 4, 0xF, 0, 0))); // dcl_blendindices1 v4
    w.extend(dcl_sampler(2, 0));
    w.extend(dcl_sampler(3, 1));
    w.extend(dcl_sampler(4, 2));
    w.extend([
        op(Opcode::Dcl, 2, 0),
        0x8000_0000,
        dst(RegType::MiscType, 0, 0x3, 0, 0),
    ]); // dcl vPos.xy
    w.extend([
        op(Opcode::Dcl, 2, 0),
        0x8000_0000,
        dst(RegType::MiscType, 1, 0xF, 0, 0),
    ]); // dcl vFace
    let b = block(PS_3_0, None, &w);
    assert_eq!(
        body_lines(&text(&b)),
        [
            "dcl_texcoord v0.xy",
            "dcl_texcoord3_pp v1.xyz",
            "dcl_color1_centroid v2",
            "dcl_normal v3",
            "dcl_blendindices1 v4",
            "dcl_2d s0",
            "dcl_cube s1",
            "dcl_volume s2",
            "dcl vPos.xy",
            "dcl vFace",
        ]
    );
    let sh = parse(&b);
    let Stmt::Dcl(d) = &sh.stmts[5] else { panic!() };
    assert_eq!(d.kind, DclKind::Sampler(SamplerKind::Tex2d));
    let Stmt::Dcl(d) = &sh.stmts[1] else { panic!() };
    assert_eq!(
        d.kind,
        DclKind::Semantic {
            usage: Some(Usage::TexCoord),
            index: 3
        }
    );
    assert!(d.dst.pp());
    // Vertex shader output declaration.
    let vs = block(
        VS_3_0,
        None,
        &dcl_semantic(0, 0, dst(RegType::Output, 0, 0xF, 0, 0)),
    );
    assert_eq!(body_lines(&text(&vs)), ["dcl_position o0"]);
}

#[test]
fn ctab_all_classes_round_trip_and_print() {
    let float4 = |class, rows, cols, elems| CtabType {
        class,
        ty: ParamType::Float,
        rows,
        columns: cols,
        elements: elems,
        members: vec![],
    };
    let sampler = |ty| CtabType {
        class: ParamClass::Object,
        ty,
        rows: 1,
        columns: 1,
        elements: 1,
        members: vec![],
    };
    let mut with_default = konst(
        "Tint",
        RegisterSet::Float4,
        9,
        1,
        float4(ParamClass::Vector, 1, 4, 1),
    );
    with_default.default = Some(bytes(&[
        0.5f32.to_bits(),
        1f32.to_bits(),
        2f32.to_bits(),
        3f32.to_bits(),
    ]));
    let ct = table(vec![
        konst(
            "Alpha",
            RegisterSet::Float4,
            0,
            1,
            float4(ParamClass::Scalar, 1, 1, 1),
        ),
        konst(
            "Offset",
            RegisterSet::Float4,
            1,
            1,
            float4(ParamClass::Vector, 1, 2, 1),
        ),
        konst(
            "Light",
            RegisterSet::Float4,
            2,
            4,
            float4(ParamClass::Vector, 1, 4, 4),
        ),
        konst(
            "World",
            RegisterSet::Float4,
            6,
            4,
            float4(ParamClass::MatrixRows, 4, 4, 1),
        ),
        konst(
            "Skin",
            RegisterSet::Float4,
            10,
            12,
            float4(ParamClass::MatrixColumns, 4, 3, 4),
        ),
        konst("UseFog", RegisterSet::Bool, 0, 1, scalar(ParamType::Bool)),
        konst("Count", RegisterSet::Int4, 0, 1, scalar(ParamType::Int)),
        konst(
            "Diffuse",
            RegisterSet::Sampler,
            0,
            1,
            sampler(ParamType::Sampler2d),
        ),
        konst(
            "Volume",
            RegisterSet::Sampler,
            1,
            1,
            sampler(ParamType::Sampler3d),
        ),
        konst(
            "Env",
            RegisterSet::Sampler,
            2,
            1,
            sampler(ParamType::SamplerCube),
        ),
        with_default,
        konst(
            "Cfg",
            RegisterSet::Float4,
            30,
            2,
            CtabType {
                class: ParamClass::Struct,
                ty: ParamType::Void,
                rows: 1,
                columns: 4,
                elements: 1,
                members: vec![
                    CtabMember {
                        name: "a".into(),
                        ty: float4(ParamClass::Scalar, 1, 1, 1),
                    },
                    CtabMember {
                        name: "b".into(),
                        ty: float4(ParamClass::Vector, 1, 3, 1),
                    },
                ],
            },
        ),
    ]);
    let b = block(
        PS_3_0,
        Some(&ct),
        &[
            op(Opcode::Mov, 2, 0),
            dst(R, 0, 0xF, 0, 0),
            src(C, 0, "xxxx", 0),
        ],
    );
    let sh = parse(&b);
    assert_eq!(sh.ctab.as_ref().unwrap(), &ct);
    let c = sh.ctab.as_ref().unwrap();
    assert_eq!(c.find(RegisterSet::Float4, 3).unwrap().name, "Light");
    assert!(c.find(RegisterSet::Float4, 200).is_none());
    assert_eq!(c.constants[0].ty.class.name(), "scalar");

    let listing = text(&b);
    for line in [
        "//   float Alpha;",
        "//   float2 Offset;",
        "//   float4 Light[4];",
        "//   row_major float4x4 World;",
        "//   float4x3 Skin[4];",
        "//   bool UseFog;",
        "//   int Count;",
        "//   sampler2D Diffuse;",
        "//   sampler3D Volume;",
        "//   samplerCUBE Env;",
    ] {
        assert!(listing.lines().any(|l| l == line), "{line}\n{listing}");
    }
    // Registers sorted by register file and number.
    let regs: Vec<&str> = listing
        .lines()
        .skip_while(|l| !l.starts_with("//   Name"))
        .skip(2)
        .take_while(|l| *l != "//")
        .collect();
    let names: Vec<&str> = regs
        .iter()
        .map(|l| l.split_whitespace().nth(2).unwrap())
        .collect();
    assert_eq!(names[0], "b0");
    assert_eq!(names[1], "i0");
    assert_eq!(names[2], "c0");
    assert_eq!(*names.last().unwrap(), "s2");
}

#[test]
fn listing_matches_the_text_d3d_prints() {
    // The block of a pixel shader checked against D3DDisassemble by hand (crosscheck-d3d does the
    // same for all 433): the text below is d3dcompiler_47's output for it.
    let ct = table(vec![
        konst(
            "g_vWaterTextureTiling",
            RegisterSet::Float4,
            3,
            1,
            CtabType {
                class: ParamClass::Vector,
                ty: ParamType::Float,
                rows: 1,
                columns: 2,
                elements: 1,
                members: vec![],
            },
        ),
        konst(
            "WaterTexture",
            RegisterSet::Sampler,
            2,
            1,
            CtabType {
                class: ParamClass::Object,
                ty: ParamType::Sampler2d,
                rows: 1,
                columns: 1,
                elements: 1,
                members: vec![],
            },
        ),
    ]);
    // Parameters are in table order; the real table is sorted, so put the sampler first.
    let mut ct = ct;
    ct.constants.reverse();
    let w = [
        op(Opcode::Def, 5, 0),
        dst(C, 0, 0xF, 0, 0),
        0.454545468f32.to_bits(),
        1f32.to_bits(),
        0,
        0,
        op(Opcode::Dcl, 2, 0),
        0x8000_0000 | 5,
        dst(V, 0, 0x3, 0, 0),
        op(Opcode::Dcl, 2, 0),
        0x8000_0000 | (2 << 27),
        dst(RegType::Sampler, 2, 0xF, 0, 0),
        op(Opcode::Mad, 4, 0),
        dst(R, 0, 0x3, 0, 0),
        src(V, 0, "xyzw", 0),
        src(C, 3, "xxxx", 0),
        src(C, 3, "yyyy", 0),
        op(Opcode::TexLd, 3, 0),
        dst(R, 0, 0xF, 0, 0),
        src(R, 0, "xyzw", 0),
        src(RegType::Sampler, 2, "xyzw", 0),
        op(Opcode::Log, 2, 0),
        dst(R, 1, 0x1, 0, 0),
        src(R, 0, "xxxx", 0),
        op(Opcode::Mul, 3, 0),
        dst(R, 0, 0x7, 0, 0),
        src(R, 1, "xyzw", 0),
        src(C, 0, "xxxx", 0),
        op(Opcode::Exp, 2, 0),
        dst(RegType::ColorOut, 0, 0x1, 0, 0),
        src(R, 0, "xxxx", 0),
        op(Opcode::Mov, 2, 0),
        dst(RegType::ColorOut, 0, 0x8, 0, 0),
        src(C, 0, "yyyy", 0),
    ];
    let listing = text(&block(PS_3_0, Some(&ct), &w));
    let want = "\
//
// Generated by Microsoft (R) HLSL Shader Compiler 9.27.952.3022
//
// Parameters:
//
//   sampler2D WaterTexture;
//   float2 g_vWaterTextureTiling;
//
//
// Registers:
//
//   Name                  Reg   Size
//   --------------------- ----- ----
//   g_vWaterTextureTiling c3       1
//   WaterTexture          s2       1
//

    ps_3_0
    def c0, 0.454545468, 1, 0, 0
    dcl_texcoord v0.xy
    dcl_2d s2
    mad r0.xy, v0, c3.x, c3.y
    texld r0, r0, s2
    log r1.x, r0.x
    mul r0.xyz, r1, c0.x
    exp oC0.x, r0.x
    mov oC0.w, c0.y

// approximately 6 instruction slots used (1 texture, 5 arithmetic)
";
    assert_eq!(listing, want);
    assert!(disasm::first_diff(&listing, want).is_none());
    // No constants: no comment header at all, like the compiler.
    let plain = text(&block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 2, 0),
            dst(RegType::ColorOut, 0, 0xF, 0, 0),
            src(C, 0, "xxxy", 0),
        ],
    ));
    assert_eq!(
        plain,
        "    ps_3_0\n    mov oC0, c0.xxxy\n\n// approximately 1 instruction slot used\n"
    );
    // Annotation adds the CTAB names after the instruction.
    let sh = parse(&block(PS_3_0, Some(&ct), &w));
    let annotated = disasm::disassemble(&sh, true);
    assert!(annotated.contains("    mad r0.xy, v0, c3.x, c3.y  // c3 = g_vWaterTextureTiling\n"));
    assert!(annotated.contains("    texld r0, r0, s2  // s2 = WaterTexture\n"));
}

#[test]
fn normalisation_is_only_about_blanks() {
    assert!(disasm::first_diff("a  \r\nb\n\n", "a\nb").is_none());
    assert_eq!(
        disasm::first_diff("a\nb", "a\nc"),
        Some((2, "b".into(), "c".into()))
    );
    assert_eq!(
        disasm::first_diff("a", "a\nb"),
        Some((2, "<end>".into(), "b".into()))
    );
    // Different number text is a difference.
    assert!(disasm::first_diff("def c0, 1.0", "def c0, 1").is_some());
}

#[test]
fn floats_print_like_printf_9g() {
    let cases: [(f32, &str); 14] = [
        (0.75, "0.75"),
        (1.0, "1"),
        (0.0, "0"),
        (-0.0, "-0"),
        (-1.5, "-1.5"),
        (0.454545468, "0.454545468"),
        (1e-8, "9.99999994e-009"),
        (-0.00999999978, "-0.00999999978"),
        (0.0001, "9.99999975e-005"),
        (1e8, "100000000"),
        (1e9, "1e+009"),
        (3.4e38, "3.39999995e+038"),
        (f32::INFINITY, "1.#INF"),
        (f32::NEG_INFINITY, "-1.#INF"),
    ];
    for (v, want) in cases {
        assert_eq!(fmt_float(v), want, "{v:e}");
    }
    assert_eq!(fmt_float(123456792.0), "123456792");
    assert_eq!(fmt_float(0.001), "0.00100000005");
}

// --- errors -------------------------------------------------------------------------------------

#[test]
fn malformed_streams_are_errors() {
    let good = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 2, 0),
            dst(R, 0, 0xF, 0, 0),
            src(C, 0, "xyzw", 0),
        ],
    );
    assert!(sm3::parse(&good).is_ok());
    // Truncated before END.
    assert!(matches!(
        sm3::parse(&good[..good.len() - 4]),
        Err(Sm3Error::Truncated(_))
    ));
    // Not a multiple of four.
    assert!(matches!(
        sm3::parse(&good[..good.len() - 1]),
        Err(Sm3Error::BadSize(_))
    ));
    // Bad version.
    let mut v = good.clone();
    v[..4].copy_from_slice(&0x1234_0300u32.to_le_bytes());
    assert!(matches!(sm3::parse(&v), Err(Sm3Error::BadVersion(_))));
    // Opcode above the SM3 range.
    let mut o = good.clone();
    o[4..8].copy_from_slice(&0x0200_0061u32.to_le_bytes());
    assert!(matches!(sm3::parse(&o), Err(Sm3Error::BadOpcode { .. })));
    // A legacy 1.x opcode (texcoord = 64) is not silently accepted.
    let mut l = good.clone();
    l[4..8].copy_from_slice(&0x0200_0040u32.to_le_bytes());
    assert!(matches!(sm3::parse(&l), Err(Sm3Error::BadOpcode { .. })));
    // Wrong operand count: `mov` with three operand dwords.
    let bad = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 3, 0),
            dst(R, 0, 0xF, 0, 0),
            src(C, 0, "xyzw", 0),
            src(C, 1, "xyzw", 0),
        ],
    );
    assert!(sm3::parse(&bad).is_err());
    // Operand count too small.
    let short = block(PS_3_0, None, &[op(Opcode::Mov, 1, 0), dst(R, 0, 0xF, 0, 0)]);
    assert!(sm3::parse(&short).is_err());
    // Relative token that is not an address register.
    let rel = block(
        VS_3_0,
        None,
        &[
            op(Opcode::Mov, 3, 0),
            dst(R, 0, 0xF, 0, 0),
            src(C, 0, "xyzw", 0) | (1 << 13),
            src(R, 0, "xxxx", 0),
        ],
    );
    assert!(sm3::parse(&rel).is_err());
    // Words after END.
    let mut tail = good.clone();
    tail.extend_from_slice(&[0, 0, 0, 0]);
    assert!(matches!(sm3::parse(&tail), Err(Sm3Error::TrailingData(1))));
    // A corrupt CTAB is an error with its own message.
    let mut c = vec![PS_3_0];
    c.extend(comment(b"CTABxxxx"));
    c.push(END);
    assert!(matches!(sm3::parse(&bytes(&c)), Err(Sm3Error::Ctab(_))));
    // Parameter token without bit 31.
    let nb = block(
        PS_3_0,
        None,
        &[
            op(Opcode::Mov, 2, 0),
            dst(R, 0, 0xF, 0, 0) & 0x7FFF_FFFF,
            src(C, 0, "xyzw", 0),
        ],
    );
    assert!(sm3::parse(&nb).is_err());
}

#[test]
fn other_comments_are_kept() {
    let mut w = vec![PS_3_0];
    w.extend(comment(b"hello world!"));
    w.extend([
        op(Opcode::Mov, 2, 0),
        dst(R, 0, 0xF, 0, 0),
        src(C, 0, "xyzw", 0),
        END,
    ]);
    let sh = parse(&bytes(&w));
    assert!(sh.ctab.is_none());
    assert_eq!(sh.comments.len(), 1);
    assert_eq!(sh.comments[0].data, b"hello world!");
}

// --- pinned binaries ----------------------------------------------------------------------------

fn load(id: BinId) -> Option<Vec<u8>> {
    match bins::find(id, &Search::from_env()) {
        Ok(bin) => Some(bin.read().expect("read binary")),
        Err(e @ BinError::NotFound { .. }) => {
            eprintln!("SKIP {id}: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn check_all_blocks_parse(id: BinId) {
    let Some(data) = load(id) else { return };
    let blocks = scan::extract(id, &data).expect("extract");
    assert_eq!(blocks.len(), scan::SM3_TOTAL);
    let mut with_samplers = 0;
    for b in &blocks {
        let bytes = &data[b.offset..b.offset + b.size];
        let sh = sm3::parse(bytes).unwrap_or_else(|e| panic!("{}: {e}", b.sha1));
        assert_eq!(sh.version.profile(), b.profile, "{}", b.sha1);
        let ct = sh
            .ctab
            .as_ref()
            .unwrap_or_else(|| panic!("{}: no CTAB", b.sha1));
        assert_eq!(ct.target, b.profile, "{}", b.sha1);
        assert!(ct.creator.starts_with("Microsoft (R) HLSL Shader Compiler"));
        assert!(sh.insts().next().is_some(), "{}", b.sha1);
        if ct.constants.iter().any(|c| c.set == RegisterSet::Sampler) {
            with_samplers += 1;
        }
        // The listing is stable: parse, print, nothing panics.
        assert!(!sm3::disassemble(&sh, true).is_empty());
    }
    assert!(with_samplers > 100, "{with_samplers}");
}

#[test]
fn all_433_linux_blocks_parse() {
    check_all_blocks_parse(BinId::Linux);
}

#[test]
fn all_433_dx9_blocks_parse() {
    check_all_blocks_parse(BinId::Dx9);
}

#[test]
fn real_blocks_have_the_pinned_opcode_set() {
    let Some(data) = load(BinId::Linux) else {
        return;
    };
    let blocks = scan::extract(BinId::Linux, &data).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let mut insts = 0usize;
    for b in &blocks {
        let sh = sm3::parse(&data[b.offset..b.offset + b.size]).unwrap();
        for i in sh.insts() {
            insts += 1;
            let name = match (i.op, i.control) {
                (Opcode::TexLd, TEXLD_BIAS) => "texldb".to_string(),
                (Opcode::TexLd, TEXLD_PROJECT) => "texldp".to_string(),
                (Opcode::Ifc | Opcode::BreakC, _) => i.op.name().to_string(),
                (o, _) => o.name().to_string(),
            };
            seen.insert(name);
        }
    }
    // research: approach §1.1 — 33 opcodes (if/else/endif and rep/endrep counted one by one) and
    // 29 817 instructions. `if`/`break` with a comparison share a mnemonic prefix here.
    assert_eq!(insts, 29_817, "instruction count");
    for want in [
        "mul", "add", "mad", "mov", "cmp", "dp3", "texldl", "rcp", "texld", "dp2add", "exp", "log",
        "rsq", "max", "nrm", "if", "else", "endif", "pow", "dp4", "frc", "lrp", "texkill", "dsx",
        "dsy", "mova", "rep", "endrep", "break", "min", "texldb", "sincos", "abs",
    ] {
        assert!(seen.contains(want), "{want} not in the blocks");
    }
    assert_eq!(seen.len(), 33, "{seen:?}");
}
