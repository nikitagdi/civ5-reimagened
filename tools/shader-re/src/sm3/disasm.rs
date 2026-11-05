//! SM3 disassembler: the text of `D3DDisassemble` (d3dcompiler_47) for `vs_3_0`/`ps_3_0`
//!
//! `disassemble(.., annotate = false)` reproduces Microsoft's text; `crosscheck-d3d` compares the
//! two. With `annotate = true` every instruction that reads a constant or sampler register gets a
//! trailing `// c3 = Name` comment with the CTAB name.

use std::fmt::Write;

use super::ctab::{Ctab, CtabConstant, ParamClass, RegisterSet};
use super::ir::*;
use crate::scan::Stage;

const COMPONENTS: [char; 4] = ['x', 'y', 'z', 'w'];
/// Indent of the version line and of top-level statements.
const BASE_INDENT: usize = 4;
/// Column width of the register table's name column is at least this wide (`Name` + padding).
const MIN_NAME_COLUMN: usize = 12;

/// Register name as written in the listing.
pub fn reg_name(reg: Reg, stage: Stage) -> String {
    let n = reg.num;
    match reg.ty {
        RegType::Temp => format!("r{n}"),
        RegType::Input => format!("v{n}"),
        RegType::Const => format!("c{n}"),
        RegType::Addr => {
            if stage == Stage::Vs {
                format!("a{n}")
            } else {
                format!("t{n}")
            }
        }
        RegType::RastOut => match n {
            0 => "oPos".into(),
            1 => "oFog".into(),
            2 => "oPts".into(),
            _ => format!("oRast{n}"),
        },
        RegType::AttrOut => format!("oD{n}"),
        RegType::Output => format!("o{n}"),
        RegType::ConstInt => format!("i{n}"),
        RegType::ColorOut => format!("oC{n}"),
        RegType::DepthOut => "oDepth".into(),
        RegType::Sampler => format!("s{n}"),
        RegType::Const2 => format!("c{}", n + 2048),
        RegType::Const3 => format!("c{}", n + 4096),
        RegType::Const4 => format!("c{}", n + 6144),
        RegType::ConstBool => format!("b{n}"),
        RegType::Loop => "aL".into(),
        RegType::TempFloat16 => format!("h{n}"),
        RegType::MiscType => match n {
            0 => "vPos".into(),
            1 => "vFace".into(),
            _ => format!("vMisc{n}"),
        },
        RegType::Label => format!("l{n}"),
        RegType::Predicate => format!("p{n}"),
    }
}

fn rel_text(rel: &Rel, stage: Stage) -> String {
    let base = reg_name(
        Reg {
            ty: rel.ty,
            num: rel.num,
        },
        stage,
    );
    if rel.ty == RegType::Loop {
        base
    } else {
        format!("{base}.{}", COMPONENTS[rel.comp as usize & 3])
    }
}

fn mask_text(mask: u8) -> String {
    if mask == 0 || mask == 0xF {
        return String::new();
    }
    let mut s = String::from(".");
    for (i, c) in COMPONENTS.iter().enumerate() {
        if mask & (1 << i) != 0 {
            s.push(*c);
        }
    }
    s
}

fn swizzle_text(sw: &[u8; 4]) -> String {
    if *sw == Src::IDENTITY {
        return String::new();
    }
    let c = |i: u8| COMPONENTS[i as usize & 3];
    if sw.iter().all(|&x| x == sw[0]) {
        return format!(".{}", c(sw[0]));
    }
    format!(".{}{}{}{}", c(sw[0]), c(sw[1]), c(sw[2]), c(sw[3]))
}

fn dst_text(d: &Dst, stage: Stage) -> String {
    let mut s = reg_name(d.reg, stage);
    if let Some(r) = &d.rel {
        let _ = write!(s, "[{}]", rel_text(r, stage));
    }
    s + &mask_text(d.mask)
}

