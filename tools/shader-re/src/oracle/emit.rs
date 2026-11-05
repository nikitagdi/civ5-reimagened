//! WGSL text of the oracle: one statement per SM3 instruction.
//!
//! Locals of the body start with `_` (`_r`, `_t5`, `_m2`); names that come from the block (the
//! sampler variables) never do, so they cannot be hidden.

use std::fmt::Write;

use super::flatten::{self, Flow};
use super::regs::{self, Analysis};
use super::{Chan, ConstBlock, FormatView, Options, Oracle, OracleError, Quantize};
use crate::scan::Stage;
use crate::sm3::Shader;
use crate::sm3::disasm;
use crate::sm3::ir::*;

const COMP: [char; 4] = ['x', 'y', 'z', 'w'];

/// `1.5f`, `1e-8f`: shortest text that reads back as the same f32.
pub fn f32_lit(v: f32) -> Result<String, String> {
    if !v.is_finite() {
        return Err(format!("the constant {v} is not finite"));
    }
    let s = format!("{v:?}");
    // `-0.0` and negative numbers are a unary minus on the literal.
    Ok(if let Some(rest) = s.strip_prefix('-') {
        format!("(-{rest}f)")
    } else {
        format!("{s}f")
    })
}

fn vec4_lit(v: &[f32; 4]) -> Result<String, String> {
    Ok(format!(
        "vec4<f32>({}, {}, {}, {})",
        f32_lit(v[0])?,
        f32_lit(v[1])?,
        f32_lit(v[2])?,
        f32_lit(v[3])?
    ))
}

fn swz(sw: &[u8; 4], n: usize) -> String {
    sw[..n].iter().map(|&c| COMP[c as usize & 3]).collect()
}

fn mask_swz(mask: u8) -> String {
    (0..4)
        .filter(|c| mask & (1 << c) != 0)
        .map(|c| COMP[c])
        .collect()
}

fn texture_type(k: SamplerKind) -> Option<&'static str> {
    match k {
        SamplerKind::Tex2d => Some("texture_2d<f32>"),
        SamplerKind::Cube => Some("texture_cube<f32>"),
        SamplerKind::Volume => Some("texture_3d<f32>"),
        SamplerKind::Tex1d | SamplerKind::Unknown => None,
    }
}

/// The body being written.
pub struct Emit<'a> {
    pub sh: &'a Shader,
    pub an: &'a Analysis,
    pub opts: &'a Options,
    pub flow: Flow,
    body: String,
    tmp: u32,
}

