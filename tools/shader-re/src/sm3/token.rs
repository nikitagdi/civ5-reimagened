//! SM3 token stream parser. Token layout: DirectX 9 SDK, d3d9types.h
//! ("Shader Code" section).

use super::ctab::{self, CTAB_TAG, CtabError};
use super::ir::*;
use crate::scan::Stage;

/// `D3DSIO_COMMENT`: opcode (low 16 bits); length in dwords in bits 16-30.
pub const OP_COMMENT: u32 = 0xFFFE;
/// `D3DSIO_END`.
pub const OP_END: u32 = 0xFFFF;
/// Highest opcode of the SM3 instruction set (`D3DSIO_BREAKP`).
pub const OP_MAX: u32 = 0x60;
const LEN_SHIFT: u32 = 24;
const LEN_MASK: u32 = 0xF;
const CONTROL_SHIFT: u32 = 16;
const CONTROL_MASK: u32 = 0xFF;
/// `D3DSHADER_INSTRUCTION_PREDICATED`.
const PREDICATED: u32 = 1 << 28;
/// Bit 31 of every parameter token is set (`D3DSP_REGTYPE`/`D3DSP_RESERVED` high bit).
const PARAM_BIT: u32 = 1 << 31;
const REG_NUM_MASK: u32 = 0x7FF;
const REGTYPE_SHIFT: u32 = 28;
const REGTYPE_MASK: u32 = 0x7000_0000;
const REGTYPE_SHIFT2: u32 = 8;
const REGTYPE_MASK2: u32 = 0x1800;
/// `D3DSHADER_ADDRESSMODE_RELATIVE`.
const ADDR_RELATIVE: u32 = 1 << 13;
const WRITEMASK_SHIFT: u32 = 16;
const DSTMOD_SHIFT: u32 = 20;
const DSTSHIFT_SHIFT: u32 = 24;
const SWIZZLE_SHIFT: u32 = 16;
const SRCMOD_SHIFT: u32 = 24;
/// `dcl` first token: usage in bits 0-4, usage index in bits 16-19, sampler type in bits 27-30.
const DCL_USAGE_MASK: u32 = 0x1F;
const DCL_INDEX_SHIFT: u32 = 16;
const DCL_INDEX_MASK: u32 = 0xF;
const DCL_TEXTURETYPE_SHIFT: u32 = 27;
const DCL_TEXTURETYPE_MASK: u32 = 0xF;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Sm3Error {
    #[error("size {0} is not a multiple of 4")]
    BadSize(usize),
    #[error("token stream ends at dword {0} before the END token")]
    Truncated(usize),
    #[error("bad version token {0:#010x}")]
    BadVersion(u32),
    #[error("dword {at}: opcode {op:#06x} is not an SM2/SM3 opcode")]
    BadOpcode { at: usize, op: u32 },
    #[error("dword {at}: {what}")]
    BadOperand { at: usize, what: &'static str },
    #[error("dword {at}: {op} has {got} operand dwords, expected {want}")]
    OperandCount {
        at: usize,
        op: &'static str,
        got: usize,
        want: String,
    },
    #[error("{0} dwords after the END token")]
    TrailingData(usize),
    #[error(transparent)]
    Ctab(#[from] CtabError),
}

/// What the operands of an opcode look like.
struct Shape {
    dst: bool,
    /// Exact source count; `None` = the rest of the instruction.
    srcs: Option<usize>,
}

fn shape(op: Opcode) -> Shape {
    use Opcode::*;
    let (dst, srcs) = match op {
        Nop | Ret | EndLoop | Else | EndIf | Break | EndRep => (false, Some(0)),
        Mov | Rcp | Rsq | Exp | Log | Lit | Frc | Abs | Sgn | Nrm | ExpP | LogP | Mova | Dsx
        | Dsy => (true, Some(1)),
        Add | Sub | Mul | Dp3 | Dp4 | Min | Max | Slt | Sge | Dst | Crs | Pow | M4x4 | M4x3
        | M3x4 | M3x3 | M3x2 | TexLd | TexLdl => (true, Some(2)),
        Mad | Lrp | Cmp | Dp2Add => (true, Some(3)),
        TexLdd => (true, Some(4)),
        SetP => (true, Some(2)),
        // SM3 `sincos dst, src`; SM2 had two more registers.
        SinCos => (true, None),
        TexKill => (true, Some(0)),
        If | Rep | Call | Label | BreakP => (false, Some(1)),
        Ifc | BreakC | CallNz | Loop => (false, Some(2)),
        Dcl | Def | DefI | DefB => (true, None),
    };
    Shape { dst, srcs }
}

struct Cursor<'a> {
    w: &'a [u32],
    pos: usize,
    end: usize,
}

impl Cursor<'_> {
    fn next(&mut self, what: &'static str) -> Result<u32, Sm3Error> {
        if self.pos >= self.end {
            return Err(Sm3Error::BadOperand { at: self.pos, what });
        }
        let t = self.w[self.pos];
        self.pos += 1;
        Ok(t)
    }
}