fn src_text(s: &Src, stage: Stage) -> String {
    let mut name = reg_name(s.reg, stage);
    if let Some(r) = &s.rel {
        let _ = write!(name, "[{}]", rel_text(r, stage));
    }
    let (prefix, suffix) = match s.modifier {
        SrcMod::None => ("", ""),
        SrcMod::Neg => ("-", ""),
        SrcMod::Bias => ("", "_bias"),
        SrcMod::BiasNeg => ("-", "_bias"),
        SrcMod::Sign => ("", "_bx2"),
        SrcMod::SignNeg => ("-", "_bx2"),
        SrcMod::Comp => ("1-", ""),
        SrcMod::X2 => ("", "_x2"),
        SrcMod::X2Neg => ("-", "_x2"),
        SrcMod::Dz => ("", "_dz"),
        SrcMod::Dw => ("", "_dw"),
        SrcMod::Abs => ("", "_abs"),
        SrcMod::AbsNeg => ("-", "_abs"),
        SrcMod::Not => ("!", ""),
    };
    // The modifier suffix goes after the register and before the swizzle: `-r0_abs.w`.
    // A relative register keeps its index list last: `c4[a0.x]`.
    let (base, rel) = match name.find('[') {
        Some(i) => (name[..i].to_string(), name[i..].to_string()),
        None => (name, String::new()),
    };
    format!("{prefix}{base}{suffix}{rel}{}", swizzle_text(&s.swizzle))
}

/// `%.9g` the way the MSVC runtime prints it (three exponent digits, `1.#INF`).
pub fn fmt_float(v: f32) -> String {
    if v.is_nan() {
        return if v.is_sign_negative() {
            "-1.#IND".into()
        } else {
            "1.#QNAN".into()
        };
    }
    if v.is_infinite() {
        return if v < 0.0 {
            "-1.#INF".into()
        } else {
            "1.#INF".into()
        };
    }
    if v == 0.0 {
        return if v.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    const PRECISION: i32 = 9;
    let d = v as f64;
    // Nine significant digits, correctly rounded; the exponent is taken after rounding.
    let sci = format!("{:.*e}", (PRECISION - 1) as usize, d);
    let (mant, exp) = sci.split_once('e').expect("exponent form");
    let exp: i32 = exp.parse().expect("exponent");
    if !(-4..PRECISION).contains(&exp) {
        let mant = trim_zeros(mant);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mant}e{sign}{:03}", exp.abs())
    } else {
        let decimals = (PRECISION - 1 - exp) as usize;
        trim_zeros(&format!("{d:.decimals$}")).to_string()
    }
}

fn trim_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

fn mnemonic(i: &Inst) -> String {
    let mut s = match i.op {
        Opcode::TexLd => match i.control {
            TEXLD_PROJECT => "texldp".to_string(),
            TEXLD_BIAS => "texldb".to_string(),
            _ => "texld".to_string(),
        },
        Opcode::Ifc | Opcode::BreakC | Opcode::SetP => {
            let base = match i.op {
                Opcode::Ifc => "if",
                Opcode::BreakC => "break",
                _ => "setp",
            };
            format!("{base}_{}", i.cmp().map_or("??", Cmp::suffix))
        }
        op => op.name().to_string(),
    };
    if let Some(d) = &i.dst {
        s += &dst_mods(d);
    }
    s
}

fn dst_mods(d: &Dst) -> String {
    let mut s = String::new();
    if d.shift != 0 {
        let n = d.shift.unsigned_abs();
        let _ = write!(s, "_{}{}", if d.shift > 0 { 'x' } else { 'd' }, 1u32 << n);
    }
    if d.sat() {
        s += "_sat";
    }
    if d.pp() {
        s += "_pp";
    }
    if d.centroid() {
        s += "_centroid";
    }
    s
}

fn inst_text(i: &Inst, stage: Stage) -> String {
    let mut out = String::new();
    if let Some(p) = &i.pred {
        let _ = write!(out, "({}) ", src_text(p, stage));
    }
    out += &mnemonic(i);
    let mut ops: Vec<String> = Vec::new();
    if let Some(d) = &i.dst {
        ops.push(dst_text(d, stage));
    }
    for s in &i.src {
        ops.push(src_text(s, stage));
    }
    if !ops.is_empty() {
        out.push(' ');
        out += &ops.join(", ");
    }
    out
}

/// One instruction as the listing prints it, with the CTAB names after it (`// c3 = Name`).
/// The oracle puts it above the WGSL of the instruction.
pub fn inst_listing(i: &Inst, sh: &Shader) -> String {
    let mut text = inst_text(i, sh.version.stage);
    if let Some(ct) = &sh.ctab {
        let a = annotation(i, ct, sh.version.stage);
        if !a.is_empty() {
            let _ = write!(text, "  // {a}");
        }
    }
    text
}