impl Emit<'_> {
    /// One line of the body, indented by the nesting.
    pub fn line(&mut self, s: impl AsRef<str>) {
        let depth = 1 + self.flow.depth();
        let _ = writeln!(self.body, "{:w$}{}", "", s.as_ref(), w = 4 * depth);
    }

    /// A fresh local name: `_t7`.
    pub fn tmp(&mut self, prefix: &str) -> String {
        self.tmp += 1;
        format!("_{prefix}{}", self.tmp)
    }

    /// `let _t = <expr>;` and the name.
    pub fn bind(&mut self, expr: &str) -> String {
        let t = self.tmp("t");
        self.line(format!("let {t} = {expr};"));
        t
    }

    /// Registers `Reg` as WGSL text: always a `vec4<f32>` (swizzle and modifiers come later).
    fn reg_expr(&self, s: &Src) -> Result<String, String> {
        let n = s.reg.num;
        let no_rel = |what: &str| {
            if s.rel.is_some() {
                Err(format!("relative addressing of {what}"))
            } else {
                Ok(())
            }
        };
        match s.reg.ty {
            RegType::Temp => {
                no_rel("r#")?;
                Ok(format!("_r[{n}]"))
            }
            RegType::Input => {
                no_rel("v#")?;
                self.an
                    .input(n)
                    .ok_or_else(|| format!("v{n} is read but has no dcl"))?;
                Ok(format!("inp.v{n}"))
            }
            RegType::Const => match &s.rel {
                None => Ok(if self.an.defs_f.contains_key(&n) {
                    format!("oracle_c{n}")
                } else {
                    format!("ora.c[{n}]")
                }),
                Some(rel) => {
                    if rel.ty != RegType::Addr || self.sh.version.stage != Stage::Vs {
                        return Err("relative addressing through aL or in a pixel shader".into());
                    }
                    Ok(format!(
                        "oracle_crel(_a0.{} + {n})",
                        COMP[rel.comp as usize & 3]
                    ))
                }
            },
            RegType::MiscType => {
                no_rel("vPos/vFace")?;
                match n {
                    0 if self.an.vpos() => Ok("inp.vpos".into()),
                    1 if self.an.vface() => Ok("vec4<f32>(inp.vface)".into()),
                    _ => Err(format!("vPos/vFace (n = {n}) is read but has no dcl")),
                }
            }
            other => Err(format!("source register of type {other:?}")),
        }
    }

    /// A source operand: its first `n` components (after the swizzle) with the modifier.
    /// `n` = 1 gives an `f32`, 2..=4 a vector.
    pub fn src(&self, s: &Src, n: usize) -> Result<String, String> {
        let base = self.reg_expr(s)?;
        let sw = if s.swizzle == Src::IDENTITY && n == 4 {
            String::new()
        } else {
            format!(".{}", swz(&s.swizzle, n))
        };
        let e = format!("{base}{sw}");
        Ok(match s.modifier {
            SrcMod::None => e,
            SrcMod::Neg => format!("(-{e})"),
            SrcMod::Abs => format!("abs({e})"),
            SrcMod::AbsNeg => format!("(-abs({e}))"),
            other => return Err(format!("source modifier {other:?}")),
        })
    }

    /// `i#` as `vec4<i32>` (an `rep` count or a comparison operand never needs more than `.x`).
    pub fn int_reg(&self, s: &Src) -> Result<String, String> {
        if s.reg.ty != RegType::ConstInt || s.rel.is_some() || s.modifier != SrcMod::None {
            return Err("integer operand that is not a plain i#".into());
        }
        let n = s.reg.num;
        if n >= regs::INT_REGS {
            return Err(format!("i{n} is out of range"));
        }
        Ok(if self.an.defs_i.contains_key(&n) {
            format!("oracle_i{n}")
        } else {
            format!("ora.i[{n}]")
        })
    }

    /// `b#` as a `bool`.
    pub fn bool_reg(&self, s: &Src) -> Result<String, String> {
        if s.reg.ty != RegType::ConstBool || s.rel.is_some() || s.modifier != SrcMod::None {
            return Err("boolean operand that is not a plain b#".into());
        }
        let n = s.reg.num;
        if n >= regs::BOOL_REGS {
            return Err(format!("b{n} is out of range"));
        }
        Ok(match self.an.defs_b.get(&n) {
            Some(v) => v.to_string(),
            None => format!("(((ora.b >> {n}u) & 1u) != 0u)"),
        })
    }

    /// Where a destination register lives in the body.
    fn dst_var(&self, d: &Dst) -> Result<String, String> {
        if d.rel.is_some() {
            return Err("relative addressing of a destination".into());
        }
        let n = d.reg.num;
        match d.reg.ty {
            RegType::Temp => Ok(format!("_r[{n}]")),
            RegType::Output => {
                if self.an.outputs.iter().any(|o| o.reg == n) {
                    Ok(format!("_o{n}"))
                } else {
                    Err(format!("o{n} is written but has no dcl"))
                }
            }
            RegType::ColorOut => Ok(format!("_oc{n}")),
            RegType::DepthOut => Ok("_od".into()),
            other => Err(format!("destination register of type {other:?}")),
        }
    }

    /// Applies shift and `_sat`, then writes the masked components under the execution mask.
    fn write(&mut self, d: &Dst, value: &str) -> Result<(), String> {
        let var = self.dst_var(d)?;
        if d.mask == 0 {
            return Ok(());
        }
        let mut v = value.to_string();
        if d.shift != 0 {
            let f = 2f32.powi(d.shift as i32);
            v = format!("({v} * {})", f32_lit(f)?);
        }
        if d.sat() {
            v = format!("saturate({v})");
        }
        let t = self.bind(&v);
        let assigns: Vec<String> = if d.mask == 0xF {
            vec![format!("{var} = {t};")]
        } else {
            (0..4)
                .filter(|c| d.mask & (1 << c) != 0)
                .map(|c| format!("{var}.{0} = {t}.{0};", COMP[c]))
                .collect()
        };
        self.guarded(&assigns);
        Ok(())
    }

    /// Statements that only run where the execution mask is set.
    pub fn guarded(&mut self, stmts: &[String]) {
        match self.flow.mask() {
            None => {
                for s in stmts {
                    self.line(s);
                }
            }
            Some(m) => {
                self.line(format!("if ({m}) {{"));
                for s in stmts {
                    self.line(format!("    {s}"));
                }
                self.line("}");
            }
        }
    }

    fn format_view(&mut self, name: &str, t: &str) -> String {
        let Some(FormatView { chans, quantize }) = self.opts.formats.get(name).copied() else {
            return t.to_string();
        };
        let q = match quantize {
            Quantize::None => t.to_string(),
            Quantize::Unorm16 => self.bind(&format!("round({t} * 65535.0) / 65535.0")),
        };
        let c = |ch: Chan| match ch {
            Chan::R => format!("{q}.x"),
            Chan::G => format!("{q}.y"),
            Chan::B => format!("{q}.z"),
            Chan::A => format!("{q}.w"),
            Chan::Zero => "0.0".into(),
            Chan::One => "1.0".into(),
        };
        format!(
            "vec4<f32>({}, {}, {}, {})",
            c(chans[0]),
            c(chans[1]),
            c(chans[2]),
            c(chans[3])
        )
    }

    fn tex_load(&mut self, i: &Inst) -> Result<String, String> {
        let [coord, samp] = i.src.as_slice() else {
            return Err("texture load without coordinate and sampler".into());
        };
        if samp.reg.ty != RegType::Sampler || samp.rel.is_some() {
            return Err("second operand is not a sampler register".into());
        }
        let res = self
            .an
            .sampler(samp.reg.num)
            .ok_or_else(|| format!("s{} is sampled but has no dcl", samp.reg.num))?
            .clone();
        let level_only = i.op == Opcode::TexLdl;
        if self.sh.version.stage == Stage::Vs && !level_only {
            return Err("texld in a vertex shader (no derivatives): only texldl".into());
        }
        let c = self.bind(&self.src(coord, 4)?);
        let (dims, project_dims) = match res.kind {
            SamplerKind::Tex2d => ("xy", "xy"),
            SamplerKind::Cube | SamplerKind::Volume => ("xyz", "xyz"),
            other => return Err(format!("sampler kind {other:?}")),
        };
        let (tex, smp) = (&res.texture_var, &res.sampler_var);
        let call = if level_only {
            format!("textureSampleLevel({tex}, {smp}, {c}.{dims}, {c}.w)")
        } else {
            match i.control {
                TEXLD_PLAIN => format!("textureSample({tex}, {smp}, {c}.{dims})"),
                TEXLD_PROJECT => {
                    format!("textureSample({tex}, {smp}, {c}.{project_dims} / {c}.w)")
                }
                TEXLD_BIAS => format!("textureSampleBias({tex}, {smp}, {c}.{dims}, {c}.w)"),
                other => return Err(format!("texld control {other}")),
            }
        };
        let t = self.bind(&call);
        let viewed = self.format_view(&res.ctab_name, &t);
        // The swizzle of the sampler operand picks components of the result.
        Ok(if samp.swizzle == Src::IDENTITY {
            viewed
        } else {
            format!("({viewed}).{}", swz(&samp.swizzle, 4))
        })
    }

    /// The value of an instruction as a `vec4<f32>` expression, or `None` for instructions that
    /// write by themselves (`mova`, `texkill`, control flow).
    fn value(&mut self, i: &Inst) -> Result<Option<String>, String> {
        let s = |me: &Self, k: usize, n: usize| -> Result<String, String> {
            me.src(
                i.src
                    .get(k)
                    .ok_or_else(|| format!("operand {k} is missing"))?,
                n,
            )
        };
        let splat = |e: String| format!("vec4<f32>({e})");
        use Opcode::*;
        Ok(Some(match i.op {
            Mov => s(self, 0, 4)?,
            Add => format!("({} + {})", s(self, 0, 4)?, s(self, 1, 4)?),
            Mul => format!("({} * {})", s(self, 0, 4)?, s(self, 1, 4)?),
            Mad => format!(
                "({} * {} + {})",
                s(self, 0, 4)?,
                s(self, 1, 4)?,
                s(self, 2, 4)?
            ),
            Min => format!("min({}, {})", s(self, 0, 4)?, s(self, 1, 4)?),
            Max => format!("max({}, {})", s(self, 0, 4)?, s(self, 1, 4)?),
            Abs => format!("abs({})", s(self, 0, 4)?),
            Frc => {
                let a = self.bind(&s(self, 0, 4)?);
                format!("({a} - floor({a}))")
            }
            Lrp => format!(
                "({0} * ({1} - {2}) + {2})",
                s(self, 0, 4)?,
                s(self, 1, 4)?,
                s(self, 2, 4)?
            ),
            Cmp => format!(
                "d3d_cmp({}, {}, {})",
                s(self, 0, 4)?,
                s(self, 1, 4)?,
                s(self, 2, 4)?
            ),
            Dp3 => splat(format!("dot({}, {})", s(self, 0, 3)?, s(self, 1, 3)?)),
            Dp4 => splat(format!("dot({}, {})", s(self, 0, 4)?, s(self, 1, 4)?)),
            Dp2Add => splat(format!(
                "dot({}, {}) + {}",
                s(self, 0, 2)?,
                s(self, 1, 2)?,
                s(self, 2, 1)?
            )),
            Rcp => splat(format!("d3d_rcp({})", s(self, 0, 1)?)),
            Rsq => splat(format!("d3d_rsq({})", s(self, 0, 1)?)),
            Exp => splat(format!("exp2({})", s(self, 0, 1)?)),
            Log => splat(format!("d3d_log({})", s(self, 0, 1)?)),
            Pow => splat(format!("d3d_pow({}, {})", s(self, 0, 1)?, s(self, 1, 1)?)),
            Nrm => format!("d3d_nrm({})", s(self, 0, 4)?),
            SinCos => {
                if i.src.len() != 1 {
                    return Err("sincos with the SM2 helper registers".into());
                }
                let a = self.bind(&s(self, 0, 1)?);
                format!("vec4<f32>(cos({a}), sin({a}), 0.0, 0.0)")
            }
            Dsx | Dsy => {
                if self.sh.version.stage != Stage::Ps {
                    return Err("derivative in a vertex shader".into());
                }
                let f = if i.op == Dsx { "dpdx" } else { "dpdy" };
                format!("{f}({})", s(self, 0, 4)?)
            }
            TexLd | TexLdl => self.tex_load(i)?,
            _ => return Ok(None),
        }))
    }

    fn inst(&mut self, i: &Inst) -> Result<(), String> {
        if i.pred.is_some() {
            return Err("predicated instruction".into());
        }
        use Opcode::*;
        match i.op {
            Nop => Ok(()),
            If | Ifc | Else | EndIf | Rep | EndRep | Break | BreakC => flatten::handle(self, i),
            Mova => {
                let d = i.dst.as_ref().ok_or("mova without destination")?;
                if d.reg.ty != RegType::Addr || d.reg.num != 0 || d.rel.is_some() {
                    return Err("mova to something other than a0".into());
                }
                if self.sh.version.stage != Stage::Vs {
                    return Err("mova in a pixel shader".into());
                }
                let src = i.src.first().ok_or("mova without source")?;
                let v = self.src(src, 4)?;
                let t = self.bind(&format!("floor({v} + vec4<f32>(0.5))"));
                let assigns: Vec<String> = (0..4)
                    .filter(|c| d.mask & (1 << c) != 0)
                    .map(|c| format!("_a0.{0} = i32({t}.{0});", COMP[c]))
                    .collect();
                self.guarded(&assigns);
                Ok(())
            }
            TexKill => {
                let d = i.dst.as_ref().ok_or("texkill without operand")?;
                if self.sh.version.stage != Stage::Ps {
                    return Err("texkill in a vertex shader".into());
                }
                if d.reg.ty != RegType::Temp || d.rel.is_some() {
                    return Err("texkill of something other than r#".into());
                }
                let comps = mask_swz(d.mask);
                let test = match comps.len() {
                    0 => return Ok(()),
                    1 => format!("_r[{}].{comps} < 0.0", d.reg.num),
                    n => format!("any(_r[{}].{comps} < vec{n}<f32>(0.0))", d.reg.num),
                };
                let t = self.bind(&test);
                self.guarded(&[format!("_kill = _kill || {t};")]);
                Ok(())
            }
            _ => {
                let Some(v) = self.value(i)? else {
                    return Err("opcode is not in the table of the oracle".into());
                };
                let d = i.dst.as_ref().ok_or("instruction without destination")?;
                self.write(d, &v)
            }
        }
    }
}