fn reg_of(tok: u32, at: usize) -> Result<Reg, Sm3Error> {
    let code = ((tok & REGTYPE_MASK) >> REGTYPE_SHIFT) | ((tok & REGTYPE_MASK2) >> REGTYPE_SHIFT2);
    let ty = RegType::from_code(code).ok_or(Sm3Error::BadOperand {
        at,
        what: "unknown register type",
    })?;
    Ok(Reg {
        ty,
        num: tok & REG_NUM_MASK,
    })
}

fn param_token(c: &mut Cursor, what: &'static str) -> Result<(u32, usize), Sm3Error> {
    let at = c.pos;
    let t = c.next(what)?;
    if t & PARAM_BIT == 0 {
        return Err(Sm3Error::BadOperand {
            at,
            what: "parameter token without bit 31",
        });
    }
    Ok((t, at))
}

fn read_rel(c: &mut Cursor) -> Result<Rel, Sm3Error> {
    let (t, at) = param_token(c, "relative address token missing")?;
    let r = reg_of(t, at)?;
    if r.ty != RegType::Addr && r.ty != RegType::Loop {
        return Err(Sm3Error::BadOperand {
            at,
            what: "relative address register is not a0/aL",
        });
    }
    Ok(Rel {
        ty: r.ty,
        num: r.num,
        comp: ((t >> SWIZZLE_SHIFT) & 3) as u8,
    })
}

fn read_dst(c: &mut Cursor) -> Result<Dst, Sm3Error> {
    let (t, at) = param_token(c, "destination token missing")?;
    let reg = reg_of(t, at)?;
    let rel = if t & ADDR_RELATIVE != 0 {
        Some(read_rel(c)?)
    } else {
        None
    };
    let shift = (((t >> DSTSHIFT_SHIFT) & 0xF) as i8) << 4 >> 4;
    Ok(Dst {
        reg,
        rel,
        mask: ((t >> WRITEMASK_SHIFT) & 0xF) as u8,
        mods: ((t >> DSTMOD_SHIFT) & 0xF) as u8,
        shift,
    })
}

fn read_src(c: &mut Cursor) -> Result<Src, Sm3Error> {
    let (t, at) = param_token(c, "source token missing")?;
    let reg = reg_of(t, at)?;
    let rel = if t & ADDR_RELATIVE != 0 {
        Some(read_rel(c)?)
    } else {
        None
    };
    let sw = t >> SWIZZLE_SHIFT;
    let modifier = SrcMod::from_code((t >> SRCMOD_SHIFT) & 0xF).ok_or(Sm3Error::BadOperand {
        at,
        what: "unknown source modifier",
    })?;
    Ok(Src {
        reg,
        rel,
        swizzle: [
            (sw & 3) as u8,
            ((sw >> 2) & 3) as u8,
            ((sw >> 4) & 3) as u8,
            ((sw >> 6) & 3) as u8,
        ],
        modifier,
    })
}

fn float(t: u32) -> f32 {
    f32::from_bits(t)
}

/// Parses an SM2/SM3 block (`vs_3_0`, `ps_3_0`) from its bytes.
pub fn parse(bytes: &[u8]) -> Result<Shader, Sm3Error> {
    if !bytes.len().is_multiple_of(4) {
        return Err(Sm3Error::BadSize(bytes.len()));
    }
    let w: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let ver = *w.first().ok_or(Sm3Error::Truncated(0))?;
    let stage = match ver >> 16 {
        0xFFFE => Stage::Vs,
        0xFFFF => Stage::Ps,
        _ => return Err(Sm3Error::BadVersion(ver)),
    };
    let version = Version {
        stage,
        major: ((ver >> 8) & 0xFF) as u8,
        minor: (ver & 0xFF) as u8,
    };
    let mut sh = Shader {
        version,
        ctab: None,
        comments: Vec::new(),
        stmts: Vec::new(),
    };
    let mut pos = 1usize;
    loop {
        let at = pos;
        let tok = *w.get(pos).ok_or(Sm3Error::Truncated(pos))?;
        pos += 1;
        let opc = tok & 0xFFFF;
        if opc == OP_COMMENT {
            let len = ((tok >> 16) & 0x7FFF) as usize;
            let body = w.get(pos..pos + len).ok_or(Sm3Error::Truncated(w.len()))?;
            pos += len;
            let data: Vec<u8> = body.iter().flat_map(|x| x.to_le_bytes()).collect();
            if body.first() == Some(&CTAB_TAG) && sh.ctab.is_none() {
                sh.ctab = Some(ctab::parse(&data[4..])?);
            } else {
                sh.comments.push(Comment { data });
            }
            continue;
        }
        if opc == OP_END {
            break;
        }
        if opc > OP_MAX || tok & PARAM_BIT != 0 {
            return Err(Sm3Error::BadOpcode { at, op: opc });
        }
        let op = Opcode::from_code(opc as u16).ok_or(Sm3Error::BadOpcode { at, op: opc })?;
        let len = ((tok >> LEN_SHIFT) & LEN_MASK) as usize;
        // `def` has four dwords after the register, which is more than the 4-bit field when
        // predicated is not set: the field is 5 for `def`, 5 for `defi`, 2 for `defb`; all fit.
        let end = pos + len;
        if end > w.len() {
            return Err(Sm3Error::Truncated(w.len()));
        }
        let control = ((tok >> CONTROL_SHIFT) & CONTROL_MASK) as u8;
        let mut c = Cursor { w: &w, pos, end };
        let stmt = parse_inst(op, tok, control, &mut c, at, len)?;
        if c.pos != end {
            return Err(Sm3Error::OperandCount {
                at,
                op: op.name(),
                got: len,
                want: format!("{} (consumed)", c.pos - pos),
            });
        }
        sh.stmts.push(stmt);
        pos = end;
    }
    if pos != w.len() {
        return Err(Sm3Error::TrailingData(w.len() - pos));
    }
    Ok(sh)
}