fn dcl_text(d: &Dcl, stage: Stage) -> String {
    let mut s = String::from("dcl");
    match d.kind {
        DclKind::Semantic { usage, index } => {
            if let Some(u) = usage {
                s.push('_');
                s += u.name();
                if index != 0 {
                    let _ = write!(s, "{index}");
                }
            }
        }
        DclKind::Sampler(k) => {
            s += match k {
                SamplerKind::Tex1d => "_1d",
                SamplerKind::Tex2d => "_2d",
                SamplerKind::Cube => "_cube",
                SamplerKind::Volume => "_volume",
                SamplerKind::Unknown => "_unknown",
            };
        }
    }
    s += &dst_mods(&d.dst);
    s.push(' ');
    s += &dst_text(&d.dst, stage);
    s
}

/// Arithmetic and texture instruction slots (the "approximately" line).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slots {
    pub arithmetic: u32,
    pub texture: u32,
}

impl Slots {
    pub fn total(&self) -> u32 {
        self.arithmetic + self.texture
    }
}

/// Slots one instruction takes: (texture, arithmetic). Fitted to the 401 distinct blocks of the
/// pinned build (every count matches the compiler's own "approximately" line there); opcodes
/// that do not occur in those blocks cost one arithmetic slot, texture loads like `texld`.
fn inst_slots(op: Opcode) -> (u32, u32) {
    use Opcode::*;
    match op {
        TexLd | TexLdd => (1, 0),
        // A load with an explicit level is counted twice.
        TexLdl => (2, 0),
        Nop | Ret | Call | CallNz | Label | Loop | EndLoop => (0, 0),
        Dp2Add | Dsx | Dsy => (0, 2),
        Nrm | Pow => (0, 3),
        SinCos => (0, 8),
        // Flow control: if 2, else 1, endif 2, break 3, rep + endrep 5.
        If | Ifc | EndIf => (0, 2),
        Else => (0, 1),
        Break | BreakC | BreakP => (0, 3),
        Rep => (0, 2),
        EndRep => (0, 3),
        _ => (0, 1),
    }
}

/// Slot count of a shader as the compiler reports it.
pub fn slots(sh: &Shader) -> Slots {
    let mut s = Slots::default();
    for i in sh.insts() {
        let (t, a) = inst_slots(i.op);
        s.texture += t;
        s.arithmetic += a;
    }
    s
}

fn param_type_text(c: &CtabConstant) -> String {
    let t = &c.ty;
    let mut s = String::new();
    match t.class {
        ParamClass::Object => s += t.ty.name(),
        ParamClass::Struct => s += "struct",
        ParamClass::Scalar => s += t.ty.name(),
        ParamClass::Vector => {
            let _ = write!(s, "{}{}", t.ty.name(), t.columns);
        }
        ParamClass::MatrixRows | ParamClass::MatrixColumns => {
            if t.class == ParamClass::MatrixRows {
                s += "row_major ";
            }
            let _ = write!(s, "{}{}x{}", t.ty.name(), t.rows, t.columns);
        }
    }
    s
}

fn write_ctab_header(out: &mut String, ct: &Ctab) {
    out.push_str("//\n");
    if !ct.creator.is_empty() {
        let _ = writeln!(out, "// Generated by {}", ct.creator);
    }
    if ct.constants.is_empty() {
        return;
    }
    out.push_str("//\n// Parameters:\n//\n");
    for c in &ct.constants {
        let arr = if c.ty.elements > 1 {
            format!("[{}]", c.ty.elements)
        } else {
            String::new()
        };
        let _ = writeln!(out, "//   {} {}{arr};", param_type_text(c), c.name);
    }
    out.push_str("//\n//\n// Registers:\n//\n");
    let mut regs: Vec<&CtabConstant> = ct.constants.iter().collect();
    regs.sort_by_key(|c| (c.set, c.reg));
    let w = regs
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(0)
        .max(MIN_NAME_COLUMN);
    let _ = writeln!(out, "//   {:<w$} {:<5} Size", "Name", "Reg");
    let _ = writeln!(out, "//   {} ----- ----", "-".repeat(w));
    for c in regs {
        let reg = format!("{}{}", c.set.prefix(), c.reg);
        let _ = writeln!(out, "//   {:<w$} {:<5} {:>4}", c.name, reg, c.count);
    }
    out.push_str("//\n\n");
}