fn unsupported(label: &str, index: usize, sh: &Shader, i: &Inst, what: String) -> OracleError {
    OracleError::Unsupported {
        block: label.to_string(),
        index,
        text: disasm::inst_listing(i, sh),
        what,
    }
}

/// The whole module.
pub fn emit(
    sh: &Shader,
    name: &str,
    sha1: &str,
    label: &str,
    an: &Analysis,
    opts: &Options,
) -> Result<Oracle, OracleError> {
    let invalid = |what: String| OracleError::Invalid {
        block: label.to_string(),
        what,
    };
    let stage = sh.version.stage;
    let mut e = Emit {
        sh,
        an,
        opts,
        flow: Flow::default(),
        body: String::new(),
        tmp: 0,
    };

    // ---- body -------------------------------------------------------------------------------
    for (index, st) in sh
        .stmts
        .iter()
        .enumerate()
        .filter(|(_, s)| matches!(s, Stmt::Inst(_)))
    {
        let Stmt::Inst(i) = st else { continue };
        let text = disasm::inst_listing(i, sh);
        e.line(format!("// {index}: {text}"));
        e.inst(i)
            .map_err(|what| unsupported(label, index, sh, i, what))?;
    }
    if let Some(open) = e.flow.open() {
        return Err(invalid(format!(
            "{open} is not closed at the end of the block"
        )));
    }
    let body = std::mem::take(&mut e.body);

    // ---- module -----------------------------------------------------------------------------
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// oracle: {name} sha1 {sha1} -- not for the product\n// {} from the original, one WGSL statement per instruction",
        sh.version.profile()
    );
    out.push_str(super::HELPERS);

    for (n, v) in &an.defs_f {
        let _ = writeln!(
            out,
            "const oracle_c{n} = {};",
            vec4_lit(v).map_err(|w| invalid(format!("def c{n}: {w}")))?
        );
    }
    for (n, v) in &an.defs_i {
        let _ = writeln!(
            out,
            "const oracle_i{n} = vec4<i32>({}, {}, {}, {});",
            v[0], v[1], v[2], v[3]
        );
    }

    // Constant block (SR-D5).
    let int_regs = if an.uses_int { regs::INT_REGS } else { 0 };
    let consts = if an.float_regs > 0 || an.uses_int || an.uses_bool {
        out.push_str("\nstruct OracleConsts {\n");
        let mut members = Vec::new();
        if an.float_regs > 0 {
            members.push(format!("    c: array<vec4<f32>, {}>,", an.float_regs));
        }
        if an.uses_int {
            members.push(format!("    i: array<vec4<i32>, {}>,", regs::INT_REGS));
        }
        if an.uses_bool {
            members.push("    b: u32,".to_string());
        }
        out.push_str(&members.join("\n"));
        out.push_str("\n}\n");
        let _ = writeln!(
            out,
            "@group({}) @binding({}) var<uniform> ora: OracleConsts;",
            regs::CONST_GROUP,
            regs::CONST_BINDING
        );
        Some(ConstBlock {
            group: regs::CONST_GROUP,
            binding: regs::CONST_BINDING,
            float_regs: an.float_regs,
            int_regs,
            bool_mask: an.uses_bool,
        })
    } else {
        None
    };

    // Samplers: a texture and a sampler per `s#`, named by the CTAB constant.
    out.push('\n');
    for s in &an.samplers {
        let ty = texture_type(s.kind)
            .ok_or_else(|| invalid(format!("dcl for s{} has the kind {:?}", s.reg, s.kind)))?;
        let _ = writeln!(
            out,
            "@group({}) @binding({}) var {}: {ty}; // s{}",
            s.group, s.texture_binding, s.texture_var, s.reg
        );
        let _ = writeln!(
            out,
            "@group({}) @binding({}) var {}: sampler;",
            s.group, s.sampler_binding, s.sampler_var
        );
    }

    // Relative reads of the constant array: clamped into the array, `def`s laid over it.
    if an.rel_const {
        let n = an.float_regs;
        if n == 0 {
            return Err(invalid(
                "relative constant read without float constants".into(),
            ));
        }
        out.push_str("\nfn oracle_crel(i: i32) -> vec4<f32> {\n");
        let _ = writeln!(out, "    var v = ora.c[clamp(i, 0, {})];", n - 1);
        for (reg, val) in &an.defs_f {
            let _ = writeln!(out, "    if (i == {reg}) {{ v = oracle_c{reg}; }}");
            let _ = val;
        }
        out.push_str("    return v;\n}\n");
    }

    // Body interface: one vec4 per register.
    let mut in_members: Vec<String> = Vec::new();
    for i in &an.inputs {
        match i.usage {
            Some(_) => in_members.push(format!("v{}: vec4<f32>", i.reg)),
            None if i.reg == 0 => in_members.push("vpos: vec4<f32>".into()),
            None => in_members.push("vface: f32".into()),
        }
    }
    let in_empty = in_members.is_empty();
    if in_empty {
        in_members.push("pad: f32".into());
    }
    let _ = writeln!(
        out,
        "\nstruct OracleIn {{\n    {},\n}}",
        in_members.join(",\n    ")
    );
    let mut out_members: Vec<String> = Vec::new();
    match stage {
        Stage::Ps => {
            for n in &an.oc {
                out_members.push(format!("oc{n}: vec4<f32>"));
            }
            if an.depth {
                out_members.push("odepth: f32".into());
            }
            out_members.push("kill: bool".into());
        }
        _ => {
            for o in &an.outputs {
                out_members.push(format!("o{}: vec4<f32>", o.reg));
            }
        }
    }
    if out_members.is_empty() {
        return Err(invalid("the block writes no output".into()));
    }
    let _ = writeln!(
        out,
        "struct OracleOut {{\n    {},\n}}",
        out_members.join(",\n    ")
    );

    // The body.
    let _ = writeln!(out, "\nfn oracle_body(inp: OracleIn) -> OracleOut {{");
    if an.temps > 0 {
        let _ = writeln!(out, "    var _r: array<vec4<f32>, {}>;", an.temps);
    }
    if an.a0 {
        out.push_str("    var _a0 = vec4<i32>(0);\n");
    }
    match stage {
        Stage::Ps => {
            for n in &an.oc {
                let _ = writeln!(out, "    var _oc{n} = vec4<f32>(0.0);");
            }
            if an.depth {
                out.push_str("    var _od = vec4<f32>(0.0);\n");
            }
            out.push_str("    var _kill = false;\n");
        }
        _ => {
            for o in &an.outputs {
                let _ = writeln!(out, "    var _o{} = vec4<f32>(0.0);", o.reg);
            }
        }
    }
    out.push_str(&body);
    out.push_str("    var _res: OracleOut;\n");
    match stage {
        Stage::Ps => {
            for n in &an.oc {
                let _ = writeln!(out, "    _res.oc{n} = _oc{n};");
            }
            if an.depth {
                out.push_str("    _res.odepth = _od.x;\n");
            }
            out.push_str("    _res.kill = _kill;\n");
        }
        _ => {
            for o in &an.outputs {
                let _ = writeln!(out, "    _res.o{0} = _o{0};", o.reg);
            }
        }
    }
    out.push_str("    return _res;\n}\n");

    // Entry point.
    let (entry_point, body_call_in) = (
        if stage == Stage::Vs {
            "oracle_vs"
        } else {
            "oracle_fs"
        },
        "_b",
    );
    let fill_in = |from: &str| -> String {
        let mut s = String::from("    var _b: OracleIn;\n");
        if in_empty {
            s.push_str("    _b.pad = 0.0;\n");
        }
        for i in &an.inputs {
            if i.usage.is_some() {
                let _ = writeln!(s, "    _b.v{0} = {from}.v{0};", i.reg);
            }
        }
        s
    };
    match stage {
        Stage::Ps => {
            let mut members = vec!["@builtin(position) pos: vec4<f32>".to_string()];
            if an.vface() {
                members.push("@builtin(front_facing) face: bool".into());
            }
            for i in an.inputs.iter().filter(|i| i.usage.is_some()) {
                let interp = if i.centroid {
                    " @interpolate(perspective, centroid)"
                } else {
                    ""
                };
                members.push(format!(
                    "@location({}){interp} v{}: vec4<f32>",
                    i.location.unwrap_or(i.reg),
                    i.reg
                ));
            }
            let _ = writeln!(
                out,
                "\nstruct OracleFsIn {{\n    {},\n}}",
                members.join(",\n    ")
            );
            let mut fo: Vec<String> = an
                .oc
                .iter()
                .map(|n| format!("@location({n}) oc{n}: vec4<f32>"))
                .collect();
            if an.depth {
                fo.push("@builtin(frag_depth) odepth: f32".into());
            }
            let ret = if fo.is_empty() {
                String::new()
            } else {
                let _ = writeln!(
                    out,
                    "struct OracleFsOut {{\n    {},\n}}",
                    fo.join(",\n    ")
                );
                " -> OracleFsOut".to_string()
            };
            let _ = writeln!(out, "\n@fragment\nfn oracle_fs(f: OracleFsIn){ret} {{");
            out.push_str(&fill_in("f"));
            if an.vpos() {
                // D3D9 vPos is the pixel position with integer pixel centres (4.6, `dcl_*`).
                out.push_str("    _b.vpos = vec4<f32>(f.pos.xy - vec2<f32>(0.5), 0.0, 0.0);\n");
            }
            if an.vface() {
                out.push_str("    _b.vface = select(-1.0, 1.0, f.face);\n");
            }
            let _ = writeln!(out, "    let _r = oracle_body({body_call_in});");
            out.push_str("    if (_r.kill) { discard; }\n");
            if !ret.is_empty() {
                out.push_str("    var _o: OracleFsOut;\n");
                for n in &an.oc {
                    let _ = writeln!(out, "    _o.oc{n} = _r.oc{n};");
                }
                if an.depth {
                    out.push_str("    _o.odepth = _r.odepth;\n");
                }
                out.push_str("    return _o;\n");
            }
            out.push_str("}\n");
        }
        _ => {
            let args: Vec<String> = an
                .inputs
                .iter()
                .filter(|i| i.usage.is_some())
                .map(|i| format!("@location({}) v{}: vec4<f32>", i.reg, i.reg))
                .collect();
            let mut vo: Vec<String> = Vec::new();
            let mut pos = None;
            for o in &an.outputs {
                match o.location {
                    None => {
                        vo.push("@builtin(position) pos: vec4<f32>".to_string());
                        pos = Some(o.reg);
                    }
                    Some(l) => vo.push(format!("@location({l}) o{}: vec4<f32>", o.reg)),
                }
            }
            let Some(pos) = pos else {
                return Err(invalid("vertex shader without dcl_position output".into()));
            };
            let _ = writeln!(
                out,
                "\nstruct OracleVsOut {{\n    {},\n}}",
                vo.join(",\n    ")
            );
            let arg = if args.is_empty() {
                String::new()
            } else {
                // Attributes arrive as arguments of the entry point.
                args.join(", ")
            };
            let _ = writeln!(out, "\n@vertex\nfn oracle_vs({arg}) -> OracleVsOut {{");
            let mut s = String::from("    var _b: OracleIn;\n");
            if in_empty {
                s.push_str("    _b.pad = 0.0;\n");
            }
            for i in an.inputs.iter().filter(|i| i.usage.is_some()) {
                let _ = writeln!(s, "    _b.v{0} = v{0};", i.reg);
            }
            out.push_str(&s);
            let _ = writeln!(out, "    let _r = oracle_body({body_call_in});");
            out.push_str("    var _o: OracleVsOut;\n");
            for o in &an.outputs {
                if o.reg == pos && o.location.is_none() {
                    let _ = writeln!(out, "    _o.pos = _r.o{};", o.reg);
                } else {
                    let _ = writeln!(out, "    _o.o{0} = _r.o{0};", o.reg);
                }
            }
            out.push_str("    return _o;\n}\n");
        }
    }

    Ok(Oracle {
        name: name.to_string(),
        sha1: sha1.to_string(),
        stage,
        wgsl: out,
        entry_point: entry_point.to_string(),
        body: "oracle_body".to_string(),
        consts,
        samplers: an.samplers.clone(),
        inputs: an.inputs.clone(),
        outputs: if stage == Stage::Vs {
            an.outputs.clone()
        } else {
            an.oc
                .iter()
                .map(|&n| super::IoReg {
                    reg: n,
                    usage: None,
                    index: 0,
                    mask: 0xF,
                    centroid: false,
                    location: Some(n),
                })
                .collect()
        },
        depth_out: an.depth,
        texkill: an.texkill,
        float_names: an.float_names.clone(),
        ctab: sh.ctab.clone(),
    })
}