fn parse_inst(
    op: Opcode,
    tok: u32,
    control: u8,
    c: &mut Cursor,
    at: usize,
    len: usize,
) -> Result<Stmt, Sm3Error> {
    match op {
        Opcode::Dcl => {
            let first = c.next("dcl token missing")?;
            let dst = read_dst(c)?;
            let tex = (first >> DCL_TEXTURETYPE_SHIFT) & DCL_TEXTURETYPE_MASK;
            let kind = if dst.reg.ty == RegType::Sampler {
                DclKind::Sampler(SamplerKind::from_code(tex))
            } else if dst.reg.ty == RegType::MiscType {
                DclKind::Semantic {
                    usage: None,
                    index: 0,
                }
            } else {
                let u = first & DCL_USAGE_MASK;
                DclKind::Semantic {
                    usage: Some(Usage::from_code(u).ok_or(Sm3Error::BadOperand {
                        at,
                        what: "unknown dcl usage",
                    })?),
                    index: ((first >> DCL_INDEX_SHIFT) & DCL_INDEX_MASK) as u8,
                }
            };
            Ok(Stmt::Dcl(Dcl { kind, dst }))
        }
        Opcode::Def => {
            let (t, a) = param_token(c, "def register missing")?;
            let reg = reg_of(t, a)?;
            let mut value = [0f32; 4];
            for v in &mut value {
                *v = float(c.next("def value missing")?);
            }
            Ok(Stmt::DefF { reg, value })
        }
        Opcode::DefI => {
            let (t, a) = param_token(c, "defi register missing")?;
            let reg = reg_of(t, a)?;
            let mut value = [0i32; 4];
            for v in &mut value {
                *v = c.next("defi value missing")? as i32;
            }
            Ok(Stmt::DefI { reg, value })
        }
        Opcode::DefB => {
            let (t, a) = param_token(c, "defb register missing")?;
            let reg = reg_of(t, a)?;
            let value = c.next("defb value missing")? & 1 != 0;
            Ok(Stmt::DefB { reg, value })
        }
        _ => {
            let sh = shape(op);
            let predicated = tok & PREDICATED != 0;
            // The predicate token is the last operand of the instruction.
            let body_end = c.end - usize::from(predicated);
            if predicated && c.end == c.pos {
                return Err(Sm3Error::OperandCount {
                    at,
                    op: op.name(),
                    got: len,
                    want: "a predicate token".into(),
                });
            }
            let full_end = c.end;
            c.end = body_end;
            let dst = if sh.dst { Some(read_dst(c)?) } else { None };
            let mut src = Vec::new();
            match sh.srcs {
                Some(n) => {
                    for _ in 0..n {
                        src.push(read_src(c)?);
                    }
                }
                None => {
                    while c.pos < c.end {
                        src.push(read_src(c)?);
                    }
                }
            }
            let pred = if predicated {
                c.end = full_end;
                Some(read_src(c)?)
            } else {
                None
            };
            if matches!(op, Opcode::Ifc | Opcode::BreakC | Opcode::SetP)
                && Cmp::from_code(control).is_none()
            {
                return Err(Sm3Error::BadOperand {
                    at,
                    what: "comparison code is not 1..=6",
                });
            }
            if op == Opcode::TexLd && control > TEXLD_BIAS {
                return Err(Sm3Error::BadOperand {
                    at,
                    what: "texld control is not 0..=2",
                });
            }
            Ok(Stmt::Inst(Inst {
                op,
                control,
                dst,
                src,
                pred,
            }))
        }
    }
}