/// Constant and sampler registers an instruction reads, with their CTAB names.
fn annotation(i: &Inst, ct: &Ctab, stage: Stage) -> String {
    let mut seen: Vec<String> = Vec::new();
    for s in &i.src {
        let set = match s.reg.ty {
            RegType::Const => RegisterSet::Float4,
            RegType::Sampler => RegisterSet::Sampler,
            RegType::ConstInt => RegisterSet::Int4,
            RegType::ConstBool => RegisterSet::Bool,
            _ => continue,
        };
        if let Some(c) = ct.find(set, s.reg.num) {
            let name = if c.reg as u32 == s.reg.num {
                c.name.clone()
            } else {
                format!("{}+{}", c.name, s.reg.num - c.reg as u32)
            };
            let t = format!("{} = {name}", reg_name(s.reg, stage));
            if !seen.contains(&t) {
                seen.push(t);
            }
        }
    }
    seen.join(", ")
}

/// Normalisation for the comparison with `D3DDisassemble` (`crosscheck-d3d`): line ends are `\n`,
/// trailing blanks are dropped, trailing empty lines are dropped. Nothing else is normalised:
/// register names, operand order, number text and comments must be equal.
pub fn normalize(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end_matches(['\r', ' ', '\t', '\0']).to_string())
        .collect();
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines
}

/// First line where two listings differ (after [`normalize`]): 1-based line number, ours, theirs.
pub fn first_diff(ours: &str, theirs: &str) -> Option<(usize, String, String)> {
    let (a, b) = (normalize(ours), normalize(theirs));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i), b.get(i));
        if x != y {
            let show = |l: Option<&String>| l.cloned().unwrap_or_else(|| "<end>".to_string());
            return Some((i + 1, show(x), show(y)));
        }
    }
    None
}

/// The listing of a shader. `annotate` adds the CTAB names after the instructions.
pub fn disassemble(sh: &Shader, annotate: bool) -> String {
    let stage = sh.version.stage;
    let mut out = String::new();
    if let Some(ct) = &sh.ctab {
        write_ctab_header(&mut out, ct);
    }
    let _ = writeln!(out, "{:BASE_INDENT$}{}", "", sh.version.profile());
    let mut depth = 0usize;
    for st in &sh.stmts {
        let (indent, text) = match st {
            Stmt::Dcl(d) => (depth, dcl_text(d, stage)),
            Stmt::DefF { reg, value } => (
                depth,
                format!(
                    "def {}, {}, {}, {}, {}",
                    reg_name(*reg, stage),
                    fmt_float(value[0]),
                    fmt_float(value[1]),
                    fmt_float(value[2]),
                    fmt_float(value[3])
                ),
            ),
            Stmt::DefI { reg, value } => (
                depth,
                format!(
                    "defi {}, {}, {}, {}, {}",
                    reg_name(*reg, stage),
                    value[0],
                    value[1],
                    value[2],
                    value[3]
                ),
            ),
            Stmt::DefB { reg, value } => {
                (depth, format!("defb {}, {}", reg_name(*reg, stage), value))
            }
            Stmt::Inst(i) => {
                let mut text = inst_text(i, stage);
                let at = match i.op {
                    Opcode::EndIf | Opcode::EndRep | Opcode::EndLoop => {
                        depth = depth.saturating_sub(1);
                        depth
                    }
                    Opcode::Else => depth.saturating_sub(1),
                    _ => depth,
                };
                if matches!(i.op, Opcode::If | Opcode::Ifc | Opcode::Rep | Opcode::Loop) {
                    depth += 1;
                }
                if annotate && let Some(ct) = &sh.ctab {
                    let a = annotation(i, ct, stage);
                    if !a.is_empty() {
                        let _ = write!(text, "  // {a}");
                    }
                }
                (at, text)
            }
        };
        let _ = writeln!(out, "{:w$}{text}", "", w = BASE_INDENT + 2 * indent);
    }
    let sl = slots(sh);
    let _ = write!(
        out,
        "\n// approximately {} instruction slot{} used",
        sl.total(),
        if sl.total() == 1 { "" } else { "s" }
    );
    if sl.texture > 0 {
        let _ = write!(
            out,
            " ({} texture, {} arithmetic)",
            sl.texture, sl.arithmetic
        );
    }
    out.push('\n');
    out
}
